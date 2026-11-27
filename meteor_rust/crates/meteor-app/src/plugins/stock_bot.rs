use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use meteor_core::FighterState;
use serde_json::{json, Value};

use crate::app_state::{AppState, InputBinding, Keymap};
use crate::components::fighter::{
    Fighter, Health, LockOnController, MotionState, PoseRuntime, Qi, SmState,
};
use crate::plugins::camera_plugin::{bot_look_degrees, BotLookDelta};
use crate::plugins::combat_plugin::NpcFighter;
use crate::plugins::input_plugin::SyntheticInputStream;
use crate::plugins::movement_plugin::FighterAir;
use crate::plugins::network_plugin::{
    NetState, NetStatus, RemoteAuthorityGeneration, RemoteFighter,
};
use crate::plugins::stage_plugin::LocalFighter;

pub(crate) const DAGGER: u8 = 5;
const WINDOW: Duration = Duration::from_millis(33);
const WATCHDOG: Duration = Duration::from_millis(350);
const RETRY: Duration = Duration::from_millis(500);
const MAX_EDGES: usize = 8;

#[derive(Resource, Debug, Clone)]
pub struct StockBotConfig {
    pub python: PathBuf,
    pub script: PathBuf,
    pub policy: PathBuf,
    pub stage_id: String,
    pub room_name: String,
    pub player_name: String,
    pub character_id: u8,
    pub weapon: u8,
    pub port: u16,
    pub log_dir: PathBuf,
}

impl StockBotConfig {
    pub fn validate(&self) -> Result<(), String> {
        for (flag, path) in [
            ("--bot-python", &self.python),
            ("--bot-script", &self.script),
            ("--bot-policy", &self.policy),
        ] {
            if !path.is_file() {
                return Err(format!(
                    "{flag} does not point to a file: {}",
                    path.display()
                ));
            }
        }
        if !meteor_scene::stage_registry::is_official_stage_id(&self.stage_id) {
            return Err(format!(
                "--bot-stage is not an original stage: {}",
                self.stage_id
            ));
        }
        if !meteor_network::modes::original_stage_supports_rule(&self.stage_id, 1) {
            return Err(format!(
                "--bot-stage does not support Rule 1: {}",
                self.stage_id
            ));
        }
        if self.character_id > 19 {
            return Err("--bot-character must be 0..19".into());
        }
        if self.weapon != DAGGER {
            return Err("the trained policy only drives dagger (weapon 5) rooms".into());
        }
        if self.port == 0 {
            return Err("--bot-port must be non-zero".into());
        }
        if self.player_name.is_empty() || self.room_name.is_empty() {
            return Err("--bot-name and --bot-room must not be empty".into());
        }
        Ok(())
    }
}

struct Brain {
    child: Mutex<Child>,
    stdin: Sender<String>,
    replies: Mutex<Receiver<Value>>,
    alive: Arc<AtomicBool>,
}

impl Brain {
    fn spawn(config: &StockBotConfig) -> Result<Self, String> {
        let console = config.log_dir.join("policy.log");
        let stderr = File::create_new(&console)
            .map_err(|error| format!("cannot create {}: {error}", console.display()))?;
        let mut command = Command::new(&config.python);
        command
            .arg("-B")
            .arg("-u")
            .arg(&config.script)
            .arg(&config.policy)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|error| {
            format!(
                "cannot start policy sidecar {}: {error}",
                config.python.display()
            )
        })?;
        let stdin = child.stdin.take().ok_or("policy sidecar has no stdin")?;
        let stdout = child.stdout.take().ok_or("policy sidecar has no stdout")?;
        let (to_brain, from_app) = channel::<String>();
        std::thread::spawn(move || {
            let mut writer = BufWriter::new(stdin);
            for line in from_app {
                if writeln!(writer, "{line}")
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let (to_app, from_brain) = channel::<Value>();
        let alive = Arc::new(AtomicBool::new(true));
        let reader_alive = Arc::clone(&alive);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                    continue;
                };
                if to_app.send(value).is_err() {
                    break;
                }
            }
            reader_alive.store(false, Ordering::SeqCst);
        });
        Ok(Self {
            child: Mutex::new(child),
            stdin: to_brain,
            replies: Mutex::new(from_brain),
            alive,
        })
    }
}

impl Drop for Brain {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

type FighterBodyQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static MotionState,
        &'static PoseRuntime,
        &'static SmState,
        &'static FighterAir,
        &'static Fighter,
        &'static Health,
        &'static Qi,
    ),
    With<LocalFighter>,
