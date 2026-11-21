use crate::app_state::{AppState, InputBinding, KeyField, Keymap};
use crate::components::fighter::MotionState;
use crate::plugins::camera_plugin::CameraBasis;
use crate::plugins::impact_fx_plugin::{CombatImpactEvent, CombatImpactKind};
use crate::plugins::input_plugin::{SyntheticAction, SyntheticInputStream};
use crate::plugins::stage_plugin::LocalFighter;
use bevy::prelude::*;
use meteor_core::MODEL_SCALE;
use meteor_input::command_parser::Dir;
use meteor_network::stock_protocol::StockSceneFrame;
use std::collections::{HashMap, HashSet};
use std::path::Path;

mod queue_a;
#[cfg(not(target_arch = "wasm32"))]
mod trace;

use queue_a::{actor_key, MatchProjection, RecordedEvent};

#[derive(Debug, Clone)]
pub struct ReplayEvent {
    pub time: f32,
    pub key: InputBinding,
    pub pressed: bool,
}

#[derive(Resource, Default)]
pub struct ReplayRecorder {
    pub active: bool,

    pub auto: bool,
    pub elapsed: f32,
    pub events: Vec<ReplayEvent>,
    pub queue_a: Vec<meteor_asset::mrc::MrcQueueAEntry>,
    pub queue_d: Vec<meteor_asset::mrc::MrcQueueDEntry>,
    pub queue_e: Vec<meteor_asset::mrc::MrcQueueEEntry>,
    mrc_state: [u8; meteor_asset::mrc::MRC_STATE_SIZE],
    mrc_header_captured: bool,
    prev: HashMap<InputBinding, bool>,
    last_vitals_key: Option<[u16; 3]>,
    last_pose_key: Option<[i32; 5]>,
    last_pose_time: f32,
}

#[derive(Resource, Default)]
pub struct ReplayPlayer {
    pub active: bool,
    pub elapsed: f32,
    pub events: Vec<ReplayEvent>,
    pub index: usize,
    pub file_name: String,

    pub paused: bool,
}

#[derive(Resource, Default)]
pub struct ReplaySeekRequest(pub Option<f32>);

impl ReplayPlayer {
    pub fn duration(&self) -> f32 {
        self.events.last().map(|e| e.time).unwrap_or(0.0)
    }
}

#[derive(Resource, Default)]
pub struct MrcPlayback {
    pub active: bool,
    pub paused: bool,
    pub elapsed: f32,
    pub doc: Option<meteor_asset::mrc::MrcDocument>,
    pub index_a: usize,
    pub index_b: usize,
    pub index_c: usize,
    pub index_d: usize,
    pub index_e: usize,
    pub file_name: String,

    pub prev_c: i32,
    pub(crate) pending_hit_locks: Vec<(Entity, u32, f32)>,
    pub(crate) burst_tracks: HashMap<u16, Vec<MrcRemoteBurst>>,

    pub(crate) frame_times: Vec<f32>,

    pub(crate) frame_cursor: usize,

    pub(crate) frame_wall: f64,

    pub(crate) frame_target: Option<f32>,
}

pub(crate) const MRC_MAX_FRAMES_PER_UPDATE: usize = 8;

pub(crate) fn mrc_recorded_frame_times(doc: &meteor_asset::mrc::MrcDocument) -> Vec<f32> {
    let mut stamps: Vec<f64> = doc
        .queue_d
        .iter()
        .map(|e| f64::from(e.time))
        .chain(doc.queue_e.iter().map(|e| f64::from(e.time)))
        .filter(|t| t.is_finite() && *t >= 0.0)
        .collect();
    stamps.sort_by(f64::total_cmp);
    stamps.dedup_by(|a, b| (*a - *b).abs() < 1e-5);
    let mut gaps: Vec<f64> = stamps
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|g| (0.004..0.0125).contains(g))
        .collect();
    if gaps.len() < 16 {
        return Vec::new();
    }
    gaps.sort_by(f64::total_cmp);
    let period = gaps[gaps.len() / 2];
    let end = f64::from(doc.duration()) + period;
    let mut frames = Vec::new();
    let mut push_span = |from: f64, to: f64, frames: &mut Vec<f32>| {
        let count = ((to - from) / period).round().max(1.0) as usize;
        for k in 1..=count {
            frames.push((from + (to - from) * k as f64 / count as f64) as f32);
        }
    };
    let first = stamps[0];
    let lead = (first / period).floor() as usize;
    for k in 0..lead {
        frames.push((first - period * (lead - k) as f64) as f32);
    }
    frames.push(first as f32);
    for w in stamps.windows(2) {
        push_span(w[0], w[1], &mut frames);
    }
    let last = *stamps.last().unwrap();
    if end > last {
        let count = ((end - last) / period).ceil() as usize;
        for k in 1..=count {
            frames.push((last + period * k as f64) as f32);
        }
    }
    frames.retain(|t| *t > 0.0);
    frames
}

#[derive(Debug, Clone)]
pub(crate) struct MrcRemoteBurst {
    start_time: f32,
    end_time: f32,

    points: Vec<Vec3>,
}

impl MrcRemoteBurst {
    fn position_at(&self, time: f32) -> Vec3 {
        let steps = (self.points.len() - 1) as f32;
        let at = ((time - self.start_time) / (self.end_time - self.start_time) * steps)
            .clamp(0.0, steps);
        let index = (at.floor() as usize).min(self.points.len() - 2);
        self.points[index].lerp(self.points[index + 1], at - index as f32)
    }
}

fn mrc_remote_bursts(doc: &meteor_asset::mrc::MrcDocument) -> HashMap<u16, Vec<MrcRemoteBurst>> {
    let mut packets: HashMap<u16, Vec<(f32, meteor_network::role_state::RoleState)>> =
        HashMap::new();
    for entry in &doc.queue_a {
        if entry.id != 2 {
            continue;
        }
        for chunk in entry.blob.chunks_exact(16) {
            let actor_id = u16::from_le_bytes([chunk[0], chunk[1]]);
            if !(1..=64).contains(&actor_id) {
                continue;
            }
            if let Ok(state) = meteor_network::role_state::decode_event2_state(&chunk[2..]) {
                packets
                    .entry(actor_id)
                    .or_default()
                    .push((entry.time, state));
            }
        }
    }
    let mut tracks = HashMap::new();
    for (actor_id, states) in packets {
        let mut bursts = Vec::new();
        let mut previous_end: Option<(f32, meteor_network::role_state::RoleState)> = None;
        let mut group_start = 0;
        while group_start < states.len() {
            let mut group_end = group_start + 1;
            while group_end < states.len() && states[group_end].0 - states[group_start].0 < 0.001 {
                group_end += 1;
            }
            let group = &states[group_start..group_end];
            if let Some((start_time, start_state)) = previous_end {
                let end_time = group.last().unwrap().0;
                let span = end_time - start_time;
                if group.len() > 1
                    && span >= 0.15
                    && group.iter().all(|(_, state)| {
                        state.state_words.pose_id == start_state.state_words.pose_id
                    })
                {
                    let mut points = Vec::with_capacity(group.len() + 1);
                    points.push(role_position(start_state));
                    points.extend(group.iter().map(|(_, state)| role_position(*state)));
                    let path_length: f32 = points
                        .windows(2)
                        .map(|pair| pair[0].distance(pair[1]))
                        .sum();
                    let max_speed = std::iter::once(start_state)
                        .chain(group.iter().map(|(_, state)| *state))
                        .map(|state| role_velocity(state).length())
                        .fold(0.0_f32, f32::max);

                    if path_length / span <= max_speed * 2.0 + 1.0 {
                        bursts.push(MrcRemoteBurst {
                            start_time,
                            end_time,
                            points,
                        });
                    }
                }
            }
            previous_end = group.last().copied();
            group_start = group_end;
        }
        if !bursts.is_empty() {
            tracks.insert(actor_id, bursts);
        }
    }
    tracks
}

#[derive(Resource, Default)]
pub(crate) struct MrcFrameOverride {
    pub(crate) position_applied: bool,
    pub(crate) facing_applied: bool,
}

#[derive(Resource, Default)]
struct MrcHeldInput {
    bindings: HashSet<InputBinding>,
}

fn mrc_gameplay_bindings(keymap: &Keymap) -> HashSet<InputBinding> {
    KeyField::BINDING_ACTIONS
        .iter()
        .map(|field| field.get(keymap))
        .chain(std::iter::once(keymap.dodge))
        .collect()
}

fn isolate_mrc_hardware_input(
    playback: Res<MrcPlayback>,
    keymap: Res<Keymap>,
    held: Res<MrcHeldInput>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
) {
    if !playback.active {
        return;
    }
    for binding in mrc_gameplay_bindings(&keymap) {
        match binding {
            InputBinding::Unbound => {}
            InputBinding::Key(key) => {
                if held.bindings.contains(&binding) {
                    keys.press(key);
                    keys.clear_just_pressed(key);
                    keys.clear_just_released(key);
                } else {
                    keys.reset(key);
                }
            }
            InputBinding::Mouse(button) => {
                if held.bindings.contains(&binding) {
                    mouse.press(button);
                    mouse.clear_just_pressed(button);
                    mouse.clear_just_released(button);
                } else {
                    mouse.reset(button);
                }
            }
        }
    }
}

fn capture_mrc_held_input(
    playback: Res<MrcPlayback>,
    keymap: Res<Keymap>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut held: ResMut<MrcHeldInput>,
) {
    held.bindings.clear();
    if playback.active {
        held.bindings.extend(
            mrc_gameplay_bindings(&keymap)
                .into_iter()
                .filter(|binding| binding.pressed(&keys, &mouse)),
        );
    }
}

#[derive(Debug, Clone)]
enum MrcSessionEvent {
    Identity {
        actor_id: u16,
        character_id: u8,
        item_id: u32,
        camp: u32,
    },
    Vitals {
        hp: f32,
        max_hp: f32,
        anger: f32,
    },
    Equip {
        item_id: u32,
        slot: u32,
    },
    Remote {
        actor_id: u16,
        state: meteor_network::role_state::RoleState,
    },
    Action {
        actor_id: u16,
        chain: meteor_network::role_state::PoseChain,
        start: bool,
    },
    Hit {
        guarded: bool,
        event: meteor_network::stock_protocol::StockHitReactionEvent,
        time: f32,
    },
    Property(meteor_network::stock_protocol::StockPropertyEffectEvent),
    World {
        time: f32,
        event: RecordedEvent,
    },
}

#[derive(Resource, Default)]
struct MrcSessionRuntime {
    pending: Vec<MrcSessionEvent>,
    pending_actions: Vec<(u16, meteor_network::role_state::PoseChain, bool)>,
    pending_hits: Vec<(
        bool,
        meteor_network::stock_protocol::StockHitReactionEvent,
        f32,
    )>,
    pending_properties: Vec<meteor_network::stock_protocol::StockPropertyEffectEvent>,
    character_by_slot: HashMap<u16, u8>,
    equip_by_slot: HashMap<u32, (u8, u8)>,
    event_counts: HashMap<u32, u64>,
    unhandled_counts: HashMap<u32, u64>,
    rebuild_remotes: bool,
    match_projection: MatchProjection,
    scene_objects: Option<super::network_plugin::stock_scene::StockSceneObjectMap>,
    scene_objects_loaded: bool,
    stage_mode: Option<&'static str>,
    restore_scene: bool,
    scene_baseline: Option<Vec<meteor_scene::scene_script_runtime::SceneItemInstance>>,
}

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct MrcRemoteFighter {
    pub(crate) actor_id: u16,
    pose_sequence: u16,
    ground_sequence: u16,
    desired_pose_id: u16,
    pose_pending: bool,
    action_sequence: Option<u16>,
    pending_action: Option<meteor_network::role_state::PoseChain>,
    pending_start: bool,
    applied_character_id: u8,
    state_age: f32,
    hit_age: f32,
    pending_stock_state: bool,
}

impl MrcPlayback {
    pub fn duration(&self) -> f32 {
        self.doc.as_ref().map(|d| d.duration()).unwrap_or(0.0)
    }

    pub fn load(&mut self, path: &Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        self.load_bytes(path.display().to_string(), &bytes)
    }

    pub fn load_bytes(&mut self, file_name: String, bytes: &[u8]) -> Result<(), String> {
        let doc = meteor_asset::mrc::decode_mrc(bytes).map_err(|e| e.to_string())?;
        self.burst_tracks = mrc_remote_bursts(&doc);
        self.frame_times = if doc.mark0 == 9 && doc.mark1 == 7 {
            mrc_recorded_frame_times(&doc)
        } else {
            Vec::new()
        };
        self.frame_cursor = 0;
        self.frame_wall = 0.0;
        self.frame_target = None;
        self.doc = Some(doc);
        self.index_a = 0;
        self.index_b = 0;
        self.index_c = 0;
        self.index_d = 0;
        self.index_e = 0;
        self.pending_hit_locks.clear();
        self.elapsed = 0.0;
        self.prev_c = 0;
        self.active = true;
        self.paused = false;
        self.file_name = file_name;
        Ok(())
    }

    fn state_word(&self, index: usize) -> Option<u32> {
        let state = &self.doc.as_ref()?.state;
        let offset = index.checked_mul(4)?;
        let bytes = state.get(offset..offset + 4)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }

    pub(crate) fn stock_scene_coordinates(&self) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|doc| doc.mark0 == 9 && doc.mark1 == 7)
    }

    pub(crate) fn local_slot(&self) -> Option<u32> {
        self.state_word(1).filter(|slot| *slot > 0)
    }

    pub(crate) fn local_character(&self) -> Option<u8> {
        let slot = self.local_slot()?;
        self.doc
            .as_ref()?
            .queue_a
            .iter()
            .find_map(|entry| match entry.id {
                4 => meteor_network::stock_protocol::decode_stock_roster(&entry.blob)
                    .ok()?
                    .into_iter()
                    .find(|member| member.actor_id == slot && member.entered && member.hero < 20)
                    .map(|member| member.hero as u8),
                9 => meteor_network::stock_protocol::StockRoleEnteredEvent::decode(&entry.blob)
                    .ok()
                    .filter(|member| member.actor_id == slot && member.hero < 20)
                    .map(|member| member.hero as u8),
                _ => None,
            })
    }

    pub(crate) fn local_weapon(&self) -> Option<(u8, u8)> {
        crate::plugins::character_plugin::weapon_slot_from_item(self.state_word(0)? as i64)
    }

    pub(crate) fn stage_id(&self) -> Option<String> {
        self.doc.as_ref()?.queue_a.iter().find_map(|entry| {
            if entry.id != 10 || entry.blob.len() < 4 {
                return None;
            }
            let bytes = &entry.blob[..4];
            ((bytes[0] == b's' || bytes[0] == b'S')
                && (bytes[1] == b'n' || bytes[1] == b'N')
                && bytes[2].is_ascii_digit()
                && bytes[3].is_ascii_digit())
            .then(|| String::from_utf8_lossy(bytes).to_ascii_lowercase())
        })
    }

    pub(crate) fn spawn_vitals(&self) -> Option<(f32, f32, f32)> {
        self.doc.as_ref()?.queue_a.iter().find_map(|entry| {
            (entry.id == 25)
                .then(|| decode_mrc_vitals(&entry.blob))
                .flatten()
        })
    }

    pub(crate) fn first_stock_local_position(&self, center: [f32; 3]) -> Option<Vec3> {
        if !self.stock_scene_coordinates() {
            return None;
        }
        let entry = self.doc.as_ref()?.queue_d.first()?;
        let source = decode_mrc_pose(entry)?;
        Some(mrc_scene_pose(source, Some(StockSceneFrame(center))))
    }
}

fn decode_mrc_vitals(blob: &[u8]) -> Option<(f32, f32, f32)> {
    if blob.len() < 4 {
        return None;
    }
    let hp = u16::from_le_bytes([blob[0], blob[1]]) as f32;
    let max_hp = u16::from_le_bytes([blob[2], blob[3]]) as f32;
    if max_hp <= 0.0 {
        return None;
    }
    let anger = blob
        .get(4..6)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]) as f32)
        .unwrap_or(0.0);
    Some((hp.clamp(0.0, max_hp), max_hp, anger.max(0.0)))
}

fn route_mrc_session_entry(
    entry: &meteor_asset::mrc::MrcQueueAEntry,
    runtime: &mut MrcSessionRuntime,
) {
    *runtime.event_counts.entry(entry.id).or_default() += 1;
    let mut handled = false;
    match entry.id {
        4 => {
            if let Ok(roster) = meteor_network::stock_protocol::decode_stock_roster(&entry.blob) {
                for member in roster {
                    runtime.pending.push(MrcSessionEvent::World {
                        time: entry.time,
                        event: RecordedEvent::PlayerName(member.actor_id, member.name),
                    });
                    if member.entered && (1..=64).contains(&member.actor_id) && member.hero < 20 {
                        runtime.pending.push(MrcSessionEvent::Identity {
                            actor_id: member.actor_id as u16,
                            character_id: member.hero as u8,
                            item_id: member.weapon_item_id,
                            camp: member.camp,
                        });
                    }
                }
                handled = true;
            }
        }
        9 => {
            if let Ok(member) =
                meteor_network::stock_protocol::StockRoleEnteredEvent::decode(&entry.blob)
            {
                if (1..=64).contains(&member.actor_id) && member.hero < 20 {
                    runtime.pending.push(MrcSessionEvent::Identity {
                        actor_id: member.actor_id as u16,
                        character_id: member.hero as u8,
                        item_id: member.hero_checksum,
                        camp: member.camp,
                    });
                    handled = true;
                }
            }
        }
        2 => {
            for chunk in entry.blob.chunks_exact(16) {
                let actor_id = u16::from_le_bytes([chunk[0], chunk[1]]);
                if actor_id == 0 || actor_id > 64 {
                    continue;
                }
                if let Ok(state) = meteor_network::role_state::decode_event2_state(&chunk[2..]) {
                    runtime
                        .pending
                        .push(MrcSessionEvent::Remote { actor_id, state });
                    handled = true;
                }
            }
        }
        61 => {
            if let Ok(event) =
                meteor_network::stock_protocol::StockPropertyEffectEvent::decode(&entry.blob)
            {
                if (1..=64).contains(&event.actor_id) {
                    runtime.pending.push(MrcSessionEvent::Property(event));
                    handled = true;
                }
            }
        }
        11 | 14 => {
            if let Ok(event) =
                meteor_network::stock_protocol::StockHitReactionEvent::decode(&entry.blob)
            {
                let pose_id = if entry.id == 14 {
                    event.pose_id >> 16
                } else {
                    event.pose_id
                };
                if (1..=64).contains(&event.target_actor_id) && pose_id <= 2048 {
                    runtime.pending.push(MrcSessionEvent::Hit {
                        guarded: entry.id == 14,
                        event,
                        time: entry.time,
                    });
                    handled = true;
                }
            }
        }
        50 | 51 => {
            if let Ok(event) =
                meteor_network::stock_protocol::StockPoseChainEvent::decode(&entry.blob)
            {
                let chain: meteor_network::role_state::PoseChain = event.into();
                if (1..=64).contains(&event.actor_id) && chain.is_valid() {
                    runtime.pending.push(MrcSessionEvent::Action {
                        actor_id: event.actor_id,
                        chain,
                        start: entry.id == 50,
                    });
                    handled = true;
                }
            }
        }
        12 if entry.blob.len() >= 8 => {
            let item_id = u32::from_le_bytes(entry.blob[0..4].try_into().unwrap());
            let slot = u32::from_le_bytes(entry.blob[4..8].try_into().unwrap());
            if (1..=64).contains(&slot) {
                runtime
                    .pending
                    .push(MrcSessionEvent::Equip { item_id, slot });
                handled = true;
            }
        }
        25 => {
            if let Some((hp, max_hp, anger)) = decode_mrc_vitals(&entry.blob) {
                runtime
                    .pending
                    .push(MrcSessionEvent::Vitals { hp, max_hp, anger });
                handled = true;
            }
        }
        10 => handled = true,
        _ => {
            if let Some(event) = RecordedEvent::decode(entry.id, &entry.blob) {
                runtime.pending.push(MrcSessionEvent::World {
                    time: entry.time,
                    event,
                });
                handled = true;
            }
        }
    }
    if !handled {
        *runtime.unhandled_counts.entry(entry.id).or_default() += 1;
    }
}

fn decode_mrc_pose(entry: &meteor_asset::mrc::MrcQueueDEntry) -> Option<Vec3> {
    let [x, y, z] = [
        f32::from_bits(entry.a),
        f32::from_bits(entry.b),
        f32::from_bits(entry.c),
    ];
    [x, y, z]
        .into_iter()
        .all(f32::is_finite)
        .then(|| Vec3::new(x * MODEL_SCALE, z * MODEL_SCALE, y * MODEL_SCALE))
}

fn decode_mrc_facing(entry: &meteor_asset::mrc::MrcQueueEEntry) -> Option<f32> {
    let x = f32::from_bits(entry.a);
    let z = f32::from_bits(entry.b);
    let length = x.hypot(z);
    (x.is_finite() && z.is_finite() && length > 1e-6).then(|| (x / length).atan2(z / length))
}

fn mrc_scene_frame(
    playback: &MrcPlayback,
    waypoints: Option<&meteor_scene::stage_loader::WaypointGraph>,
) -> Option<StockSceneFrame> {
    if !playback.stock_scene_coordinates() {
        return None;
    }
    let center = waypoints.map(|graph| graph.center).unwrap_or_else(|| {
        let stage = playback.stage_id().unwrap_or_default();
        meteor_scene::stage_loader::get_stage_center(&stage, None)
    });
    Some(StockSceneFrame(center))
}

fn mrc_scene_pose(position: Vec3, frame: Option<StockSceneFrame>) -> Vec3 {
    let Some(StockSceneFrame(center)) = frame else {
        return position;
    };
    Vec3::new(
        position.x - center[0] * MODEL_SCALE,
        position.y - center[2] * MODEL_SCALE,
        center[1] * MODEL_SCALE - position.z,
    )
}

fn mrc_scene_facing(yaw: f32, frame: Option<StockSceneFrame>) -> f32 {
    if frame.is_some() {
        yaw.sin().atan2(-yaw.cos())
    } else {
        yaw
    }
}

