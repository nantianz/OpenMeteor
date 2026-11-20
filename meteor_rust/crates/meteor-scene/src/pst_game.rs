use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::pst_runtime::PstValue;
use PstValue::{Array, Num, Str};

#[derive(Clone, Default)]
pub struct ScriptActor {
    pub name: String,
    pub position: [f32; 3],
    pub yaw: f32,
    pub alive: bool,
    pub team: i64,
    pub pose: i64,
    pub weapon: i64,
    pub weapon_type: i64,
    pub spare_weapon: i64,
    pub spare_type: i64,
    pub anger: f32,
    pub base: [f32; 5],
    pub effective: [f32; 3],
    pub kills: i64,
    pub deaths: i64,
    pub buffs: Vec<i64>,
    pub can_switch: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActorAction {
    Position([f32; 3]),
    Yaw(f32),
    Team(i64),
    Name(String),
    Base(usize, f32),
    RemoveBuff(i64),
    Equip(i64),
    Model(u8),
    DropWeapon,
    DropVip,
    AllowSwitch(bool),
    Reborn(Option<[f32; 3]>),
    Pose(i64),
    Combo(String),
    ImmediateCommand(String),
    LockTarget,
    AggressPose(u16),
    InputKey(String, bool),
    Kill,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GameRequest {
    Actor(i64, ActorAction),
    TempItem {
        actor: i64,
        resource: [i64; 2],
        position: Option<[f32; 3]>,
        yaw: Option<f32>,
        thrown: bool,
    },
    Enter(i64),
    Exit,
    SoundPlay(String, String, bool, bool),
    SoundStop(String),
    SoundPosition(String, [f32; 3]),
    SoundBind(String, i64),
    SoundUnbind(String),
    Notice(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptEvent {
    pub name: String,
    pub args: Vec<PstValue>,
}

type Raycast = Arc<dyn Fn([f32; 3], [f32; 3]) -> Option<[f32; 3]> + Send + Sync>;

#[derive(Default)]
struct StageState {
    active: bool,
    actors: BTreeMap<i64, ScriptActor>,
    bounds: Option<[[f32; 3]; 2]>,
    raycast: Option<Raycast>,
    raycast_used: bool,
    callbacks: Vec<ScriptEvent>,
    requests: Vec<GameRequest>,
    waves: BTreeMap<i64, ScriptWave>,
    wave_id: i64,
    esc_locked: bool,
    command_locked: bool,
    mouse_released: bool,
    chat_hidden: bool,
    esc_hint: [String; 3],
    command_hint: [String; 3],
}

#[derive(Clone, Default)]
struct ScriptWave {
    spatial: bool,
    path: Option<String>,
    position: Option<[f32; 3]>,
}

impl StageState {
    fn wave(&mut self, storage: Option<ScriptStorage>, name: &str, args: &[PstValue]) -> PstValue {
        if !self.active || self.requests.len() >= 1024 {
            return Num(0.0);
        }
        let id = args.first().and_then(int).unwrap_or(0);
        if matches!(name, "WaveNew" | "WaveClone") {
            if self.waves.len() >= 32 {
                return Num(-1.0);
            }
            let wave = if name == "WaveNew" {
                ScriptWave {
                    spatial: id != 0,
                    ..Default::default()
                }
            } else if let Some(wave) = self.waves.get(&id) {
                wave.clone()
            } else {
                return Num(-1.0);
            };
            self.wave_id += 1;
            self.waves.insert(self.wave_id, wave);
            return PstValue::from(self.wave_id);
        }
        let track = format!("~wave{id}");
        let Some(wave) = self.waves.get_mut(&id) else {
            return Num(0.0);
        };
        match (name, args.get(1)) {
            ("WaveDelete", _) => {
                self.waves.remove(&id);
                self.requests.push(GameRequest::SoundStop(track));
            }
            ("WaveLoad", Some(Str(path))) if audio_path(path) => {
                if !storage
                    .and_then(|storage| (storage.read_asset)(path))
                    .is_some_and(|bytes| {
                        bytes.len() <= 16 * 1024 * 1024
                            && bytes.get(..4) == Some(b"RIFF")
                            && bytes.get(8..12) == Some(b"WAVE")
                    })
                {
                    return Num(0.0);
                }
                wave.path = Some(path.clone());
            }
            ("WavePlay", looping) => {
                let Some(path) = wave.path.clone() else {
                    return Num(0.0);
                };
                self.requests.push(GameRequest::SoundPlay(
                    track.clone(),
                    path,
                    looping.is_some_and(PstValue::truthy),
                    wave.spatial,
                ));
                if let Some(position) = wave.position {
                    self.requests
                        .push(GameRequest::SoundPosition(track, position));
                }
            }
            ("WaveSetPosition", Some(position)) => {
                let Some(position) = vector(position) else {
                    return Num(0.0);
                };
                wave.position = Some(position);
                self.requests
                    .push(GameRequest::SoundPosition(track, position));
            }
            ("WaveStop", _) => self.requests.push(GameRequest::SoundStop(track)),
            _ => return Num(0.0),
        }
        Num(1.0)
    }
}

#[derive(Default)]
struct SessionState {
    shared: HashMap<String, PstValue>,
    stage: StageState,
    startup_events: HashSet<String>,
    pressed: HashSet<String>,
    down: HashSet<String>,
    window_size: [f32; 2],
    now: f64,
    execute_line_disabled: bool,
    suppress_script_messages: bool,
    console_output: Option<Vec<String>>,
}

#[derive(Clone, Copy)]
pub struct ScriptStorage {
    pub read: fn(&str) -> Option<String>,
    pub write: fn(&str, &str) -> bool,
    pub read_asset: fn(&str) -> Option<Vec<u8>>,
}

#[derive(Clone, Default)]
pub struct PstGame {
    state: Arc<Mutex<SessionState>>,
    storage: Option<ScriptStorage>,
    pub(crate) startup_scripts: crate::pst_startup::StartupRegistry,
}

pub fn vector(value: &PstValue) -> Option<[f32; 3]> {
    let Array(values) = value else { return None };
    let [x, y, z] = values.as_slice() else {
        return None;
    };
    Some([number(x)?, number(y)?, number(z)?])
}

pub fn vector_value(position: [f32; 3]) -> PstValue {
    Array(
        position
            .into_iter()
            .map(|value| Num(f64::from(value)))
            .collect(),
    )
}

fn number(value: &PstValue) -> Option<f32> {
    let Num(value) = value else { return None };
    (*value as f32).is_finite().then_some(*value as f32)
}

fn int(value: &PstValue) -> Option<i64> {
    let Num(value) = value else { return None };
    (value.fract() == 0.0 && value.abs() <= f64::from(i32::MAX)).then_some(*value as i64)
}

fn text(value: &PstValue) -> Option<&str> {
    let Str(value) = value else { return None };
    Some(value)
}

fn flag(value: bool) -> PstValue {
    Num(f64::from(u8::from(value)))
}

fn key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn audio_path(value: &str) -> bool {
    value.len() <= 192
        && value.ends_with(".wav")
        && value.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.- ".contains(&byte))
        })
}

impl PstGame {
    pub(crate) fn storage(&self) -> Option<ScriptStorage> {
        self.storage
    }
    pub fn new(storage: ScriptStorage) -> Self {
        Self {
            storage: Some(storage),
            ..Self::default()
        }
    }

    pub fn begin_stage(&self, active: bool) {
        self.state.lock().unwrap().stage = StageState {
            active,
            ..Default::default()
        };
    }

    pub fn active(&self) -> bool {
        self.state.lock().unwrap().stage.active
    }

    pub fn set_startup_events(&self, events: HashSet<String>) {
        self.state.lock().unwrap().startup_events = events;
    }

    pub(crate) fn register_startup_event(&self, event: &str) {
        self.state
            .lock()
            .unwrap()
            .startup_events
            .insert(event.into());
    }

    pub(crate) fn suppress_script_messages(&self, value: bool) {
        self.state.lock().unwrap().suppress_script_messages = value;
    }

    pub fn receives(&self, name: &str) -> bool {
        let state = self.state.lock().unwrap();
        state.stage.active
            && state.startup_events.contains(name)
            && !(name == "OnScriptMessage" && state.suppress_script_messages)
    }

    pub fn set_input(
        &self,
        pressed: HashSet<String>,
        down: HashSet<String>,
        size: [f32; 2],
        now: f64,
    ) {
        let mut state = self.state.lock().unwrap();
        state.pressed = pressed;
        state.down = down;
        state.window_size = size;
        state.now = now;
    }

    pub fn set_actors(&self, actors: BTreeMap<i64, ScriptActor>) {
        self.state.lock().unwrap().stage.actors = actors;
    }

    pub fn record_life_event(&self, id: i64, reborn: bool) {
        if let Some(actor) = self.state.lock().unwrap().stage.actors.get_mut(&id) {
            actor.alive = reborn;
            actor.base[1] = if reborn { actor.base[0] } else { 0.0 };
            if !reborn {
                actor.deaths += 1;
            }
        }
    }

    pub fn record_kill(&self, id: i64) {
        if let Some(actor) = self.state.lock().unwrap().stage.actors.get_mut(&id) {
            actor.kills += 1;
        }
    }

    pub fn set_map(&self, bounds: Option<[[f32; 3]; 2]>, raycast: Option<Raycast>) {
        let mut state = self.state.lock().unwrap();
        state.stage.bounds = bounds;
        state.stage.raycast = raycast;
    }

    pub fn raycast_used(&self) -> bool {
        self.state.lock().unwrap().stage.raycast_used
    }

    pub fn publish(&self, name: &str, args: &[PstValue]) {
        let mut state = self.state.lock().unwrap();
        if state.stage.active
            && state.startup_events.contains(name)
            && state.stage.callbacks.len() < 4096
        {
            state.stage.callbacks.push(ScriptEvent {
                name: name.into(),
                args: args.to_vec(),
            });
        }
    }

    pub fn take_events(&self) -> Vec<ScriptEvent> {
        std::mem::take(&mut self.state.lock().unwrap().stage.callbacks)
    }

    pub fn take_requests(&self) -> Vec<GameRequest> {
        std::mem::take(&mut self.state.lock().unwrap().stage.requests)
    }

    pub fn now(&self) -> f64 {
        self.state.lock().unwrap().now
    }

    pub fn execute_line_enabled(&self) -> bool {
        !self.state.lock().unwrap().execute_line_disabled
    }

    pub fn disable_execute_line(&self) {
        self.state.lock().unwrap().execute_line_disabled = true;
    }

    pub fn begin_console_capture(&self) {
        self.state.lock().unwrap().console_output = Some(Vec::new());
    }

    pub fn end_console_capture(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .console_output
            .take()
            .unwrap_or_default()
    }

    pub(crate) fn record_output(&self, args: &[PstValue]) {
        let mut state = self.state.lock().unwrap();
        let callbacks = state.stage.active
            && !state.suppress_script_messages
            && state.startup_events.contains("OnScriptMessage");
        if !callbacks && state.console_output.is_none() {
            return;
        }
        let words: Vec<_> = args.iter().map(crate::pst_runtime::value_text).collect();
        let message: String = words.join(" ").chars().take(4096).collect();
        if let Some(output) = state
            .console_output
            .as_mut()
            .filter(|output| output.len() < 64)
        {
            output.push(message.clone());
        }
        if callbacks && state.stage.callbacks.len() < 4096 {
            state.stage.callbacks.push(ScriptEvent {
                name: "OnScriptMessage".into(),
                args: vec![Str(message)],
            });
        }
    }

    pub fn key_hint(&self, console: bool) -> Option<String> {
        let state = self.state.lock().unwrap();
        let (locked, hints) = if console {
            (state.stage.command_locked, &state.stage.command_hint)
        } else {
            (state.stage.esc_locked, &state.stage.esc_hint)
        };
        locked.then(|| hints[2].clone())
    }

    pub fn mouse_released(&self) -> bool {
        self.state.lock().unwrap().stage.mouse_released
    }

    pub fn chat_hidden(&self) -> bool {
        self.state.lock().unwrap().stage.chat_hidden
    }

    pub(crate) fn builtin(&self, name: &str, args: &[PstValue]) -> Option<PstValue> {
        let storage_result = match (name, args) {
            ("LoadImageFile", [Str(path)]) if crate::pst_ui::is_image_path(path) => Some(
                self.storage
                    .and_then(|storage| (storage.read_asset)(path))
                    .filter(|bytes| !bytes.is_empty() && bytes.len() <= 4 * 1024 * 1024)
                    .map_or(Num(0.0), |_| Str(path.clone())),
            ),
            ("SaveString", [Str(slot), Str(data)]) => Some(flag(
                key(slot)
                    && data.len() <= 1024 * 1024
                    && self
                        .storage
                        .is_some_and(|storage| (storage.write)(slot, data)),
            )),
            ("LoadString", [Str(slot)]) => Some(Str(self
                .storage
                .filter(|_| key(slot))
                .and_then(|storage| (storage.read)(slot))
                .unwrap_or_default())),
            ("SaveVector", [Str(slot), value]) => Some(flag(
                key(slot)
                    && vector(value)
                        .and_then(|point| serde_json::to_string(&point).ok())
                        .is_some_and(|data| {
                            self.storage
                                .is_some_and(|storage| (storage.write)(slot, &data))
                        }),
            )),
            ("LoadVector", [Str(slot)]) => Some(
                self.storage
                    .filter(|_| key(slot))
                    .and_then(|storage| (storage.read)(slot))
                    .and_then(|data| serde_json::from_str::<[f32; 3]>(&data).ok())
                    .filter(|point| point.iter().all(|value| value.is_finite()))
                    .map_or(Num(0.0), vector_value),
            ),
            ("SaveString" | "LoadString" | "SaveVector" | "LoadVector", _) => Some(Num(0.0)),
            _ => None,
        };
        if storage_result.is_some() {
            return storage_result;
        }
        let mut state = self.state.lock().unwrap();
        let first = args.first().unwrap_or(&Num(0.0));
        let result = match name {
            "EnableKeyCommand" | "EnableKeyEsc" => {
                if !state.stage.active || args.len() != 1 {
                    return Some(Num(0.0));
                }
                let enabled = first.truthy();
                let (locked, hints) = if name == "EnableKeyCommand" {
                    (state.stage.command_locked, &state.stage.command_hint)
                } else {
                    (state.stage.esc_locked, &state.stage.esc_hint)
                };
                if locked == enabled {
                    let hint = hints[usize::from(!enabled)].clone();
                    if !hint.is_empty() {
                        state.stage.requests.push(GameRequest::Notice(hint));
                    }
                }
                if name == "EnableKeyCommand" {
                    state.stage.command_locked = !enabled;
                } else {
                    state.stage.esc_locked = !enabled;
                }
                Num(1.0)
            }
            "SetKeyCommandHint" | "SetKeyEscHint" => {
                let [Str(enabled), Str(disabled), Str(locked)] = args else {
                    return Some(Num(0.0));
                };
                let hints =
                    [enabled, disabled, locked].map(|hint| hint.chars().take(256).collect());
                if name == "SetKeyCommandHint" {
                    state.stage.command_hint = hints;
                } else {
                    state.stage.esc_hint = hints;
                }
                Num(1.0)
            }
            "ShowChatText" => {
                state.stage.chat_hidden = !first.truthy();
                Num(1.0)
            }
            "ShowMouse" => {
                state.stage.mouse_released = args.first().is_none_or(PstValue::truthy);
                Num(1.0)
            }
            "GetGameVersionText" => {
                PstValue::from(concat!("OpenMeteor ", env!("CARGO_PKG_VERSION")))
            }
            "WaveNew" | "WaveClone" | "WaveDelete" | "WaveLoad" | "WavePlay"
            | "WaveSetPosition" | "WaveStop" => state.stage.wave(self.storage, name, args),
            "SetSharedVarInt" | "SetSharedVarString" => {
                let [Str(key_name), value] = args else {
                    return Some(Num(0.0));
                };
                if !key(key_name)
                    || state.shared.len() >= 1024 && !state.shared.contains_key(key_name)
                {
                    return Some(Num(0.0));
                }
                let value = if name == "SetSharedVarInt" {
                    int(value).map(PstValue::from)
                } else {
                    text(value)
                        .filter(|value| value.len() <= 4096)
                        .map(PstValue::from)
                };
                if let Some(value) = value {
                    state.shared.insert(key_name.clone(), value);
                    Num(1.0)
                } else {
                    Num(0.0)
                }
            }
            "GetSharedVarInt" => text(first)
                .and_then(|key| state.shared.get(key))
                .and_then(int)
                .map_or(Num(0.0), PstValue::from),
            "GetSharedVarString" => text(first)
                .and_then(|key| state.shared.get(key))
                .and_then(text)
                .map_or_else(|| Str(String::new()), PstValue::from),
            "IsKeyPressed" => flag(
                text(first).is_some_and(|key| state.pressed.contains(&key.to_ascii_uppercase())),
            ),
            "IsKeyDown" => {
                flag(text(first).is_some_and(|key| state.down.contains(&key.to_ascii_uppercase())))
            }
            "GetGlobalTime" => Num(state.now),
            "GetGameWindowSize" => Array(
                state
                    .window_size
                    .into_iter()
                    .map(|n| Num(f64::from(n)))
                    .collect(),
            ),
            "GetCharIdList" => Array(
                state
                    .stage
                    .actors
                    .keys()
                    .copied()
                    .map(PstValue::from)
                    .collect(),
            ),
            "GetMapBoundingBoxMin" | "GetMapBoundingBoxMax" | "GetMapBoundingBoxSize" => {
                state.stage.bounds.map_or(Num(0.0), |[min, max]| {
                    vector_value(match name {
                        "GetMapBoundingBoxMin" => min,
                        "GetMapBoundingBoxMax" => max,
                        _ => std::array::from_fn(|index| max[index] - min[index]),
                    })
                })
            }
            "WorldRayCast" | "IsTwoPointsVisible" => {
                state.stage.raycast_used = true;
                let [a, b] = args else { return Some(Num(0.0)) };
                let (Some(a), Some(b), Some(raycast)) =
                    (vector(a), vector(b), &state.stage.raycast)
                else {
                    return Some(Num(0.0));
                };
                let hit = raycast(a, b);
                if name == "IsTwoPointsVisible" {
                    flag(hit.is_none())
                } else {
                    hit.map_or(Num(0.0), vector_value)
                }
            }
            _ => {
                if let Some(result) = actor_query(&state.stage, name, args) {
                    return Some(result);
                }
                let request = game_request(name, args)?;
                match request {
                    Some(request)
                        if (state.stage.active || matches!(request, GameRequest::Enter(_)))
                            && state.stage.requests.len() < 1024 =>
                    {
                        if let GameRequest::Actor(id, action) = &request {
                            let Some(actor) = state.stage.actors.get_mut(id) else {
                                return Some(Num(0.0));
                            };
                            project_actor_action(actor, action);
                        }
                        state.stage.requests.push(request);
                        Num(1.0)
                    }
                    _ => Num(0.0),
                }
            }
        };
        Some(result)
    }
}

fn actor_query(stage: &StageState, name: &str, args: &[PstValue]) -> Option<PstValue> {
    let actor = args
        .first()
        .and_then(int)
        .and_then(|id| stage.actors.get(&id));
    let base = args
        .get(1)
        .and_then(int)
        .and_then(|index| usize::try_from(index).ok());
    Some(match name {
        "GetCharName" if args.first().and_then(int) == Some(0) && actor.is_some() => {
            PstValue::from("player")
        }
        "GetDisplayName" | "GetCharName" => Str(actor.map_or("", |actor| &actor.name).to_owned()),
        "IsAlive" => flag(actor.is_some_and(|actor| actor.alive)),
        "CharGetPosition" => actor.map_or(Num(0.0), |actor| vector_value(actor.position)),
        "CharGetRotation" => actor.map_or(Num(0.0), |actor| vector_value([0.0, 0.0, actor.yaw])),
        "CharGetRotationYaw" => {
            actor.map_or(Num(0.0), |actor| PstValue::from(f64::from(actor.yaw)))
        }
        "GetActivePose" => PstValue::from(actor.map_or(-1, |actor| actor.pose)),
        "GetWeapon" => PstValue::from(actor.map_or(-1, |actor| actor.weapon)),
        "GetWeaponType" => PstValue::from(actor.map_or(-1, |actor| actor.weapon_type)),
        "GetActorTeam" => PstValue::from(actor.map_or(0, |actor| actor.team)),
        "GetKill" => PstValue::from(actor.map_or(0, |actor| actor.kills)),
        "GetDead" => PstValue::from(actor.map_or(0, |actor| actor.deaths)),
        "GetAnger" => actor.map_or(Num(0.0), |actor| Num(f64::from(actor.anger))),
        "IsAllowCharSwitchWeapon" => flag(actor.is_some_and(|actor| actor.can_switch)),
        "CharGetBufferList" => Array(
            actor
                .into_iter()
                .flat_map(|actor| &actor.buffs)
                .copied()
                .map(PstValue::from)
                .collect(),
        ),
        "GetAttack" | "GetDefense" | "GetSpeed" => {
            let index = match name {
                "GetAttack" => 0,
                "GetDefense" => 1,
                _ => 2,
            };
            Num(actor.map_or(0.0, |actor| f64::from(actor.effective[index])))
        }
        "CharGetBaseAttrib" => Num(base
            .and_then(|index| actor?.base.get(index).copied())
            .map_or(0.0, f64::from)),
        _ => return None,
    })
}

fn project_actor_action(actor: &mut ScriptActor, action: &ActorAction) {
    match action {
        ActorAction::Position(position) => actor.position = *position,
        ActorAction::Yaw(yaw) => actor.yaw = *yaw,
        ActorAction::Team(team) => actor.team = *team,
        ActorAction::Name(name) => actor.name.clone_from(name),
        ActorAction::Base(index, value) => {
            if let Some(effective) = index
                .checked_sub(2)
                .and_then(|index| actor.effective.get_mut(index))
            {
                *effective += *value - actor.base[*index];
            }
            actor.base[*index] = *value;
        }
        ActorAction::AllowSwitch(allowed) => actor.can_switch = *allowed,
        ActorAction::RemoveBuff(id) => actor.buffs.retain(|buff| buff != id),
        _ => {}
    }
}

fn game_request(name: &str, args: &[PstValue]) -> Option<Option<GameRequest>> {
    fn parse(name: &str, args: &[PstValue]) -> Option<GameRequest> {
        let first = args.first().unwrap_or(&Num(0.0));
        let second = args.get(1).unwrap_or(&Num(0.0));
        let track = || Some(text(first).filter(|name| key(name))?.to_owned());
        Some(match name {
            "EnterScene" => GameRequest::Enter(int(first)?),
            "ExitScene" => GameRequest::Exit,
            "PrintSystemText" => GameRequest::Notice(text(first)?.chars().take(256).collect()),
            "SoundPlay" => GameRequest::SoundPlay(
                track()?,
                text(second).filter(|path| audio_path(path))?.into(),
                args.get(2).is_some_and(PstValue::truthy),
                true,
            ),
            "SoundStop" => GameRequest::SoundStop(track()?),
            "SoundSetPosition" => GameRequest::SoundPosition(track()?, vector(second)?),
            "SoundBindActor" => GameRequest::SoundBind(track()?, int(second)?),
            "SoundUnbind" => GameRequest::SoundUnbind(track()?),
            "CharCreateTempItem" | "CharDropTempItem" => {
                let actor = int(first)?;
                let resource = [int(second)?, int(args.get(2)?)?];
                let thrown = name == "CharDropTempItem";
                let (position, yaw) = match args.get(3..) {
                    Some([]) if thrown => (None, None),
                    Some([position]) if !thrown => (Some(vector(position)?), None),
                    Some([position, yaw]) if thrown => {
                        (Some(vector(position)?), Some(number(yaw)?))
                    }
                    _ => return None,
                };
                GameRequest::TempItem {
                    actor,
                    resource,
                    position,
                    yaw,
                    thrown,
                }
            }
            _ => {
                let action = match name {
                    "CharSetPosition" => ActorAction::Position(vector(second)?),
                    "CharSetRotation" => ActorAction::Yaw(vector(second)?[2]),
                    "CharSetRotationYaw" => ActorAction::Yaw(number(second)?),
                    "SetTeam" => {
                        ActorAction::Team(int(second).filter(|team| (1..=2).contains(team))?)
                    }
                    "SetDisplayName" => ActorAction::Name(text(second)?.chars().take(64).collect()),
                    "CharSetBaseAttrib" => {
                        let index = int(second).filter(|index| (0..5).contains(index))? as usize;
                        ActorAction::Base(index, number(args.get(2)?)?)
                    }
                    "CharRemoveBuffer" => ActorAction::RemoveBuff(int(second)?),
                    "CharEquipWeapon" => ActorAction::Equip(int(second)?),
                    "CharSetModel" => ActorAction::Model(u8::try_from(int(second)?).ok()?),
                    "CharDropWeapon" => ActorAction::DropWeapon,
                    "CharDropVipItem" => ActorAction::DropVip,
                    "SetAllowCharSwitchWeapon" => ActorAction::AllowSwitch(second.truthy()),
                    "RebornAt" => ActorAction::Reborn(Some(vector(second)?)),
                    "RebornAround" => ActorAction::Reborn(None),
                    "CharShowPose" => ActorAction::Pose(int(second)?),
                    "CharJump" => ActorAction::Combo("j".into()),
                    "CharBaoQi" => ActorAction::Combo("burst".into()),
                    "CharAttack" => ActorAction::Combo(text(second)?.chars().take(64).collect()),
                    "CharAttackImm" => {
                        ActorAction::ImmediateCommand(text(second)?.chars().take(64).collect())
                    }
                    "CharJumpImm" => ActorAction::ImmediateCommand("j".into()),
                    "CharLockTarget" => ActorAction::LockTarget,
                    "SetAggressPose" => ActorAction::AggressPose(u16::try_from(int(second)?).ok()?),
                    "InputGameKey" => {
                        let key = text(second)?;
                        if !matches!(
                            key,
                            "attack"
                                | "jump"
                                | "guard"
                                | "crouch"
                                | "lock"
                                | "switch"
                                | "burst"
                                | "fake"
                                | "cure"
                                | "dropweapon"
                                | "dropitem"
                                | "forward"
                                | "back"
                                | "left"
                                | "right"
                        ) {
                            return None;
                        }
                        ActorAction::InputKey(key.into(), args.get(2)?.truthy())
                    }
                    "ServerKill" => ActorAction::Kill,
                    _ => return None,
                };
                GameRequest::Actor(int(first)?, action)
            }
        })
    }
    match name {
        "EnterScene"
        | "ExitScene"
        | "PrintSystemText"
        | "SoundPlay"
        | "SoundStop"
        | "SoundSetPosition"
        | "SoundBindActor"
        | "SoundUnbind"
        | "CharCreateTempItem"
        | "CharDropTempItem"
        | "CharSetPosition"
        | "CharSetRotation"
        | "CharSetRotationYaw"
        | "SetTeam"
        | "SetDisplayName"
        | "CharSetBaseAttrib"
        | "CharRemoveBuffer"
        | "CharEquipWeapon"
        | "CharSetModel"
        | "CharDropWeapon"
        | "CharDropVipItem"
        | "SetAllowCharSwitchWeapon"
        | "RebornAt"
        | "RebornAround"
        | "CharShowPose"
        | "CharJump"
        | "CharBaoQi"
        | "CharAttack"
        | "CharAttackImm"
        | "CharJumpImm"
        | "CharLockTarget"
        | "SetAggressPose"
        | "InputGameKey"
        | "ServerKill" => Some(parse(name, args)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission_runtime::MissionScriptRuntime;
    use crate::pst_runtime::{parse_program, NullHost, Runtime};
    use crate::scene_script_runtime::{SceneItemInstance, SceneScriptRuntime};

    #[test]
    fn timers_and_shared_state_survive_script_scope_and_keep_due_order() {
        let game = PstGame::default();
        let mut first = Runtime::new(
            parse_program(
                r#"
            Arm() {
                SetSharedVarString("steps", "");
                DelayCall(2, "Append", "B");
                DelayCall(1, "Append", "A");
                DelayCall(1, "Skip");
                CancelDelayedCalls("Skip");
            }
            Append(string value) {
                SetSharedVarString("steps", GetSharedVarString("steps") + value);
            }
            Skip() { SetSharedVarString("steps", "wrong"); }
        "#,
            )
            .unwrap(),
            NullHost,
            2000,
        );
        first.set_game(game.clone());
        first.call("Arm", &[]).unwrap();
        game.set_input(default(), default(), [0.0; 2], 3.0);
        first.run_due(None).unwrap();
        game.begin_stage(false);
        assert_eq!(
            game.builtin("GetSharedVarString", &[PstValue::from("steps")]),
            Some(PstValue::from("AB"))
        );
    }

    #[test]
    fn item_event_reaches_legacy_scene_mission_and_startup_with_stable_id() {
        let game = PstGame::default();
        game.begin_stage(true);
        game.set_startup_events(HashSet::from(["Item_OnTouch".into()]));
        let source = r#"
            Key_OnTouch(int id, int actor) { SetSharedVarInt("event", 10); }
            Item_OnTouch(int id, int actor) { SetSharedVarInt("event", GetSharedVarInt("event") + actor); }
        "#;
        let mut scene = SceneScriptRuntime::load(
            source,
            vec![SceneItemInstance {
                index: 0,
                name: "Key".into(),
                props: default(),
            }],
            2000,
        )
        .unwrap();
        scene.set_game(game.clone());
        let mut mission = MissionScriptRuntime::new(
            r#"Item_OnTouch(int id, int actor) { SetSharedVarInt("event", GetSharedVarInt("event") * 2 + id); }"#,
            default(),
        );
        mission.set_game(game.clone());
        scene
            .dispatch_item_event(
                "Key",
                "OnTouch",
                &[PstValue::from(7_i64)],
                Some(&mut mission),
            )
            .unwrap();
        assert_eq!(
            game.builtin("GetSharedVarInt", &[PstValue::from("event")]),
            Some(PstValue::from(34_i64))
        );
        assert_eq!(
            game.take_events(),
            [ScriptEvent {
                name: "Item_OnTouch".into(),
                args: vec![PstValue::from(0_i64), PstValue::from(7_i64)],
            }]
        );
    }

    fn default<T: Default>() -> T {
        T::default()
    }
    #[test]
    fn console_switch_collections_and_session_lock_follow_pst_semantics() {
        let game = PstGame::default();
        let mut runtime = Runtime::new(parse_program("int seed = 3;").unwrap(), NullHost, 2000);
        runtime.set_game(game.clone());
        let value = runtime
            .execute_line_with_peer(
                r#"
            var table = Map();
            table = MapSet(table, "token", ArrayPush(Array(4, 5), 6));
            switch (seed) {
                case 1: return 0;
                case 3: seed += 4;
                default: seed += ArraySize(MapGet(table, "token"));
            }
            seed
        "#,
                None,
            )
            .unwrap();
        assert_eq!(value, Num(10.0));
        assert_eq!(
            runtime
                .call("CanInvokeGameAPI", &[PstValue::from("ReadFile")])
                .unwrap(),
            Num(1.0)
        );
        assert_eq!(
            runtime
                .call("CanInvokeGameAPI", &[PstValue::from("EncryptScript")])
                .unwrap(),
            Num(0.0)
        );
        assert_eq!(
            runtime
                .execute_line_with_peer("DisableExecuteLine()", None)
                .unwrap(),
            Num(1.0)
        );
        game.begin_stage(true);
        assert!(runtime.execute_line_with_peer("seed = 999", None).is_err());
    }

    #[test]
    fn file_handles_and_waves_use_the_installed_assets_and_real_request_types() {
        use std::sync::Mutex;
        static SLOT: Mutex<Option<String>> = Mutex::new(None);
        let game = PstGame::new(ScriptStorage {
            read: |_| SLOT.lock().unwrap().clone(),
            write: |_, text| {
                *SLOT.lock().unwrap() = Some(text.into());
                true
            },
            read_asset: |path| (path == "sound/chime.wav").then(|| b"RIFF0000WAVE".to_vec()),
        });
        game.begin_stage(true);
        let mut runtime = Runtime::new(parse_program("").unwrap(), NullHost, 2000);
        runtime.set_game(game.clone());
        assert_eq!(
            runtime
                .execute_line_with_peer(
                    r#"
            int f = OpenFile("UserData/Saves/as_demo.sav", "w");
            WriteFile(f, 42);
            CloseFile(f);
            f = OpenFile("as_demo", "r");
            ReadFile(f)
        "#,
                    None
                )
                .unwrap(),
            Num(42.0)
        );
        assert_eq!(
            runtime
                .execute_line_with_peer(r#"OpenFile("../outside", "w")"#, None)
                .unwrap(),
            Num(-1.0)
        );
        assert_eq!(
            runtime
                .execute_line_with_peer(
                    r#"
            int track = WaveNew(false);
            WaveLoad(track, "sound/chime.wav");
            var clone = WaveClone(track);
            WavePlay(clone, true)
        "#,
                    None
                )
                .unwrap(),
            Num(1.0)
        );
        assert_eq!(
            game.take_requests(),
            [GameRequest::SoundPlay(
                "~wave2".into(),
                "sound/chime.wav".into(),
                true,
                false
            )]
        );
    }
}
