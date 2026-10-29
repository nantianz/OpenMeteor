use crate::app_state::{AppState, InputBinding, Keymap};
use crate::components::fighter::MotionState;
use crate::plugins::camera_plugin::CameraBasis;
use crate::plugins::input_plugin::{SyntheticAction, SyntheticInputStream};
use crate::plugins::stage_plugin::LocalFighter;
use bevy::prelude::*;
use meteor_core::MODEL_SCALE;
use meteor_input::command_parser::Dir;
use std::collections::HashMap;
use std::path::Path;

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
}

#[derive(Resource, Default)]
pub(crate) struct MrcFrameOverride {
    pub(crate) position_applied: bool,
    pub(crate) facing_applied: bool,
}

#[derive(Debug, Clone, Copy)]
enum MrcSessionEvent {
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
}

#[derive(Resource, Default)]
struct MrcSessionRuntime {
    pending: Vec<MrcSessionEvent>,
    equip_by_slot: HashMap<u32, (u8, u8)>,
    event_counts: HashMap<u32, u64>,
    unhandled_counts: HashMap<u32, u64>,
    rebuild_remotes: bool,
}

#[derive(Component, Debug, Clone, Copy)]
struct MrcRemoteFighter {
    actor_id: u16,
    pose_sequence: u16,
    desired_pose_id: u16,
    desired_pose_sequence: u16,
}

impl MrcPlayback {
    pub fn duration(&self) -> f32 {
        self.doc.as_ref().map(|d| d.duration()).unwrap_or(0.0)
    }

    pub fn load(&mut self, path: &Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let doc = meteor_asset::mrc::decode_mrc(&bytes).map_err(|e| e.to_string())?;
        self.doc = Some(doc);
        self.index_a = 0;
        self.index_b = 0;
        self.index_c = 0;
        self.index_d = 0;
        self.index_e = 0;
        self.elapsed = 0.0;
        self.prev_c = 0;
        self.active = true;
        self.paused = false;
        self.file_name = path.display().to_string();
        Ok(())
    }

    fn state_word(&self, index: usize) -> Option<u32> {
        let state = &self.doc.as_ref()?.state;
        let offset = index.checked_mul(4)?;
        let bytes = state.get(offset..offset + 4)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }

    pub(crate) fn local_slot(&self) -> Option<u32> {
        self.state_word(1).filter(|slot| *slot > 0)
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
        _ => {}
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

#[allow(clippy::too_many_arguments)]
fn tick_mrc_playback(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    keymap: Res<Keymap>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut synthetic: ResMut<SyntheticInputStream>,
    mut playback: ResMut<MrcPlayback>,
    seek: Res<ReplaySeekRequest>,
    mut frame_override: ResMut<MrcFrameOverride>,
    mut session: ResMut<MrcSessionRuntime>,
    mut basis: ResMut<CameraBasis>,
    mut fighters: Query<(&mut Transform, &mut MotionState), With<LocalFighter>>,
) {
    frame_override.position_applied = false;
    frame_override.facing_applied = false;
    if seek.0.is_some() {
        return;
    }
    if !playback.active || playback.paused {
        return;
    }
    playback.elapsed += crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let t = playback.elapsed;
    synthetic.set_frame_clock_seconds(f64::from(t));

    let (index_a, index_b, index_c, index_d, index_e, packed, pose, facing, finished) = {
        let Some(doc) = playback.doc.as_ref() else {
            return;
        };
        let mut i_a = playback.index_a;
        while i_a < doc.queue_a.len() && doc.queue_a[i_a].time <= t {
            route_mrc_session_entry(&doc.queue_a[i_a], &mut session);
            i_a += 1;
        }
        let mut pose = None;
        let mut i_d = playback.index_d;
        while i_d < doc.queue_d.len() && doc.queue_d[i_d].time <= t {
            pose = decode_mrc_pose(&doc.queue_d[i_d]).or(pose);
            i_d += 1;
        }
        let mut facing = None;
        let mut i_e = playback.index_e;
        while i_e < doc.queue_e.len() && doc.queue_e[i_e].time <= t {
            facing = decode_mrc_facing(&doc.queue_e[i_e]).or(facing);
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
                    synthetic.push_action(
                        binding,
                        action,
                        entry.value != 0,
                        Some(f64::from(entry.time)),
                    );
                }
            } else if let Some(binding) = mrc_code_to_key(entry.code, &keymap) {
                inject_replay_binding(
                    &mut keys,
                    &mut mouse,
                    &mut synthetic,
                    binding,
                    entry.value != 0,
                    Some(f64::from(entry.time)),
                );
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
        )
    };
    playback.index_a = index_a;
    playback.index_b = index_b;
    playback.index_c = index_c;
    playback.index_d = index_d;
    playback.index_e = index_e;
    if pose.is_some() || facing.is_some() {
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
        crate::components::fighter::EntryBlendKind::Network,
        false,
        false,
        Some(meteor_combat::weapon_strategy::weapon_to_strategy(
            fighter.weapon,
        )),
    );
    true
}

#[allow(clippy::too_many_arguments)]
fn apply_mrc_session_events(
    mut commands: Commands,
    playback: Res<MrcPlayback>,
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    mut session: ResMut<MrcSessionRuntime>,
    mut local: MrcLocalVitalsQuery<'_, '_>,
    mut remotes: MrcRemoteQuery<'_, '_>,
) {
    let events = std::mem::take(&mut session.pending);
    let rebuild_remotes = std::mem::take(&mut session.rebuild_remotes);
    if events.is_empty() && !rebuild_remotes {
        return;
    }
    if rebuild_remotes {
        for (entity, ..) in &mut remotes {
            commands.entity(entity).despawn();
        }
    }
    let mut vitals = None;
    let mut remote_states = HashMap::new();
    let local_slot = playback.local_slot();
    for event in &events {
        match *event {
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
            )) = remotes
                .iter_mut()
                .find(|(_, remote, ..)| remote.actor_id == actor_id)
            {
                remote.desired_pose_id = state.state_words.pose_id;
                remote.desired_pose_sequence = state.state_words.pose_sequence;
                target.translation = position;
                target.yaw = yaw;
                motion.velocity = velocity;
                motion.facing = yaw;
                motion.movement_mode = role_movement_mode(state);
                air.air.airborne = !state.state_words.grounded;
                air.ground_contact = state.state_words.grounded;
                if (pose.pose_id != state.state_words.pose_id
                    || remote.pose_sequence != state.state_words.pose_sequence)
                    && apply_mrc_remote_pose(
                        assets.as_deref(),
                        &fighter,
                        &mut pose,
                        &mut blend,
                        state.state_words.pose_id,
                    )
                {
                    remote.pose_sequence = state.state_words.pose_sequence;
                }
                continue;
            }
        }

        let character_id = (actor_id.saturating_sub(1) % 20) as u8;
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
                    desired_pose_id: state.state_words.pose_id,
                    desired_pose_sequence: state.state_words.pose_sequence,
                },
                crate::plugins::network_plugin::RemoteFighter {
                    player_id: format!("mrc:{actor_id}"),
                },
                fighter,
                crate::components::fighter::Health {
                    current: 2_000.0,
                    max: 2_000.0,
                },
                crate::components::fighter::Qi::default(),
                crate::components::fighter::CombatState::default(),
                MotionState {
                    movement_mode: role_movement_mode(state),
                    velocity,
                    facing: yaw,
                },
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
                Transform::from_translation(position),
                crate::plugins::network_plugin::RemoteTransformTarget {
                    translation: position,
                    yaw,
                },
                crate::components::fighter::TargetHitClock::default(),
                crate::components::fighter::VisualRootPin::default(),
                Visibility::default(),
                InheritedVisibility::default(),
            ),
        ));
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