#[allow(clippy::too_many_arguments)]
fn tick_mrc_playback(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    keymap: Res<Keymap>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut synthetic: ResMut<SyntheticInputStream>,
    mut playback: ResMut<MrcPlayback>,
    waypoints: Option<Res<meteor_scene::stage_loader::WaypointGraph>>,
    seek: Res<ReplaySeekRequest>,
    mut frame_override: ResMut<MrcFrameOverride>,
    mut session: ResMut<MrcSessionRuntime>,
    mut basis: ResMut<CameraBasis>,
    mut lock: Option<ResMut<crate::components::fighter::LockOnController>>,
    mut fighters: Query<
        (
            &mut Transform,
            &mut MotionState,
            Option<&crate::components::fighter::SmState>,
            Option<&mut crate::plugins::movement_plugin::FighterAir>,
            Option<&mut Visibility>,
        ),
        With<LocalFighter>,
    >,
    lock_targets: Query<(&Transform, Option<&MotionState>), Without<LocalFighter>>,
) {
    frame_override.position_applied = false;
    let step = frame_clock.as_deref().and_then(|clock| clock.substep);
    if step.is_none_or(|step| step.first) {
        frame_override.facing_applied = false;
    }
    if step.is_some() {
        keys.clear();
        mouse.clear();
    }
    if seek.0.is_some() {
        return;
    }
    if !playback.active || playback.paused {
        return;
    }
    if let Some(target) = playback.frame_target.take() {
        playback.elapsed = target;
    } else {
        playback.elapsed +=
            crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    }
    let t = playback.elapsed;
    let scene_frame = mrc_scene_frame(&playback, waypoints.as_deref());
    synthetic.set_frame_clock_seconds(f64::from(t));

    let (
        index_a,
        index_b,
        index_c,
        index_d,
        index_e,
        packed,
        pose,
        facing,
        finished,
        (pre_entry, entered_now),
    ) = {
        let Some(doc) = playback.doc.as_ref() else {
            return;
        };

        let session_cutoff = doc.queue_b[playback.index_b..]
            .iter()
            .take_while(|entry| entry.time <= t)
            .find(|entry| {
                entry.value != 0
                    && (mrc_code_to_synthetic_action(entry.code).is_some()
                        || matches!(entry.code, 0x11 | 0x12))
            })
            .map_or(t, |entry| entry.time);
        let mut i_a = playback.index_a;
        while i_a < doc.queue_a.len() && doc.queue_a[i_a].time <= session_cutoff {
            route_mrc_session_entry(&doc.queue_a[i_a], &mut session);
            i_a += 1;
        }
        let mut pose = None;
        let mut i_d = playback.index_d;
        while i_d < doc.queue_d.len() && doc.queue_d[i_d].time <= t {
            pose = decode_mrc_pose(&doc.queue_d[i_d])
                .map(|position| mrc_scene_pose(position, scene_frame))
                .or(pose);
            i_d += 1;
        }

        let pre_entry = scene_frame.is_some() && i_d == 0;
        let entered_now = scene_frame.is_some() && playback.index_d == 0 && i_d > 0;
        if pre_entry {
            pose = doc
                .queue_d
                .first()
                .and_then(decode_mrc_pose)
                .map(|position| mrc_scene_pose(position, scene_frame));
        }
        let mut facing = None;
        let mut i_e = playback.index_e;
        while i_e < doc.queue_e.len() && doc.queue_e[i_e].time <= t {
            facing = decode_mrc_facing(&doc.queue_e[i_e])
                .map(|yaw| mrc_scene_facing(yaw, scene_frame))
                .or(facing);
            i_e += 1;
        }

        let mut i_b = playback.index_b;
        while i_b < doc.queue_b.len() && doc.queue_b[i_b].time <= t {
            let entry = &doc.queue_b[i_b];
            if matches!(entry.code, 0x11 | 0x12) {
                if entry.value != 0 {
                    synthetic.push_dodge(
                        replay_dodge_direction(&keys, &mouse, &keymap),
                        entry.code == 0x11,
                    );
                }
            } else if let Some(action) = mrc_code_to_synthetic_action(entry.code) {
                if let Some(binding) = mrc_code_to_key(entry.code, &keymap) {
                    set_replay_input(&mut keys, &mut mouse, binding, entry.value != 0);
                    synthetic.push_action(binding, action, entry.value != 0, Some(f64::from(t)));
                }
            } else if let Some(binding) = mrc_code_to_key(entry.code, &keymap) {
                if matches!(entry.code, 0x57 | 0x53 | 0x41 | 0x44) {
                    set_replay_input(&mut keys, &mut mouse, binding, entry.value != 0);
                    synthetic.push_mrc_direction(binding, entry.value != 0, Some(f64::from(t)));
                } else {
                    inject_replay_binding(
                        &mut keys,
                        &mut mouse,
                        &mut synthetic,
                        binding,
                        entry.value != 0,
                        Some(f64::from(t)),
                    );
                }
            }
            i_b += 1;
        }

        let mut packed = None;
        let mut i_c = playback.index_c;
        while i_c < doc.queue_c.len() && doc.queue_c[i_c].time <= t {
            packed = Some(doc.queue_c[i_c].packed as i32);
            i_c += 1;
        }
        (
            i_a,
            i_b,
            i_c,
            i_d,
            i_e,
            packed,
            pose,
            facing,
            i_a >= doc.queue_a.len()
                && i_b >= doc.queue_b.len()
                && i_c >= doc.queue_c.len()
                && i_d >= doc.queue_d.len()
                && i_e >= doc.queue_e.len(),
            (pre_entry, entered_now),
        )
    };
    playback.index_a = index_a;
    playback.index_b = index_b;
    playback.index_c = index_c;
    playback.index_d = index_d;
    playback.index_e = index_e;
    if pose.is_some() || facing.is_some() {
        if let Ok((mut transform, mut motion, sm, mut air, visibility)) = fighters.single_mut() {
            if let Some(mut visibility) = visibility {
                if pre_entry && *visibility != Visibility::Hidden {
                    *visibility = Visibility::Hidden;
                } else if entered_now && *visibility == Visibility::Hidden {
                    *visibility = Visibility::Inherited;
                }
            }
            if pre_entry {
                if let Some(air) = air.as_deref_mut() {
                    air.air.vertical_speed = 0.0;
                    air.air.planar_x = 0.0;
                    air.air.planar_z = 0.0;
                }
            }
            if let Some(position) = pose {
                transform.translation = position;

                frame_override.position_applied = true;

                let ordinary_ground = sm
                    .is_some_and(|sm| sm.current == meteor_core::FighterState::Name)
                    && air.as_deref().is_none_or(|air| !air.air.airborne);
                if facing.is_none() && ordinary_ground && playback.stock_scene_coordinates() {
                    if let Some((target, target_motion)) = lock
                        .as_deref()
                        .and_then(|lock| lock.lock_entity)
                        .and_then(|entity| lock_targets.get(entity).ok())
                    {
                        if let Some(yaw) = meteor_combat::lock_on::locked_facing_yaw(
                            position.to_array().into(),
                            target.translation.to_array().into(),
                            target_motion.map(|motion| motion.facing),
                        ) {
                            motion.facing = yaw;
                        }
                    }
                }
            }
            if let Some(yaw) = facing {
                motion.facing = yaw;
                basis.yaw = yaw;
                frame_override.facing_applied = true;

                if playback.stock_scene_coordinates() {
                    if let Some(lock) = lock.as_deref_mut() {
                        lock.system.clear_lock();
                        lock.lock_entity = None;
                    }
                }
            }
        }
    }
    if let Some(packed) = packed {
        apply_mrc_packed_input(packed, &keymap, &mut keys, &mut mouse, &mut synthetic);
        playback.prev_c = packed;
    }

    if finished {
        playback.paused = true;
    }
}

type MrcLocalVitalsQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static crate::components::fighter::Fighter,
        &'static mut crate::components::fighter::Health,
        &'static mut crate::components::fighter::Qi,
        &'static mut crate::components::fighter::DeathRespawn,
        &'static mut crate::components::fighter::CombatState,
        &'static mut crate::components::fighter::SmState,
        &'static mut crate::components::fighter::PoseRuntime,
        &'static mut crate::components::fighter::EntryBlend,
    ),
    With<LocalFighter>,
>;

type MrcRemoteQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut MrcRemoteFighter,
        &'static mut crate::components::fighter::Fighter,
        &'static mut crate::components::fighter::PoseRuntime,
        &'static mut crate::components::fighter::EntryBlend,
        &'static mut MotionState,
        &'static mut crate::plugins::movement_plugin::FighterAir,
        &'static mut crate::plugins::network_plugin::RemoteTransformTarget,
        Option<&'static crate::plugins::network_plugin::RemoteHitMotionWindow>,
        &'static mut Transform,
    ),
    (With<MrcRemoteFighter>, Without<LocalFighter>),
>;

fn role_position(state: meteor_network::role_state::RoleState) -> Vec3 {
    Vec3::new(state.position.x, state.position.y, state.position.z)
}

fn role_velocity(state: meteor_network::role_state::RoleState) -> Vec3 {
    Vec3::new(state.velocity.x, state.velocity.y, state.velocity.z)
}

fn role_movement_mode(state: meteor_network::role_state::RoleState) -> meteor_core::MovementMode {
    if !state.state_words.grounded {
        meteor_core::MovementMode::Airborne
    } else if state.velocity.x * state.velocity.x + state.velocity.z * state.velocity.z > 0.01 {
        meteor_core::MovementMode::Run
    } else {
        meteor_core::MovementMode::Idle
    }
}

fn apply_mrc_action_pose(
    assets: Option<&crate::plugins::character_plugin::CharacterAssetsSet>,
    fighter: &crate::components::fighter::Fighter,
    pose: &mut crate::components::fighter::PoseRuntime,
    blend: &mut crate::components::fighter::EntryBlend,
    chain: meteor_network::role_state::PoseChain,
) -> bool {
    let Some(character) =
        assets.and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
    else {
        return false;
    };
    if !character.poses.contains_key(&chain.pose_id) {
        return false;
    }

    character.apply_pose_with_blend(
        pose,
        blend,
        chain.pose_id,
        true,
        Some(chain.blend_seconds),
        crate::components::fighter::EntryBlendKind::Chained,
        false,
        true,
        None,
    );
    blend.duration = chain.blend_seconds.max(0.0);
    pose.loops = false;
    true
}

fn apply_mrc_start_pose(
    assets: Option<&crate::plugins::character_plugin::CharacterAssetsSet>,
    fighter: &crate::components::fighter::Fighter,
    pose: &mut crate::components::fighter::PoseRuntime,
    blend: &mut crate::components::fighter::EntryBlend,
    chain: meteor_network::role_state::PoseChain,
) -> bool {
    let Some(character) =
        assets.and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
    else {
        return false;
    };
    if !character.poses.contains_key(&chain.pose_id) {
        return false;
    }
    apply_stock_start_pose(character, pose, blend, chain);
    true
}

pub(crate) fn apply_stock_start_pose(
    character: &crate::plugins::character_plugin::CharacterAssets,
    pose: &mut crate::components::fighter::PoseRuntime,
    blend: &mut crate::components::fighter::EntryBlend,
    chain: meteor_network::role_state::PoseChain,
) {
    character.apply_pose_with_blend(
        pose,
        blend,
        chain.pose_id,
        true,
        (chain.blend_seconds > 0.01).then_some(chain.blend_seconds),
        crate::components::fighter::EntryBlendKind::Generic,
        false,
        true,
        None,
    );
    if chain.blend_seconds > 0.01 {
        blend.duration = chain.blend_seconds;
    }
    pose.link_past_end = true;
}

fn apply_mrc_discrete_pose(
    assets: Option<&crate::plugins::character_plugin::CharacterAssetsSet>,
    fighter: &crate::components::fighter::Fighter,
    pose: &mut crate::components::fighter::PoseRuntime,
    blend: &mut crate::components::fighter::EntryBlend,
    chain: meteor_network::role_state::PoseChain,
    start: bool,
) -> bool {
    if start {
        apply_mrc_start_pose(assets, fighter, pose, blend, chain)
    } else {
        apply_mrc_action_pose(assets, fighter, pose, blend, chain)
    }
}

fn apply_mrc_remote_pose(
    assets: Option<&crate::plugins::character_plugin::CharacterAssetsSet>,
    fighter: &crate::components::fighter::Fighter,
    pose: &mut crate::components::fighter::PoseRuntime,
    blend: &mut crate::components::fighter::EntryBlend,
    pose_id: u16,
) -> bool {
    let Some(character) =
        assets.and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
    else {
        return false;
    };
    character.apply_pose_with_blend(
        pose,
        blend,
        pose_id,
        true,
        Some(0.1),
        crate::components::fighter::EntryBlendKind::Generic,
        false,
        true,
        None,
    );
    true
}

#[allow(clippy::too_many_arguments)]
fn apply_mrc_session_events(
    mut commands: Commands,
    playback: Res<MrcPlayback>,
    waypoints: Option<Res<meteor_scene::stage_loader::WaypointGraph>>,
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    mut session: ResMut<MrcSessionRuntime>,
    mut local: MrcLocalVitalsQuery<'_, '_>,
    mut remotes: MrcRemoteQuery<'_, '_>,
    mut net: Option<ResMut<super::network_plugin::NetState>>,
    stage_assets: Option<Res<crate::app_state::LoadedStageAssets>>,
) {
    let events = std::mem::take(&mut session.pending);
    let rebuild_remotes = std::mem::take(&mut session.rebuild_remotes);
    if events.is_empty() && session.pending_actions.is_empty() && !rebuild_remotes {
        if playback.active {
            if let Some(net) = net.as_deref_mut() {
                session.match_projection.update_timer(playback.elapsed, net);
            }
        }
        return;
    }
    if rebuild_remotes {
        for (entity, ..) in &mut remotes {
            commands.entity(entity).despawn();
        }
    }
    let mut vitals = if rebuild_remotes {
        playback.spawn_vitals()
    } else {
        None
    };
    let current_vitals = local
        .single()
        .ok()
        .map(|(_, health, qi, ..)| (health.current, health.max, qi.current));
    let mut remote_states = HashMap::new();
    let local_slot = playback.local_slot();
    let mut actions = std::mem::take(&mut session.pending_actions);
    let scene_frame = mrc_scene_frame(&playback, waypoints.as_deref());
    if !session.scene_objects_loaded && net.is_some() {
        let stage = playback
            .doc
            .as_ref()
            .and_then(|doc| doc.queue_a.iter().find(|entry| entry.id == 10))
            .and_then(|entry| {
                meteor_network::stock_protocol::StockStageConfigEvent::decode(&entry.blob).ok()
            });
        if let Some(config) = stage {
            session.scene_objects = stage_assets
                .as_ref()
                .and_then(|assets| assets.des.as_deref())
                .and_then(|des| {
                    super::network_plugin::stock_scene_objects_from_des(des, config.field_c)
                })
                .or_else(|| {
                    super::network_plugin::stock_scene_objects(&config.stage_id, config.field_c)
                });
            session.stage_mode = Some(super::network_plugin::stock_mode_from_rule(
                config.field_a as i32,
            ));
        }
        session.scene_objects_loaded = true;
    }
    if let Some(net) = net.as_deref_mut() {
        if rebuild_remotes {
            MatchProjection::clear_net(net);
            session.match_projection = MatchProjection::default();
            session.restore_scene = true;
        }
        if let Some(local_slot) = local_slot {
            net.player_id = actor_key(local_slot);
        }
        if let Some(mode) = session.stage_mode {
            if net.mode != mode {
                net.mode = mode.to_owned();
            }
        }
    }
    for event in events {
        match event {
            MrcSessionEvent::Identity {
                actor_id,
                character_id,
                item_id,
                camp,
            } => {
                session.character_by_slot.insert(actor_id, character_id);
                if let Some(weapon) =
                    crate::plugins::character_plugin::weapon_slot_from_item(item_id as i64)
                {
                    session.equip_by_slot.insert(u32::from(actor_id), weapon);
                }
                if let Some(net) = net.as_deref_mut() {
                    let weapon = session
                        .equip_by_slot
                        .get(&u32::from(actor_id))
                        .map_or(0, |slot| slot.0);
                    session.match_projection.member(
                        net,
                        u32::from(actor_id),
                        character_id,
                        weapon,
                        (camp != 0).then_some(camp),
                    );
                }
            }
            MrcSessionEvent::Vitals { hp, max_hp, anger } => {
                vitals = Some((hp, max_hp, anger));
            }
            MrcSessionEvent::Equip { item_id, slot } => {
                if let Some(weapon) =
                    crate::plugins::character_plugin::weapon_slot_from_item(item_id as i64)
                {
                    session.equip_by_slot.insert(slot, weapon);
                }
            }
            MrcSessionEvent::Remote { actor_id, state } => {
                if local_slot != Some(actor_id as u32) {
                    remote_states.insert(actor_id, state);
                }
            }
            MrcSessionEvent::Action {
                actor_id,
                chain,
                start,
            } => {
                actions.push((actor_id, chain, start));
            }
            MrcSessionEvent::Hit {
                guarded,
                event,
                time: event_time,
            } => {
                session.pending_hits.push((guarded, event, event_time));
            }
            MrcSessionEvent::Property(event) => session.pending_properties.push(event),
            MrcSessionEvent::World { time, event } => {
                if let Some(net) = net.as_deref_mut() {
                    if matches!(&event, RecordedEvent::Respawn(None)) && local_slot.is_some() {
                        if let Some((_, max_hp, anger)) = vitals.or(current_vitals) {
                            vitals = Some((max_hp, max_hp, anger));
                        }
                    }
                    if let RecordedEvent::Left(actor) = &event {
                        if let Ok(actor_id) = u16::try_from(*actor) {
                            remote_states.remove(&actor_id);
                            session.character_by_slot.remove(&actor_id);
                            actions.retain(|(candidate, _, _)| *candidate != actor_id);
                            if !rebuild_remotes {
                                for (entity, remote, ..) in &mut remotes {
                                    if remote.actor_id == actor_id {
                                        commands.entity(entity).despawn();
                                    }
                                }
                            }
                        }
                        session.equip_by_slot.remove(actor);
                    }
                    let session = &mut *session;
                    session.match_projection.apply(
                        event,
                        time,
                        local_slot,
                        rebuild_remotes,
                        scene_frame,
                        session.scene_objects.as_ref(),
                        net,
                    );
                }
            }
        }
    }
    if let Some(net) = net.as_deref_mut() {
        session.match_projection.update_timer(playback.elapsed, net);
    }
    if session.match_projection.local_life == Some(false) {
        if let Some((_, max_hp, anger)) = vitals.or(current_vitals) {
            vitals = Some((0.0, max_hp, anger));
        }
    }
    if let Some((hp, max_hp, anger)) = vitals {
        if let Ok((
            fighter,
            mut health,
            mut qi,
            mut death,
            mut combat,
            mut sm,
            mut pose,
            mut blend,
        )) = local.single_mut()
        {
            let was_alive = health.current > 0.0;
            health.max = max_hp.max(1.0);
            health.current = hp.clamp(0.0, health.max);
            qi.current = anger.clamp(0.0, qi.max);
            death.active = health.current <= 0.0;
            if was_alive && death.active {
                combat.state = meteor_core::FighterState::Dead;
                sm.previous = sm.current;
                sm.current = meteor_core::FighterState::Dead;
                sm.locked = true;
                if let Some(character) = assets
                    .as_deref()
                    .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
                {
                    let death_pose = meteor_combat::hit_reaction::DEATH_POSES
                        .iter()
                        .copied()
                        .find(|pose_id| character.poses.contains_key(pose_id))
                        .unwrap_or(meteor_combat::hit_reaction::ENVIRONMENT_HIT_POSE);
                    character.apply_pose_with_blend(
                        &mut pose,
                        &mut blend,
                        death_pose,
                        true,
                        None,
                        crate::components::fighter::EntryBlendKind::Generic,
                        true,
                        false,
                        None,
                    );
                }
            } else if !was_alive && !death.active {
                death.timer = 0.0;
                combat.state = meteor_core::FighterState::Name;
                sm.previous = sm.current;
                sm.current = meteor_core::FighterState::Name;
                sm.locked = false;
                if let Some(character) = assets
                    .as_deref()
                    .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
                {
                    character.apply_pose(&mut pose, 0, true);
                }
            }
        }
    }

    for (actor_id, state) in remote_states {
        if session.match_projection.left.contains(&u32::from(actor_id)) {
            continue;
        }
        let state = scene_frame.map_or(state, |frame| frame.to_scene(state));
        let position = role_position(state);
        let velocity = role_velocity(state);
        let yaw = meteor_network::role_state::yaw_from_forward(state.forward);
        if !rebuild_remotes {
            if let Some((
                _,
                mut remote,
                fighter,
                mut pose,
                mut blend,
                mut motion,
                mut air,
                mut target,
                hit_window,
                _transform,
            )) = remotes
                .iter_mut()
                .find(|(_, remote, ..)| remote.actor_id == actor_id)
            {
                let transition = meteor_network::role_state::decide_pose_transition(
                    meteor_network::role_state::RemotePoseRuntime {
                        pose_id: remote.desired_pose_id,
                        ground_sequence: remote.ground_sequence,
                        applied_sequence: remote.pose_sequence,
                    },
                    state,
                );
                remote.ground_sequence = transition.ground_sequence;
                remote.pose_sequence = transition.applied_sequence;
                if transition.apply {
                    remote.desired_pose_id = state.state_words.pose_id;
                    remote.pose_pending = true;
                    if remote.pending_action.is_some_and(|action| {
                        meteor_network::role_state::pose_sequence_is_newer(
                            state.state_words.pose_sequence,
                            action.sequence,
                        )
                    }) {
                        remote.pending_action = None;
                        remote.pending_start = false;
                    }
                }
                target.translation = position;
                target.yaw = yaw;

                remote.state_age = 0.0;
                remote.pending_stock_state = true;
                motion.velocity = velocity;
                motion.movement_mode = role_movement_mode(state);
                air.air.airborne = !state.state_words.grounded;
                air.ground_contact = state.state_words.grounded;
                if remote.pose_pending
                    && !(hit_window.is_some_and(|window| window.remaining > 0.0)
                        && remote.pending_action.is_none())
                    && match remote.pending_action {
                        Some(chain) => apply_mrc_discrete_pose(
                            assets.as_deref(),
                            &fighter,
                            &mut pose,
                            &mut blend,
                            chain,
                            remote.pending_start,
                        ),
                        None => apply_mrc_remote_pose(
                            assets.as_deref(),
                            &fighter,
                            &mut pose,
                            &mut blend,
                            remote.desired_pose_id,
                        ),
                    }
                {
                    remote.pose_pending = false;
                    remote.pending_action = None;
                    remote.pending_start = false;
                    remote.applied_character_id = fighter.character_id;
                }
                continue;
            }
        }

        let character_id = session
            .character_by_slot
            .get(&actor_id)
            .copied()
            .unwrap_or(0);
        let (weapon, variant) = session
            .equip_by_slot
            .get(&(actor_id as u32))
            .copied()
            .unwrap_or((5, 0));
        let fighter = crate::components::fighter::Fighter {
            character_id,
            player_slot: actor_id.min(u8::MAX as u16) as u8,
            weapon,
            weapon_slots: [weapon, 0],
            weapon_slot: 0,
            weapon_slot_variants: [variant, 0],
            weapon_variant: variant,
        };
        let mut pose = crate::components::fighter::PoseRuntime::idle();
        let mut blend = crate::components::fighter::EntryBlend::default();
        let applied = apply_mrc_remote_pose(
            assets.as_deref(),
            &fighter,
            &mut pose,
            &mut blend,
            state.state_words.pose_id,
        );
        let visible_position = if rebuild_remotes {
            playback
                .burst_tracks
                .get(&actor_id)
                .and_then(|bursts| {
                    bursts.iter().find(|burst| {
                        burst.start_time <= playback.elapsed && playback.elapsed <= burst.end_time
                    })
                })
                .map(|burst| mrc_scene_pose(burst.position_at(playback.elapsed), scene_frame))
                .unwrap_or(position)
        } else {
            position
        };
        let mut air = crate::plugins::movement_plugin::FighterAir::default();
        air.air.airborne = !state.state_words.grounded;
        air.ground_contact = state.state_words.grounded;
        commands.spawn((
            (
                MrcRemoteFighter {
                    actor_id,
                    pose_sequence: if applied {
                        state.state_words.pose_sequence
                    } else {
                        0
                    },
                    ground_sequence: state.state_words.pose_sequence,
                    desired_pose_id: state.state_words.pose_id,
                    pose_pending: !applied,
                    action_sequence: None,
                    pending_action: None,
                    pending_start: false,
                    applied_character_id: character_id,
                    state_age: 0.0,
                    hit_age: 10.0,
                    pending_stock_state: false,
                },
                crate::plugins::network_plugin::RemoteFighter {
                    player_id: actor_key(u32::from(actor_id)),
                },
                fighter,
                crate::components::fighter::Health {
                    current: 2_000.0,
                    max: 2_000.0,
                },
                crate::components::fighter::Qi::default(),
                crate::components::fighter::CombatState::default(),
                super::stage_plugin::FighterPlacement {
                    position: visible_position,
                    facing: yaw,
                }
                .spawn_bundle(role_movement_mode(state), velocity),
                pose,
                blend,
            ),
            (
                crate::components::fighter::SmState::default(),
                crate::components::fighter::DeathRespawn {
                    spawn_pos: position,
                    ..default()
                },
                air,
                crate::plugins::movement_plugin::FighterDodge::default(),
                crate::plugins::network_plugin::RemoteTransformTarget {
                    translation: position,
                    yaw,
                },
                crate::components::fighter::TargetHitClock::default(),
                crate::plugins::network_plugin::RemoteHitMotionWindow::default(),
                crate::components::fighter::VisualRootPin::default(),
                Visibility::default(),
                InheritedVisibility::default(),
            ),
        ));
    }

    for (actor_id, chain, start) in actions {
        if session.match_projection.dead.contains(&u32::from(actor_id))
            || session.match_projection.left.contains(&u32::from(actor_id))
        {
            continue;
        }
        if local_slot == Some(u32::from(actor_id)) {
            let Ok((fighter, _, _, _, mut combat, mut sm, mut pose, mut blend)) =
                local.single_mut()
            else {
                session.pending_actions.push((actor_id, chain, start));
                continue;
            };

            if pose.pose_id == chain.pose_id
                && matches!(
                    sm.current,
                    meteor_core::FighterState::Attack1
                        | meteor_core::FighterState::Attack2
                        | meteor_core::FighterState::Attack3
                )
            {
                continue;
            }
            if !apply_mrc_discrete_pose(
                assets.as_deref(),
                fighter,
                &mut pose,
                &mut blend,
                chain,
                start,
            ) {
                session.pending_actions.push((actor_id, chain, start));
                continue;
            }

            sm.previous = sm.current;
            sm.current = meteor_core::FighterState::Attack1;
            sm.time_in_state = 0.0;
            sm.locked = true;
            sm.attack_stage = 1;
            sm.attack_queued = None;
            sm.linked_recovery_pose_id = None;
            combat.state = meteor_core::FighterState::Attack1;
            continue;
        }
        let Some((_, mut remote, fighter, mut pose, mut blend, ..)) = remotes
            .iter_mut()
            .find(|(_, remote, ..)| remote.actor_id == actor_id)
        else {
            session.pending_actions.push((actor_id, chain, start));
            continue;
        };
        if remote.action_sequence == Some(chain.sequence)
            || (remote.pose_sequence != chain.sequence
                && meteor_network::role_state::pose_sequence_is_newer(
                    remote.pose_sequence,
                    chain.sequence,
                ))
        {
            continue;
        }
        remote.action_sequence = Some(chain.sequence);
        remote.pose_sequence = chain.sequence;
        remote.desired_pose_id = chain.pose_id;
        remote.pending_action = Some(chain);
        remote.pending_start = start;
        remote.pose_pending = true;
        if apply_mrc_discrete_pose(
            assets.as_deref(),
            &fighter,
            &mut pose,
            &mut blend,
            chain,
            start,
        ) {
            remote.pose_pending = false;
            remote.pending_action = None;
            remote.pending_start = false;
            remote.applied_character_id = fighter.character_id;
        }
    }

    if rebuild_remotes {
        return;
    }
    for (entity, remote, mut fighter, ..) in &mut remotes {
        let Some((weapon, variant)) = session
            .equip_by_slot
            .get(&(remote.actor_id as u32))
            .copied()
        else {
            continue;
        };
        if fighter.weapon != weapon || fighter.weapon_variant != variant {
            fighter.weapon = weapon;
            fighter.weapon_slots = [weapon, 0];
            fighter.weapon_slot = 0;
            fighter.weapon_slot_variants = [variant, 0];
            fighter.weapon_variant = variant;
            commands
                .entity(entity)
                .insert(crate::components::fighter::WeaponVariantChanged);
        }
    }
}