>;

type RemoteBodyQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static MotionState,
        &'static PoseRuntime,
        &'static SmState,
        &'static FighterAir,
        &'static Fighter,
        &'static Health,
        &'static Qi,
        &'static RemoteFighter,
        &'static RemoteAuthorityGeneration,
    ),
    (Without<NpcFighter>, Without<LocalFighter>),
>;

type Keys = ButtonInput<KeyCode>;
type Mouse = ButtonInput<MouseButton>;
type SessionKey = (u32, u64, u64);

#[derive(Clone, Copy)]
enum BotPause {
    BetweenRounds,
    WeaponUnconfirmed,
}

impl BotPause {
    fn keeps_memory(self, previous: SessionKey, next: SessionKey) -> bool {
        let (old_round, old_local, old_opponent) = previous;
        let (new_round, new_local, new_opponent) = next;
        old_local == new_local
            && old_opponent == new_opponent
            && match self {
                Self::BetweenRounds => old_round.checked_add(1) == Some(new_round),
                Self::WeaponUnconfirmed => old_round == new_round,
            }
    }
}

#[derive(Resource)]
pub(crate) struct StockBot {
    pub(super) config: StockBotConfig,
    brain: Option<Brain>,
    startup_at: Option<Instant>,
    pub(super) ready: bool,
    pub(super) error: Option<String>,
    tick: u64,
    awaiting: Option<(u64, Instant)>,
    pending: Vec<(Instant, String, bool)>,
    applied: BTreeMap<String, bool>,
    unknown: BTreeSet<String>,
    session: Option<SessionKey>,
    paused_session: Option<(SessionKey, BotPause)>,
    cooldown: Option<Instant>,
    last_decision: Option<Instant>,
    last_weapons: Option<(u8, u8)>,
    release_requested: bool,
    trace: Option<File>,
}

impl StockBot {
    fn new(config: StockBotConfig) -> Self {
        let trace = File::create_new(config.log_dir.join("trace.jsonl")).ok();
        Self {
            config,
            brain: None,
            startup_at: None,
            ready: false,
            error: None,
            tick: 0,
            awaiting: None,
            pending: Vec::new(),
            applied: BTreeMap::new(),
            unknown: BTreeSet::new(),
            session: None,
            paused_session: None,
            cooldown: None,
            last_decision: None,
            last_weapons: None,
            release_requested: false,
            trace,
        }
    }

    fn trace(&mut self, value: Value) {
        if let Some(file) = self.trace.as_mut() {
            let _ = writeln!(file, "{value}");
            let _ = file.flush();
        }
    }

    fn send(&self, value: Value) -> bool {
        self.brain
            .as_ref()
            .is_some_and(|brain| brain.stdin.send(value.to_string()).is_ok())
    }

    fn abort(&mut self, reason: &str) {
        self.paused_session = None;
        if self.session.is_none() && self.pending.is_empty() && !self.release_requested {
            return;
        }
        self.trace(json!({"event": "abort", "reason": reason}));
        self.session = None;

        self.pending.clear();
        self.tick = 0;
        self.last_decision = None;
        self.cooldown = Some(Instant::now() + RETRY);
        self.release_requested = true;
    }

    fn pause(&mut self, reason: &str, kind: BotPause) {
        if let Some(key) = self.session {
            self.abort(reason);
            self.paused_session = Some((key, kind));
        } else if let (BotPause::BetweenRounds, Some((key, BotPause::WeaponUnconfirmed))) =
            (kind, self.paused_session)
        {
            self.paused_session = Some((key, kind));
        }
    }

    fn stop(&mut self, reason: String) {
        error!("stock bot: {reason}");
        self.trace(json!({"event": "error", "message": reason}));
        self.abort(&reason);
        self.brain = None;
        self.awaiting = None;
        self.ready = false;
        self.error = Some(reason);
    }

    fn begin(&mut self, key: SessionKey, now: Instant) {
        if self.session.is_some() {
            self.pause("round or fighter changed", BotPause::BetweenRounds);
        }
        if self.awaiting.is_some() || self.cooldown.is_some_and(|until| now < until) {
            return;
        }
        let same_opponent = self
            .paused_session
            .take()
            .is_some_and(|(previous, kind)| kind.keeps_memory(previous, key));
        if !self.send(json!({"reset": true, "same_opponent": same_opponent})) {
            self.stop("policy sidecar stdin closed".into());
            return;
        }
        self.session = Some(key);
        self.trace(json!({"event": "session", "round": key.0, "same_opponent": same_opponent}));
    }