fn sync_mrc_remote_pose(
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    mut remotes: Query<(
        &mut MrcRemoteFighter,
        &crate::components::fighter::Fighter,
        &mut crate::components::fighter::PoseRuntime,
        &mut crate::components::fighter::EntryBlend,
    )>,
) {
    for (mut remote, fighter, mut pose, mut blend) in &mut remotes {
        if (pose.pose_id != remote.desired_pose_id
            || remote.pose_sequence != remote.desired_pose_sequence)
            && apply_mrc_remote_pose(
                assets.as_deref(),
                fighter,
                &mut pose,
                &mut blend,
                remote.desired_pose_id,
            )
        {
            remote.pose_sequence = remote.desired_pose_sequence;
        }
    }
}

fn mrc_code_to_key(code: u32, keymap: &Keymap) -> Option<InputBinding> {
    Some(match code {
        0x57 => keymap.move_up,
        0x53 => keymap.move_down,
        0x41 => keymap.move_left,
        0x44 => keymap.move_right,
        0x10 => keymap.run,
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
        keymap.run,
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
    matches!(
        code,
        0x57 | 0x53 | 0x41 | 0x44 | 0x10 | 0x26 | 0x28 | 0x25 | 0x27
    )
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
    if packed & 0x10000 != 0 {
        set_replay_held_binding(keys, mouse, synthetic, keymap.run, true);
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_replay_seek(
    mut request: ResMut<ReplaySeekRequest>,
    keymap: Res<Keymap>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut player: ResMut<ReplayPlayer>,
    mut playback: ResMut<MrcPlayback>,
    mut session: ResMut<MrcSessionRuntime>,
    mut frame_override: ResMut<MrcFrameOverride>,
    mut basis: ResMut<CameraBasis>,
    mut fighters: Query<(&mut Transform, &mut MotionState), With<LocalFighter>>,
) {
    let Some(target) = request.0.take() else {
        return;
    };

    let continuous = continuous_replay_inputs(&keymap);
    for binding in &continuous {
        match *binding {
            InputBinding::Key(key) => keys.reset(key),
            InputBinding::Mouse(button) => mouse.reset(button),
        }
    }

    if playback.active {
        let t = target.clamp(0.0, playback.duration());
        session.pending.clear();
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
                route_mrc_session_entry(&doc.queue_a[i_a], &mut session);
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
                pose = decode_mrc_pose(&doc.queue_d[i_d]).or(pose);
                i_d += 1;
            }
            let mut facing = None;
            let mut i_e = 0;
            while i_e < doc.queue_e.len() && doc.queue_e[i_e].time <= t {
                facing = decode_mrc_facing(&doc.queue_e[i_e]).or(facing);
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

pub struct ReplayPlugin;

impl Plugin for ReplayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ReplayRecorder>()
            .init_resource::<ReplayPlayer>()
            .init_resource::<ReplaySeekRequest>()
            .init_resource::<MrcPlayback>()
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
                (
                    tick_mrc_playback,
                    apply_replay_seek,
                    apply_mrc_session_events,
                )
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .after(crate::plugins::camera_plugin::prepare_move_basis)
                    .before(crate::plugins::input_plugin::capture_local_command_input)
                    .before(crate::plugins::ai_plugin::drive_npc_ai)
                    .run_if(in_state(AppState::Playing))
                    .run_if(crate::plugins::simulation_frame),
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
                    .run_if(crate::plugins::simulation_frame),
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
    mut frame_override: ResMut<MrcFrameOverride>,
    session: Res<MrcSessionRuntime>,
) {
    if playback.active {
        for binding in record_inputs(&keymap) {
            set_replay_input(&mut keys, &mut mouse, binding, false);
        }
    }
    playback.active = false;
    playback.paused = false;
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
    request: Res<crate::app_state::LaunchRequest>,
) {
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
    session.pending.clear();
    session.equip_by_slot.clear();
    session.event_counts.clear();
    session.unhandled_counts.clear();
    session.rebuild_remotes = false;
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
        (keymap.run, 0x10),
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
        assert_eq!(mrc_code_to_key(0x10, &keymap), Some(keymap.run));
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
                    velocity: Vec3::new(4.0, 0.0, 5.0),
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
        assert_eq!(motion.velocity.x, 0.0);
        assert_eq!(motion.velocity.z, 0.0);
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