#[allow(clippy::type_complexity)]
fn apply_mrc_remote_life(
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    session: Res<MrcSessionRuntime>,
    mut fighters: Query<(
        &mut MrcRemoteFighter,
        &crate::components::fighter::Fighter,
        &mut crate::components::fighter::Health,
        &mut crate::components::fighter::DeathRespawn,
        &mut crate::components::fighter::CombatState,
        &mut crate::components::fighter::SmState,
        &mut crate::components::fighter::PoseRuntime,
        &mut crate::components::fighter::EntryBlend,
    )>,
) {
    use meteor_core::FighterState;

    for (mut remote, fighter, mut health, mut death, mut combat, mut sm, mut pose, mut blend) in
        &mut fighters
    {
        let recorded_dead = session
            .match_projection
            .dead
            .contains(&u32::from(remote.actor_id));
        if recorded_dead == death.active {
            continue;
        }
        death.active = recorded_dead;
        death.timer = 0.0;
        health.current = if recorded_dead { 0.0 } else { health.max };
        let state = if recorded_dead {
            FighterState::Dead
        } else {
            FighterState::Name
        };
        combat.state = state;
        sm.previous = sm.current;
        sm.current = state;
        sm.locked = recorded_dead;
        if recorded_dead {
            remote.pending_action = None;
            remote.pose_pending = false;
            if let Some(character) = assets
                .as_deref()
                .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
            {
                let death_pose = meteor_combat::hit_reaction::DEATH_POSES
                    .iter()
                    .copied()
                    .find(|pose| character.poses.contains_key(pose))
                    .unwrap_or(meteor_combat::hit_reaction::ENVIRONMENT_HIT_POSE);
                character.apply_pose_with_blend(
                    &mut pose,
                    &mut blend,
                    death_pose,
                    true,
                    None,
                    crate::components::fighter::EntryBlendKind::Generic,
                    true,
                    false,
                    None,
                );
            }
        } else if let Some(character) = assets
            .as_deref()
            .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
        {
            character.apply_pose(&mut pose, remote.desired_pose_id, true);
        }
    }
}

fn apply_mrc_target_clears(
    playback: Res<MrcPlayback>,
    mut session: ResMut<MrcSessionRuntime>,
    remotes: Query<(Entity, &MrcRemoteFighter)>,
    local: Query<Entity, With<LocalFighter>>,
    mut locks: Query<&mut crate::components::fighter::LockOnController>,
) {
    let entity_for = |actor| {
        if playback.local_slot() == Some(actor) {
            local.single().ok()
        } else {
            remotes
                .iter()
                .find(|(_, remote)| u32::from(remote.actor_id) == actor)
                .map(|(entity, _)| entity)
        }
    };
    for (actor, target) in std::mem::take(&mut session.match_projection.cleared_targets) {
        let (Some(actor), Some(target)) = (entity_for(actor), entity_for(target)) else {
            continue;
        };
        if let Ok(mut lock) = locks.get_mut(actor) {
            if lock.lock_entity == Some(target) {
                lock.lock_entity = None;
                lock.system.clear_lock();
            }
            if lock.track_entity == Some(target) {
                lock.track_entity = None;
                lock.system.track_target = None;
            }
        }
    }
}

fn apply_mrc_weapon_grants(
    mut commands: Commands,
    playback: Res<MrcPlayback>,
    mut session: ResMut<MrcSessionRuntime>,
    mut fighters: Query<(
        Entity,
        Option<&MrcRemoteFighter>,
        Option<&LocalFighter>,
        &mut crate::components::fighter::Fighter,
    )>,
) {
    for (actor, item_id) in std::mem::take(&mut session.match_projection.weapon_grants) {
        let Some((weapon, variant)) =
            super::character_plugin::weapon_slot_from_item(i64::from(item_id))
        else {
            continue;
        };
        for (entity, remote, local, mut fighter) in &mut fighters {
            let fighter_actor = remote
                .map(|remote| u32::from(remote.actor_id))
                .or_else(|| local.and_then(|_| playback.local_slot()));
            if fighter_actor != Some(actor) {
                continue;
            }
            if super::item_plugin::store_picked_weapon(&mut fighter, weapon, variant).is_some() {
                commands
                    .entity(entity)
                    .insert(crate::components::fighter::WeaponVariantChanged);
            }
            break;
        }
    }
}

fn restore_mrc_scene(
    playback: Res<MrcPlayback>,
    mut session: ResMut<MrcSessionRuntime>,
    stage: Option<ResMut<super::stage_plugin::StageScriptRuntime>>,
    feed: Option<ResMut<super::system_notice::SystemNoticeFeed>>,
) {
    if !playback.active {
        return;
    }
    let restore = std::mem::take(&mut session.restore_scene);
    if restore {
        if let Some(mut feed) = feed {
            *feed = super::system_notice::SystemNoticeFeed::default();
        }
    }
    let Some(mut stage) = stage else { return };
    let Some(runtime) = stage.script_mut() else {
        return;
    };
    if session.scene_baseline.is_none() {
        session.scene_baseline = Some(runtime.host().instances().to_vec());
    }
    if restore {
        if let Some(instances) = session.scene_baseline.as_ref() {
            let current = runtime.host_mut().instances_mut();
            if current.len() == instances.len() {
                current.clone_from_slice(instances);
            }
        }
    }
}