    fn due(&self, now: Instant) -> bool {
        self.last_decision
            .is_none_or(|last| now.duration_since(last) >= WINDOW)
    }

    fn handle_reply(&mut self, message: &Value) {
        if message.get("ready").and_then(Value::as_bool) == Some(true) {
            self.ready = true;
            self.trace(json!({"event": "ready"}));
            info!("stock bot: policy sidecar ready");
            return;
        }
        let tick = message.get("tick").and_then(Value::as_u64);
        let reset = message.get("reset").and_then(Value::as_bool) == Some(true);
        let error = message.get("error").and_then(Value::as_str);
        if tick.is_none() && reset {
            return;
        }
        if let Some(error) = error {
            warn!("stock bot: policy error: {error}");
            self.trace(json!({"event": "error", "tick": tick, "message": error}));
        }
        let Some((sent, sent_at)) = self.awaiting else {
            return;
        };
        if tick != Some(sent) {
            return;
        }
        self.awaiting = None;
        if self.session.is_none() {
            return;
        }
        if reset {
            self.abort("policy reset the session");
            return;
        }
        let Some(edges) = parse_edges(message) else {
            self.abort("malformed policy edges");
            return;
        };
        self.tick = sent + 1;
        let now = Instant::now();
        schedule_edges(&mut self.pending, &edges, now);
        self.trace(json!({
            "event": "window",
            "tick": sent,
            "edges": edges,
            "latency_ms": now.duration_since(sent_at).as_millis() as u64,
        }));
    }
}

pub struct StockBotPlugin {
    config: StockBotConfig,
}

impl StockBotPlugin {
    pub fn new(config: StockBotConfig) -> Self {
        Self { config }
    }
}

impl Plugin for StockBotPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(StockBot::new(self.config.clone()))
            .init_resource::<BotLookDelta>()
            .add_systems(Startup, launch_brain)
            .add_systems(PreUpdate, pump_brain)
            .add_systems(
                Update,
                (bot_window, bot_apply)
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .after(super::network_plugin::NetworkFrameApply)
                    .before(super::camera_plugin::CameraInputSet)
                    .before(super::input_plugin::capture_local_command_input)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(OnExit(AppState::Playing), bot_leave_playing);
    }
}

fn launch_brain(mut bot: ResMut<StockBot>) {
    match Brain::spawn(&bot.config) {
        Ok(brain) => {
            info!(
                "stock bot: sidecar started script={} policy={}",
                bot.config.script.display(),
                bot.config.policy.display()
            );
            let policy = bot.config.policy.display().to_string();
            bot.trace(json!({"event": "spawn", "policy": policy}));
            bot.brain = Some(brain);
            bot.startup_at = Some(Instant::now());
        }
        Err(error) => bot.stop(error),
    }
}

fn pump_brain(mut bot: ResMut<StockBot>) {
    let (messages, alive) = {
        let Some(brain) = bot.brain.as_ref() else {
            return;
        };
        let replies = brain.replies.lock().unwrap();
        (
            replies.try_iter().collect::<Vec<Value>>(),
            brain.alive.load(Ordering::SeqCst),
        )
    };
    if !alive {
        bot.stop("policy sidecar exited; see policy.log".into());
        return;
    }
    for message in &messages {
        bot.handle_reply(message);
    }
    if !bot.ready
        && bot
            .startup_at
            .is_some_and(|at| at.elapsed() > Duration::from_secs(30))
    {
        bot.stop("policy sidecar did not start; see policy.log".into());
    } else if bot
        .awaiting
        .is_some_and(|(_, sent)| sent.elapsed() > WATCHDOG)
    {
        bot.stop("policy window timed out; restart the bot host".into());
    }
}

