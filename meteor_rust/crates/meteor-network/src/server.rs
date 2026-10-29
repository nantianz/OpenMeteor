use crate::combat_authority::{
    decode_packed_obb, obb_center_near, AttackCatalog, AttackCatalogError, AttackRecord,
    ATTACK_SOURCE_EFFECT, ATTACK_SOURCE_PRECISE, MAX_ATTACK_OBB_HALF_SIZE, MAX_HURT_OBB_HALF_SIZE,
};
use crate::escort::{character_capsule_intersects_flag, escort_stage, position_inside_safe_zone};
use crate::host_authority::{clamp_frame_dt, InputAuthority, InputReject, InputStamp};
use crate::modes::{
    can_respawn, is_escort_stage, is_siege_stage, is_team_mode, mode_rule_id,
    round_time_limit_seconds, RESPAWN_DELAY_SECONDS,
};
use crate::protocol::{
    CombatReactionWire, ItemEffectWire, ItemWeaponSlotWire, Message, RoomMemberWire, RoomRulesWire,
    RoomSummary, SceneOp,
};
use crate::role_state::{
    decode_request1_state, encode_event2_state, yaw_from_forward, RoleState, RoleVec3,
    ROLE_DOWNLINK_HZ, ROLE_UPLINK_HZ,
};
use crate::wire_v4::{
    CombatJudge as V4CombatJudge, Input as V4Input, InputVisual as V4InputVisual,
    ItemEffect as V4ItemEffect, RoleSnapshot as V4RoleSnapshot,
    RoleStateRecord as V4RoleStateRecord, Snapshot as V4Snapshot, SnapshotDrop as V4SnapshotDrop,
    SnapshotPlayer, Vec3, WeaponSlot as V4WeaponSlot,
};
use meteor_combat::attack_window::TargetHitClock;
use meteor_combat::hit_resolve::{
    guard_anger_gain, resolve_hit, weapon_base_atk, CombatStats, HitOptions,
};
use meteor_combat::hitbox::{obbs_intersect, Obb};
use meteor_combat::ranged_weapons::{
    ranged_attack_profile, ranged_target_box, segment_hits_box_distance, Point3 as RangedPoint3,
    GUN_RANGE, GUN_SPAWN_HEIGHT,
};
use rand::{seq::SliceRandom, RngCore};
use serde_json::{json, Value as JsonValue};
use std::collections::{HashMap, HashSet};
use std::path::Path;

const DEFAULT_MAX_PLAYERS: usize = 16;
const MAX_SCENE_OPS_PER_TRANSACTION: usize = 64;
const MAX_SCENE_STATE_OPS: usize = 1024;
const ROLE_RATE_MIN: u32 = 8;
const ROLE_RATE_MAX: u32 = 20;
const ROLE_RATE_SAMPLE_SECONDS: f32 = 1.0;
const ROLE_RATE_FEEDBACK_THRESHOLD: i32 = 3;
const PLAYER_BASE_HEALTH: f32 = 1000.0;
const PLAYER_ANGER_MAX: f32 = 100.0;
const PROJECTILE_TRACK_MAX_AGE_SECONDS: f32 = 12.0;
const COLLISION_SNAPSHOT_MAX_AGE: f32 = 0.5;
const PRECISE_BOX_REACH_XZ: f32 = 10.0;
const PRECISE_BOX_REACH_Y: f32 = 14.0;
const ITEM_CLAIM_MAX_DISTANCE: f32 = 6.0;
const MAX_ITEM_REFERENCE_LEN: usize = 64;
const MAX_ITEM_MODEL_LEN: usize = 64;

const WEAPON_DROP_HEIGHT: f32 = 50.0 * 0.12;

const WEAPON_DROP_SPEED: f32 = 100.0 * 0.12;
const MAX_DYNAMIC_DROPS_PER_PLAYER: usize = 8;
const MAX_DYNAMIC_DROPS_PER_ROOM: usize = 128;
const EFFECT_BOX_REACH_XZ: f32 = 48.0;
const EFFECT_BOX_REACH_Y: f32 = 48.0;
const CLAIM_STALENESS_SLACK_XZ: f32 = 3.0;
const MAX_COMBAT_JUDGES: usize = 8;
const MAX_ATTACK_OBBS: usize = 24;
const MAX_HURT_OBBS: usize = 24;
const MAX_CLAIM_TARGETS: usize = 4;
const MAX_LIVE_COMBAT_INSTANCES: usize = 32;
const MAX_PROJECTILE_STEP: f32 = 5.0;
const MAX_PROJECTILE_CONTINUITY_ERROR: f32 = 5.0;
const GUN_CAMERA_ORIGIN_MAX_DISTANCE: f32 = 24.0;
const GUN_CAMERA_OWNER_RAY_TOLERANCE: f32 = 6.0;

const CURE_RANGE: f32 = 80.0 * 0.12;