fn recorded_reaction_facing(
    defender: Vec3,
    facing: f32,
    attacker: Vec3,
    attacks: &[meteor_asset::pos::AttackBlock],
    reaction_pose: u16,
) -> Option<f32> {
    use meteor_combat::hit_reaction::{facing_bucket, FacingBucket};
    let mut allowed = Vec::new();
    for attack in attacks {
        for (bucket, field) in [
            (FacingBucket::Front, attack.target_pose_front),
            (FacingBucket::Back, attack.target_pose_back),
            (FacingBucket::Left, attack.target_pose_left),
            (FacingBucket::Right, attack.target_pose_right),
        ] {
            if field == Some(reaction_pose) && !allowed.contains(&bucket) {
                allowed.push(bucket);
            }
        }
    }
    let current = facing_bucket(attacker.x, attacker.z, defender.x, defender.z, facing);
    if allowed.is_empty() || allowed.contains(&current) {
        return None;
    }
    let to_attacker = (attacker.x - defender.x).atan2(attacker.z - defender.z);
    let wrap = |a: f32| {
        (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
    };
    let rel = wrap(to_attacker - facing);
    let (q, t, e) = (
        std::f32::consts::FRAC_PI_4,
        3.0 * std::f32::consts::FRAC_PI_4,
        0.01,
    );
    allowed
        .into_iter()
        .filter_map(|bucket| {
            let target = match bucket {
                FacingBucket::Front => rel.clamp(-q + e, q - e),
                FacingBucket::Left => rel.clamp(q + e, t - e),
                FacingBucket::Right => rel.clamp(-t + e, -q - e),
                FacingBucket::Back => {
                    if rel >= 0.0 {
                        rel.max(t + e)
                    } else {
                        rel.min(-t - e)
                    }
                }
                FacingBucket::Default => return None,
            };
            let yaw = to_attacker - target;
            Some((wrap(yaw - facing).abs(), facing + wrap(yaw - facing)))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, yaw)| yaw)
}

fn apply_mrc_hit_events(
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    mut playback: ResMut<MrcPlayback>,
    mut session: ResMut<MrcSessionRuntime>,
    mut impacts: Option<ResMut<Messages<CombatImpactEvent>>>,
    mut front_variant: Local<u32>,
    mut local: Query<
        (
            Entity,
            &crate::components::fighter::Fighter,
            &mut crate::components::fighter::PoseRuntime,
            &mut crate::components::fighter::EntryBlend,
            &mut crate::components::fighter::FighterHitReaction,
            &crate::plugins::movement_plugin::FighterAir,
            &mut crate::components::fighter::CombatState,
            &mut crate::components::fighter::SmState,
            Option<(&Transform, &mut MotionState)>,
        ),
        (With<LocalFighter>, Without<MrcRemoteFighter>),
    >,
    mut remotes: Query<
        (
            Entity,
            &mut MrcRemoteFighter,
            &crate::components::fighter::Fighter,
            &mut crate::components::fighter::PoseRuntime,
            &mut crate::components::fighter::EntryBlend,
            &mut crate::components::fighter::FighterHitReaction,
            &crate::plugins::movement_plugin::FighterAir,
            &mut crate::plugins::network_plugin::RemoteHitMotionWindow,
            Option<&Transform>,
        ),
        (With<MrcRemoteFighter>, Without<LocalFighter>),
    >,
) {
    let hits = std::mem::take(&mut session.pending_hits);
    for (guarded, event, event_time) in hits {
        if session
            .match_projection
            .dead
            .contains(&u32::from(event.target_actor_id))
            || session
                .match_projection
                .left
                .contains(&u32::from(event.target_actor_id))
        {
            continue;
        }
        if playback.local_slot() == Some(event.target_actor_id) {
            let Ok((
                entity,
                fighter,
                mut pose,
                mut blend,
                mut reaction,
                air,
                mut combat,
                mut sm,
                mut placement,
            )) = local.single_mut()
            else {
                session.pending_hits.push((guarded, event, event_time));
                continue;
            };

            if !guarded && playback.stock_scene_coordinates() {
                if let (Some((transform, motion)), Some(assets)) =
                    (placement.as_mut(), assets.as_deref())
                {
                    let attackers = remotes
                        .iter()
                        .filter_map(|(_, remote, attacker, attacker_pose, .., at)| {
                            let at = at?;
                            let attacks = &assets
                                .0
                                .get(&format!("p{}", attacker.character_id))?
                                .poses
                                .get(&attacker_pose.pose_id)?
                                .attacks;
                            Some((remote.actor_id, at.translation, attacks.clone()))
                        })
                        .filter(|(actor, ..)| u32::from(*actor) != event.target_actor_id);
                    let best = attackers
                        .filter_map(|(_, at, attacks)| {
                            recorded_reaction_facing(
                                transform.translation,
                                motion.facing,
                                at,
                                &attacks,
                                event.pose_id as u16,
                            )
                            .map(|yaw| (at.distance_squared(transform.translation), yaw))
                        })
                        .min_by(|a, b| a.0.total_cmp(&b.0));
                    if let Some((_, yaw)) = best {
                        motion.facing = yaw;
                    }
                }
            }
            let Some(character) = assets
                .as_deref()
                .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
            else {
                session.pending_hits.push((guarded, event, event_time));
                continue;
            };

            let selector = event.pose_id & 0xffff;
            let effect_pose = if guarded {
                (event.pose_id >> 16) as u16
            } else {
                event.pose_id as u16
            };
            let (pose_id, head_pose_id) = if guarded {
                let strategy = meteor_combat::weapon_strategy::weapon_to_strategy(fighter.weapon);
                let head = meteor_physics::locomotion::guard_pose_for_weapon(strategy);
                let poses: Vec<u16> = character.poses.keys().copied().collect();
                let guard_pose = if selector > 3 {
                    0
                } else {
                    meteor_combat::hit_reaction::guard_reaction_pose_id(
                        strategy,
                        selector,
                        &poses,
                        *front_variant,
                    )
                    .or(head.filter(|id| character.poses.contains_key(id)))
                    .unwrap_or(0)
                };
                if selector == 0 {
                    *front_variant = front_variant.wrapping_add(1);
                }
                (guard_pose, head)
            } else {
                (effect_pose, None)
            };
            let chain = meteor_network::role_state::PoseChain {
                pose_id,
                minimum_seconds: event.minimum_seconds(),
                sequence: 0,
                blend_seconds: 0.05,
            };
            if !apply_mrc_action_pose(assets.as_deref(), fighter, &mut pose, &mut blend, chain) {
                continue;
            }
            let minimum = event.minimum_seconds().max(0.0);
            let (start, end) = character
                .pose_frames
                .get(&pose_id)
                .copied()
                .unwrap_or((0, 0));
            let bucket = match selector {
                1 if guarded => meteor_combat::hit_reaction::FacingBucket::Right,
                2 if guarded => meteor_combat::hit_reaction::FacingBucket::Left,
                3 if guarded => meteor_combat::hit_reaction::FacingBucket::Back,
                _ => meteor_combat::hit_reaction::FacingBucket::Front,
            };
            reaction.begin(
                meteor_combat::hit_reaction::HitReact {
                    kind: if guarded {
                        meteor_combat::hit_reaction::HitReactKind::Guard
                    } else {
                        meteor_combat::hit_reaction::HitReactKind::BeHit
                    },
                    pose_id: Some(pose_id),
                    head_pose_id,
                    lock_remaining: minimum.max(
                        meteor_combat::hit_reaction::pose_duration_seconds(start, end),
                    ),
                    minimum_remaining: minimum,
                    authored_minimum: minimum,
                    motion_scale: event.motion_scale(),
                    down_remaining: 0.0,
                    bucket,
                    environmental: false,
                    selector,
                    rewind_count: 0,
                    play_to_end_pose_id: None,
                },
                air.air.airborne,
            );

            if let Some((_, motion)) = placement.as_ref() {
                reaction.bank_be_hit_carry(motion.velocity, true);
            }
            let state = if guarded {
                meteor_core::FighterState::Guard
            } else {
                meteor_core::FighterState::BeHit
            };
            combat.state = state;
            sm.previous = sm.current;
            sm.current = state;
            sm.time_in_state = 0.0;
            sm.locked = true;
            sm.attack_stage = 0;
            sm.attack_queued = None;
            sm.linked_recovery_pose_id = None;
            sm.exe_ground_mode_ticks = 0;

            if let Some(impacts) = impacts.as_deref_mut() {
                if (90..=117).contains(&effect_pose) && event.context < 12 {
                    impacts.write(CombatImpactEvent {
                        kind: if guarded {
                            CombatImpactKind::Guard
                        } else {
                            CombatImpactKind::Hit
                        },
                        attacker: entity,
                        defender: entity,
                        reaction_pose: Some(effect_pose),
                        effect_strategy: Some(event.context as u8),
                        contact: placement
                            .as_ref()
                            .map_or(Vec3::ZERO, |(transform, _)| transform.translation),
                        direction: Vec3::ZERO,
                    });
                }
            }
            continue;
        }
        let Some((
            target_entity,
            mut remote,
            fighter,
            mut pose,
            mut blend,
            mut reaction,
            air,
            mut window,
            transform,
        )) = remotes
            .iter_mut()
            .find(|(_, remote, ..)| u32::from(remote.actor_id) == event.target_actor_id)
        else {
            session.pending_hits.push((guarded, event, event_time));
            continue;
        };
        let Some(character) = assets
            .as_deref()
            .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
        else {
            session.pending_hits.push((guarded, event, event_time));
            continue;
        };
        let selector = event.pose_id & 0xffff;
        let effect_pose = if guarded {
            (event.pose_id >> 16) as u16
        } else {
            event.pose_id as u16
        };
        let (pose_id, head_pose_id) = if guarded {
            let strategy = meteor_combat::weapon_strategy::weapon_to_strategy(fighter.weapon);
            let head = meteor_physics::locomotion::guard_pose_for_weapon(strategy);
            let poses: Vec<u16> = character.poses.keys().copied().collect();
            let guard_pose = if selector > 3 {
                0
            } else {
                meteor_combat::hit_reaction::guard_reaction_pose_id(
                    strategy,
                    selector,
                    &poses,
                    *front_variant,
                )
                .or(head.filter(|id| character.poses.contains_key(id)))
                .unwrap_or(0)
            };
            if selector == 0 {
                *front_variant = front_variant.wrapping_add(1);
            }
            (guard_pose, head)
        } else {
            (effect_pose, None)
        };
        let chain = meteor_network::role_state::PoseChain {
            pose_id,
            minimum_seconds: event.minimum_seconds(),
            sequence: 0,
            blend_seconds: 0.05,
        };
        if !apply_mrc_action_pose(assets.as_deref(), fighter, &mut pose, &mut blend, chain) {
            continue;
        }
        let minimum = event.minimum_seconds().max(0.0);
        let (start, end) = character
            .pose_frames
            .get(&pose_id)
            .copied()
            .unwrap_or((0, 0));
        let bucket = match selector {
            1 if guarded => meteor_combat::hit_reaction::FacingBucket::Right,
            2 if guarded => meteor_combat::hit_reaction::FacingBucket::Left,
            3 if guarded => meteor_combat::hit_reaction::FacingBucket::Back,
            _ => meteor_combat::hit_reaction::FacingBucket::Front,
        };
        reaction.begin(
            meteor_combat::hit_reaction::HitReact {
                kind: if guarded {
                    meteor_combat::hit_reaction::HitReactKind::Guard
                } else {
                    meteor_combat::hit_reaction::HitReactKind::BeHit
                },
                pose_id: Some(pose_id),
                head_pose_id,
                lock_remaining: minimum.max(meteor_combat::hit_reaction::pose_duration_seconds(
                    start, end,
                )),
                minimum_remaining: minimum,
                authored_minimum: minimum,
                motion_scale: event.motion_scale(),
                down_remaining: 0.0,
                bucket,
                environmental: false,
                selector,
                rewind_count: 0,
                play_to_end_pose_id: None,
            },
            air.air.airborne,
        );
        window.remaining = 0.4;
        remote.hit_age = 0.0;
        remote.desired_pose_id = pose_id;
        remote.pose_pending = false;
        remote.pending_action = None;
        remote.applied_character_id = fighter.character_id;
        if playback.stock_scene_coordinates() {
            playback
                .pending_hit_locks
                .push((target_entity, event.context, event_time));
        }

        if let Some(impacts) = impacts.as_deref_mut() {
            if (90..=117).contains(&effect_pose) && event.context < 12 {
                impacts.write(CombatImpactEvent {
                    kind: if guarded {
                        CombatImpactKind::Guard
                    } else {
                        CombatImpactKind::Hit
                    },
                    attacker: target_entity,
                    defender: target_entity,
                    reaction_pose: Some(effect_pose),
                    effect_strategy: Some(event.context as u8),
                    contact: transform.map_or(Vec3::ZERO, |transform| transform.translation),
                    direction: Vec3::ZERO,
                });
            }
        }
    }
}

fn apply_mrc_property_events(
    playback: Res<MrcPlayback>,
    resources: Option<Res<crate::plugins::combat_plugin::CombatResourceData>>,
    mut session: ResMut<MrcSessionRuntime>,
    mut fighters: Query<(
        &mut crate::components::fighter::FighterStatusEffects,
        Option<&MrcRemoteFighter>,
        Option<&LocalFighter>,
    )>,
) {
    let pending = std::mem::take(&mut session.pending_properties);
    for event in pending {
        let Some(effect) = resources.as_deref().and_then(|resources| {
            (event.category == 2)
                .then(|| resources.0.status_effect(event.resource_id))
                .flatten()
        }) else {
            if resources.is_none() {
                session.pending_properties.push(event);
            }
            continue;
        };
        let mut consumed = false;
        for (mut status, remote, local) in &mut fighters {
            let actor = remote
                .map(|remote| u32::from(remote.actor_id))
                .or_else(|| local.and_then(|_| playback.local_slot()));
            if actor == Some(event.actor_id) {
                status.apply(effect);
                consumed = true;
                break;
            }
        }
        if !consumed {
            session.pending_properties.push(event);
        }
    }
}

fn smooth_mrc_remote_fighters(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    playback: Res<MrcPlayback>,
    waypoints: Option<Res<meteor_scene::stage_loader::WaypointGraph>>,
    mut remotes: Query<(
        &mut MrcRemoteFighter,
        &mut Transform,
        &mut MotionState,
        &mut crate::plugins::network_plugin::RemoteTransformTarget,
    )>,
) {
    if !playback.active || playback.paused {
        return;
    }
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    if dt <= 0.0 {
        return;
    }
    let scene_frame = mrc_scene_frame(&playback, waypoints.as_deref());
    for (mut remote, mut transform, mut motion, mut target) in &mut remotes {
        let skip_motion = std::mem::take(&mut remote.pending_stock_state);
        if skip_motion {
            remote.state_age = 0.0;
        } else {
            remote.state_age += dt;
        }
        remote.hit_age += dt;
        let correction = target.translation - transform.translation;
        let factor = meteor_network::role_state::role_position_correction_factor(
            correction.length_squared(),
            motion.velocity.length_squared(),
            dt,
            remote.hit_age,
        );
        transform.translation = transform.translation.lerp(target.translation, factor);
        motion.facing = meteor_network::role_state::yaw_from_forward(
            meteor_network::role_state::correct_role_forward(
                meteor_network::role_state::forward_from_yaw(motion.facing),
                meteor_network::role_state::forward_from_yaw(target.yaw),
                dt,
                remote.hit_age,
            ),
        );
        if !skip_motion {
            let delta = meteor_network::role_state::remote_velocity_delta(
                remote.state_age,
                remote.hit_age,
                meteor_network::role_state::RoleVec3 {
                    x: motion.velocity.x,
                    y: motion.velocity.y,
                    z: motion.velocity.z,
                },
                dt,
            );
            let delta = Vec3::new(delta.x, delta.y, delta.z);
            transform.translation += delta;
            target.translation += delta;
        }
        if remote.hit_age > 0.5 {
            if let Some(position) = playback
                .burst_tracks
                .get(&remote.actor_id)
                .and_then(|bursts| {
                    bursts.iter().find(|burst| {
                        burst.start_time <= playback.elapsed && playback.elapsed <= burst.end_time
                    })
                })
                .map(|burst| mrc_scene_pose(burst.position_at(playback.elapsed), scene_frame))
            {
                transform.translation = position;
            }
        }
        transform.rotation = Quat::IDENTITY;
    }
}

fn sync_mrc_remote_pose(
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    mut remotes: Query<(
        &mut MrcRemoteFighter,
        &crate::components::fighter::Fighter,
        &mut crate::components::fighter::PoseRuntime,
        &mut crate::components::fighter::EntryBlend,
        Option<&crate::plugins::network_plugin::RemoteHitMotionWindow>,
    )>,
) {
    for (mut remote, fighter, mut pose, mut blend, hit_window) in &mut remotes {
        if (remote.pose_pending || fighter.character_id != remote.applied_character_id)
            && !(hit_window.is_some_and(|window| window.remaining > 0.0)
                && remote.pending_action.is_none())
            && match remote.pending_action {
                Some(chain) => apply_mrc_discrete_pose(
                    assets.as_deref(),
                    fighter,
                    &mut pose,
                    &mut blend,
                    chain,
                    remote.pending_start,
                ),
                None => apply_mrc_remote_pose(
                    assets.as_deref(),
                    fighter,
                    &mut pose,
                    &mut blend,
                    remote.desired_pose_id,
                ),
            }
        {
            remote.pose_pending = false;
            remote.pending_action = None;
            remote.pending_start = false;
            remote.applied_character_id = fighter.character_id;
        }
    }
}

fn apply_mrc_identities(
    mut commands: Commands,
    playback: Res<MrcPlayback>,
    session: Res<MrcSessionRuntime>,
    fighters: Query<(
        Entity,
        &crate::components::fighter::Fighter,
        Option<&MrcRemoteFighter>,
        Option<&LocalFighter>,
    )>,
) {
    if !playback.active {
        return;
    }
    for (entity, fighter, remote, local) in &fighters {
        let actor_id = remote.map(|remote| remote.actor_id).or_else(|| {
            local
                .and_then(|_| playback.local_slot())
                .and_then(|slot| u16::try_from(slot).ok())
        });
        let Some(character_id) = actor_id
            .and_then(|id| session.character_by_slot.get(&id))
            .copied()
        else {
            continue;
        };
        if fighter.character_id != character_id {
            commands
                .entity(entity)
                .insert(crate::components::fighter::CharacterVariantChanged {
                    requested_character_id: character_id,
                });
        }
    }
}

fn mrc_code_to_key(code: u32, keymap: &Keymap) -> Option<InputBinding> {
    Some(match code {
        0x57 => keymap.move_up,
        0x53 => keymap.move_down,
        0x41 => keymap.move_left,
        0x44 => keymap.move_right,
        0x1000 => keymap.attack,
        0x1200 => keymap.jump,
        0x20 => keymap.guard,
        0x43 => keymap.crouch,
        0x45 => keymap.weapon_switch,
        0x47 => keymap.drop_weapon,
        0x48 => keymap.drop_item,
        0x54 => keymap.cure,
        0x59 => keymap.fake,
        0x46 => keymap.qi_burst,
        0x11 => keymap.dodge,
        0x12 => keymap.dodge,
        0x26 => keymap.camera_up,
        0x28 => keymap.camera_down,
        0x25 => keymap.left_turn,
        0x27 => keymap.right_turn,
        0x51 => keymap.lock_toggle,
        _ => return None,
    })
}

fn mrc_code_to_synthetic_action(code: u32) -> Option<SyntheticAction> {
    Some(match code {
        0x1000 => SyntheticAction::Attack,
        0x1200 => SyntheticAction::Jump,
        0x20 => SyntheticAction::Guard,
        0x43 => SyntheticAction::Crouch,
        0x45 => SyntheticAction::WeaponSwitch,
        0x54 => SyntheticAction::Cure,
        0x59 => SyntheticAction::Fake,
        0x46 => SyntheticAction::QiBurst,
        0x51 => SyntheticAction::LockToggle,
        _ => return None,
    })
}

fn set_replay_input(
    keys: &mut ButtonInput<KeyCode>,
    mouse: &mut ButtonInput<MouseButton>,
    binding: InputBinding,
    pressed: bool,
) {
    match (binding, pressed) {
        (InputBinding::Unbound, _) => {}
        (InputBinding::Key(key), true) => keys.press(key),
        (InputBinding::Key(key), false) => keys.release(key),
        (InputBinding::Mouse(button), true) => mouse.press(button),
        (InputBinding::Mouse(button), false) => mouse.release(button),
    }
}

fn replay_dodge_direction(
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    keymap: &Keymap,
) -> Dir {
    if keymap.move_up.pressed(keys, mouse) {
        Dir::Up
    } else if keymap.move_down.pressed(keys, mouse) {
        Dir::Down
    } else if keymap.move_right.pressed(keys, mouse) {
        Dir::Right
    } else if keymap.move_left.pressed(keys, mouse) {
        Dir::Left
    } else {
        Dir::Up
    }
}

fn inject_replay_binding(
    keys: &mut ButtonInput<KeyCode>,
    mouse: &mut ButtonInput<MouseButton>,
    synthetic: &mut SyntheticInputStream,
    binding: InputBinding,
    pressed: bool,
    event_seconds: Option<f64>,
) {
    set_replay_input(keys, mouse, binding, pressed);
    synthetic.push_replay_binding(binding, pressed, event_seconds);
}

fn set_replay_held_binding(
    keys: &mut ButtonInput<KeyCode>,
    mouse: &mut ButtonInput<MouseButton>,
    synthetic: &mut SyntheticInputStream,
    binding: InputBinding,
    pressed: bool,
) {
    if binding.pressed(keys, mouse) == pressed {
        return;
    }
    inject_replay_binding(keys, mouse, synthetic, binding, pressed, None);
}

fn clear_replay_edge(
    keys: &mut ButtonInput<KeyCode>,
    mouse: &mut ButtonInput<MouseButton>,
    binding: InputBinding,
) {
    match binding {
        InputBinding::Unbound => {}
        InputBinding::Key(key) => {
            keys.clear_just_pressed(key);
            keys.clear_just_released(key);
        }
        InputBinding::Mouse(button) => {
            mouse.clear_just_pressed(button);
            mouse.clear_just_released(button);
        }
    }
}

fn continuous_replay_inputs(keymap: &Keymap) -> Vec<InputBinding> {
    let mut inputs = vec![
        keymap.move_up,
        keymap.move_down,
        keymap.move_left,
        keymap.move_right,
        keymap.camera_up,
        keymap.camera_down,
        keymap.left_turn,
        keymap.right_turn,
    ];
    inputs.sort_by_key(|binding| binding.name());
    inputs.dedup();
    inputs
}

fn continuous_mrc_binding(code: u32, keymap: &Keymap) -> Option<InputBinding> {
    matches!(code, 0x57 | 0x53 | 0x41 | 0x44 | 0x26 | 0x28 | 0x25 | 0x27)
        .then(|| mrc_code_to_key(code, keymap))
        .flatten()
}

fn apply_mrc_packed_input(
    packed: i32,
    keymap: &Keymap,
    keys: &mut ButtonInput<KeyCode>,
    mouse: &mut ButtonInput<MouseButton>,
    synthetic: &mut SyntheticInputStream,
) {
    let forward = (packed & 0xff) - 1;
    let strafe = ((packed >> 8) & 0xff) - 1;
    set_replay_held_binding(keys, mouse, synthetic, keymap.move_up, forward > 0);
    set_replay_held_binding(keys, mouse, synthetic, keymap.move_down, forward < 0);
    set_replay_held_binding(keys, mouse, synthetic, keymap.move_right, strafe > 0);
    set_replay_held_binding(keys, mouse, synthetic, keymap.move_left, strafe < 0);
}

#[allow(clippy::too_many_arguments)]
fn apply_replay_seek(
    mut request: ResMut<ReplaySeekRequest>,
    keymap: Res<Keymap>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut player: ResMut<ReplayPlayer>,
    mut playback: ResMut<MrcPlayback>,
    waypoints: Option<Res<meteor_scene::stage_loader::WaypointGraph>>,
    mut session: ResMut<MrcSessionRuntime>,
    mut frame_override: ResMut<MrcFrameOverride>,
    mut basis: ResMut<CameraBasis>,
    mut fighters: Query<(&mut Transform, &mut MotionState), With<LocalFighter>>,
    mut local_blends: Query<&mut crate::components::fighter::EntryBlend, With<LocalFighter>>,
) {
    let Some(target) = request.0.take() else {
        return;
    };

    let continuous = continuous_replay_inputs(&keymap);
    for binding in &continuous {
        match *binding {
            InputBinding::Unbound => {}
            InputBinding::Key(key) => keys.reset(key),
            InputBinding::Mouse(button) => mouse.reset(button),
        }
    }

    if playback.active {
        let t = target.clamp(0.0, playback.duration());
        playback.frame_cursor = playback.frame_times.partition_point(|frame| *frame <= t);
        playback.frame_wall = f64::from(t);
        playback.frame_target = None;
        let scene_frame = mrc_scene_frame(&playback, waypoints.as_deref());
        session.pending.clear();
        session.pending_actions.clear();
        session.pending_hits.clear();
        playback.pending_hit_locks.clear();
        session.pending_properties.clear();
        session.character_by_slot.clear();
        session.equip_by_slot.clear();
        session.event_counts.clear();
        session.unhandled_counts.clear();
        session.rebuild_remotes = true;
        let (index_a, index_b, index_c, index_d, index_e, packed, pose, facing) = {
            let Some(doc) = playback.doc.as_ref() else {
                return;
            };
            let mut i_a = 0;
            while i_a < doc.queue_a.len() && doc.queue_a[i_a].time <= t {
                if !matches!(doc.queue_a[i_a].id, 11 | 14 | 50 | 51 | 61) {
                    route_mrc_session_entry(&doc.queue_a[i_a], &mut session);
                }
                i_a += 1;
            }
            let mut held = HashMap::new();
            let mut i_b = 0;
            while i_b < doc.queue_b.len() && doc.queue_b[i_b].time <= t {
                let entry = &doc.queue_b[i_b];
                if let Some(binding) = continuous_mrc_binding(entry.code, &keymap) {
                    held.insert(binding, entry.value != 0);
                }
                i_b += 1;
            }
            for (binding, pressed) in held {
                set_replay_input(&mut keys, &mut mouse, binding, pressed);
            }
            let mut packed = None;
            let mut i_c = 0;
            while i_c < doc.queue_c.len() && doc.queue_c[i_c].time <= t {
                packed = Some(doc.queue_c[i_c].packed as i32);
                i_c += 1;
            }
            let mut pose = None;
            let mut i_d = 0;
            while i_d < doc.queue_d.len() && doc.queue_d[i_d].time <= t {
                pose = decode_mrc_pose(&doc.queue_d[i_d])
                    .map(|position| mrc_scene_pose(position, scene_frame))
                    .or(pose);
                i_d += 1;
            }
            let mut facing = None;
            let mut i_e = 0;
            while i_e < doc.queue_e.len() && doc.queue_e[i_e].time <= t {
                facing = decode_mrc_facing(&doc.queue_e[i_e])
                    .map(|yaw| mrc_scene_facing(yaw, scene_frame))
                    .or(facing);
                i_e += 1;
            }
            (i_a, i_b, i_c, i_d, i_e, packed, pose, facing)
        };
        if let Some(packed) = packed {
            let mut ignored_edges = SyntheticInputStream::default();
            apply_mrc_packed_input(packed, &keymap, &mut keys, &mut mouse, &mut ignored_edges);
            playback.prev_c = packed;
        } else {
            playback.prev_c = 0;
        }
        playback.elapsed = t;
        if let Ok(mut blend) = local_blends.single_mut() {
            blend.started_at_seconds = None;
            blend.elapsed = 0.0;
        }
        playback.index_a = index_a;
        playback.index_b = index_b;
        playback.index_c = index_c;
        playback.index_d = index_d;
        playback.index_e = index_e;
        frame_override.position_applied = false;
        frame_override.facing_applied = false;
        if let Ok((mut transform, mut motion)) = fighters.single_mut() {
            if let Some(position) = pose {
                transform.translation = position;
                motion.velocity.x = 0.0;
                motion.velocity.z = 0.0;
                frame_override.position_applied = true;
            }
            if let Some(yaw) = facing {
                motion.facing = yaw;
                basis.yaw = yaw;
                frame_override.facing_applied = true;
            }
        }
    } else if player.active {
        let t = target.clamp(0.0, player.duration());
        let mut held = HashMap::new();
        let mut index = 0;
        while index < player.events.len() && player.events[index].time <= t {
            let event = &player.events[index];
            if continuous.contains(&event.key) {
                held.insert(event.key, event.pressed);
            }
            index += 1;
        }
        for (binding, pressed) in held {
            set_replay_input(&mut keys, &mut mouse, binding, pressed);
        }
        player.elapsed = t;
        player.index = index;
    }

    for binding in continuous {
        clear_replay_edge(&mut keys, &mut mouse, binding);
    }
}

fn record_inputs(keymap: &Keymap) -> Vec<InputBinding> {
    KeyFieldLike::all(keymap)
}

struct KeyFieldLike;
impl KeyFieldLike {
    fn all(keymap: &Keymap) -> Vec<InputBinding> {
        use crate::app_state::KeyField;
        let mut inputs: Vec<InputBinding> = KeyField::BINDING_ACTIONS
            .iter()
            .map(|f| f.get(keymap))
            .collect();
        inputs.push(InputBinding::Key(KeyCode::Escape));
        inputs.sort_by_key(|b| b.name());
        inputs.dedup();
        inputs
    }
}

const REPLAY_MAGIC: &[u8; 5] = b"MREP1";

pub const REPLAY_DIR: &str = "replays";

#[derive(bevy::ecs::schedule::ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct MrcSubstep;

#[derive(Resource)]
pub(super) struct RecordedSubsteps;

pub(super) fn use_recorded_substeps(
    schedule: Option<Res<RecordedSubsteps>>,
    installed: Option<Res<super::FrameClockInstalled>>,
    clock: Option<Res<super::FrameClock>>,
    playback: Option<Res<MrcPlayback>>,
) -> bool {
    schedule.is_some()
        && installed.is_some()
        && clock.is_some_and(|clock| clock.sim_dt > 0.0 && !clock.replay_steps.is_empty())
        && playback.is_some_and(|playback| playback.active && !playback.paused)
}

pub struct ReplayPlugin;

impl Plugin for ReplayPlugin {
    fn build(&self, app: &mut App) {
        use super::input_plugin::{capture_local_command_input, capture_local_hold_inputs};
        use bevy::ecs::schedule::common_conditions::not;
        app.insert_resource(RecordedSubsteps)
            .init_schedule(MrcSubstep)
            .add_systems(
                MrcSubstep,
                (
                    tick_mrc_playback,
                    apply_mrc_session_events,
                    apply_mrc_remote_life,
                    apply_mrc_target_clears,
                    apply_mrc_weapon_grants,
                    apply_mrc_hit_events,
                    apply_mrc_property_events,
                    apply_mrc_identities,
                    smooth_mrc_remote_fighters,
                    restore_mrc_scene,
                    capture_mrc_held_input,
                    capture_local_command_input,
                    capture_local_hold_inputs,
                    sync_mrc_remote_pose,
                )
                    .chain(),
            );
        #[cfg(not(target_arch = "wasm32"))]
        trace::install(app);
        app.init_resource::<ReplayRecorder>()
            .init_resource::<ReplayPlayer>()
            .init_resource::<ReplaySeekRequest>()
            .init_resource::<MrcPlayback>()
            .init_resource::<MrcHeldInput>()
            .init_resource::<MrcFrameOverride>()
            .init_resource::<MrcSessionRuntime>()
            .init_resource::<SyntheticInputStream>()
            .add_systems(
                OnEnter(AppState::Playing),
                reset_replay_session.run_if(crate::plugins::starts_fresh_playing_session),
            )
            .add_systems(
                OnExit(AppState::Playing),
                (save_recording, stop_mrc_playback)
                    .chain()
                    .run_if(crate::plugins::ends_playing_session),
            )
            .add_systems(
                Update,
                isolate_mrc_hardware_input
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .before(crate::plugins::camera_plugin::CameraInputSet)
                    .before(tick_mrc_playback)
                    .before(apply_replay_seek)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                Update,
                tick_mrc_playback
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .after(crate::plugins::camera_plugin::prepare_move_basis)
                    .before(crate::plugins::input_plugin::capture_local_command_input)
                    .run_if(in_state(AppState::Playing))
                    .run_if(crate::plugins::simulation_frame)
                    .run_if(not(use_recorded_substeps)),
            )
            .add_systems(
                Update,
                apply_replay_seek
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .after(tick_mrc_playback)
                    .before(apply_mrc_session_events)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                Update,
                (
                    apply_mrc_session_events,
                    apply_mrc_remote_life,
                    apply_mrc_target_clears,
                    apply_mrc_weapon_grants,
                    apply_mrc_hit_events,
                    apply_mrc_property_events,
                    apply_mrc_identities,
                    smooth_mrc_remote_fighters.run_if(crate::plugins::simulation_frame),
                )
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .after(apply_replay_seek)
                    .before(crate::plugins::input_plugin::capture_local_command_input)
                    .before(crate::plugins::ai_plugin::drive_npc_ai)
                    .run_if(in_state(AppState::Playing))
                    .run_if(not(use_recorded_substeps)),
            )
            .add_systems(
                Update,
                restore_mrc_scene
                    .after(apply_mrc_session_events)
                    .before(super::stage_plugin::tick_stage_script)
                    .run_if(in_state(AppState::Playing))
                    .run_if(not(use_recorded_substeps)),
            )
            .add_systems(
                Update,
                replay_playback
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .after(apply_replay_seek)
                    .before(crate::plugins::input_plugin::capture_local_command_input)
                    .before(crate::plugins::ai_plugin::drive_npc_ai)
                    .run_if(in_state(AppState::Playing))
                    .run_if(crate::plugins::simulation_frame),
            )
            .add_systems(
                Update,
                capture_mrc_held_input
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .after(apply_replay_seek)
                    .after(replay_playback)
                    .before(crate::plugins::input_plugin::capture_local_command_input)
                    .run_if(in_state(AppState::Playing))
                    .run_if(not(use_recorded_substeps)),
            )
            .add_systems(
                Update,
                replay_record
                    .after(crate::plugins::FighterFrameSet::CameraFinalize)
                    .run_if(in_state(AppState::Playing))
                    .run_if(crate::plugins::simulation_frame),
            )
            .add_systems(
                Update,
                sync_mrc_remote_pose
                    .in_set(crate::plugins::FighterFrameSet::PoseDecision)
                    .run_if(in_state(AppState::Playing))
                    .run_if(crate::plugins::simulation_frame)
                    .run_if(not(use_recorded_substeps)),
            );
    }
}

fn mrc_stage_entry(stage_id: &str) -> Option<meteor_asset::mrc::MrcQueueAEntry> {
    let stage_number = stage_id.strip_prefix("sn")?.parse::<u32>().ok()?;
    if stage_number == 0 || stage_number > 99 {
        return None;
    }
    let mut blob = vec![0; 16];
    blob[0..4].copy_from_slice(format!("sn{stage_number:02}").as_bytes());
    blob[4..8].copy_from_slice(&1u32.to_le_bytes());
    blob[8..12].copy_from_slice(&stage_number.to_le_bytes());
    Some(meteor_asset::mrc::MrcQueueAEntry {
        id: 10,
        time: 0.0,
        blob,
    })
}

fn weapon_item_from_slot(weapon: u8, variant: u8) -> u32 {
    match (weapon, variant) {
        (1..=9, 0..=4) => u32::from(variant) * 9 + u32::from(weapon),
        (1, 5) => 46,
        (2, 5) => 59,
        (10, 0..=3) => 47 + u32::from(variant),
        (11, 0..=3) => 51 + u32::from(variant),
        (12, 0..=3) => 55 + u32::from(variant),
        _ => 0,
    }
}

fn stop_mrc_playback(
    keymap: Res<Keymap>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut playback: ResMut<MrcPlayback>,
    mut held: ResMut<MrcHeldInput>,
    mut frame_override: ResMut<MrcFrameOverride>,
    session: Res<MrcSessionRuntime>,
    net: Option<ResMut<super::network_plugin::NetState>>,
) {
    if playback.active {
        if let Some(mut net) = net {
            MatchProjection::clear_net(&mut net);
        }
        for binding in record_inputs(&keymap) {
            set_replay_input(&mut keys, &mut mouse, binding, false);
        }
    }
    playback.active = false;
    playback.paused = false;
    playback.pending_hit_locks.clear();
    held.bindings.clear();
    frame_override.position_applied = false;
    frame_override.facing_applied = false;
    if !session.unhandled_counts.is_empty() {
        let mut counts: Vec<(u32, u64)> = session
            .unhandled_counts
            .iter()
            .map(|(id, count)| (*id, *count))
            .collect();
        counts.sort_unstable_by_key(|(id, _)| *id);
        info!("mrc QueueA routed but unhandled event counts: {counts:?}");
    }
}

fn reset_replay_session(
    mut recorder: ResMut<ReplayRecorder>,
    mut player: ResMut<ReplayPlayer>,
    mut session: ResMut<MrcSessionRuntime>,
    mut held: ResMut<MrcHeldInput>,
    request: Res<crate::app_state::LaunchRequest>,
) {
    held.bindings.clear();

    recorder.active = recorder.auto;
    recorder.elapsed = 0.0;
    recorder.events.clear();
    recorder.queue_a.clear();
    recorder.queue_d.clear();
    recorder.queue_e.clear();
    recorder.mrc_state = [0; meteor_asset::mrc::MRC_STATE_SIZE];
    recorder.mrc_header_captured = false;
    if let Some(entry) = mrc_stage_entry(&request.stage_id) {
        recorder.queue_a.push(entry);
    }
    recorder.prev.clear();
    recorder.last_vitals_key = None;
    recorder.last_pose_key = None;
    recorder.last_pose_time = 0.0;
    player.elapsed = 0.0;
    player.index = 0;
    *session = MrcSessionRuntime::default();
    if player.active {
        info!(
            "replay playback started: {} ({} events)",
            player.file_name,
            player.events.len()
        );
    }
}

fn replay_record(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keymap: Res<Keymap>,
    mut recorder: ResMut<ReplayRecorder>,
    fighters: Query<
        (
            &Transform,
            &MotionState,
            &crate::components::fighter::Fighter,
            &crate::components::fighter::Health,
            &crate::components::fighter::Qi,
        ),
        With<LocalFighter>,
    >,
) {
    if !recorder.active {
        return;
    }
    recorder.elapsed += crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let now = recorder.elapsed;
    for key in record_inputs(&keymap) {
        let pressed = key.pressed(&keys, &mouse);
        let prev = recorder.prev.get(&key).copied().unwrap_or(false);
        if pressed != prev {
            recorder.events.push(ReplayEvent {
                time: now,
                key,
                pressed,
            });
            recorder.prev.insert(key, pressed);
        }
    }
    if let Ok((transform, motion, fighter, health, qi)) = fighters.single() {
        if !recorder.mrc_header_captured {
            let item_id = weapon_item_from_slot(fighter.weapon, fighter.weapon_variant);
            recorder.mrc_state[0..4].copy_from_slice(&item_id.to_le_bytes());
            recorder.mrc_state[4..8].copy_from_slice(&1u32.to_le_bytes());
            recorder.mrc_header_captured = true;
        }
        record_mrc_pose_sample(&mut recorder, now, transform.translation, motion.facing);
        record_mrc_vitals_sample(&mut recorder, now, health.current, health.max, qi.current);
    }
}

const MRC_POSE_HEARTBEAT_SECONDS: f32 = 0.5;

fn record_mrc_pose_sample(recorder: &mut ReplayRecorder, time: f32, position: Vec3, yaw: f32) {
    if !position.is_finite() || !yaw.is_finite() {
        return;
    }
    let facing_x = yaw.sin();
    let facing_z = yaw.cos();
    let key = [
        (position.x * 1_000.0).round() as i32,
        (position.y * 1_000.0).round() as i32,
        (position.z * 1_000.0).round() as i32,
        (facing_x * 10_000.0).round() as i32,
        (facing_z * 10_000.0).round() as i32,
    ];
    let heartbeat = time - recorder.last_pose_time >= MRC_POSE_HEARTBEAT_SECONDS;
    if recorder.last_pose_key == Some(key) && !heartbeat {
        return;
    }
    recorder.last_pose_key = Some(key);
    recorder.last_pose_time = time;
    recorder.queue_d.push(meteor_asset::mrc::MrcQueueDEntry {
        a: (position.x / MODEL_SCALE).to_bits(),
        b: (position.z / MODEL_SCALE).to_bits(),
        c: (position.y / MODEL_SCALE).to_bits(),
        time,
    });
    recorder.queue_e.push(meteor_asset::mrc::MrcQueueEEntry {
        a: facing_x.to_bits(),
        b: facing_z.to_bits(),
        time,
    });
}

fn record_mrc_vitals_sample(
    recorder: &mut ReplayRecorder,
    time: f32,
    hp: f32,
    max_hp: f32,
    anger: f32,
) {
    fn quantize(value: f32) -> u16 {
        if value.is_nan() {
            0
        } else {
            value.round().clamp(0.0, u16::MAX as f32) as u16
        }
    }

    let key = [quantize(hp), quantize(max_hp), quantize(anger)];
    if recorder.last_vitals_key == Some(key) {
        return;
    }
    recorder.last_vitals_key = Some(key);
    let mut blob = Vec::with_capacity(6);
    for value in key {
        blob.extend_from_slice(&value.to_le_bytes());
    }
    recorder
        .queue_a
        .push(meteor_asset::mrc::MrcQueueAEntry { id: 25, time, blob });
}

fn replay_playback(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    keymap: Res<Keymap>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut synthetic: ResMut<SyntheticInputStream>,
    mut player: ResMut<ReplayPlayer>,
) {
    if !player.active || player.paused {
        return;
    }
    player.elapsed += crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    synthetic.set_frame_clock_seconds(f64::from(player.elapsed));
    while player.index < player.events.len() && player.events[player.index].time <= player.elapsed {
        let event = &player.events[player.index];
        if event.key == keymap.dodge {
            if event.pressed {
                synthetic.push_dodge(
                    replay_dodge_direction(&keys, &mouse, &keymap),
                    keymap.guard.pressed(&keys, &mouse),
                );
            }
        } else {
            inject_replay_binding(
                &mut keys,
                &mut mouse,
                &mut synthetic,
                event.key,
                event.pressed,
                Some(f64::from(event.time)),
            );
        }
        player.index += 1;
    }
}

fn save_recording(mut recorder: ResMut<ReplayRecorder>, keymap: Res<Keymap>) {
    if !recorder.active
        || (recorder.events.is_empty()
            && recorder.queue_a.is_empty()
            && recorder.queue_d.is_empty())
    {
        return;
    }
    let dir = Path::new(REPLAY_DIR);
    let _ = std::fs::create_dir_all(dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = format!("rec_{stamp}.rep");
    let path = dir.join(&name);
    let bytes = encode_replay(&recorder.events);
    match std::fs::write(&path, bytes) {
        Ok(()) => info!(
            "replay saved: {} ({} events, {:.1}s)",
            path.display(),
            recorder.events.len(),
            recorder.elapsed
        ),
        Err(error) => warn!("replay save failed: {error}"),
    }

    let mrc_bytes = export_mrc_with_samples(
        &recorder.events,
        &keymap,
        recorder.mrc_state,
        &recorder.queue_a,
        &recorder.queue_d,
        &recorder.queue_e,
    );
    let mrc_path = dir.join(format!("rec_{stamp}.mrc"));
    match std::fs::write(&mrc_path, mrc_bytes) {
        Ok(()) => info!("mrc saved: {}", mrc_path.display()),
        Err(error) => warn!("mrc save failed: {error}"),
    }
    recorder.active = false;
}

pub fn export_mrc(events: &[ReplayEvent], keymap: &Keymap) -> Vec<u8> {
    export_mrc_with_samples(
        events,
        keymap,
        [0; meteor_asset::mrc::MRC_STATE_SIZE],
        &[],
        &[],
        &[],
    )
}

fn export_mrc_with_samples(
    events: &[ReplayEvent],
    keymap: &Keymap,
    state: [u8; meteor_asset::mrc::MRC_STATE_SIZE],
    queue_a: &[meteor_asset::mrc::MrcQueueAEntry],
    queue_d: &[meteor_asset::mrc::MrcQueueDEntry],
    queue_e: &[meteor_asset::mrc::MrcQueueEEntry],
) -> Vec<u8> {
    let doc = meteor_asset::mrc::MrcDocument {
        magic: meteor_asset::mrc::MRC_MAGIC,
        header_size: meteor_asset::mrc::MRC_HEADER_SIZE,
        mark0: 0,
        mark1: 0,
        tail: 0,
        state,
        meta: [0; 8],
        queue_a: queue_a.to_vec(),
        queue_b: events
            .iter()
            .filter_map(|event| {
                key_to_mrc_code(event.key, keymap).map(|code| meteor_asset::mrc::MrcQueueBEntry {
                    code,
                    value: u32::from(event.pressed),
                    time: event.time,
                })
            })
            .collect(),
        queue_c: Vec::new(),
        queue_d: queue_d.to_vec(),
        queue_e: queue_e.to_vec(),
    };
    meteor_asset::mrc::encode_mrc(&doc)
}

fn key_to_mrc_code(key: InputBinding, keymap: &Keymap) -> Option<u32> {
    let table: &[(InputBinding, u32)] = &[
        (keymap.move_up, 0x57),
        (keymap.move_down, 0x53),
        (keymap.move_left, 0x41),
        (keymap.move_right, 0x44),
        (keymap.attack, 0x1000),
        (keymap.jump, 0x1200),
        (keymap.guard, 0x20),
        (keymap.crouch, 0x43),
        (keymap.weapon_switch, 0x45),
        (keymap.drop_weapon, 0x47),
        (keymap.drop_item, 0x48),
        (keymap.cure, 0x54),
        (keymap.fake, 0x59),
        (keymap.qi_burst, 0x46),
        (keymap.dodge, 0x12),
        (keymap.camera_up, 0x26),
        (keymap.camera_down, 0x28),
        (keymap.left_turn, 0x25),
        (keymap.right_turn, 0x27),
        (keymap.lock_toggle, 0x51),
    ];
    table
        .iter()
        .find(|(bound, _)| *bound == key)
        .map(|(_, code)| *code)
}

pub fn encode_replay(events: &[ReplayEvent]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(REPLAY_MAGIC);
    bytes.extend_from_slice(&(events.len() as u32).to_le_bytes());
    for event in events {
        bytes.extend_from_slice(&event.time.to_le_bytes());
        let name = event.key.name();
        bytes.push(name.len() as u8);
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(event.pressed as u8);
    }
    bytes
}

pub fn decode_replay(bytes: &[u8]) -> Option<Vec<ReplayEvent>> {
    if bytes.len() < 9 || &bytes[..5] != REPLAY_MAGIC {
        return None;
    }
    let count = u32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]) as usize;
    let mut events = Vec::with_capacity(count);
    let mut offset = 9usize;
    for _ in 0..count {
        if offset + 4 > bytes.len() {
            return None;
        }
        let time = f32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]);
        offset += 4;
        if offset >= bytes.len() {
            return None;
        }
        let name_len = bytes[offset] as usize;
        offset += 1;
        if offset + name_len + 1 > bytes.len() {
            return None;
        }
        let name = String::from_utf8_lossy(&bytes[offset..offset + name_len]).to_string();
        offset += name_len;
        let pressed = bytes[offset] != 0;
        offset += 1;
        let key = InputBinding::from_name(&name)?;
        events.push(ReplayEvent { time, key, pressed });
    }
    Some(events)
}

