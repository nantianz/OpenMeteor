use bevy::prelude::*;
use meteor_core::{CharacterId, FighterState, MovementMode, PoseId};

pub use meteor_combat::state_machine::QueuedAttack;

#[derive(Component, Debug, Clone, Default)]
pub struct PoseEffectSpawn {
    pub pose_id: u16,

    pub pose_instance_id: u32,

    pub spawned: bool,
}

#[derive(Component, Debug, Clone)]
#[require(
    WornEffect,
    Concealment,
    GunAimState,
    RangedFireGate,
    WeaponForm,
    FighterHitReaction,
    PoseEffectSpawn,
    FighterStatusEffects
)]
pub struct Fighter {
    pub character_id: CharacterId,

    pub player_slot: u8,

    pub weapon: u8,

    pub weapon_slots: [u8; 2],

    pub weapon_slot: u8,

    pub weapon_slot_variants: [u8; 2],

    pub weapon_variant: u8,
}

#[derive(Component, Debug, Clone, Default)]
pub struct FighterStatusEffects {
    pub effects: meteor_combat::status_effects::HitStatusEffects,
    pub clock: f64,
    pub applied_max_health_bonus: i32,
    pub owns_concealment: bool,
    pub pending: meteor_combat::status_effects::StatusDelta,

    pub pending_periodic_hit: bool,
    pub worn: Vec<&'static str>,
}

impl FighterStatusEffects {
    pub fn clear(&mut self, worn: &mut WornEffect) {
        worn.effects
            .retain(|effect| !self.worn.contains(&effect.name.as_str()));
        *self = Self::default();
    }
    pub fn apply(&mut self, definition: meteor_asset::res::HitStatusEffect) {
        let delta = self.effects.apply(definition, self.clock);
        self.pending.health += delta.health;
        self.pending.anger += delta.anger;
    }
}

#[derive(Component, Debug, Clone, Default)]
pub struct FighterHitReaction {
    pub current: Option<meteor_combat::hit_reaction::HitReact>,
    pub authored_minimum: f32,
    pub root_expected_pose_id: Option<u16>,
    pub root_pose_id: Option<u16>,
    pub root_prev: Option<[f32; 3]>,
    pub root_started: bool,
    pub vertical_velocity: f32,
    pub airborne: bool,
    pub ground_y: Option<f32>,

    pub carry_x: f32,
    pub carry_z: f32,
}

impl FighterHitReaction {
    pub fn begin(&mut self, reaction: meteor_combat::hit_reaction::HitReact, airborne: bool) {
        self.root_expected_pose_id = reaction.pose_id;
        self.root_pose_id = None;
        self.root_prev = None;
        self.root_started = false;
        self.vertical_velocity = 0.0;
        self.airborne = airborne;
        self.ground_y = None;
        self.carry_x = 0.0;
        self.carry_z = 0.0;
        self.authored_minimum = reaction.authored_minimum.max(0.0);
        self.current = Some(reaction);
    }

    pub fn bank_be_hit_carry(&mut self, composed: Vec3, locally_driven: bool) {
        if locally_driven {
            self.carry_x = if composed.x.is_finite() {
                composed.x
            } else {
                0.0
            };
            self.carry_z = if composed.z.is_finite() {
                composed.z
            } else {
                0.0
            };
        } else {
            self.carry_x = 0.0;
            self.carry_z = 0.0;
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Component, Debug, Clone, Default)]
pub struct WeaponVariantChanged;

#[derive(Component, Debug, Clone, Copy, Default)]
pub struct CharacterVariantChanged {
    pub requested_character_id: u8,
}

#[derive(Component, Debug, Clone, Copy, Default)]
pub struct WeaponForm {
    pub current: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WornEffectInstance {
    pub name: String,

    pub remaining: f32,
}

#[derive(Component, Debug, Clone, Default)]
pub struct WornEffect {
    pub effects: Vec<WornEffectInstance>,
}

impl WornEffect {
    pub fn refresh(&mut self, name: &str, remaining: f32) {
        let remaining = remaining.max(0.0);
        if let Some(effect) = self
            .effects
            .iter_mut()
            .find(|effect| effect.name.eq_ignore_ascii_case(name))
        {
            effect.name = name.to_string();
            effect.remaining = remaining;
        } else if !name.is_empty() && remaining > 0.0 {
            self.effects.push(WornEffectInstance {
                name: name.to_string(),
                remaining,
            });
        }
    }