#[allow(clippy::too_many_arguments)]
fn bot_window(
    mut bot: ResMut<StockBot>,
    net: Res<NetState>,
    lock: Option<Res<LockOnController>>,
    local: FighterBodyQuery<'_, '_>,
    remotes: RemoteBodyQuery<'_, '_>,
) {
    if bot.brain.is_none() || !bot.ready {
        return;
    }
    let now = Instant::now();
    let match_ended = net
        .battle_result
        .as_ref()
        .is_some_and(|result| result.terminal);
    if !net.is_stock_udp() || net.status != NetStatus::InRoom || match_ended {
        bot.abort("not in an active match");
        bot.trace_weapons(None);
        return;
    }
    if !net.round_active || net.battle_result.is_some() {
        bot.pause("between rounds", BotPause::BetweenRounds);
        bot.trace_weapons(None);
        return;
    }
    let Ok((local_entity, lt, lm, lp, ls, la, lf, lh, lq)) = local.single() else {
        bot.abort("local fighter missing");
        return;
    };
    let mut opponent = None;
    let mut nearest = f32::INFINITY;
    for (entity, t, m, p, s, a, f, h, q, remote, generation) in &remotes {
        if generation.role_state_tick.is_none()
            || net
                .stock_npc_actors
                .values()
                .any(|actor| remote.player_id == format!("stock-{actor}"))
        {
            continue;
        }
        let dx = t.translation.x - lt.translation.x;
        let dz = t.translation.z - lt.translation.z;
        let distance = dx * dx + dz * dz;
        if distance < nearest {
            nearest = distance;
            opponent = Some((entity, t, m, p, s, a, f, h, q, remote));
        }
    }
    let Some((opponent_entity, ot, om, op, os, oa, of, oh, oq, remote)) = opponent else {
        bot.abort("waiting for an opponent");
        bot.trace_weapons(None);
        return;
    };
    let observed_weapon = net
        .stock_equipped_weapons
        .get(&remote.player_id)
        .map(|&(weapon, _)| weapon);
    let weapons = (lf.weapon, observed_weapon.unwrap_or(0));
    bot.trace_weapons(Some(weapons));
    if weapons != (DAGGER, DAGGER) || of.weapon != DAGGER {
        bot.pause(
            "both fighters must hold a dagger",
            BotPause::WeaponUnconfirmed,
        );
        return;
    }
    let key = (
        net.stock_round_number,
        local_entity.to_bits(),
        opponent_entity.to_bits(),
    );
    if bot.session != Some(key) {
        bot.begin(key, now);
        if bot.session != Some(key) {
            return;
        }
    }
    if bot.awaiting.is_some() || !bot.pending.is_empty() || !bot.due(now) {
        return;
    }
    let message = json!({
        "tick": bot.tick,
        "self": body_json(lt, lm, lp, ls, la, lf, lh, lq),
        "opponent": body_json(ot, om, op, os, oa, of, oh, oq),
        "camera": {"locked": lock.as_deref().is_some_and(|lock| lock.lock_entity.is_some())},
    });
    let tick = bot.tick;
    if bot.send(message) {
        bot.awaiting = Some((tick, now));
        bot.last_decision = Some(now);
    } else {
        bot.stop("policy sidecar stdin closed".into());
    }
}

impl StockBot {
    fn trace_weapons(&mut self, weapons: Option<(u8, u8)>) {
        if self.last_weapons == weapons {
            return;
        }
        self.last_weapons = weapons;
        if let Some((own, other)) = weapons {
            self.trace(json!({"event": "weapons", "self": own, "opponent": other}));
        }
    }
}

fn bot_apply(
    mut bot: ResMut<StockBot>,
    keymap: Res<Keymap>,
    mut synthetic: ResMut<SyntheticInputStream>,
    mut keys: ResMut<Keys>,
    mut mouse: ResMut<Mouse>,
    mut look: ResMut<BotLookDelta>,
) {
    look.0 = 0.0;
    if std::mem::take(&mut bot.release_requested) {
        release_all(&mut bot, &keymap, &mut synthetic, &mut keys, &mut mouse);
    }
    if bot.pending.is_empty() {
        reassert(&bot, &keymap, &mut keys, &mut mouse);
        return;
    }
    let now = Instant::now();
    let mut index = 0;
    while index < bot.pending.len() {
        if bot.pending[index].0 <= now {
            let (_, name, pressed) = bot.pending.remove(index);
            if let Some(delta) = bot_look_degrees(&name, pressed) {
                look.0 = (look.0 + delta).clamp(-120.0, 120.0);
                continue;
            }
            apply_edge(
                &mut bot,
                &keymap,
                &mut synthetic,
                &mut keys,
                &mut mouse,
                &name,
                pressed,
            );
        } else {
            index += 1;
        }
    }
    reassert(&bot, &keymap, &mut keys, &mut mouse);
}