pub fn load_latest_replay(dir: &Path) -> Option<(String, Vec<ReplayEvent>)> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut latest: Option<(String, std::time::SystemTime)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rep") {
            continue;
        }
        let modified = entry.metadata().ok()?.modified().ok()?;
        if latest.as_ref().map(|(_, t)| modified > *t).unwrap_or(true) {
            latest = Some((path.to_string_lossy().to_string(), modified));
        }
    }
    let (path, _) = latest?;
    let bytes = std::fs::read(&path).ok()?;
    let events = decode_replay(&bytes)?;
    Some((path, events))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_recorded_frames_sample_each_input_before_the_controller() {
        use super::super::{fighter_frame, FrameClock, FrameClockInstalled};
        use meteor_asset::mrc::{MrcDocument, MrcQueueBEntry, MrcQueueDEntry};

        #[derive(Resource, Default)]
        struct Samples(Vec<(f32, bool)>);

        let mut app = App::new();
        super::super::configure_fighter_frame_sets(&mut app);
        fighter_frame::install(&mut app);
        let mut clock = FrameClock::default();
        clock.replay_steps = vec![0.01, 0.01];
        clock.sim_dt = 0.02;
        app.init_resource::<Time>()
            .init_resource::<Keymap>()
            .init_resource::<CameraBasis>()
            .init_resource::<ReplaySeekRequest>()
            .init_resource::<MrcFrameOverride>()
            .init_resource::<MrcSessionRuntime>()
            .init_resource::<SyntheticInputStream>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<Samples>()
            .insert_resource(clock)
            .insert_resource(FrameClockInstalled)
            .insert_resource(RecordedSubsteps)
            .insert_resource(MrcPlayback {
                active: true,
                frame_times: vec![0.01, 0.02, 0.2],
                frame_cursor: 2,
                frame_target: Some(0.02),
                doc: Some(MrcDocument {
                    magic: meteor_asset::mrc::MRC_MAGIC,
                    header_size: meteor_asset::mrc::MRC_HEADER_SIZE,
                    mark0: 9,
                    mark1: 7,
                    tail: 0,
                    state: [0; 20],
                    meta: [0; 8],
                    queue_a: vec![],
                    queue_b: vec![
                        MrcQueueBEntry {
                            code: 0x57,
                            value: 1,
                            time: 0.005,
                        },
                        MrcQueueBEntry {
                            code: 0x57,
                            value: 0,
                            time: 0.015,
                        },
                    ],
                    queue_c: vec![],
                    queue_d: vec![MrcQueueDEntry {
                        a: 0,
                        b: 0,
                        c: 0,
                        time: 0.2,
                    }],
                    queue_e: vec![],
                }),
                ..default()
            })
            .init_schedule(MrcSubstep)
            .add_systems(MrcSubstep, tick_mrc_playback)
            .add_systems(
                fighter_frame::FighterStepStart,
                |playback: Res<MrcPlayback>,
                 keys: Res<ButtonInput<KeyCode>>,
                 mut samples: ResMut<Samples>| {
                    samples
                        .0
                        .push((playback.elapsed, keys.pressed(KeyCode::KeyW)));
                },
            );

        app.update();
        assert_eq!(
            app.world().resource::<Samples>().0,
            [(0.01, true), (0.02, false)]
        );
    }

    #[test]
    fn replay_roundtrip_preserves_events() {
        let events = vec![
            ReplayEvent {
                time: 0.1,
                key: InputBinding::Key(KeyCode::KeyJ),
                pressed: true,
            },
            ReplayEvent {
                time: 0.3,
                key: InputBinding::Key(KeyCode::KeyJ),
                pressed: false,
            },
            ReplayEvent {
                time: 1.0,
                key: InputBinding::Mouse(MouseButton::Right),
                pressed: true,
            },
        ];
        let bytes = encode_replay(&events);
        let decoded = decode_replay(&bytes).unwrap();
        assert_eq!(decoded.len(), 3);
        for (a, b) in events.iter().zip(&decoded) {
            assert_eq!(a.time, b.time);
            assert_eq!(a.key, b.key);
            assert_eq!(a.pressed, b.pressed);
        }
    }

    #[test]
    fn replay_decode_rejects_bad_magic() {
        assert!(decode_replay(b"XXXX1\x03\x00\x00\x00").is_none());
        assert!(decode_replay(b"").is_none());
        assert!(decode_replay(b"MREP1").is_none());
    }

    #[test]
    fn replay_decode_rejects_truncated() {
        let bytes = [b'M', b'R', b'E', b'P', b'1', 1, 0, 0, 0];
        assert!(decode_replay(&bytes).is_none());
    }

    #[test]
    fn key_name_mapping_covers_record_keys() {
        let keymap = Keymap::default();
        for binding in record_inputs(&keymap) {
            let name = binding.name();
            assert_eq!(InputBinding::from_name(&name), Some(binding), "{name}");
        }
        assert_eq!(
            InputBinding::from_name("Mouse2"),
            Some(InputBinding::Mouse(MouseButton::Right))
        );
        assert_eq!(
            InputBinding::from_name("KeyX"),
            Some(InputBinding::Key(KeyCode::KeyX))
        );
        assert!(InputBinding::from_name("NotAKey").is_none());
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod mrc_tests {
    use super::*;
    use crate::app_state::Keymap;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn mrc_input_isolation_keeps_recorded_holds_without_live_controls() {
        let keymap = Keymap::default();
        let mut app = App::new();
        app.insert_resource(keymap.clone())
            .insert_resource(MrcPlayback {
                active: true,
                ..default()
            })
            .init_resource::<MrcHeldInput>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>();

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.world_mut()
            .run_system_once(isolate_mrc_hardware_input)
            .unwrap();
        assert!(!keymap.move_up.pressed(
            app.world().resource::<ButtonInput<KeyCode>>(),
            app.world().resource::<ButtonInput<MouseButton>>(),
        ));
        assert!(!keymap.attack.pressed(
            app.world().resource::<ButtonInput<KeyCode>>(),
            app.world().resource::<ButtonInput<MouseButton>>(),
        ));

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyS);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.world_mut()
            .run_system_once(capture_mrc_held_input)
            .unwrap();

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyS);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        app.world_mut()
            .run_system_once(isolate_mrc_hardware_input)
            .unwrap();
        let keys = app.world().resource::<ButtonInput<KeyCode>>();
        let mouse = app.world().resource::<ButtonInput<MouseButton>>();
        assert!(!keys.pressed(KeyCode::KeyW));
        assert!(keys.pressed(KeyCode::KeyS));
        assert!(!keys.just_pressed(KeyCode::KeyS));
        assert!(!keys.just_released(KeyCode::KeyS));
        assert!(mouse.pressed(MouseButton::Left));
        assert!(!mouse.just_pressed(MouseButton::Left));
        assert!(!mouse.just_released(MouseButton::Left));
    }

    fn empty_mrc_document() -> meteor_asset::mrc::MrcDocument {
        meteor_asset::mrc::MrcDocument {
            magic: meteor_asset::mrc::MRC_MAGIC,
            header_size: meteor_asset::mrc::MRC_HEADER_SIZE,
            mark0: 0,
            mark1: 0,
            tail: 0,
            state: [0; meteor_asset::mrc::MRC_STATE_SIZE],
            meta: [0; 8],
            queue_a: Vec::new(),
            queue_b: Vec::new(),
            queue_c: Vec::new(),
            queue_d: Vec::new(),
            queue_e: Vec::new(),
        }
    }

    #[test]
    fn mrc_remote_event2_updates_authority_target_and_smooths_visible_root() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<MrcPlayback>()
            .init_resource::<MrcSessionRuntime>()
            .add_systems(
                Update,
                (apply_mrc_session_events, smooth_mrc_remote_fighters).chain(),
            );
        app.world_mut().resource_mut::<MrcPlayback>().active = true;
        let state_at = |x| {
            meteor_network::role_state::role_state_from_runtime(
                meteor_network::role_state::RoleStateRuntime {
                    position: meteor_network::role_state::RoleVec3 { x, y: 0.0, z: 2.0 },
                    forward: meteor_network::role_state::forward_from_yaw(0.0),
                    velocity: meteor_network::role_state::RoleVec3::ZERO,
                    pose_id: 0,
                    grounded: true,
                    guard_route: false,
                    playback_arg: 0,
                    pose_sequence: 1,
                },
            )
        };
        app.world_mut()
            .resource_mut::<MrcSessionRuntime>()
            .pending
            .push(MrcSessionEvent::Remote {
                actor_id: 1,
                state: state_at(1.0),
            });
        app.update();
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<MrcRemoteFighter>>()
            .single(app.world())
            .unwrap();
        assert!((app.world().get::<Transform>(entity).unwrap().translation.x - 1.0).abs() < 0.03);

        app.world_mut()
            .resource_mut::<MrcSessionRuntime>()
            .pending
            .push(MrcSessionEvent::Remote {
                actor_id: 1,
                state: state_at(20.0),
            });
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_nanos(1_000_000_000 / 60));
        app.update();
        let visible = app.world().get::<Transform>(entity).unwrap().translation;
        let target = app
            .world()
            .get::<crate::plugins::network_plugin::RemoteTransformTarget>(entity)
            .unwrap()
            .translation;
        assert!((target.x - 20.0).abs() < 0.03);
        assert!(
            visible.x > 1.0 && visible.x < target.x,
            "visible root must approach the recorded target without snapping: {visible:?}"
        );
        for _ in 0..90 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_nanos(1_000_000_000 / 60));
            app.update();
        }
        let visible = app.world().get::<Transform>(entity).unwrap().translation;
        assert!(
            (visible.x - target.x).abs() < 0.05,
            "visible root did not converge to the recorded target: {visible:?}"
        );
    }

    #[test]
    fn original_dagger_jump_remote_opening_has_no_packet_sized_visual_jump() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let bytes = std::fs::read(root.join("mrcs/tests/匕首小跳.mrc")).expect("dagger recording");
        let mut playback = MrcPlayback::default();
        playback
            .load_bytes("匕首小跳.mrc".into(), &bytes)
            .expect("valid MRC");
        assert!(playback.burst_tracks.get(&1).is_some_and(|bursts| bursts
            .iter()
            .any(|burst| burst.start_time < 0.7 && burst.end_time > 1.3)));
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Keymap>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ReplaySeekRequest>()
            .init_resource::<MrcFrameOverride>()
            .init_resource::<MrcSessionRuntime>()
            .init_resource::<CameraBasis>()
            .init_resource::<SyntheticInputStream>()
            .insert_resource(playback)
            .add_systems(
                Update,
                (
                    tick_mrc_playback,
                    apply_mrc_session_events,
                    smooth_mrc_remote_fighters,
                )
                    .chain(),
            );
        let mut previous: Option<Vec3> = None;
        let mut max_step = 0.0f32;
        let mut burst_end_gap = 0.0f32;
        for _ in 0..120 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_nanos(1_000_000_000 / 60));
            app.update();
            let t = app.world().resource::<MrcPlayback>().elapsed;
            let pair = app
                .world_mut()
                .query_filtered::<(
                    &Transform,
                    &crate::plugins::network_plugin::RemoteTransformTarget,
                ), With<MrcRemoteFighter>>()
                .single(app.world())
                .ok();
            let visible = pair.map(|(transform, _)| transform.translation);
            if (1.31..1.34).contains(&t) {
                if let Some((transform, target)) = pair {
                    burst_end_gap =
                        burst_end_gap.max(transform.translation.distance(target.translation));
                }
            }
            if let (Some(before), Some(after)) = (previous, visible) {
                if (0.7..1.6).contains(&t) {
                    max_step = max_step.max(before.distance(after));
                }
            }
            previous = visible;
        }
        assert!(
            max_step < 2.0,
            "dagger opening packet burst produced a {max_step:.2} world visual jump"
        );
        assert!(
            burst_end_gap < 2.0,
            "dagger burst display remained {burst_end_gap:.2} world behind the recorded position"
        );
    }

    #[test]
    fn mrc_burst_reconstruction_keeps_unexplained_teleports_discrete() {
        let mut doc = empty_mrc_document();
        let entry = |time, x| {
            let state = meteor_network::role_state::role_state_from_runtime(
                meteor_network::role_state::RoleStateRuntime {
                    position: meteor_network::role_state::RoleVec3 { x, y: 0.0, z: 0.0 },
                    forward: meteor_network::role_state::forward_from_yaw(0.0),
                    velocity: meteor_network::role_state::RoleVec3::ZERO,
                    pose_id: 144,
                    grounded: true,
                    guard_route: false,
                    playback_arg: 0,
                    pose_sequence: 1,
                },
            );
            meteor_asset::mrc::MrcQueueAEntry {
                id: 2,
                time,
                blob: [
                    1u16.to_le_bytes().as_slice(),
                    meteor_network::role_state::encode_event2_state(state).as_slice(),
                ]
                .concat(),
            }
        };
        doc.queue_a = vec![entry(0.0, 0.0), entry(0.5, 20.0), entry(0.5, 40.0)];
        assert!(
            mrc_remote_bursts(&doc).get(&1).is_none(),
            "zero recorded speed cannot justify a 40-world interpolation"
        );
    }

    #[test]
    fn paused_seek_into_dagger_packet_burst_shows_the_reconstructed_position() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let bytes = std::fs::read(root.join("mrcs/tests/匕首小跳.mrc")).expect("dagger recording");
        let mut playback = MrcPlayback::default();
        playback
            .load_bytes("匕首小跳.mrc".into(), &bytes)
            .expect("valid MRC");
        playback.paused = true;
        let burst = playback
            .burst_tracks
            .get(&1)
            .unwrap()
            .iter()
            .find(|burst| burst.start_time < 1.0 && burst.end_time > 1.0)
            .unwrap();
        let expected = mrc_scene_pose(burst.position_at(1.0), mrc_scene_frame(&playback, None));
        let mut app = App::new();
        app.init_resource::<Keymap>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ReplayPlayer>()
            .init_resource::<ReplaySeekRequest>()
            .init_resource::<MrcFrameOverride>()
            .init_resource::<MrcSessionRuntime>()
            .init_resource::<CameraBasis>()
            .insert_resource(playback)
            .add_systems(
                Update,
                (apply_replay_seek, apply_mrc_session_events).chain(),
            );
        app.world_mut().resource_mut::<ReplaySeekRequest>().0 = Some(1.0);
        app.update();
        let (visible, authority) = app
            .world_mut()
            .query_filtered::<(
                &Transform,
                &crate::plugins::network_plugin::RemoteTransformTarget,
            ), With<MrcRemoteFighter>>()
            .single(app.world())
            .unwrap();
        assert!(visible.translation.abs_diff_eq(expected, 1e-5));
        assert!(
            visible.translation.distance(authority.translation) > 1.0,
            "seek needs the position between the two sparse packet batches"
        );
    }

    #[test]
    fn fist_ultimate_hit_keeps_the_smoothed_opponent_near_the_recorded_local_pose() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let bytes = std::fs::read(root.join("mrcs/tests/指虎大绝.mrc")).expect("fist recording");
        let mut playback = MrcPlayback::default();
        playback
            .load_bytes("指虎大绝.mrc".into(), &bytes)
            .expect("valid MRC");
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Keymap>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ReplaySeekRequest>()
            .init_resource::<MrcFrameOverride>()
            .init_resource::<MrcSessionRuntime>()
            .init_resource::<CameraBasis>()
            .init_resource::<SyntheticInputStream>()
            .insert_resource(playback)
            .add_systems(
                Update,
                (
                    tick_mrc_playback,
                    apply_mrc_session_events,
                    smooth_mrc_remote_fighters,
                )
                    .chain(),
            );
        let local = app
            .world_mut()
            .spawn((
                LocalFighter,
                Transform::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
            ))
            .id();
        let mut distance_at_hit = None;
        for _ in 0..16 * 60 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_nanos(1_000_000_000 / 60));
            app.update();
            let t = app.world().resource::<MrcPlayback>().elapsed;
            if distance_at_hit.is_none() && t >= 15.207 {
                let remote = app
                    .world_mut()
                    .query_filtered::<&Transform, With<MrcRemoteFighter>>()
                    .single(app.world())
                    .unwrap()
                    .translation;
                let local_pos = app.world().get::<Transform>(local).unwrap().translation;
                distance_at_hit =
                    Some(Vec2::new(remote.x - local_pos.x, remote.z - local_pos.z).length());
            }
        }
        let distance = distance_at_hit.expect("recorded hit moment");
        assert!(
            distance < 3.0,
            "smoothing left the fist target {distance:.2} world away at the recorded hit"
        );
    }

    fn test_fighter() -> crate::components::fighter::Fighter {
        crate::components::fighter::Fighter {
            character_id: 0,
            player_slot: 0,
            weapon: 6,
            weapon_slots: [6, 0],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        }
    }

    #[test]
    fn mrc_code_maps_to_keymap_keys() {
        let keymap = Keymap::default();
        assert_eq!(
            mrc_code_to_key(0x57, &keymap),
            Some(InputBinding::Key(KeyCode::KeyW))
        );
        assert_eq!(
            mrc_code_to_key(0x53, &keymap),
            Some(InputBinding::Key(KeyCode::KeyS))
        );
        assert_eq!(
            mrc_code_to_key(0x41, &keymap),
            Some(InputBinding::Key(KeyCode::KeyA))
        );
        assert_eq!(
            mrc_code_to_key(0x44, &keymap),
            Some(InputBinding::Key(KeyCode::KeyD))
        );
        assert_eq!(mrc_code_to_key(0x10, &keymap), None);
        assert_eq!(
            key_to_mrc_code(InputBinding::Key(KeyCode::ControlLeft), &keymap),
            None
        );
        assert_eq!(mrc_code_to_key(0x1000, &keymap), Some(keymap.attack));
        assert_eq!(mrc_code_to_key(0x1200, &keymap), Some(keymap.jump));
        assert_eq!(mrc_code_to_key(0x20, &keymap), Some(keymap.guard));
        assert_eq!(keymap.guard, InputBinding::Mouse(MouseButton::Right));
        assert_eq!(
            key_to_mrc_code(InputBinding::Key(KeyCode::KeyG), &keymap),
            Some(0x47),
            "the Web-default G key records only DropWeapon, not Guard"
        );
        assert_eq!(key_to_mrc_code(keymap.guard, &keymap), Some(0x20));
        assert_eq!(mrc_code_to_key(0x45, &keymap), Some(keymap.weapon_switch));
        assert_eq!(mrc_code_to_key(0x43, &keymap), Some(keymap.crouch));
        assert_eq!(mrc_code_to_key(0x48, &keymap), Some(keymap.drop_item));
        assert_eq!(mrc_code_to_key(0x54, &keymap), Some(keymap.cure));
        assert_eq!(mrc_code_to_key(0x59, &keymap), Some(keymap.fake));
        assert_eq!(mrc_code_to_key(0x46, &keymap), Some(keymap.qi_burst));
        assert_eq!(mrc_code_to_key(0x11, &keymap), Some(keymap.dodge));
        assert_eq!(mrc_code_to_key(0x12, &keymap), Some(keymap.dodge));
        assert_eq!(mrc_code_to_key(0x26, &keymap), Some(keymap.camera_up));
        assert_eq!(mrc_code_to_key(0x28, &keymap), Some(keymap.camera_down));
        assert_eq!(mrc_code_to_key(0x25, &keymap), Some(keymap.left_turn));
        assert_eq!(mrc_code_to_key(0x27, &keymap), Some(keymap.right_turn));
        assert_eq!(mrc_code_to_key(0x51, &keymap), Some(keymap.lock_toggle));
        assert_eq!(mrc_code_to_key(0x09, &keymap), None);
        assert_eq!(key_to_mrc_code(keymap.fake, &keymap), Some(0x59));
        assert_eq!(key_to_mrc_code(keymap.drop_item, &keymap), Some(0x48));
        assert_eq!(key_to_mrc_code(keymap.qi_burst, &keymap), Some(0x46));
        assert_eq!(key_to_mrc_code(keymap.camera_up, &keymap), Some(0x26));
    }

    #[test]
    fn production_mrc_frame_applies_absolute_pose_facing_and_remaining_commands() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.init_resource::<Keymap>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<ButtonInput<MouseButton>>();
        app.init_resource::<MrcPlayback>();
        app.init_resource::<ReplaySeekRequest>();
        app.init_resource::<MrcFrameOverride>();
        app.init_resource::<MrcSessionRuntime>();
        app.init_resource::<CameraBasis>();
        app.init_resource::<SyntheticInputStream>();
        app.add_systems(
            Update,
            (tick_mrc_playback, apply_mrc_session_events).chain(),
        );
        let fighter = app
            .world_mut()
            .spawn((
                LocalFighter,
                test_fighter(),
                Transform::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::new(4.0, 8.0, 5.0),
                    facing: 0.0,
                },
                crate::components::fighter::Health::default(),
                crate::components::fighter::Qi::default(),
                crate::components::fighter::DeathRespawn::default(),
                crate::components::fighter::CombatState::default(),
                crate::components::fighter::SmState::default(),
                crate::components::fighter::PoseRuntime::idle(),
                crate::components::fighter::EntryBlend::default(),
            ))
            .id();
        app.world_mut().resource_mut::<MrcPlayback>().active = true;
        app.world_mut().resource_mut::<MrcPlayback>().doc = Some(meteor_asset::mrc::MrcDocument {
            magic: meteor_asset::mrc::MRC_MAGIC,
            header_size: meteor_asset::mrc::MRC_HEADER_SIZE,
            mark0: 0,
            mark1: 0,
            tail: 0,
            state: [0; meteor_asset::mrc::MRC_STATE_SIZE],
            meta: [0; 8],
            queue_a: vec![meteor_asset::mrc::MrcQueueAEntry {
                id: 25,
                time: 0.01,
                blob: [0xdc, 0x05, 0xd0, 0x07, 60, 0].to_vec(),
            }],
            queue_b: vec![meteor_asset::mrc::MrcQueueBEntry {
                code: 0x59,
                value: 1,
                time: 0.01,
            }],
            queue_c: vec![meteor_asset::mrc::MrcQueueCEntry {
                packed: 2,
                time: 0.01,
            }],
            queue_d: vec![meteor_asset::mrc::MrcQueueDEntry {
                a: 10.0f32.to_bits(),
                b: 20.0f32.to_bits(),
                c: 30.0f32.to_bits(),
                time: 0.01,
            }],
            queue_e: vec![meteor_asset::mrc::MrcQueueEEntry {
                a: 1.0f32.to_bits(),
                b: 0.0f32.to_bits(),
                time: 0.01,
            }],
        });
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(20));

        app.update();

        let transform = app.world().get::<Transform>(fighter).unwrap();
        assert!(transform
            .translation
            .abs_diff_eq(Vec3::new(1.2, 3.6, 2.4), 1e-5));
        let motion = app.world().get::<MotionState>(fighter).unwrap();
        assert!((motion.facing - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(
            motion.velocity,
            Vec3::new(4.0, 8.0, 5.0),
            "original MRC vt+118 receives a null velocity pointer and preserves +390..398"
        );
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::Health>(fighter)
                .unwrap()
                .current,
            1500.0
        );
        let keymap = app.world().resource::<Keymap>();
        let keys = app.world().resource::<ButtonInput<KeyCode>>();
        let mouse = app.world().resource::<ButtonInput<MouseButton>>();
        assert!(keymap.fake.pressed(keys, mouse));
        assert!(keymap.move_up.pressed(keys, mouse));
        assert!(keymap.move_left.pressed(keys, mouse));
        let frame = app.world().resource::<MrcFrameOverride>();
        assert!(frame.position_applied && frame.facing_applied);
        assert!((app.world().resource::<CameraBasis>().yaw - motion.facing).abs() < 1e-6);
        assert!(app.world().resource::<MrcPlayback>().paused);
    }

    #[test]
    fn paused_mrc_seek_runs_when_simulation_frame_is_frozen() {
        let mut app = App::new();
        app.insert_resource(State::new(AppState::Playing))
            .insert_resource(crate::plugins::FrameClock::default())
            .init_resource::<Time>()
            .init_resource::<Keymap>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<CameraBasis>();
        crate::plugins::configure_fighter_frame_sets(&mut app);
        app.add_plugins(ReplayPlugin);
        let mut doc = empty_mrc_document();
        doc.queue_d.push(meteor_asset::mrc::MrcQueueDEntry {
            a: 0,
            b: 0,
            c: 0,
            time: 20.0,
        });
        app.insert_resource(MrcPlayback {
            active: true,
            paused: true,
            doc: Some(doc),
            ..default()
        });
        let mut blend = crate::components::fighter::EntryBlend::default();
        blend.duration = 0.3;
        blend.elapsed = 0.1;
        blend.started_at_seconds = Some(18.0);
        let local = app
            .world_mut()
            .spawn((
                LocalFighter,
                Transform::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                blend,
            ))
            .id();
        app.world_mut().resource_mut::<ReplaySeekRequest>().0 = Some(7.0);
        app.update();
        let playback = app.world().resource::<MrcPlayback>();
        assert_eq!(playback.elapsed, 7.0);
        assert!(playback.paused);
        assert!(app.world().resource::<ReplaySeekRequest>().0.is_none());
        let blend = app
            .world()
            .get::<crate::components::fighter::EntryBlend>(local)
            .unwrap();
        assert_eq!(blend.started_at_seconds, None);
        assert_eq!(blend.elapsed, 0.0);
    }

    #[test]
    fn production_seek_rebuilds_all_mrc_queues_without_replaying_actions() {
        fn remote_entry(time: f32, x: f32, pose_id: u16) -> meteor_asset::mrc::MrcQueueAEntry {
            let role = meteor_network::role_state::role_state_from_runtime(
                meteor_network::role_state::RoleStateRuntime {
                    position: meteor_network::role_state::RoleVec3 { x, y: 0.0, z: 2.0 },
                    forward: meteor_network::role_state::forward_from_yaw(0.25),
                    velocity: meteor_network::role_state::RoleVec3::ZERO,
                    pose_id,
                    grounded: true,
                    guard_route: false,
                    playback_arg: 0,
                    pose_sequence: pose_id,
                },
            );
            let mut blob = 2u16.to_le_bytes().to_vec();
            blob.extend_from_slice(&meteor_network::role_state::encode_event2_state(role));
            meteor_asset::mrc::MrcQueueAEntry { id: 2, time, blob }
        }

        let mut app = App::new();
        app.init_resource::<Keymap>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<ButtonInput<MouseButton>>();
        app.init_resource::<ReplayPlayer>();
        app.init_resource::<MrcPlayback>();
        app.init_resource::<ReplaySeekRequest>();
        app.init_resource::<MrcFrameOverride>();
        app.init_resource::<MrcSessionRuntime>();
        app.init_resource::<CameraBasis>();
        app.add_systems(
            Update,
            (apply_replay_seek, apply_mrc_session_events).chain(),
        );
        let local = app
            .world_mut()
            .spawn((
                LocalFighter,
                test_fighter(),
                Transform::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                crate::components::fighter::Health::default(),
                crate::components::fighter::Qi::default(),
                crate::components::fighter::DeathRespawn::default(),
                crate::components::fighter::CombatState::default(),
                crate::components::fighter::SmState::default(),
                crate::components::fighter::PoseRuntime::idle(),
                crate::components::fighter::EntryBlend::default(),
            ))
            .id();
        let mut doc = empty_mrc_document();
        doc.queue_a = vec![
            meteor_asset::mrc::MrcQueueAEntry {
                id: 12,
                time: 0.5,
                blob: [24u32.to_le_bytes(), 2u32.to_le_bytes()].concat(),
            },
            meteor_asset::mrc::MrcQueueAEntry {
                id: 25,
                time: 1.0,
                blob: [
                    1800u16.to_le_bytes(),
                    2000u16.to_le_bytes(),
                    10u16.to_le_bytes(),
                ]
                .concat(),
            },
            remote_entry(1.0, 1.0, 10),
            meteor_asset::mrc::MrcQueueAEntry {
                id: 50,
                time: 1.25,
                blob: [1u16, 473, 0, 11, 10]
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            },
            meteor_asset::mrc::MrcQueueAEntry {
                id: 25,
                time: 3.0,
                blob: [
                    1000u16.to_le_bytes(),
                    2000u16.to_le_bytes(),
                    30u16.to_le_bytes(),
                ]
                .concat(),
            },
            remote_entry(3.0, 5.0, 20),
        ];
        doc.queue_b = vec![
            meteor_asset::mrc::MrcQueueBEntry {
                code: 0x57,
                value: 1,
                time: 0.5,
            },
            meteor_asset::mrc::MrcQueueBEntry {
                code: 0x1000,
                value: 1,
                time: 0.75,
            },
            meteor_asset::mrc::MrcQueueBEntry {
                code: 0x57,
                value: 0,
                time: 3.0,
            },
        ];
        doc.queue_c = vec![
            meteor_asset::mrc::MrcQueueCEntry {
                packed: 2,
                time: 1.0,
            },
            meteor_asset::mrc::MrcQueueCEntry {
                packed: 1 | (1 << 8),
                time: 3.0,
            },
        ];
        doc.queue_d = vec![
            meteor_asset::mrc::MrcQueueDEntry {
                a: 10.0f32.to_bits(),
                b: 20.0f32.to_bits(),
                c: 30.0f32.to_bits(),
                time: 1.0,
            },
            meteor_asset::mrc::MrcQueueDEntry {
                a: 40.0f32.to_bits(),
                b: 50.0f32.to_bits(),
                c: 60.0f32.to_bits(),
                time: 3.0,
            },
        ];
        doc.queue_e = vec![
            meteor_asset::mrc::MrcQueueEEntry {
                a: 0.0f32.to_bits(),
                b: 1.0f32.to_bits(),
                time: 1.0,
            },
            meteor_asset::mrc::MrcQueueEEntry {
                a: 1.0f32.to_bits(),
                b: 0.0f32.to_bits(),
                time: 3.0,
            },
        ];
        {
            let mut playback = app.world_mut().resource_mut::<MrcPlayback>();
            playback.active = true;
            playback.paused = true;
            playback.doc = Some(doc);
        }

        app.world_mut().resource_mut::<ReplaySeekRequest>().0 = Some(3.0);
        app.update();
        assert!(app
            .world()
            .resource::<MrcSessionRuntime>()
            .pending_actions
            .is_empty());
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::Health>(local)
                .unwrap()
                .current,
            1000.0
        );
        assert!(app
            .world()
            .get::<Transform>(local)
            .unwrap()
            .translation
            .abs_diff_eq(Vec3::new(4.8, 7.2, 6.0), 1e-5));
        let keymap = app.world().resource::<Keymap>();
        let keys = app.world().resource::<ButtonInput<KeyCode>>();
        let mouse = app.world().resource::<ButtonInput<MouseButton>>();
        assert!(!keymap.move_up.pressed(keys, mouse));
        assert!(!keymap.attack.pressed(keys, mouse));
        assert!(!keymap.attack.just_pressed(keys, mouse));
        let remote_targets: Vec<Vec3> = app
            .world_mut()
            .query_filtered::<&crate::plugins::network_plugin::RemoteTransformTarget, With<MrcRemoteFighter>>()
            .iter(app.world())
            .map(|target| target.translation)
            .collect();
        assert_eq!(remote_targets.len(), 1);
        assert!(remote_targets[0].abs_diff_eq(Vec3::new(5.0, 0.0, 2.0), 0.03));
        let remote_entities: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<MrcRemoteFighter>>()
            .iter(app.world())
            .collect();
        assert_eq!(remote_entities.len(), 1);
        let remote_entity = remote_entities[0];
        assert!(app
            .world()
            .get::<crate::components::fighter::VisualRootPin>(remote_entity)
            .is_some());
        assert_eq!(
            app.world()
                .get::<Transform>(remote_entity)
                .unwrap()
                .rotation,
            Quat::IDENTITY
        );

        app.world_mut().resource_mut::<ReplaySeekRequest>().0 = Some(1.5);
        app.update();
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::Health>(local)
                .unwrap()
                .current,
            1800.0
        );
        assert!(app
            .world()
            .get::<Transform>(local)
            .unwrap()
            .translation
            .abs_diff_eq(Vec3::new(1.2, 3.6, 2.4), 1e-5));
        let keymap = app.world().resource::<Keymap>();
        let keys = app.world().resource::<ButtonInput<KeyCode>>();
        let mouse = app.world().resource::<ButtonInput<MouseButton>>();
        assert!(keymap.move_up.pressed(keys, mouse));
        assert!(!keymap.move_up.just_pressed(keys, mouse));
        assert!(!keymap.attack.pressed(keys, mouse));
        let remote_targets: Vec<Vec3> = app
            .world_mut()
            .query_filtered::<&crate::plugins::network_plugin::RemoteTransformTarget, With<MrcRemoteFighter>>()
            .iter(app.world())
            .map(|target| target.translation)
            .collect();
        assert_eq!(remote_targets.len(), 1);
        assert!(remote_targets[0].abs_diff_eq(Vec3::new(1.0, 0.0, 2.0), 0.03));
        let playback = app.world().resource::<MrcPlayback>();
        assert_eq!(playback.elapsed, 1.5);
        assert_eq!(playback.index_d, 1);
        assert!(playback.paused);
    }

    #[test]
    fn recorder_writes_quantized_pose_pairs_and_idle_heartbeat() {
        let mut recorder = ReplayRecorder::default();
        let position = Vec3::new(1.2, 3.6, 2.4);
        record_mrc_pose_sample(&mut recorder, 0.1, position, std::f32::consts::FRAC_PI_2);
        record_mrc_pose_sample(&mut recorder, 0.2, position, std::f32::consts::FRAC_PI_2);
        assert_eq!(recorder.queue_d.len(), 1);
        assert_eq!(recorder.queue_e.len(), 1);
        record_mrc_pose_sample(&mut recorder, 0.6, position, std::f32::consts::FRAC_PI_2);
        assert_eq!(recorder.queue_d.len(), 2);
        assert_eq!(recorder.queue_e.len(), 2);
        assert!(decode_mrc_pose(&recorder.queue_d[0])
            .unwrap()
            .abs_diff_eq(position, 1e-5));
        assert!(
            (decode_mrc_facing(&recorder.queue_e[0]).unwrap() - std::f32::consts::FRAC_PI_2).abs()
                < 1e-6
        );

        let bytes = export_mrc_with_samples(
            &[],
            &Keymap::default(),
            [0; meteor_asset::mrc::MRC_STATE_SIZE],
            &recorder.queue_a,
            &recorder.queue_d,
            &recorder.queue_e,
        );
        let decoded = meteor_asset::mrc::decode_mrc(&bytes).unwrap();
        assert_eq!(decoded.queue_d.len(), 2);
        assert_eq!(decoded.queue_e.len(), 2);
    }

    #[test]
    fn recorder_writes_changed_vitals_and_round_trips_queue_a() {
        let mut recorder = ReplayRecorder::default();
        record_mrc_vitals_sample(&mut recorder, 0.1, 2_000.0, 2_000.0, 0.0);
        record_mrc_vitals_sample(&mut recorder, 0.2, 2_000.4, 2_000.0, 0.0);
        record_mrc_vitals_sample(&mut recorder, 0.3, 1_500.0, 2_000.0, 12.6);
        record_mrc_vitals_sample(&mut recorder, 0.4, f32::NAN, f32::INFINITY, -1.0);

        assert_eq!(recorder.queue_a.len(), 3);
        assert_eq!(
            decode_mrc_vitals(&recorder.queue_a[0].blob),
            Some((2000.0, 2000.0, 0.0))
        );
        assert_eq!(
            decode_mrc_vitals(&recorder.queue_a[1].blob),
            Some((1500.0, 2000.0, 13.0))
        );
        assert_eq!(
            decode_mrc_vitals(&recorder.queue_a[2].blob),
            Some((0.0, 65535.0, 0.0))
        );

        let bytes = export_mrc_with_samples(
            &[],
            &Keymap::default(),
            [0; meteor_asset::mrc::MRC_STATE_SIZE],
            &recorder.queue_a,
            &[],
            &[],
        );
        let decoded = meteor_asset::mrc::decode_mrc(&bytes).unwrap();
        assert_eq!(decoded.queue_a.len(), 3);
        assert_eq!(decoded.queue_a[1].id, 25);
        assert_eq!(decoded.queue_a[1].time, 0.3);
        assert_eq!(decoded.queue_a[1].blob, recorder.queue_a[1].blob);
    }

    #[test]
    fn recorder_boot_context_uses_original_stage_and_weapon_layout() {
        let stage = mrc_stage_entry("sn06").unwrap();
        assert_eq!(stage.id, 10);
        assert_eq!(&stage.blob[0..4], b"sn06");
        assert_eq!(u32::from_le_bytes(stage.blob[4..8].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(stage.blob[8..12].try_into().unwrap()), 6);
        assert!(mrc_stage_entry("custom01").is_none());

        for item_id in 1..=59 {
            let Some((weapon, variant)) =
                crate::plugins::character_plugin::weapon_slot_from_item(item_id)
            else {
                continue;
            };
            assert_eq!(weapon_item_from_slot(weapon, variant), item_id as u32);
        }
        assert_eq!(weapon_item_from_slot(12, 4), 0);
    }

    #[test]
    fn queue_a_boot_context_uses_stage_weapon_slot_and_spawn_vitals() {
        let mut doc = empty_mrc_document();
        doc.state[0..4].copy_from_slice(&24u32.to_le_bytes());
        doc.state[4..8].copy_from_slice(&3u32.to_le_bytes());
        doc.queue_a.push(meteor_asset::mrc::MrcQueueAEntry {
            id: 10,
            time: 0.0,
            blob: [b's', b'n', b'0', b'6', 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0].to_vec(),
        });
        doc.queue_a.push(meteor_asset::mrc::MrcQueueAEntry {
            id: 25,
            time: 0.0,
            blob: [0xdc, 0x05, 0xd0, 0x07, 60, 0].to_vec(),
        });
        let playback = MrcPlayback {
            doc: Some(doc),
            ..default()
        };
        assert_eq!(playback.stage_id().as_deref(), Some("sn06"));
        assert_eq!(playback.local_slot(), Some(3));
        assert_eq!(playback.local_weapon(), Some((6, 2)));
        assert_eq!(playback.spawn_vitals(), Some((1500.0, 2000.0, 60.0)));
    }

    #[test]
    fn queue_a_routes_known_payloads_and_counts_unknown_ids() {
        let role = meteor_network::role_state::role_state_from_runtime(
            meteor_network::role_state::RoleStateRuntime {
                position: meteor_network::role_state::RoleVec3 {
                    x: 1.2,
                    y: 0.9,
                    z: 2.4,
                },
                forward: meteor_network::role_state::forward_from_yaw(0.5),
                velocity: meteor_network::role_state::RoleVec3::ZERO,
                pose_id: 260,
                grounded: true,
                guard_route: false,
                playback_arg: 0,
                pose_sequence: 7,
            },
        );
        let mut remote_blob = 2u16.to_le_bytes().to_vec();
        remote_blob.extend_from_slice(&meteor_network::role_state::encode_event2_state(role));
        let mut runtime = MrcSessionRuntime::default();
        for entry in [
            meteor_asset::mrc::MrcQueueAEntry {
                id: 2,
                time: 0.1,
                blob: remote_blob,
            },
            meteor_asset::mrc::MrcQueueAEntry {
                id: 12,
                time: 0.1,
                blob: [24u32.to_le_bytes(), 2u32.to_le_bytes()].concat(),
            },
            meteor_asset::mrc::MrcQueueAEntry {
                id: 25,
                time: 0.1,
                blob: [
                    1000u16.to_le_bytes(),
                    2000u16.to_le_bytes(),
                    50u16.to_le_bytes(),
                ]
                .concat(),
            },
            meteor_asset::mrc::MrcQueueAEntry {
                id: 67,
                time: 0.1,
                blob: vec![1, 2, 3],
            },
        ] {
            route_mrc_session_entry(&entry, &mut runtime);
        }
        assert_eq!(runtime.pending.len(), 3);
        assert_eq!(runtime.event_counts.get(&2), Some(&1));
        assert_eq!(runtime.event_counts.get(&67), Some(&1));
        assert_eq!(runtime.unhandled_counts.get(&67), Some(&1));
    }

    #[test]
    fn original_mrc_seek_restores_authoritative_death_and_revival() {
        use super::super::network_plugin::NetState;
        use crate::components::fighter::{
            CombatState, DeathRespawn, EntryBlend, Health, PoseRuntime, Qi, SmState,
        };

        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../mrcs/佳佳VS竹子/佳佳VS竹子.mrc");
        let mut playback = MrcPlayback::default();
        playback.load(&path).expect("original recording");

        let des = (0..14)
            .map(|index| format!("Object D_Replay{index}\n{{\n Position: 0 0 0\n Custom:\n {{\n name=\"machine\"\n model=\"Dummy\"\n }}\n}}\n"))
            .collect::<String>();
        let mut app = App::new();
        app.init_resource::<Keymap>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ReplayPlayer>()
            .init_resource::<ReplaySeekRequest>()
            .init_resource::<MrcFrameOverride>()
            .init_resource::<MrcSessionRuntime>()
            .init_resource::<CameraBasis>()
            .init_resource::<NetState>()
            .insert_resource(crate::app_state::LoadedStageAssets {
                des: Some(des.into_bytes()),
                ..default()
            })
            .insert_resource(playback)
            .add_systems(
                Update,
                (
                    apply_replay_seek,
                    apply_mrc_session_events,
                    apply_mrc_remote_life,
                )
                    .chain(),
            );
        let local = app
            .world_mut()
            .spawn((
                LocalFighter,
                test_fighter(),
                Health {
                    current: 2000.0,
                    max: 2000.0,
                },
                Qi::default(),
                DeathRespawn::default(),
                CombatState::default(),
                SmState::default(),
                PoseRuntime::idle(),
                EntryBlend::default(),
            ))
            .id();

        for (time, expected_hp, remote_dead) in [
            (250.0, 0.0, false),
            (277.55, 1963.0, false),
            (250.0, 0.0, false),
            (253.0, 2000.0, false),
            (244.0, 733.0, true),
        ] {
            app.world_mut().resource_mut::<ReplaySeekRequest>().0 = Some(time);
            app.update();
            assert_eq!(
                app.world().get::<DeathRespawn>(local).unwrap().active,
                expected_hp == 0.0
            );
            assert_eq!(
                app.world().get::<Health>(local).unwrap().current,
                expected_hp
            );
            let mut actors = app
                .world_mut()
                .query::<(&MrcRemoteFighter, &DeathRespawn)>();
            let (_, death) = actors
                .iter(app.world())
                .find(|(remote, _)| remote.actor_id == 3)
                .unwrap();
            assert_eq!(death.active, remote_dead);
            let net = app.world().resource::<NetState>();
            assert_eq!(net.remote_kills.get("mrc:4"), Some(&1));
            assert_eq!(net.round_remaining_seconds, Some(5806.0 - time));
            assert!(
                !net.pending_scene_ops.is_empty(),
                "scene snapshot must resolve to authored props"
            );
            assert!(net.pending_system_notices.is_empty());
        }
    }

    #[test]
    fn stock_mrc_uses_the_loaded_stage_frame_for_local_and_remote_positions() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../mrcs/tests/指虎大绝.mrc");
        let mut playback = MrcPlayback::default();
        playback.load(&path).expect("original fist recording");
        assert!(playback.stock_scene_coordinates());
        let waypoints = meteor_scene::stage_loader::WaypointGraph {
            center: [100.0, 200.0, 0.0],
            ..default()
        };
        let frame = mrc_scene_frame(&playback, Some(&waypoints)).unwrap();
        let doc = playback.doc.as_ref().unwrap();
        let local = decode_mrc_pose(&doc.queue_d[0]).unwrap();
        let mapped = mrc_scene_pose(local, Some(frame));
        assert!(mapped.abs_diff_eq(Vec3::new(local.x - 12.0, local.y, 24.0 - local.z,), 1e-4));
        let source_yaw = decode_mrc_facing(&doc.queue_e[0]).unwrap();
        let mapped_yaw = mrc_scene_facing(source_yaw, Some(frame));
        assert!((mapped_yaw.sin() - source_yaw.sin()).abs() < 1e-5);
        assert!((mapped_yaw.cos() + source_yaw.cos()).abs() < 1e-5);
        let remote = doc
            .queue_a
            .iter()
            .find_map(|entry| {
                (entry.id == 2 && entry.blob.len() >= 16)
                    .then(|| {
                        meteor_network::role_state::decode_event2_state(&entry.blob[2..16]).ok()
                    })
                    .flatten()
            })
            .expect("event2 in original recording");
        let mapped_remote = frame.to_scene(remote);
        assert!((mapped_remote.position.x - (remote.position.x - 12.0)).abs() < 1e-5);
        assert!((mapped_remote.position.z - (24.0 - remote.position.z)).abs() < 1e-5);
        let mut own = empty_mrc_document();
        own.mark0 = 0;
        own.mark1 = 0;
        playback.doc = Some(own);
        assert!(mrc_scene_frame(&playback, Some(&waypoints)).is_none());
        assert_eq!(mrc_scene_pose(local, None), local);
    }

    #[test]
    fn original_event51_applies_remote_action_blend_once_after_actor_entry() {
        use crate::plugins::character_plugin::{load_character_asset_for_test, CharacterAssetsSet};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character = load_character_asset_for_test(&[root], "p0").expect("original p0");
        let mut assets = CharacterAssetsSet::default();
        assets.0.insert("p0".into(), character);
        let mut app = App::new();
        app.insert_resource(assets);
        let mut doc = empty_mrc_document();
        doc.state[4..8].copy_from_slice(&1u32.to_le_bytes());
        app.insert_resource(MrcPlayback {
            doc: Some(doc),
            active: true,
            ..default()
        });
        app.init_resource::<MrcSessionRuntime>();
        app.add_systems(
            Update,
            (apply_mrc_session_events, sync_mrc_remote_pose).chain(),
        );
        let action = meteor_asset::mrc::MrcQueueAEntry {
            id: 51,
            time: 10.67306,
            blob: [2u16, 372, 0, 19, 50]
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect(),
        };
        route_mrc_session_entry(
            &action,
            &mut app.world_mut().resource_mut::<MrcSessionRuntime>(),
        );
        app.update();
        assert_eq!(
            app.world()
                .resource::<MrcSessionRuntime>()
                .pending_actions
                .len(),
            1
        );
        let state = meteor_network::role_state::role_state_from_runtime(
            meteor_network::role_state::RoleStateRuntime {
                position: meteor_network::role_state::RoleVec3::ZERO,
                forward: meteor_network::role_state::forward_from_yaw(0.0),
                velocity: meteor_network::role_state::RoleVec3::ZERO,
                pose_id: 372,
                grounded: true,
                guard_route: false,
                playback_arg: 0,
                pose_sequence: 19,
            },
        );
        app.world_mut()
            .resource_mut::<MrcSessionRuntime>()
            .pending
            .push(MrcSessionEvent::Remote { actor_id: 2, state });
        app.update();
        app.update();
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<MrcRemoteFighter>>()
            .single(app.world())
            .unwrap();
        let pose = app
            .world()
            .get::<crate::components::fighter::PoseRuntime>(entity)
            .unwrap();
        let instance = pose.pose_instance_id;
        assert_eq!(pose.pose_id, 372);
        assert!(!pose.loops);
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::EntryBlend>(entity)
                .unwrap()
                .duration,
            0.5
        );
        route_mrc_session_entry(
            &action,
            &mut app.world_mut().resource_mut::<MrcSessionRuntime>(),
        );
        app.update();
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::PoseRuntime>(entity)
                .unwrap()
                .pose_instance_id,
            instance
        );
    }

    #[test]
    fn original_fist_recording_feeds_full_anger_command_through_production_input() {
        use crate::plugins::input_plugin::{
            capture_local_command_input, DesktopDodgeInput, LocalCombatInput, LocalCommandInput,
            LocalCrouchInput, LocalGuardInput, LocalJumpInput,
        };
        use meteor_input::command_parser::Dir;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../mrcs/tests/指虎大绝.mrc");
        let mut playback = MrcPlayback::default();
        playback.load(&path).expect("original fist recording");
        let doc = playback.doc.as_ref().unwrap();
        let attack_time = doc
            .queue_b
            .iter()
            .find(|entry| {
                entry.code == 0x1000 && entry.value != 0 && (14.0..15.0).contains(&entry.time)
            })
            .unwrap()
            .time;
        let first_hit_time = doc
            .queue_a
            .iter()
            .find(|entry| entry.id == 11 && entry.time > attack_time)
            .unwrap()
            .time;
        let pos_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../origin_res/pmodel/P0.POS");
        let pos_text = crate::text_codec::decode_text(&std::fs::read(pos_path).unwrap());
        let fist = meteor_asset::pos::parse_poses(&pos_text)
            .into_iter()
            .find(|pose| pose.id == 421)
            .unwrap();
        let authored_hit_offset =
            (fist.attacks[0].start - fist.start) as f32 / meteor_core::ANIM_FPS;
        assert!(((first_hit_time - attack_time) - authored_hit_offset).abs() < 0.02);
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Time<bevy::time::Real>>()
            .init_resource::<Keymap>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ReplaySeekRequest>()
            .init_resource::<ReplayPlayer>()
            .init_resource::<MrcFrameOverride>()
            .init_resource::<MrcSessionRuntime>()
            .init_resource::<CameraBasis>()
            .init_resource::<SyntheticInputStream>()
            .init_resource::<LocalCommandInput>()
            .init_resource::<LocalCrouchInput>()
            .init_resource::<LocalGuardInput>()
            .init_resource::<LocalCombatInput>()
            .init_resource::<LocalJumpInput>()
            .init_resource::<DesktopDodgeInput>()
            .add_message::<bevy::window::WindowEvent>()
            .insert_resource(playback)
            .add_systems(
                Update,
                (
                    tick_mrc_playback,
                    apply_replay_seek,
                    apply_mrc_session_events,
                    capture_local_command_input,
                )
                    .chain(),
            );
        let mut fighter = test_fighter();
        fighter.weapon = 11;
        fighter.weapon_slots = [11, 0];
        app.world_mut().spawn((
            LocalFighter,
            fighter,
            Transform::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
            crate::components::fighter::Health::default(),
            crate::components::fighter::Qi::default(),
            crate::components::fighter::DeathRespawn::default(),
            crate::components::fighter::CombatState::default(),
            crate::components::fighter::SmState::default(),
            crate::components::fighter::PoseRuntime::idle(),
            crate::components::fighter::EntryBlend::default(),
        ));
        app.world_mut().resource_mut::<ReplaySeekRequest>().0 = Some(14.6);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.world_mut().resource_mut::<MrcPlayback>().paused = false;
        let mut attack = None;
        let mut parser = meteor_input::command_parser::CommandInput::default();
        for _ in 0..22 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(20));
            app.world_mut()
                .resource_mut::<Time<bevy::time::Real>>()
                .advance_by(std::time::Duration::from_millis(20));
            app.update();
            let edges = app.world().resource::<LocalCombatInput>().edges.clone();
            for edge in &edges {
                match *edge {
                    crate::plugins::input_plugin::LocalCombatInputEdge::DirectionPress(dir) => {
                        parser.push_direction(dir, 14_900.0)
                    }
                    crate::plugins::input_plugin::LocalCombatInputEdge::DirectionRelease(dir) => {
                        parser.release_direction(dir)
                    }
                    crate::plugins::input_plugin::LocalCombatInputEdge::Attack => {
                        let qi = app
                            .world_mut()
                            .query_filtered::<&crate::components::fighter::Qi, With<LocalFighter>>()
                            .single(app.world())
                            .unwrap()
                            .current;
                        attack = Some((parser.peek(), qi));
                    }
                    _ => {}
                }
            }
            if attack.is_some() {
                break;
            }
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .clear();
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
        }
        assert_eq!(
            attack,
            Some((vec![Dir::Left, Dir::Right, Dir::Down], 100.0))
        );

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(20));
        app.world_mut()
            .resource_mut::<Time<bevy::time::Real>>()
            .advance_by(std::time::Duration::from_millis(20));
        app.update();
        let local = app
            .world_mut()
            .query_filtered::<&Transform, With<LocalFighter>>()
            .single(app.world())
            .unwrap()
            .translation;
        let remote = app
            .world_mut()
            .query_filtered::<&Transform, With<MrcRemoteFighter>>()
            .single(app.world())
            .unwrap()
            .translation;
        let distance = Vec2::new(local.x - remote.x, local.z - remote.z).length();
        assert!(
            (distance - 2.14).abs() < 0.25,
            "fist ultimate separation: {distance}"
        );

        app.world_mut().resource_mut::<ReplaySeekRequest>().0 = Some(14.93);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(33));
        app.world_mut()
            .resource_mut::<Time<bevy::time::Real>>()
            .advance_by(std::time::Duration::from_millis(33));
        app.update();
        assert!(app
            .world()
            .resource::<LocalCombatInput>()
            .edges
            .contains(&crate::plugins::input_plugin::LocalCombatInputEdge::Attack));
        let qi_at_attack = app
            .world_mut()
            .query_filtered::<&crate::components::fighter::Qi, With<LocalFighter>>()
            .single(app.world())
            .unwrap()
            .current;
        assert_eq!(qi_at_attack, 100.0, "later MRC vitals overtook attack");
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        app.world_mut()
            .resource_mut::<Time<bevy::time::Real>>()
            .advance_by(std::time::Duration::from_millis(16));
        app.update();
        let qi_after_attack = app
            .world_mut()
            .query_filtered::<&crate::components::fighter::Qi, With<LocalFighter>>()
            .single(app.world())
            .unwrap()
            .current;
        assert_eq!(qi_after_attack, 0.0);
    }

    #[test]
    fn original_hit_and_property_events_update_the_remote_without_repeating_local_hits() {
        use crate::plugins::character_plugin::{load_character_asset_for_test, CharacterAssetsSet};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character = load_character_asset_for_test(&[root.clone()], "p0").expect("original p0");
        let mut assets = CharacterAssetsSet::default();
        assets.0.insert("p0".into(), character);
        let table =
            meteor_asset::res::parse_meteor_res(&std::fs::read(root.join("Meteor.res")).unwrap())
                .unwrap()
                .combat_table();
        assert_eq!(table.status_effect(37).unwrap().anger, -5);
        let mut app = App::new();
        app.insert_resource(assets)
            .insert_resource(crate::plugins::combat_plugin::CombatResourceData(table));
        let mut doc = empty_mrc_document();
        doc.state[4..8].copy_from_slice(&2u32.to_le_bytes());
        app.insert_resource(MrcPlayback {
            doc: Some(doc),
            active: true,
            ..default()
        });
        app.init_resource::<MrcSessionRuntime>();
        app.add_systems(
            Update,
            (
                apply_mrc_session_events,
                apply_mrc_hit_events,
                apply_mrc_property_events,
                sync_mrc_remote_pose,
            )
                .chain(),
        );
        let state = meteor_network::role_state::role_state_from_runtime(
            meteor_network::role_state::RoleStateRuntime {
                position: meteor_network::role_state::RoleVec3::ZERO,
                forward: meteor_network::role_state::forward_from_yaw(0.0),
                velocity: meteor_network::role_state::RoleVec3::ZERO,
                pose_id: 0,
                grounded: true,
                guard_route: false,
                playback_arg: 0,
                pose_sequence: 1,
            },
        );
        app.world_mut()
            .resource_mut::<MrcSessionRuntime>()
            .pending
            .push(MrcSessionEvent::Remote { actor_id: 1, state });
        app.update();
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<MrcRemoteFighter>>()
            .single(app.world())
            .unwrap();
        let hit = meteor_network::stock_protocol::StockHitReactionEvent {
            pose_id: 100,
            move_hundred: 150,
            value_hundred: 20,
            context: 4,
            target_actor_id: 1,
        };
        route_mrc_session_entry(
            &meteor_asset::mrc::MrcQueueAEntry {
                id: 11,
                time: 10.44105,
                blob: hit.encode().to_vec(),
            },
            &mut app.world_mut().resource_mut::<MrcSessionRuntime>(),
        );
        route_mrc_session_entry(
            &meteor_asset::mrc::MrcQueueAEntry {
                id: 61,
                time: 11.0,
                blob: meteor_network::stock_protocol::StockPropertyEffectEvent {
                    actor_id: 1,
                    category: 2,
                    resource_id: 37,
                }
                .encode(),
            },
            &mut app.world_mut().resource_mut::<MrcSessionRuntime>(),
        );
        app.update();
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::PoseRuntime>(entity)
                .unwrap()
                .pose_id,
            100
        );
        let reaction = app
            .world()
            .get::<crate::components::fighter::FighterHitReaction>(entity)
            .unwrap()
            .current
            .as_ref()
            .unwrap();
        assert!((reaction.motion_scale - 1.5).abs() < 1e-6);
        assert!((reaction.authored_minimum - 0.2).abs() < 1e-6);
        assert_eq!(
            app.world()
                .get::<crate::plugins::network_plugin::RemoteHitMotionWindow>(entity)
                .unwrap()
                .remaining,
            0.4
        );
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::FighterStatusEffects>(entity)
                .unwrap()
                .pending
                .anger,
            -5
        );
        let mut later = state;
        later.state_words.pose_sequence = 2;
        app.world_mut()
            .resource_mut::<MrcSessionRuntime>()
            .pending
            .push(MrcSessionEvent::Remote {
                actor_id: 1,
                state: later,
            });
        app.update();
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::PoseRuntime>(entity)
                .unwrap()
                .pose_id,
            100
        );
        app.world_mut()
            .get_mut::<crate::plugins::network_plugin::RemoteHitMotionWindow>(entity)
            .unwrap()
            .remaining = 0.0;
        app.update();
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::PoseRuntime>(entity)
                .unwrap()
                .pose_id,
            0
        );
    }

    #[test]
    fn original_ninja_burn_uses_recorded_start_hit_pose_and_vitals() {
        use crate::components::fighter::{
            CombatState, EntryBlend, FighterHitReaction, Health, PoseRuntime, Qi, SmState,
        };
        use crate::plugins::character_plugin::{load_character_asset_for_test, CharacterAssetsSet};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let doc = meteor_asset::mrc::decode_mrc(
            &std::fs::read(root.join("mrcs/tests/忍烧.mrc")).expect("ninja recording"),
        )
        .expect("valid MRC");
        assert_eq!(u32::from_le_bytes(doc.state[4..8].try_into().unwrap()), 2);
        let character =
            load_character_asset_for_test(&[root.join("origin_res")], "p0").expect("original p0");
        let mut assets = CharacterAssetsSet::default();
        assets.0.insert("p0".into(), character);
        let mut app = App::new();
        app.insert_resource(assets)
            .insert_resource(MrcPlayback {
                doc: Some(doc.clone()),
                active: true,
                ..default()
            })
            .init_resource::<MrcSessionRuntime>()
            .add_systems(
                Update,
                (apply_mrc_session_events, apply_mrc_hit_events).chain(),
            );
        let local = app
            .world_mut()
            .spawn((
                LocalFighter,
                test_fighter(),
                PoseRuntime::idle(),
                EntryBlend::default(),
                FighterHitReaction::default(),
                crate::plugins::movement_plugin::FighterAir::default(),
                CombatState::default(),
                SmState::default(),
                Health::default(),
                Qi::default(),
                crate::components::fighter::DeathRespawn::default(),
            ))
            .id();
        let start = doc
            .queue_a
            .iter()
            .find(|entry| entry.id == 50 && (entry.time - 10.99837).abs() < 0.001)
            .expect("recorded ninja Pose 473 start");
        let remote_state = doc
            .queue_a
            .iter()
            .rev()
            .find(|entry| entry.id == 2 && entry.time < start.time)
            .and_then(|entry| {
                entry
                    .blob
                    .chunks_exact(16)
                    .find(|chunk| u16::from_le_bytes(chunk[..2].try_into().unwrap()) == 1)
            })
            .and_then(|chunk| meteor_network::role_state::decode_event2_state(&chunk[2..]).ok())
            .expect("remote event2 before Pose 473");
        app.world_mut()
            .resource_mut::<MrcSessionRuntime>()
            .pending
            .push(MrcSessionEvent::Remote {
                actor_id: 1,
                state: remote_state,
            });
        app.update();
        let remote = app
            .world_mut()
            .query_filtered::<Entity, With<MrcRemoteFighter>>()
            .single(app.world())
            .unwrap();
        route_mrc_session_entry(
            start,
            &mut app.world_mut().resource_mut::<MrcSessionRuntime>(),
        );
        app.update();
        assert_eq!(app.world().get::<PoseRuntime>(remote).unwrap().pose_id, 473);
        assert!((app.world().get::<EntryBlend>(remote).unwrap().duration - 0.10).abs() < 1e-6);
        assert!(
            app.world()
                .get::<PoseRuntime>(remote)
                .unwrap()
                .link_past_end
        );
        let hit = doc
            .queue_a
            .iter()
            .find(|entry| entry.id == 11 && (entry.time - 11.55843).abs() < 0.001)
            .expect("first recorded burn hit");
        let recorded_hit =
            meteor_network::stock_protocol::StockHitReactionEvent::decode(&hit.blob).unwrap();
        assert_eq!(
            (recorded_hit.target_actor_id, recorded_hit.pose_id),
            (2, 109)
        );
        route_mrc_session_entry(
            hit,
            &mut app.world_mut().resource_mut::<MrcSessionRuntime>(),
        );
        app.update();
        assert_eq!(app.world().get::<PoseRuntime>(local).unwrap().pose_id, 109);
        assert_eq!(
            app.world().get::<SmState>(local).unwrap().current,
            meteor_core::FighterState::BeHit
        );
        assert_eq!(
            app.world()
                .get::<FighterHitReaction>(local)
                .unwrap()
                .current
                .as_ref()
                .unwrap()
                .authored_minimum,
            3.0
        );

        let vitals = doc
            .queue_a
            .iter()
            .find(|entry| entry.id == 25 && (entry.time - 11.61444).abs() < 0.001)
            .expect("first post-hit HP snapshot");
        route_mrc_session_entry(
            vitals,
            &mut app.world_mut().resource_mut::<MrcSessionRuntime>(),
        );
        app.update();
        assert_eq!(app.world().get::<Health>(local).unwrap().current, 1960.0);
    }

    #[test]
    fn production_mrc_snapshot_does_not_rewind_natural_link_or_restart_same_pose() {
        use crate::components::fighter::{EntryBlend, PoseRuntime};
        use crate::plugins::character_plugin::{load_character_asset_for_test, CharacterAssetsSet};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character = load_character_asset_for_test(&[root], "p0").expect("original p0");
        let mut assets = CharacterAssetsSet::default();
        assets.0.insert("p0".into(), character);
        let mut app = App::new();
        app.insert_resource(assets);
        app.init_resource::<MrcPlayback>();
        app.init_resource::<MrcSessionRuntime>();
        app.add_systems(
            Update,
            (apply_mrc_session_events, sync_mrc_remote_pose).chain(),
        );
        let mut state = meteor_network::role_state::role_state_from_runtime(
            meteor_network::role_state::RoleStateRuntime {
                position: meteor_network::role_state::RoleVec3::ZERO,
                forward: meteor_network::role_state::forward_from_yaw(0.0),
                velocity: meteor_network::role_state::RoleVec3::ZERO,
                pose_id: 421,
                grounded: true,
                guard_route: false,
                playback_arg: 0,
                pose_sequence: 7,
            },
        );
        app.world_mut()
            .resource_mut::<MrcSessionRuntime>()
            .pending
            .push(MrcSessionEvent::Remote { actor_id: 1, state });
        app.update();
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<MrcRemoteFighter>>()
            .single(app.world())
            .unwrap();

        let mut linked = PoseRuntime::idle();
        app.world().resource::<CharacterAssetsSet>().0["p0"].apply_pose(&mut linked, 422, true);
        linked.frame += 2.0;
        let frame = linked.frame;
        let instance = linked.pose_instance_id;
        app.world_mut()
            .entity_mut(entity)
            .insert((linked, EntryBlend::default()));
        app.update();
        assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().pose_id, 422);
        state.state_words.pose_sequence = 8;
        app.world_mut()
            .resource_mut::<MrcSessionRuntime>()
            .pending
            .push(MrcSessionEvent::Remote { actor_id: 1, state });
        app.update();
        let pose = app.world().get::<PoseRuntime>(entity).unwrap();
        assert_eq!(
            (pose.pose_id, pose.frame, pose.pose_instance_id),
            (422, frame, instance)
        );
        state.state_words.pose_id = 473;
        state.state_words.pose_sequence = 9;
        app.world_mut()
            .resource_mut::<MrcSessionRuntime>()
            .pending
            .push(MrcSessionEvent::Remote { actor_id: 1, state });
        app.update();
        assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().pose_id, 473);
        state.state_words.pose_id = 421;
        state.state_words.pose_sequence = 1;
        app.world_mut()
            .resource_mut::<MrcSessionRuntime>()
            .pending
            .push(MrcSessionEvent::Remote { actor_id: 1, state });
        app.update();
        assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().pose_id, 473);
    }

    #[test]
    fn production_mrc_roster_restores_original_ninja_and_late_hero_identity() {
        use crate::components::fighter::{CharacterVariantChanged, Fighter};
        let mut app = App::new();
        app.init_resource::<MrcPlayback>();
        app.init_resource::<MrcSessionRuntime>();
        app.add_systems(
            Update,
            (apply_mrc_session_events, apply_mrc_identities).chain(),
        );
        let mut doc = empty_mrc_document();
        doc.state[4..8].copy_from_slice(&2u32.to_le_bytes());
        {
            let mut playback = app.world_mut().resource_mut::<MrcPlayback>();
            playback.active = true;
            playback.doc = Some(doc);
        }
        let mut local_fighter = test_fighter();
        local_fighter.character_id = 7;
        let local = app.world_mut().spawn((LocalFighter, local_fighter)).id();

        let roster = [
            2, 2, 1, 2, 1, 1, 56, 1, 1, 1, 1, 1, 1, 1, 1, 9, 0xb8, 0xc4, 0xc3, 0xfb, 0xce, 0xe9,
            0xd4, 0xb2, 1, 1,
        ];
        let state = meteor_network::role_state::role_state_from_runtime(
            meteor_network::role_state::RoleStateRuntime {
                position: meteor_network::role_state::RoleVec3::ZERO,
                forward: meteor_network::role_state::forward_from_yaw(0.0),
                velocity: meteor_network::role_state::RoleVec3::ZERO,
                pose_id: 474,
                grounded: true,
                guard_route: false,
                playback_arg: 0,
                pose_sequence: 3,
            },
        );
        {
            let mut runtime = app.world_mut().resource_mut::<MrcSessionRuntime>();
            route_mrc_session_entry(
                &meteor_asset::mrc::MrcQueueAEntry {
                    id: 4,
                    time: 0.59926,
                    blob: roster.to_vec(),
                },
                &mut runtime,
            );
            runtime
                .pending
                .push(MrcSessionEvent::Remote { actor_id: 1, state });
        }
        app.update();
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<MrcRemoteFighter>>()
            .single(app.world())
            .unwrap();
        let fighter = app.world().get::<Fighter>(entity).unwrap();
        assert_eq!(
            (fighter.character_id, fighter.weapon, fighter.weapon_variant),
            (0, 12, 0)
        );
        assert_eq!(
            app.world()
                .resource::<MrcSessionRuntime>()
                .unhandled_counts
                .get(&4),
            None
        );

        {
            let mut runtime = app.world_mut().resource_mut::<MrcSessionRuntime>();
            for (actor_id, hero) in [(1, 4), (2, 0)] {
                let event = meteor_network::stock_protocol::StockRoleEnteredEvent {
                    actor_id,
                    hero,
                    hero_checksum: 0,
                    camp: 0,
                };
                route_mrc_session_entry(
                    &meteor_asset::mrc::MrcQueueAEntry {
                        id: 9,
                        time: 1.0,
                        blob: event.encode().to_vec(),
                    },
                    &mut runtime,
                );
            }
        }
        app.update();
        assert_eq!(
            app.world()
                .get::<CharacterVariantChanged>(entity)
                .unwrap()
                .requested_character_id,
            4
        );
        assert_eq!(
            app.world()
                .get::<CharacterVariantChanged>(local)
                .unwrap()
                .requested_character_id,
            0
        );

        assert_eq!(app.world().get::<Fighter>(entity).unwrap().character_id, 0);
    }

    #[test]
    fn production_queue_a_projects_vitals_equipment_and_remote_actor() {
        let mut app = App::new();
        app.init_resource::<MrcPlayback>();
        app.init_resource::<MrcSessionRuntime>();
        app.add_systems(Update, apply_mrc_session_events);
        let mut doc = empty_mrc_document();
        doc.state[4..8].copy_from_slice(&3u32.to_le_bytes());
        app.world_mut().resource_mut::<MrcPlayback>().doc = Some(doc);
        let local = app
            .world_mut()
            .spawn((
                LocalFighter,
                test_fighter(),
                crate::components::fighter::Health::default(),
                crate::components::fighter::Qi::default(),
                crate::components::fighter::DeathRespawn::default(),
                crate::components::fighter::CombatState::default(),
                crate::components::fighter::SmState::default(),
                crate::components::fighter::PoseRuntime::idle(),
                crate::components::fighter::EntryBlend::default(),
            ))
            .id();
        let state = meteor_network::role_state::role_state_from_runtime(
            meteor_network::role_state::RoleStateRuntime {
                position: meteor_network::role_state::RoleVec3 {
                    x: 3.0,
                    y: 1.5,
                    z: -4.0,
                },
                forward: meteor_network::role_state::forward_from_yaw(0.75),
                velocity: meteor_network::role_state::RoleVec3::ZERO,
                pose_id: 260,
                grounded: true,
                guard_route: false,
                playback_arg: 0,
                pose_sequence: 9,
            },
        );
        app.world_mut()
            .resource_mut::<MrcSessionRuntime>()
            .pending
            .extend([
                MrcSessionEvent::Equip {
                    item_id: 24,
                    slot: 2,
                },
                MrcSessionEvent::Vitals {
                    hp: 1250.0,
                    max_hp: 2000.0,
                    anger: 70.0,
                },
                MrcSessionEvent::Remote { actor_id: 2, state },
            ]);

        app.update();

        assert_eq!(
            app.world()
                .get::<crate::components::fighter::Health>(local)
                .unwrap()
                .current,
            1250.0
        );
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::Qi>(local)
                .unwrap()
                .current,
            70.0
        );
        let mut query = app.world_mut().query::<(
            &MrcRemoteFighter,
            &crate::components::fighter::Fighter,
            &crate::plugins::network_plugin::RemoteTransformTarget,
        )>();
        let remotes: Vec<_> = query.iter(app.world()).collect();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].0.actor_id, 2);
        assert_eq!(remotes[0].1.weapon, 6);
        assert_eq!(remotes[0].1.weapon_variant, 2);
        assert!(remotes[0]
            .2
            .translation
            .abs_diff_eq(Vec3::new(3.0, 1.5, -4.0), 1e-6));
        drop(remotes);

        app.world_mut()
            .resource_mut::<MrcSessionRuntime>()
            .pending
            .push(MrcSessionEvent::Vitals {
                hp: 0.0,
                max_hp: 2000.0,
                anger: 100.0,
            });
        app.update();
        assert!(
            app.world()
                .get::<crate::components::fighter::DeathRespawn>(local)
                .unwrap()
                .active
        );
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::SmState>(local)
                .unwrap()
                .current,
            meteor_core::FighterState::Dead
        );
        app.world_mut()
            .resource_mut::<MrcSessionRuntime>()
            .pending
            .push(MrcSessionEvent::Vitals {
                hp: 2000.0,
                max_hp: 2000.0,
                anger: 0.0,
            });
        app.update();
        assert!(
            !app.world()
                .get::<crate::components::fighter::DeathRespawn>(local)
                .unwrap()
                .active
        );
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::SmState>(local)
                .unwrap()
                .current,
            meteor_core::FighterState::Name
        );
    }

    #[test]
    fn origin_res_mrc_demo_inputs_map_to_keys() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let dir = root.join("mrcfile");
        if !dir.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let keymap = Keymap::default();
        let mut mapped = 0usize;
        let mut packed_ok = 0usize;
        let mut pose_ok = 0usize;
        let mut facing_ok = 0usize;
        for entry in std::fs::read_dir(&dir).unwrap().take(6) {
            let entry = entry.unwrap();
            let bytes = std::fs::read(entry.path()).unwrap();
            let doc = meteor_asset::mrc::decode_mrc(&bytes)
                .unwrap_or_else(|e| panic!("{}: {e}", entry.path().display()));
            for b in &doc.queue_b {
                if mrc_code_to_key(b.code, &keymap).is_some() {
                    mapped += 1;
                }
            }
            for c in &doc.queue_c {
                let forward = (c.packed as i32 & 0xff) - 1;
                let strafe = ((c.packed as i32 >> 8) & 0xff) - 1;
                assert!(
                    (-1..=1).contains(&forward) && (-1..=1).contains(&strafe),
                    "packed axes in [-1,1]: f={forward} s={strafe}"
                );
                packed_ok += 1;
            }
            for d in &doc.queue_d {
                assert!(decode_mrc_pose(d).is_some());
                pose_ok += 1;
            }
            for e in &doc.queue_e {
                assert!(decode_mrc_facing(e).is_some());
                facing_ok += 1;
            }
        }
        assert!(mapped > 0, "golden demos carry mappable command-49 inputs");
        assert!(packed_ok > 0, "golden demos carry packed move axes");
        assert!(pose_ok > 0, "golden demos carry finite queue-D positions");
        assert!(facing_ok > 0, "golden demos carry finite queue-E facing");
    }
}