fn new_session_token() -> String {
    let mut bytes = [0_u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

const CURE_RECOVERY_SECONDS: f32 = 91.0 / 30.0;
const GEOMETRY_POSITION_MAX_ERROR_XZ: f32 = 6.0;

const ROUND_SETTLE_SECONDS: f32 = 10.0;

#[derive(Debug, Clone)]
struct ProjectileTrack {
    end: RoleVec3,
    updated_at: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoundPhase {
    Active,
    Break,
}

#[derive(Debug, Clone)]
struct RoundState {
    number: u32,
    phase: RoundPhase,
    ends_at: f32,
    next_round_at: Option<f32>,
}

#[derive(Debug, Clone)]
struct PendingRoundEnd {
    winner_team: Option<i32>,
    reason: &'static str,
}

#[derive(Debug, Clone)]
struct EscortFlag {
    home: RoleVec3,
    position: RoleVec3,
    carrier_id: Option<String>,
    dropped_at: Option<f32>,
}

fn room_state_label(room: &Room) -> &'static str {
    if room.lobby_return_at.is_some() {
        "ended"
    } else if room.started {
        "running"
    } else {
        "lobby"
    }
}

#[derive(Debug, Clone, Copy)]
struct ItemSegment {
    property: i32,
    operation: i32,
    value: i32,
    periodic: bool,
}

#[derive(Debug, Clone, Copy)]
struct ItemDefinition {
    id: u32,
    duration: f32,
    tick_every: f32,
    worn_duration: Option<f32>,
    segments: &'static [ItemSegment],
    refine_variant: Option<u8>,
}

#[derive(Debug, Clone)]
struct ActiveItemEffect {
    model: String,
    expires_at: f32,
    tick_every: f32,
    next_tick_at: f32,
    segments: Vec<ItemSegment>,
}

#[derive(Debug, Clone)]
struct WornItemEffect {
    model: String,
    expires_at: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct ItemEffectUpdate {
    changed: bool,
    died: bool,
}

#[derive(Debug, Clone)]
struct WeaponSlotState {
    weapon_id: u8,
    weapon: String,
    model: String,
    resource_class: u8,
    variant: u8,
    attack_power: f32,
    defense: f32,
}

impl WeaponSlotState {
    fn wire(&self, index: usize) -> ItemWeaponSlotWire {
        ItemWeaponSlotWire {
            index: index as u8,
            present: true,
            weapon: self.weapon.clone(),
            weapon_id: Some(self.weapon_id),
            model: Some(self.model.clone()),
            resource_class: self.resource_class,
            variant: self.variant,
            attack_power: self.attack_power,
            defense_power: self.defense,
        }
    }
}

#[derive(Debug, Clone)]
struct ClaimedSceneItem {
    model: String,
    respawn_at: Option<f32>,
}

#[derive(Debug, Clone)]
struct DynamicWeaponDrop {
    model: String,
    rest_position: RoleVec3,
    owner_id: String,
    source: DynamicWeaponDropSource,
    claimable_at: f32,
    expires_at: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DynamicWeaponDropSource {
    DropWeapon,
    DropItem,
    Death,
}

const HP_300: [ItemSegment; 1] = [ItemSegment {
    property: 2,
    operation: 0,
    value: 300,
    periodic: false,
}];
const HP_500: [ItemSegment; 1] = [ItemSegment {
    property: 2,
    operation: 0,
    value: 500,
    periodic: false,
}];
const HP_100: [ItemSegment; 1] = [ItemSegment {
    property: 2,
    operation: 0,
    value: 100,
    periodic: false,
}];
const HP_50: [ItemSegment; 1] = [ItemSegment {
    property: 2,
    operation: 0,
    value: 50,
    periodic: false,
}];
const ANGER_100: [ItemSegment; 1] = [ItemSegment {
    property: 6,
    operation: 0,
    value: 100,
    periodic: false,
}];
const ANGER_25: [ItemSegment; 1] = [ItemSegment {
    property: 6,
    operation: 0,
    value: 25,
    periodic: false,
}];
const ANGER_1: [ItemSegment; 1] = [ItemSegment {
    property: 6,
    operation: 0,
    value: 1,
    periodic: false,
}];
const DEF_20: [ItemSegment; 1] = [ItemSegment {
    property: 4,
    operation: 0,
    value: 20,
    periodic: false,
}];
const DEF_100: [ItemSegment; 1] = [ItemSegment {
    property: 4,
    operation: 0,
    value: 100,
    periodic: false,
}];
const ATK_25: [ItemSegment; 1] = [ItemSegment {
    property: 3,
    operation: 0,
    value: 25,
    periodic: false,
}];
const ATK_125: [ItemSegment; 1] = [ItemSegment {
    property: 3,
    operation: 0,
    value: 125,
    periodic: false,
}];
const MAX_HP_500: [ItemSegment; 1] = [ItemSegment {
    property: 1,
    operation: 0,
    value: 500,
    periodic: false,
}];
const MAX_HP_6999: [ItemSegment; 1] = [ItemSegment {
    property: 1,
    operation: 0,
    value: 6999,
    periodic: false,
}];
const SPEED_150: [ItemSegment; 1] = [ItemSegment {
    property: 5,
    operation: 1,
    value: 150,
    periodic: false,
}];
const CONCEALMENT: [ItemSegment; 1] = [ItemSegment {
    property: 10,
    operation: 0,
    value: 1,
    periodic: false,
}];
const POISON: [ItemSegment; 1] = [ItemSegment {
    property: 2,
    operation: 0,
    value: -10,
    periodic: true,
}];
const HOT: [ItemSegment; 1] = [ItemSegment {
    property: 6,
    operation: 0,
    value: 1,
    periodic: true,
}];

fn item_definition(model: &str) -> Option<ItemDefinition> {
    Some(match model.trim().to_ascii_lowercase().as_str() {
        "item01" => ItemDefinition {
            id: 1,
            duration: 0.0,
            tick_every: 0.0,
            worn_duration: None,
            segments: &HP_300,
            refine_variant: None,
        },
        "item02" => ItemDefinition {
            id: 2,
            duration: 600.0,
            tick_every: 0.0,
            worn_duration: Some(600.0),
            segments: &MAX_HP_500,
            refine_variant: None,
        },
        "item03" => ItemDefinition {
            id: 3,
            duration: 90.0,
            tick_every: 0.0,
            worn_duration: Some(90.0),
            segments: &DEF_20,
            refine_variant: None,
        },
        "item04" => ItemDefinition {
            id: 4,
            duration: 60.0,
            tick_every: 0.0,
            worn_duration: Some(60.0),
            segments: &ATK_25,
            refine_variant: None,
        },
        "item05" => ItemDefinition {
            id: 5,
            duration: 60.0,
            tick_every: 0.0,
            worn_duration: Some(60.0),
            segments: &SPEED_150,
            refine_variant: None,
        },
        "item07" => ItemDefinition {
            id: 7,
            duration: 60.0,
            tick_every: 0.0,
            worn_duration: None,
            segments: &CONCEALMENT,
            refine_variant: None,
        },
        "item08" => ItemDefinition {
            id: 8,
            duration: 0.0,
            tick_every: 0.0,
            worn_duration: None,
            segments: &ANGER_100,
            refine_variant: None,
        },
        "item09" => ItemDefinition {
            id: 9,
            duration: 0.0,
            tick_every: 0.0,
            worn_duration: None,
            segments: &[],
            refine_variant: Some(0),
        },
        "item0101" => ItemDefinition {
            id: 16,
            duration: 0.0,
            tick_every: 0.0,
            worn_duration: None,
            segments: &HP_500,
            refine_variant: None,
        },
        "item0102" => ItemDefinition {
            id: 17,
            duration: 0.0,
            tick_every: 0.0,
            worn_duration: None,
            segments: &HP_100,
            refine_variant: None,
        },
        "item0103" => ItemDefinition {
            id: 18,
            duration: 30.0,
            tick_every: 5.0,
            worn_duration: Some(30.0),
            segments: &POISON,
            refine_variant: None,
        },
        "item0901" => ItemDefinition {
            id: 23,
            duration: 0.0,
            tick_every: 0.0,
            worn_duration: None,
            segments: &[],
            refine_variant: Some(1),
        },
        "item0902" => ItemDefinition {
            id: 24,
            duration: 0.0,
            tick_every: 0.0,
            worn_duration: None,
            segments: &[],
            refine_variant: Some(2),
        },
        "item0903" => ItemDefinition {
            id: 25,
            duration: 0.0,
            tick_every: 0.0,
            worn_duration: None,
            segments: &[],
            refine_variant: Some(3),
        },
        "item0904" => ItemDefinition {
            id: 26,
            duration: 0.0,
            tick_every: 0.0,
            worn_duration: None,
            segments: &[],
            refine_variant: Some(4),
        },
        "item10" => ItemDefinition {
            id: 27,
            duration: 60.0,
            tick_every: 0.0,
            worn_duration: Some(60.0),
            segments: &MAX_HP_6999,
            refine_variant: None,
        },
        "item11" => ItemDefinition {
            id: 28,
            duration: 0.0,
            tick_every: 0.0,
            worn_duration: None,
            segments: &ANGER_25,
            refine_variant: None,
        },
        "item12" => ItemDefinition {
            id: 29,
            duration: 9999.9,
            tick_every: 0.0,
            worn_duration: Some(9999.9),
            segments: &DEF_100,
            refine_variant: None,
        },
        "item13" => ItemDefinition {
            id: 30,
            duration: 9999.9,
            tick_every: 0.0,
            worn_duration: Some(9999.9),
            segments: &ATK_125,
            refine_variant: None,
        },
        "item14" => ItemDefinition {
            id: 31,
            duration: 9999.9,
            tick_every: 0.0,
            worn_duration: Some(9999.9),
            segments: &SPEED_150,
            refine_variant: None,
        },
        "item15" => ItemDefinition {
            id: 32,
            duration: 0.0,
            tick_every: 0.0,
            worn_duration: Some(9999.9),
            segments: &HP_50,
            refine_variant: None,
        },
        "item16" => ItemDefinition {
            id: 33,
            duration: 0.0,
            tick_every: 0.0,
            worn_duration: Some(9999.9),
            segments: &ANGER_1,
            refine_variant: None,
        },
        "hot" => ItemDefinition {
            id: 39,
            duration: 9999.9,
            tick_every: 1.0,
            worn_duration: Some(9999.9),
            segments: &HOT,
            refine_variant: None,
        },
        _ => return None,
    })
}

fn weapon_resource_class(weapon_id: u8) -> Option<u8> {
    Some(match weapon_id {
        6 => 0,
        5 => 1,
        3 => 2,
        1 => 3,
        9 => 4,
        8 => 5,
        2 => 6,
        7 => 7,
        4 => 8,
        10 => 9,
        11 => 10,
        12 => 11,
        _ => return None,
    })
}

fn weapon_id_for_resource_class(resource_class: u8) -> Option<u8> {
    Some(match resource_class {
        0 => 6,
        1 => 5,
        2 => 3,
        3 => 1,
        4 => 9,
        5 => 8,
        6 => 2,
        7 => 7,
        8 => 4,
        9 => 10,
        10 => 11,
        11 => 12,
        _ => return None,
    })
}

fn weapon_slot_for_model(catalog: &AttackCatalog, model: &str) -> Option<WeaponSlotState> {
    let normalized = model.trim().to_ascii_lowercase();
    let rest = normalized.strip_prefix('w')?;
    let (resource_class, variant_with_suffix) = rest.split_once('_')?;
    let resource_class = resource_class.parse::<u8>().ok()?;
    let variant = variant_with_suffix
        .trim_end_matches(['r', 'l', 'a', 'b'])
        .parse::<u8>()
        .ok()?;
    let weapon_id = weapon_id_for_resource_class(resource_class)?;
    let stats = catalog.weapon_stats_for_model(&normalized)?;
    Some(WeaponSlotState {
        weapon_id,
        weapon: web_weapon_name(weapon_id).to_string(),
        model: format!("w{resource_class}_{variant}"),
        resource_class,
        variant,
        attack_power: stats.attack as f32,
        defense: stats.defense as f32,
    })
}

fn base_weapon_slot(catalog: Option<&AttackCatalog>, weapon_id: u8) -> WeaponSlotState {
    let resource_class = weapon_resource_class(weapon_id).unwrap_or(0);
    let model = format!("w{resource_class}_0");
    catalog
        .and_then(|catalog| weapon_slot_for_model(catalog, &model))
        .unwrap_or_else(|| WeaponSlotState {
            weapon_id,
            weapon: web_weapon_name(weapon_id).to_string(),
            model,
            resource_class,
            variant: 0,
            attack_power: weapon_base_atk(weapon_id) as f32,
            defense: 0.0,
        })
}

fn room_allows_item(rules: &RoomRulesWire, category: u32, id: u32) -> bool {
    match category {
        1 => rules.hide_item.unwrap_or(true),
        2 => match id {
            1 | 9 | 16..=18 | 23..=27 => rules.health_item.unwrap_or(true),
            2..=5 | 28..=33 | 39 => rules.update_item.unwrap_or(true),
            6..=8 => rules.special_item.unwrap_or(true),
            _ => false,
        },
        _ => false,
    }
}

fn valid_item_claim_fields(
    reference: &str,
    player_id: &str,
    model: &str,
    position: Option<[f32; 3]>,
) -> bool {
    !reference.trim().is_empty()
        && reference.len() <= MAX_ITEM_REFERENCE_LEN
        && !player_id.trim().is_empty()
        && player_id.len() <= 32
        && !model.trim().is_empty()
        && model.len() <= MAX_ITEM_MODEL_LEN
        && position.is_none_or(|position| position.into_iter().all(f32::is_finite))
}

fn round_drop_component(value: f32) -> f32 {
    (value * 10_000.0).round() / 10_000.0
}

fn weapon_drop_trajectory(position: RoleVec3, yaw: f32) -> ([f32; 3], [f32; 3]) {
    let yaw = if yaw.is_finite() { yaw } else { 0.0 };
    let velocity = [
        yaw.sin() * WEAPON_DROP_SPEED,
        0.0,
        yaw.cos() * WEAPON_DROP_SPEED,
    ];
    let position = [position.x, position.y + WEAPON_DROP_HEIGHT, position.z];
    (
        position.map(round_drop_component),
        velocity.map(round_drop_component),
    )
}

fn can_spawn_dynamic_weapon_drop(
    room: &Room,
    player_id: &str,
    source: DynamicWeaponDropSource,
) -> bool {
    if room.dynamic_weapon_drops.len() >= MAX_DYNAMIC_DROPS_PER_ROOM {
        return false;
    }
    source != DynamicWeaponDropSource::DropItem
        || room
            .dynamic_weapon_drops
            .values()
            .filter(|drop| {
                drop.source == DynamicWeaponDropSource::DropItem && drop.owner_id == player_id
            })
            .count()
            < MAX_DYNAMIC_DROPS_PER_PLAYER
}

fn spawn_dynamic_weapon_drop(
    room: &mut Room,
    player_id: &str,
    dropped: WeaponSlotState,
    source: DynamicWeaponDropSource,
) -> Option<Message> {
    if !can_spawn_dynamic_weapon_drop(room, player_id, source) {
        return None;
    }
    let combatant = room.combatants.get(player_id)?;
    let (position, velocity) = weapon_drop_trajectory(combatant.foot_position(), combatant.yaw);
    let active_weapon = combatant.active_weapon().cloned();
    let weapon_slot = combatant.weapon_slot as u8;
    let weapon_slots = combatant.weapon_slots_wire();
    let reference = format!("D_wpNet{}", room.next_dynamic_drop_id);
    room.next_dynamic_drop_id = room.next_dynamic_drop_id.saturating_add(1);
    room.dynamic_weapon_drops.insert(
        reference.clone(),
        DynamicWeaponDrop {
            model: dropped.model.clone(),
            rest_position: RoleVec3 {
                x: position[0],
                y: position[1],
                z: position[2],
            },
            owner_id: player_id.to_string(),
            source,
            claimable_at: f32::INFINITY,
            expires_at: room.elapsed + 5.0,
        },
    );
    Some(Message::WeaponDropped {
        player_id: player_id.to_string(),
        reference,
        model: dropped.model,
        position,
        velocity,
        weapon: active_weapon
            .as_ref()
            .map(|weapon| weapon.weapon.clone())
            .unwrap_or_default(),
        weapon_model: active_weapon.map(|weapon| weapon.model),
        weapon_slot,
        weapon_slots,
    })
}

#[derive(Debug, Clone)]
struct Combatant {
    hit_status_effects: meteor_combat::status_effects::HitStatusEffects,

    team: Option<i32>,
    base_max_health: f32,
    max_health: f32,
    health: f32,
    anger: f32,
    attack_bonus: f32,
    defense_bonus: f32,
    weapon_slots: [Option<WeaponSlotState>; 2],
    weapon_slot: usize,
    item_effects: Vec<ActiveItemEffect>,
    worn_item_effects: Vec<WornItemEffect>,
    accepted_anger_spends: std::collections::VecDeque<(u32, u16)>,
    last_pose_chain_sequence: u32,
    last_self_effect: Option<(u32, u32)>,
    pending_self_health: i32,
    kills: u32,
    deaths: u32,

    is_leader: bool,

    next_cure_at: f32,

    respawn_at: Option<f32>,
    guard: bool,
    position: RoleVec3,
    yaw: f32,
    pose_id: u16,
    pose_cursor: f32,
    target_hit_clock: TargetHitClock,
    world_position: Option<RoleVec3>,
    world_position_updated_at: f32,
    hurt_obbs: Vec<Obb>,
    hurt_whole_obb: Option<Obb>,
    collision_updated_at: f32,
    projectile_tracks: HashMap<String, ProjectileTrack>,
}

impl Combatant {
    fn new(max_health: f32, team: Option<i32>, base_weapon: WeaponSlotState) -> Self {
        Self {
            team,
            base_max_health: max_health,
            max_health,
            health: max_health,
            anger: 0.0,
            attack_bonus: 0.0,
            defense_bonus: 0.0,
            weapon_slots: [Some(base_weapon), None],
            weapon_slot: 0,
            item_effects: Vec::new(),
            hit_status_effects: Default::default(),
            worn_item_effects: Vec::new(),
            accepted_anger_spends: std::collections::VecDeque::new(),
            last_pose_chain_sequence: 0,
            last_self_effect: None,
            pending_self_health: 0,
            kills: 0,
            deaths: 0,
            is_leader: false,
            next_cure_at: 0.0,
            respawn_at: None,
            guard: false,
            position: RoleVec3::default(),
            yaw: 0.0,
            pose_id: 0,
            pose_cursor: 0.0,
            target_hit_clock: TargetHitClock::new(),
            world_position: None,
            world_position_updated_at: f32::NEG_INFINITY,
            hurt_obbs: Vec::new(),
            hurt_whole_obb: None,
            collision_updated_at: f32::NEG_INFINITY,
            projectile_tracks: HashMap::new(),
        }
    }

    fn foot_position(&self) -> RoleVec3 {
        self.world_position.unwrap_or(self.position)
    }

    fn advance(&mut self, dt: f32, elapsed: f32) {
        self.target_hit_clock.advance(dt);

        if elapsed - self.world_position_updated_at > COLLISION_SNAPSHOT_MAX_AGE {
            self.world_position = None;
        }
        self.projectile_tracks
            .retain(|_, track| elapsed - track.updated_at <= PROJECTILE_TRACK_MAX_AGE_SECONDS);
    }

    fn reset_geometry(&mut self) {
        self.world_position = None;
        self.world_position_updated_at = f32::NEG_INFINITY;
        self.hurt_obbs.clear();
        self.hurt_whole_obb = None;
        self.collision_updated_at = f32::NEG_INFINITY;
    }

    fn revive(&mut self) {
        self.item_effects.clear();
        self.hit_status_effects.active.clear();
        self.worn_item_effects.clear();
        self.accepted_anger_spends.clear();
        self.attack_bonus = 0.0;
        self.defense_bonus = 0.0;
        self.max_health = self.base_max_health;
        self.health = self.max_health;
        self.anger = 0.0;
        self.guard = false;
        self.pose_id = 0;
        self.pose_cursor = 0.0;
        self.next_cure_at = 0.0;
        self.respawn_at = None;
        self.target_hit_clock = TargetHitClock::new();
        self.reset_geometry();
        self.projectile_tracks.clear();
    }

    fn active_weapon(&self) -> Option<&WeaponSlotState> {
        self.weapon_slots.get(self.weapon_slot)?.as_ref()
    }

    fn weapon_slots_wire(&self) -> Vec<ItemWeaponSlotWire> {
        self.weapon_slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                slot.as_ref().map_or(
                    ItemWeaponSlotWire {
                        index: index as u8,
                        present: false,
                        weapon: String::new(),
                        weapon_id: None,
                        model: None,
                        resource_class: 0,
                        variant: 0,
                        attack_power: 0.0,
                        defense_power: 0.0,
                    },
                    |slot| slot.wire(index),
                )
            })
            .collect()
    }

    fn item_effects_wire(&self, elapsed: f32) -> Vec<ItemEffectWire> {
        let mut by_model = HashMap::<&str, f32>::new();
        for effect in &self.item_effects {
            let remaining = (effect.expires_at - elapsed).max(0.0);
            if remaining > 0.0 {
                by_model
                    .entry(&effect.model)
                    .and_modify(|current| *current = current.max(remaining))
                    .or_insert(remaining);
            }
        }
        for effect in &self.worn_item_effects {
            let remaining = (effect.expires_at - elapsed).max(0.0);
            if remaining > 0.0 {
                by_model
                    .entry(&effect.model)
                    .and_modify(|current| *current = current.max(remaining))
                    .or_insert(remaining);
            }
        }
        let mut result: Vec<_> = by_model
            .into_iter()
            .map(|(model, remaining)| ItemEffectWire {
                model: model.to_string(),
                remaining,
            })
            .collect();
        result.extend(self.hit_status_effects.active.iter().filter_map(|effect| {
            let remaining = (effect.expires_at - f64::from(elapsed)) as f32;
            (remaining > 0.0).then(|| ItemEffectWire {
                model: format!("status:{}", effect.definition.id),
                remaining,
            })
        }));
        result
    }

    fn refresh_item_stats(&mut self) {
        let mut max_health = self.base_max_health;
        let mut attack_bonus = 0.0;
        let mut defense_bonus = 0.0;
        for effect in &self.item_effects {
            for segment in &effect.segments {
                if segment.periodic {
                    continue;
                }
                let value = if segment.operation == 1 {
                    match segment.property {
                        1 => self.base_max_health * (segment.value as f32 / 100.0 - 1.0),
                        _ => 0.0,
                    }
                } else {
                    segment.value as f32
                };
                match segment.property {
                    1 => max_health += value,
                    3 => attack_bonus += value,
                    4 => defense_bonus += value,
                    _ => {}
                }
            }
        }
        self.max_health = max_health.max(1.0).trunc();
        self.health = self.health.clamp(0.0, self.max_health);
        self.attack_bonus = attack_bonus.trunc();
        self.defense_bonus = defense_bonus.trunc();
    }

    fn clear_item_effects(&mut self) {
        self.pending_self_health = 0;
        self.item_effects.clear();
        self.hit_status_effects.active.clear();
        self.worn_item_effects.clear();
        self.refresh_item_stats();
    }

    fn apply_item(&mut self, model: &str, definition: ItemDefinition, elapsed: f32) {
        let normalized = model.trim().to_ascii_lowercase();
        if let Some(worn_duration) = definition.worn_duration {
            self.worn_item_effects
                .retain(|effect| effect.model != normalized);
            self.worn_item_effects.push(WornItemEffect {
                model: normalized.clone(),
                expires_at: elapsed + worn_duration.max(0.0),
            });
        }
        let mut persistent = Vec::new();
        for segment in definition.segments {
            match (segment.periodic, segment.property) {
                (false, 2) => {
                    self.health = (self.health + segment.value as f32).clamp(0.0, self.max_health)
                }
                (false, 6) => {
                    self.anger = (self.anger + segment.value as f32).clamp(0.0, PLAYER_ANGER_MAX)
                }
                _ => persistent.push(*segment),
            }
        }
        if !persistent.is_empty() {
            self.item_effects
                .retain(|effect| effect.model != normalized);
            self.item_effects.push(ActiveItemEffect {
                model: normalized,
                expires_at: elapsed + definition.duration.max(0.0),
                tick_every: definition.tick_every.max(0.0),
                next_tick_at: elapsed + definition.tick_every.max(0.0),
                segments: persistent,
            });
            self.refresh_item_stats();
        }
    }

    fn update_item_effects(&mut self, elapsed: f32) -> ItemEffectUpdate {
        let health_before = self.health;
        self.health = (self.health + std::mem::take(&mut self.pending_self_health) as f32)
            .clamp(0.0, self.max_health);
        let max_health_before = self.max_health;
        let anger_before = self.anger;
        let status_before = self.hit_status_effects.active.len();
        let status_delta = self.hit_status_effects.tick(f64::from(elapsed));
        self.health = (self.health + status_delta.health as f32).clamp(0.0, self.max_health);
        self.anger = (self.anger + status_delta.anger as f32).clamp(0.0, PLAYER_ANGER_MAX);
        let active_before = self.item_effects.len();
        let worn_before = self.worn_item_effects.len();
        let mut ticked = false;
        let mut periodic_health_delta = 0_i32;
        for effect in &mut self.item_effects {
            if effect.tick_every <= 0.0 {
                continue;
            }
            while effect.next_tick_at <= elapsed && effect.next_tick_at < effect.expires_at {
                ticked = true;
                effect.next_tick_at += effect.tick_every;
                for segment in &effect.segments {
                    if !segment.periodic {
                        continue;
                    }
                    match segment.property {
                        2 => {
                            periodic_health_delta =
                                periodic_health_delta.saturating_add(segment.value)
                        }
                        6 => {
                            self.anger =
                                (self.anger + segment.value as f32).clamp(0.0, PLAYER_ANGER_MAX)
                        }
                        _ => {}
                    }
                }
            }
        }
        if periodic_health_delta < 0 {
            self.take_damage(periodic_health_delta.saturating_abs());
        } else if periodic_health_delta > 0 {
            self.health = (self.health + periodic_health_delta as f32).clamp(0.0, self.max_health);
        }
        let effect_count = self.item_effects.len();
        self.item_effects
            .retain(|effect| effect.expires_at > elapsed);
        self.worn_item_effects
            .retain(|effect| effect.expires_at > elapsed);
        if self.item_effects.len() != effect_count {
            self.refresh_item_stats();
        }
        ItemEffectUpdate {
            changed: ticked
                || self.hit_status_effects.active.len() != status_before
                || self.health != health_before
                || self.max_health != max_health_before
                || self.anger != anger_before
                || self.item_effects.len() != active_before
                || self.worn_item_effects.len() != worn_before,
            died: self.health <= 0.0,
        }
    }
}

impl CombatStats for Combatant {
    fn hp(&self) -> f32 {
        self.health
    }

    fn alive(&self) -> bool {
        self.health > 0.0
    }

    fn invincible(&self) -> bool {
        self.target_hit_clock.spawn_protected()
    }

    fn anger(&self) -> f32 {
        self.anger
    }

    fn anger_max(&self) -> f32 {
        PLAYER_ANGER_MAX
    }

    fn def(&self) -> f32 {
        self.defense_bonus
    }

    fn attack_bonus(&self) -> f32 {
        self.attack_bonus + self.hit_status_effects.attack_bonus() as f32
    }

    fn take_damage(&mut self, amount: i32) -> i32 {
        let applied = amount.max(0).min(self.health.ceil() as i32);
        self.health = (self.health - applied as f32).max(0.0);
        self.target_hit_clock.alive = self.health > 0.0;
        applied
    }

    fn gain_anger_from_damage(&mut self, damage: i32) -> i32 {
        let gain = ((14_i64 * i64::from(damage.max(0))) / 100) as i32;
        self.anger = (self.anger + gain as f32).min(self.anger_max());
        gain
    }

    fn gain_anger_on_guard(&mut self, raw_atk: i32) -> i32 {
        let gain = guard_anger_gain(raw_atk);
        let before = self.anger;
        self.anger = (self.anger + gain as f32).min(self.anger_max());
        (self.anger - before).max(0.0) as i32
    }

    fn reset_spawn_clock(&mut self) {
        self.target_hit_clock.spawn_clock = 0.0;
    }
}

#[derive(Debug, Clone)]
struct LateJoinBarrier {
    attempt_id: String,
    joiner: String,
    waiting: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct Room {
    pending_joiners: HashSet<String>,
    late_join_barrier: Option<LateJoinBarrier>,
    next_join_attempt: u64,
    pub id: String,
    pub name: String,
    pub mode: String,
    pub stage: String,
    pub configured: bool,
    pub started: bool,

    match_epoch: u32,
    round_epoch: u32,
    pub max_players: usize,
    pub password: String,
    pub rules: RoomRulesWire,

    input_authorities: HashMap<String, InputAuthority>,

    role_states: HashMap<String, RoleState>,

    requested_role_rates: HashMap<String, u32>,

    authority_tick: u32,
    snapshot_accumulator: f32,
    reconciliation_snapshot_accumulator: f32,

    role_rate: u32,
    role_rate_sample_accumulator: f32,
    role_rate_feedback_accumulator: i32,

    elapsed: f32,

    round: Option<RoundState>,

    team_wins: [u32; 2],

    lobby_return_at: Option<f32>,

    pending_round_end: Option<PendingRoundEnd>,

    escort_flag: Option<EscortFlag>,

    scene_state: Vec<SceneOp>,
    pending_transfers: HashMap<String, scene_transfer::PendingTransfer>,

    claimed_scene_items: HashMap<String, ClaimedSceneItem>,

    dynamic_weapon_drops: HashMap<String, DynamicWeaponDrop>,

    next_dynamic_drop_id: u64,

    combatants: HashMap<String, Combatant>,
    hazards: meteor_scene::hazards::StageHazards,

    pub members: HashMap<String, RoomMemberWire>,

    pub order: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WireFormat {
    #[default]
    Json,
    V4,
}

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub player_id: Option<String>,
    pub room_id: Option<String>,
    pub name: String,
    pub client_id: String,
    pub session_token: String,

    pub wire_format: WireFormat,
}

#[derive(Debug, Default)]
pub struct RoomServer {
    rooms: HashMap<String, Room>,
    scene_transfers: HashMap<String, scene_transfer::SceneTransfers>,

    sessions: HashMap<u64, Session>,
    next_player_id: u64,
    next_room_id: u64,

    combat_catalog: Option<AttackCatalog>,
    origin_res: Option<std::path::PathBuf>,
}

#[derive(Debug, Default)]
pub struct Dispatch {
    pub to_sender: Vec<Message>,

    pub to_others: Vec<Message>,

    pub to_room: Vec<Message>,

    pub to_conn: Vec<(u64, Message)>,

    pub close_conn: Vec<(u64, u16, String)>,
}

#[derive(Debug, Default)]
pub struct AuthorityDispatch {
    pub controls: Vec<(u64, Message)>,

    pub events: Vec<(u64, Message)>,

    pub role_snapshots: Vec<(u64, V4RoleSnapshot)>,

    pub snapshots: Vec<(u64, V4Snapshot)>,
}

impl RoomServer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_combat_catalog(combat_catalog: AttackCatalog) -> Self {
        Self {
            combat_catalog: Some(combat_catalog),
            ..Self::default()
        }
    }

    pub fn from_mpk() -> Result<Self, AttackCatalogError> {
        let mut server = Self::with_combat_catalog(AttackCatalog::from_mpk()?);
        server.origin_res = Some(std::path::PathBuf::new());
        Ok(server)
    }

    pub fn from_origin_res(root: &Path) -> Result<Self, AttackCatalogError> {
        let mut server = Self::with_combat_catalog(AttackCatalog::from_origin_res(root)?);
        server.origin_res = Some(root.to_owned());
        Ok(server)
    }

    pub fn has_combat_catalog(&self) -> bool {
        self.combat_catalog.is_some()
    }

    pub fn connect(&mut self, conn_id: u64) {
        self.sessions.insert(conn_id, Session::default());
    }

    pub fn set_wire_format(&mut self, conn_id: u64, wire_format: WireFormat) {
        if let Some(session) = self.sessions.get_mut(&conn_id) {
            session.wire_format = wire_format;
        }
    }

    pub fn wire_format(&self, conn_id: u64) -> WireFormat {
        self.sessions
            .get(&conn_id)
            .map(|session| session.wire_format)
            .unwrap_or(WireFormat::Json)
    }

    pub fn disconnect(&mut self, conn_id: u64) -> Vec<Message> {
        let Some(session) = self.sessions.remove(&conn_id) else {
            return Vec::new();
        };
        let (Some(room_id), Some(player_id)) = (session.room_id, session.player_id) else {
            return Vec::new();
        };
        let (removed, state) = Self::remove_room_member(&mut self.rooms, &room_id, &player_id);
        let mut events = Vec::new();
        if removed {
            events.push(Message::RoomPlayerLeft { player_id });
            events.extend(state);
        }
        events
    }

    fn remove_room_member(
        rooms: &mut HashMap<String, Room>,
        room_id: &str,
        player_id: &str,
    ) -> (bool, Option<Message>) {
        let mut removed = false;
        let mut room_empty = false;
        if let Some(room) = rooms.get_mut(room_id) {
            removed = room.members.remove(player_id).is_some();
            if removed {
                room.order.retain(|id| id != player_id);
                room.input_authorities.remove(player_id);
                room.role_states.remove(player_id);
                room.pending_transfers.remove(player_id);
                room.requested_role_rates.remove(player_id);
                room.combatants.remove(player_id);
                room.pending_joiners.remove(player_id);

                room.late_join_barrier = None;
                room_empty = room.order.is_empty();
            }
        }
        if room_empty {
            rooms.remove(room_id);
            (removed, None)
        } else {
            let state = removed
                .then(|| Self::room_state_from(rooms, room_id))
                .flatten();
            (removed, state)
        }
    }

    fn notify_previous_room(
        dispatch: &mut Dispatch,
        targets: &[u64],
        player_id: &str,
        state: Option<Message>,
    ) {
        for target in targets {
            dispatch.to_conn.push((
                *target,
                Message::RoomPlayerLeft {
                    player_id: player_id.to_string(),
                },
            ));
            if let Some(state) = &state {
                dispatch.to_conn.push((*target, state.clone()));
            }
        }
    }

    pub fn connection_for_player(&self, player_id: &str) -> Option<u64> {
        self.sessions
            .iter()
            .find(|(_, session)| session.player_id.as_deref() == Some(player_id))
            .map(|(conn_id, _)| *conn_id)
    }

    pub fn room_connections(&self, conn_id: u64) -> Vec<u64> {
        let Some(session) = self.sessions.get(&conn_id) else {
            return Vec::new();
        };
        let Some(room_id) = session.room_id.clone() else {
            return Vec::new();
        };
        let exclude = session.player_id.clone();
        self.sessions
            .iter()
            .filter(|(id, other)| {
                **id != conn_id
                    && other.room_id.as_deref() == Some(room_id.as_str())
                    && other.player_id != exclude
            })
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn room_all_connections(&self, conn_id: u64) -> Vec<u64> {
        let Some(session) = self.sessions.get(&conn_id) else {
            return Vec::new();
        };
        let Some(room_id) = session.room_id.clone() else {
            return Vec::new();
        };
        self.sessions
            .iter()
            .filter(|(_, other)| other.room_id.as_deref() == Some(room_id.as_str()))
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn room_summaries(&self) -> Vec<RoomSummary> {
        let mut rooms = self
            .rooms
            .values()
            .map(|room| RoomSummary {
                room_id: room.id.clone(),
                name: room.name.clone(),
                mode: room.configured.then(|| room.mode.clone()),
                stage_id: room.configured.then(|| room.stage.clone()),
                configured: room.configured,
                state: room_state_label(room).into(),
                has_password: !room.password.is_empty(),
                player_count: room.members.len(),
                max_players: room.max_players,
                team_counts: Some([1, 2].map(|team| {
                    room.members
                        .values()
                        .filter(|member| member.team == Some(team))
                        .count()
                })),
                owner_name: room
                    .order
                    .first()
                    .and_then(|id| room.members.get(id))
                    .map(|member| member.name.clone()),
            })
            .collect::<Vec<_>>();
        rooms.sort_by(|left, right| {
            let left_lobby = left.state == "lobby";
            let right_lobby = right.state == "lobby";
            right_lobby
                .cmp(&left_lobby)
                .then_with(|| left.state.cmp(&right.state))
                .then_with(|| left.room_id.cmp(&right.room_id))
        });
        rooms
    }

    fn handle_hello(
        &mut self,
        conn_id: u64,
        name: String,
        resume_token: Option<&str>,
        _character_id: u8,
        _weapon: u8,
    ) -> Dispatch {
        let mut dispatch = Dispatch::default();
        let wire_format = self.wire_format(conn_id);
        let resume_token = resume_token.filter(|token| !token.is_empty() && token.len() <= 128);
        let active_conn = resume_token.and_then(|token| {
            self.sessions.iter().find_map(|(other_conn, session)| {
                (*other_conn != conn_id && session.session_token == token).then_some(*other_conn)
            })
        });
        let mut resumed = false;
        let restored = if let Some(other_conn) = active_conn {
            let session = self.sessions.remove(&other_conn);
            if session.is_some() {
                dispatch
                    .close_conn
                    .push((other_conn, 4000, "session resumed elsewhere".into()));
            }
            resumed = session.is_some();
            session
        } else {
            None
        };

        let mut session = if let Some(session) = restored {
            session
        } else {
            self.next_player_id += 1;
            let identity = format!("p{}", self.next_player_id);
            Session {
                player_id: Some(identity.clone()),
                room_id: None,
                name: name.clone(),
                client_id: format!("c{}", self.next_player_id),
                session_token: new_session_token(),
                wire_format,
            }
        };
        session.name = name;
        session.wire_format = wire_format;
        let player_id = session.player_id.clone().unwrap_or_default();
        let room_id = session.room_id.clone().unwrap_or_default();
        let client_id = session.client_id.clone();
        let session_token = session.session_token.clone();
        self.sessions.insert(conn_id, session);
        dispatch.to_sender.push(Message::Welcome {
            player_id,
            room_id,
            members: Vec::new(),
            client_id,
            session_token,
            resumed,
        });
        if resumed {
            self.push_resume_bootstrap(conn_id, &mut dispatch);
        }
        dispatch
    }

    fn push_resume_bootstrap(&mut self, conn_id: u64, dispatch: &mut Dispatch) {
        let Some(session) = self.sessions.get(&conn_id).cloned() else {
            return;
        };
        let (Some(room_id), Some(player_id)) = (session.room_id, session.player_id) else {
            return;
        };
        let Some((
            last_processed_input_seq,
            players,
            scene_state,
            match_epoch,
            round_epoch,
            mode,
            stage,
            started,
            slots,
        )) = self.rooms.get_mut(&room_id).map(|room| {
            room.reconciliation_snapshot_accumulator = 1.0;
            (
                room.input_authorities
                    .get(&player_id)
                    .map(|authority| authority.last_input_sequence)
                    .unwrap_or_default(),
                room.order
                    .iter()
                    .filter_map(|id| room.members.get(id).cloned())
                    .collect::<Vec<_>>(),
                room.scene_state.clone(),
                (room.match_epoch > 0).then_some(room.match_epoch),
                (room.round_epoch > 0).then_some(room.round_epoch),
                room.mode.clone(),
                room.stage.clone(),
                room.started,
                room.order.clone(),
            )
        })
        else {
            if let Some(session) = self.sessions.get_mut(&conn_id) {
                session.room_id = None;
                session.player_id = None;
            }
            return;
        };
        let pending_join = self.rooms[&room_id].pending_joiners.contains(&player_id);
        let room_state = self.web_room_state(&room_id).unwrap_or(JsonValue::Null);
        let state = Self::room_state_from(&self.rooms, &room_id);
        dispatch.to_sender.push(Message::RoomJoined {
            room_id: room_id.clone(),
            player_id,
            match_epoch,
            round_epoch,
            mode: Some(mode.clone()),
            resumed: true,
            last_processed_input_seq,
            room: room_state,
            scene_state,
        });
        if let Some(state) = state {
            dispatch.to_sender.push(state.clone());
            dispatch.to_others.push(state);
        }
        if started && !pending_join {
            dispatch.to_sender.push(Message::MatchStart {
                room_id: room_id.clone(),
                match_epoch,
                round_epoch,
                mode,
                stage,
                players,
                slots,
                resumed: true,
                late_joined: false,
                authority: "server".into(),
                host_id: String::new(),
            });
        }
    }

    fn begin_late_join(&mut self, room_id: &str, player_id: &str, dispatch: &mut Dispatch) {
        let connected: HashSet<_> = self
            .sessions
            .values()
            .filter_map(|s| s.player_id.clone())
            .collect();
        let Some(room) = self.rooms.get_mut(room_id) else {
            return;
        };
        if !room.started
            || room.lobby_return_at.is_some()
            || !room.pending_joiners.contains(player_id)
            || room.late_join_barrier.is_some()
            || (is_team_mode(&room.mode)
                && !room
                    .members
                    .get(player_id)
                    .is_some_and(|m| matches!(m.team, Some(1 | 2))))
        {
            return;
        }
        room.next_join_attempt += 1;
        let attempt_id = format!("late-{}-{}", room.match_epoch, room.next_join_attempt);
        let players: Vec<_> = room
            .order
            .iter()
            .filter_map(|id| room.members.get(id))
            .filter(|m| {
                connected.contains(&m.player_id)
                    && (!room.pending_joiners.contains(&m.player_id) || m.player_id == player_id)
            })
            .cloned()
            .collect();
        room.late_join_barrier = Some(LateJoinBarrier {
            attempt_id: attempt_id.clone(),
            joiner: player_id.into(),
            waiting: players.iter().map(|m| m.player_id.clone()).collect(),
        });
        let message = Message::MatchAssetPrepare {
            room_id: room_id.into(),
            match_epoch: room.match_epoch,
            round_epoch: room.round_epoch,
            attempt_id,
            purpose: "late-join".into(),
            joiner_id: Some(player_id.into()),
            players,
        };
        self.push_room_state(room_id, dispatch);
        dispatch.to_room.push(message);
    }

    fn finish_late_join(&mut self, room_id: &str, dispatch: &mut Dispatch) {
        let Some(room) = self.rooms.get_mut(room_id) else {
            return;
        };
        if !room
            .late_join_barrier
            .as_ref()
            .is_some_and(|barrier| barrier.waiting.is_empty())
        {
            return;
        }
        let joiner = room.late_join_barrier.take().unwrap().joiner;
        if !room.started || room.lobby_return_at.is_some() {
            return;
        }
        let Some(member) = room.members.get_mut(&joiner) else {
            return;
        };
        member.ready = true;
        let combatant = Combatant::new(
            player_max_health(&room.rules),
            member.team.map(|team| team as i32),
            base_weapon_slot(self.combat_catalog.as_ref(), member.weapon),
        );
        room.pending_joiners.remove(&joiner);
        room.combatants.insert(joiner.clone(), combatant);
        room.input_authorities
            .insert(joiner.clone(), InputAuthority::default());
        room.requested_role_rates
            .insert(joiner.clone(), ROLE_UPLINK_HZ);
        room.reconciliation_snapshot_accumulator = 1.0;
        let message = Message::MatchStart {
            room_id: room.id.clone(),
            match_epoch: Some(room.match_epoch),
            round_epoch: Some(room.round_epoch),
            mode: room.mode.clone(),
            stage: room.stage.clone(),
            players: room
                .order
                .iter()
                .filter_map(|id| room.members.get(id).cloned())
                .collect(),
            slots: room.order.clone(),
            resumed: false,
            late_joined: true,
            authority: "server".into(),
            host_id: String::new(),
        };
        self.push_room_state(room_id, dispatch);
        if let Some(conn) = self.connection_for_player(&joiner) {
            dispatch.to_conn.push((conn, message));
        }
    }

    pub fn handle(&mut self, conn_id: u64, message: Message) -> Dispatch {
        let mut dispatch = Dispatch::default();

        if matches!(message, Message::Quit) {
            return dispatch;
        }
        if let Message::Hello {
            name,
            resume_token,
            character_id,
            weapon,
        } = &message
        {
            return self.handle_hello(
                conn_id,
                name.clone(),
                resume_token.as_deref(),
                *character_id,
                *weapon,
            );
        }
        let previous_room_others = self.room_connections(conn_id);

        let pending_join = self.sessions.get(&conn_id).is_some_and(|session| {
            session
                .room_id
                .as_ref()
                .zip(session.player_id.as_ref())
                .is_some_and(|(room, player)| {
                    self.rooms
                        .get(room)
                        .is_some_and(|r| r.pending_joiners.contains(player))
                })
        });
        if pending_join
            && !matches!(
                message,
                Message::RoomJoin { .. }
                    | Message::RoomLeave
                    | Message::RoomReady { .. }
                    | Message::TeamSelect { .. }
                    | Message::MatchAssetReady { .. }
                    | Message::RoomList { .. }
                    | Message::Ping { .. }
                    | Message::Pong { .. }
                    | Message::ChatSend { .. }
                    | Message::RoomCreate { .. }
                    | Message::RoomProfile { .. }
            )
        {
            return dispatch;
        }

        if matches!(message, Message::CombatHit { .. })
            && self.wire_format(conn_id) == WireFormat::V4
        {
            dispatch.to_sender.push(Message::Error {
                code: "V4_COMBAT_HIT_FORBIDDEN".into(),
                text: "v4 clients must submit Input.combat, not combat.hit".into(),
            });
            return dispatch;
        }

        if let Message::CombatHit { target, damage, .. } = &message {
            let sender_room = self.sessions.get(&conn_id).and_then(|s| s.room_id.clone());
            if let Some(sender_room) = sender_room {
                if let Some(target_conn) = self.connection_for_player(target) {
                    let same_room = self
                        .sessions
                        .get(&target_conn)
                        .map(|s| s.room_id.as_deref() == Some(sender_room.as_str()))
                        .unwrap_or(false);
                    if same_room {
                        dispatch.to_conn.push((
                            target_conn,
                            Message::CombatHit {
                                target: target.clone(),
                                damage: *damage,
                                attacker_id: None,
                                guarded: None,
                                health: None,
                                anger: None,
                                motion_scale: None,
                                reaction: None,
                                tick: None,
                            },
                        ));
                    }
                }
            }
            return dispatch;
        }

        if matches!(message, Message::RoomList { rooms: None }) {
            if self
                .sessions
                .get(&conn_id)
                .and_then(|session| session.player_id.as_ref())
                .is_none()
            {
                dispatch.to_sender.push(Message::Error {
                    code: "no_identity".into(),
                    text: "send hello first".into(),
                });
            } else {
                dispatch.to_sender.push(Message::RoomList {
                    rooms: Some(self.room_summaries()),
                });
            }
            return dispatch;
        }
        let Some(session) = self.sessions.get_mut(&conn_id) else {
            dispatch.to_sender.push(Message::Error {
                code: "no_session".into(),
                text: "connection not registered".into(),
            });
            return dispatch;
        };
        match message {
            Message::RoomCreate {
                mode,
                stage,
                name,
                character_id,
                weapon,
                room_name,
                password,
                rules,
            } => {
                let Some(player_id) = session.player_id.clone() else {
                    dispatch.to_sender.push(Message::Error {
                        code: "no_identity".into(),
                        text: "send hello first".into(),
                    });
                    return dispatch;
                };
                if !name.trim().is_empty() {
                    session.name = name;
                }
                let configured = !mode.is_empty() && !stage.is_empty();
                if configured
                    && (!is_web_mode(&mode)
                        || !is_web_stage(&stage)
                        || (mode == "escort" && !is_escort_stage(&stage))
                        || (mode == "siege" && !is_siege_stage(&stage)))
                {
                    dispatch.to_sender.push(Message::Error {
                        code: "bad_room_settings".into(),
                        text: "mode or stage is not supported".into(),
                    });
                    return dispatch;
                }
                if !configured && (!mode.is_empty() || !stage.is_empty()) {
                    dispatch.to_sender.push(Message::Error {
                        code: "bad_room_settings".into(),
                        text: "mode and stage must be configured together".into(),
                    });
                    return dispatch;
                }
                let mut room_rules = RoomRulesWire::web_defaults();
                room_rules.merge_from(rules);
                let max_players = room_rules
                    .max_players
                    .unwrap_or(DEFAULT_MAX_PLAYERS)
                    .clamp(2, DEFAULT_MAX_PLAYERS);
                room_rules.max_players = Some(max_players);

                room_rules.transport = Some("server".into());

                if let Some(previous_room_id) = session.room_id.take() {
                    let (removed, state) =
                        Self::remove_room_member(&mut self.rooms, &previous_room_id, &player_id);
                    if removed {
                        Self::notify_previous_room(
                            &mut dispatch,
                            &previous_room_others,
                            &player_id,
                            state,
                        );
                    }
                }
                self.next_room_id += 1;
                let room_id = format!("room{}", self.next_room_id);
                let member = RoomMemberWire {
                    connected: true,
                    player_id: player_id.clone(),
                    name: if session.name.is_empty() {
                        player_id.clone()
                    } else {
                        session.name.clone()
                    },
                    character_id,
                    weapon,
                    team: initial_lobby_team(&mode, std::iter::empty()),
                    ready: false,
                };
                let room = Room {
                    pending_joiners: HashSet::new(),
                    late_join_barrier: None,
                    next_join_attempt: 0,
                    id: room_id.clone(),
                    name: if room_name.trim().is_empty() {
                        format!("{} 的房间", session.name)
                    } else {
                        room_name.chars().take(24).collect()
                    },
                    mode,
                    stage,
                    configured,
                    started: false,
                    match_epoch: 0,
                    round_epoch: 0,
                    max_players,
                    password,
                    rules: room_rules,
                    input_authorities: HashMap::new(),
                    role_states: HashMap::new(),
                    requested_role_rates: HashMap::new(),
                    authority_tick: 0,
                    snapshot_accumulator: 0.0,
                    reconciliation_snapshot_accumulator: 0.0,
                    role_rate: ROLE_DOWNLINK_HZ,
                    role_rate_sample_accumulator: 0.0,
                    role_rate_feedback_accumulator: 0,
                    elapsed: 0.0,
                    round: None,
                    team_wins: [0, 0],
                    lobby_return_at: None,
                    pending_round_end: None,
                    escort_flag: None,
                    scene_state: Vec::new(),
                    pending_transfers: HashMap::new(),
                    claimed_scene_items: HashMap::new(),
                    dynamic_weapon_drops: HashMap::new(),
                    next_dynamic_drop_id: 1,
                    combatants: HashMap::new(),
                    hazards: Default::default(),
                    members: HashMap::from([(player_id.clone(), member)]),
                    order: vec![player_id.clone()],
                };
                self.rooms.insert(room_id.clone(), room);
                session.room_id = Some(room_id.clone());
                self.push_room_state(&room_id, &mut dispatch);
            }
            Message::RoomJoin {
                room_id,
                mode,
                stage,
                name,
                character_id,
                weapon,
                password,
            } => {
                let Some(player_id) = session.player_id.clone() else {
                    dispatch.to_sender.push(Message::Error {
                        code: "no_identity".into(),
                        text: "send hello first".into(),
                    });
                    return dispatch;
                };

                let mut room_id = room_id;
                if room_id.is_empty() {
                    match self
                        .rooms
                        .iter()
                        .find(|(_, room)| {
                            room_state_label(room) == "lobby" && room.password.is_empty()
                        })
                        .map(|(id, _)| id.clone())
                    {
                        Some(existing) => room_id = existing,
                        None => {
                            let mode = mode.unwrap_or_else(|| "deathmatch".to_string());
                            let stage = stage.unwrap_or_else(|| "sn01".to_string());
                            if !is_web_mode(&mode)
                                || !is_web_stage(&stage)
                                || (mode == "escort" && !is_escort_stage(&stage))
                                || (mode == "siege" && !is_siege_stage(&stage))
                            {
                                dispatch.to_sender.push(Message::Error {
                                    code: "bad_room_settings".into(),
                                    text: "mode or stage is not supported".into(),
                                });
                                return dispatch;
                            }
                            self.next_room_id += 1;
                            room_id = format!("room{}", self.next_room_id);
                            let room = Room {
                                pending_joiners: HashSet::new(),
                                late_join_barrier: None,
                                next_join_attempt: 0,
                                id: room_id.clone(),
                                name: format!("{} 的房间", session.name),
                                mode,
                                stage,
                                configured: true,
                                started: false,
                                match_epoch: 0,
                                round_epoch: 0,
                                max_players: DEFAULT_MAX_PLAYERS,
                                password: String::new(),
                                rules: RoomRulesWire::web_defaults(),
                                input_authorities: HashMap::new(),
                                role_states: HashMap::new(),
                                requested_role_rates: HashMap::new(),
                                authority_tick: 0,
                                snapshot_accumulator: 0.0,
                                reconciliation_snapshot_accumulator: 0.0,
                                role_rate: ROLE_DOWNLINK_HZ,
                                role_rate_sample_accumulator: 0.0,
                                role_rate_feedback_accumulator: 0,
                                elapsed: 0.0,
                                round: None,
                                team_wins: [0, 0],
                                lobby_return_at: None,
                                pending_round_end: None,
                                escort_flag: None,
                                scene_state: Vec::new(),
                                pending_transfers: HashMap::new(),
                                claimed_scene_items: HashMap::new(),
                                dynamic_weapon_drops: HashMap::new(),
                                next_dynamic_drop_id: 1,
                                combatants: HashMap::new(),
                                hazards: Default::default(),
                                members: HashMap::new(),
                                order: Vec::new(),
                            };
                            self.rooms.insert(room_id.clone(), room);
                        }
                    }
                }

                if session.room_id.as_deref() != Some(room_id.as_str()) {
                    if let Some(previous_room_id) = session.room_id.take() {
                        let (removed, state) = Self::remove_room_member(
                            &mut self.rooms,
                            &previous_room_id,
                            &player_id,
                        );
                        if removed {
                            Self::notify_previous_room(
                                &mut dispatch,
                                &previous_room_others,
                                &player_id,
                                state,
                            );
                        }
                    }
                }
                let Some(room) = self.rooms.get_mut(&room_id) else {
                    dispatch.to_sender.push(Message::Error {
                        code: "no_room".into(),
                        text: format!("room {room_id} not found"),
                    });
                    return dispatch;
                };
                if room_state_label(room) == "ended" || room.late_join_barrier.is_some() {
                    dispatch.to_sender.push(Message::Error {
                        code: "started".into(),
                        text: "room is settling or synchronizing models".into(),
                    });
                    return dispatch;
                }
                if !room.password.is_empty() && room.password != password {
                    dispatch.to_sender.push(Message::Error {
                        code: if password.is_empty() {
                            "ROOM_PASSWORD_REQUIRED"
                        } else {
                            "ROOM_PASSWORD_WRONG"
                        }
                        .into(),
                        text: "room password does not match".into(),
                    });
                    return dispatch;
                }
                if room.members.len() >= room.max_players && !room.members.contains_key(&player_id)
                {
                    dispatch.to_sender.push(Message::Error {
                        code: "full".into(),
                        text: "room is full".into(),
                    });
                    return dispatch;
                }

                if room.members.contains_key(&player_id) {
                    self.push_room_state(&room_id, &mut dispatch);
                    return dispatch;
                }
                let member = RoomMemberWire {
                    connected: true,
                    player_id: player_id.clone(),
                    name: if name.is_empty() {
                        player_id.clone()
                    } else {
                        name
                    },
                    character_id,
                    weapon,
                    team: if room.started {
                        None
                    } else {
                        initial_lobby_team(&room.mode, room.members.values())
                    },
                    ready: !room.started,
                };
                room.members.insert(player_id.clone(), member);
                room.order.push(player_id.clone());
                if room.started {
                    room.pending_joiners.insert(player_id.clone());
                }
                session.room_id = Some(room_id.clone());
                self.push_room_state(&room_id, &mut dispatch);
            }
            Message::RoomLeave => {
                if let Some(room_id) = session.room_id.take() {
                    if let Some(player_id) = session.player_id.clone() {
                        let (removed, state) =
                            Self::remove_room_member(&mut self.rooms, &room_id, &player_id);
                        if removed {
                            Self::notify_previous_room(
                                &mut dispatch,
                                &previous_room_others,
                                &player_id,
                                state,
                            );
                        }

                        dispatch.to_sender.push(Message::RoomLeft);
                    }
                }
            }
            Message::RoomReady { ready } => {
                if let (Some(room_id), Some(player_id)) =
                    (session.room_id.clone(), session.player_id.clone())
                {
                    if let Some(room) = self.rooms.get_mut(&room_id) {
                        let team_required = ready
                            && is_team_mode(&room.mode)
                            && room
                                .members
                                .get(&player_id)
                                .is_some_and(|member| !matches!(member.team, Some(1 | 2)));
                        if team_required {
                            dispatch.to_sender.push(Message::Error {
                                code: "TEAM_REQUIRED".into(),
                                text: "select Meteor or Butterfly before readying".into(),
                            });
                            return dispatch;
                        }
                        if let Some(member) = room.members.get_mut(&player_id) {
                            member.ready = ready && !room.pending_joiners.contains(&player_id);
                        }
                        self.push_room_state(&room_id, &mut dispatch);
                        if ready {
                            self.begin_late_join(&room_id, &player_id, &mut dispatch);
                        }
                    }
                }
            }
            Message::TeamSelect { team } => {
                let (Some(room_id), Some(player_id)) =
                    (session.room_id.clone(), session.player_id.clone())
                else {
                    dispatch.to_sender.push(Message::Error {
                        code: "NOT_IN_ROOM".into(),
                        text: "join a room before selecting a team".into(),
                    });
                    return dispatch;
                };
                let Some(room) = self.rooms.get_mut(&room_id) else {
                    return dispatch;
                };
                if !is_team_mode(&room.mode) {
                    dispatch.to_sender.push(Message::Error {
                        code: "NO_TEAMS".into(),
                        text: "the current mode has no teams".into(),
                    });
                    return dispatch;
                }
                if !matches!(team, 1 | 2) {
                    dispatch.to_sender.push(Message::Error {
                        code: "BAD_TEAM".into(),
                        text: "team must be Meteor (1) or Butterfly (2)".into(),
                    });
                    return dispatch;
                }
                if room.started
                    && (!room.pending_joiners.contains(&player_id)
                        || room.late_join_barrier.is_some())
                {
                    dispatch.to_sender.push(Message::Error {
                        code: "MATCH_IN_PROGRESS".into(),
                        text: "cannot change teams after the match starts".into(),
                    });
                    return dispatch;
                }
                let Some(member) = room.members.get(&player_id) else {
                    dispatch.to_sender.push(Message::Error {
                        code: "NOT_IN_ROOM".into(),
                        text: "player is not a member of this room".into(),
                    });
                    return dispatch;
                };
                let changing_sides = member.team != Some(team);
                let team_capacity = room.max_players.div_ceil(2);
                let team_is_full = room
                    .members
                    .values()
                    .filter(|member| member.team == Some(team))
                    .count()
                    >= team_capacity;
                if changing_sides && team_is_full {
                    dispatch.to_sender.push(Message::Error {
                        code: "TEAM_FULL".into(),
                        text: "selected team is full".into(),
                    });
                    return dispatch;
                }
                if let Some(member) = room.members.get_mut(&player_id) {
                    member.team = Some(team);
                    member.ready = false;
                }
                self.push_room_state(&room_id, &mut dispatch);
                self.begin_late_join(&room_id, &player_id, &mut dispatch);
            }
            Message::MatchAssetReady { attempt_id, ok } => {
                let (Some(room_id), Some(player_id)) =
                    (session.room_id.clone(), session.player_id.clone())
                else {
                    return dispatch;
                };
                let Some(room) = self.rooms.get_mut(&room_id) else {
                    return dispatch;
                };
                let Some(barrier) = room.late_join_barrier.as_mut() else {
                    return dispatch;
                };
                if barrier.attempt_id != attempt_id || !barrier.waiting.contains(&player_id) {
                    return dispatch;
                }
                if !ok {
                    room.late_join_barrier = None;
                    dispatch.to_room.push(Message::Error {
                        code: "ASSET_LOAD_FAILED".into(),
                        text: "model loading failed; retry readying to enter".into(),
                    });
                    self.push_room_state(&room_id, &mut dispatch);
                } else {
                    barrier.waiting.remove(&player_id);
                    self.finish_late_join(&room_id, &mut dispatch);
                }
            }
            Message::RoomProfile { character, weapon } => {
                let (Some(room_id), Some(player_id)) =
                    (session.room_id.clone(), session.player_id.clone())
                else {
                    dispatch.to_sender.push(Message::Error {
                        code: "not_in_room".into(),
                        text: "join a room before updating the loadout".into(),
                    });
                    return dispatch;
                };
                let Some(character_id) = web_character_id(&character) else {
                    dispatch.to_sender.push(Message::Error {
                        code: "bad_character".into(),
                        text: format!("unsupported character {character}"),
                    });
                    return dispatch;
                };
                let Some(weapon_id) = web_weapon_id(&weapon) else {
                    dispatch.to_sender.push(Message::Error {
                        code: "bad_weapon".into(),
                        text: format!("unsupported weapon {weapon}"),
                    });
                    return dispatch;
                };
                if let Some(room) = self.rooms.get_mut(&room_id) {
                    if room.started
                        && (!room.pending_joiners.contains(&player_id)
                            || room.late_join_barrier.is_some()
                            || room.lobby_return_at.is_some())
                    {
                        dispatch.to_sender.push(Message::Error {
                            code: "room_locked".into(),
                            text:
                                "cannot update a loadout while loading or participating in a match"
                                    .into(),
                        });
                        return dispatch;
                    }
                    if let Some(member) = room.members.get_mut(&player_id) {
                        member.character_id = character_id;
                        member.weapon = weapon_id;
                        member.ready = false;
                    }
                    self.push_room_state(&room_id, &mut dispatch);
                }
            }
            Message::RoomSettings {
                mode,
                stage_id,
                rules,
            } => {
                let (Some(room_id), Some(player_id)) =
                    (session.room_id.clone(), session.player_id.clone())
                else {
                    dispatch.to_sender.push(Message::Error {
                        code: "not_in_room".into(),
                        text: "join a room before changing settings".into(),
                    });
                    return dispatch;
                };
                let Some(room) = self.rooms.get_mut(&room_id) else {
                    return dispatch;
                };
                if room.order.first().map(String::as_str) != Some(player_id.as_str()) {
                    dispatch.to_sender.push(Message::Error {
                        code: "host_only".into(),
                        text: "only the host can change room settings".into(),
                    });
                    return dispatch;
                }
                if room.started {
                    dispatch.to_sender.push(Message::Error {
                        code: "room_locked".into(),
                        text: "cannot change settings after the match starts".into(),
                    });
                    return dispatch;
                }
                if !is_web_mode(&mode)
                    || !is_web_stage(&stage_id)
                    || (mode == "escort" && !is_escort_stage(&stage_id))
                    || (mode == "siege" && !is_siege_stage(&stage_id))
                {
                    dispatch.to_sender.push(Message::Error {
                        code: "bad_room_settings".into(),
                        text: "mode or stage is not supported".into(),
                    });
                    return dispatch;
                }
                let max_players = rules
                    .max_players
                    .unwrap_or(room.max_players)
                    .clamp(2, DEFAULT_MAX_PLAYERS);
                if max_players < room.members.len() {
                    dispatch.to_sender.push(Message::Error {
                        code: "room_too_large".into(),
                        text: "maxPlayers cannot be lower than the current player count".into(),
                    });
                    return dispatch;
                }
                room.mode = mode;
                room.stage = stage_id;
                let was_configured = room.configured;
                room.configured = true;
                room.max_players = max_players;
                room.rules.merge_from(rules);
                room.rules.max_players = Some(max_players);
                room.rules.transport = Some("server".into());
                if !is_team_mode(&room.mode) {
                    room.rules.friendly_fire = Some(false);
                }
                reset_lobby_teams_for_mode(room);
                if was_configured {
                    for member in room.members.values_mut() {
                        member.ready = false;
                    }
                }
                self.push_room_state(&room_id, &mut dispatch);
            }
            Message::MatchStart { .. } | Message::RoomStart => {
                let (Some(room_id), Some(player_id)) =
                    (session.room_id.clone(), session.player_id.clone())
                else {
                    return dispatch;
                };
                let Some(room) = self.rooms.get(&room_id) else {
                    return dispatch;
                };
                let host = room.order.first().map(|id| id.as_str());
                if host != Some(player_id.as_str()) {
                    dispatch.to_sender.push(Message::Error {
                        code: "not_host".into(),
                        text: "only the host can start".into(),
                    });
                    return dispatch;
                }
                if room.started {
                    dispatch.to_sender.push(Message::Error {
                        code: "MATCH_IN_PROGRESS".into(),
                        text: "match already started".into(),
                    });
                    return dispatch;
                }
                if !room.configured {
                    dispatch.to_sender.push(Message::Error {
                        code: "ROOM_NOT_CONFIGURED".into(),
                        text: "configure mode and stage before starting".into(),
                    });
                    return dispatch;
                }
                if room.mode == "escort" && !is_escort_stage(&room.stage) {
                    dispatch.to_sender.push(Message::Error {
                        code: "ESCORT_STAGE_UNSUPPORTED".into(),
                        text: "escort requires the authored sn03 flag and safe zones".into(),
                    });
                    return dispatch;
                }
                if room.mode == "siege" && !is_siege_stage(&room.stage) {
                    dispatch.to_sender.push(Message::Error {
                        code: "SIEGE_STAGE_UNSUPPORTED".into(),
                        text: "siege requires an authored GoalBlue/GoalRed stage".into(),
                    });
                    return dispatch;
                }
                if is_team_mode(&room.mode)
                    && room
                        .members
                        .values()
                        .any(|member| !matches!(member.team, Some(1 | 2)))
                {
                    dispatch.to_sender.push(Message::Error {
                        code: "TEAM_REQUIRED".into(),
                        text: "every player must select Meteor or Butterfly".into(),
                    });
                    return dispatch;
                }
                if room.order.iter().skip(1).any(|player_id| {
                    room.members
                        .get(player_id)
                        .is_some_and(|member| !member.ready)
                }) {
                    dispatch.to_sender.push(Message::Error {
                        code: "PLAYERS_NOT_READY".into(),
                        text: "every non-host player must be ready".into(),
                    });
                    return dispatch;
                }
                if room.stage == "sn22" {
                    if let Err(error) = scene_transfer::validate(self.origin_res.as_deref(), room) {
                        dispatch.to_sender.push(Message::Error {
                            code: "ASSET_LOAD_FAILED".into(),
                            text: error,
                        });
                        return dispatch;
                    }
                }
                let mut players = room
                    .order
                    .iter()
                    .filter_map(|id| room.members.get(id).cloned())
                    .collect::<Vec<_>>();
                players.sort_by_key(|m| room.order.iter().position(|id| id == &m.player_id));
                let slots = room.order.clone();
                let catalog = self.combat_catalog.as_ref();
                if let Some(room) = self.rooms.get_mut(&room_id) {
                    room.hazards =
                        environment::load_hazards(self.origin_res.as_deref(), &room.stage);
                    room.started = true;
                    room.match_epoch = room.match_epoch.wrapping_add(1).max(1);
                    room.round_epoch = 0;

                    room.input_authorities = room
                        .members
                        .keys()
                        .cloned()
                        .map(|player_id| (player_id, InputAuthority::default()))
                        .collect();
                    room.role_states.clear();
                    room.requested_role_rates = room
                        .members
                        .keys()
                        .cloned()
                        .map(|player_id| (player_id, ROLE_UPLINK_HZ))
                        .collect();
                    room.authority_tick = 0;
                    room.snapshot_accumulator = 0.0;

                    room.reconciliation_snapshot_accumulator = 1.0;
                    room.role_rate = ROLE_DOWNLINK_HZ;
                    room.role_rate_sample_accumulator = 0.0;
                    room.role_rate_feedback_accumulator = 0;
                    room.elapsed = 0.0;
                    room.scene_state.clear();
                    room.claimed_scene_items.clear();
                    room.dynamic_weapon_drops.clear();
                    room.next_dynamic_drop_id = 1;
                    room.team_wins = [0, 0];
                    room.lobby_return_at = None;
                    let max_health = player_max_health(&room.rules);
                    let teams = combat_teams_from_members(room);
                    room.combatants = room
                        .members
                        .keys()
                        .cloned()
                        .map(|player_id| {
                            let team = teams.get(&player_id).copied().flatten();
                            let weapon_id = room
                                .members
                                .get(&player_id)
                                .map(|member| member.weapon)
                                .unwrap_or(6);
                            (
                                player_id,
                                Combatant::new(
                                    max_health,
                                    team,
                                    base_weapon_slot(catalog, weapon_id),
                                ),
                            )
                        })
                        .collect();

                    if mode_rule_id(&room.mode).is_some() {
                        dispatch.to_room.push(Self::start_round(room, 1));
                    }
                }
                let Some(room) = self.rooms.get(&room_id) else {
                    return dispatch;
                };
                let message = Message::MatchStart {
                    room_id: room.id.clone(),
                    match_epoch: Some(room.match_epoch),
                    round_epoch: Some(room.round_epoch),
                    mode: room.mode.clone(),
                    stage: room.stage.clone(),
                    players: players.clone(),
                    slots: slots.clone(),
                    resumed: false,
                    late_joined: false,
                    authority: "server".into(),
                    host_id: String::new(),
                };
                dispatch.to_room.push(message);
                self.push_room_state(&room_id, &mut dispatch);
            }
            Message::Snapshot { fighter, .. } => {
                let (Some(room_id), Some(player_id)) =
                    (session.room_id.clone(), session.player_id.clone())
                else {
                    return dispatch;
                };
                if self.rooms.get(&room_id).map(|r| r.started).unwrap_or(false) {
                    dispatch.to_others.push(Message::Snapshot {
                        player_id: Some(player_id),
                        fighter,
                    });
                }
            }
            Message::MatchEnd {
                winner_team,
                winner_player_id,
                result,
                reason,
                ..
            } => {
                let (Some(room_id), Some(player_id)) =
                    (session.room_id.clone(), session.player_id.clone())
                else {
                    return dispatch;
                };

                let accepted = self.rooms.get(&room_id).is_some_and(|room| {
                    room.started
                        && room.order.first() == Some(&player_id)
                        && matches!(winner_team, Some(1 | 2))
                });
                if !accepted {
                    dispatch.to_sender.push(Message::Error {
                        code: "MATCH_END_REJECTED".into(),
                        text: "only the active room authority may end a match".into(),
                    });
                    return dispatch;
                }
                if let Some(room) = self.rooms.get_mut(&room_id) {
                    dispatch.to_room.push(Self::end_match(
                        room,
                        result.or_else(|| Some("victory".into())),
                        winner_team,
                        winner_player_id,
                        &reason,
                    ));
                }
                self.push_room_state(&room_id, &mut dispatch);
            }
            Message::ObjectiveEnd { winner_team } => {
                let (Some(room_id), Some(player_id)) =
                    (session.room_id.clone(), session.player_id.clone())
                else {
                    return dispatch;
                };

                let accepted = self.rooms.get(&room_id).is_some_and(|room| {
                    room.started
                        && room.order.first() == Some(&player_id)
                        && room.mode == "siege"
                        && is_siege_stage(&room.stage)
                        && matches!(winner_team, 1 | 2)
                });
                if !accepted {
                    dispatch.to_sender.push(Message::Error {
                        code: "OBJECTIVE_END_REJECTED".into(),
                        text: "only the siege scene authority may end this objective".into(),
                    });
                    return dispatch;
                }
                if let Some(room) = self.rooms.get_mut(&room_id) {
                    dispatch.to_room.extend(Self::end_round(
                        room,
                        Some(winner_team),
                        "siege-objective",
                        None,
                        None,
                    ));
                }
                self.push_room_state(&room_id, &mut dispatch);
            }
            Message::RoleSelfEffect {
                seq,
                room_id,
                match_epoch,
                round_epoch,
                pose_id,
                pose_instance,
            } => {
                let (Some(session_room), Some(player_id)) =
                    (session.room_id.as_ref(), session.player_id.as_ref())
                else {
                    return dispatch;
                };
                if &room_id != session_room {
                    return dispatch;
                }
                let Some(effect) = self
                    .combat_catalog
                    .as_ref()
                    .and_then(|catalog| catalog.pose_self_status_effect(pose_id))
                    .filter(|effect| {
                        effect.attack_bonus > 0 || (effect.health < 0 && effect.duration == 0.0)
                    })
                else {
                    return dispatch;
                };
                let Some(room) = self.rooms.get_mut(session_room) else {
                    return dispatch;
                };
                if !room.started
                    || room.match_epoch != match_epoch
                    || room.round_epoch != round_epoch
                    || !room
                        .round
                        .as_ref()
                        .is_some_and(|round| round.phase == RoundPhase::Active)
                {
                    return dispatch;
                }
                let Some(caster) = room.combatants.get_mut(player_id) else {
                    return dispatch;
                };
                if !caster.alive()
                    || caster
                        .last_self_effect
                        .is_some_and(|(last_seq, last_instance)| {
                            seq <= last_seq || pose_instance == last_instance
                        })
                {
                    return dispatch;
                }
                let authority = room.input_authorities.entry(player_id.clone()).or_default();
                if authority
                    .accept(InputStamp {
                        sequence: seq,
                        carries_role_state: false,
                        carries_role_rate_request: false,
                    })
                    .is_err()
                {
                    return dispatch;
                }
                caster.last_self_effect = Some((seq, pose_instance));

                let delta = caster
                    .hit_status_effects
                    .apply(effect, f64::from(room.elapsed));
                caster.pending_self_health =
                    caster.pending_self_health.saturating_add(delta.health);
                dispatch.to_room.push(Message::ItemEffect {
                    player_id: player_id.clone(),
                    health: caster.health,
                    max_health: caster.max_health,
                    anger: caster.anger,
                    item_effects: caster.item_effects_wire(room.elapsed),
                });
                dispatch.to_room.extend(Self::update_item_effects(room));
            }
            Message::RolePoseChain {
                seq,
                room_id,
                match_epoch,
                round_epoch,
                chain,
                ..
            } => {
                let (Some(session_room), Some(player_id)) =
                    (session.room_id.as_ref(), session.player_id.as_ref())
                else {
                    return dispatch;
                };
                if &room_id != session_room || !chain.is_valid() || chain.minimum_seconds != 0.0 {
                    return dispatch;
                }
                let Some(room) = self.rooms.get_mut(session_room) else {
                    return dispatch;
                };
                if !room.started
                    || room.match_epoch != match_epoch
                    || room.round_epoch != round_epoch
                    || !room
                        .round
                        .as_ref()
                        .is_some_and(|round| round.phase == RoundPhase::Active)
                {
                    return dispatch;
                }
                if room
                    .input_authorities
                    .get(player_id)
                    .is_some_and(|authority| authority.last_role_state_sequence > seq)
                    && room.role_states.get(player_id).is_some_and(|state| {
                        crate::role_state::pose_sequence_is_newer(
                            state.state_words.pose_sequence,
                            chain.sequence,
                        )
                    })
                {
                    return dispatch;
                }
                let Some(combatant) = room.combatants.get_mut(player_id) else {
                    return dispatch;
                };
                if !combatant.alive() || seq <= combatant.last_pose_chain_sequence {
                    return dispatch;
                }
                let authority = room.input_authorities.entry(player_id.clone()).or_default();
                if authority
                    .accept(InputStamp {
                        sequence: seq,
                        carries_role_state: false,
                        carries_role_rate_request: false,
                    })
                    .is_err()
                {
                    return dispatch;
                }
                combatant.last_pose_chain_sequence = seq;

                room.authority_tick = room.authority_tick.wrapping_add(1);
                dispatch.to_others.push(Message::RolePoseChain {
                    seq,
                    room_id,
                    player_id: player_id.clone(),
                    match_epoch,
                    round_epoch,
                    tick: room.authority_tick,
                    chain,
                });
            }
            Message::SceneTransferAck {
                room_id: transfer_room,
                match_epoch,
                round_epoch,
                serial,
            } => {
                if let (Some(room_id), Some(player_id)) = (&session.room_id, &session.player_id) {
                    if let Some(room) = self.rooms.get_mut(room_id) {
                        if room.id == transfer_room
                            && room.match_epoch == match_epoch
                            && room.round_epoch == round_epoch
                            && room
                                .pending_transfers
                                .get(player_id)
                                .is_some_and(|p| p.serial == serial)
                        {
                            room.pending_transfers.remove(player_id);
                        }
                    }
                }
            }
            Message::SceneTransfer { .. } => {
                dispatch.to_sender.push(Message::Error {
                    code: "FORBIDDEN".into(),
                    text: "scene transfers are server-owned".into(),
                });
            }
            Message::WeaponDropRest {
                match_epoch,
                round_epoch,
                reference,
                position,
            } => {
                let (Some(room_id), Some(player_id)) = (&session.room_id, &session.player_id)
                else {
                    return dispatch;
                };
                let Some(room) = self.rooms.get_mut(room_id) else {
                    return dispatch;
                };

                if room.order.first() != Some(player_id)
                    || match_epoch != Some(room.match_epoch)
                    || round_epoch != Some(room.round_epoch)
                    || !position.iter().all(|v| v.is_finite())
                {
                    return dispatch;
                }
                let Some(drop) = room.dynamic_weapon_drops.get_mut(&reference) else {
                    return dispatch;
                };
                if drop.claimable_at.is_finite() || room.elapsed > drop.expires_at {
                    return dispatch;
                }
                drop.rest_position = RoleVec3 {
                    x: position[0],
                    y: position[1],
                    z: position[2],
                };
                drop.claimable_at = room.elapsed;
                dispatch.to_room.push(Message::WeaponDropRest {
                    match_epoch,
                    round_epoch,
                    reference,
                    position,
                });
            }
            Message::ScenePropHit {
                room_id,
                match_epoch,
                round_epoch,
                reference,
                pose_id,
                frame,
                ..
            } => {
                let (Some(session_room), Some(player_id)) =
                    (session.room_id.clone(), session.player_id.clone())
                else {
                    return dispatch;
                };
                if room_id != session_room || reference.is_empty() || reference.len() > 128 {
                    return dispatch;
                }
                let Some(room) = self.rooms.get(&room_id) else {
                    return dispatch;
                };
                if !room.started
                    || room.match_epoch != match_epoch
                    || room.round_epoch != round_epoch
                    || !room
                        .round
                        .as_ref()
                        .is_some_and(|round| round.phase == RoundPhase::Active)
                    || room.order.first() == Some(&player_id)
                {
                    return dispatch;
                }
                let Some(member) = room.members.get(&player_id) else {
                    return dispatch;
                };
                let Some(actor) = room
                    .combatants
                    .get(&player_id)
                    .filter(|actor| actor.alive())
                else {
                    return dispatch;
                };
                let Some(catalog) = self.combat_catalog.as_ref() else {
                    return dispatch;
                };
                if catalog
                    .active_attack_index(member.character_id, pose_id, frame)
                    .is_none()
                {
                    return dispatch;
                }
                let weapon = actor.active_weapon();
                let damage = meteor_combat::hit_resolve::sum_attack_power(
                    weapon.map_or(member.weapon, |weapon| weapon.weapon_id),
                    pose_id,
                    u32::from(pose_id),
                    weapon.map(|weapon| weapon.attack_power as i32),
                    CombatStats::attack_bonus(actor) as i32,
                    &|pose| catalog.pose_attack_percent(pose),
                );
                let Some(host_id) = room.order.first() else {
                    return dispatch;
                };
                let host_conn = self.sessions.iter().find_map(|(id, session)| {
                    (session.room_id.as_ref() == Some(&room_id)
                        && session.player_id.as_ref() == Some(host_id))
                    .then_some(*id)
                });
                if let Some(host_conn) = host_conn {
                    dispatch.to_conn.push((
                        host_conn,
                        Message::ScenePropHit {
                            room_id,
                            match_epoch,
                            round_epoch,
                            player_id: player_id.clone(),
                            reference,
                            pose_id,
                            frame,
                            damage,
                        },
                    ));
                }
            }
            Message::SceneOps { ops, .. } => {
                let (Some(room_id), Some(player_id)) =
                    (session.room_id.clone(), session.player_id.clone())
                else {
                    return dispatch;
                };
                let accepted = self.rooms.get(&room_id).is_some_and(|room| {
                    room.order.first() == Some(&player_id) && valid_scene_ops(&ops)
                });
                if !accepted {
                    dispatch.to_sender.push(Message::Error {
                        code: "SCENE_OPS_REJECTED".into(),
                        text: "only the scene authority may submit a valid NetEvent transaction"
                            .into(),
                    });
                    return dispatch;
                }
                if let Some(room) = self.rooms.get_mut(&room_id) {
                    record_scene_state(room, &ops);
                }

                dispatch.to_others.push(Message::SceneOps {
                    room_id,
                    authority_generated: false,
                    ops,
                });
            }
            Message::ItemClaim {
                reference,
                player_id: collector_id,
                model,
                position,
                renewable,
            } => {
                let (Some(room_id), Some(scene_host_id)) =
                    (session.room_id.clone(), session.player_id.clone())
                else {
                    return dispatch;
                };
                if !valid_item_claim_fields(&reference, &collector_id, &model, position) {
                    dispatch.to_sender.push(Message::Error {
                        code: "ITEM_CLAIM_REJECTED".into(),
                        text: "item claim has invalid fields".into(),
                    });
                    return dispatch;
                }
                let Some(catalog) = self.combat_catalog.as_ref() else {
                    dispatch.to_sender.push(Message::Error {
                        code: "ITEM_ASSETS_UNAVAILABLE".into(),
                        text: "original Meteor.res item registry is unavailable".into(),
                    });
                    return dispatch;
                };
                let Some(room) = self.rooms.get_mut(&room_id) else {
                    return dispatch;
                };

                let dynamic_drop = room.dynamic_weapon_drops.get(&reference).cloned();
                let dynamic_drop_claimable = dynamic_drop
                    .as_ref()
                    .is_none_or(|drop| room.elapsed >= drop.claimable_at);
                let effective_model = dynamic_drop
                    .as_ref()
                    .map(|drop| drop.model.clone())
                    .unwrap_or_else(|| model.clone());
                let effective_position = dynamic_drop.as_ref().map_or(position, |drop| {
                    Some([
                        drop.rest_position.x,
                        drop.rest_position.y,
                        drop.rest_position.z,
                    ])
                });
                let effective_renewable = dynamic_drop.is_none() && renewable;
                let Some(metadata) = catalog.scene_item_metadata(&effective_model) else {
                    dispatch.to_sender.push(Message::Error {
                        code: "ITEM_CLAIM_REJECTED".into(),
                        text: "item model is not an authored category-1/2 resource".into(),
                    });
                    return dispatch;
                };
                let accepted_host = room.started
                    && room.order.first() == Some(&scene_host_id)
                    && dynamic_drop_claimable
                    && (dynamic_drop.is_some()
                        || room_allows_item(&room.rules, metadata.category, metadata.id))
                    && !room.claimed_scene_items.contains_key(&reference);
                let Some(collector) = room.combatants.get_mut(&collector_id) else {
                    dispatch.to_sender.push(Message::Error {
                        code: "ITEM_CLAIM_REJECTED".into(),
                        text: "collector is not an active room combatant".into(),
                    });
                    return dispatch;
                };
                let near_collector = effective_position.is_none_or(|position| {
                    let foot = collector.position;
                    let dx = foot.x - position[0];
                    let dy = foot.y - position[1];
                    let dz = foot.z - position[2];
                    dx.mul_add(dx, dy.mul_add(dy, dz * dz))
                        <= ITEM_CLAIM_MAX_DISTANCE * ITEM_CLAIM_MAX_DISTANCE
                });
                if !accepted_host || !collector.alive() || !near_collector {
                    dispatch.to_sender.push(Message::Error {
                        code: "ITEM_CLAIM_REJECTED".into(),
                        text: "item claim is not authorized, available, alive, or in range".into(),
                    });
                    return dispatch;
                }

                let normalized_model = effective_model.trim().to_ascii_lowercase();
                let (kind, picked_weapon_model) = match metadata.category {
                    1 => {
                        let Some(slot) = weapon_slot_for_model(catalog, &normalized_model) else {
                            dispatch.to_sender.push(Message::Error {
                                code: "ITEM_CLAIM_REJECTED".into(),
                                text: "weapon model lacks original combat data".into(),
                            });
                            return dispatch;
                        };
                        if collector
                            .weapon_slots
                            .iter()
                            .flatten()
                            .any(|existing| existing.resource_class == slot.resource_class)
                        {
                            dispatch.to_sender.push(Message::Error {
                                code: "ITEM_CLAIM_REJECTED".into(),
                                text: "player already carries this weapon class".into(),
                            });
                            return dispatch;
                        }
                        let inactive = usize::from(collector.weapon_slot == 0);
                        let slot_index = if collector.weapon_slots[inactive].is_none() {
                            inactive
                        } else if let Some(index) =
                            collector.weapon_slots.iter().position(Option::is_none)
                        {
                            index
                        } else {
                            dispatch.to_sender.push(Message::Error {
                                code: "ITEM_CLAIM_REJECTED".into(),
                                text: "player weapon slots are full".into(),
                            });
                            return dispatch;
                        };
                        let picked = slot.model.clone();
                        collector.weapon_slots[slot_index] = Some(slot);
                        ("weapon".to_string(), Some(picked))
                    }
                    2 => {
                        let Some(definition) = item_definition(&normalized_model) else {
                            dispatch.to_sender.push(Message::Error {
                                code: "ITEM_CLAIM_REJECTED".into(),
                                text: "item model has no verified gameplay definition".into(),
                            });
                            return dispatch;
                        };
                        if definition.id != metadata.id {
                            dispatch.to_sender.push(Message::Error {
                                code: "ITEM_CLAIM_REJECTED".into(),
                                text: "item definition does not match Meteor.res record".into(),
                            });
                            return dispatch;
                        }
                        let picked_weapon_model = definition.refine_variant.and_then(|variant| {
                            let current = collector.active_weapon()?.clone();
                            let model = format!("w{}_{}", current.resource_class, variant);
                            let refined = weapon_slot_for_model(catalog, &model)?;
                            if refined.model != current.model {
                                collector.weapon_slots[collector.weapon_slot] =
                                    Some(refined.clone());
                            }
                            Some(refined.model)
                        });
                        collector.apply_item(&normalized_model, definition, room.elapsed);
                        (
                            if definition.refine_variant.is_some() {
                                "refine".to_string()
                            } else {
                                "item".to_string()
                            },
                            picked_weapon_model,
                        )
                    }
                    _ => unreachable!("scene_item_metadata only returns category 1/2"),
                };
                let respawn_seconds = effective_renewable
                    .then_some(metadata.respawn_seconds)
                    .flatten()
                    .filter(|seconds| *seconds > 0)
                    .map(|seconds| seconds as f32);
                if dynamic_drop.is_some() {
                    room.dynamic_weapon_drops.remove(&reference);
                } else {
                    room.claimed_scene_items.insert(
                        reference.clone(),
                        ClaimedSceneItem {
                            model: normalized_model.clone(),
                            respawn_at: respawn_seconds.map(|seconds| room.elapsed + seconds),
                        },
                    );
                }
                let ops = vec![
                    SceneOp {
                        kind: "set".into(),
                        reference: JsonValue::String(reference.clone()),
                        field: "attribute".into(),
                        args: vec![JsonValue::from("active"), JsonValue::from(0)],
                    },
                    SceneOp {
                        kind: "set".into(),
                        reference: JsonValue::String(reference.clone()),
                        field: "attribute".into(),
                        args: vec![JsonValue::from("interactive"), JsonValue::from(0)],
                    },
                ];
                let active_weapon = collector.active_weapon().cloned();
                let claimed = Message::ItemClaimed {
                    reference: reference.clone(),
                    player_id: collector_id,
                    model: normalized_model,
                    kind,
                    picked_weapon_model,
                    weapon: active_weapon
                        .as_ref()
                        .map(|weapon| weapon.weapon.clone())
                        .unwrap_or_default(),
                    weapon_model: active_weapon.map(|weapon| weapon.model),
                    weapon_slot: collector.weapon_slot as u8,
                    weapon_slots: collector.weapon_slots_wire(),
                    health: collector.health,
                    max_health: collector.max_health,
                    anger: collector.anger,
                    respawn_seconds,
                    item_effects: collector.item_effects_wire(room.elapsed),
                };
                if dynamic_drop.is_none() {
                    record_scene_state(room, &ops);

                    dispatch.to_room.push(Message::SceneOps {
                        room_id,
                        authority_generated: true,
                        ops,
                    });
                }
                dispatch.to_room.push(claimed);
            }
            Message::ChatSend { text } => {
                let Some(player_id) = session.player_id.clone() else {
                    return dispatch;
                };
                let Some(room_id) = session.room_id.clone() else {
                    return dispatch;
                };
                let name = self
                    .rooms
                    .get(&room_id)
                    .and_then(|room| room.members.get(&player_id))
                    .map(|member| member.name.clone())
                    .unwrap_or_else(|| player_id.clone());
                dispatch.to_others.push(Message::ChatMessage {
                    player_id: Some(player_id),
                    name,
                    text,
                });
            }
            Message::RoomLatency { mut latency, .. } => {
                if let (Some(room_id), Some(player_id)) =
                    (session.room_id.clone(), session.player_id.clone())
                {
                    latency.server_rtt = latency
                        .server_rtt
                        .filter(|ms| ms.is_finite() && *ms >= 0.0 && *ms <= 60_000.0);
                    latency.host_rtt = None;
                    latency.p2p_state = "server".into();
                    dispatch.to_room.push(Message::RoomLatency {
                        room_id,
                        player_id,
                        latency,
                    });
                }
            }
            Message::Ping { client_time } => {
                dispatch.to_sender.push(Message::Pong {
                    client_time,
                    server_time: None,
                });
            }
            _ => {}
        }
        dispatch
    }

    pub fn advance_authority(&mut self, clock_dt: f32, server_time: f64) -> AuthorityDispatch {
        let dt = clamp_frame_dt(clock_dt);
        let mut due_role_rooms = Vec::new();
        let mut due_reconciliation_rooms = Vec::new();
        let mut changed_rates = Vec::new();
        let mut lifecycle_events = Vec::new();
        let mut room_state_updates = Vec::new();
        self.scene_transfers
            .retain(|id, _| self.rooms.get(id).is_some_and(|r| r.started));
        for room in self.rooms.values_mut() {
            if room.lobby_return_at.is_some() {
                room.elapsed += dt;
                if room
                    .lobby_return_at
                    .is_some_and(|return_at| room.elapsed > return_at)
                {
                    lifecycle_events.push((room.id.clone(), Self::reset_to_lobby(room)));
                    room_state_updates.push(room.id.clone());
                }
                continue;
            }
            if !room.started {
                continue;
            }
            room.authority_tick = room.authority_tick.wrapping_add(1);
            room.elapsed += dt;
            room.dynamic_weapon_drops
                .retain(|_, drop| drop.claimable_at.is_finite() || room.elapsed <= drop.expires_at);
            for combatant in room.combatants.values_mut() {
                combatant.advance(dt, room.elapsed);
            }
            lifecycle_events.extend(
                environment::update_hazards(room, dt)
                    .into_iter()
                    .map(|message| (room.id.clone(), message)),
            );
            lifecycle_events.extend(
                Self::update_item_effects(room)
                    .into_iter()
                    .map(|message| (room.id.clone(), message)),
            );
            lifecycle_events.extend(
                scene_transfer::update(&mut self.scene_transfers, self.origin_res.as_deref(), room)
                    .into_iter()
                    .map(|message| (room.id.clone(), message)),
            );
            lifecycle_events.extend(
                Self::update_combat_respawns(room)
                    .into_iter()
                    .map(|message| (room.id.clone(), message)),
            );
            lifecycle_events.extend(
                Self::update_claimed_scene_items(room)
                    .into_iter()
                    .map(|message| (room.id.clone(), message)),
            );
            lifecycle_events.extend(
                Self::update_escort(room)
                    .into_iter()
                    .map(|message| (room.id.clone(), message)),
            );
            let round_events = Self::update_round(room);
            let ended = round_events
                .iter()
                .any(|message| matches!(message, Message::MatchEnd { .. }));
            lifecycle_events.extend(
                round_events
                    .into_iter()
                    .map(|message| (room.id.clone(), message)),
            );
            if ended {
                room_state_updates.push(room.id.clone());
                continue;
            }
            if let Some(rate) = Self::update_role_rate(room, dt) {
                changed_rates.push((room.id.clone(), rate));
            }
            room.snapshot_accumulator += dt;
            let snapshot_interval = 1.0 / room.role_rate as f32;
            if room.snapshot_accumulator >= snapshot_interval {
                room.snapshot_accumulator %= snapshot_interval;
                due_role_rooms.push((room.id.clone(), room.authority_tick));
            }
            room.reconciliation_snapshot_accumulator += dt;
            if room.reconciliation_snapshot_accumulator >= 1.0 {
                room.reconciliation_snapshot_accumulator %= 1.0;
                due_reconciliation_rooms.push((room.id.clone(), room.authority_tick));
            }
        }
        let mut dispatch = AuthorityDispatch::default();
        for (room_id, rate) in changed_rates {
            for conn_id in self.v4_room_connections(&room_id) {
                dispatch
                    .controls
                    .push((conn_id, Message::RoleRate { rate }));
            }
        }
        for room_id in room_state_updates {
            if let Some(state) = self.room_state_message(&room_id) {
                lifecycle_events.push((room_id, state));
            }
        }
        for (room_id, message) in lifecycle_events {
            for conn_id in self.room_connections_by_id(&room_id) {
                dispatch.events.push((conn_id, message.clone()));
            }
        }
        for (room_id, tick) in due_role_rooms {
            let recipients = self.v4_room_recipients(&room_id);
            let Some(room) = self.rooms.get(&room_id) else {
                continue;
            };
            for (conn_id, recipient_id) in recipients {
                let snapshot = Self::v4_role_snapshot_for(room, &recipient_id, tick, server_time);
                if !snapshot.roles.is_empty() {
                    dispatch.role_snapshots.push((conn_id, snapshot));
                }
            }
        }
        for (room_id, tick) in due_reconciliation_rooms {
            let recipients = self.v4_room_recipients(&room_id);
            let Some(room) = self.rooms.get(&room_id) else {
                continue;
            };
            for (conn_id, recipient_id) in recipients {
                dispatch.snapshots.push((
                    conn_id,
                    Self::v4_snapshot_for(room, &recipient_id, tick, server_time),
                ));
            }
        }
        dispatch
    }

    fn update_role_rate(room: &mut Room, dt: f32) -> Option<u32> {
        room.role_rate_sample_accumulator += dt.max(0.0);
        let mut changed_rate = None;
        while room.role_rate_sample_accumulator >= ROLE_RATE_SAMPLE_SECONDS {
            room.role_rate_sample_accumulator -= ROLE_RATE_SAMPLE_SECONDS;
            let votes = room
                .requested_role_rates
                .values()
                .fold(0_i32, |votes, requested| {
                    votes + i32::from(*requested > room.role_rate)
                        - i32::from(*requested < room.role_rate)
                });
            if votes > 0 {
                room.role_rate_feedback_accumulator += 1;
            } else if votes < 0 {
                room.role_rate_feedback_accumulator -= 1;
            }
            let candidate = if room.role_rate_feedback_accumulator > ROLE_RATE_FEEDBACK_THRESHOLD {
                Some(room.role_rate + 1)
            } else if room.role_rate_feedback_accumulator < -ROLE_RATE_FEEDBACK_THRESHOLD {
                room.role_rate.checked_sub(1)
            } else {
                None
            };
            let Some(candidate) = candidate else {
                continue;
            };
            room.role_rate_feedback_accumulator = 0;
            if !(ROLE_RATE_MIN..=ROLE_RATE_MAX).contains(&candidate) || candidate == room.role_rate
            {
                continue;
            }
            room.role_rate = candidate;
            room.snapshot_accumulator = room.snapshot_accumulator.min(1.0 / candidate as f32);
            changed_rate = Some(candidate);
        }
        changed_rate
    }

    fn update_item_effects(room: &mut Room) -> Vec<Message> {
        let respawn_at =
            can_respawn(mode_rule_id(&room.mode)).then_some(room.elapsed + RESPAWN_DELAY_SECONDS);
        let mut deaths = Vec::new();
        let mut events = Vec::new();
        for (player_id, combatant) in &mut room.combatants {
            let was_alive = combatant.alive()
                || (combatant.respawn_at.is_none()
                    && !combatant.hit_status_effects.active.is_empty());
            let update = was_alive.then(|| combatant.update_item_effects(room.elapsed));
            if update.is_some_and(|update| update.changed) {
                events.push(Message::ItemEffect {
                    player_id: player_id.clone(),
                    health: combatant.health,
                    max_health: combatant.max_health,
                    anger: combatant.anger,
                    item_effects: combatant.item_effects_wire(room.elapsed),
                });
            }
            if update.is_some_and(|update| update.died) {
                combatant.deaths = combatant.deaths.saturating_add(1);
                combatant.respawn_at = respawn_at;
                let vip_winner = (mode_rule_id(&room.mode) == Some(4) && combatant.is_leader)
                    .then_some(match combatant.team {
                        Some(1) => Some(2),
                        Some(2) => Some(1),
                        _ => None,
                    })
                    .flatten();
                let dropped_weapon = combatant.active_weapon().cloned();
                combatant.clear_item_effects();
                deaths.push((player_id.clone(), vip_winner, dropped_weapon));
            }
        }
        for (player_id, vip_winner, dropped_weapon) in deaths {
            if let Some(winner_team) = vip_winner {
                room.pending_round_end.get_or_insert(PendingRoundEnd {
                    winner_team: Some(winner_team),
                    reason: "vip",
                });
            }
            if let Some(dropped_weapon) = dropped_weapon {
                if let Some(dropped) = spawn_dynamic_weapon_drop(
                    room,
                    &player_id,
                    dropped_weapon,
                    DynamicWeaponDropSource::Death,
                ) {
                    events.push(dropped);
                }
            }
            events.push(Message::CombatDeath {
                attacker_id: None,
                target_id: player_id,
                respawn_seconds: respawn_at.map(|_| RESPAWN_DELAY_SECONDS),
                tick: Some(room.authority_tick),
            });
        }
        events
    }

    fn update_claimed_scene_items(room: &mut Room) -> Vec<Message> {
        let due = room
            .claimed_scene_items
            .iter()
            .filter_map(|(reference, claim)| {
                claim
                    .respawn_at
                    .is_some_and(|at| room.elapsed >= at)
                    .then_some(reference.clone())
            })
            .collect::<Vec<_>>();
        let mut events = Vec::with_capacity(due.len().saturating_mul(2));
        for reference in due {
            let Some(claim) = room.claimed_scene_items.remove(&reference) else {
                continue;
            };
            let ops = vec![
                SceneOp {
                    kind: "set".into(),
                    reference: JsonValue::String(reference.clone()),
                    field: "attribute".into(),
                    args: vec![JsonValue::from("active"), JsonValue::from(1)],
                },
                SceneOp {
                    kind: "set".into(),
                    reference: JsonValue::String(reference.clone()),
                    field: "attribute".into(),
                    args: vec![JsonValue::from("interactive"), JsonValue::from(1)],
                },
            ];
            record_scene_state(room, &ops);
            events.push(Message::SceneOps {
                room_id: room.id.clone(),
                authority_generated: true,
                ops,
            });
            events.push(Message::ItemRespawned {
                reference,
                model: claim.model,
            });
        }
        events
    }

    fn update_combat_respawns(room: &mut Room) -> Vec<Message> {
        if !can_respawn(mode_rule_id(&room.mode)) {
            return Vec::new();
        }
        let mut events = Vec::new();
        for (player_id, combatant) in &mut room.combatants {
            if combatant
                .respawn_at
                .is_some_and(|respawn_at| room.elapsed > respawn_at)
            {
                combatant.revive();
                events.push(Message::CombatRespawn {
                    player_id: player_id.clone(),
                    reason: "timer".to_string(),
                    healer_id: None,
                });
            }
        }
        events
    }

    fn update_escort(room: &mut Room) -> Vec<Message> {
        if mode_rule_id(&room.mode) != Some(2)
            || !room
                .round
                .as_ref()
                .is_some_and(|round| round.phase == RoundPhase::Active)
        {
            return Vec::new();
        }
        let Some(stage) = escort_stage(&room.stage) else {
            return Vec::new();
        };
        let Some(flag) = room.escort_flag.as_ref().cloned() else {
            return Vec::new();
        };
        if let Some(carrier_id) = flag.carrier_id.clone() {
            let carrier = room.combatants.get(&carrier_id);
            let Some(carrier) = carrier.filter(|carrier| carrier.alive()) else {
                return Self::drop_escort_flag(room).into_iter().collect();
            };
            let position = carrier.foot_position();
            let team = carrier.team.unwrap_or_default();
            if let Some(flag) = room.escort_flag.as_mut() {
                flag.position = position;
            }
            if position_inside_safe_zone(position, team, stage) {
                room.pending_round_end.get_or_insert(PendingRoundEnd {
                    winner_team: Some(team),
                    reason: "escort",
                });
                return vec![Message::EscortDelivered {
                    player_id: carrier_id,
                    team,
                    safe_zone: if team == 1 { "safeA" } else { "safeB" }.to_string(),
                }];
            }
            return Vec::new();
        }
        if flag
            .dropped_at
            .is_some_and(|dropped_at| room.elapsed - dropped_at >= stage.return_seconds)
        {
            return Self::return_escort_flag(room, "timeout")
                .into_iter()
                .collect();
        }
        let pickup = room.order.iter().find_map(|player_id| {
            room.combatants.get(player_id).and_then(|combatant| {
                (combatant.alive()
                    && character_capsule_intersects_flag(combatant.foot_position(), flag.position))
                .then_some((
                    player_id.clone(),
                    combatant.team.unwrap_or_default(),
                    flag.position,
                ))
            })
        });
        let Some((player_id, team, position)) = pickup else {
            return Vec::new();
        };
        if let Some(flag) = room.escort_flag.as_mut() {
            flag.carrier_id = Some(player_id.clone());
            flag.dropped_at = None;
        }
        vec![Message::EscortTaken {
            player_id,
            team,
            position: [position.x, position.y, position.z],
        }]
    }

    fn drop_escort_flag(room: &mut Room) -> Option<Message> {
        let player_id = room.escort_flag.as_ref()?.carrier_id.clone()?;
        let position = room
            .combatants
            .get(&player_id)
            .map(Combatant::foot_position)
            .or_else(|| room.escort_flag.as_ref().map(|flag| flag.position))?;
        let (return_seconds, flag) = {
            let stage = escort_stage(&room.stage)?;
            (stage.return_seconds, room.escort_flag.as_mut()?)
        };
        flag.carrier_id = None;
        flag.position = position;
        flag.dropped_at = Some(room.elapsed);
        Some(Message::EscortDropped {
            player_id,
            position: [position.x, position.y, position.z],
            return_seconds,
        })
    }

    fn return_escort_flag(room: &mut Room, reason: &str) -> Option<Message> {
        let flag = room.escort_flag.as_mut()?;
        flag.position = flag.home;
        flag.carrier_id = None;
        flag.dropped_at = None;
        Some(Message::EscortReturned {
            reason: reason.to_string(),
            position: [flag.position.x, flag.position.y, flag.position.z],
        })
    }

    fn round_time_minutes(room: &Room) -> i32 {
        room.rules
            .round_time
            .unwrap_or(999)
            .clamp(1, 999)
            .try_into()
            .expect("round-time range fits i32")
    }

    fn game_num(room: &Room) -> u32 {
        room.rules.game_num.unwrap_or(999).clamp(1, 9999)
    }

    fn team_wins_wire(room: &Room) -> Option<std::collections::BTreeMap<String, u32>> {
        is_team_mode(&room.mode).then(|| {
            std::collections::BTreeMap::from([
                ("1".to_string(), room.team_wins[0]),
                ("2".to_string(), room.team_wins[1]),
            ])
        })
    }

    fn start_round(room: &mut Room, number: u32) -> Message {
        room.hazards
            .build_from_stage(room.hazards.volumes.clone(), Vec::new());
        room.round_epoch = room.round_epoch.wrapping_add(1).max(1);
        room.pending_transfers.clear();
        if room.stage == "sn22" {
            room.role_states.clear();
        }
        for combatant in room.combatants.values_mut() {
            combatant.is_leader = false;
            combatant.revive();
        }
        room.pending_round_end = None;
        if mode_rule_id(&room.mode) == Some(4) {
            Self::assign_assassination_targets(room);
        }
        room.escort_flag = (mode_rule_id(&room.mode) == Some(2))
            .then(|| escort_stage(&room.stage))
            .flatten()
            .map(|stage| EscortFlag {
                home: stage.flag_home,
                position: stage.flag_home,
                carrier_id: None,
                dropped_at: None,
            });
        let round_time = Self::round_time_minutes(room);
        let game_num = Self::game_num(room);
        room.round = Some(RoundState {
            number,
            phase: RoundPhase::Active,
            ends_at: room.elapsed + round_time_limit_seconds(Some(round_time)) as f32,
            next_round_at: None,
        });
        Message::RoundStart {
            match_epoch: Some(room.match_epoch),
            round_epoch: Some(room.round_epoch),
            round: number,
            round_time,
            game_num: game_num as i32,
            team_wins: Self::team_wins_wire(room),
        }
    }

    fn assign_assassination_targets(room: &mut Room) {
        let mut rng = rand::thread_rng();
        for team in [1, 2] {
            let candidates = room
                .order
                .iter()
                .filter(|player_id| {
                    room.combatants.get(*player_id).is_some_and(|combatant| {
                        combatant.team == Some(team) && combatant.health > 0.0
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            if let Some(player_id) = candidates.choose(&mut rng) {
                if let Some(combatant) = room.combatants.get_mut(player_id) {
                    combatant.is_leader = true;
                }
            }
        }
    }

    fn end_round(
        room: &mut Room,
        winner_team: Option<i32>,
        reason: &str,
        winner_player_id: Option<String>,
        winner_teams: Option<Vec<i32>>,
    ) -> Vec<Message> {
        let Some(round) = room.round.as_ref() else {
            return Vec::new();
        };
        if round.phase != RoundPhase::Active {
            return Vec::new();
        }
        let round_number = round.number;
        let scoring_teams = if is_team_mode(&room.mode) {
            winner_teams.unwrap_or_else(|| winner_team.into_iter().collect())
        } else {
            Vec::new()
        };
        let mut unique_scoring_teams = Vec::new();
        for team in scoring_teams {
            if matches!(team, 1 | 2) && !unique_scoring_teams.contains(&team) {
                room.team_wins[(team - 1) as usize] += 1;
                unique_scoring_teams.push(team);
            }
        }
        let round_end = Message::RoundEnd {
            match_epoch: Some(room.match_epoch),
            round_epoch: Some(room.round_epoch),
            round: round_number,
            winner_team,
            winner_teams: unique_scoring_teams,
            winner_player_id,
            reason: reason.to_string(),
            team_wins: Self::team_wins_wire(room),
        };
        let game_num = Self::game_num(room);
        if game_num != 9999 && round_number >= game_num {
            let rule = mode_rule_id(&room.mode);
            let match_end = if rule == Some(1) {
                let highest_kills = room
                    .combatants
                    .values()
                    .map(|combatant| combatant.kills)
                    .max()
                    .unwrap_or(0);
                let leaders = room
                    .order
                    .iter()
                    .filter(|player_id| {
                        room.combatants
                            .get(*player_id)
                            .is_some_and(|combatant| combatant.kills == highest_kills)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if leaders.len() == 1 {
                    Self::end_match(
                        room,
                        Some("victory".to_string()),
                        None,
                        leaders.into_iter().next(),
                        reason,
                    )
                } else {
                    Self::end_match(room, Some("draw".to_string()), None, None, reason)
                }
            } else if room.team_wins[0] == room.team_wins[1] {
                Self::end_match(room, Some("draw".to_string()), None, None, reason)
            } else {
                let winner_team = (room.team_wins[0] > room.team_wins[1])
                    .then_some(1)
                    .or(Some(2));
                Self::end_match(room, Some("victory".to_string()), winner_team, None, reason)
            };
            return vec![round_end, match_end];
        }
        if let Some(round) = room.round.as_mut() {
            round.phase = RoundPhase::Break;
            round.next_round_at = Some(room.elapsed + ROUND_SETTLE_SECONDS);
        }

        for combatant in room.combatants.values_mut() {
            combatant.respawn_at = None;
        }
        vec![round_end]
    }

    fn end_match(
        room: &mut Room,
        result: Option<String>,
        winner_team: Option<i32>,
        winner_player_id: Option<String>,
        reason: &str,
    ) -> Message {
        room.started = false;
        room.lobby_return_at = Some(room.elapsed + ROUND_SETTLE_SECONDS);
        Message::MatchEnd {
            winner_team,
            winner_player_id,
            result,
            reason: reason.to_string(),
            lobby_return_seconds: Some(ROUND_SETTLE_SECONDS),
        }
    }

    fn update_round(room: &mut Room) -> Vec<Message> {
        let Some(round) = room.round.as_ref() else {
            return Vec::new();
        };
        match round.phase {
            RoundPhase::Break => {
                let next_round_at = round.next_round_at;
                let number = round.number;
                if next_round_at.is_some_and(|at| room.elapsed > at) {
                    vec![Self::start_round(room, number + 1)]
                } else {
                    Vec::new()
                }
            }
            RoundPhase::Active => {
                let ends_at = round.ends_at;
                let rule = mode_rule_id(&room.mode);
                if let Some(pending) = room.pending_round_end.take() {
                    return Self::end_round(room, pending.winner_team, pending.reason, None, None);
                }
                if rule == Some(5) {
                    let mut roster = [0_u32; 2];
                    let mut alive = [0_u32; 2];
                    for combatant in room.combatants.values() {
                        let Some(team) = combatant.team else {
                            continue;
                        };
                        let index = (team - 1) as usize;
                        roster[index] += 1;
                        if combatant.health > 0.0 {
                            alive[index] += 1;
                        }
                    }
                    if roster[0] > 0 && roster[1] > 0 && (alive[0] == 0 || alive[1] == 0) {
                        return if alive[0] == alive[1] {
                            Self::end_round(room, None, "eliminated", None, Some(vec![1, 2]))
                        } else {
                            Self::end_round(
                                room,
                                Some(if alive[0] > 0 { 1 } else { 2 }),
                                "eliminated",
                                None,
                                None,
                            )
                        };
                    }
                }
                if room.elapsed >= ends_at {
                    return Self::end_round(room, None, "timeout", None, None);
                }
                Vec::new()
            }
        }
    }

    fn reset_to_lobby(room: &mut Room) -> Message {
        room.pending_joiners.clear();
        room.late_join_barrier = None;
        room.started = false;
        room.lobby_return_at = None;
        room.round = None;
        room.team_wins = [0, 0];
        room.pending_round_end = None;
        room.escort_flag = None;
        room.input_authorities.clear();
        room.role_states.clear();
        room.requested_role_rates.clear();
        room.combatants.clear();
        room.claimed_scene_items.clear();
        room.dynamic_weapon_drops.clear();
        room.next_dynamic_drop_id = 1;
        for member in room.members.values_mut() {
            member.ready = false;
        }
        Message::MatchReset
    }

    fn v4_room_connections(&self, room_id: &str) -> Vec<u64> {
        self.v4_room_recipients(room_id)
            .into_iter()
            .map(|(conn_id, _)| conn_id)
            .collect()
    }

    fn room_connections_by_id(&self, room_id: &str) -> Vec<u64> {
        self.sessions
            .iter()
            .filter_map(|(conn_id, session)| {
                (session.room_id.as_deref() == Some(room_id)).then_some(*conn_id)
            })
            .collect()
    }

    fn v4_room_recipients(&self, room_id: &str) -> Vec<(u64, String)> {
        self.sessions
            .iter()
            .filter_map(|(conn_id, session)| {
                (session.room_id.as_deref() == Some(room_id)
                    && session.wire_format == WireFormat::V4
                    && !session.player_id.as_ref().is_some_and(|id| {
                        self.rooms
                            .get(room_id)
                            .is_some_and(|room| room.pending_joiners.contains(id))
                    }))
                .then(|| {
                    session
                        .player_id
                        .as_ref()
                        .map(|player_id| (*conn_id, player_id.clone()))
                })
                .flatten()
            })
            .collect()
    }

    fn v4_role_snapshot_for(
        room: &Room,
        recipient_id: &str,
        tick: u32,
        server_time: f64,
    ) -> V4RoleSnapshot {
        let roles = room
            .order
            .iter()
            .filter(|player_id| player_id.as_str() != recipient_id)
            .filter_map(|player_id| {
                room.role_states
                    .get(player_id)
                    .map(|state| V4RoleStateRecord {
                        id: player_id.clone(),
                        role_state: encode_event2_state(*state).to_vec(),
                    })
            })
            .collect();
        V4RoleSnapshot {
            room_id: room.id.clone(),
            tick,
            server_time,
            roles,
            match_epoch: Some(room.match_epoch),
            round_epoch: Some(room.round_epoch),
        }
    }

    fn v4_snapshot_for(room: &Room, recipient_id: &str, tick: u32, server_time: f64) -> V4Snapshot {
        let max_health = player_max_health(&room.rules);
        let players = room
            .order
            .iter()
            .filter(|id| !room.pending_joiners.contains(*id))
            .filter_map(|player_id| room.members.get(player_id))
            .map(|member| {
                let state = room.role_states.get(&member.player_id);
                let combatant = room.combatants.get(&member.player_id);
                let active_weapon = combatant.and_then(Combatant::active_weapon);
                let is_recipient = member.player_id == recipient_id;
                SnapshotPlayer {
                    id: member.player_id.clone(),
                    name: member.name.clone(),
                    character: format!("p{}", member.character_id),
                    weapon: active_weapon
                        .map(|weapon| weapon.weapon.clone())
                        .unwrap_or_else(|| web_weapon_name(member.weapon).into()),
                    weapon_model: active_weapon.map(|weapon| weapon.model.clone()),
                    weapon_slot: combatant.map_or(0, |combatant| combatant.weapon_slot as u32),
                    team: combatant
                        .and_then(|combatant| combatant.team)
                        .map(|team| team as u32),
                    is_leader: combatant.is_some_and(|combatant| combatant.is_leader),
                    connected: true,
                    ready: member.ready,
                    health: combatant.map_or(max_health, |combatant| combatant.health),
                    max_health: Some(
                        combatant.map_or(max_health, |combatant| combatant.max_health),
                    ),
                    anger: combatant.map_or(0.0, |combatant| combatant.anger),
                    kills: combatant.map_or(0, |combatant| combatant.kills),
                    round_kills: 0,
                    round_wins: 0,
                    deaths: combatant.map_or(0, |combatant| combatant.deaths),
                    guard: combatant.map_or_else(
                        || state.is_some_and(|state| state.state_words.guard_route),
                        |combatant| combatant.guard,
                    ),
                    alive: combatant.is_none_or(Combatant::alive),
                    pose_id: combatant.map_or_else(
                        || state.map_or(0, |state| state.state_words.pose_id as u32),
                        |combatant| combatant.pose_id as u32,
                    ),
                    pose_cursor: combatant.map_or(0.0, |combatant| combatant.pose_cursor),
                    lift: 0.0,

                    role_state: if is_recipient {
                        Vec::new()
                    } else {
                        state
                            .map(|state| encode_event2_state(*state))
                            .map_or_else(Vec::new, |bytes| bytes.to_vec())
                    },
                    position: if is_recipient {
                        state.map(|state| Vec3 {
                            x: state.position.x,
                            y: state.position.y,
                            z: state.position.z,
                        })
                    } else {
                        None
                    },
                    yaw: if is_recipient {
                        state.map(|state| yaw_from_forward(state.forward))
                    } else {
                        None
                    },
                    weapon_slots: combatant
                        .map(|combatant| {
                            combatant
                                .weapon_slots
                                .iter()
                                .enumerate()
                                .map(|(index, slot)| {
                                    slot.as_ref().map_or(
                                        V4WeaponSlot {
                                            index: index as u32,
                                            present: false,
                                            weapon: String::new(),
                                            weapon_id: None,
                                            model: None,
                                            resource_class: 0,
                                            variant: 0,
                                            attack_power: 0.0,
                                            defense_power: 0.0,
                                        },
                                        |weapon| V4WeaponSlot {
                                            index: index as u32,
                                            present: true,
                                            weapon: weapon.weapon.clone(),
                                            weapon_id: Some(weapon.weapon_id as u32),
                                            model: Some(weapon.model.clone()),
                                            resource_class: weapon.resource_class as u32,
                                            variant: weapon.variant as u32,
                                            attack_power: weapon.attack_power,
                                            defense_power: weapon.defense,
                                        },
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    item_effects: combatant
                        .map(|combatant| {
                            combatant
                                .item_effects_wire(room.elapsed)
                                .into_iter()
                                .map(|effect| V4ItemEffect {
                                    model: effect.model,
                                    remaining: effect.remaining,
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    worn_effects: combatant
                        .map(|combatant| {
                            combatant
                                .worn_item_effects
                                .iter()
                                .map(|effect| V4ItemEffect {
                                    model: effect.model.clone(),
                                    remaining: (effect.expires_at - room.elapsed).max(0.0),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            })
            .collect();
        let flag = room.escort_flag.as_ref().map(|flag| {
            let stage = escort_stage(&room.stage)
                .expect("escort flag can only exist on an authored escort stage");
            let safe_zone = |index: usize| {
                let zone = stage.safe_zones[index];
                json!({
                    "min": { "x": zone.min_x, "y": zone.min_y, "z": zone.min_z },
                    "max": { "x": zone.max_x, "y": zone.max_y, "z": zone.max_z },
                    "center": { "x": zone.center_x, "y": zone.center_y, "z": zone.center_z },
                })
            };
            let state = if flag.carrier_id.is_some() {
                "carried"
            } else if flag.dropped_at.is_some() {
                "dropped"
            } else {
                "home"
            };
            let return_in = flag
                .dropped_at
                .map(|dropped_at| (stage.return_seconds - (room.elapsed - dropped_at)).max(0.0));
            let value = json!({
                "home": [flag.home.x, flag.home.y, flag.home.z],
                "position": [flag.position.x, flag.position.y, flag.position.z],
                "carrierId": flag.carrier_id,
                "state": state,
                "returnIn": return_in,
                "safeZones": { "1": safe_zone(0), "2": safe_zone(1) },
            });
            crate::protocol::dynamic_object_from_json(value.as_object().expect("flag JSON object"))
        });
        V4Snapshot {
            room_id: room.id.clone(),
            tick,
            server_time,
            players,
            npcs: Vec::new(),
            drops: room
                .dynamic_weapon_drops
                .iter()
                .map(|(reference, drop)| V4SnapshotDrop {
                    r#ref: reference.clone(),
                    model: drop.model.clone(),
                    position: Some(Vec3 {
                        x: drop.rest_position.x,
                        y: drop.rest_position.y,
                        z: drop.rest_position.z,
                    }),

                    settled: room.elapsed >= drop.claimable_at,
                })
                .collect(),
            flag,
            r#match: None,
            match_epoch: Some(room.match_epoch),
            round_epoch: Some(room.round_epoch),
        }
    }

    pub fn handle_v4_input(&mut self, conn_id: u64, input: V4Input) -> Dispatch {
        let mut dispatch = Dispatch::default();
        if input.seq == 0 {
            dispatch.to_sender.push(Message::Error {
                code: "BAD_INPUT_SEQ".into(),
                text: "input.seq must be a positive integer".into(),
            });
            return dispatch;
        }
        if input
            .role_rate_request
            .is_some_and(|rate| !(7..=21).contains(&rate))
        {
            dispatch.to_sender.push(Message::Error {
                code: "BAD_ROLE_RATE".into(),
                text: "input.roleRateRequest must be between 7 and 21 Hz".into(),
            });
            return dispatch;
        }
        let role_state = if input.role_state.is_empty() {
            None
        } else {
            match decode_request1_state(&input.role_state) {
                Ok(state) => Some(state),
                Err(_) => {
                    dispatch.to_sender.push(Message::Error {
                        code: "BAD_ROLE_STATE".into(),
                        text: "input.roleState must be the original 20-byte request-1 body".into(),
                    });
                    return dispatch;
                }
            }
        };
        let Some(session) = self.sessions.get(&conn_id) else {
            dispatch.to_sender.push(Message::Error {
                code: "no_session".into(),
                text: "connection not registered".into(),
            });
            return dispatch;
        };
        let (Some(room_id), Some(player_id)) = (session.room_id.clone(), session.player_id.clone())
        else {
            dispatch.to_sender.push(Message::Error {
                code: "NOT_IN_ROOM".into(),
                text: "player has not joined a room".into(),
            });
            return dispatch;
        };

        let (rooms, combat_catalog) = (&mut self.rooms, &self.combat_catalog);
        let Some(room) = rooms.get_mut(&room_id) else {
            dispatch.to_sender.push(Message::Error {
                code: "NOT_IN_ROOM".into(),
                text: "player room no longer exists".into(),
            });
            return dispatch;
        };

        if !room.started
            || !room.members.contains_key(&player_id)
            || room.pending_joiners.contains(&player_id)
        {
            return dispatch;
        }

        if input.match_epoch != Some(room.match_epoch)
            || input.round_epoch != Some(room.round_epoch)
        {
            return dispatch;
        }

        if room.pending_transfers.contains_key(&player_id) {
            return dispatch;
        }
        let admission = match room
            .input_authorities
            .entry(player_id.clone())
            .or_default()
            .accept(InputStamp {
                sequence: input.seq,
                carries_role_state: role_state.is_some(),
                carries_role_rate_request: input.role_rate_request.is_some(),
            }) {
            Ok(admission) => admission,
            Err(InputReject::SequenceJump) => {
                dispatch.to_sender.push(Message::Error {
                    code: "BAD_INPUT_SEQ".into(),
                    text: "input.seq jump is too large".into(),
                });
                return dispatch;
            }
        };
        if admission.role_rate_advanced {
            if let Some(rate) = input.role_rate_request {
                room.requested_role_rates.insert(player_id.clone(), rate);
            }
        }
        if admission.role_state_advanced {
            let state =
                role_state.expect("role-state watermark requires a decoded request-1 state");
            room.role_states.insert(player_id.clone(), state);
            if let Some(combatant) = room.combatants.get_mut(&player_id) {
                combatant.position = state.position;
                combatant.yaw = yaw_from_forward(state.forward);
                combatant.guard = state.state_words.guard_route;
                combatant.pose_id = state.state_words.pose_id;
            }
        }
        if admission.input_advanced {
            if let Some(combatant) = room.combatants.get_mut(&player_id) {
                if !admission.role_state_advanced {
                    combatant.guard = input.guard;
                    if let Some(visual) = input.visual.as_ref() {
                        if visual.pose_id <= 2048 {
                            combatant.pose_id = visual.pose_id as u16;
                        }
                    }
                }
                update_combatant_visual(combatant, input.visual.as_ref(), room.elapsed);
            }
        }

        if admission.discrete_allowed {
            for (index, action) in input.actions.iter().take(MAX_COMBAT_JUDGES).enumerate() {
                let accepted = room
                    .input_authorities
                    .get_mut(&player_id)
                    .is_some_and(|authority| authority.accept_discrete(input.seq, index as u16));
                if accepted {
                    dispatch
                        .to_room
                        .extend(accept_combat_action(room, &player_id, action));
                }
            }
            let Some(catalog) = combat_catalog.as_ref() else {
                if !input.combat.is_empty() {
                    dispatch.to_sender.push(Message::Error {
                        code: "SERVER_COMBAT_CATALOG_UNAVAILABLE".into(),
                        text: "server cannot validate combat without original P*.POS assets".into(),
                    });
                }
                return dispatch;
            };
            for (index, judge) in input.combat.iter().take(MAX_COMBAT_JUDGES).enumerate() {
                let accepted =
                    room.input_authorities
                        .get_mut(&player_id)
                        .is_some_and(|authority| {
                            authority
                                .accept_discrete(input.seq, 128_u16.saturating_add(index as u16))
                        });
                if accepted {
                    dispatch.to_room.extend(accept_combat_judge(
                        room,
                        catalog,
                        &player_id,
                        judge,
                        input.visual.as_ref(),
                    ));
                }
            }
        }
        dispatch
    }

    fn web_room_state(&self, room_id: &str) -> Option<JsonValue> {
        let room = self.rooms.get(room_id)?;
        let life_adjust = room.rules.life_adjust.unwrap_or(200);
        let max_health = (1000_u32.saturating_mul(life_adjust) / 100).max(1);
        let players = room
            .order
            .iter()
            .filter_map(|id| room.members.get(id))
            .map(|member| {
                let connected = self.connection_for_player(&member.player_id).is_some();
                let is_leader = room
                    .combatants
                    .get(&member.player_id)
                    .is_some_and(|combatant| combatant.is_leader);
                json!({
                    "id": member.player_id.clone(),
                    "name": member.name.clone(),
                    "character": format!("p{}", member.character_id),
                    "weapon": web_weapon_name(member.weapon),
                    "team": member.team,
                    "isLeader": is_leader,
                    "battlePending": room.pending_joiners.contains(&member.player_id),
                    "assetReady": false,
                    "connected": connected,
                    "ready": member.ready,
                    "health": max_health,
                    "maxHealth": max_health,
                    "kills": 0,
                    "roundKills": 0,
                    "deaths": 0,
                    "network": {
                        "serverRtt": JsonValue::Null,
                        "hostRtt": JsonValue::Null,
                        "p2pState": "waiting",
                    },
                })
            })
            .collect::<Vec<_>>();
        Some(json!({
            "roomId": room.id.clone(),
            "name": room.name.clone(),
            "mode": if room.configured { JsonValue::String(room.mode.clone()) } else { JsonValue::Null },
            "rule": JsonValue::Null,
            "stageId": if room.configured { JsonValue::String(room.stage.clone()) } else { JsonValue::Null },
            "configured": room.configured,
            "state": room_state_label(room),
            "authority": "server",
            "hostPlayerId": room.order.first(),
            "assetLoading": room.late_join_barrier.is_some(),
            "assetPurpose": JsonValue::Null,
            "hasPassword": !room.password.is_empty(),
            "ownerPlayerId": room.order.first(),
            "players": players,
            "maxPlayers": room.max_players,
            "scoreLimit": 3,
            "friendlyFire": room.rules.friendly_fire.unwrap_or(false),
            "rules": room.rules,
            "teamWins": JsonValue::Null,
            "round": JsonValue::Null,
            "result": JsonValue::Null,
            "winnerTeam": JsonValue::Null,
            "winnerPlayerId": JsonValue::Null,
        }))
    }

    fn room_state_from(rooms: &HashMap<String, Room>, room_id: &str) -> Option<Message> {
        let room = rooms.get(room_id)?;
        let mut members = room
            .order
            .iter()
            .filter_map(|id| room.members.get(id).cloned())
            .collect::<Vec<_>>();
        members.sort_by_key(|m| room.order.iter().position(|id| id == &m.player_id));
        Some(Message::RoomState {
            lobby: crate::protocol::RoomLobbyWire {
                state: Some(room_state_label(room).into()),
                asset_loading: Some(room.late_join_barrier.is_some()),
                ..Default::default()
            },
            room_id: room.id.clone(),
            match_epoch: (room.match_epoch > 0).then_some(room.match_epoch),
            round_epoch: (room.round_epoch > 0).then_some(room.round_epoch),
            mode: room.mode.clone(),
            stage: room.stage.clone(),
            members,
            started: room.started,
            rules: room.rules.clone(),
        })
    }

    fn room_state_message(&self, room_id: &str) -> Option<Message> {
        Self::room_state_from(&self.rooms, room_id)
    }

    fn push_room_state(&self, room_id: &str, dispatch: &mut Dispatch) {
        let Some(state) = self.room_state_message(room_id) else {
            return;
        };
        dispatch.to_sender.push(state.clone());

        dispatch.to_others.push(state);
        let Some(room_state) = self.web_room_state(room_id) else {
            return;
        };
        let mode = self
            .rooms
            .get(room_id)
            .map(|room| room.mode.clone())
            .unwrap_or_default();
        let scene_state = self
            .rooms
            .get(room_id)
            .map(|room| room.scene_state.clone())
            .unwrap_or_default();
        let recipients = self
            .sessions
            .iter()
            .filter_map(|(conn_id, session)| {
                (session.room_id.as_deref() == Some(room_id))
                    .then(|| {
                        session
                            .player_id
                            .clone()
                            .map(|player_id| (*conn_id, player_id))
                    })
                    .flatten()
            })
            .collect::<Vec<_>>();
        for (conn_id, player_id) in recipients {
            let last_processed_input_seq = self
                .rooms
                .get(room_id)
                .and_then(|room| room.input_authorities.get(&player_id))
                .map(|authority| authority.last_input_sequence)
                .unwrap_or_default();
            dispatch.to_conn.push((
                conn_id,
                Message::RoomJoined {
                    room_id: room_id.to_string(),
                    player_id,
                    match_epoch: self
                        .rooms
                        .get(room_id)
                        .and_then(|room| (room.match_epoch > 0).then_some(room.match_epoch)),
                    round_epoch: self
                        .rooms
                        .get(room_id)
                        .and_then(|room| (room.round_epoch > 0).then_some(room.round_epoch)),
                    mode: Some(mode.clone()),
                    resumed: false,
                    last_processed_input_seq,
                    room: room_state.clone(),
                    scene_state: scene_state.clone(),
                },
            ));
        }
    }
}

fn valid_scene_ops(ops: &[SceneOp]) -> bool {
    !ops.is_empty()
        && ops.len() <= MAX_SCENE_OPS_PER_TRANSACTION
        && ops.iter().all(|op| {
            matches!(op.kind.as_str(), "set" | "effect")
                && !op.field.is_empty()
                && op.field.len() <= 128
                && op.args.len() <= 32
                && matches!(op.reference, JsonValue::Number(_) | JsonValue::String(_))
        })
}

fn record_scene_state(room: &mut Room, ops: &[SceneOp]) {
    for op in ops.iter().filter(|op| op.kind == "set") {
        let key = scene_state_key(op);
        if let Some(index) = room
            .scene_state
            .iter()
            .position(|existing| scene_state_key(existing) == key)
        {
            room.scene_state.remove(index);
        }
        room.scene_state.push(op.clone());
    }
    while room.scene_state.len() > MAX_SCENE_STATE_OPS {
        room.scene_state.remove(0);
    }
}

fn scene_state_key(op: &SceneOp) -> String {
    let attribute = if op.field == "attribute" {
        op.args
            .first()
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
    } else {
        ""
    };
    let reference = serde_json::to_string(&op.reference).unwrap_or_default();
    format!("{reference}\u{1f}{}\u{1f}{attribute}", op.field)
}

fn update_combatant_visual(
    combatant: &mut Combatant,
    visual: Option<&V4InputVisual>,
    elapsed: f32,
) {
    let Some(visual) = visual else {
        return;
    };
    if visual.cursor.is_finite() {
        combatant.pose_cursor = visual.cursor.clamp(0.0, 100_000.0);
    }
    let Some(position) = visual.world_position.as_ref().and_then(role_vec3_from_wire) else {
        combatant.reset_geometry();
        return;
    };
    let dx = position.x - combatant.position.x;
    let dz = position.z - combatant.position.z;
    let position_error = (dx * dx + dz * dz).sqrt();
    if !position_error.is_finite() || position_error > GEOMETRY_POSITION_MAX_ERROR_XZ {
        combatant.reset_geometry();
        return;
    }
    combatant.world_position = Some(position);
    combatant.world_position_updated_at = elapsed;
    if visual.carries_hurt_obbs {
        combatant.hurt_obbs = visual
            .hurt_obbs
            .iter()
            .take(MAX_HURT_OBBS)
            .filter_map(|packed| decode_packed_obb(&packed.values, MAX_HURT_OBB_HALF_SIZE))
            .collect();
    }
    if visual.carries_hurt_whole_obb {
        combatant.hurt_whole_obb = visual
            .hurt_whole_obb
            .as_ref()
            .and_then(|packed| decode_packed_obb(&packed.values, MAX_HURT_OBB_HALF_SIZE));
    }
    if visual.carries_hurt_obbs || visual.carries_hurt_whole_obb {
        combatant.collision_updated_at = elapsed;
    }
}

fn accept_combat_action(room: &mut Room, player_id: &str, action: &str) -> Vec<Message> {
    if action == "cure" {
        return accept_cure(room, player_id);
    }
    let drop_source = match action {
        "drop_weapon" => Some(DynamicWeaponDropSource::DropWeapon),
        "drop_item" => Some(DynamicWeaponDropSource::DropItem),
        _ => None,
    };
    if drop_source.is_some_and(|source| !can_spawn_dynamic_weapon_drop(room, player_id, source)) {
        return Vec::new();
    }
    let Some(combatant) = room.combatants.get_mut(player_id) else {
        return Vec::new();
    };

    if action == "drop_item" {
        let Some(dropped) = combatant.active_weapon().cloned() else {
            return Vec::new();
        };
        return spawn_dynamic_weapon_drop(
            room,
            player_id,
            dropped,
            DynamicWeaponDropSource::DropItem,
        )
        .into_iter()
        .collect();
    }
    if !combatant.alive() {
        return Vec::new();
    }
    if let Some(spend) = crate::protocol::parse_anger_spend_action(action) {
        let Some(weapon_id) = combatant.active_weapon().map(|weapon| weapon.weapon_id) else {
            return Vec::new();
        };
        let Some(expected_cost) =
            meteor_combat::combo_engine::anger_cost_for_weapon_action(weapon_id, spend.action_id)
        else {
            return Vec::new();
        };
        let key = (spend.pose_instance_id, spend.action_id);
        if expected_cost != spend.cost
            || combatant.accepted_anger_spends.contains(&key)
            || combatant.anger < f32::from(expected_cost)
        {
            return Vec::new();
        }
        combatant.anger -= f32::from(expected_cost);
        combatant.accepted_anger_spends.push_back(key);
        while combatant.accepted_anger_spends.len() > 64 {
            combatant.accepted_anger_spends.pop_front();
        }

        return vec![Message::ItemEffect {
            player_id: player_id.to_string(),
            health: combatant.health,
            max_health: combatant.max_health,
            anger: combatant.anger,
            item_effects: combatant.item_effects_wire(room.elapsed),
        }];
    }
    match action {
        "switch_weapon" => {
            let alternate = usize::from(combatant.weapon_slot == 0);
            if combatant.weapon_slots[alternate].is_none() {
                return Vec::new();
            }
            combatant.weapon_slot = alternate;
            let active_weapon = combatant.active_weapon().cloned();
            return vec![Message::WeaponSwitched {
                player_id: player_id.to_string(),
                weapon: active_weapon
                    .as_ref()
                    .map(|weapon| weapon.weapon.clone())
                    .unwrap_or_default(),
                weapon_model: active_weapon.map(|weapon| weapon.model),
                weapon_slot: combatant.weapon_slot as u8,
                weapon_slots: combatant.weapon_slots_wire(),
            }];
        }

        "drop_weapon" => {
            let alternate = usize::from(combatant.weapon_slot == 0);
            if combatant.active_weapon().is_none() {
                return Vec::new();
            }
            let dropped = combatant.weapon_slots[alternate].take();
            let Some(dropped) = dropped else {
                return Vec::new();
            };
            return spawn_dynamic_weapon_drop(
                room,
                player_id,
                dropped,
                DynamicWeaponDropSource::DropWeapon,
            )
            .into_iter()
            .collect();
        }

        "attack" => {}

        "qi_burst" if combatant.anger >= 60.0 => {
            combatant.anger -= 60.0;
        }

        "dodge" | "burst" => {}
        _ => {}
    }
    Vec::new()
}

fn accept_cure(room: &mut Room, healer_id: &str) -> Vec<Message> {
    if mode_rule_id(&room.mode) != Some(4)
        || !room
            .round
            .as_ref()
            .is_some_and(|round| round.phase == RoundPhase::Active)
    {
        return Vec::new();
    }
    let Some(healer) = room.combatants.get_mut(healer_id) else {
        return Vec::new();
    };
    if !healer.alive() || !healer.is_leader || room.elapsed < healer.next_cure_at {
        return Vec::new();
    }
    healer.next_cure_at = room.elapsed + CURE_RECOVERY_SECONDS;
    let healer_team = healer.team;
    let healer_position = healer.foot_position();
    let cure_range_squared = CURE_RANGE * CURE_RANGE;
    let targets = room
        .combatants
        .iter()
        .filter_map(|(target_id, target)| {
            (target_id != healer_id
                && target.team == healer_team
                && !target.alive()
                && role_distance_squared(healer_position, target.foot_position())
                    <= cure_range_squared)
                .then_some(target_id.clone())
        })
        .collect::<Vec<_>>();
    let mut events = Vec::with_capacity(targets.len().saturating_mul(2));
    for target_id in targets {
        let Some(target) = room.combatants.get_mut(&target_id) else {
            continue;
        };
        target.revive();
        let position = target.foot_position();
        events.push(Message::CombatCure {
            healer_id: healer_id.to_string(),
            target_id: target_id.clone(),
            position: Some([position.x, position.y, position.z]),
        });
        events.push(Message::CombatRespawn {
            player_id: target_id,
            reason: "cure".to_string(),
            healer_id: Some(healer_id.to_string()),
        });
    }
    events
}

fn accept_combat_judge(
    room: &mut Room,
    catalog: &AttackCatalog,
    attacker_id: &str,
    judge: &V4CombatJudge,
    input_visual: Option<&V4InputVisual>,
) -> Vec<Message> {
    if !valid_combat_judge_shape(judge) {
        return Vec::new();
    }
    let Some(attacker_member) = room.members.get(attacker_id).cloned() else {
        return Vec::new();
    };
    let Some(record) = catalog.attack_record(
        attacker_member.character_id,
        judge.pose_id as u16,
        judge.attack_index,
    ) else {
        return Vec::new();
    };
    let Some(active_index) = catalog.active_attack_index(
        attacker_member.character_id,
        judge.pose_id as u16,
        judge.frame,
    ) else {
        return Vec::new();
    };
    if active_index != judge.attack_index
        || judge.frame.floor() < record.start as f32
        || judge.frame.floor() > record.end as f32
        || (judge.frame - (record.pose_start as f32 + judge.cursor)).abs() > 0.75
    {
        return Vec::new();
    }
    let Some(attacker) = room.combatants.get(attacker_id) else {
        return Vec::new();
    };

    if !attacker.alive() {
        return Vec::new();
    }

    let is_precise = judge.kind == "precise";
    let is_effect = judge.kind == "effect";
    let attack_boxes = if is_precise || is_effect {
        let Some(input_visual) = input_visual else {
            return Vec::new();
        };
        let required_source = if is_precise {
            ATTACK_SOURCE_PRECISE
        } else {
            ATTACK_SOURCE_EFFECT
        };
        if record.source_flags & required_source == 0
            || input_visual.pose_id != judge.pose_id
            || (input_visual.cursor - judge.cursor).abs() > 2.0
        {
            return Vec::new();
        }
        let max_xz = if is_precise {
            PRECISE_BOX_REACH_XZ
        } else {
            EFFECT_BOX_REACH_XZ
        };
        let max_y = if is_precise {
            PRECISE_BOX_REACH_Y
        } else {
            EFFECT_BOX_REACH_Y
        };
        let Some(foot) = combat_owner_foot(attacker, Some(input_visual)) else {
            return Vec::new();
        };
        let boxes = judge
            .boxes
            .iter()
            .take(MAX_ATTACK_OBBS)
            .map(|packed| decode_packed_obb(&packed.values, MAX_ATTACK_OBB_HALF_SIZE))
            .collect::<Option<Vec<_>>>();
        let Some(boxes) = boxes else {
            return Vec::new();
        };
        if boxes.is_empty()
            || !boxes.iter().all(|obb| {
                obb_center_near(
                    obb,
                    meteor_core::Vec3::new(foot.x, foot.y, foot.z),
                    max_xz,
                    max_y,
                )
            })
        {
            return Vec::new();
        }
        Some(boxes)
    } else {
        if !accept_ranged_geometry(room, attacker_id, &attacker_member, judge, input_visual) {
            return Vec::new();
        }
        None
    };

    let target_ids = if !judge.target_ids.is_empty() && (is_precise || is_effect) {
        judge
            .target_ids
            .iter()
            .take(MAX_CLAIM_TARGETS)
            .filter(|id| !id.is_empty() && id.len() <= 32)
            .cloned()
            .collect()
    } else {
        room.order.clone()
    };
    let mut events = Vec::new();
    for target_id in target_ids {
        if target_id == attacker_id {
            continue;
        }
        let Some(target) = room.combatants.get(&target_id) else {
            continue;
        };
        if !can_attack_combatant(target) {
            continue;
        }
        let hits_target = match (&attack_boxes, judge.kind.as_str()) {
            (Some(boxes), "precise") if !judge.target_ids.is_empty() => boxes.iter().any(|obb| {
                obb_center_near(
                    obb,
                    meteor_core::Vec3::new(
                        target.foot_position().x,
                        target.foot_position().y,
                        target.foot_position().z,
                    ),
                    PRECISE_BOX_REACH_XZ + CLAIM_STALENESS_SLACK_XZ,
                    PRECISE_BOX_REACH_Y,
                )
            }),
            (Some(boxes), "effect") if !judge.target_ids.is_empty() => boxes.iter().any(|obb| {
                obb_center_near(
                    obb,
                    meteor_core::Vec3::new(
                        target.foot_position().x,
                        target.foot_position().y,
                        target.foot_position().z,
                    ),
                    EFFECT_BOX_REACH_XZ + CLAIM_STALENESS_SLACK_XZ,
                    EFFECT_BOX_REACH_Y,
                )
            }),
            (Some(boxes), "precise") => {
                room.elapsed - target.collision_updated_at <= COLLISION_SNAPSHOT_MAX_AGE
                    && boxes.iter().any(|attack| {
                        target
                            .hurt_obbs
                            .iter()
                            .any(|hurt| obbs_intersect(attack, hurt))
                    })
            }
            (Some(boxes), "effect") => {
                room.elapsed - target.collision_updated_at <= COLLISION_SNAPSHOT_MAX_AGE
                    && target.hurt_whole_obb.as_ref().is_some_and(|whole| {
                        boxes.iter().any(|attack| obbs_intersect(attack, whole))
                    })
            }
            (None, "ray" | "projectile") => ranged_judge_hits_target(judge, target),
            _ => false,
        };
        if !hits_target {
            continue;
        }
        events.extend(resolve_confirmed_hit(
            room,
            catalog,
            attacker_id,
            &attacker_member,
            &target_id,
            judge.pose_id as u16,
            &record,
        ));
    }
    events
}

fn valid_combat_judge_shape(judge: &V4CombatJudge) -> bool {
    matches!(
        judge.kind.as_str(),
        "precise" | "effect" | "ray" | "projectile"
    ) && judge.pose_id <= 2048
        && judge.attack_index <= 31
        && judge.frame.is_finite()
        && (0.0..=100_000.0).contains(&judge.frame)
        && judge.cursor.is_finite()
        && (0.0..=100_000.0).contains(&judge.cursor)
}

fn can_attack_combatant(target: &Combatant) -> bool {
    target.alive()
        && target.target_hit_clock.accepts_target_hit()
        && !target.target_hit_clock.spawn_protected()
}

fn combat_owner_foot(
    attacker: &Combatant,
    input_visual: Option<&V4InputVisual>,
) -> Option<RoleVec3> {
    let foot = input_visual?
        .world_position
        .as_ref()
        .and_then(role_vec3_from_wire)?;
    let dx = foot.x - attacker.position.x;
    let dz = foot.z - attacker.position.z;
    let position_error = (dx * dx + dz * dz).sqrt();
    (position_error.is_finite() && position_error <= GEOMETRY_POSITION_MAX_ERROR_XZ).then_some(foot)
}

fn accept_ranged_geometry(
    room: &mut Room,
    attacker_id: &str,
    attacker_member: &RoomMemberWire,
    judge: &V4CombatJudge,
    input_visual: Option<&V4InputVisual>,
) -> bool {
    let Some(profile) = ranged_attack_profile(judge.pose_id as u16, attacker_member.weapon, None)
    else {
        return false;
    };
    let Some(start) = judge.start.as_ref().and_then(role_vec3_from_wire) else {
        return false;
    };
    let Some(end) = judge.end.as_ref().and_then(role_vec3_from_wire) else {
        return false;
    };
    let segment_length = point_distance(start, end);
    if !segment_length.is_finite() || segment_length <= 1e-5 {
        return false;
    }
    let Some(attacker) = room.combatants.get_mut(attacker_id) else {
        return false;
    };
    let Some(foot) = combat_owner_foot(attacker, input_visual) else {
        return false;
    };
    if judge.kind == "ray" {
        if !profile.ray || judge.ranged_kind != "gun" || segment_length > GUN_RANGE + 0.5 {
            return false;
        }
        if point_distance(start, foot) > GUN_CAMERA_ORIGIN_MAX_DISTANCE {
            return false;
        }
        let direction = RoleVec3 {
            x: (end.x - start.x) / segment_length,
            y: (end.y - start.y) / segment_length,
            z: (end.z - start.z) / segment_length,
        };
        let anchor = RoleVec3 {
            x: foot.x,
            y: foot.y + GUN_SPAWN_HEIGHT,
            z: foot.z,
        };
        return point_ray_distance(anchor, start, direction) <= GUN_CAMERA_OWNER_RAY_TOLERANCE;
    }
    if judge.kind != "projectile" || segment_length > MAX_PROJECTILE_STEP {
        return false;
    }
    let expected_kind = match profile.kind {
        meteor_combat::ranged_weapons::RangedKind::Dart => "dart",
        meteor_combat::ranged_weapons::RangedKind::Wheel => "wheel",
        meteor_combat::ranged_weapons::RangedKind::Gun => "gun",
    };
    if judge.ranged_kind != expected_kind {
        return false;
    }
    let track_key = format!(
        "{}:{}:{}:{}",
        judge.ranged_kind, judge.pose_id, judge.pose_instance_id, judge.attack_index
    );
    if let Some(track) = attacker.projectile_tracks.get_mut(&track_key) {
        if point_distance(start, track.end) > MAX_PROJECTILE_CONTINUITY_ERROR {
            return false;
        }
        track.end = end;
        track.updated_at = room.elapsed;
    } else {
        if point_distance(start, foot) > 6.0 {
            return false;
        }
        attacker.projectile_tracks.insert(
            track_key,
            ProjectileTrack {
                end,
                updated_at: room.elapsed,
            },
        );
        while attacker.projectile_tracks.len() > MAX_LIVE_COMBAT_INSTANCES {
            let Some(key) = attacker.projectile_tracks.keys().next().cloned() else {
                break;
            };
            attacker.projectile_tracks.remove(&key);
        }
    }
    true
}

fn ranged_judge_hits_target(judge: &V4CombatJudge, target: &Combatant) -> bool {
    let (Some(start), Some(end)) = (
        judge.start.as_ref().and_then(role_vec3_from_wire),
        judge.end.as_ref().and_then(role_vec3_from_wire),
    ) else {
        return false;
    };
    let foot = target.foot_position();
    let hit_scale = if judge.ranged_kind == "wheel" {
        2.6
    } else {
        1.0
    };
    let box_ = ranged_target_box(
        RangedPoint3 {
            x: foot.x,
            y: foot.y,
            z: foot.z,
        },
        hit_scale,
    );
    segment_hits_box_distance(
        RangedPoint3 {
            x: start.x,
            y: start.y,
            z: start.z,
        },
        RangedPoint3 {
            x: end.x,
            y: end.y,
            z: end.z,
        },
        box_,
    )
    .is_some()
}

fn resolve_confirmed_hit(
    room: &mut Room,
    catalog: &AttackCatalog,
    attacker_id: &str,
    attacker_member: &RoomMemberWire,
    target_id: &str,
    pose_id: u16,
    record: &AttackRecord,
) -> Vec<Message> {
    let (attacker_weapon_id, attacker_weapon_power, attacker_attack_bonus) = room
        .combatants
        .get(attacker_id)
        .map(|combatant| {
            let weapon = combatant.active_weapon();
            (
                weapon.map_or(attacker_member.weapon, |weapon| weapon.weapon_id),
                weapon.map(|weapon| weapon.attack_power as i32),
                CombatStats::attack_bonus(combatant) as i32,
            )
        })
        .unwrap_or((attacker_member.weapon, None, 0));
    let defender_weapon_def = room
        .combatants
        .get(target_id)
        .and_then(Combatant::active_weapon)
        .map(|weapon| weapon.defense as i32)
        .or_else(|| {
            room.members
                .get(target_id)
                .and_then(|member| catalog.weapon_stats(member.weapon))
                .map(|stats| stats.defense)
        });
    let pose_attack_percent = |id| catalog.pose_attack_percent(id);
    let Some(attacker_team) = room
        .combatants
        .get(attacker_id)
        .map(|combatant| combatant.team)
    else {
        return Vec::new();
    };
    let Some(defender_team) = room
        .combatants
        .get(target_id)
        .map(|combatant| combatant.team)
    else {
        return Vec::new();
    };
    let respawn_at =
        can_respawn(mode_rule_id(&room.mode)).then_some(room.elapsed + RESPAWN_DELAY_SECONDS);

    if !team_combat_allowed(
        &room.mode,
        room.rules.friendly_fire.unwrap_or(false),
        attacker_team,
        defender_team,
    ) {
        return Vec::new();
    }
    let Some(target) = room.combatants.get_mut(target_id) else {
        return Vec::new();
    };
    if !can_attack_combatant(target) {
        return Vec::new();
    }

    target.target_hit_clock.reset_target_hit_clock();
    if target.target_hit_clock.spawn_protected() {
        return Vec::new();
    }
    let result = resolve_hit(
        target,
        &HitOptions {
            attacker_team: attacker_team.unwrap_or(0),
            defender_team: defender_team.unwrap_or(0),
            defender_in_guard: target.guard,
            defender_weapon_def,
            weapon_id: attacker_weapon_id,
            pose_id,
            attack_power: attacker_weapon_power,
            attack_bonus: attacker_attack_bonus,
            hit_key: None,
            force_hit_flags: None,
            attack_type: record.attack.attack_type as i32,
            dword1: None,
            attack: Some(&record.attack),
            pose_attack_percent: &pose_attack_percent,
        },
    );
    let status_effect = (!result.guarded && !result.killed)
        .then(|| catalog.pose_status_effect(pose_id))
        .flatten();
    if let Some(effect) = status_effect {
        let delta = target
            .hit_status_effects
            .apply(effect, f64::from(room.elapsed));
        target.health = (target.health + delta.health as f32).clamp(0.0, target.max_health);
        target.anger = (target.anger + delta.anger as f32).clamp(0.0, PLAYER_ANGER_MAX);
    }
    let effect_projection = status_effect.map(|_| Message::ItemEffect {
        player_id: target_id.to_string(),
        health: target.health,
        max_health: target.max_health,
        anger: target.anger,
        item_effects: target.item_effects_wire(room.elapsed),
    });
    let health = target.health;
    let anger = target.anger;
    let vip_winner = if result.killed && mode_rule_id(&room.mode) == Some(4) && target.is_leader {
        match target.team {
            Some(1) => Some(2),
            Some(2) => Some(1),
            _ => None,
        }
    } else {
        None
    };
    let death_dropped_weapon = if result.killed {
        target.deaths = target.deaths.saturating_add(1);
        target.respawn_at = respawn_at;

        target.clear_item_effects();
        target.active_weapon().cloned()
    } else {
        None
    };
    if result.killed {
        if let Some(attacker) = room.combatants.get_mut(attacker_id) {
            attacker.kills = attacker.kills.saturating_add(1);
        }
    }
    if let Some(winner_team) = vip_winner {
        room.pending_round_end.get_or_insert(PendingRoundEnd {
            winner_team: Some(winner_team),
            reason: "vip",
        });
    }
    let hit = Message::CombatHit {
        target: target_id.to_string(),
        damage: result.damage as f32,
        attacker_id: Some(attacker_id.to_string()),
        guarded: Some(result.guarded),
        health: Some(health),
        anger: Some(anger),
        motion_scale: Some(if result.guarded {
            result.defense_move
        } else {
            result.target_move
        }),
        reaction: Some(CombatReactionWire {
            defense_move: record.attack.defense_move,
            target_move: record.attack.target_move,
            defense_value: record.attack.defense_value,
            target_value: record.attack.target_value,
            target_pose: record.attack.target_pose,
            target_pose_front: record.attack.target_pose_front,
            target_pose_back: record.attack.target_pose_back,
            target_pose_left: record.attack.target_pose_left,
            target_pose_right: record.attack.target_pose_right,
        }),
        tick: Some(room.authority_tick),
    };
    let mut events = vec![hit];
    events.extend(effect_projection);
    if result.killed {
        if let Some(dropped_weapon) = death_dropped_weapon {
            if let Some(dropped) = spawn_dynamic_weapon_drop(
                room,
                target_id,
                dropped_weapon,
                DynamicWeaponDropSource::Death,
            ) {
                events.push(dropped);
            }
        }
        events.push(Message::CombatDeath {
            attacker_id: Some(attacker_id.to_string()),
            target_id: target_id.to_string(),
            respawn_seconds: respawn_at.map(|_| RESPAWN_DELAY_SECONDS),
            tick: Some(room.authority_tick),
        });
        if mode_rule_id(&room.mode) == Some(2)
            && room
                .escort_flag
                .as_ref()
                .is_some_and(|flag| flag.carrier_id.as_deref() == Some(target_id))
        {
            if let Some(dropped) = RoomServer::drop_escort_flag(room) {
                events.push(dropped);
            }
        }
    }
    events
}

fn initial_lobby_team<'a>(
    mode: &str,
    existing: impl IntoIterator<Item = &'a RoomMemberWire>,
) -> Option<u32> {
    if !is_team_mode(mode) {
        return None;
    }
    let (team_one, team_two) =
        existing
            .into_iter()
            .fold((0, 0), |counts, member| match member.team {
                Some(1) => (counts.0 + 1, counts.1),
                Some(2) => (counts.0, counts.1 + 1),
                _ => counts,
            });
    Some(if team_one <= team_two { 1 } else { 2 })
}

fn reset_lobby_teams_for_mode(room: &mut Room) {
    let team_mode = is_team_mode(&room.mode);
    for (index, player_id) in room.order.iter().enumerate() {
        if let Some(member) = room.members.get_mut(player_id) {
            member.team = team_mode.then_some(if index % 2 == 0 { 1 } else { 2 });
        }
    }
}

fn combat_teams_from_members(room: &Room) -> HashMap<String, Option<i32>> {
    room.members
        .iter()
        .map(|(player_id, member)| {
            (
                player_id.clone(),
                is_team_mode(&room.mode)
                    .then_some(member.team.unwrap_or(0) as i32)
                    .filter(|team| matches!(*team, 1 | 2)),
            )
        })
        .collect()
}

fn team_combat_allowed(
    mode: &str,
    friendly_fire: bool,
    attacker_team: Option<i32>,
    defender_team: Option<i32>,
) -> bool {
    !is_team_mode(mode)
        || friendly_fire
        || !matches!(
            (attacker_team, defender_team),
            (Some(attacker), Some(defender)) if attacker == defender
        )
}

fn role_vec3_from_wire(value: &Vec3) -> Option<RoleVec3> {
    [value.x, value.y, value.z]
        .iter()
        .all(|component| component.is_finite() && component.abs() <= 512.0)
        .then_some(RoleVec3 {
            x: value.x,
            y: value.y,
            z: value.z,
        })
}

fn point_distance(left: RoleVec3, right: RoleVec3) -> f32 {
    role_distance_squared(left, right).sqrt()
}

fn role_distance_squared(left: RoleVec3, right: RoleVec3) -> f32 {
    let dx = left.x - right.x;
    let dy = left.y - right.y;
    let dz = left.z - right.z;
    dx * dx + dy * dy + dz * dz
}

fn point_ray_distance(point: RoleVec3, start: RoleVec3, direction: RoleVec3) -> f32 {
    let ox = point.x - start.x;
    let oy = point.y - start.y;
    let oz = point.z - start.z;
    let along = ox * direction.x + oy * direction.y + oz * direction.z;
    if along < 0.0 {
        return f32::INFINITY;
    }
    let dx = ox - direction.x * along;
    let dy = oy - direction.y * along;
    let dz = oz - direction.z * along;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn player_max_health(rules: &RoomRulesWire) -> f32 {
    let life_adjust = rules.life_adjust.unwrap_or(200);
    (PLAYER_BASE_HEALTH * life_adjust as f32 / 100.0)
        .round()
        .max(1.0)
}

fn web_weapon_name(weapon_id: u8) -> &'static str {
    match weapon_id {
        1 => "dart",
        2 => "wheel",
        3 => "gun",
        4 => "stinger",
        5 => "dagger",
        6 => "sword",
        7 => "spear",
        8 => "blade",
        9 => "hammer",
        10 => "staffBlade",
        11 => "fist",
        12 => "ninjaBlade",
        _ => "sword",
    }
}

fn web_character_id(character: &str) -> Option<u8> {
    character
        .strip_prefix('p')
        .or_else(|| character.strip_prefix('P'))
        .and_then(|value| value.parse().ok())
}

fn web_weapon_id(weapon: &str) -> Option<u8> {
    Some(match weapon {
        "dart" => 1,
        "wheel" => 2,
        "gun" => 3,
        "stinger" => 4,
        "dagger" => 5,
        "sword" => 6,
        "spear" => 7,
        "blade" => 8,
        "hammer" => 9,
        "staffBlade" => 10,
        "fist" => 11,
        "ninjaBlade" => 12,
        _ => return None,
    })
}

fn is_web_mode(mode: &str) -> bool {
    matches!(
        mode,
        "leader" | "escort" | "siege" | "assassin" | "deathmatch"
    )
}

fn is_web_stage(stage_id: &str) -> bool {
    stage_id
        .strip_prefix("sn")
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (1..=22).contains(&number))
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::FighterSnapshot;
    use crate::role_state::{encode_request1_state, role_state_from_runtime, RoleStateRuntime};
    use crate::wire_v4::{CombatJudge, Input as V4Input, InputVisual, PackedObb};

    fn running_late_join(mode: &str) -> RoomServer {
        let mut server = RoomServer::new();
        for id in 1..=3 {
            server.connect(id);
            server.handle(id, hello());
        }
        server.handle(1, create_room(mode, "sn01"));
        server.handle(1, Message::RoomStart);
        server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p1")
            .unwrap()
            .health = 321.0;
        server
    }

    fn late_attempt(server: &RoomServer) -> String {
        server.rooms["room1"]
            .late_join_barrier
            .as_ref()
            .unwrap()
            .attempt_id
            .clone()
    }

    #[test]
    fn late_join_profile_updates_prepare_and_spawn_but_locks_after_start() {
        for mode in ["leader", "deathmatch"] {
            let mut server = running_late_join(mode);
            server.handle(2, join_room("room1"));
            let profile = Message::RoomProfile {
                character: "p8".into(),
                weapon: "staffBlade".into(),
            };
            let changed = server.handle(2, profile.clone());
            assert!(changed.to_sender.iter().any(|m| matches!(m, Message::RoomState { members, .. }
                if members.iter().any(|p| p.player_id == "p2" && p.character_id == 8 && p.weapon == 10))));
            assert!(server.rooms["room1"].pending_joiners.contains("p2"));
            assert!(server.rooms["room1"].late_join_barrier.is_none());
            let locked = server.handle(1, profile.clone());
            assert!(locked
                .to_sender
                .iter()
                .any(|m| matches!(m, Message::Error { code, .. } if code == "room_locked")));
            let started = server.handle(
                2,
                if mode == "leader" {
                    Message::RoomReady { ready: true }
                } else {
                    Message::TeamSelect { team: 2 }
                },
            );
            assert!(started.to_room.iter().any(|m| matches!(m, Message::MatchAssetPrepare { players, .. }
                if players.iter().any(|p| p.player_id == "p2" && p.character_id == 8 && p.weapon == 10))));
            let locked = server.handle(
                2,
                Message::RoomProfile {
                    character: "p1".into(),
                    weapon: "sword".into(),
                },
            );
            assert!(locked
                .to_sender
                .iter()
                .any(|m| matches!(m, Message::Error { code, .. } if code == "room_locked")));
            let attempt = late_attempt(&server);
            server.handle(
                2,
                Message::MatchAssetReady {
                    attempt_id: attempt.clone(),
                    ok: true,
                },
            );
            let completed = server.handle(
                1,
                Message::MatchAssetReady {
                    attempt_id: attempt,
                    ok: true,
                },
            );
            assert!(completed.to_conn.iter().any(|(conn, m)| *conn == 2 && matches!(m, Message::MatchStart { players, .. }
                if players.iter().any(|p| p.player_id == "p2" && p.character_id == 8 && p.weapon == 10))));
            let locked = server.handle(2, profile);
            assert!(locked
                .to_sender
                .iter()
                .any(|m| matches!(m, Message::Error { code, .. } if code == "room_locked")));
        }
    }

    #[test]
    fn running_join_waits_for_all_models_and_preserves_current_battle() {
        let mut server = running_late_join("leader");
        let joined = server.handle(2, join_room("room1"));
        assert!(!joined
            .to_room
            .iter()
            .any(|m| matches!(m, Message::MatchAssetPrepare { .. })));
        let joined = server.handle(2, Message::RoomReady { ready: true });
        assert!(joined.to_room.iter().any(|m| matches!(m, Message::MatchAssetPrepare { purpose, joiner_id: Some(id), .. } if purpose == "late-join" && id == "p2")));
        assert!(!server.rooms["room1"].combatants.contains_key("p2"));
        assert!(
            !RoomServer::v4_snapshot_for(&server.rooms["room1"], "p1", 1, 0.0)
                .players
                .iter()
                .any(|p| p.id == "p2")
        );
        server.handle_v4_input(2, v4_input(1, vec![], None));
        assert!(!server.rooms["room1"].input_authorities.contains_key("p2"));
        let attempt = late_attempt(&server);
        server.handle(
            2,
            Message::MatchAssetReady {
                attempt_id: "stale".into(),
                ok: true,
            },
        );
        server.handle(
            2,
            Message::MatchAssetReady {
                attempt_id: attempt.clone(),
                ok: true,
            },
        );
        assert!(!server.rooms["room1"].combatants.contains_key("p2"));
        let completed = server.handle(
            1,
            Message::MatchAssetReady {
                attempt_id: attempt,
                ok: true,
            },
        );
        assert!(completed.to_conn.iter().any(|(conn, m)| *conn == 2
            && matches!(
                m,
                Message::MatchStart {
                    late_joined: true,
                    resumed: false,
                    match_epoch: Some(1),
                    round_epoch: Some(1),
                    ..
                }
            )));
        assert!(!completed
            .to_room
            .iter()
            .any(|m| matches!(m, Message::MatchStart { .. })));
        let room = &server.rooms["room1"];
        assert_eq!(room.combatants["p1"].health, 321.0);
        assert_eq!(room.combatants["p2"].health, player_max_health(&room.rules));
        assert!(room.combatants["p2"].target_hit_clock.spawn_protected());
        assert!(room.pending_joiners.is_empty());
        assert_eq!(room.order, ["p1", "p2"]);
        let duplicate = server.handle(1, Message::RoomStart);
        assert!(duplicate
            .to_sender
            .iter()
            .any(|m| matches!(m, Message::Error { code, .. } if code == "MATCH_IN_PROGRESS")));
        assert_eq!(server.rooms["room1"].combatants["p1"].health, 321.0);
    }

    #[test]
    fn running_team_join_selects_side_and_retries_failed_models() {
        let mut server = running_late_join("deathmatch");
        server.handle(2, join_room("room1"));
        assert_eq!(server.rooms["room1"].members["p2"].team, None);
        assert!(server.rooms["room1"].late_join_barrier.is_none());
        server.handle(2, Message::TeamSelect { team: 2 });
        let first = late_attempt(&server);
        server.handle(
            2,
            Message::MatchAssetReady {
                attempt_id: first.clone(),
                ok: false,
            },
        );
        assert!(server.rooms["room1"].late_join_barrier.is_none());
        assert!(!server.rooms["room1"].members["p2"].ready);
        server.handle(2, Message::RoomReady { ready: true });
        let retry = late_attempt(&server);
        assert_ne!(first, retry);
        server.handle(
            1,
            Message::MatchAssetReady {
                attempt_id: first,
                ok: true,
            },
        );
        assert_eq!(
            server.rooms["room1"]
                .late_join_barrier
                .as_ref()
                .unwrap()
                .waiting
                .len(),
            2
        );
        server.handle(
            1,
            Message::MatchAssetReady {
                attempt_id: retry.clone(),
                ok: true,
            },
        );
        server.handle(
            2,
            Message::MatchAssetReady {
                attempt_id: retry,
                ok: true,
            },
        );
        assert_eq!(server.rooms["room1"].combatants["p2"].team, Some(2));
        let switched = server.handle(2, Message::TeamSelect { team: 1 });
        assert!(switched
            .to_sender
            .iter()
            .any(|m| matches!(m, Message::Error { code, .. } if code == "MATCH_IN_PROGRESS")));
    }

    #[test]
    fn running_join_keeps_capacity_password_and_settlement_guards() {
        let mut server = running_late_join("leader");
        server.rooms.get_mut("room1").unwrap().password = "secret".into();
        let denied = server.handle(2, join_room("room1"));
        assert!(denied
            .to_sender
            .iter()
            .any(|m| matches!(m, Message::Error { code, .. } if code == "ROOM_PASSWORD_REQUIRED")));
        server.rooms.get_mut("room1").unwrap().password.clear();
        server.rooms.get_mut("room1").unwrap().max_players = 1;
        let full = server.handle(2, join_room("room1"));
        assert!(full
            .to_sender
            .iter()
            .any(|m| matches!(m, Message::Error { code, .. } if code == "full")));
        server.rooms.get_mut("room1").unwrap().max_players = 16;
        server.rooms.get_mut("room1").unwrap().lobby_return_at = Some(10.0);
        let ended = server.handle(2, join_room("room1"));
        assert!(ended
            .to_sender
            .iter()
            .any(|m| matches!(m, Message::Error { code, .. } if code == "started")));
    }

    #[test]
    fn running_join_disconnect_cancels_barrier_without_spawning() {
        let mut server = running_late_join("leader");
        server.handle(2, join_room("room1"));
        let token = server.sessions[&2].session_token.clone();
        server.disconnect(2);
        assert!(server.rooms["room1"].late_join_barrier.is_none());
        server.connect(4);
        let resumed = server.handle(
            4,
            Message::Hello {
                name: "back".into(),
                resume_token: Some(token),
                character_id: 0,
                weapon: 6,
            },
        );
        assert!(!resumed
            .to_sender
            .iter()
            .any(|m| matches!(m, Message::MatchStart { .. })));
        assert!(resumed
            .to_sender
            .iter()
            .any(|m| matches!(m, Message::Welcome { resumed: false, .. })));
        server.handle(4, join_room("room1"));
        server.handle(4, Message::RoomReady { ready: true });
        assert!(server.rooms["room1"].late_join_barrier.is_some());
        server.handle(4, Message::RoomLeave);
        assert!(server.rooms["room1"].late_join_barrier.is_none());
        assert!(server.rooms["room1"].pending_joiners.is_empty());
    }

    fn create_room(mode: &str, stage: &str) -> Message {
        Message::RoomCreate {
            mode: mode.into(),
            stage: stage.into(),
            name: String::new(),
            character_id: 0,
            weapon: 6,
            room_name: String::new(),
            password: String::new(),
            rules: Default::default(),
        }
    }

    fn join_room(room_id: &str) -> Message {
        Message::RoomJoin {
            room_id: room_id.into(),
            mode: None,
            stage: None,
            name: "测试对手".into(),
            character_id: 3,
            weapon: 7,
            password: String::new(),
        }
    }

    fn join_empty() -> Message {
        join_room("")
    }

    fn hello() -> Message {
        Message::Hello {
            name: "tester".into(),
            resume_token: None,
            character_id: 0,
            weapon: 6,
        }
    }

    #[test]
    fn item07_authority_retains_property_ten_until_expiry() {
        let definition = item_definition("item07").expect("item07 definition");
        assert_eq!(definition.segments.len(), 1);
        assert_eq!(definition.segments[0].property, 10);
        let mut combatant = Combatant::new(1000.0, None, base_weapon_slot(None, 6));
        combatant.apply_item("item07", definition, 10.0);
        let effects = combatant.item_effects_wire(20.0);
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].model, "item07");
        assert_eq!(effects[0].remaining, 50.0);
        combatant.update_item_effects(70.0);
        assert!(combatant.item_effects_wire(70.0).is_empty());
    }

    fn snapshot() -> Message {
        Message::Snapshot {
            player_id: None,
            fighter: FighterSnapshot {
                x: 1.0,
                y: 2.0,
                z: 3.0,
                yaw: 0.0,
                pose_id: 0,
                pose_frame: 0,
                pose_start: 0,
                pose_end: 1,
                pose_loop_start: 0,
                pose_loop_end: 1,
                pose_loops: true,
                hp: 100.0,
                kills: 0,
                max_hp: 100.0,
                weapon: 6,
                movement_mode: 0,
            },
        }
    }

    fn v4_input(seq: u32, role_state: Vec<u8>, role_rate_request: Option<u32>) -> V4Input {
        V4Input {
            seq,
            role_state,
            role_rate_request,
            match_epoch: Some(1),
            round_epoch: Some(1),
            ..Default::default()
        }
    }

    fn role_state(position: RoleVec3, pose_id: u16) -> Vec<u8> {
        encode_request1_state(role_state_from_runtime(RoleStateRuntime {
            position,
            forward: RoleVec3 {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            velocity: RoleVec3::ZERO,
            pose_id,
            grounded: true,
            guard_route: false,
            playback_arg: 0,
            pose_sequence: 0,
        }))
        .to_vec()
    }

    fn identity_obb(x: f32, y: f32, z: f32) -> PackedObb {
        PackedObb {
            values: vec![
                x, y, z, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.75, 1.5, 0.75,
            ],
        }
    }

    fn visual(pose_id: u32, cursor: f32, world_position: Option<Vec3>) -> InputVisual {
        InputVisual {
            pose_id,
            cursor,
            lift: 0.0,
            character: String::new(),
            weapon: String::new(),
            world_position,
            carries_hurt_obbs: false,
            hurt_obbs: Vec::new(),
            carries_hurt_whole_obb: false,
            hurt_whole_obb: None,
        }
    }

    #[test]
    fn hello_creates_identity() {
        let mut server = RoomServer::new();
        server.connect(1);
        let dispatch = server.handle(1, hello());
        assert_eq!(dispatch.to_sender.len(), 1);
        match &dispatch.to_sender[0] {
            Message::Welcome { player_id, .. } => assert_eq!(player_id, "p1"),
            other => panic!("expected welcome, got {other:?}"),
        }
    }

    #[test]
    fn pose_chain_authority_binds_identity_epochs_and_wrapping_sequence() {
        use crate::role_state::PoseChain;
        let mut server = RoomServer::new();
        for conn in [1, 2] {
            server.connect(conn);
            server.handle(conn, hello());
        }
        server.handle(1, create_room("deathmatch", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(2, Message::RoomReady { ready: true });
        server.handle(1, Message::RoomStart);
        let room = &server.rooms["room1"];
        let (match_epoch, round_epoch) = (room.match_epoch, room.round_epoch);
        let request = |seq: u32| Message::RolePoseChain {
            seq,
            room_id: "room1".into(),
            player_id: "forged-player".into(),
            match_epoch,
            round_epoch,
            tick: u32::MAX,
            chain: PoseChain {
                pose_id: 253,
                minimum_seconds: 0.0,
                sequence: (seq + 65533) as u16,
                blend_seconds: 0.3,
            },
        };

        server.rooms.get_mut("room1").unwrap().authority_tick = 20;
        let dispatch = server.handle(2, request(2));
        assert!(dispatch.to_sender.is_empty() && dispatch.to_room.is_empty());
        assert!(matches!(&dispatch.to_others[..], [Message::RolePoseChain {
            player_id, tick: 21, chain, ..
        }] if player_id == "p2" && chain.sequence == 65535 && chain.blend_seconds == 0.3));
        assert!(server.handle(2, request(2)).to_others.is_empty());
        assert_eq!(server.handle(2, request(3)).to_others.len(), 1);
        assert!(server.handle(2, request(1)).to_others.is_empty());
        for bad in 0..5 {
            let mut message = request(4);
            if let Message::RolePoseChain {
                room_id,
                match_epoch,
                round_epoch,
                chain,
                ..
            } = &mut message
            {
                match bad {
                    0 => *room_id = "other-room".into(),
                    1 => *match_epoch += 1,
                    2 => *round_epoch += 1,
                    3 => chain.blend_seconds = f32::NAN,
                    _ => chain.minimum_seconds = 1.0,
                }
            }
            assert!(server.handle(2, message).to_others.is_empty());
        }
        assert_eq!(
            server.handle(2, request(4)).to_others.len(),
            1,
            "bad input must not consume sequence"
        );
        server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p2")
            .unwrap()
            .health = 0.0;
        assert!(server.handle(2, request(5)).to_others.is_empty());
        server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p2")
            .unwrap()
            .health = 100.0;
        server
            .rooms
            .get_mut("room1")
            .unwrap()
            .round
            .as_mut()
            .unwrap()
            .phase = RoundPhase::Break;
        assert!(server.handle(2, request(5)).to_others.is_empty());
    }

    #[test]
    fn authoritative_guard_anger_uses_web_divisor_and_actual_capped_gain() {
        let mut combatant = Combatant::new(1000.0, None, base_weapon_slot(None, 6));
        assert_eq!(CombatStats::gain_anger_on_guard(&mut combatant, 35), 0);
        assert_eq!(CombatStats::gain_anger_on_guard(&mut combatant, 36), 1);
        assert_eq!(combatant.anger, 1.0);

        combatant.anger = PLAYER_ANGER_MAX - 1.0;
        assert_eq!(CombatStats::gain_anger_on_guard(&mut combatant, 1_000), 1);
        assert_eq!(combatant.anger, PLAYER_ANGER_MAX);
    }

    #[test]
    fn ninja_self_effect_authority_death_is_once_and_has_no_killer() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut server =
            RoomServer::with_combat_catalog(AttackCatalog::from_origin_res(&root).unwrap());
        for conn in [1, 2] {
            server.connect(conn);
            server.handle(conn, hello());
        }
        server.handle(1, create_room("deathmatch", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(2, Message::RoomReady { ready: true });
        server.handle(1, Message::RoomStart);
        let room = &server.rooms["room1"];
        let (match_epoch, round_epoch) = (room.match_epoch, room.round_epoch);
        let request = |seq| Message::RoleSelfEffect {
            seq,
            room_id: "room1".into(),
            match_epoch,
            round_epoch,
            pose_id: 468,
            pose_instance: 10,
        };
        let dispatch = server.handle(1, request(1));
        assert!(dispatch
            .to_room
            .iter()
            .any(|m| matches!(m, Message::CombatDeath {
            attacker_id: None, target_id, .. } if target_id == "p1")));
        let caster = &server.rooms["room1"].combatants["p1"];
        assert_eq!(caster.health, 0.0);
        assert_eq!((caster.kills, caster.deaths), (0, 1));
        assert!(
            caster.respawn_at.is_none(),
            "deathmatch preserves the original no-respawn rule"
        );
        assert!(server.handle(1, request(2)).to_room.is_empty());
        RoomServer::update_item_effects(server.rooms.get_mut("room1").unwrap());
        assert_eq!(server.rooms["room1"].combatants["p1"].deaths, 1);
    }

    #[test]
    fn web_self_effect_authority_replay_refresh_expiry_and_death() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut server =
            RoomServer::with_combat_catalog(AttackCatalog::from_origin_res(&root).unwrap());
        for conn in [1, 2] {
            server.connect(conn);
            server.handle(conn, hello());
        }
        server.handle(1, create_room("deathmatch", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(2, Message::RoomReady { ready: true });
        server.handle(1, Message::RoomStart);
        let room = &server.rooms["room1"];
        let (match_epoch, round_epoch) = (room.match_epoch, room.round_epoch);
        let request = |seq, pose_instance| Message::RoleSelfEffect {
            seq,
            room_id: "room1".into(),
            match_epoch,
            round_epoch,
            pose_id: 489,
            pose_instance,
        };
        for invalid in 0..4 {
            let mut message = request(1, 10);
            if let Message::RoleSelfEffect {
                room_id,
                match_epoch,
                round_epoch,
                pose_id,
                ..
            } = &mut message
            {
                match invalid {
                    0 => *room_id = "other-room".into(),
                    1 => *match_epoch += 1,
                    2 => *round_epoch += 1,
                    _ => *pose_id = 490,
                }
            }
            assert!(server.handle(1, message).to_room.is_empty());
        }
        let dispatch = server.handle(1, request(1, 10));
        assert!(
            matches!(&dispatch.to_room[..], [Message::ItemEffect { player_id, item_effects, .. }]
            if player_id == "p1" && item_effects.iter().any(|e| e.model == "status:34" && e.remaining == 90.0))
        );
        assert_eq!(
            CombatStats::attack_bonus(&server.rooms["room1"].combatants["p1"]),
            40.0
        );
        assert_eq!(
            CombatStats::attack_bonus(&server.rooms["room1"].combatants["p2"]),
            0.0
        );
        server.rooms.get_mut("room1").unwrap().elapsed = 30.0;
        assert!(server.handle(1, request(1, 10)).to_room.is_empty());
        assert!(
            server.handle(1, request(2, 10)).to_room.is_empty(),
            "same Action cannot refresh"
        );

        assert_eq!(
            server.rooms["room1"].combatants["p1"].item_effects_wire(30.0)[0].remaining,
            60.0
        );
        assert_eq!(server.handle(1, request(3, 20)).to_room.len(), 1);
        let room = server.rooms.get_mut("room1").unwrap();
        assert_eq!(room.combatants["p1"].hit_status_effects.active.len(), 1);
        assert_eq!(CombatStats::attack_bonus(&room.combatants["p1"]), 40.0);
        room.elapsed = 119.9;
        RoomServer::update_item_effects(room);
        assert_eq!(CombatStats::attack_bonus(&room.combatants["p1"]), 40.0);
        room.elapsed = 120.0;
        let expired = RoomServer::update_item_effects(room);
        assert_eq!(CombatStats::attack_bonus(&room.combatants["p1"]), 0.0);
        assert!(expired.iter().any(
            |e| matches!(e, Message::ItemEffect { player_id, item_effects, .. }
            if player_id == "p1" && item_effects.is_empty())
        ));
        assert_eq!(server.handle(1, request(4, 30)).to_room.len(), 1);
        let caster = server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p1")
            .unwrap();
        caster.health = 0.0;
        caster.clear_item_effects();
        assert_eq!(CombatStats::attack_bonus(caster), 0.0);
        assert!(server.handle(1, request(5, 40)).to_room.is_empty());
        let room = server.rooms.get_mut("room1").unwrap();
        room.combatants.get_mut("p1").unwrap().health = 100.0;
        room.round.as_mut().unwrap().phase = RoundPhase::Break;
        assert!(server.handle(1, request(5, 40)).to_room.is_empty());
    }

    #[test]
    fn room_status_effect_hit_projection_tick_and_expiry() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let catalog = AttackCatalog::from_origin_res(&root).unwrap();
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(1, Message::RoomStart);
        let room = server.rooms.get_mut("room1").unwrap();
        let member = room.members["p1"].clone();
        room.elapsed = 3.0;
        room.combatants
            .get_mut("p2")
            .unwrap()
            .target_hit_clock
            .advance(3.0);
        let attack = catalog.attack_record(0, 210, 0).unwrap();
        let events = resolve_confirmed_hit(room, &catalog, "p1", &member, "p2", 210, &attack);
        assert!(events.iter().any(|event| matches!(event, Message::ItemEffect { item_effects, .. } if item_effects.iter().any(|effect| effect.model == "status:11"))));
        let hp = room.combatants["p2"].health;
        room.elapsed = 6.0;
        let events = RoomServer::update_item_effects(room);
        assert_eq!(room.combatants["p2"].health, hp - 10.0);
        assert!(events
            .iter()
            .any(|event| matches!(event, Message::ItemEffect { .. })));
        room.elapsed = 18.0;
        RoomServer::update_item_effects(room);
        assert_eq!(room.combatants["p2"].health, hp - 40.0);
        assert!(room.combatants["p2"].item_effects_wire(18.0).is_empty());
    }

    #[test]
    fn connection_wire_format_defaults_to_json_and_records_v4() {
        let mut server = RoomServer::new();
        server.connect(1);
        assert_eq!(server.wire_format(1), WireFormat::Json);
        server.set_wire_format(1, WireFormat::V4);
        assert_eq!(server.wire_format(1), WireFormat::V4);
        assert_eq!(server.wire_format(404), WireFormat::Json);
    }

    #[test]
    fn create_room_and_join() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        let create = server.handle(1, create_room("leader", "sn01"));

        assert!(create
            .to_sender
            .iter()
            .any(|m| matches!(m, Message::RoomState { members, .. } if members.len() == 1)));
        let join = server.handle(2, join_room("room1"));
        assert!(join
            .to_sender
            .iter()
            .any(|m| matches!(m, Message::RoomState { members, .. } if members.len() == 2)));
        assert!(join
            .to_others
            .iter()
            .any(|m| matches!(m, Message::RoomState { members, .. } if members.len() == 2)));
    }

    #[test]
    fn room_state_broadcast_carries_friendly_fire_rule() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.handle(1, hello());
        server.handle(1, create_room("deathmatch", "sn01"));
        server.handle(
            1,
            Message::RoomSettings {
                mode: "deathmatch".into(),
                stage_id: "sn01".into(),
                rules: RoomRulesWire {
                    friendly_fire: Some(true),
                    ..Default::default()
                },
            },
        );

        let state = RoomServer::room_state_from(&server.rooms, "room1")
            .expect("configured room has room.state");
        assert!(matches!(
            state,
            Message::RoomState { rules, .. } if rules.friendly_fire == Some(true)
        ));
    }

    #[test]
    fn objective_modes_reject_stages_without_original_objective_data() {
        for (mode, stage) in [("escort", "sn01"), ("siege", "sn01")] {
            let mut server = RoomServer::new();
            server.connect(1);
            server.handle(1, hello());

            let rejected = server.handle(1, create_room(mode, stage));
            assert!(matches!(
                rejected.to_sender.as_slice(),
                [Message::Error { code, .. }] if code == "bad_room_settings"
            ));
            assert!(server.rooms.is_empty());
        }
    }

    #[test]
    fn objective_stage_whitelist_covers_settings_auto_join_and_start_defense() {
        let mut settings_server = RoomServer::new();
        settings_server.connect(1);
        settings_server.handle(1, hello());
        settings_server.handle(1, create_room("leader", "sn01"));
        let rejected_settings = settings_server.handle(
            1,
            Message::RoomSettings {
                mode: "siege".into(),
                stage_id: "sn01".into(),
                rules: RoomRulesWire::default(),
            },
        );
        assert!(matches!(
            rejected_settings.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "bad_room_settings"
        ));
        assert_eq!(settings_server.rooms["room1"].mode, "leader");

        let mut auto_join_server = RoomServer::new();
        auto_join_server.connect(1);
        auto_join_server.handle(1, hello());
        let rejected_auto_join = auto_join_server.handle(
            1,
            Message::RoomJoin {
                room_id: String::new(),
                mode: Some("escort".into()),
                stage: Some("sn01".into()),
                name: "tester".into(),
                character_id: 0,
                weapon: 6,
                password: String::new(),
            },
        );
        assert!(matches!(
            rejected_auto_join.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "bad_room_settings"
        ));
        assert!(auto_join_server.rooms.is_empty());

        let mut start_server = RoomServer::new();
        start_server.connect(1);
        start_server.handle(1, hello());
        start_server.handle(1, create_room("leader", "sn01"));
        let room = start_server.rooms.get_mut("room1").unwrap();
        room.mode = "siege".into();
        let rejected_start = start_server.handle(1, Message::RoomStart);
        assert!(matches!(
            rejected_start.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "SIEGE_STAGE_UNSUPPORTED"
        ));
    }

    #[test]
    fn web_create_preserves_unconfigured_room_loadout_rules_and_password_gate() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());

        let created = server.handle(
            1,
            Message::RoomCreate {
                mode: String::new(),
                stage: String::new(),
                name: "Web host".into(),
                character_id: 3,
                weapon: 8,
                room_name: "Web lobby".into(),
                password: "secret".into(),
                rules: RoomRulesWire {
                    max_players: Some(8),
                    life_adjust: Some(175),
                    hide_item: Some(false),
                    ..Default::default()
                },
            },
        );
        assert_eq!(
            created.to_conn.len(),
            1,
            "room.joined is emitted once per recipient"
        );
        let room = created
            .to_conn
            .iter()
            .find_map(|(target, message)| {
                (*target == 1).then_some(match message {
                    Message::RoomJoined { room, .. } => Some(room),
                    _ => None,
                })
            })
            .flatten()
            .expect("host receives a Web-shaped room.joined");
        assert_eq!(room["name"], "Web lobby");
        assert!(room["mode"].is_null());
        assert!(room["stageId"].is_null());
        assert_eq!(room["configured"], false);
        assert_eq!(room["hasPassword"], true);
        assert_eq!(room["players"][0]["character"], "p3");
        assert_eq!(room["players"][0]["weapon"], "blade");
        assert_eq!(room["maxPlayers"], 8);
        assert_eq!(room["rules"]["lifeAdjust"], 175);
        assert_eq!(room["rules"]["hideItem"], false);
        assert_eq!(room["players"][0]["maxHealth"], 1750);

        let summary = &server.room_summaries()[0];
        assert_eq!(summary.mode, None);
        assert_eq!(summary.stage_id, None);
        assert!(!summary.configured);
        assert!(summary.has_password);

        let missing_password = server.handle(2, join_room("room1"));
        assert!(matches!(
            missing_password.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "ROOM_PASSWORD_REQUIRED"
        ));
        let wrong_password = server.handle(
            2,
            Message::RoomJoin {
                room_id: "room1".into(),
                mode: None,
                stage: None,
                name: "guest".into(),
                character_id: 4,
                weapon: 9,
                password: "wrong".into(),
            },
        );
        assert!(matches!(
            wrong_password.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "ROOM_PASSWORD_WRONG"
        ));
        let joined = server.handle(
            2,
            Message::RoomJoin {
                room_id: "room1".into(),
                mode: None,
                stage: None,
                name: "guest".into(),
                character_id: 4,
                weapon: 9,
                password: "secret".into(),
            },
        );
        assert!(joined.to_conn.iter().any(|(target, message)| {
            *target == 2
                && matches!(message, Message::RoomJoined { room, .. }
                    if room["players"][1]["character"] == "p4" && room["players"][1]["weapon"] == "hammer")
        }));

        server.connect(3);
        server.handle(3, hello());
        let automatic = server.handle(3, join_empty());
        assert!(automatic.to_sender.iter().any(|message| {
            matches!(message, Message::RoomState { room_id, .. } if room_id == "room2")
        }));

        let rejected_start = server.handle(1, Message::RoomStart);
        assert!(matches!(
            rejected_start.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "ROOM_NOT_CONFIGURED"
        ));
        let configured = server.handle(
            1,
            Message::RoomSettings {
                mode: "leader".into(),
                stage_id: "sn05".into(),
                rules: RoomRulesWire {
                    round_time: Some(15),
                    game_num: Some(3),
                    friendly_fire: Some(true),
                    ..Default::default()
                },
            },
        );
        assert!(configured.to_conn.iter().any(|(target, message)| {
            *target == 1
                && matches!(message, Message::RoomJoined { room, .. }
                    if room["configured"] == true
                        && room["mode"] == "leader"
                        && room["stageId"] == "sn05"
                        && room["rules"]["roundTime"] == 15
                        && room["rules"]["gameNum"] == 3
                        && room["rules"]["friendlyFire"] == false
                        && room["players"][1]["ready"] == true)
        }));
    }

    #[test]
    fn room_list_matches_web_summary_shape_and_order() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        assert!(matches!(
            server.handle(1, Message::RoomList { rooms: None }).to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "no_identity"
        ));
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(2, create_room("deathmatch", "sn02"));
        server.handle(
            1,
            Message::MatchStart {
                room_id: String::new(),
                match_epoch: None,
                round_epoch: None,
                mode: String::new(),
                stage: String::new(),
                players: vec![],
                slots: vec![],
                resumed: false,
                late_joined: false,
                authority: String::new(),
                host_id: String::new(),
            },
        );
        let list = server.handle(2, Message::RoomList { rooms: None });
        let Message::RoomList { rooms: Some(rooms) } = &list.to_sender[0] else {
            panic!("expected room list, got {:?}", list.to_sender);
        };
        assert_eq!(
            rooms
                .iter()
                .map(|room| room.room_id.as_str())
                .collect::<Vec<_>>(),
            vec!["room2", "room1"],
        );
        let lobby = &rooms[0];
        assert_eq!(lobby.name, "tester 的房间");
        assert_eq!(lobby.mode.as_deref(), Some("deathmatch"));
        assert_eq!(lobby.stage_id.as_deref(), Some("sn02"));
        assert!(lobby.configured);
        assert_eq!(lobby.state, "lobby");
        assert!(!lobby.has_password);
        assert_eq!(lobby.player_count, 1);
        assert_eq!(lobby.max_players, DEFAULT_MAX_PLAYERS);
        assert_eq!(lobby.owner_name.as_deref(), Some("tester"));
        assert_eq!(rooms[1].state, "running");
    }

    #[test]
    fn scene_ops_require_host_and_late_join_receives_collapsed_set_state() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("leader", "sn01"));

        let host_ops = vec![
            SceneOp {
                kind: "set".into(),
                reference: JsonValue::from(4),
                field: "attribute".into(),
                args: vec![JsonValue::from("active"), JsonValue::from(0)],
            },
            SceneOp {
                kind: "set".into(),
                reference: JsonValue::from(4),
                field: "attribute".into(),
                args: vec![JsonValue::from("interactive"), JsonValue::from(0)],
            },
            SceneOp {
                kind: "set".into(),
                reference: JsonValue::from(4),
                field: "attribute".into(),
                args: vec![JsonValue::from("active"), JsonValue::from(1)],
            },
        ];
        let accepted = server.handle(
            1,
            Message::SceneOps {
                room_id: String::new(),
                authority_generated: false,
                ops: host_ops.clone(),
            },
        );
        assert!(accepted.to_sender.is_empty());
        let joined = server.handle(2, join_room("room1"));
        let scene_state = server
            .rooms
            .get("room1")
            .map(|room| room.scene_state.clone())
            .expect("room exists");
        assert_eq!(scene_state.len(), 2);
        assert_eq!(scene_state[0].args, host_ops[1].args);
        assert_eq!(scene_state[1].args, host_ops[2].args);
        assert!(joined.to_conn.iter().any(|(target, message)| {
            *target == 2
                && matches!(message, Message::RoomJoined { scene_state: delivered, .. }
                    if delivered == &scene_state)
        }));

        let rejected = server.handle(
            2,
            Message::SceneOps {
                room_id: "room1".into(),
                authority_generated: false,
                ops: host_ops,
            },
        );
        assert!(matches!(
            rejected.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "SCENE_OPS_REJECTED"
        ));
    }

    #[test]
    fn scene_prop_hit_authenticates_sender_and_damage_without_remote_pose_updates() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut server = RoomServer::from_origin_res(&root).unwrap();
        for conn in [1, 2, 3] {
            server.connect(conn);
            server.handle(conn, hello());
        }
        server.handle(1, create_room("deathmatch", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(1, Message::RoomStart);
        let room = &server.rooms["room1"];
        let hit = Message::ScenePropHit {
            room_id: room.id.clone(),
            match_epoch: room.match_epoch,
            round_epoch: room.round_epoch,
            player_id: "p1".into(),
            reference: "D_BBox01".into(),
            pose_id: 262,
            frame: 9770.0,
            damage: i32::MAX,
        };

        assert!(room.role_states.is_empty());
        let accepted = server.handle(2, hit.clone());
        assert!(matches!(accepted.to_conn.as_slice(),
            [(1, Message::ScenePropHit { player_id, damage, reference, .. })]
                if player_id == "p2" && *damage > 0 && *damage < i32::MAX && reference == "D_BBox01"));
        assert!(accepted.to_room.is_empty() && accepted.to_others.is_empty());
        assert!(server.handle(3, hit.clone()).to_conn.is_empty(), "outsider");
        assert!(
            server.handle(1, hit.clone()).to_conn.is_empty(),
            "host already applies its local hit"
        );
        let mut stale = hit.clone();
        if let Message::ScenePropHit { round_epoch, .. } = &mut stale {
            *round_epoch += 1;
        }
        assert!(server.handle(2, stale).to_conn.is_empty());
        let mut invalid = hit.clone();
        if let Message::ScenePropHit { frame, .. } = &mut invalid {
            *frame = f32::NAN;
        }
        assert!(server.handle(2, invalid).to_conn.is_empty());
        let mut idle = hit.clone();
        if let Message::ScenePropHit { pose_id, frame, .. } = &mut idle {
            *pose_id = 0;
            *frame = 0.0;
        }
        assert!(server.handle(2, idle).to_conn.is_empty());
        server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p2")
            .unwrap()
            .health = 0.0;
        assert!(server.handle(2, hit).to_conn.is_empty(), "dead player");
    }

    #[test]
    fn scene_item_claim_is_host_validated_authoritative_and_respawns_from_resource_delay() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let mut server = RoomServer::from_origin_res(&root).expect("original catalog loads");
        server.connect(1);
        server.handle(1, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(1, Message::RoomStart);
        let room = server.rooms.get_mut("room1").unwrap();
        let collector = room.combatants.get_mut("p1").unwrap();
        collector.position = RoleVec3::default();
        collector.health = 400.0;

        let accepted = server.handle(
            1,
            Message::ItemClaim {
                reference: "D_It15".into(),
                player_id: "p1".into(),
                model: "item15".into(),
                position: Some([0.0, 0.0, 0.0]),
                renewable: true,
            },
        );
        assert!(accepted.to_room.iter().any(|message| {
            matches!(message, Message::SceneOps { authority_generated, ops, .. }
                if *authority_generated
                    && ops.len() == 2
                    && ops[0].args == [JsonValue::from("active"), JsonValue::from(0)])
        }));
        assert!(accepted.to_room.iter().any(|message| {
            matches!(message, Message::ItemClaimed {
                reference,
                player_id,
                kind,
                health,
                respawn_seconds,
                ..
            } if reference == "D_It15"
                && player_id == "p1"
                && kind == "item"
                && *health == 450.0
                && *respawn_seconds == Some(10.0))
        }));
        assert_eq!(server.rooms["room1"].claimed_scene_items.len(), 1);
        assert_eq!(server.rooms["room1"].combatants["p1"].health, 450.0);

        let duplicate = server.handle(
            1,
            Message::ItemClaim {
                reference: "D_It15".into(),
                player_id: "p1".into(),
                model: "item15".into(),
                position: Some([0.0, 0.0, 0.0]),
                renewable: true,
            },
        );
        assert!(matches!(
            duplicate.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "ITEM_CLAIM_REJECTED"
        ));

        let mut early_events = Vec::new();
        for _ in 0..39 {
            early_events.extend(server.advance_authority(0.25, 0.0).events);
        }
        assert!(!early_events.iter().any(|(_, message)| {
            matches!(message, Message::ItemRespawned { reference, .. } if reference == "D_It15")
        }));
        let due = server.advance_authority(0.25, 0.0);
        assert!(due.events.iter().any(|(_, message)| {
            matches!(message, Message::ItemRespawned { reference, model }
                if reference == "D_It15" && model == "item15")
        }));
        assert!(server.rooms["room1"].claimed_scene_items.is_empty());
    }

    #[test]
    fn ws_sn04_bun_claim_heals_guest_and_broadcasts_once() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut server = RoomServer::from_origin_res(&root).unwrap();
        for connection in [1, 2] {
            server.connect(connection);
            server.handle(connection, hello());
        }
        server.handle(1, create_room("siege", "sn04"));
        server.handle(2, join_room("room1"));
        server.handle(1, Message::RoomStart);
        let room = server.rooms.get_mut("room1").unwrap();
        room.combatants.get_mut("p1").unwrap().position = RoleVec3 {
            x: 50.0,
            y: 0.0,
            z: 0.0,
        };
        room.combatants.get_mut("p2").unwrap().position = RoleVec3::ZERO;
        room.combatants.get_mut("p2").unwrap().health = 400.0;
        let claim = Message::ItemClaim {
            reference: "D_itRJug0101".into(),
            player_id: "p2".into(),
            model: "Item01".into(),
            position: Some([0.0; 3]),
            renewable: true,
        };
        assert!(
            server.handle(2, claim.clone()).to_room.is_empty(),
            "guest cannot self-award health"
        );
        let accepted = server.handle(1, claim.clone());
        assert!(
            matches!(accepted.to_room.iter().find(|m| matches!(m, Message::ItemClaimed { .. })),
            Some(Message::ItemClaimed { player_id, health, .. }) if player_id == "p2" && *health > 400.0)
        );
        let healed = server.rooms["room1"].combatants["p2"].health;
        assert!(server.handle(1, claim).to_room.is_empty());
        assert_eq!(server.rooms["room1"].combatants["p2"].health, healed);
    }

    #[test]
    fn scene_item_claim_uses_authoritative_collector_and_weapon_slots() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let mut server = RoomServer::from_origin_res(&root).expect("original catalog loads");
        server.connect(1);
        server.handle(1, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(1, Message::RoomStart);
        server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p1")
            .unwrap()
            .position = RoleVec3::default();

        let out_of_range = server.handle(
            1,
            Message::ItemClaim {
                reference: "D_Wp01".into(),
                player_id: "p1".into(),
                model: "W1_0".into(),
                position: Some([6.01, 0.0, 0.0]),
                renewable: true,
            },
        );
        assert!(matches!(
            out_of_range.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "ITEM_CLAIM_REJECTED"
        ));

        let accepted = server.handle(
            1,
            Message::ItemClaim {
                reference: "D_Wp01".into(),
                player_id: "p1".into(),
                model: "W1_0".into(),
                position: Some([6.0, 0.0, 0.0]),
                renewable: true,
            },
        );
        assert!(accepted.to_room.iter().any(|message| {
            matches!(message, Message::ItemClaimed {
                kind,
                picked_weapon_model,
                weapon_slots,
                respawn_seconds,
                ..
            } if kind == "weapon"
                && picked_weapon_model.as_deref() == Some("w1_0")
                && weapon_slots.iter().filter(|slot| slot.present).count() == 2
                && *respawn_seconds == Some(45.0))
        }));
        let collector = &server.rooms["room1"].combatants["p1"];
        assert_eq!(
            collector.weapon_slots[1]
                .as_ref()
                .map(|slot| slot.model.as_str()),
            Some("w1_0")
        );
    }

    #[test]
    fn dynamic_weapon_drop_switches_snapshots_and_reclaims_without_scene_ops() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let mut server = RoomServer::from_origin_res(&root).expect("original catalog loads");
        server.connect(1);
        server.handle(1, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(1, Message::RoomStart);
        server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p1")
            .unwrap()
            .position = RoleVec3::default();

        server.handle(
            1,
            Message::ItemClaim {
                reference: "D_Wp01".into(),
                player_id: "p1".into(),
                model: "W1_0".into(),
                position: Some([0.0, 0.0, 0.0]),
                renewable: true,
            },
        );
        let switched = accept_combat_action(
            server.rooms.get_mut("room1").unwrap(),
            "p1",
            "switch_weapon",
        );
        assert!(matches!(
            switched.as_slice(),
            [Message::WeaponSwitched { weapon_model, weapon_slot, .. }]
                if weapon_model.as_deref() == Some("w1_0") && *weapon_slot == 1
        ));
        let dropped =
            accept_combat_action(server.rooms.get_mut("room1").unwrap(), "p1", "drop_weapon");
        assert!(matches!(
            dropped.as_slice(),
            [Message::WeaponDropped { reference, model, position, velocity, weapon_model, weapon_slot, .. }]
                if reference == "D_wpNet1"
                    && model == "w0_0"
                    && *position == [0.0, 6.0, 0.0]
                    && *velocity == [0.0, 0.0, 12.0]
                    && weapon_model.as_deref() == Some("w1_0")
                    && *weapon_slot == 1
        ));
        let snapshot = RoomServer::v4_snapshot_for(&server.rooms["room1"], "p1", 1, 0.0);
        assert!(matches!(
            snapshot.drops.as_slice(),
            [V4SnapshotDrop { r#ref, model, position: Some(position), settled, .. }]
                if r#ref == "D_wpNet1"
                    && model == "w0_0"
                    && position.y == 6.0
                    && position.z == 0.0
                    && !settled
        ));

        let early = server.handle(
            1,
            Message::ItemClaim {
                reference: "D_wpNet1".into(),
                player_id: "p1".into(),
                model: "item01".into(),
                position: Some([999.0, 0.0, 999.0]),
                renewable: true,
            },
        );
        assert!(matches!(
            early.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "ITEM_CLAIM_REJECTED"
        ));
        server.advance_authority(0.25, 0.0);
        let airborne = RoomServer::v4_snapshot_for(&server.rooms["room1"], "p1", 2, 0.0);
        assert!(!airborne.drops[0].settled);
        server.advance_authority(0.10, 0.0);
        assert!(
            !RoomServer::v4_snapshot_for(&server.rooms["room1"], "p1", 3, 0.0).drops[0].settled
        );
        let room = &server.rooms["room1"];
        let ack = Message::WeaponDropRest {
            match_epoch: Some(room.match_epoch),
            round_epoch: Some(room.round_epoch),
            reference: "D_wpNet1".into(),
            position: [0.0, 1.0, 1.0],
        };
        let accepted = server.handle(1, ack.clone());
        assert!(
            matches!(accepted.to_room.as_slice(), [Message::WeaponDropRest { position, .. }] if *position == [0.0, 1.0, 1.0])
        );
        assert!(
            server.handle(1, ack).to_room.is_empty(),
            "duplicate confirmation is idempotent"
        );
        let ready = RoomServer::v4_snapshot_for(&server.rooms["room1"], "p1", 4, 0.0);
        assert!(ready.drops[0].settled);
        assert_eq!(
            ready.drops[0].position.as_ref().unwrap().z,
            1.0,
            "late join sees the wall point"
        );

        let reclaimed = server.handle(
            1,
            Message::ItemClaim {
                reference: "D_wpNet1".into(),
                player_id: "p1".into(),
                model: "item01".into(),
                position: Some([999.0, 0.0, 999.0]),
                renewable: true,
            },
        );
        assert!(reclaimed.to_room.iter().any(|message| {
            matches!(message, Message::ItemClaimed { model, respawn_seconds, .. }
                if model == "w0_0" && respawn_seconds.is_none())
        }));
        assert!(!reclaimed
            .to_room
            .iter()
            .any(|message| matches!(message, Message::SceneOps { .. })));
        let room = &server.rooms["room1"];
        assert!(room.dynamic_weapon_drops.is_empty());
        assert_eq!(room.claimed_scene_items.len(), 1);
        assert!(!room.claimed_scene_items.contains_key("D_wpNet1"));
    }

    #[test]
    fn anger_spend_action_is_authoritative_and_idempotent() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.handle(1, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(1, Message::RoomStart);
        server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p1")
            .unwrap()
            .anger = 100.0;
        let action = crate::protocol::encode_anger_spend_action(17, 150, 60).unwrap();

        let events = accept_combat_action(server.rooms.get_mut("room1").unwrap(), "p1", &action);
        assert!(
            matches!(&events[..], [Message::ItemEffect { player_id, anger: 40.0, .. }] if player_id == "p1")
        );
        assert_eq!(server.rooms["room1"].combatants["p1"].anger, 40.0);

        assert!(
            accept_combat_action(server.rooms.get_mut("room1").unwrap(), "p1", &action).is_empty()
        );
        assert_eq!(
            server.rooms["room1"].combatants["p1"].anger, 40.0,
            "transport replay cannot deduct the same pose/action twice"
        );
        let forged = crate::protocol::encode_anger_spend_action(18, 150, 10).unwrap();
        accept_combat_action(server.rooms.get_mut("room1").unwrap(), "p1", &forged);
        assert_eq!(server.rooms["room1"].combatants["p1"].anger, 40.0);
    }

    #[test]
    fn weapon_drop_rest_rejects_non_owner_stale_nonfinite_and_expired_updates() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut server = RoomServer::from_origin_res(&root).unwrap();
        server.connect(1);
        server.handle(1, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(1, Message::RoomStart);
        accept_combat_action(server.rooms.get_mut("room1").unwrap(), "p1", "drop_item");
        let room = &server.rooms["room1"];
        let (m, r) = (room.match_epoch, room.round_epoch);
        let ack = |epoch, position| Message::WeaponDropRest {
            match_epoch: Some(epoch),
            round_epoch: Some(r),
            reference: "D_wpNet1".into(),
            position,
        };
        assert!(server
            .handle(1, ack(m.wrapping_add(1), [0.0; 3]))
            .to_room
            .is_empty());
        assert!(server
            .handle(1, ack(m, [f32::NAN, 0.0, 0.0]))
            .to_room
            .is_empty());

        server
            .rooms
            .get_mut("room1")
            .unwrap()
            .order
            .insert(0, "other-owner".into());
        assert!(server.handle(1, ack(m, [0.0; 3])).to_room.is_empty());
        server.rooms.get_mut("room1").unwrap().order.remove(0);
        assert!(!server.rooms["room1"].dynamic_weapon_drops["D_wpNet1"]
            .claimable_at
            .is_finite());
        server.rooms.get_mut("room1").unwrap().elapsed += 5.001;
        assert!(server.handle(1, ack(m, [0.0; 3])).to_room.is_empty());
        server.advance_authority(0.01, 0.0);
        assert!(server.rooms["room1"].dynamic_weapon_drops.is_empty());
    }

    #[test]
    fn drop_item_throws_copy_without_mutating_carried_weapon_slots() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.handle(1, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(1, Message::RoomStart);
        let before = server.rooms["room1"].combatants["p1"].weapon_slots_wire();
        let slot = server.rooms["room1"].combatants["p1"].weapon_slot;

        let dropped =
            accept_combat_action(server.rooms.get_mut("room1").unwrap(), "p1", "drop_item");
        assert!(matches!(
            dropped.as_slice(),
            [Message::WeaponDropped { reference, weapon_slot, weapon_slots, .. }]
                if reference == "D_wpNet1"
                    && *weapon_slot == slot as u8
                    && *weapon_slots == before
        ));
        let combatant = &server.rooms["room1"].combatants["p1"];
        assert_eq!(combatant.weapon_slot, slot);
        assert_eq!(combatant.weapon_slots_wire(), before);
    }

    #[test]
    fn drop_item_copy_precedes_the_alive_gate_like_command_28() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.handle(1, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(1, Message::RoomStart);
        let room = server.rooms.get_mut("room1").unwrap();
        room.combatants.get_mut("p1").unwrap().health = 0.0;

        let dropped = accept_combat_action(room, "p1", "drop_item");
        assert!(matches!(
            dropped.as_slice(),
            [Message::WeaponDropped { reference, .. }] if reference == "D_wpNet1"
        ));
    }

    #[test]
    fn drop_item_copy_quota_does_not_consume_slots_or_block_g() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.handle(1, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(1, Message::RoomStart);
        let room = server.rooms.get_mut("room1").unwrap();
        let spare = room.combatants["p1"].active_weapon().cloned().unwrap();
        room.combatants.get_mut("p1").unwrap().weapon_slots[1] = Some(spare);
        let before = room.combatants["p1"].weapon_slots_wire();

        for _ in 0..10 {
            accept_combat_action(room, "p1", "drop_item");
        }
        assert_eq!(
            room.dynamic_weapon_drops
                .values()
                .filter(|drop| drop.source == DynamicWeaponDropSource::DropItem)
                .count(),
            MAX_DYNAMIC_DROPS_PER_PLAYER
        );
        assert_eq!(room.combatants["p1"].weapon_slots_wire(), before);

        let dropped = accept_combat_action(room, "p1", "drop_weapon");
        assert!(matches!(
            dropped.as_slice(),
            [Message::WeaponDropped { .. }]
        ));
        assert_eq!(
            room.dynamic_weapon_drops.len(),
            MAX_DYNAMIC_DROPS_PER_PLAYER + 1
        );

        let filler = room.dynamic_weapon_drops.values().next().unwrap().clone();
        while room.dynamic_weapon_drops.len() < MAX_DYNAMIC_DROPS_PER_ROOM {
            let index = room.dynamic_weapon_drops.len();
            room.dynamic_weapon_drops
                .insert(format!("D_wpFiller{index}"), filler.clone());
        }
        let next_ref = room.next_dynamic_drop_id;
        assert!(accept_combat_action(room, "p1", "drop_item").is_empty());
        assert_eq!(room.next_dynamic_drop_id, next_ref);
    }

    #[test]
    fn siege_objective_end_requires_scene_authority_and_original_stage() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("siege", "sn04"));
        server.handle(2, join_room("room1"));
        server.handle(1, Message::RoomStart);

        let forged_match_end = server.handle(
            2,
            Message::MatchEnd {
                winner_team: Some(2),
                winner_player_id: None,
                result: Some("victory".into()),
                reason: "forged".into(),
                lobby_return_seconds: None,
            },
        );
        assert!(matches!(
            forged_match_end.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "MATCH_END_REJECTED"
        ));

        let non_host = server.handle(2, Message::ObjectiveEnd { winner_team: 1 });
        assert!(matches!(
            non_host.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "OBJECTIVE_END_REJECTED"
        ));
        let bad_team = server.handle(1, Message::ObjectiveEnd { winner_team: 3 });
        assert!(matches!(
            bad_team.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "OBJECTIVE_END_REJECTED"
        ));
        assert!(server.rooms.get("room1").is_some_and(|room| room.started));

        let accepted = server.handle(1, Message::ObjectiveEnd { winner_team: 2 });
        assert!(accepted.to_room.iter().any(|message| {
            matches!(message, Message::RoundEnd { winner_team, reason, .. }
                if *winner_team == Some(2) && reason == "siege-objective")
        }));
        assert!(server.rooms.get("room1").is_some_and(|room| room.started));
    }

    #[test]
    fn escort_uses_sn03_flag_hull_safe_zone_drop_and_return_authority() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("escort", "sn03"));
        server.handle(2, join_room("room1"));
        server.handle(2, Message::RoomReady { ready: true });
        server.handle(1, Message::RoomStart);

        let home = escort_stage("sn03").unwrap().flag_home;
        server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p1")
            .unwrap()
            .position = home;
        let taken = server.advance_authority(1.0 / 60.0, 0.0);
        assert!(taken.events.iter().any(|(_, message)| {
            matches!(message, Message::EscortTaken { player_id, team, position }
                if player_id == "p1" && *team == 1 && *position == [home.x, home.y, home.z])
        }));
        assert!(
            RoomServer::v4_snapshot_for(&server.rooms["room1"], "p1", 1, 0.0)
                .flag
                .is_some()
        );

        let room = server.rooms.get_mut("room1").unwrap();
        let carrier = room.combatants.get_mut("p1").unwrap();
        carrier.position = RoleVec3 {
            x: 171.0,
            y: 99.0,
            z: 140.0,
        };
        let delivered = server.advance_authority(1.0 / 60.0, 0.0);
        assert!(delivered.events.iter().any(|(_, message)| {
            matches!(message, Message::EscortDelivered { player_id, team, safe_zone }
                if player_id == "p1" && *team == 1 && safe_zone == "safeA")
        }));
        assert!(delivered.events.iter().any(|(_, message)| {
            matches!(message, Message::RoundEnd { winner_team, reason, .. }
                if *winner_team == Some(1) && reason == "escort")
        }));

        let mut returned_server = RoomServer::new();
        returned_server.connect(1);
        returned_server.connect(2);
        returned_server.handle(1, hello());
        returned_server.handle(2, hello());
        returned_server.handle(1, create_room("escort", "sn03"));
        returned_server.handle(2, join_room("room1"));
        returned_server.handle(2, Message::RoomReady { ready: true });
        returned_server.handle(1, Message::RoomStart);
        returned_server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p1")
            .unwrap()
            .position = home;
        returned_server.advance_authority(1.0 / 60.0, 0.0);
        returned_server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p1")
            .unwrap()
            .health = 0.0;
        let dropped = returned_server.advance_authority(1.0 / 60.0, 0.0);
        assert!(dropped.events.iter().any(|(_, message)| {
            matches!(message, Message::EscortDropped { player_id, return_seconds, .. }
                if player_id == "p1" && *return_seconds == 5.0)
        }));
        let mut return_events = Vec::new();
        for _ in 0..20 {
            return_events.extend(returned_server.advance_authority(0.25, 0.0).events);
        }
        assert!(return_events.iter().any(|(_, message)| {
            matches!(message, Message::EscortReturned { reason, position }
                if reason == "timeout" && *position == [home.x, home.y, home.z])
        }));
    }

    #[test]
    fn match_start_broadcasts_slots() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("deathmatch", "sn01"));
        server.handle(2, join_room("room1"));
        let start = server.handle(
            1,
            Message::MatchStart {
                room_id: String::new(),
                match_epoch: None,
                round_epoch: None,
                mode: String::new(),
                stage: String::new(),
                players: vec![],
                slots: vec![],
                resumed: false,
                late_joined: false,
                authority: String::new(),
                host_id: String::new(),
            },
        );
        assert!(start
            .to_room
            .iter()
            .any(|m| matches!(m, Message::MatchStart { slots, .. } if slots == &["p1".to_string(), "p2".to_string()])));

        let denied = server.handle(
            2,
            Message::MatchStart {
                room_id: String::new(),
                match_epoch: None,
                round_epoch: None,
                mode: String::new(),
                stage: String::new(),
                players: vec![],
                slots: vec![],
                resumed: false,
                late_joined: false,
                authority: String::new(),
                host_id: String::new(),
            },
        );
        assert!(denied
            .to_sender
            .iter()
            .any(|m| matches!(m, Message::Error { code, .. } if code == "not_host")));
    }

    #[test]
    fn authority_round_lifecycle_uses_web_break_and_lobby_gates() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("deathmatch", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(2, Message::RoomReady { ready: true });
        server.rooms.get_mut("room1").unwrap().rules.game_num = Some(1);

        let start = server.handle(1, Message::RoomStart);
        assert!(matches!(
            start.to_room.first(),
            Some(Message::RoundStart {
                round: 1,
                round_time: 999,
                game_num: 1,
                ..
            })
        ));

        server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p2")
            .unwrap()
            .health = 0.0;
        let ended = server.advance_authority(1.0 / 60.0, 0.0);
        assert!(ended.events.iter().any(|(_, message)| {
            matches!(message, Message::RoundEnd { reason, winner_team, .. }
                if reason == "eliminated" && *winner_team == Some(1))
        }));
        assert!(ended.events.iter().any(|(_, message)| {
            matches!(message, Message::MatchEnd {
                winner_team: Some(1),
                result: Some(result),
                lobby_return_seconds: Some(seconds),
                ..
            } if result == "victory" && *seconds == ROUND_SETTLE_SECONDS)
        }));
        assert!(server.rooms.get("room1").is_some_and(|room| !room.started));
        assert_eq!(server.room_summaries()[0].state, "ended");

        for _ in 0..40 {
            server.advance_authority(0.25, 0.0);
        }
        assert!(server.rooms["room1"].lobby_return_at.is_some());
        let reset = server.advance_authority(0.25, 0.0);
        assert!(reset
            .events
            .iter()
            .any(|(_, message)| matches!(message, Message::MatchReset)));
        assert!(server.rooms["room1"]
            .members
            .values()
            .all(|member| !member.ready));
    }

    #[test]
    fn snapshots_relay_to_others_with_player_id() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(
            1,
            Message::MatchStart {
                room_id: String::new(),
                match_epoch: None,
                round_epoch: None,
                mode: String::new(),
                stage: String::new(),
                players: vec![],
                slots: vec![],
                resumed: false,
                late_joined: false,
                authority: String::new(),
                host_id: String::new(),
            },
        );
        let relay = server.handle(1, snapshot());
        assert!(relay.to_others.iter().any(|m| matches!(
            m,
            Message::Snapshot {
                player_id: Some(id),
                ..
            } if id == "p1"
        )));

        let mut server2 = RoomServer::new();
        server2.connect(1);
        server2.handle(1, hello());
        server2.handle(1, create_room("leader", "sn01"));
        let quiet = server2.handle(1, snapshot());
        assert!(quiet.to_others.is_empty());
    }

    #[test]
    fn room_start_balances_team_modes_and_exports_snapshot_teams() {
        let mut server = RoomServer::new();
        for conn_id in 1..=3 {
            server.connect(conn_id);
            server.handle(conn_id, hello());
        }
        server.handle(1, create_room("deathmatch", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(3, join_room("room1"));
        server.handle(1, Message::RoomStart);

        let room = server.rooms.get("room1").unwrap();
        assert_eq!(
            room.combatants.get("p1").and_then(|state| state.team),
            Some(1)
        );
        assert_eq!(
            room.combatants.get("p2").and_then(|state| state.team),
            Some(2)
        );
        assert_eq!(
            room.combatants.get("p3").and_then(|state| state.team),
            Some(1)
        );

        let snapshot = RoomServer::v4_snapshot_for(room, "p1", 1, 0.0);
        let teams = snapshot
            .players
            .into_iter()
            .map(|player| (player.id, player.team))
            .collect::<HashMap<_, _>>();
        assert_eq!(teams.get("p1"), Some(&Some(1)));
        assert_eq!(teams.get("p2"), Some(&Some(2)));
        assert_eq!(teams.get("p3"), Some(&Some(1)));

        assert_eq!(
            initial_lobby_team("leader", std::iter::empty::<&RoomMemberWire>()),
            None
        );
    }

    #[test]
    fn assassin_round_marks_player_vips_and_ends_on_first_vip_death() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("assassin", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(1, Message::RoomStart);

        let vip_by_team = server.rooms["room1"]
            .combatants
            .iter()
            .filter_map(|(player_id, combatant)| {
                combatant
                    .is_leader
                    .then_some((player_id.clone(), combatant.team))
            })
            .collect::<HashMap<_, _>>();
        assert_eq!(vip_by_team.len(), 2);
        let meteor_vip = vip_by_team
            .iter()
            .find_map(|(player_id, team)| (*team == Some(1)).then_some(player_id.clone()))
            .expect("Meteor VIP");
        assert!(vip_by_team.values().any(|team| *team == Some(2)));

        let snapshot = RoomServer::v4_snapshot_for(&server.rooms["room1"], "p1", 1, 0.0);
        assert!(snapshot
            .players
            .iter()
            .find(|player| player.id == meteor_vip)
            .is_some_and(|player| player.is_leader));

        let room = server.rooms.get_mut("room1").unwrap();
        room.combatants.get_mut(&meteor_vip).unwrap().health = 0.0;
        room.pending_round_end.replace(PendingRoundEnd {
            winner_team: Some(2),
            reason: "vip",
        });
        let ended = server.advance_authority(1.0 / 60.0, 0.0);
        assert!(ended.events.iter().any(|(_, message)| {
            matches!(message, Message::RoundEnd { winner_team, reason, .. }
                if *winner_team == Some(2) && reason == "vip")
        }));
    }

    #[test]
    fn assassin_cure_is_vip_only_authoritative_and_pose_rate_limited() {
        let mut server = RoomServer::new();
        for conn_id in 1..=3 {
            server.connect(conn_id);
            server.handle(conn_id, hello());
        }
        server.handle(1, create_room("assassin", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(3, join_room("room1"));
        server.handle(1, Message::RoomStart);

        let team_one = ["p1", "p3"];
        let healer_id = team_one
            .iter()
            .find(|player_id| {
                server.rooms["room1"]
                    .combatants
                    .get(**player_id)
                    .is_some_and(|combatant| combatant.is_leader)
            })
            .expect("one team-one VIP")
            .to_string();
        let target_id = team_one
            .iter()
            .find(|player_id| **player_id != healer_id.as_str())
            .expect("team-one ally")
            .to_string();
        {
            let room = server.rooms.get_mut("room1").unwrap();
            let healer = room.combatants.get_mut(&healer_id).unwrap();
            healer.position = RoleVec3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            };
            let target = room.combatants.get_mut(&target_id).unwrap();
            target.health = 0.0;
            target.position = RoleVec3 {
                x: 2.0,
                y: 2.0,
                z: 3.0,
            };
            target.respawn_at = None;
        }

        let healer_connection = healer_id[1..].parse::<u64>().unwrap();
        let mut cure = v4_input(1, Vec::new(), None);
        cure.actions = vec!["cure".into()];
        let accepted = server.handle_v4_input(healer_connection, cure);
        assert!(accepted.to_room.iter().any(|message| {
            matches!(message, Message::CombatCure { healer_id: event_healer, target_id: event_target, position }
                if event_healer == &healer_id && event_target == &target_id
                    && *position == Some([2.0, 2.0, 3.0]))
        }));
        assert!(accepted.to_room.iter().any(|message| {
            matches!(message, Message::CombatRespawn { player_id, reason, healer_id: Some(event_healer) }
                if player_id == &target_id && reason == "cure" && event_healer == &healer_id)
        }));
        assert_eq!(
            server.rooms["room1"].combatants[&target_id].health,
            server.rooms["room1"].combatants[&target_id].max_health
        );

        server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut(&target_id)
            .unwrap()
            .health = 0.0;
        let mut replay = v4_input(2, Vec::new(), None);
        replay.actions = vec!["cure".into()];
        let rejected = server.handle_v4_input(healer_connection, replay);
        assert!(rejected.to_room.is_empty());
        assert_eq!(server.rooms["room1"].combatants[&target_id].health, 0.0);
    }

    #[test]
    fn team_combat_permission_matches_web_friendly_fire_gate() {
        assert!(team_combat_allowed("leader", false, None, None));
        assert!(!team_combat_allowed("deathmatch", false, Some(1), Some(1)));
        assert!(team_combat_allowed("deathmatch", false, Some(1), Some(2)));
        assert!(team_combat_allowed("deathmatch", false, None, None));
        assert!(team_combat_allowed("deathmatch", true, Some(1), Some(1)));
    }

    #[test]
    fn team_select_enforces_web_lobby_capacity_and_start_gates() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(2, join_room("room1"));

        let no_teams = server.handle(2, Message::TeamSelect { team: 1 });
        assert!(matches!(
            no_teams.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "NO_TEAMS"
        ));

        server.handle(
            1,
            Message::RoomSettings {
                mode: "deathmatch".into(),
                stage_id: "sn01".into(),
                rules: RoomRulesWire {
                    max_players: Some(2),
                    ..Default::default()
                },
            },
        );
        let room = server.rooms.get("room1").unwrap();
        assert_eq!(
            room.members.get("p1").and_then(|member| member.team),
            Some(1)
        );
        assert_eq!(
            room.members.get("p2").and_then(|member| member.team),
            Some(2)
        );

        let full = server.handle(2, Message::TeamSelect { team: 1 });
        assert!(matches!(
            full.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "TEAM_FULL"
        ));
        let start_before_ready = server.handle(1, Message::RoomStart);
        assert!(matches!(
            start_before_ready.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "PLAYERS_NOT_READY"
        ));

        server.handle(2, Message::RoomReady { ready: true });
        server.handle(1, Message::RoomStart);
        let room = server.rooms.get("room1").unwrap();
        assert!(room.started);
        assert_eq!(
            room.combatants.get("p1").and_then(|state| state.team),
            Some(1)
        );
        assert_eq!(
            room.combatants.get("p2").and_then(|state| state.team),
            Some(2)
        );
    }

    #[test]
    fn ws_sn06_hell_death_is_authoritative_and_respawns_only_after_three_seconds() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut server = RoomServer::from_origin_res(&root).unwrap();
        server.connect(1);
        server.handle(1, hello());
        server.set_wire_format(1, WireFormat::V4);
        server.handle(1, create_room("leader", "sn06"));
        server.handle(1, Message::RoomStart);
        let volume = server.rooms["room1"]
            .hazards
            .volumes
            .iter()
            .find(|volume| volume.damage == 100000)
            .expect("sn06 original hell volume");
        let center = |axis| (volume.box_.min[axis] + volume.box_.max[axis]) * 0.5;
        let corpse = RoleVec3 {
            x: center(0),
            y: center(1),
            z: center(2),
        };
        let mut state = decode_request1_state(&[0; 20]).unwrap();
        state.position = corpse;
        server.handle_v4_input(1, v4_input(1, encode_request1_state(state).to_vec(), None));

        server.advance_authority(1.0 / 60.0, 0.0);
        assert!(server.rooms["room1"].combatants["p1"].alive());
        server
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p1")
            .unwrap()
            .target_hit_clock
            .advance(10.0);
        let mut deaths = 0;
        for _ in 0..20 {
            let dispatch = server.advance_authority(1.0 / 60.0, 0.0);
            deaths += dispatch.events.iter().filter(|(_, event)| matches!(event,
                Message::CombatDeath { attacker_id: None, target_id, respawn_seconds: Some(3.0), .. }
                if target_id == "p1")).count();
        }
        assert_eq!(deaths, 1);
        let room = server.rooms.get_mut("room1").unwrap();
        assert_eq!(room.combatants["p1"].health, 0.0);
        assert_eq!(room.combatants["p1"].kills, 0);
        assert_eq!(room.combatants["p1"].deaths, 1);
        let deadline = room.combatants["p1"].respawn_at.unwrap();
        room.elapsed = deadline;
        assert!(RoomServer::update_combat_respawns(room).is_empty());
        room.elapsed += 0.001;
        let events = RoomServer::update_combat_respawns(room);
        assert!(events.iter().any(|event| matches!(event,
            Message::CombatRespawn { player_id, reason, .. } if player_id == "p1" && reason == "timer")));
        assert!(room.combatants["p1"].alive());
        assert!(room.combatants["p1"].target_hit_clock.spawn_protected());

        let des = std::fs::read_to_string(root.join("sn06/sn06.des")).unwrap();
        let desc = meteor_asset::des::parse_stage_description(&des);
        let pad = desc
            .scene_objects
            .iter()
            .chain(&desc.dummies)
            .find(|object| object.name_attr.as_deref() == Some("teamA01"))
            .unwrap();
        let center = meteor_scene::stage_loader::get_stage_center("sn06", None);
        let scale = meteor_core::constants::MODEL_SCALE;
        state.position = RoleVec3 {
            x: (pad.position.x - center[0]) * scale,
            y: (pad.position.z - center[2]) * scale,
            z: -(pad.position.y - center[1]) * scale,
        };
        server.handle_v4_input(1, v4_input(2, encode_request1_state(state).to_vec(), None));
        for _ in 0..240 {
            server.advance_authority(1.0 / 60.0, 0.0);
        }
        assert!(server.rooms["room1"].combatants["p1"].alive());
        assert_eq!(server.rooms["room1"].combatants["p1"].deaths, 1);
        for mode in ["assassin", "deathmatch"] {
            let mut room = server.rooms["room1"].clone();
            room.mode = mode.into();
            let actor = room.combatants.get_mut("p1").unwrap();
            actor.position = corpse;
            actor.team = Some(1);
            actor.is_leader = true;
            actor.target_hit_clock.advance(10.0);
            let events = environment::update_hazards(&mut room, 0.2);
            assert!(events.iter().any(|event| matches!(
                event,
                Message::CombatDeath {
                    attacker_id: None,
                    respawn_seconds: None,
                    ..
                }
            )));
            assert!(room.combatants["p1"].respawn_at.is_none());
            room.elapsed += 4.0;
            assert!(RoomServer::update_combat_respawns(&mut room).is_empty());
            if mode == "assassin" {
                assert_eq!(room.pending_round_end.unwrap().winner_team, Some(2));
            }
        }
    }

    #[test]
    fn authority_respawns_leader_only_after_strict_three_second_delay() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.handle(1, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(1, Message::RoomStart);
        {
            let combatant = server
                .rooms
                .get_mut("room1")
                .and_then(|room| room.combatants.get_mut("p1"))
                .unwrap();
            combatant.health = 0.0;
            combatant.target_hit_clock.alive = false;
            combatant.anger = 75.0;
            combatant.guard = true;
            combatant.respawn_at = Some(RESPAWN_DELAY_SECONDS);
        }

        for _ in 0..180 {
            server.advance_authority(1.0 / 60.0, 0.0);
        }
        assert_eq!(
            server.rooms["room1"].combatants["p1"].health, 0.0,
            "the original comparison is strict > 3.0 seconds"
        );

        server.advance_authority(1.0 / 60.0, 0.0);
        let combatant = &server.rooms["room1"].combatants["p1"];
        assert_eq!(combatant.health, combatant.max_health);
        assert_eq!(combatant.anger, 0.0);
        assert!(!combatant.guard);
        assert!(combatant.target_hit_clock.spawn_protected());
    }

    #[test]
    fn v4_input_uses_independent_request1_watermark_and_join_ack() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.handle(1, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(1, Message::RoomStart);

        let mut stale = v4_input(9, vec![0; 20], None);
        stale.match_epoch = Some(0);
        server.handle_v4_input(1, stale);
        assert_eq!(
            server.rooms["room1"].input_authorities["p1"].last_input_sequence, 0,
            "a stale match generation must not advance input watermarks"
        );

        let first = server.handle_v4_input(1, v4_input(10, vec![0; 20], Some(21)));
        assert!(first.to_sender.is_empty());
        let room = server.rooms.get("room1").unwrap();
        let watermarks = room.input_authorities.get("p1").unwrap();
        assert_eq!(watermarks.last_input_sequence, 10);
        assert_eq!(watermarks.last_role_state_sequence, 10);
        assert_eq!(room.requested_role_rates.get("p1"), Some(&21));
        assert_eq!(
            room.role_states.get("p1").unwrap().position,
            Default::default()
        );

        server.handle_v4_input(1, v4_input(12, Vec::new(), None));
        server.handle_v4_input(1, v4_input(11, vec![0; 20], Some(19)));
        let room = server.rooms.get("room1").unwrap();
        let watermarks = room.input_authorities.get("p1").unwrap();
        assert_eq!(watermarks.last_input_sequence, 12);
        assert_eq!(watermarks.last_role_state_sequence, 11);
        assert_eq!(watermarks.last_role_rate_sequence, 11);
        assert_eq!(room.requested_role_rates.get("p1"), Some(&19));

        let mut state = Dispatch::default();
        server.push_room_state("room1", &mut state);
        assert!(state.to_conn.iter().any(|(conn_id, message)| {
            *conn_id == 1
                && matches!(message, Message::RoomJoined { last_processed_input_seq, .. }
                    if *last_processed_input_seq == 12)
        }));

        let malformed = server.handle_v4_input(1, v4_input(13, vec![0; 19], None));
        assert!(matches!(
            malformed.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "BAD_ROLE_STATE"
        ));
        let jump = server.handle_v4_input(1, v4_input(10_013, Vec::new(), None));
        assert!(matches!(
            jump.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "BAD_INPUT_SEQ"
        ));
        let room = server.rooms.get("room1").unwrap();
        assert_eq!(
            room.input_authorities
                .get("p1")
                .unwrap()
                .last_input_sequence,
            12,
            "validation failures must not advance watermarks"
        );
    }

    #[test]
    fn visual_foot_expires_independently_of_hurt_box_updates() {
        let mut target = Combatant::new(2000.0, None, base_weapon_slot(None, 6));
        let anchor = Vec3 {
            x: 1.0,
            y: 2.0,
            z: 0.0,
        };
        update_combatant_visual(&mut target, Some(&visual(262, 8.0, Some(anchor))), 10.0);
        assert_eq!(target.collision_updated_at, f32::NEG_INFINITY);
        target.position = RoleVec3 {
            x: 100.0,
            y: 0.0,
            z: 0.0,
        };
        target.advance(0.5, 10.5);
        assert_eq!(
            target.foot_position().x,
            1.0,
            "fresh visual remains valid at the boundary"
        );
        target.advance(0.01, 10.51);
        assert_eq!(
            target.foot_position(),
            target.position,
            "expired visual yields to request 1"
        );
        update_combatant_visual(
            &mut target,
            Some(&visual(
                262,
                8.0,
                Some(Vec3 {
                    x: 101.0,
                    y: 2.0,
                    z: 0.0,
                }),
            )),
            11.0,
        );
        target.advance(0.01, 11.01);
        assert_eq!(
            target.foot_position().x,
            101.0,
            "a new attack refreshes its own lifetime"
        );
        target.reset_geometry();
        assert_eq!(target.foot_position(), target.position);
        assert_eq!(target.world_position_updated_at, f32::NEG_INFINITY);
    }

    #[test]
    fn v4_combat_can_hit_either_player_after_their_attack_anchor_expires() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        for (attacker_conn, target_conn, target_id) in [(2, 1, "p1"), (1, 2, "p2")] {
            let mut server = RoomServer::from_origin_res(&root).unwrap();
            for conn in [1, 2] {
                server.connect(conn);
                server.handle(conn, hello());
            }
            server.handle(1, create_room("leader", "sn01"));
            server.handle(2, join_room("room1"));
            server.handle(1, Message::RoomStart);

            let mut old_attack = v4_input(1, role_state(RoleVec3::ZERO, 262), None);
            old_attack.visual = Some(visual(262, 8.0, Some(Vec3::default())));
            server.handle_v4_input(target_conn, old_attack);
            for _ in 0..121 {
                server.advance_authority(1.0 / 60.0, 0.0);
            }

            for conn in [attacker_conn, target_conn] {
                server.handle_v4_input(
                    conn,
                    v4_input(
                        2,
                        role_state(
                            RoleVec3 {
                                x: 100.0,
                                y: 0.0,
                                z: 0.0,
                            },
                            0,
                        ),
                        None,
                    ),
                );
            }
            let mut hit = v4_input(3, Vec::new(), None);
            hit.visual = Some(visual(
                262,
                8.0,
                Some(Vec3 {
                    x: 100.0,
                    y: 0.0,
                    z: 0.0,
                }),
            ));
            hit.combat = vec![CombatJudge {
                kind: "precise".into(),
                pose_id: 262,
                attack_index: 0,
                frame: 9770.0,
                cursor: 8.0,
                boxes: vec![identity_obb(100.0, 2.0, 0.0)],
                target_ids: vec![target_id.into()],
                ..Default::default()
            }];
            let dispatch = server.handle_v4_input(attacker_conn, hit);
            assert!(dispatch.to_room.iter().any(|message| matches!(message,
                Message::CombatHit { target, damage, .. } if target == target_id && *damage > 0.0
            )), "player {attacker_conn} must hit moved {target_id}: {:?}", dispatch.to_room);
            assert!(server.rooms["room1"].combatants[target_id].health < 2000.0);
        }
    }

    #[test]
    fn v4_combat_uses_original_pos_and_res_damage_result() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let mut server = RoomServer::from_origin_res(&root).unwrap();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(1, Message::RoomStart);

        let attacker = v4_input(1, role_state(RoleVec3::ZERO, 262), None);
        server.handle_v4_input(1, attacker);

        let mut target = v4_input(
            1,
            role_state(
                RoleVec3 {
                    x: 1.0,
                    y: 0.0,
                    z: 0.0,
                },
                0,
            ),
            None,
        );
        let mut target_visual = visual(
            0,
            0.0,
            Some(Vec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            }),
        );
        target_visual.carries_hurt_obbs = true;
        target_visual.hurt_obbs = vec![identity_obb(1.0, 2.0, 0.0)];
        target.visual = Some(target_visual);
        server.handle_v4_input(2, target);

        for _ in 0..121 {
            server.advance_authority(1.0 / 60.0, 0.0);
        }
        let mut target_refresh = v4_input(2, Vec::new(), None);
        let mut refreshed_visual = visual(
            0,
            0.0,
            Some(Vec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            }),
        );
        refreshed_visual.carries_hurt_obbs = true;
        refreshed_visual.hurt_obbs = vec![identity_obb(1.0, 2.0, 0.0)];
        target_refresh.visual = Some(refreshed_visual);
        server.handle_v4_input(2, target_refresh);

        let mut invalid = v4_input(2, Vec::new(), None);
        invalid.visual = Some(visual(262, 13.0, Some(Vec3::default())));
        invalid.combat = vec![CombatJudge {
            kind: "precise".into(),
            pose_id: 262,
            attack_index: 0,
            pose_instance_id: 1,
            frame: 9775.0,
            cursor: 13.0,
            boxes: vec![identity_obb(1.0, 2.0, 0.0)],
            ranged_kind: String::new(),
            start: None,
            end: None,
            target_ids: Vec::new(),
        }];
        assert!(server.handle_v4_input(1, invalid).to_room.is_empty());
        assert_eq!(server.rooms["room1"].combatants["p2"].health, 2000.0);

        server.handle_v4_input(
            1,
            v4_input(
                4,
                role_state(
                    RoleVec3 {
                        x: 0.25,
                        y: 0.0,
                        z: 0.0,
                    },
                    0,
                ),
                None,
            ),
        );

        let mut valid = v4_input(3, Vec::new(), None);
        valid.visual = Some(visual(262, 8.0, Some(Vec3::default())));
        valid.combat = vec![CombatJudge {
            kind: "precise".into(),
            pose_id: 262,
            attack_index: 0,
            pose_instance_id: 1,
            frame: 9770.0,
            cursor: 8.0,
            boxes: vec![identity_obb(1.0, 2.0, 0.0)],
            ranged_kind: String::new(),
            start: None,
            end: None,
            target_ids: Vec::new(),
        }];
        let expected_tick = server.rooms["room1"].authority_tick;
        let result = server.handle_v4_input(1, valid);
        assert!(
            matches!(
                result.to_room.as_slice(),
                [Message::CombatHit {
                    target,
                    damage,
                    attacker_id: Some(attacker_id),
                    guarded: Some(false),
                    health: Some(1976.0),
                    reaction: Some(reaction),
                    tick: Some(tick),
                    ..
                }] if target == "p2"
                    && *damage == 24.0
                    && attacker_id == "p1"
                    && reaction.target_move == 5.0
                    && reaction.target_value == 0.1
                    && reaction.target_pose == Some(93)
                    && *tick == expected_tick
            ),
            "unexpected combat dispatch: {:?}",
            result.to_room
        );

        assert_eq!(server.rooms["room1"].combatants["p2"].health, 1976.0);
        assert_eq!(
            server.rooms["room1"].input_authorities["p1"].last_input_sequence, 4,
            "the delayed claim must not rewind the envelope watermark"
        );
    }

    #[test]
    fn authority_clock_sends_recipient_specific_event2_snapshots_to_v4_peers() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(1, Message::RoomStart);
        server.set_wire_format(1, WireFormat::V4);
        server.set_wire_format(2, WireFormat::V4);
        server.handle_v4_input(1, v4_input(1, vec![0; 20], None));
        server.handle_v4_input(2, v4_input(1, vec![0; 20], None));

        let mut snapshots = Vec::new();
        let mut role_snapshots = Vec::new();
        for _ in 0..4 {
            let dispatch = server.advance_authority(1.0 / 60.0, 1234.0);
            snapshots.extend(dispatch.snapshots);
            role_snapshots.extend(dispatch.role_snapshots);
        }
        assert_eq!(
            snapshots.len(),
            2,
            "match bootstrap should emit one reconciliation snapshot per v6 peer"
        );
        assert_eq!(
            role_snapshots.len(),
            2,
            "16 Hz should emit one event-2 role snapshot per v6 peer"
        );
        let for_p1 = snapshots
            .iter()
            .find_map(|(conn_id, snapshot)| (*conn_id == 1).then_some(snapshot))
            .unwrap();
        let p1_self = for_p1
            .players
            .iter()
            .find(|player| player.id == "p1")
            .unwrap();
        let p2_remote = for_p1
            .players
            .iter()
            .find(|player| player.id == "p2")
            .unwrap();
        assert!(p1_self.role_state.is_empty());
        assert!(p1_self.position.is_some());
        assert_eq!(p2_remote.role_state.len(), 14);
        assert!(p2_remote.position.is_none());
        assert_eq!(for_p1.tick, 1);
        assert_eq!(for_p1.server_time, 1234.0);
        assert_eq!(for_p1.match_epoch, Some(1));
        assert_eq!(for_p1.round_epoch, Some(1));

        let role_for_p1 = role_snapshots
            .iter()
            .find_map(|(conn_id, snapshot)| (*conn_id == 1).then_some(snapshot))
            .unwrap();
        assert_eq!(role_for_p1.tick, 4);
        assert_eq!(role_for_p1.roles.len(), 1);
        assert_eq!(role_for_p1.roles[0].id, "p2");
        assert_eq!(role_for_p1.roles[0].role_state.len(), 14);
        assert_eq!(role_for_p1.match_epoch, Some(1));
        assert_eq!(role_for_p1.round_epoch, Some(1));
    }

    #[test]
    fn authority_clock_adapts_role_rate_and_emits_reliable_control() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.handle(1, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(1, Message::RoomStart);
        server.set_wire_format(1, WireFormat::V4);

        let mut controls = Vec::new();

        for _ in 0..244 {
            controls.extend(server.advance_authority(1.0 / 60.0, 1234.0).controls);
        }
        assert_eq!(server.rooms.get("room1").unwrap().role_rate, 17);
        assert!(controls.iter().any(|(conn_id, message)| {
            *conn_id == 1 && matches!(message, Message::RoleRate { rate } if *rate == 17)
        }));
    }

    #[test]
    fn join_empty_creates_or_reuses_room() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());

        let created = server.handle(1, join_empty());
        assert!(created
            .to_sender
            .iter()
            .any(|m| matches!(m, Message::RoomState { room_id, mode, stage, members, .. }
                if room_id == "room1" && mode == "deathmatch" && stage == "sn01" && members.len() == 1)));

        let joined = server.handle(2, join_empty());
        assert!(joined
            .to_sender
            .iter()
            .any(|m| matches!(m, Message::RoomState { room_id, members, .. }
                if room_id == "room1" && members.len() == 2)));

        let state = server
            .handle(2, join_empty())
            .to_sender
            .into_iter()
            .find_map(|m| match m {
                Message::RoomState { members, .. } => Some(members),
                _ => None,
            });
        let members = state.expect("room state with members");
        let p2 = members.iter().find(|m| m.player_id == "p2").unwrap();
        assert_eq!(p2.name, "测试对手");
        assert_eq!(p2.character_id, 3);
        assert_eq!(p2.weapon, 7);
        assert!(p2.ready);
        assert_eq!(members.iter().filter(|m| m.player_id == "p2").count(), 1);
    }

    #[test]
    fn room_latency_binds_session_identity_and_sanitizes_rtt() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.handle(1, hello());
        server.handle(1, join_room(""));
        for (value, expected) in [(42.0, Some(42.0)), (-1.0, None), (f64::NAN, None)] {
            let dispatch = server.handle(
                1,
                Message::RoomLatency {
                    room_id: "other-room".into(),
                    player_id: "victim".into(),
                    latency: crate::protocol::RoomLatencyWire {
                        server_rtt: Some(value),
                        host_rtt: Some(1.0),
                        p2p_state: "direct".into(),
                    },
                },
            );
            let Message::RoomLatency {
                room_id,
                player_id,
                latency,
            } = &dispatch.to_room[0]
            else {
                panic!("latency broadcast");
            };
            assert_eq!(room_id, "room1");
            assert_eq!(player_id, "p1");
            assert_eq!(latency.server_rtt, expected);
            assert_eq!(latency.host_rtt, None);
        }
        server.connect(2);
        server.handle(2, hello());
        assert!(server
            .handle(
                2,
                Message::RoomLatency {
                    room_id: "room1".into(),
                    player_id: "p1".into(),
                    latency: Default::default(),
                }
            )
            .to_room
            .is_empty());
    }

    #[test]
    fn chat_relays_to_room_others_with_name() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, join_room(""));
        server.handle(2, join_room(""));
        let chat = server.handle(
            1,
            Message::ChatSend {
                text: "来了来了".into(),
            },
        );

        assert_eq!(chat.to_others.len(), 1);
        assert!(matches!(
            &chat.to_others[0],
            Message::ChatMessage { player_id, name, text }
                if player_id.as_deref() == Some("p1") && name == "测试对手" && text == "来了来了"
        ));

        server.connect(3);
        server.handle(3, hello());
        let lonely = server.handle(
            3,
            Message::ChatSend {
                text: "solo".into(),
            },
        );
        assert!(lonely.to_others.is_empty());
    }

    #[test]
    fn combat_hit_relays_to_target_connection() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(
            1,
            Message::MatchStart {
                room_id: String::new(),
                match_epoch: None,
                round_epoch: None,
                mode: String::new(),
                stage: String::new(),
                players: vec![],
                slots: vec![],
                resumed: false,
                late_joined: false,
                authority: String::new(),
                host_id: String::new(),
            },
        );

        let hit = server.handle(
            1,
            Message::CombatHit {
                target: "p2".into(),
                damage: 12.5,
                attacker_id: None,
                guarded: None,
                health: None,
                anger: None,
                motion_scale: None,
                reaction: None,
                tick: None,
            },
        );
        assert_eq!(hit.to_conn.len(), 1);
        assert_eq!(hit.to_conn[0].0, 2);
        assert!(matches!(
            &hit.to_conn[0].1,
            Message::CombatHit { target, damage, .. } if target == "p2" && *damage == 12.5
        ));
        assert!(hit.to_sender.is_empty());

        let miss = server.handle(
            2,
            Message::CombatHit {
                target: "ghost".into(),
                damage: 5.0,
                attacker_id: None,
                guarded: None,
                health: None,
                anger: None,
                motion_scale: None,
                reaction: None,
                tick: None,
            },
        );
        assert!(miss.to_conn.is_empty());

        let mut fresh = RoomServer::new();
        fresh.connect(1);
        fresh.connect(2);
        fresh.handle(1, hello());
        fresh.handle(2, hello());
        fresh.handle(1, create_room("leader", "sn01"));
        fresh.handle(2, join_room("room1"));
        let early = fresh.handle(
            1,
            Message::CombatHit {
                target: "p2".into(),
                damage: 3.0,
                attacker_id: None,
                guarded: None,
                health: None,
                anger: None,
                motion_scale: None,
                reaction: None,
                tick: None,
            },
        );
        assert_eq!(early.to_conn.len(), 1);
        assert_eq!(early.to_conn[0].0, 2);

        server.set_wire_format(1, WireFormat::V4);
        let forbidden = server.handle(
            1,
            Message::CombatHit {
                target: "p2".into(),
                damage: 99.0,
                attacker_id: None,
                guarded: None,
                health: None,
                anger: None,
                motion_scale: None,
                reaction: None,
                tick: None,
            },
        );
        assert!(matches!(
            forbidden.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "V4_COMBAT_HIT_FORBIDDEN"
        ));
        assert!(forbidden.to_conn.is_empty());
    }

    #[test]
    fn disconnect_removes_member_and_authority_immediately() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(1, Message::RoomStart);
        server.handle_v4_input(2, v4_input(7, role_state(RoleVec3::ZERO, 0), None));
        let left = server.disconnect(2);
        assert!(matches!(&left[0], Message::RoomPlayerLeft { player_id } if player_id == "p2"));
        assert!(matches!(&left[1], Message::RoomState { members, .. } if members.len() == 1));
        let room = &server.rooms["room1"];
        assert!(!room.members.contains_key("p2"));
        assert!(!room.role_states.contains_key("p2"));
        assert!(!room.input_authorities.contains_key("p2"));
        assert!(!room.combatants.contains_key("p2"));
        assert!(server.disconnect(2).is_empty());
        server.disconnect(1);
        assert!(server.rooms.is_empty());
    }

    #[test]
    fn live_takeover_restores_identity_watermark_and_running_match_bootstrap() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        let welcome = server.handle(2, hello());
        let token = welcome
            .to_sender
            .iter()
            .find_map(|message| match message {
                Message::Welcome { session_token, .. } => Some(session_token.clone()),
                _ => None,
            })
            .unwrap();
        server.handle(1, create_room("leader", "sn01"));
        server.handle(2, join_room("room1"));
        server.handle(1, Message::RoomStart);
        server.set_wire_format(2, WireFormat::V4);
        server.handle_v4_input(2, v4_input(7, role_state(RoleVec3::ZERO, 0), None));
        assert_eq!(
            server.rooms["room1"].input_authorities["p2"].last_input_sequence,
            7
        );

        server.connect(3);
        server.set_wire_format(3, WireFormat::V4);
        let resumed = server.handle(
            3,
            Message::Hello {
                name: "reconnected".into(),
                resume_token: Some(token),
                character_id: 0,
                weapon: 6,
            },
        );
        assert!(resumed.to_sender.iter().any(|message| {
            matches!(message, Message::Welcome { player_id, resumed, .. }
                if player_id == "p2" && *resumed)
        }));
        assert!(resumed.to_sender.iter().any(|message| {
            matches!(message, Message::RoomJoined {
                player_id,
                resumed,
                last_processed_input_seq,
                ..
            } if player_id == "p2" && *resumed && *last_processed_input_seq == 7)
        }));
        assert!(resumed
            .to_sender
            .iter()
            .any(|message| { matches!(message, Message::MatchStart { resumed, .. } if *resumed) }));
        assert_eq!(server.connection_for_player("p2"), Some(3));
        assert_eq!(server.rooms["room1"].members.len(), 2);
        assert_eq!(
            server.rooms["room1"].input_authorities["p2"].last_input_sequence,
            7
        );

        let bootstrap = server.advance_authority(1.0 / 60.0, 1234.0);
        assert!(bootstrap.snapshots.iter().any(|(conn_id, _)| *conn_id == 3));
    }

    #[test]
    fn active_resume_token_takeover_supersedes_previous_connection() {
        let mut server = RoomServer::new();
        server.connect(1);
        let welcome = server.handle(1, hello());
        let token = welcome
            .to_sender
            .iter()
            .find_map(|message| match message {
                Message::Welcome { session_token, .. } => Some(session_token.clone()),
                _ => None,
            })
            .unwrap();
        server.connect(2);
        let takeover = server.handle(
            2,
            Message::Hello {
                name: "new endpoint".into(),
                resume_token: Some(token),
                character_id: 0,
                weapon: 6,
            },
        );
        assert_eq!(
            takeover.close_conn,
            vec![(1, 4000, "session resumed elsewhere".into())]
        );
        assert!(takeover.to_sender.iter().any(|message| {
            matches!(message, Message::Welcome { player_id, resumed, .. }
                if player_id == "p1" && *resumed)
        }));
        assert!(!server.sessions.contains_key(&1));
        assert_eq!(server.connection_for_player("p1"), Some(2));
    }

    #[test]
    fn leave_acknowledges_sender_and_notifies_previous_room_directly() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(2, join_room("room1"));
        let leave = server.handle(2, Message::RoomLeave);
        assert_eq!(leave.to_sender, vec![Message::RoomLeft]);
        assert!(leave.to_others.is_empty());
        assert!(leave.to_conn.iter().any(|(target, message)| {
            *target == 1
                && matches!(message, Message::RoomPlayerLeft { player_id } if player_id == "p2")
        }));
        assert!(leave.to_conn.iter().any(|(target, message)| {
            *target == 1
                && matches!(message, Message::RoomState { members, .. } if members.len() == 1)
        }));
    }

    #[test]
    fn changing_rooms_removes_old_member_before_joining_new_room() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(2, join_room("room1"));
        let created = server.handle(2, create_room("deathmatch", "sn02"));
        assert!(created.to_conn.iter().any(|(target, message)| {
            *target == 1
                && matches!(message, Message::RoomPlayerLeft { player_id } if player_id == "p2")
        }));
        assert!(created.to_conn.iter().any(|(target, message)| {
            *target == 1
                && matches!(message, Message::RoomState { room_id, members, .. }
                    if room_id == "room1" && members.len() == 1)
        }));
        let summaries = server.room_summaries();
        assert_eq!(summaries.len(), 2);
        assert_eq!(
            summaries
                .iter()
                .find(|room| room.room_id == "room1")
                .unwrap()
                .team_counts,
            Some([0, 0])
        );
        assert_eq!(
            summaries
                .iter()
                .find(|room| room.room_id == "room2")
                .unwrap()
                .team_counts,
            Some([1, 0])
        );
        assert_eq!(
            summaries
                .iter()
                .find(|room| room.room_id == "room1")
                .map(|room| room.player_count),
            Some(1)
        );
        assert_eq!(
            summaries
                .iter()
                .find(|room| room.room_id == "room2")
                .map(|room| room.player_count),
            Some(1)
        );
    }

    #[test]
    fn web_lobby_profile_settings_and_start_controls_update_room_state() {
        let mut server = RoomServer::new();
        server.connect(1);
        server.connect(2);
        server.handle(1, hello());
        server.handle(2, hello());
        server.handle(1, create_room("leader", "sn01"));
        server.handle(2, join_room("room1"));

        let profile = server.handle(
            2,
            Message::RoomProfile {
                character: "p3".into(),
                weapon: "blade".into(),
            },
        );
        assert!(profile.to_conn.iter().any(|(target, message)| {
            *target == 2
                && matches!(message, Message::RoomJoined { room, .. }
                    if room["players"][1]["character"] == "p3" && room["players"][1]["weapon"] == "blade")
        }));

        let settings = server.handle(
            1,
            Message::RoomSettings {
                mode: "deathmatch".into(),
                stage_id: "sn05".into(),
                rules: crate::protocol::RoomRulesWire {
                    max_players: Some(8),
                    ..Default::default()
                },
            },
        );
        assert!(settings.to_conn.iter().any(|(target, message)| {
            *target == 1
                && matches!(message, Message::RoomJoined { room, .. }
                    if room["mode"] == "deathmatch" && room["stageId"] == "sn05" && room["maxPlayers"] == 8)
        }));
        assert_eq!(server.room_summaries()[0].max_players, 8);

        let rejected = server.handle(
            2,
            Message::RoomSettings {
                mode: "leader".into(),
                stage_id: "sn01".into(),
                rules: Default::default(),
            },
        );
        assert!(matches!(
            rejected.to_sender.as_slice(),
            [Message::Error { code, .. }] if code == "host_only"
        ));

        server.handle(2, Message::RoomReady { ready: true });
        let start = server.handle(1, Message::RoomStart);
        assert!(start.to_room.iter().any(
            |message| matches!(message, Message::MatchStart { mode, stage, .. }
                if mode == "deathmatch" && stage == "sn05")
        ));
    }
}

#[path = "server_environment.rs"]
mod environment;

#[path = "server_scene_transfer.rs"]
mod scene_transfer;