    pub fn clear(&mut self) {
        self.effects.clear();
    }
}

#[derive(Component, Debug, Clone, Copy, Default)]
pub struct Concealment {
    pub remaining: f32,
}

#[derive(Component, Debug, Clone, Copy, Default)]
pub struct GunAimState {
    pub aiming: bool,
}

#[derive(Component, Debug, Clone, Copy, Default)]
pub struct RangedFireGate {
    pub equipped_weapon: Option<(u8, u8, u8)>,
    pub entity_pose_instance: Option<u32>,
    pub ray_pose_instance: Option<u32>,
    pub wheel_out: bool,
}

#[derive(Component, Debug, Clone, Copy, Default)]
pub struct EscortFlagCarrier;

#[derive(Component, Debug, Clone, Default)]
pub struct WeaponSwitchActive {
    pub to_weapon: u8,
    pub to_slot: u8,
    pub to_variant: u8,

    pub empty_target: bool,

    pub pose_id: u16,
    pub pose_instance_id: u32,

    pub marker_frame: u32,
    pub end_frame: u32,

    pub committed: bool,

    pub end_sampled: bool,

    pub airborne: bool,

    pub carry_x: f32,
    pub carry_z: f32,

    pub root_started: bool,
    pub prev_root_x: f32,
    pub prev_root_y: f32,
}

#[derive(Component, Debug, Clone, Default)]
pub struct HeadTrackState {
    pub current: f32,
}

#[derive(Component, Debug, Clone)]
pub struct WeaponModel {
    pub fighter: Entity,

    pub weapon: u8,
    pub slot: u8,
    pub variant: u8,

    pub forms: Option<Vec<u8>>,
}

#[derive(Component, Debug, Clone)]
pub struct WeaponCollisionGeometry {
    pub fighter: Entity,
    pub weapon: u8,
    pub slot: u8,
    pub variant: u8,

    pub part: String,

    pub left: bool,
    pub boxes: Vec<meteor_asset::weapon_collision::WeaponCollisionBox>,
    pub forms: Option<Vec<u8>>,
}

#[derive(Component, Debug, Clone)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Health {
    pub fn new() -> Self {
        Self {
            current: meteor_core::DEFAULT_HP,
            max: meteor_core::DEFAULT_HP,
        }
    }
}

impl Default for Health {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Component, Debug, Clone)]
pub struct TargetHitClock {
    pub elapsed: f32,

    pub spawn_elapsed: f32,
}

pub const TARGET_HIT_INTERVAL_SECONDS: f32 = 0.2;

pub const SPAWN_INVULNERABLE_SECONDS: f32 = 2.0;

impl TargetHitClock {
    pub fn tick(&mut self, dt: f32) {
        self.tick_target(dt);
        self.tick_spawn(dt);
    }

    pub fn tick_target(&mut self, dt: f32) {
        if self.elapsed.is_finite() {
            self.elapsed += dt;
        }
    }

    pub fn tick_spawn(&mut self, dt: f32) {
        self.spawn_elapsed += dt;
    }

    pub fn accepts_target_hit(&self) -> bool {
        self.elapsed > TARGET_HIT_INTERVAL_SECONDS
    }

    pub fn spawn_protected(&self) -> bool {
        self.spawn_elapsed < SPAWN_INVULNERABLE_SECONDS
    }

    pub fn reset(&mut self) {
        self.elapsed = 0.0;
    }

    pub fn reset_spawn(&mut self) {
        self.spawn_elapsed = 0.0;
    }
}

impl Default for TargetHitClock {
    fn default() -> Self {
        Self {
            elapsed: f32::INFINITY,
            spawn_elapsed: 0.0,
        }
    }
}

#[derive(Component, Debug, Clone)]
pub struct Qi {
    pub current: f32,
    pub max: f32,
}

impl Qi {
    pub fn new() -> Self {
        Self {
            current: 0.0,
            max: meteor_core::DEFAULT_QI_MAX,
        }
    }
}

impl Default for Qi {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Component, Debug, Clone)]
pub struct CombatState {
    pub state: FighterState,
}

impl Default for CombatState {
    fn default() -> Self {
        Self {
            state: FighterState::Name,
        }
    }
}

#[derive(Component, Debug, Clone, Default)]
pub struct ComboState {
    pub current_pose: PoseId,

    pub pose_timer: u32,

    pub buffer: Option<ComboInput>,
}

#[derive(Debug, Clone)]
pub struct ComboInput {
    pub direction: Direction,
    pub action: ComboAction,
    pub air: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Neutral,
    Forward,
    Back,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComboAction {
    Attack,
    Guard,
    Jump,
    Dodge,
}

#[derive(Component, Debug, Clone)]
pub struct LockTarget {
    pub target: Option<Entity>,
}

#[derive(Component, Debug, Clone)]
pub struct MotionState {
    pub movement_mode: MovementMode,
    pub velocity: Vec3,

    pub facing: f32,
}

#[derive(Component, Debug, Clone)]
pub struct PoseRuntime {
    pub pose_id: PoseId,

    pub frame: f32,
    pub start: u32,
    pub end: u32,