#[cfg(test)]
mod export_tests {
    use super::*;

    #[test]
    fn export_mrc_round_trips_key_events() {
        let keymap = Keymap::default();
        let events = vec![
            ReplayEvent {
                time: 0.1,
                key: InputBinding::Key(KeyCode::KeyW),
                pressed: true,
            },
            ReplayEvent {
                time: 0.5,
                key: InputBinding::Mouse(MouseButton::Left),
                pressed: true,
            },
            ReplayEvent {
                time: 0.8,
                key: InputBinding::Key(KeyCode::KeyW),
                pressed: false,
            },
        ];
        let bytes = export_mrc(&events, &keymap);
        let doc = meteor_asset::mrc::decode_mrc(&bytes).expect("mrc decodes");

        assert_eq!(doc.queue_b.len(), 3);
        assert_eq!(doc.queue_b[0].code, 0x57);
        assert_eq!(doc.queue_b[0].value, 1);
        assert!((doc.queue_b[0].time - 0.1).abs() < 1e-6);
        assert_eq!(doc.queue_b[1].code, 0x1000);
        assert_eq!(doc.queue_b[2].value, 0);

        let stray = vec![ReplayEvent {
            time: 0.1,
            key: InputBinding::Key(KeyCode::F13),
            pressed: true,
        }];
        let doc2 = meteor_asset::mrc::decode_mrc(&export_mrc(&stray, &keymap)).unwrap();
        assert!(doc2.queue_b.is_empty());
    }
}

#[cfg(test)]
pub(crate) fn install_mrc_motion_audit(
    app: &mut App,
    mut doc: meteor_asset::mrc::MrcDocument,
    start: f32,
    centered_graph: meteor_scene::stage_loader::WaypointGraph,
    position_corrections: bool,
) {
    if !position_corrections {
        doc.queue_d.clear();
    }
    app.insert_resource(MrcPlayback {
        active: true,
        elapsed: start,
        doc: Some(doc),
        ..default()
    })
    .insert_resource(centered_graph)
    .init_resource::<ReplaySeekRequest>()
    .init_resource::<MrcFrameOverride>()
    .init_resource::<MrcSessionRuntime>()
    .add_systems(
        Update,
        tick_mrc_playback
            .in_set(crate::plugins::FighterFrameSet::Movement)
            .before(crate::plugins::input_plugin::capture_local_command_input)
            .run_if(crate::plugins::simulation_frame),
    );
}