fn reassert(bot: &StockBot, keymap: &Keymap, keys: &mut Keys, mouse: &mut Mouse) {
    for (name, pressed) in &bot.applied {
        if !*pressed {
            continue;
        }
        if let Some(binding) = binding_for(name, keymap) {
            set_binding(keys, mouse, binding, true);
        }
    }
}

fn bot_leave_playing(
    mut bot: ResMut<StockBot>,
    keymap: Res<Keymap>,
    mut synthetic: ResMut<SyntheticInputStream>,
    mut keys: ResMut<Keys>,
    mut mouse: ResMut<Mouse>,
) {
    bot.abort("left the round");
    release_all(&mut bot, &keymap, &mut synthetic, &mut keys, &mut mouse);
    bot.release_requested = false;
}

fn apply_edge(
    bot: &mut StockBot,
    keymap: &Keymap,
    synthetic: &mut SyntheticInputStream,
    keys: &mut Keys,
    mouse: &mut Mouse,
    name: &str,
    pressed: bool,
) {
    let Some(binding) = binding_for(name, keymap) else {
        if bot.unknown.insert(name.to_string()) {
            warn!("stock bot: unknown policy edge {name}");
            bot.trace(json!({"event": "unknown_edge", "name": name}));
        }
        return;
    };
    if binding == InputBinding::Unbound || bot.applied.get(name) == Some(&pressed) {
        return;
    }
    set_binding(keys, mouse, binding, pressed);
    synthetic.push_live_binding(binding, pressed);
    bot.applied.insert(name.to_string(), pressed);
}

fn release_all(
    bot: &mut StockBot,
    keymap: &Keymap,
    synthetic: &mut SyntheticInputStream,
    keys: &mut Keys,
    mouse: &mut Mouse,
) {
    for (name, pressed) in std::mem::take(&mut bot.applied) {
        if !pressed {
            continue;
        }
        if let Some(binding) = binding_for(&name, keymap) {
            if binding != InputBinding::Unbound {
                set_binding(keys, mouse, binding, false);
                synthetic.push_live_binding(binding, false);
            }
        }
    }
}

pub(crate) fn binding_for(name: &str, keymap: &Keymap) -> Option<InputBinding> {
    Some(match name {
        "forward" => keymap.move_up,
        "backward" => keymap.move_down,
        "left" => keymap.move_left,
        "right" => keymap.move_right,
        "attack" => keymap.attack,
        "guard" => keymap.guard,
        "jump" => keymap.jump,
        "crouch" => keymap.crouch,
        "fake" => keymap.fake,
        "qi_burst" => keymap.qi_burst,
        "lock_toggle" => keymap.lock_toggle,
        "left_turn" => keymap.left_turn,
        "right_turn" => keymap.right_turn,
        "switch_weapon" => keymap.weapon_switch,
        "drop_weapon" => keymap.drop_weapon,
        "drop_item" => keymap.drop_item,
        "cure" => keymap.cure,
        _ => return None,
    })
}

fn set_binding(keys: &mut Keys, mouse: &mut Mouse, binding: InputBinding, pressed: bool) {
    match (binding, pressed) {
        (InputBinding::Unbound, _) => {}
        (InputBinding::Key(key), true) => keys.press(key),
        (InputBinding::Key(key), false) => keys.release(key),
        (InputBinding::Mouse(button), true) => mouse.press(button),
        (InputBinding::Mouse(button), false) => mouse.release(button),
    }
}

fn parse_edges(message: &Value) -> Option<Vec<(String, bool)>> {
    let edges = message.get("edges")?.as_array()?;
    if edges.len() > MAX_EDGES {
        return None;
    }
    let mut parsed = Vec::with_capacity(edges.len());
    for edge in edges {
        let pair = edge.as_array()?;
        if pair.len() != 2 {
            return None;
        }
        parsed.push((pair[0].as_str()?.to_string(), pair[1].as_bool()?));
    }
    Some(parsed)
}

fn schedule_edges(
    pending: &mut Vec<(Instant, String, bool)>,
    edges: &[(String, bool)],
    now: Instant,
) {
    for (name, pressed) in edges {
        pending.push((now, name.clone(), *pressed));
    }
}