    pub loop_start: u32,
    pub loop_end: u32,

    pub loops: bool,

    pub pose_instance_id: u32,
}

impl PoseRuntime {
    pub fn idle() -> Self {
        Self {
            pose_id: 0,
            frame: 0.0,
            start: 0,
            end: 1,
            loop_start: 0,
            loop_end: 1,
            loops: true,
            pose_instance_id: 0,
        }
    }

    pub fn single_frame_loop_guard(&self) -> bool {
        self.loops && self.loop_start == self.loop_end && self.end > self.loop_start
    }

    pub fn is_fall_hold(&self) -> bool {
        self.loops
            && self.loop_start == self.loop_end
            && self.loop_start == self.start
            && self.end > self.start
    }

    pub fn plays_to_end(&self) -> bool {
        !self.loops || self.single_frame_loop_guard()
    }

    pub fn finished(&self) -> bool {
        self.plays_to_end() && self.frame >= self.end as f32
    }
}

#[derive(Component, Debug, Clone, Default)]
pub struct EntryBlend {
    pub pending_local_chain: Option<(u32, u16, f32)>,

    pub from_root: [f32; 3],

    pub from_quats: Vec<[f32; 4]>,

    pub from_dummy_pos: Vec<[f32; 3]>,

    pub from_dummy_quats: Vec<[f32; 4]>,

    pub duration: f32,

    pub elapsed: f32,

    pub started_at_seconds: Option<f64>,

    pub to_frame: f32,

    pub pose_id: u16,

    pub kind: EntryBlendKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntryBlendKind {
    #[default]
    Generic,

    Chained,

    Network,
}

impl Default for PoseRuntime {
    fn default() -> Self {
        Self::idle()
    }
}

#[derive(Component, Debug, Clone, Default)]
pub struct VisualRootPin {
    pub active: bool,
    pub initialized: bool,
    pub release_after_sample: bool,
    pub pose_id: u16,
    pub baseline: [f32; 3],
    pub lock_xy: bool,
    pub lock_z: bool,
}

#[derive(Component, Debug, Clone)]
pub struct SmState {
    pub current: FighterState,
    pub previous: FighterState,
    pub time_in_state: f32,
    pub locked: bool,
    pub attack_stage: u8,
    pub attack_queued: Option<QueuedAttack>,
    pub linked_recovery_pose_id: Option<PoseId>,

    pub exe_ground_mode_ticks: u8,
}

impl Default for SmState {
    fn default() -> Self {
        Self {
            current: FighterState::Name,
            previous: FighterState::Name,
            time_in_state: 0.0,
            locked: false,
            attack_stage: 0,
            attack_queued: None,
            linked_recovery_pose_id: None,
            exe_ground_mode_ticks: 0,
        }
    }
}

#[derive(Component, Debug, Clone, Default)]
pub struct DeathRespawn {
    pub active: bool,
    pub timer: f32,

    pub spawn_pos: Vec3,
}

#[derive(Resource, Default)]
pub struct LockOnController {
    pub system: meteor_combat::lock_on::LockOnSystem,

    pub lock_entity: Option<Entity>,

    pub track_entity: Option<Entity>,
}

#[derive(Resource, Debug, Clone, Default)]
pub struct MatchEnd {
    pub result: i32,

    pub settled: bool,

    pub elapsed: f32,

    pub settle_elapsed: f32,
}

#[derive(Resource, Debug, Clone, Default)]
pub struct KillScore {
    pub player_kills: u32,

    pub npc_kills: u32,
}

#[derive(Component, Debug, Clone, Default)]
pub struct AiIntent {
    pub want_taunt: bool,

    pub navigation_jump_target: Option<Vec3>,

    pub want_attack: bool,

    pub attack_directions: Vec<meteor_combat::combo_engine::Dir>,

    pub want_qi_burst: bool,

    pub enemy_entity: Option<Entity>,

    pub lock_target: Option<Entity>,

    pub ranged_aim: Option<Vec3>,

    pub want_guard: bool,

    pub dodge_burst: Option<bool>,

    pub dodge_dir: Vec2,

    pub attack_cooldown: f32,

    pub move_dir: Vec2,

    pub locomotion_input: Vec2,

    pub enemy_id: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fighter_always_installs_required_runtime_components() {
        let mut world = World::new();
        let entity = world
            .spawn(Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 6,
                weapon_slots: [6, 5],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            })
            .id();
        assert!(world.get::<WornEffect>(entity).is_some());
        assert!(world.get::<Concealment>(entity).is_some());
        assert!(world.get::<GunAimState>(entity).is_some());
        assert!(world.get::<RangedFireGate>(entity).is_some());
        assert!(world.get::<FighterHitReaction>(entity).is_some());
    }
}