fn body_json(
    transform: &Transform,
    motion: &MotionState,
    pose: &PoseRuntime,
    state: &SmState,
    air: &FighterAir,
    fighter: &Fighter,
    health: &Health,
    qi: &Qi,
) -> Value {
    json!({
        "position": [
            transform.translation.x,
            transform.translation.y,
            -transform.translation.z,
        ],
        "forward_xz": [motion.facing.sin(), -motion.facing.cos()],
        "pose_id": pose.pose_id,
        "grounded": !air.air.airborne,
        "guard_route": state.current == FighterState::Guard,
        "state": format!("{:?}", state.current),
        "weapon_class": fighter.weapon,
        "hp": health.current,
        "max_hp": health.max,
        "anger": qi.current,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use meteor_core::MovementMode;

    #[test]
    fn opponent_memory_only_survives_a_known_pause() {
        let active = (4, 11, 22);
        assert!(BotPause::BetweenRounds.keeps_memory(active, (5, 11, 22)));
        assert!(BotPause::WeaponUnconfirmed.keeps_memory(active, active));
        assert!(!BotPause::BetweenRounds.keeps_memory(active, active));
        assert!(!BotPause::BetweenRounds.keeps_memory(active, (5, 11, 33)));
        assert!(!BotPause::BetweenRounds.keeps_memory(active, (6, 11, 22)));
        assert!(!BotPause::WeaponUnconfirmed.keeps_memory(active, (4, 33, 22)));
    }

    fn motion(facing: f32) -> MotionState {
        MotionState {
            movement_mode: MovementMode::Idle,
            velocity: Vec3::ZERO,
            facing,
        }
    }

    fn fighter(weapon: u8) -> Fighter {
        Fighter {
            character_id: 1,
            player_slot: 0,
            weapon,
            weapon_slots: [weapon, 0],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        }
    }

    #[test]
    fn policy_names_map_to_the_same_keys_as_the_lab_fixture() {
        let keymap = Keymap::default();
        let mut seen = BTreeSet::new();
        for name in [
            "forward",
            "backward",
            "left",
            "right",
            "attack",
            "guard",
            "jump",
            "crouch",
            "fake",
            "qi_burst",
            "lock_toggle",
            "left_turn",
            "right_turn",
            "switch_weapon",
            "drop_weapon",
            "drop_item",
            "cure",
        ] {
            assert!(binding_for(name, &keymap).is_some(), "{name}");
            assert!(seen.insert(name), "{name} listed twice");
        }
        assert_eq!(binding_for("dodge", &keymap), None);
    }

    #[test]
    fn schedule_matches_the_fixture_slice_bounds() {
        let edges: Vec<(String, bool)> = (0..8).map(|index| (format!("e{index}"), true)).collect();
        let now = Instant::now();
        let mut pending = Vec::new();
        schedule_edges(&mut pending, &edges, now);
        let slots: Vec<u64> = pending
            .iter()
            .map(|(due, _, _)| due.duration_since(now).as_millis() as u64)
            .collect();
        assert_eq!(slots, vec![0, 0, 0, 0, 0, 0, 0, 0]);
        let names: Vec<&str> = pending.iter().map(|(_, name, _)| name.as_str()).collect();
        assert_eq!(names, ["e0", "e1", "e2", "e3", "e4", "e5", "e6", "e7"]);
    }

    #[test]
    fn parse_edges_rejects_malformed_and_overlong_windows() {
        let good = json!({"tick": 3, "edges": [["guard", true], ["attack", false]]});
        assert_eq!(
            parse_edges(&good).unwrap(),
            vec![("guard".into(), true), ("attack".into(), false)]
        );
        assert!(parse_edges(&json!({"tick": 3})).is_none());
        assert!(parse_edges(&json!({"edges": [["guard"]]})).is_none());
        assert!(parse_edges(&json!({"edges": [["guard", 1]]})).is_none());
        assert!(parse_edges(&json!({"edges": vec![("guard", true); 9]})).is_none());
    }

    #[test]
    fn body_matches_the_lab_fixture_coordinates_and_fields() {
        let body = body_json(
            &Transform::from_xyz(1.0, 2.0, 3.0),
            &motion(0.0),
            &PoseRuntime::idle(),
            &SmState::default(),
            &FighterAir::default(),
            &fighter(DAGGER),
            &Health {
                current: 700.0,
                max: 1000.0,
            },
            &Qi::default(),
        );
        assert_eq!(body["position"], json!([1.0, 2.0, -3.0]));
        assert_eq!(body["forward_xz"], json!([0.0, -1.0]));
        assert_eq!(body["grounded"], json!(true));
        assert_eq!(body["state"], json!("Name"));
        assert_eq!(body["weapon_class"], json!(5));
        assert_eq!(body["hp"], json!(700.0));
    }
}
