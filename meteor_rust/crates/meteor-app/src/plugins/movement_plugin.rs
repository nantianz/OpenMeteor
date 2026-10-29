use crate::components::fighter::{
    CombatState, EntryBlend, EntryBlendKind, Fighter, FighterHitReaction, GunAimState,
    LockOnController, MotionState, PoseRuntime, RangedFireGate, SmState, WeaponSwitchActive,
};
use crate::plugins::camera_plugin::CameraBasis;
use crate::plugins::character_contact_plugin::{
    commit_fighter_position, resolve_fighter_xz, sync_fighter_contact_state, CharacterContactFrame,
};
use crate::plugins::character_plugin::{FighterDummy, FighterModel};
use crate::plugins::input_plugin::{
    DesktopDodgeInput, LocalCrouchInput, LocalGuardInput, LocalJumpInput,
};
use crate::plugins::network_plugin::RemoteFighter;
use crate::plugins::stage_collider::StageCollider;
use crate::plugins::stage_plugin::{LocalFighter, NpcPerformance};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use meteor_combat::state_machine::{can_interrupt, Action};
use meteor_combat::weapon_strategy::WEAPON_SWITCH_POSE_AIR;
use meteor_core::{FighterState, MovementMode, MODEL_SCALE};
use meteor_input::command_parser::Dir;
use meteor_physics::carry_velocity::{
    clamp_composed_delta_3d, clamp_composed_velocity_3d, CARRY_MAX_SPEED as MOVER_MAX_SPEED,
};
use meteor_physics::collision::{CollisionSystem, FootHit, GroundHit, Point3, WALKABLE_SURFACE_NY};
use meteor_physics::dodge::DodgeState;
use meteor_physics::jump::{self, AirMotionMode, AirState, JumpStartInput};
use meteor_physics::locomotion::{
    compose_loco_axes, dodge_pose_for_input as mapped_dodge_pose_for_input, pose as L,
};
use meteor_physics::planar_drive;
use meteor_physics::wall_run;

pub(crate) const COLLIDER_RADIUS: f32 = 0.55;
pub(crate) const COLLIDER_HEIGHT: f32 = 1.8;

const STRONG_JUMP_HOLD: f32 = 0.11;

const CARRY_DECAY_GROUND: f32 = 9.0;

const CARRY_MAX_SPEED: f32 = 72.0;

const FORWARD_STRAFE_SCALE: f32 = 0.2;

const WALL_CLING_DELAY: f32 = 0.1;
const STEEP_SUPPORT_SNAP: f32 = 2.5;
const CARRY_DECAY_STEEP: f32 = 0.2;

#[derive(Component, Default)]
pub struct FighterAir {
    pub air: AirState,

    pub ground_contact: bool,

    pub(crate) mover_vertical_contact: bool,

    pub(crate) air_attack_recovery_pose: Option<u16>,

    pub(crate) attack_ground_pose_pending: bool,

    pub(crate) recovery_ground_mode: Option<bool>,

    pub(crate) locomotion_pose_sample: Option<(bool, f32)>,

    pub jump_hold: f32,

    pub jump_charge_active: bool,

    pub ledge_fall: bool,

    pub pending_ledge_fall: bool,

    pub ground_external_velocity: Vec3,

    pub carry_decay_steep: bool,

    pub carry_no_support: bool,

    pub ground_entry_blend_pending: bool,

    pub motion_root_pin: Option<[f32; 3]>,

    pub peak_lift: f32,

    pub ground_y: f32,

    pub motion_root_pin_pose_id: u16,

    pub(crate) late_visual_pose_snap: bool,

    pub(crate) jump_root_xy_started: bool,
    pub(crate) jump_prev_pose_id: u16,
    pub(crate) jump_prev_root_x: f32,
    pub(crate) jump_prev_root_y: f32,
    pub(crate) stand_motion_root_x: f32,
    pub(crate) stand_motion_root_y: f32,
    pub(crate) pin_root_z: f32,

    pub(crate) landed_planar: Vec2,
}

#[derive(Component, Default)]
pub struct FighterDodge {
    pub state: DodgeState,

    pub pose: u16,

    pub pose_entered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JumpChargeRequest {
    None,
    Released,
    HeldThreshold,
    BufferedGroundMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AirVerticalPath {
    AuthoredCurve,
    LaunchChargeHold,
    WeaponSwitchGravity,
    Ballistic,
}

fn select_air_vertical_path(
    anim_driven: bool,
    launching: bool,
    launch_lifted: bool,
    switching_weapon_air: bool,
) -> AirVerticalPath {
    if anim_driven {
        AirVerticalPath::AuthoredCurve
    } else if launching && !launch_lifted {
        AirVerticalPath::LaunchChargeHold
    } else if switching_weapon_air {
        AirVerticalPath::WeaponSwitchGravity
    } else {
        AirVerticalPath::Ballistic
    }
}

pub(crate) fn clamp_air_motion_step(
    planar_x: f32,
    vertical_speed: f32,
    planar_z: f32,
    vertical_displacement: f32,
) -> (f32, f32, f32, f32) {
    let (vx, vy, vz, scale) =
        clamp_composed_velocity_3d(planar_x, vertical_speed, planar_z, MOVER_MAX_SPEED);
    (vx, vy, vz, vertical_displacement * scale)
}

pub(crate) fn accepted_motion_velocity(dx: f32, dy: f32, dz: f32, dt: f32) -> Vec3 {
    if dt > 0.0 && dt.is_finite() {
        Vec3::new(dx / dt, dy / dt, dz / dt)
    } else {
        Vec3::ZERO
    }
}

pub(crate) fn sample_air_ground<M: meteor_physics::collision::MeshCollider>(
    air: &mut FighterAir,
    mover: &CollisionSystem<M>,
    pos: Point3,
) -> Option<GroundHit> {
    let sampled = mover.sample_ground_hit(pos.x, pos.z, pos.y)?;
    if sampled.y > pos.y {
        Some(GroundHit {
            y: air.ground_y,
            nx: 0.0,
            ny: 1.0,
            nz: 0.0,
        })
    } else {
        air.ground_y = sampled.y;
        Some(sampled)
    }
}

pub(crate) fn update_ground_external_motion<M: meteor_physics::collision::MeshCollider>(
    air: &mut FighterAir,
    mover: &CollisionSystem<M>,
    pos: Point3,
    dt: f32,
) -> Vec3 {
    let hit = mover
        .mesh
        .as_ref()
        .and_then(|mesh| mesh.sample_ground_hit(pos.x, pos.z, mover.ground_y, pos.y));
    update_ground_external_from_hit(air, hit, pos.y, dt)
}

fn update_ground_external_from_hit(
    air: &mut FighterAir,
    hit: Option<GroundHit>,
    current_y: f32,
    dt: f32,
) -> Vec3 {
    let hit = hit.filter(|hit| hit.ny > 0.0 && (hit.y - current_y).abs() <= STEEP_SUPPORT_SNAP);
    if let Some(hit) = hit {
        air.carry_no_support = false;
        air.carry_decay_steep = hit.ny < WALKABLE_SURFACE_NY;
        if air.carry_decay_steep {
            let normal = Vec3::new(hit.nx, hit.ny, hit.nz).normalize_or_zero();
            let gravity = -jump::GRAVITY;
            air.ground_external_velocity.x += gravity * normal.y * normal.x * dt.max(0.0);
            air.ground_external_velocity.z += gravity * normal.y * normal.z * dt.max(0.0);
            air.ground_external_velocity.y = 0.0;
            return air.ground_external_velocity;
        }
    }
    air.ground_external_velocity = Vec3::ZERO;
    air.ground_external_velocity
}

pub(crate) fn update_descent_slide(
    air: &mut FighterAir,
    foot: Option<FootHit>,
    vertical_speed: f32,
    dt: f32,
) {
    let slide = meteor_physics::collision::descent_slide_velocity(foot, vertical_speed, dt);
    air.ground_external_velocity = Vec3::new(slide.x, 0.0, slide.z);
    if let Some(hit) = foot {
        air.carry_no_support = false;
        air.carry_decay_steep = hit.ny < WALKABLE_SURFACE_NY;
    }
}

pub(crate) fn consume_descent_slide(air: &mut FighterAir) {
    air.air.planar_x += air.ground_external_velocity.x;
    air.air.planar_z += air.ground_external_velocity.z;
    air.ground_external_velocity = Vec3::ZERO;
}

pub(crate) fn record_carry_support(
    air: &mut FighterAir,
    vertical_request: f32,
    surface: Option<(f32, f32, f32)>,
    planar_contact: bool,
) {
    if vertical_request == 0.0 {
        return;
    }
    if let Some((_, ny, _)) = surface {
        air.carry_no_support = false;
        air.carry_decay_steep = ny < WALKABLE_SURFACE_NY;
    } else if !planar_contact {
        air.carry_no_support = true;
    }
}

pub(crate) fn action_carry_decay(air: &FighterAir) -> f32 {
    if air.carry_decay_steep || air.carry_no_support {
        CARRY_DECAY_STEEP
    } else {
        CARRY_DECAY_GROUND
    }
}

#[allow(clippy::too_many_arguments)]
fn update_jump_charge(
    hold_seconds: &mut f32,
    active: &mut bool,
    ground_mode_ticks: &mut u8,
    pressed_edge: bool,
    held: bool,
    released_edge: bool,
    can_begin_charge: bool,
    can_pay_buffered_jump: bool,
    observed_hold_seconds: Option<f32>,
    wall_dt: f32,
) -> JumpChargeRequest {
    if pressed_edge {
        *hold_seconds = 0.0;
        *active = can_begin_charge;
    }

    if *active {
        if let Some(observed) = observed_hold_seconds {
            *hold_seconds = observed.max(0.0);
        } else if held || pressed_edge {
            *hold_seconds += wall_dt.max(0.0);
        }
    }

    let had_ground_mode_window = *ground_mode_ticks > 0;
    *ground_mode_ticks = ground_mode_ticks.saturating_sub(1);

    if released_edge {
        let request = if *active {
            JumpChargeRequest::Released
        } else {
            JumpChargeRequest::None
        };
        *active = false;
        *ground_mode_ticks = 0;
        return request;
    }

    if had_ground_mode_window && *active && held {
        if *hold_seconds >= STRONG_JUMP_HOLD {
            *active = false;
            *ground_mode_ticks = 0;
            return if can_pay_buffered_jump {
                JumpChargeRequest::BufferedGroundMode
            } else {
                JumpChargeRequest::None
            };
        }

        *ground_mode_ticks = 1;
    }

    JumpChargeRequest::None
}

fn jump_charge_key_down_allowed(state: FighterState, ordinary_locomotion: bool) -> bool {
    ordinary_locomotion
        || matches!(
            state,
            FighterState::Guard | FighterState::Fake | FighterState::Down
        )
}

fn jump_release_reaches_motion(
    ordinary_locomotion: bool,
    action_accepted: bool,
    weapon_action_vetoed: bool,
    request: JumpChargeRequest,
) -> bool {
    (ordinary_locomotion || action_accepted)
        && !weapon_action_vetoed
        && matches!(
            request,
            JumpChargeRequest::Released | JumpChargeRequest::HeldThreshold
        )
}

fn preserve_post_jump_action(action_accepted: bool, state: FighterState) -> bool {
    action_accepted && state != FighterState::Jump
}

#[derive(Component, Default)]
pub struct FighterWall {
    pub cling_active: bool,

    pub cling_pose: u16,

    pub kick_pose: u16,

    pub wall_normal: Vec3,

    pub contact_seen: bool,

    pub remaining: f32,

    pub external_accel: Vec3,
}

#[derive(Component, Default)]
pub struct FighterAttackMotion {
    pub active: bool,

    pub prev_pose_id: u16,

    pub prev_pose_instance_id: u32,

    pub prev_group: Option<u16>,

    pub prev_root_x: f32,
    pub prev_root_y: f32,

    pub hold_delta: bool,

    pub stopping: bool,

    pub carry_x: f32,
    pub carry_z: f32,

    pub carry_banked: bool,

    pub(crate) pending_air_dx: f32,
    pub(crate) pending_air_dz: f32,
    pub(crate) air_step_pending: bool,
}

pub fn key_to_dir(key: &KeyCode, keymap: &crate::app_state::Keymap) -> Option<Dir> {
    if keymap.move_up.as_key() == Some(*key) {
        Some(Dir::Up)
    } else if keymap.move_down.as_key() == Some(*key) {
        Some(Dir::Down)
    } else if keymap.move_left.as_key() == Some(*key) {
        Some(Dir::Left)
    } else if keymap.move_right.as_key() == Some(*key) {
        Some(Dir::Right)
    } else {
        None
    }
}

pub(crate) fn directional_input(
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    keymap: &crate::app_state::Keymap,
) -> Vec3 {
    let mut input = Vec3::ZERO;
    if keymap.move_up.pressed(keys, mouse) {
        input.z += 1.0;
    }
    if keymap.move_down.pressed(keys, mouse) {
        input.z -= 1.0;
    }
    if keymap.move_left.pressed(keys, mouse) {
        input.x -= 1.0;
    }
    if keymap.move_right.pressed(keys, mouse) {
        input.x += 1.0;
    }
    input
}

fn is_running(
    loco_running: bool,
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    keymap: &crate::app_state::Keymap,
) -> bool {
    loco_running || keymap.run.pressed(keys, mouse)
}

fn world_direction(input: Vec3, yaw: f32) -> Vec3 {
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let forward = Vec3::new(sin_yaw, 0.0, cos_yaw);
    let right = Vec3::new(-cos_yaw, 0.0, sin_yaw);
    forward * input.z + right * input.x
}

pub(crate) fn tick_air_input_drive(
    air: &mut FighterAir,
    dt: f32,
    input: Vec3,
    facing: f32,
    crouching: bool,
    speed_factor: f32,
    action_vetoed: bool,
) {
    let axes = compose_loco_axes(
        input.x,
        input.z,
        if crouching { 1.0 } else { FORWARD_STRAFE_SCALE },
    );
    let direction = world_direction(Vec3::new(axes.local_x, 0.0, axes.local_z), facing);
    air.air.tick_planar(
        dt,
        direction.x,
        direction.z,
        jump::AirInputDrive {
            speed_factor,
            forward: input.z > 0.0 && !crouching,
            ground_contact: air.ground_contact && !air.carry_decay_steep,
            action_vetoed,
        },
    );
}

fn ordinary_mover_support(
    previous: bool,
    planar_contact: bool,
    surface: Option<(f32, f32, f32)>,
    blocked: bool,
) -> bool {
    if let Some((_, ny, _)) = surface {
        ny >= WALKABLE_SURFACE_NY
    } else if blocked {
        true
    } else {
        planar_contact && previous
    }
}

pub(crate) fn project_planar_air_velocity_on_contact(
    air: &mut AirState,
    requested_x: f32,
    requested_z: f32,
    accepted_x: f32,
    accepted_z: f32,
) -> bool {
    let blocked_x = requested_x - accepted_x;
    let blocked_z = requested_z - accepted_z;
    let blocked = blocked_x.hypot(blocked_z);

    if blocked <= (requested_x.hypot(requested_z) * 1e-3).max(1e-6) {
        return false;
    }
    let normal_x = blocked_x / blocked;
    let normal_z = blocked_z / blocked;
    let into = air.planar_x * normal_x + air.planar_z * normal_z;
    if into > 0.0 {
        air.planar_x -= normal_x * into;
        air.planar_z -= normal_z * into;
    }
    true
}

pub(crate) fn project_air_velocity_on_surface(air: &mut AirState, nx: f32, ny: f32, nz: f32) {
    let len = (nx * nx + ny * ny + nz * nz).sqrt();
    if len < 1e-6 {
        air.vertical_speed = air.vertical_speed.min(0.0);
        return;
    }
    let nx = nx / len;
    let ny = ny / len;
    let nz = nz / len;
    let into = air.planar_x * nx + air.vertical_speed * ny + air.planar_z * nz;
    if into < 0.0 {
        air.planar_x -= nx * into;
        air.vertical_speed -= ny * into;
        air.planar_z -= nz * into;
    }
    if ny >= WALKABLE_SURFACE_NY {
        air.vertical_speed = air.vertical_speed.min(0.0);
    }
}

fn can_turn(state: &SmState) -> bool {
    !matches!(
        state.current,
        FighterState::Attack1
            | FighterState::Attack2
            | FighterState::Attack3
            | FighterState::Guard
            | FighterState::Burst
            | FighterState::Cure
            | FighterState::BeHit
            | FighterState::Down
            | FighterState::Fake
    )
}

#[derive(Debug, Clone, Copy)]
struct WallProbe {
    normal: Vec3,
}

fn probe_wall<M: meteor_physics::collision::MeshCollider>(
    sys: &CollisionSystem<M>,
    pos: Point3,
    dir: Vec3,
    max_dist: f32,
) -> Option<WallProbe> {
    let direction = dir.normalize_or_zero();
    if direction.length_squared() < 1e-12 {
        return None;
    }
    let hit = sys.segment_hit(
        pos,
        Point3::new(
            pos.x + direction.x * max_dist,
            pos.y + direction.y * max_dist,
            pos.z + direction.z * max_dist,
        ),
    )?;
    let normal = Vec3::new(hit.nx, hit.ny, hit.nz);
    wall_run::wall_normal_climbable(wall_run::Vec3::new(hit.nx, hit.ny, hit.nz))
        .then_some(WallProbe { normal })
}

fn try_enter_wall_cling<M: meteor_physics::collision::MeshCollider>(
    sys: &CollisionSystem<M>,
    pos: Point3,
    facing: Vec3,
    planar_v: Vec3,
    input_x: f32,
) -> Option<(u16, WallProbe)> {
    let front = wall_run::wall_cling_probe_direction(
        wall_run::Vec3::new(facing.x, 0.0, facing.z),
        wall_run::Vec3::new(planar_v.x, 0.0, planar_v.z),
    );
    let probes = [
        Vec3::new(front.x, front.y, front.z),
        Vec3::new(-facing.z * input_x, 0.0, facing.x * input_x),
    ];
    for direction in probes {
        if let Some(contact) = probe_wall(sys, pos, direction, wall_run::WALL_CLING_PROBE) {
            let pose = wall_run::cling_pose_for_normal(
                wall_run::Vec3::new(contact.normal.x, contact.normal.y, contact.normal.z),
                wall_run::Vec3::new(facing.x, 0.0, facing.z),
            );
            return Some((pose, contact));
        }
    }
    None
}

fn wall_kick_pose_for_cling(cling_pose: u16) -> u16 {
    match cling_pose {
        L::WALL_CLING_F => L::JUMP_B,
        L::WALL_CLING_L => L::WALL_KICK_L,
        L::WALL_CLING_R => L::WALL_KICK_R,
        _ => L::JUMP_F,
    }
}

fn try_wall_jump<M: meteor_physics::collision::MeshCollider>(
    air: &mut AirState,
    wall: &mut FighterWall,
    sys: &CollisionSystem<M>,
    pos: Point3,
    facing: Vec3,
    current_pose_id: u16,
    strong: bool,
) -> Option<u16> {
    if wall.cling_active {
        let pose = wall_kick_pose_for_cling(current_pose_id);
        let speed = if strong {
            jump::JUMP_VERTICAL_SPEED_STRONG
        } else {
            jump::JUMP_VERTICAL_SPEED
        };
        let velocity = (wall.wall_normal + Vec3::Y).normalize_or_zero() * speed;
        air.planar_x = velocity.x;
        air.planar_z = velocity.z;
        air.vertical_speed = velocity.y;
        Some(pose)
    } else {
        let right = Vec3::new(-facing.z, 0.0, facing.x);
        let probes = [
            (L::JUMP_B, facing),
            (L::WALL_KICK_R, (facing + right).normalize_or_zero()),
            (L::WALL_KICK_L, (facing - right).normalize_or_zero()),
        ];
        for (pose, dir) in probes {
            if let Some(contact) = probe_wall(sys, pos, dir, wall_run::WALL_KICK_PROBE) {
                let outward = contact.normal;
                let into = air.planar_x * outward.x
                    + air.vertical_speed * outward.y
                    + air.planar_z * outward.z;
                air.planar_x -= outward.x * into;
                air.planar_z -= outward.z * into;
                air.vertical_speed -= outward.y * into;
                let speed = if strong {
                    wall_run::WALL_KICK_SPEED_STRONG
                } else {
                    wall_run::WALL_KICK_SPEED
                };
                let impulse = (outward + Vec3::Y).normalize_or_zero() * speed;
                air.planar_x += impulse.x;
                air.planar_z += impulse.z;
                air.vertical_speed += impulse.y;
                wall.wall_normal = outward;
                return Some(pose);
            }
        }
        None
    }
}

fn tick_wall_cling<M: meteor_physics::collision::MeshCollider>(
    entity: Entity,
    contacts: Option<&mut CharacterContactFrame>,
    air: &mut AirState,
    wall: &mut FighterWall,
    sys: &CollisionSystem<M>,
    pos: Point3,
    dt: f32,
) -> meteor_physics::collision::WallMove {
    let v = Vec3::new(air.planar_x, air.vertical_speed, air.planar_z);
    let acceleration = v * -wall_run::WALL_VELOCITY_DAMPING
        + wall.external_accel
        + Vec3::Y * (wall_run::WALL_GRAVITY_SCALE * jump::GRAVITY);
    let v = v + acceleration * dt;
    let probe = wall.wall_normal * -wall_run::WALL_CONTACT_PROBE;
    let result = super::character_contact_plugin::resolve_fighter_wall_motion(
        contacts,
        entity,
        sys,
        pos,
        Point3::new(v.x, v.y, v.z),
        Point3::new(probe.x, probe.y, probe.z),
        dt,
    );
    air.planar_x = result.velocity.x;
    air.vertical_speed = result.velocity.y;
    air.planar_z = result.velocity.z;
    wall.contact_seen |= result.flags != 0;
    result
}

fn dodge_pose_for_input(
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    keymap: &crate::app_state::Keymap,
    strategy_id: u8,
    roll: bool,
) -> u16 {
    let input = directional_input(keys, mouse, keymap);
    dodge_pose_for_axes(Vec2::new(input.x, input.z), strategy_id, roll)
}

fn dodge_pose_for_axes(input: Vec2, strategy_id: u8, roll: bool) -> u16 {
    mapped_dodge_pose_for_input(input.x, input.y, false, roll, Some(strategy_id))
}

pub struct MovementPlugin;

#[derive(Resource, Default)]
struct DeferredAirRootMotion;

impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        let actor_schedule = super::fighter_frame::schedule(app);
        app.init_resource::<DeferredAirRootMotion>()
            .add_systems(
                actor_schedule,
                prepare_air_attack_modes
                    .before(move_local_fighter)
                    .in_set(crate::plugins::FighterFrameSet::Locomotion),
            )
            .add_systems(
                actor_schedule,
                move_local_fighter
                    .run_if(super::fighter_frame::current_is_local)
                    .after(crate::plugins::camera_plugin::prepare_move_basis)
                    .in_set(crate::plugins::FighterFrameSet::Locomotion),
            )
            .add_systems(
                actor_schedule,
                hold_airborne_fall_pose.in_set(crate::plugins::FighterFrameSet::SkeletonSample),
            )
            .add_systems(
                actor_schedule,
                (
                    attack_root_motion,
                    weapon_switch_root_motion,
                    dodge_root_motion,
                    local_ballistic_air_root_motion.run_if(super::fighter_frame::current_is_local),
                    air_curve_root_motion,
                    crate::plugins::ai_plugin::npc_ballistic_air_root_motion,
                )
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::RootMotion),
            );
    }
}

pub(crate) fn hold_airborne_fall_pose(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    mut query: Query<(Entity, &FighterAir, &mut PoseRuntime), Without<RemoteFighter>>,
) {
    for (entity, air, mut pose) in &mut query {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        if air.air.airborne && pose.is_fall_hold() {
            pose.frame = pose.start as f32;
        }
    }
}

fn is_attacking_state(state: FighterState) -> bool {
    matches!(
        state,
        FighterState::Attack1 | FighterState::Attack2 | FighterState::Attack3
    )
}

#[cfg(test)]
fn compose_attack_planar_step(
    anim: Vec2,
    carry: Vec2,
    external: Vec2,
    carry_decay: f32,
    dt: f32,
) -> (f32, f32, f32, f32) {
    let (carry_x, carry_z, raw_x, raw_z) =
        advance_attack_planar_step(anim, carry, external, carry_decay, dt);
    let (dx, _, dz, _) = clamp_composed_delta_3d(raw_x, 0.0, raw_z, dt, CARRY_MAX_SPEED);
    (carry_x, carry_z, dx, dz)
}

fn advance_attack_planar_step(
    anim: Vec2,
    carry: Vec2,
    external: Vec2,
    carry_decay: f32,
    dt: f32,
) -> (f32, f32, f32, f32) {
    if dt <= 0.0 || !dt.is_finite() {
        return (carry.x, carry.y, anim.x, anim.y);
    }
    let k = (1.0 - carry_decay * dt).max(0.0);
    let carry_x = carry.x * k;
    let carry_z = carry.y * k;
    let tx = anim.x + (carry_x + external.x) * dt;
    let tz = anim.y + (carry_z + external.y) * dt;
    (carry_x, carry_z, tx, tz)
}

fn compose_dodge_planar_step(
    anim: Vec2,
    residual: Vec2,
    external: Vec2,
    carry_decay: f32,
    dt: f32,
) -> (f32, f32, f32, f32) {
    if dt <= 0.0 || !dt.is_finite() {
        return (residual.x, residual.y, anim.x, anim.y);
    }
    let k = (1.0 - carry_decay * dt).max(0.0);
    let residual_x = residual.x * k;
    let residual_z = residual.y * k;
    let (dx, _, dz, _) = clamp_composed_delta_3d(
        anim.x + (residual_x + external.x) * dt,
        0.0,
        anim.y + (residual_z + external.y) * dt,
        dt,
        CARRY_MAX_SPEED,
    );
    (residual_x, residual_z, dx, dz)
}

pub(crate) fn ordinary_locomotion_allowed(sm: &SmState, switching: bool) -> bool {
    !switching
        && !is_attacking_state(sm.current)
        && !matches!(
            sm.current,
            FighterState::Guard
                | FighterState::Dodge
                | FighterState::Burst
                | FighterState::Fake
                | FighterState::Cure
                | FighterState::BeHit
                | FighterState::Down
                | FighterState::Dead
        )
}

pub(crate) fn wheel_vetoes_dodge_action(wheel_out: bool, action_is_burst: bool) -> bool {
    wheel_out && !action_is_burst
}

fn can_start_dodge_action(
    sm: &SmState,
    airborne: bool,
    switching_weapon: bool,
    wheel_out: bool,
    burst: bool,
) -> bool {
    if switching_weapon
        || matches!(sm.current, FighterState::BeHit | FighterState::Dead)
        || !can_interrupt(
            sm.current,
            if burst { Action::Burst } else { Action::Dodge },
        )
        || (airborne && !matches!(sm.current, FighterState::Down | FighterState::Fake))
    {
        return false;
    }
    !wheel_vetoes_dodge_action(wheel_out, burst)
}

fn accepted_dodge_still_owns_state(state: FighterState, burst: bool) -> bool {
    state
        == if burst {
            FighterState::Burst
        } else {
            FighterState::Dodge
        }
}

pub(crate) fn dodge_pose_finished(pose: &PoseRuntime) -> bool {
    if pose.loops && !pose.single_frame_loop_guard() {
        pose.frame >= pose.loop_end as f32
    } else {
        pose.finished()
    }
}

pub(crate) fn apply_dodge_pose_now(
    assets: Option<&crate::plugins::character_plugin::CharacterAssetsSet>,
    fighter: &Fighter,
    pose: &mut PoseRuntime,
    blend: &mut EntryBlend,
    dodge_pose: u16,
) -> bool {
    if pose.pose_id == dodge_pose {
        return true;
    }
    let Some(character_assets) =
        assets.and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
    else {
        return false;
    };
    if !character_assets.poses.contains_key(&dodge_pose) {
        return false;
    }
    character_assets.apply_pose_with_blend(
        pose,
        blend,
        dodge_pose,
        true,
        None,
        EntryBlendKind::Generic,
        false,
        false,
        Some(meteor_combat::weapon_strategy::weapon_to_strategy(
            fighter.weapon,
        )),
    );
    true
}

pub(crate) fn enter_dodge_state(sm: &mut SmState, combat: &mut CombatState, burst: bool) {
    sm.previous = sm.current;
    sm.current = if burst {
        FighterState::Burst
    } else {
        FighterState::Dodge
    };
    sm.time_in_state = 0.0;
    sm.locked = false;
    sm.attack_queued = None;
    sm.linked_recovery_pose_id = None;
    sm.exe_ground_mode_ticks = 0;
    combat.state = sm.current;
}

pub(crate) fn finish_dodge_state(sm: &mut SmState, combat: &mut CombatState, guard_held: bool) {
    sm.previous = sm.current;
    sm.current = if guard_held {
        FighterState::Guard
    } else {
        FighterState::Name
    };
    sm.time_in_state = 0.0;
    sm.locked = false;
    sm.attack_queued = None;
    sm.linked_recovery_pose_id = None;
    sm.exe_ground_mode_ticks = 0;
    combat.state = sm.current;
}

fn enter_jump_state(sm: &mut SmState, combat: &mut CombatState) {
    if sm.current == FighterState::Jump {
        combat.state = FighterState::Jump;
        return;
    }
    sm.previous = sm.current;
    sm.current = FighterState::Jump;
    sm.time_in_state = 0.0;
    sm.locked = false;
    sm.attack_queued = None;
    sm.linked_recovery_pose_id = None;
    sm.exe_ground_mode_ticks = 0;
    combat.state = FighterState::Jump;
}

fn land_jump_state(sm: &mut SmState, combat: &mut CombatState) {
    if sm.current != FighterState::Jump {
        return;
    }
    sm.previous = sm.current;
    sm.current = FighterState::Name;
    sm.time_in_state = 0.0;
    sm.locked = false;
    sm.attack_stage = 0;
    sm.attack_queued = None;
    sm.linked_recovery_pose_id = None;
    sm.exe_ground_mode_ticks = 0;
    combat.state = FighterState::Name;
}

fn land_air_attack_state(
    sm: &mut SmState,
    combat: &mut CombatState,
    pose_id: u16,
    recovery_pose_id: Option<u16>,
) -> Option<u16> {
    if !matches!(
        sm.current,
        FighterState::Attack1 | FighterState::Attack2 | FighterState::Attack3
    ) {
        return None;
    }
    sm.attack_stage = sm.attack_stage.max(1);

    if recovery_pose_id.is_some_and(|id| id != pose_id) {
        sm.attack_queued = None;
    }
    sm.linked_recovery_pose_id = recovery_pose_id;
    combat.state = sm.current;
    recovery_pose_id.filter(|&id| id != pose_id)
}

type AttackMotionQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static mut Transform,
        &'static mut MotionState,
        &'static mut FighterAttackMotion,
        &'static PoseRuntime,
        &'static crate::components::fighter::EntryBlend,
        &'static SmState,
        &'static FighterAir,
    ),
    (
        With<Fighter>,
        Without<crate::plugins::network_plugin::RemoteFighter>,
    ),
>;

#[allow(clippy::too_many_arguments)]
pub(crate) fn attack_root_motion(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    common_amb: Option<Res<crate::plugins::character_plugin::CommonAmb>>,
    collider: Res<StageCollider>,
    mut contacts: Option<ResMut<CharacterContactFrame>>,
    mut query: AttackMotionQuery<'_, '_>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let Some(mesh) = &collider.mesh else {
        return;
    };
    let mut sys = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    sys.set_mesh_collider(mesh);
    let Some(assets) = assets else {
        return;
    };
    let Some(common_amb) = common_amb else {
        return;
    };

    for (entity, fighter, mut transform, mut motion, mut am, pose, blend, sm, air) in &mut query {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };

        am.air_step_pending = false;
        if air.air_attack_recovery_pose == Some(pose.pose_id) && pose.finished() {
            continue;
        }

        let motion_root = character_assets.motion_root_at(&common_amb.0, pose.pose_id, pose.frame);
        let (rx, ry) = (motion_root[0], motion_root[1]);
        let attacking = is_attacking_state(sm.current);

        if attacking && !am.active {
            let start_root =
                character_assets.motion_root_at(&common_amb.0, pose.pose_id, pose.start as f32);
            am.active = true;
            am.stopping = false;
            am.prev_pose_id = pose.pose_id;
            am.prev_pose_instance_id = pose.pose_instance_id;
            am.prev_group = character_assets.act.groups.get(&pose.pose_id).copied();
            let entry_held = blend.duration > 0.0 && blend.pose_id == pose.pose_id;
            am.prev_root_x = if entry_held { rx } else { start_root[0] };
            am.prev_root_y = if entry_held { ry } else { start_root[1] };
            am.hold_delta = entry_held;
            if am.carry_banked {
                am.carry_banked = false;
            } else {
                am.carry_x = motion.velocity.x;
                am.carry_z = motion.velocity.z;
            }
        } else if !attacking && am.active && !am.stopping {
            am.stopping = true;
        }

        if !am.active {
            continue;
        }

        if am.prev_pose_id != pose.pose_id || am.prev_pose_instance_id != pose.pose_instance_id {
            let same_pose_restarted = am.prev_pose_id == pose.pose_id;
            let next_group = character_assets.act.groups.get(&pose.pose_id).copied();
            let frozen_chained_blend = blend.duration > 0.0
                && blend.pose_id == pose.pose_id
                && blend.kind == crate::components::fighter::EntryBlendKind::Chained;
            let continuous = !same_pose_restarted
                && !frozen_chained_blend
                && am.prev_group.is_some()
                && am.prev_group == next_group;
            am.prev_pose_id = pose.pose_id;
            am.prev_pose_instance_id = pose.pose_instance_id;
            am.prev_group = next_group;
            if !continuous {
                am.prev_root_x = rx;
                am.prev_root_y = ry;
                am.hold_delta = true;
            }
        }

        let mut anim_x = 0.0;
        let mut anim_z = 0.0;
        if am.hold_delta {
            am.prev_root_x = rx;
            am.prev_root_y = ry;
            am.hold_delta = false;
        } else if !am.stopping || pose.pose_id == am.prev_pose_id {
            let [dx, dy, _] = if blend.duration > 0.0 && blend.pose_id == pose.pose_id {
                [0.0; 3]
            } else {
                character_assets.authored_root_step(&common_amb.0, pose, dt)
            };
            am.prev_root_x = rx;
            am.prev_root_y = ry;
            if dx.abs() > 1e-6 || dy.abs() > 1e-6 {
                (anim_x, anim_z) =
                    meteor_anim::root_motion::root_delta_to_world_xz(dx, dy, motion.facing, 1.0);
            }
        }

        if dt > 0.0 {
            let (carry_x, carry_z, raw_tx, raw_tz) = advance_attack_planar_step(
                Vec2::new(anim_x, anim_z),
                Vec2::new(am.carry_x, am.carry_z),
                Vec2::new(
                    air.ground_external_velocity.x,
                    air.ground_external_velocity.z,
                ),
                action_carry_decay(air),
                dt,
            );
            am.carry_x = carry_x;
            am.carry_z = carry_z;
            if air.air.airborne {
                am.pending_air_dx = raw_tx;
                am.pending_air_dz = raw_tz;
                am.air_step_pending = true;
            } else {
                let (tx, _, tz, _) =
                    clamp_composed_delta_3d(raw_tx, 0.0, raw_tz, dt, CARRY_MAX_SPEED);
                let mut resolved_x = 0.0;
                let mut resolved_z = 0.0;
                if tx.abs() > 1e-9 || tz.abs() > 1e-9 {
                    let pos = Point3::new(
                        transform.translation.x,
                        transform.translation.y,
                        transform.translation.z,
                    );
                    let (res_x, res_z, res_y) = resolve_fighter_xz(
                        contacts.as_deref_mut(),
                        entity,
                        &sys,
                        pos,
                        tx,
                        tz,
                        false,
                    );
                    transform.translation.x += res_x;
                    transform.translation.z += res_z;

                    transform.translation.y += res_y;
                    commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);
                    resolved_x = res_x;
                    resolved_z = res_z;

                    if res_x == 0.0 && tx != 0.0 {
                        am.carry_x = 0.0;
                    }
                    if res_z == 0.0 && tz != 0.0 {
                        am.carry_z = 0.0;
                    }
                }

                motion.velocity = Vec3::new(resolved_x / dt, 0.0, resolved_z / dt);
            }
        }

        if am.carry_banked {
            am.carry_banked = false;
        }

        if am.stopping {
            am.active = false;
            am.stopping = false;
        }
    }
}

type WeaponSwitchMotionQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static mut Transform,
        &'static mut MotionState,
        &'static FighterAir,
        &'static mut WeaponSwitchActive,
        &'static PoseRuntime,
        &'static FighterHitReaction,
    ),
    (With<Fighter>, Without<RemoteFighter>),
>;

#[allow(clippy::too_many_arguments)]
pub(crate) fn weapon_switch_root_motion(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    common_amb: Option<Res<crate::plugins::character_plugin::CommonAmb>>,
    collider: Res<StageCollider>,
    mut contacts: Option<ResMut<CharacterContactFrame>>,
    mut query: WeaponSwitchMotionQuery<'_, '_>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    if dt <= 0.0 {
        return;
    }
    let (Some(assets), Some(common), Some(mesh)) = (assets, common_amb, collider.mesh.as_ref())
    else {
        return;
    };
    let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    mover.set_mesh_collider(mesh);

    for (entity, fighter, mut transform, mut motion, air, mut switching, pose, hit_reaction) in
        &mut query
    {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        if switching.airborne
            || air.air.airborne
            || hit_reaction.current.is_some()
            || pose.pose_id != switching.pose_id
            || pose.pose_instance_id != switching.pose_instance_id
            || (switching.end_sampled && pose.frame >= switching.end_frame as f32)
        {
            continue;
        }
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        let root = character_assets.motion_root_at(&common.0, pose.pose_id, pose.frame);
        let mut anim = Vec2::ZERO;
        if switching.root_started {
            let (world_x, world_z) = meteor_anim::root_motion::root_delta_to_world_xz(
                root[0] - switching.prev_root_x,
                root[1] - switching.prev_root_y,
                motion.facing,
                1.0,
            );
            anim = Vec2::new(world_x, world_z);
        }
        switching.root_started = true;
        switching.prev_root_x = root[0];
        switching.prev_root_y = root[1];

        let (carry_x, carry_z, requested_x, requested_z) = compose_dodge_planar_step(
            anim,
            Vec2::new(switching.carry_x, switching.carry_z),
            Vec2::new(
                air.ground_external_velocity.x,
                air.ground_external_velocity.z,
            ),
            action_carry_decay(air),
            dt,
        );
        switching.carry_x = carry_x;
        switching.carry_z = carry_z;
        let pos = Point3::new(
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
        );
        let (accepted_x, accepted_z, accepted_y) = resolve_fighter_xz(
            contacts.as_deref_mut(),
            entity,
            &mover,
            pos,
            requested_x,
            requested_z,
            false,
        );
        transform.translation.x += accepted_x;
        transform.translation.z += accepted_z;
        transform.translation.y += accepted_y;
        let carry_dx = carry_x * dt;
        let carry_dz = carry_z * dt;
        if accepted_x == 0.0 && carry_dx != 0.0 && requested_x != 0.0 {
            switching.carry_x = 0.0;
        }
        if accepted_z == 0.0 && carry_dz != 0.0 && requested_z != 0.0 {
            switching.carry_z = 0.0;
        }
        motion.velocity = accepted_motion_velocity(accepted_x, accepted_y, accepted_z, dt);
        commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);
    }
}

type LocalFighterQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static mut Transform,
        &'static mut MotionState,
        &'static mut FighterAir,
        &'static mut FighterDodge,
        &'static mut FighterWall,
        &'static mut PoseRuntime,
        &'static mut crate::components::fighter::EntryBlend,
        &'static mut SmState,
        &'static mut CombatState,
        &'static GunAimState,
        (
            &'static RangedFireGate,
            &'static crate::components::fighter::FighterStatusEffects,
        ),
        &'static mut FighterHitReaction,
        Option<&'static WeaponSwitchActive>,
    ),
    With<LocalFighter>,
>;

#[derive(SystemParam)]
pub(crate) struct MovementAuxInput<'w> {
    mrc_playback: Option<Res<'w, crate::plugins::replay_plugin::MrcPlayback>>,
    mrc_frame: Option<Res<'w, crate::plugins::replay_plugin::MrcFrameOverride>>,
    crouch_input: Res<'w, LocalCrouchInput>,
    jump_input: Option<ResMut<'w, LocalJumpInput>>,
    deferred_air_root_motion: Option<Res<'w, DeferredAirRootMotion>>,
    desktop_dodge: Option<ResMut<'w, DesktopDodgeInput>>,
}

fn start_jump_with_velocity_transfer(
    air: &mut AirState,
    motion: &mut MotionState,
    strong: bool,
    impulse: jump::Impulse,
) {
    air.start_jump(strong, impulse);

    motion.velocity.x = 0.0;
    motion.velocity.z = 0.0;
}

fn snap_jump_motion_baseline(air: &mut FighterAir, start_root: [f32; 3], pose_id: u16) {
    air.stand_motion_root_x = start_root[0];
    air.stand_motion_root_y = start_root[1];
    air.pin_root_z = start_root[2];
    air.jump_root_xy_started = false;
    air.jump_prev_pose_id = pose_id;
    air.air.vertical_pose_id = pose_id;
    air.air.prev_root_z = Some(start_root[2]);
    air.motion_root_pin = Some([
        air.stand_motion_root_x,
        air.stand_motion_root_y,
        air.pin_root_z,
    ]);
    air.motion_root_pin_pose_id = pose_id;
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn start_scripted_jump(
    fighter: &Fighter,
    assets: &crate::plugins::character_plugin::CharacterAssetsSet,
    common: &crate::plugins::character_plugin::CommonAmb,
    air: &mut FighterAir,
    motion: &mut MotionState,
    pose: &mut PoseRuntime,
    blend: &mut EntryBlend,
    sm: &mut SmState,
    combat: &mut CombatState,
    strong: bool,
) -> bool {
    if air.air.airborne || !ordinary_locomotion_allowed(sm, false) {
        return false;
    }
    let Some(character) = assets.0.get(&format!("p{}", fighter.character_id)) else {
        return false;
    };
    let jump_pose = meteor_physics::locomotion::jump_pose_for_input(0.0, 0.0);
    let Some(definition) = character.poses.get(&jump_pose) else {
        return false;
    };
    let impulse = jump::jump_impulse(JumpStartInput {
        carry_x: motion.velocity.x,
        carry_z: motion.velocity.z,
    });
    start_jump_with_velocity_transfer(&mut air.air, motion, strong, impulse);

    air.ledge_fall = false;
    air.peak_lift = 0.0;
    air.jump_hold = 0.0;
    air.jump_charge_active = false;
    motion.movement_mode = MovementMode::Airborne;
    enter_jump_state(sm, combat);
    let root = character.motion_root_at(&common.0, jump_pose, definition.start as f32);
    snap_jump_motion_baseline(air, root, jump_pose);
    character.apply_pose_with_blend(
        pose,
        blend,
        jump_pose,
        true,
        None,
        EntryBlendKind::Generic,
        true,
        false,
        Some(meteor_combat::weapon_strategy::weapon_to_strategy(
            fighter.weapon,
        )),
    );
    true
}

fn apply_wall_kick_pose_now(
    assets: Option<&crate::plugins::character_plugin::CharacterAssetsSet>,
    common: Option<&crate::plugins::character_plugin::CommonAmb>,
    fighter: &Fighter,
    pose: &mut PoseRuntime,
    blend: &mut EntryBlend,
    air: &mut FighterAir,
    kick_pose: u16,
) -> bool {
    let Some(character_assets) =
        assets.and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
    else {
        return false;
    };
    let Some(pose_def) = character_assets.poses.get(&kick_pose) else {
        return false;
    };
    let Some(common) = common else {
        return false;
    };
    character_assets.apply_pose_with_blend(
        pose,
        blend,
        kick_pose,
        true,
        None,
        EntryBlendKind::Generic,
        true,
        false,
        Some(meteor_combat::weapon_strategy::weapon_to_strategy(
            fighter.weapon,
        )),
    );

    let start_root = character_assets.motion_root_at(&common.0, kick_pose, pose_def.start as f32);
    snap_jump_motion_baseline(air, start_root, kick_pose);
    true
}

fn apply_wall_cling_pose_now(
    assets: Option<&crate::plugins::character_plugin::CharacterAssetsSet>,
    common: Option<&crate::plugins::character_plugin::CommonAmb>,
    fighter: &Fighter,
    pose: &mut PoseRuntime,
    blend: &mut EntryBlend,
    air: &mut FighterAir,
    cling_pose: u16,
) -> bool {
    let applied = apply_wall_kick_pose_now(assets, common, fighter, pose, blend, air, cling_pose);
    if applied {
        air.late_visual_pose_snap = true;
    }
    applied
}

fn release_wall_motion(air: &mut FighterAir, wall: &mut FighterWall, pose_id: u16, current_y: f32) {
    wall.cling_active = false;
    wall.contact_seen = false;
    wall.external_accel = Vec3::ZERO;
    air.air.motion_mode = AirMotionMode::Ballistic;
    air.air.vertical_pose_id = pose_id;
    air.air.prev_root_z = None;
    air.air.launch_pose_id = 0;
    air.air.launch_lifted = true;
    air.jump_root_xy_started = false;
    air.jump_prev_pose_id = pose_id;
    air.peak_lift = air.peak_lift.max((current_y - air.ground_y).max(0.0));
}

pub(crate) fn finish_air_landing_state(air: &mut FighterAir) {
    air.landed_planar = Vec2::new(air.air.planar_x, air.air.planar_z);
    air.air.land();
    air.ground_contact = true;
    air.carry_no_support = false;
    air.ledge_fall = false;
    air.pending_ledge_fall = false;
    air.peak_lift = 0.0;
    air.motion_root_pin = None;
    air.motion_root_pin_pose_id = 0;
    air.jump_root_xy_started = false;
    air.late_visual_pose_snap = false;
}

fn finish_air_landing(air: &mut FighterAir, wall: &mut FighterWall) {
    finish_air_landing_state(air);
    wall.cling_active = false;
    wall.contact_seen = false;
    wall.external_accel = Vec3::ZERO;
    wall.remaining = 0.0;
}

fn displayed_motion_root(
    fighter: Entity,
    models: &Query<(Entity, &FighterModel)>,
    dummies: &Query<(&FighterDummy, &Transform), Without<LocalFighter>>,
) -> Option<[f32; 3]> {
    let model = models
        .iter()
        .find_map(|(entity, model)| (model.fighter == fighter).then_some(entity))?;
    dummies.iter().find_map(|(dummy, transform)| {
        (dummy.model == model && dummy.name == "d_base").then_some(transform.translation.to_array())
    })
}

fn begin_ground_self_launch(
    air: &mut FighterAir,
    wall: &mut FighterWall,
    motion: &mut MotionState,
    pose_id: u16,
    start_root: [f32; 3],
    displayed_root: [f32; 3],
) {
    air.air.start_ground_launch(pose_id);
    air.peak_lift = 0.0;
    air.air.prev_root_z = Some(start_root[2]);
    air.stand_motion_root_x = displayed_root[0];
    air.stand_motion_root_y = displayed_root[1];
    air.pin_root_z = displayed_root[2];
    air.jump_root_xy_started = false;
    air.jump_prev_pose_id = pose_id;

    air.motion_root_pin = Some(displayed_root);
    air.motion_root_pin_pose_id = pose_id;
    air.ground_contact = true;
    air.ledge_fall = false;
    wall.cling_active = false;
    wall.contact_seen = false;
    wall.external_accel = Vec3::ZERO;
    motion.movement_mode = MovementMode::Airborne;
}

fn attack_clip_owns_landing(
    attacking: bool,
    fall_hold: bool,
    authored_link: Option<u16>,
    link_exists: bool,
) -> bool {
    attacking && !fall_hold && authored_link.is_some() && link_exists
}

fn release_air_attack_hold_on_contact(pose: &mut PoseRuntime, attacking: bool, contact: bool) {
    if attacking && contact && pose.is_fall_hold() {
        pose.frame = pose.loop_end as f32;
        pose.loops = false;
    }
}

fn should_begin_ground_self_launch(state: FighterState, airborne: bool, pose_id: u16) -> bool {
    is_attacking_state(state) && !airborne && jump::is_self_launch_attack_pose(pose_id)
}

pub(crate) fn air_curve_motion_owned(
    motion_mode: AirMotionMode,
    switching_weapon_air: bool,
) -> bool {
    switching_weapon_air || matches!(motion_mode, AirMotionMode::Curve | AirMotionMode::CurveExit)
}

fn handoff_landed_air_weapon_switch(switch: &mut WeaponSwitchActive) -> bool {
    if !switch.airborne {
        return false;
    }
    switch.airborne = false;
    switch.root_started = false;
    true
}

type AirAttackPrepareQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static PoseRuntime,
        &'static SmState,
        &'static mut FighterAir,
        &'static mut FighterWall,
        &'static mut MotionState,
    ),
    (With<Fighter>, Without<RemoteFighter>),
>;

fn prepare_air_attack_modes(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    common_amb: Option<Res<crate::plugins::character_plugin::CommonAmb>>,
    models: Query<(Entity, &FighterModel)>,
    dummies: Query<(&FighterDummy, &Transform), Without<LocalFighter>>,
    mut query: AirAttackPrepareQuery<'_, '_>,
) {
    let (Some(assets), Some(common)) = (assets, common_amb) else {
        return;
    };
    for (entity, fighter, pose, sm, mut air, mut wall, mut motion) in &mut query {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        if !is_attacking_state(sm.current) {
            if air.air_attack_recovery_pose == Some(pose.pose_id) && pose.finished() {
                air.air_attack_recovery_pose = None;
                air.air.motion_mode = AirMotionMode::Ballistic;
                air.air.prev_root_z = None;
                air.ledge_fall = true;
                air.attack_ground_pose_pending = true;
                air.recovery_ground_mode = Some(true);
            } else if air.air_attack_recovery_pose != Some(pose.pose_id) {
                air.air_attack_recovery_pose = None;
            }
            continue;
        }
        air.recovery_ground_mode = None;
        if pose.is_fall_hold() {
            air.air_attack_recovery_pose = Some(pose.pose_id);
        } else if air.air_attack_recovery_pose != Some(pose.pose_id) {
            air.air_attack_recovery_pose = None;
        }
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        let start_root =
            character_assets.motion_root_at(&common.0, pose.pose_id, pose.start as f32);

        if should_begin_ground_self_launch(sm.current, air.air.airborne, pose.pose_id) {
            let displayed_root =
                displayed_motion_root(entity, &models, &dummies).unwrap_or(start_root);
            begin_ground_self_launch(
                &mut air,
                &mut wall,
                &mut motion,
                pose.pose_id,
                start_root,
                displayed_root,
            );
            continue;
        }
        if !air.air.airborne {
            continue;
        }

        let from_wall = wall.cling_active;
        wall.cling_active = false;
        wall.contact_seen = false;
        wall.external_accel = Vec3::ZERO;
        if !matches!(
            air.air.motion_mode,
            AirMotionMode::Curve | AirMotionMode::CurveExit
        ) || from_wall
        {
            air.air.start_curve(false, pose.pose_id);
            air.air.vertical_pose_id = pose.pose_id;
            air.air.prev_root_z = Some(start_root[2]);
        }
        motion.movement_mode = MovementMode::Airborne;
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_jump_root_horizontal<M: meteor_physics::collision::MeshCollider>(
    entity: Entity,
    mut contacts: Option<&mut CharacterContactFrame>,
    air: &mut FighterAir,
    pose: &PoseRuntime,
    motion: &MotionState,
    transform: &mut Transform,
    sys: &mut CollisionSystem<M>,
    root_x: f32,
    root_y: f32,
    attacking: bool,
) {
    if attacking {
        air.jump_root_xy_started = false;
        air.jump_prev_pose_id = pose.pose_id;
        return;
    }
    let pose_changed = pose.pose_id != air.jump_prev_pose_id;
    air.jump_prev_pose_id = pose.pose_id;
    if !air.jump_root_xy_started || pose_changed {
        air.jump_prev_root_x = root_x;
        air.jump_prev_root_y = root_y;
        if !air.jump_root_xy_started {
            air.stand_motion_root_x = root_x;
            air.stand_motion_root_y = root_y;
            air.jump_root_xy_started = true;
        }
    } else {
        let (wx, wz) = meteor_anim::root_motion::root_delta_to_world_xz(
            root_x - air.jump_prev_root_x,
            root_y - air.jump_prev_root_y,
            motion.facing,
            meteor_anim::root_motion::ROOT_MOTION_SCALE,
        );
        air.jump_prev_root_x = root_x;
        air.jump_prev_root_y = root_y;
        let pos = Point3::new(
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
        );
        let (rx, rz, ry) =
            resolve_fighter_xz(contacts.as_deref_mut(), entity, sys, pos, wx, wz, false);
        transform.translation.x += rx;
        transform.translation.z += rz;
        transform.translation.y += ry;
        commit_fighter_position(contacts, entity, transform.translation);
    }
}

type LocalBallisticAirQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static mut Transform,
        &'static mut MotionState,
        &'static mut FighterAir,
        &'static FighterDodge,
        &'static mut FighterWall,
        &'static mut PoseRuntime,
        &'static mut EntryBlend,
        &'static mut SmState,
        &'static mut CombatState,
        &'static FighterHitReaction,
        Option<&'static WeaponSwitchActive>,
        (
            Option<&'static RangedFireGate>,
            &'static crate::components::fighter::FighterStatusEffects,
        ),
    ),
    With<LocalFighter>,
>;

#[allow(clippy::too_many_arguments)]
fn local_ballistic_air_root_motion(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    common_amb: Option<Res<crate::plugins::character_plugin::CommonAmb>>,
    collider: Res<StageCollider>,
    mut contacts: Option<ResMut<CharacterContactFrame>>,
    player_performance: Option<Res<crate::plugins::stage_plugin::PlayerPerformance>>,
    crouch_input: Res<LocalCrouchInput>,
    buffs: Res<crate::plugins::item_plugin::ItemBuffs>,
    keymap: Res<crate::app_state::Keymap>,
    mut query: LocalBallisticAirQuery<'_, '_>,
    lock_facing: (
        ResMut<LockOnController>,
        ResMut<CameraBasis>,
        Query<&Transform, Without<LocalFighter>>,
        Option<Res<crate::plugins::replay_plugin::MrcPlayback>>,
    ),
) {
    let (mut lock, mut basis, targets, playback) = lock_facing;
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    if dt <= 0.0 {
        return;
    }
    let Some(mesh) = collider.mesh.as_ref() else {
        return;
    };
    let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    mover.set_mesh_collider(mesh);

    for (
        entity,
        fighter,
        mut transform,
        mut motion,
        mut air,
        dodge,
        mut wall,
        mut pose,
        mut blend,
        mut sm,
        mut combat,
        hit_reaction,
        weapon_switch,
        (ranged_gate, status_effects),
    ) in &mut query
    {
        if dodge.state.active || hit_reaction.current.is_some() {
            continue;
        }
        if !air.air.airborne && air.recovery_ground_mode.is_none() {
            let pos = Point3::new(
                transform.translation.x,
                transform.translation.y,
                transform.translation.z,
            );
            let ground = mover.sample_ground_hit(pos.x, pos.z, pos.y);
            if let Some(hit) = ground {
                air.ground_y = hit.y;
            }
            if ground.is_some_and(|hit| {
                pos.y - hit.y <= jump::STEP_DOWN
                    && mover.can_occupy(Point3::new(pos.x, hit.y, pos.z))
            }) {
                air.pending_ledge_fall = false;
                continue;
            }
            let (dy, foot, blocked) = mover.resolve_descent(pos, -jump::STEP_DOWN);
            let walkable = meteor_physics::collision::descent_has_support(foot, blocked);
            if walkable {
                transform.translation.y += dy.min(0.0);
                air.pending_ledge_fall = false;
                commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);
                continue;
            }
            air.air.start_fall(motion.velocity.x, motion.velocity.z);
            air.pending_ledge_fall = false;
            air.ground_contact = false;
            air.ledge_fall = true;
            air.peak_lift = (pos.y - air.ground_y).max(0.0);
            motion.movement_mode = MovementMode::Airborne;
            air.motion_root_pin = Some([
                air.stand_motion_root_x,
                air.stand_motion_root_y,
                air.pin_root_z,
            ]);
            air.motion_root_pin_pose_id = pose.pose_id;
            continue;
        }
        let switching_weapon_air =
            weapon_switch.is_some() && pose.pose_id == WEAPON_SWITCH_POSE_AIR;
        if air_curve_motion_owned(air.air.motion_mode, switching_weapon_air) {
            continue;
        }

        let input_blocked = player_performance
            .as_deref()
            .is_some_and(|performance| performance.input_blocked);
        let raw_input = if input_blocked {
            Vec3::ZERO
        } else {
            directional_input(&keys, &mouse, &keymap)
        };
        let ordinary_locomotion =
            !input_blocked && ordinary_locomotion_allowed(&sm, weapon_switch.is_some());
        let input = if ordinary_locomotion {
            raw_input
        } else {
            Vec3::ZERO
        };
        let loco = compose_loco_axes(input.x, input.z, FORWARD_STRAFE_SCALE);
        let crouching =
            ordinary_locomotion_allowed(&sm, weapon_switch.is_some()) && crouch_input.held;
        let running =
            ordinary_locomotion && !crouching && is_running(loco.running, &keys, &mouse, &keymap);
        let facing = Vec3::new(motion.facing.sin(), 0.0, motion.facing.cos());
        let attacking = is_attacking_state(sm.current);
        let tick_start = transform.translation;
        let tick_ground = sample_air_ground(
            &mut air,
            &mover,
            Point3::new(tick_start.x, tick_start.y, tick_start.z),
        );
        let tick_ground_y = tick_ground.map_or(f32::NEG_INFINITY, |hit| hit.y);

        if wall.cling_active {
            wall.remaining -= dt;
            air.air.air_time += dt;
            let mut touching = false;
            if input.z > 0.1 && wall.remaining > 0.0 {
                let p = Point3::new(
                    transform.translation.x,
                    transform.translation.y,
                    transform.translation.z,
                );
                let result = tick_wall_cling(
                    entity,
                    contacts.as_deref_mut(),
                    &mut air.air,
                    &mut wall,
                    &mover,
                    p,
                    dt,
                );
                transform.translation =
                    Vec3::new(result.position.x, result.position.y, result.position.z);
                touching = result.flags != 0;
                let vy = result.vertical_speed_before_contact;
                update_descent_slide(&mut air, result.vertical_surface, vy, dt);
            }
            if !touching {
                release_wall_motion(&mut air, &mut wall, pose.pose_id, transform.translation.y);
            }
            motion.movement_mode = if wall.cling_active {
                MovementMode::WallRun
            } else {
                MovementMode::Airborne
            };
            air.ground_contact = false;
            motion.velocity = Vec3::new(air.air.planar_x, air.air.vertical_speed, air.air.planar_z);
            commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);
            continue;
        }

        air.jump_root_xy_started = false;
        air.jump_prev_pose_id = pose.pose_id;
        let vertical_speed_before = air.air.vertical_speed;
        let recovery_mode = air.recovery_ground_mode;
        let ground_mode = recovery_mode == Some(true);
        air.locomotion_pose_sample = Some((ground_mode, vertical_speed_before));
        if recovery_mode.is_some() {
            air.attack_ground_pose_pending = ground_mode;
            let direction =
                world_direction(Vec3::new(loco.local_x, 0.0, loco.local_z), motion.facing);
            let params = planar_drive::DriveParams {
                dt,
                speed_factor: status_effects.effects.speed_factor(buffs.speed_factor),
                forward: input.z > 0.0 && !crouching,
                ground_support: air.ground_contact,
                air_mode0: !ground_mode,
                action_vetoed: ranged_gate.is_some_and(|gate| gate.wheel_out),
            };
            let velocity = [air.air.planar_x, vertical_speed_before, air.air.planar_z];
            let accel = planar_drive::input_accel(
                Vec3::from_array(velocity).length(),
                direction.to_array(),
                &params,
            );
            let mut driven = planar_drive::apply_friction(velocity, direction.to_array(), &params);
            let external = air.ground_external_velocity;
            driven[0] += accel[0] * dt + external.x;
            driven[1] += jump::GRAVITY * dt;
            driven[2] += accel[2] * dt + external.z;
            let driven = planar_drive::apply_drag(driven, &params);
            air.air.planar_x = driven[0];
            air.air.vertical_speed = driven[1];
            air.air.planar_z = driven[2];
            air.air.air_time += dt;
            air.ground_external_velocity = Vec3::ZERO;
        } else {
            tick_air_input_drive(
                &mut air,
                dt,
                input,
                motion.facing,
                crouching,
                status_effects.effects.speed_factor(buffs.speed_factor),
                ranged_gate.is_some_and(|gate| gate.wheel_out),
            );
            consume_descent_slide(&mut air);
        }
        let world_disp = if recovery_mode.is_some() {
            air.air.vertical_speed * dt
        } else {
            air.air.tick_vertical(dt, false)
        };
        let (px, vy, pz, world_disp) = clamp_air_motion_step(
            air.air.planar_x,
            air.air.vertical_speed,
            air.air.planar_z,
            world_disp,
        );
        air.air.planar_x = px;
        air.air.vertical_speed = vy;
        air.air.planar_z = pz;

        let pos = Point3::new(
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
        );
        let requested_x = px * dt;
        let requested_z = pz * dt;
        let (rx, rz, ry, planar_surface) =
            super::character_contact_plugin::resolve_fighter_xz_with_surface(
                contacts.as_deref_mut(),
                entity,
                &mover,
                pos,
                requested_x,
                requested_z,
                false,
            );
        project_planar_air_velocity_on_contact(&mut air.air, requested_x, requested_z, rx, rz);
        transform.translation.x += rx;
        transform.translation.z += rz;
        let vertical_origin = Point3::new(
            transform.translation.x,
            transform.translation.y + ry,
            transform.translation.z,
        );
        let (dy, foot, surface, blocked) = if world_disp >= 0.0 {
            let (dy, hit, blocked) = mover.resolve_ascent_with_surface(vertical_origin, world_disp);
            (
                dy,
                None::<FootHit>,
                hit.map(|hit| (hit.nx, hit.ny, hit.nz)),
                blocked,
            )
        } else {
            let (dy, hit, blocked) =
                mover.resolve_descent_with_surface(vertical_origin, world_disp, planar_surface);
            (dy, hit, hit.map(|hit| (hit.nx, hit.ny, hit.nz)), blocked)
        };
        air.mover_vertical_contact = surface.is_some() || blocked;

        if air.mover_vertical_contact {
            air.air.cling_armed = false;
        }
        let contact_speed = air.air.vertical_speed;
        update_descent_slide(&mut air, foot, contact_speed, dt);
        record_carry_support(
            &mut air,
            world_disp,
            surface,
            planar_surface.is_some()
                || (rx - requested_x).abs() > 1e-5
                || (rz - requested_z).abs() > 1e-5,
        );
        if recovery_mode.is_some() {
            if foot.is_some() || planar_surface.is_some() {
                air.air.vertical_speed = air.air.vertical_speed.max(0.0);
            }
        } else if let Some((nx, ny, nz)) = surface {
            if world_disp <= 0.0 || ny < 0.0 {
                project_air_velocity_on_surface(&mut air.air, nx, ny, nz);
            }
        }
        if blocked {
            air.air.vertical_speed = 0.0;
        }
        transform.translation.y += ry + dy;
        if recovery_mode.is_none() {
            transform.translation.y = transform.translation.y.max(tick_ground_y);
        }
        commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);

        let ground_y = tick_ground_y;
        let mover_floor_contact =
            world_disp < 0.0 && meteor_physics::collision::descent_has_support(foot, blocked);
        let near_ground =
            transform.translation.y <= ground_y + jump::LAND_EPS || mover_floor_contact;
        air.peak_lift = air
            .peak_lift
            .max((transform.translation.y - air.ground_y).max(0.0));
        air.ground_contact = ordinary_mover_support(
            air.ground_contact,
            planar_surface.is_some()
                || (rx - requested_x).abs() > 1e-5
                || (rz - requested_z).abs() > 1e-5,
            surface,
            blocked,
        );
        let character_assets = assets
            .as_ref()
            .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)));
        let authored_link = character_assets
            .and_then(|assets| assets.poses.get(&pose.pose_id))
            .and_then(|pose| pose.link);
        let owns_landing = attack_clip_owns_landing(
            attacking,
            pose.is_fall_hold(),
            authored_link,
            authored_link.is_some_and(|link| {
                character_assets.is_some_and(|assets| assets.poses.contains_key(&link))
            }),
        );
        let can_land =
            near_ground && jump::lands_on_contact(vertical_speed_before) && !owns_landing;
        let timed_out = air.air.air_time >= jump::MAX_AIR_TIME && near_ground && !owns_landing;
        if let Some(was_ground_mode) = recovery_mode {
            let vertical_contact = air.mover_vertical_contact;
            let next_ground_mode = vertical_contact
                && (was_ground_mode || jump::lands_on_contact(vertical_speed_before));
            air.recovery_ground_mode = Some(next_ground_mode);

            air.ground_contact = mover_floor_contact;
            air.air.airborne = !(next_ground_mode && mover_floor_contact);
            if next_ground_mode {
                land_jump_state(&mut sm, &mut combat);
            }
        } else if can_land || timed_out {
            if transform.translation.y <= ground_y + jump::LAND_EPS {
                transform.translation.y = ground_y;
            }
            finish_air_landing(&mut air, &mut wall);
            air.ground_entry_blend_pending = true;
            land_jump_state(&mut sm, &mut combat);
        } else if near_ground {
            air.air.cling_armed = false;
        }

        if air.air.airborne
            && ordinary_locomotion
            && !attacking
            && !wall.cling_active
            && air.air.cling_armed
            && air.air.air_time >= WALL_CLING_DELAY
            && input.z > 0.1
            && air.air.vertical_speed >= 0.0
        {
            let cling_pos = Point3::new(
                transform.translation.x,
                transform.translation.y,
                transform.translation.z,
            );
            if let Some((cling_pose, contact)) = try_enter_wall_cling(
                &mover,
                cling_pos,
                facing,
                Vec3::new(air.air.planar_x, 0.0, air.air.planar_z),
                input.x,
            ) {
                if !apply_wall_cling_pose_now(
                    assets.as_deref(),
                    common_amb.as_deref(),
                    fighter,
                    &mut pose,
                    &mut blend,
                    &mut air,
                    cling_pose,
                ) {
                    air.motion_root_pin = Some([
                        air.stand_motion_root_x,
                        air.stand_motion_root_y,
                        air.pin_root_z,
                    ]);
                    let accepted = transform.translation - tick_start;
                    motion.velocity =
                        accepted_motion_velocity(accepted.x, accepted.y, accepted.z, dt);
                    continue;
                }
                let before_projection =
                    wall_run::Vec3::new(air.air.planar_x, air.air.vertical_speed, air.air.planar_z);
                let wall_normal =
                    wall_run::Vec3::new(contact.normal.x, contact.normal.y, contact.normal.z);
                let external = wall_run::wall_cling_external_accel(before_projection, wall_normal);
                wall.cling_active = true;
                wall.cling_pose = cling_pose;
                wall.wall_normal = contact.normal;
                wall.contact_seen = false;
                wall.kick_pose = 0;
                wall.remaining = wall_run::WALL_CLING_MAX_TIME;
                wall.external_accel = Vec3::new(external.x, external.y, external.z);
                air.air.cling_armed = false;
                air.air.motion_mode = AirMotionMode::Wall;
                air.ground_contact = false;
                let into = air.air.planar_x * contact.normal.x
                    + air.air.vertical_speed * contact.normal.y
                    + air.air.planar_z * contact.normal.z;
                air.air.planar_x -= contact.normal.x * into;
                air.air.planar_z -= contact.normal.z * into;
                air.air.vertical_speed -= contact.normal.y * into;
                motion.movement_mode = MovementMode::WallRun;

                motion.velocity =
                    Vec3::new(air.air.planar_x, air.air.vertical_speed, air.air.planar_z);
                continue;
            }
        }

        if air.air.airborne
            && !attacking
            && vertical_speed_before < jump::FALL_POSE_SPEED
            && weapon_switch.is_none()
            && !air.attack_ground_pose_pending
            && !jump::fall_pose_keep(pose.pose_id)
            && !pose.is_fall_hold()
        {
            if let Some(character_assets) = character_assets {
                let fall_id = jump::fall_pose_for(pose.pose_id);
                if character_assets.poses.contains_key(&fall_id) && fall_id != pose.pose_id {
                    character_assets.apply_pose_with_blend(
                        &mut pose,
                        &mut blend,
                        fall_id,
                        true,
                        None,
                        EntryBlendKind::Generic,
                        false,
                        false,
                        None,
                    );
                    air.motion_root_pin_pose_id = fall_id;
                    air.late_visual_pose_snap = false;
                }
            }
        }

        if air.air.airborne {
            motion.movement_mode = MovementMode::Airborne;
            air.motion_root_pin = Some([
                air.stand_motion_root_x,
                air.stand_motion_root_y,
                air.pin_root_z,
            ]);
        } else {
            motion.movement_mode = if crouching {
                MovementMode::Crouch
            } else if input.length_squared() > 0.0 {
                if running {
                    MovementMode::Run
                } else {
                    MovementMode::Walk
                }
            } else {
                MovementMode::Idle
            };
            air.motion_root_pin = None;
            air.jump_root_xy_started = false;
        }
        let accepted = transform.translation - tick_start;
        motion.velocity = if recovery_mode.is_some() {
            Vec3::new(air.air.planar_x, air.air.vertical_speed, air.air.planar_z)
        } else if air.air.airborne {
            accepted_motion_velocity(accepted.x, accepted.y, accepted.z, dt)
        } else {
            Vec3::new(air.landed_planar.x, 0.0, air.landed_planar.y)
        };

        if ground_mode && !playback.as_ref().is_some_and(|p| p.active) {
            if let Some(target) = lock.lock_entity.and_then(|e| targets.get(e).ok()) {
                apply_ground_lock_facing(
                    &mut lock,
                    &mut basis,
                    &mut motion,
                    transform.translation,
                    target.translation,
                );
            }
        }
        commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);
    }
}

type AirCurveMotionQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static mut Transform,
        &'static mut MotionState,
        &'static mut FighterAir,
        &'static mut PoseRuntime,
        &'static mut crate::components::fighter::EntryBlend,
        &'static mut SmState,
        &'static mut CombatState,
        Option<&'static mut FighterAttackMotion>,
        &'static FighterHitReaction,
        Option<&'static mut WeaponSwitchActive>,
        Option<&'static LocalFighter>,
    ),
    (With<Fighter>, Without<RemoteFighter>),
>;

#[allow(clippy::too_many_arguments)]
fn air_curve_root_motion(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    common_amb: Option<Res<crate::plugins::character_plugin::CommonAmb>>,
    collider: Res<StageCollider>,
    mut contacts: Option<ResMut<CharacterContactFrame>>,
    mut query: AirCurveMotionQuery<'_, '_>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    if dt <= 0.0 {
        return;
    }
    let (Some(assets), Some(common), Some(mesh)) = (assets, common_amb, collider.mesh.as_ref())
    else {
        return;
    };
    let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    mover.set_mesh_collider(mesh);

    let mut ordered = query
        .iter_mut()
        .map(|(entity, fighter, _, _, _, _, _, _, _, _, _, _, local)| {
            (
                local.is_none(),
                fighter.player_slot,
                entity.to_bits(),
                entity,
            )
        })
        .collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|&(non_local, slot, bits, _)| (non_local, slot, bits));

    for (_, _, _, entity) in ordered {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        let Ok((
            _,
            fighter,
            mut transform,
            mut motion,
            mut air,
            mut pose,
            mut blend,
            mut sm,
            mut combat,
            mut attack_motion,
            hit_reaction,
            mut switch,
            _,
        )) = query.get_mut(entity)
        else {
            continue;
        };
        if !air.air.airborne || hit_reaction.current.is_some() {
            continue;
        }
        let tick_start = transform.translation;
        let tick_ground = sample_air_ground(
            &mut air,
            &mover,
            Point3::new(tick_start.x, tick_start.y, tick_start.z),
        );
        let tick_ground_y = tick_ground.map_or(f32::NEG_INFINITY, |hit| hit.y);
        let switching_weapon_air = switch.is_some() && pose.pose_id == WEAPON_SWITCH_POSE_AIR;
        if !air_curve_motion_owned(air.air.motion_mode, switching_weapon_air) {
            continue;
        }
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        let attacking = is_attacking_state(sm.current);

        if air.air_attack_recovery_pose == Some(pose.pose_id) && pose.finished() {
            if let Some(motion) = attack_motion.as_deref_mut() {
                motion.air_step_pending = false;
            }
            continue;
        }

        if !(blend.duration > 0.0 && blend.pose_id == pose.pose_id) {
            release_air_attack_hold_on_contact(&mut pose, attacking, air.mover_vertical_contact);
        }
        let attack_planar_owned = attacking
            || attack_motion
                .as_deref()
                .is_some_and(|motion| motion.air_step_pending);
        if pose.pose_id != air.air.vertical_pose_id {
            air.air.vertical_pose_id = pose.pose_id;
            air.air.prev_root_z = None;
            if air.air.launch_pose_id != 0 && pose.pose_id != air.air.launch_pose_id {
                air.air.launch_pose_id = 0;
                air.air.launch_lifted = true;
            }
        }

        let motion_root = character_assets.motion_root_at(&common.0, pose.pose_id, pose.frame);
        let root_z = motion_root[2];

        let anim_step_src = if blend.duration > 0.0 && blend.pose_id == pose.pose_id {
            0.0
        } else {
            character_assets.authored_root_step(&common.0, &pose, dt)[2]
        };
        air.air.prev_root_z = Some(root_z);
        let anim_speed_src = anim_step_src / dt;
        let curve_active = matches!(
            air.air.motion_mode,
            AirMotionMode::Curve | AirMotionMode::CurveExit
        );
        let anim_driven = curve_active && anim_speed_src.abs() >= 1.0;
        let launching = air.air.launch_pose_id == pose.pose_id && air.air.launch_pose_id != 0;

        if switching_weapon_air {
            air.jump_root_xy_started = false;
            air.jump_prev_pose_id = pose.pose_id;
            let (px, pz) =
                jump::apply_air_weapon_switch_damping(air.air.planar_x, air.air.planar_z, dt);
            air.air.planar_x = px;
            air.air.planar_z = pz;
        } else {
            apply_jump_root_horizontal(
                entity,
                contacts.as_deref_mut(),
                &mut air,
                &pose,
                &motion,
                &mut transform,
                &mut mover,
                motion_root[0],
                motion_root[1],
                attack_planar_owned,
            );
        }

        let vertical_speed_before = air.air.vertical_speed;
        if !attack_planar_owned {
            consume_descent_slide(&mut air);
        }
        let mut world_disp = match select_air_vertical_path(
            anim_driven,
            launching,
            air.air.launch_lifted,
            switching_weapon_air,
        ) {
            AirVerticalPath::AuthoredCurve => {
                air.air.air_time += dt;
                air.air.vertical_speed = anim_speed_src * MODEL_SCALE;
                anim_step_src * MODEL_SCALE
            }
            AirVerticalPath::LaunchChargeHold => {
                air.air.air_time += dt;
                air.air.vertical_speed = 0.0;
                0.0
            }
            AirVerticalPath::WeaponSwitchGravity => {
                air.air.air_time += dt;
                let (new_v, disp, _, _) = jump::integrate_vertical(
                    air.air.vertical_speed,
                    air.air.planar_x,
                    air.air.planar_z,
                    dt,
                    1.0,
                    false,
                );
                air.air.vertical_speed = new_v;
                disp
            }
            AirVerticalPath::Ballistic => air.air.tick_vertical(dt, false),
        };
        if launching && anim_driven && anim_speed_src > 0.0 && !air.air.launch_lifted {
            air.air.launch_lifted = true;
            air.air.air_time = 0.0;
        }

        let (vx, vy, vz, capped_world_disp) = if attack_planar_owned {
            let (planar_dx, planar_dz) = attack_motion
                .as_deref_mut()
                .filter(|motion| motion.air_step_pending)
                .map(|motion| {
                    motion.air_step_pending = false;
                    (motion.pending_air_dx, motion.pending_air_dz)
                })
                .unwrap_or((0.0, 0.0));
            let (dx, dy, dz, scale) =
                clamp_composed_delta_3d(planar_dx, world_disp, planar_dz, dt, MOVER_MAX_SPEED);
            (dx / dt, air.air.vertical_speed * scale, dz / dt, dy)
        } else {
            clamp_air_motion_step(
                air.air.planar_x,
                air.air.vertical_speed,
                air.air.planar_z,
                world_disp,
            )
        };
        air.air.planar_x = vx;
        air.air.vertical_speed = vy;
        air.air.planar_z = vz;
        world_disp = capped_world_disp;

        let pos = Point3::new(
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
        );
        if mover.blocks_motion_sweep(pos, Point3::new(vx * dt, world_disp, vz * dt)) {
            air.air.planar_x = 0.0;
            air.air.planar_z = 0.0;
            air.air.vertical_speed = 0.0;
            motion.velocity = Vec3::ZERO;
            if let Some(attack_motion) = attack_motion.as_deref_mut() {
                attack_motion.carry_x = 0.0;
                attack_motion.carry_z = 0.0;
            }

            continue;
        }
        let requested_x = vx * dt;
        let requested_z = vz * dt;
        let (rx, rz, ry, planar_surface) =
            super::character_contact_plugin::resolve_fighter_xz_with_surface(
                contacts.as_deref_mut(),
                entity,
                &mover,
                pos,
                requested_x,
                requested_z,
                true,
            );
        project_planar_air_velocity_on_contact(&mut air.air, requested_x, requested_z, rx, rz);
        if attack_planar_owned {
            if let Some(attack_motion) = attack_motion.as_deref_mut() {
                if rx == 0.0 && requested_x != 0.0 {
                    attack_motion.carry_x = 0.0;
                }
                if rz == 0.0 && requested_z != 0.0 {
                    attack_motion.carry_z = 0.0;
                }
            }
        }
        transform.translation.x += rx;
        transform.translation.z += rz;
        let vertical_origin = Point3::new(
            transform.translation.x,
            transform.translation.y + ry,
            transform.translation.z,
        );
        let (dy, foot, surface, blocked) = if world_disp >= 0.0 {
            let (dy, hit, blocked) = mover.resolve_y(vertical_origin, world_disp);
            (
                dy,
                None::<FootHit>,
                hit.map(|hit| (hit.nx, hit.ny, hit.nz)),
                blocked,
            )
        } else {
            let (dy, hit, blocked) =
                mover.resolve_descent_with_surface(vertical_origin, world_disp, planar_surface);
            (dy, hit, hit.map(|hit| (hit.nx, hit.ny, hit.nz)), blocked)
        };
        air.mover_vertical_contact = surface.is_some() || blocked;
        let contact_speed = air.air.vertical_speed;
        update_descent_slide(&mut air, foot, contact_speed, dt);
        record_carry_support(
            &mut air,
            world_disp,
            surface,
            planar_surface.is_some()
                || (rx - requested_x).abs() > 1e-5
                || (rz - requested_z).abs() > 1e-5,
        );
        if let Some((nx, ny, nz)) = surface {
            project_air_velocity_on_surface(&mut air.air, nx, ny, nz);
        }
        if blocked {
            air.air.vertical_speed = 0.0;
        }
        transform.translation.y += ry + dy;
        transform.translation.y = transform.translation.y.max(tick_ground_y);

        let ground_y = tick_ground_y;
        let mover_floor_contact =
            world_disp < 0.0 && meteor_physics::collision::descent_has_support(foot, blocked);
        let near_ground =
            transform.translation.y <= ground_y + jump::LAND_EPS || mover_floor_contact;
        air.peak_lift = air
            .peak_lift
            .max((transform.translation.y - air.ground_y).max(0.0));
        air.ground_contact = near_ground;
        let authored_link = character_assets
            .poses
            .get(&pose.pose_id)
            .and_then(|pose| pose.link);
        let owns_landing = attack_clip_owns_landing(
            attacking,
            pose.is_fall_hold(),
            authored_link,
            authored_link.is_some_and(|link| character_assets.poses.contains_key(&link)),
        );
        let can_land = near_ground
            && air.air_attack_recovery_pose != Some(pose.pose_id)
            && !launching
            && jump::lands_on_contact(vertical_speed_before)
            && !owns_landing;
        let timed_out = air.air.air_time >= jump::MAX_AIR_TIME
            && air.air_attack_recovery_pose != Some(pose.pose_id)
            && (!launching || air.air.launch_lifted)
            && near_ground
            && !owns_landing;

        if can_land || timed_out {
            if transform.translation.y <= ground_y + jump::LAND_EPS {
                transform.translation.y = ground_y;
            }
            finish_air_landing_state(&mut air);
            air.ground_entry_blend_pending = !attacking;
            if switching_weapon_air {
                if let Some(switch) = switch.as_deref_mut() {
                    let _ = handoff_landed_air_weapon_switch(switch);
                }
            }
            if attacking {
                let recovery_pose =
                    authored_link.filter(|&link| character_assets.poses.contains_key(&link));
                if let Some(linked_pose) =
                    land_air_attack_state(&mut sm, &mut combat, pose.pose_id, recovery_pose)
                {
                    character_assets.apply_pose_with_blend(
                        &mut pose,
                        &mut blend,
                        linked_pose,
                        true,
                        None,
                        EntryBlendKind::Generic,
                        false,
                        false,
                        Some(meteor_combat::weapon_strategy::weapon_to_strategy(
                            fighter.weapon,
                        )),
                    );
                }
                motion.movement_mode = MovementMode::Idle;
            } else {
                land_jump_state(&mut sm, &mut combat);
                motion.movement_mode = MovementMode::Idle;
            }
        } else {
            if near_ground {
                air.air.cling_armed = false;
            }
            if air.air.motion_mode == AirMotionMode::CurveExit
                && air.air.vertical_speed < AirState::CURVE_EXIT_SPEED
            {
                air.air.motion_mode = AirMotionMode::Ballistic;
            }
            if air.air.motion_mode == AirMotionMode::Ballistic
                && !attacking
                && !switching_weapon_air
                && contact_speed < jump::FALL_POSE_SPEED
                && !air.attack_ground_pose_pending
                && !jump::fall_pose_keep(pose.pose_id)
                && !pose.is_fall_hold()
            {
                let fall_id = jump::fall_pose_for(pose.pose_id);
                if character_assets.poses.contains_key(&fall_id) && fall_id != pose.pose_id {
                    character_assets.apply_pose_with_blend(
                        &mut pose,
                        &mut blend,
                        fall_id,
                        true,
                        None,
                        crate::components::fighter::EntryBlendKind::Generic,
                        false,
                        false,
                        None,
                    );
                    air.motion_root_pin_pose_id = fall_id;
                    air.late_visual_pose_snap = false;
                }
            }
            motion.movement_mode = MovementMode::Airborne;
            air.motion_root_pin = Some([
                air.stand_motion_root_x,
                air.stand_motion_root_y,
                air.pin_root_z,
            ]);
        }
        let accepted = transform.translation - tick_start;
        motion.velocity = accepted_motion_velocity(accepted.x, accepted.y, accepted.z, dt);
        commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);
    }
}

type DodgeRootMotionQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static FighterAir,
        &'static mut Transform,
        &'static mut MotionState,
        &'static mut FighterDodge,
        &'static PoseRuntime,
        &'static mut SmState,
        &'static mut CombatState,
        Option<&'static LocalFighter>,
        Option<&'static NpcPerformance>,
    ),
    (With<Fighter>, Without<RemoteFighter>),
>;

#[allow(clippy::too_many_arguments)]
pub(crate) fn dodge_root_motion(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    collider: Res<StageCollider>,
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    common_amb: Option<Res<crate::plugins::character_plugin::CommonAmb>>,
    mut contacts: Option<ResMut<CharacterContactFrame>>,
    guard_input: Res<LocalGuardInput>,
    mut query: DodgeRootMotionQuery<'_, '_>,
) {
    let Some(assets) = assets else {
        return;
    };
    let Some(common_amb) = common_amb else {
        return;
    };
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    if dt <= 0.0 {
        return;
    }
    let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    if let Some(mesh) = collider.mesh.as_ref() {
        mover.set_mesh_collider(mesh);
    }

    for (
        entity,
        fighter,
        air,
        mut transform,
        mut motion,
        mut dodge,
        pose,
        mut sm,
        mut combat,
        local,
        npc_performance,
    ) in &mut query
    {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        if !dodge.state.active {
            continue;
        }
        if pose.pose_id == dodge.pose {
            dodge.pose_entered = true;
        } else if dodge.pose_entered {
            dodge.state.stop();
            dodge.pose_entered = false;
            motion.movement_mode = MovementMode::Idle;
            if matches!(sm.current, FighterState::Dodge | FighterState::Burst) {
                let guard_held = if local.is_some() {
                    guard_input.held
                } else {
                    npc_performance.is_some_and(|performance| performance.guard_hold() > 0.0)
                };
                finish_dodge_state(&mut sm, &mut combat, guard_held);
            }
            continue;
        } else {
            continue;
        }

        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };

        let motion_root = character_assets.motion_root_at(&common_amb.0, pose.pose_id, pose.frame);
        let (root_x, root_y) = (motion_root[0], motion_root[1]);
        let mut anim_x = 0.0;
        let mut anim_z = 0.0;
        if !dodge.state.root_started {
            dodge.state.prev_root_x = root_x;
            dodge.state.prev_root_y = root_y;
            dodge.state.root_started = true;
        } else {
            let (world_x, world_z) = meteor_anim::root_motion::root_delta_to_world_xz(
                root_x - dodge.state.prev_root_x,
                root_y - dodge.state.prev_root_y,
                motion.facing,
                meteor_physics::dodge::DODGE_ROOT_MOTION_SCALE,
            );
            dodge.state.prev_root_x = root_x;
            dodge.state.prev_root_y = root_y;
            let axis = dodge.state.motion_axis.unwrap_or_else(|| {
                let axis = if world_x.abs() >= world_z.abs() {
                    (
                        if world_x == 0.0 {
                            1.0
                        } else {
                            world_x.signum()
                        },
                        0.0,
                    )
                } else {
                    (
                        0.0,
                        if world_z == 0.0 {
                            1.0
                        } else {
                            world_z.signum()
                        },
                    )
                };
                dodge.state.motion_axis = Some(axis);
                axis
            });
            dodge.state.raw_progress += world_x * axis.0 + world_z * axis.1;
            let next = dodge.state.raw_progress.max(dodge.state.progress);
            let progress_delta = next - dodge.state.progress;
            dodge.state.progress = next;
            anim_x = axis.0 * progress_delta;
            anim_z = axis.1 * progress_delta;
        }

        let (residual_x, residual_z, requested_x, requested_z) = compose_dodge_planar_step(
            Vec2::new(anim_x, anim_z),
            Vec2::new(dodge.state.residual_x, dodge.state.residual_z),
            Vec2::new(
                air.ground_external_velocity.x,
                air.ground_external_velocity.z,
            ),
            action_carry_decay(air),
            dt,
        );
        dodge.state.residual_x = residual_x;
        dodge.state.residual_z = residual_z;
        let carry_x = residual_x * dt;
        let carry_z = residual_z * dt;
        let pos = Point3::new(
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
        );
        let (accepted_x, accepted_z, accepted_y) = resolve_fighter_xz(
            contacts.as_deref_mut(),
            entity,
            &mover,
            pos,
            requested_x,
            requested_z,
            false,
        );
        transform.translation.x += accepted_x;
        transform.translation.z += accepted_z;
        transform.translation.y += accepted_y;
        if accepted_x == 0.0 && carry_x != 0.0 && requested_x != 0.0 {
            dodge.state.residual_x = 0.0;
        }
        if accepted_z == 0.0 && carry_z != 0.0 && requested_z != 0.0 {
            dodge.state.residual_z = 0.0;
        }
        motion.velocity = Vec3::new(accepted_x / dt, 0.0, accepted_z / dt);
        commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);

        if dodge_pose_finished(pose) {
            dodge.state.stop();
            dodge.pose_entered = false;
            motion.movement_mode = MovementMode::Idle;
            if matches!(sm.current, FighterState::Dodge | FighterState::Burst) {
                let guard_held = if local.is_some() {
                    guard_input.held
                } else {
                    npc_performance.is_some_and(|performance| performance.guard_hold() > 0.0)
                };
                finish_dodge_state(&mut sm, &mut combat, guard_held);
            }
        }
    }
}

fn apply_ground_lock_facing(
    lock: &mut LockOnController,
    basis: &mut CameraBasis,
    motion: &mut MotionState,
    player: Vec3,
    target: Vec3,
) {
    let player = player.to_array().into();
    let target = target.to_array().into();
    if meteor_combat::lock_on::locked_attack_gate_failed(player, target) {
        lock.system.clear_lock();
        lock.lock_entity = None;
        basis.yaw = motion.facing;
    } else if let Some(yaw) = meteor_combat::lock_on::locked_attack_facing_yaw(player, target) {
        motion.facing = yaw;
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn move_local_fighter(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut basis: ResMut<CameraBasis>,
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    common_amb: Option<Res<crate::plugins::character_plugin::CommonAmb>>,
    collider: Res<StageCollider>,
    mut contacts: Option<ResMut<CharacterContactFrame>>,
    mut lock: ResMut<LockOnController>,
    buffs: Res<crate::plugins::item_plugin::ItemBuffs>,
    player_performance: Option<Res<crate::plugins::stage_plugin::PlayerPerformance>>,
    mut aux_input: MovementAuxInput<'_>,
    keymap: Res<crate::app_state::Keymap>,
    mut query: LocalFighterQuery<'_, '_>,
    targets: Query<&Transform, Without<LocalFighter>>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());

    let jump_wall_dt = frame_clock.as_deref().map_or_else(
        || time.delta_secs().max(0.0),
        |clock| clock.wall_dt.max(0.0),
    );
    let Some(mesh) = &collider.mesh else {
        return;
    };
    let mut sys = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    sys.set_mesh_collider(mesh);
    let replay_facing_locked = aux_input
        .mrc_playback
        .as_ref()
        .is_some_and(|playback| playback.active);
    let replay_pose_frame = aux_input
        .mrc_frame
        .as_ref()
        .is_some_and(|frame| frame.position_applied);
    let crouch_held = aux_input.crouch_input.held;
    let deferred_air_root_motion = aux_input.deferred_air_root_motion.is_some();
    let (mut desktop_dodge_request, desktop_dodge_state_accepted, desktop_dodge_roll) =
        if let Some(request) = aux_input.desktop_dodge.as_deref_mut() {
            let pending = request.pending.take();
            let state_accepted = pending.is_some() && request.pending_state_accepted;
            let roll = if pending.is_some() {
                request.pending_roll.take()
            } else {
                None
            };
            request.pending_state_accepted = false;
            (pending, state_accepted, roll)
        } else {
            (None, false, None)
        };

    for (
        entity,
        fighter,
        mut transform,
        mut motion,
        mut air,
        mut dodge,
        mut wall,
        mut pose,
        mut blend,
        mut sm,
        mut combat_state,
        gun_aim,
        (ranged_gate, status_effects),
        mut hit_reaction,
        weapon_switch,
    ) in &mut query
    {
        air.locomotion_pose_sample = None;
        if dodge.state.active || hit_reaction.current.is_some() || weapon_switch.is_some() {
            air.recovery_ground_mode = None;
        }
        let input_blocked = player_performance
            .as_deref()
            .is_some_and(|performance| performance.input_blocked);
        let raw_input = if input_blocked {
            Vec3::ZERO
        } else {
            directional_input(&keys, &mouse, &keymap)
        };

        if matches!(sm.current, FighterState::Fake | FighterState::Down)
            && raw_input.length_squared() > 0.01
            && !dodge.state.active
            && !air.air.airborne
            && !wheel_vetoes_dodge_action(ranged_gate.wheel_out, false)
        {
            let dodge_pose = dodge_pose_for_input(
                &keys,
                &mouse,
                &keymap,
                meteor_combat::weapon_strategy::weapon_to_strategy(fighter.weapon),
                true,
            );
            enter_dodge_state(&mut sm, &mut combat_state, false);
            hit_reaction.clear();
            dodge.state.start(
                dodge_pose,
                motion.facing,
                true,
                motion.velocity.x,
                motion.velocity.z,
            );
            dodge.pose = dodge_pose;
            dodge.pose_entered = false;
            motion.movement_mode = MovementMode::Dodge;
            let _ = apply_dodge_pose_now(
                assets.as_deref(),
                fighter,
                &mut pose,
                &mut blend,
                dodge.pose,
            );
        }
        let reaction_owns_motion = hit_reaction.current.as_ref().is_some_and(|reaction| {
            matches!(
                reaction.kind,
                meteor_combat::hit_reaction::HitReactKind::BeHit
                    | meteor_combat::hit_reaction::HitReactKind::Guard
                    | meteor_combat::hit_reaction::HitReactKind::Down
                    | meteor_combat::hit_reaction::HitReactKind::GetUp
                    | meteor_combat::hit_reaction::HitReactKind::Dead
            )
        });
        if reaction_owns_motion {
            motion.movement_mode = if air.air.airborne {
                MovementMode::Airborne
            } else {
                MovementMode::Idle
            };
            continue;
        }
        let ordinary_locomotion =
            !input_blocked && ordinary_locomotion_allowed(&sm, weapon_switch.is_some());
        let input = if ordinary_locomotion {
            raw_input
        } else {
            Vec3::ZERO
        };
        let crouching = ordinary_locomotion_allowed(&sm, weapon_switch.is_some())
            && !air.air.airborne
            && crouch_held;

        let crouch_drive =
            ordinary_locomotion && !air.air.airborne && (crouch_held || gun_aim.aiming);
        let loco = compose_loco_axes(
            input.x,
            input.z,
            if crouch_drive {
                1.0
            } else {
                FORWARD_STRAFE_SCALE
            },
        );
        let running = ordinary_locomotion
            && !crouch_drive
            && is_running(loco.running, &keys, &mouse, &keymap);
        let locomotion_input = Vec3::new(loco.local_x, 0.0, loco.local_z);

        let basis_yaw = if lock.lock_entity.is_some() {
            motion.facing
        } else {
            basis.yaw
        };
        let pos = Point3::new(
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
        );

        if input_blocked {
            air.jump_hold = 0.0;
            air.jump_charge_active = false;
            sm.exe_ground_mode_ticks = 0;
        }
        let (
            jump_pressed_edge,
            jump_held,
            jump_released_edge,
            observed_jump_hold,
            jump_action_accepted,
        ) = aux_input.jump_input.as_deref().map_or_else(
            || {
                (
                    keymap.jump.just_pressed(&keys, &mouse),
                    keymap.jump.pressed(&keys, &mouse),
                    keymap.jump.just_released(&keys, &mouse),
                    None,
                    false,
                )
            },
            |jump| {
                (
                    jump.pressed,
                    jump.held,
                    jump.released,
                    Some(jump.hold_seconds),
                    jump.action_accepted,
                )
            },
        );

        let jump_mode_accepts_key_down =
            jump_charge_key_down_allowed(sm.current, ordinary_locomotion);
        let can_pay_buffered_jump =
            !input_blocked && !ranged_gate.wheel_out && sm.current == FighterState::Name;
        let mut jump_hold = air.jump_hold;
        let mut jump_charge_active = air.jump_charge_active;
        let mut exe_ground_mode_ticks = sm.exe_ground_mode_ticks;
        let mut jump_request = update_jump_charge(
            &mut jump_hold,
            &mut jump_charge_active,
            &mut exe_ground_mode_ticks,
            jump_pressed_edge,
            jump_held,
            jump_released_edge,
            !input_blocked && !ranged_gate.wheel_out && jump_mode_accepts_key_down,
            can_pay_buffered_jump,
            observed_jump_hold,
            jump_wall_dt,
        );

        if jump_request == JumpChargeRequest::None
            && jump_charge_active
            && jump_held
            && jump_hold > STRONG_JUMP_HOLD
            && ordinary_locomotion
            && !input_blocked
            && !ranged_gate.wheel_out
        {
            jump_request = JumpChargeRequest::HeldThreshold;
            jump_charge_active = false;
            if let Some(jump) = aux_input.jump_input.as_deref_mut() {
                jump.consume_charge();
            }
        }
        air.jump_hold = jump_hold;
        air.jump_charge_active = jump_charge_active;
        sm.exe_ground_mode_ticks = exe_ground_mode_ticks;
        let jump_release = jump_release_reaches_motion(
            ordinary_locomotion,
            jump_action_accepted,
            ranged_gate.wheel_out,
            jump_request,
        );
        let buffered_ground_jump = jump_request == JumpChargeRequest::BufferedGroundMode;

        let gesture = desktop_dodge_request
            .take()
            .map(|request| (request, desktop_dodge_state_accepted, desktop_dodge_roll));
        let dodge_request = gesture;
        if !input_blocked
            && dodge_request.is_some_and(|(request, state_accepted, _)| {
                if state_accepted {
                    accepted_dodge_still_owns_state(sm.current, request.burst)
                } else {
                    can_start_dodge_action(
                        &sm,
                        air.air.airborne,
                        weapon_switch.is_some(),
                        ranged_gate.wheel_out,
                        request.burst,
                    )
                }
            })
            && !dodge.state.active
        {
            let (request, state_accepted, roll_override) = dodge_request.unwrap();
            let roll = roll_override.unwrap_or_else(|| {
                request.burst
                    || crouch_held
                    || gun_aim.aiming
                    || matches!(sm.current, FighterState::Fake | FighterState::Down)
            });
            let dodge_pose = dodge_pose_for_axes(
                request.direction,
                meteor_combat::weapon_strategy::weapon_to_strategy(fighter.weapon),
                roll,
            );
            dodge.state.start(
                dodge_pose,
                motion.facing,
                roll,
                motion.velocity.x,
                motion.velocity.z,
            );
            dodge.pose = dodge_pose;
            dodge.pose_entered = false;
            motion.movement_mode = MovementMode::Dodge;
            if !state_accepted {
                enter_dodge_state(&mut sm, &mut combat_state, request.burst);
            }
            let _ = apply_dodge_pose_now(
                assets.as_deref(),
                fighter,
                &mut pose,
                &mut blend,
                dodge.pose,
            );
        }

        sync_fighter_contact_state(
            contacts.as_deref_mut(),
            entity,
            transform.translation,
            pose.pose_id,
            sm.current,
            motion.movement_mode,
            air.air.airborne || hit_reaction.airborne,
        );

        if !ordinary_locomotion
            && dodge.state.active
            && !matches!(sm.current, FighterState::Dodge | FighterState::Burst)
        {
            dodge.state.stop();
            dodge.pose_entered = false;
        }

        let ground_hit = sys.sample_ground_hit(pos.x, pos.z, pos.y);
        if !air.air.airborne {
            if let Some(hit) = ground_hit {
                air.ground_y = hit.y;
            }
        }
        if !air.air.airborne && air.recovery_ground_mode.is_none() && !replay_pose_frame {
            let supported = ground_hit.is_some_and(|hit| {
                pos.y - hit.y <= jump::STEP_DOWN && sys.can_occupy(Point3::new(pos.x, hit.y, pos.z))
            });
            if !supported {
                let (dy, foot, blocked) = sys.resolve_descent(pos, -jump::STEP_DOWN);
                let walkable = meteor_physics::collision::descent_has_support(foot, blocked);
                if walkable {
                    transform.translation.y += dy.min(0.0);
                    air.pending_ledge_fall = false;
                } else {
                    if deferred_air_root_motion {
                        air.pending_ledge_fall = true;
                    } else {
                        air.air.start_fall(motion.velocity.x, motion.velocity.z);
                        air.ground_contact = false;
                        air.ledge_fall = true;
                        air.peak_lift = (pos.y - air.ground_y).max(0.0);
                        motion.movement_mode = MovementMode::Airborne;
                    }
                }
            } else {
                transform.translation.y = ground_hit.expect("checked support").y;
                air.pending_ledge_fall = false;
            }
        }
        let pos = Point3::new(
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
        );
        let on_ground = !air.air.airborne || air.recovery_ground_mode == Some(true);
        let mut took_off_this_tick = false;

        if ((jump_release && on_ground) || buffered_ground_jump) && !dodge.state.active {
            let preserve_post_jump_action =
                preserve_post_jump_action(jump_action_accepted, sm.current);
            let strong = air.jump_hold >= STRONG_JUMP_HOLD;
            let jump_pose = meteor_physics::locomotion::jump_pose_for_input(input.x, input.z);

            let impulse = jump::jump_impulse(JumpStartInput {
                carry_x: motion.velocity.x,
                carry_z: motion.velocity.z,
            });
            air.recovery_ground_mode = None;
            start_jump_with_velocity_transfer(&mut air.air, &mut motion, strong, impulse);
            took_off_this_tick = true;

            air.peak_lift = 0.0;
            motion.movement_mode = MovementMode::Airborne;
            air.ledge_fall = false;
            if !preserve_post_jump_action {
                enter_jump_state(&mut sm, &mut combat_state);
            }

            if !preserve_post_jump_action {
                if let Some(character_assets) = assets
                    .as_ref()
                    .and_then(|a| a.0.get(&format!("p{}", fighter.character_id)))
                {
                    if let Some(common) = common_amb.as_ref() {
                        let start_frame = character_assets
                            .poses
                            .get(&jump_pose)
                            .map(|def| def.start as f32)
                            .unwrap_or(0.0);
                        let start_root =
                            character_assets.motion_root_at(&common.0, jump_pose, start_frame);
                        snap_jump_motion_baseline(&mut air, start_root, jump_pose);
                    }
                    character_assets.apply_pose_with_blend(
                        &mut pose,
                        &mut blend,
                        jump_pose,
                        true,
                        None,
                        crate::components::fighter::EntryBlendKind::Generic,
                        true,
                        false,
                        Some(meteor_combat::weapon_strategy::weapon_to_strategy(
                            fighter.weapon,
                        )),
                    );
                }
            }
            air.jump_hold = 0.0;
            air.jump_charge_active = false;
            if buffered_ground_jump {
                if let Some(jump) = aux_input.jump_input.as_deref_mut() {
                    jump.consume_charge();
                }
            }
        }

        if dodge.state.active {
            continue;
        }

        if air.air.airborne || air.recovery_ground_mode.is_some() {
            let tick_start = transform.translation;
            let tick_ground = sample_air_ground(
                &mut air,
                &sys,
                Point3::new(tick_start.x, tick_start.y, tick_start.z),
            );
            let tick_ground_y = tick_ground.map_or(f32::NEG_INFINITY, |hit| hit.y);
            let facing = Vec3::new(motion.facing.sin(), 0.0, motion.facing.cos());
            let attacking = is_attacking_state(sm.current);

            if jump_release && !took_off_this_tick && !dodge.state.active {
                let strong = air.jump_hold >= STRONG_JUMP_HOLD;
                let before_kick = air.air.clone();
                if let Some(kick_pose) = try_wall_jump(
                    &mut air.air,
                    &mut wall,
                    &sys,
                    pos,
                    facing,
                    pose.pose_id,
                    strong,
                ) {
                    if apply_wall_kick_pose_now(
                        assets.as_deref(),
                        common_amb.as_deref(),
                        fighter,
                        &mut pose,
                        &mut blend,
                        &mut air,
                        kick_pose,
                    ) {
                        wall.kick_pose = kick_pose;
                        wall.cling_active = false;
                        wall.contact_seen = false;
                        wall.external_accel = Vec3::ZERO;
                        air.air.motion_mode = AirMotionMode::Ballistic;

                        air.air.cling_armed = true;
                        air.air.air_time = 0.0;
                        air.peak_lift = 0.0;
                        air.jump_hold = 0.0;
                        motion.movement_mode = MovementMode::Airborne;
                    } else {
                        air.air = before_kick;
                    }
                }
            }

            if deferred_air_root_motion {
                motion.movement_mode = if wall.cling_active {
                    MovementMode::WallRun
                } else {
                    MovementMode::Airborne
                };
                air.motion_root_pin = Some([
                    air.stand_motion_root_x,
                    air.stand_motion_root_y,
                    air.pin_root_z,
                ]);
                continue;
            }

            if wall.cling_active {
                wall.remaining -= dt;
                air.air.air_time += dt;
                let mut touching = false;
                if input.z > 0.1 && wall.remaining > 0.0 {
                    let p = Point3::new(
                        transform.translation.x,
                        transform.translation.y,
                        transform.translation.z,
                    );
                    let result = tick_wall_cling(
                        entity,
                        contacts.as_deref_mut(),
                        &mut air.air,
                        &mut wall,
                        &sys,
                        p,
                        dt,
                    );
                    transform.translation =
                        Vec3::new(result.position.x, result.position.y, result.position.z);
                    touching = result.flags != 0;
                    let vy = result.vertical_speed_before_contact;
                    update_descent_slide(&mut air, result.vertical_surface, vy, dt);
                }
                if !touching {
                    release_wall_motion(&mut air, &mut wall, pose.pose_id, transform.translation.y);
                }
                motion.movement_mode = if wall.cling_active {
                    MovementMode::WallRun
                } else {
                    MovementMode::Airborne
                };
                air.ground_contact = false;
                motion.velocity =
                    Vec3::new(air.air.planar_x, air.air.vertical_speed, air.air.planar_z);
                commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);
                continue;
            }

            let switching_weapon_air =
                weapon_switch.is_some() && pose.pose_id == WEAPON_SWITCH_POSE_AIR;
            if attacking
                && !matches!(
                    air.air.motion_mode,
                    AirMotionMode::Curve | AirMotionMode::CurveExit
                )
            {
                air.air.start_curve(false, pose.pose_id);
            }
            if air_curve_motion_owned(air.air.motion_mode, switching_weapon_air) {
                motion.movement_mode = MovementMode::Airborne;
                motion.velocity = Vec3::new(air.air.planar_x, 0.0, air.air.planar_z);
                air.motion_root_pin = Some([
                    air.stand_motion_root_x,
                    air.stand_motion_root_y,
                    air.pin_root_z,
                ]);
                continue;
            }

            if attacking
                && air.air.motion_mode != AirMotionMode::Wall
                && air.air.motion_mode != AirMotionMode::Curve
                && air.air.motion_mode != AirMotionMode::CurveExit
            {
                air.air.start_curve(false, pose.pose_id);
            }

            let (root_z_src, curve_consumed_dy, root_x_src, root_y_src) =
                match (assets.as_ref(), common_amb.as_ref()) {
                    (Some(a), Some(c)) => match a.0.get(&format!("p{}", fighter.character_id)) {
                        Some(character_assets) => {
                            let vertical_sample_pose_id = pose.pose_id;
                            if vertical_sample_pose_id != air.air.vertical_pose_id {
                                air.air.vertical_pose_id = vertical_sample_pose_id;
                                air.air.prev_root_z = None;

                                if air.air.launch_pose_id != 0
                                    && pose.pose_id != air.air.launch_pose_id
                                {
                                    air.air.launch_pose_id = 0;
                                    air.air.launch_lifted = true;
                                }
                            }
                            let motion_root =
                                character_assets.motion_root_at(&c.0, pose.pose_id, pose.frame);
                            (motion_root[2], false, motion_root[0], motion_root[1])
                        }
                        None => (0.0, true, 0.0, 0.0),
                    },
                    _ => (0.0, true, 0.0, 0.0),
                };

            let _ = (root_x_src, root_y_src);
            air.jump_root_xy_started = false;
            air.jump_prev_pose_id = pose.pose_id;

            let curve_active = matches!(
                air.air.motion_mode,
                AirMotionMode::Curve | AirMotionMode::CurveExit
            );
            let anim_step_src = match air.air.prev_root_z {
                Some(prev) => root_z_src - prev,
                None => 0.0,
            };
            let anim_speed_src = if dt > 1e-6 { anim_step_src / dt } else { 0.0 };
            air.air.prev_root_z = Some(root_z_src);
            let anim_driven = curve_active && anim_speed_src.abs() >= 1.0 && !curve_consumed_dy;
            let launching = air.air.launch_pose_id == pose.pose_id && air.air.launch_pose_id != 0;

            if switching_weapon_air {
                let (px, pz) =
                    jump::apply_air_weapon_switch_damping(air.air.planar_x, air.air.planar_z, dt);
                air.air.planar_x = px;
                air.air.planar_z = pz;
            } else {
                tick_air_input_drive(
                    &mut air,
                    dt,
                    input,
                    motion.facing,
                    ordinary_locomotion && crouch_held,
                    status_effects.effects.speed_factor(buffs.speed_factor),
                    ranged_gate.wheel_out,
                );
            }
            let vertical_speed_before = air.air.vertical_speed;

            consume_descent_slide(&mut air);
            let world_disp = match select_air_vertical_path(
                anim_driven,
                launching,
                air.air.launch_lifted,
                switching_weapon_air,
            ) {
                AirVerticalPath::AuthoredCurve => {
                    air.air.air_time += dt;
                    air.air.vertical_speed = anim_speed_src * MODEL_SCALE;
                    anim_step_src * MODEL_SCALE
                }
                AirVerticalPath::LaunchChargeHold => {
                    air.air.air_time += dt;
                    air.air.vertical_speed = 0.0;
                    0.0
                }
                AirVerticalPath::WeaponSwitchGravity => {
                    air.air.air_time += dt;
                    let (new_v, disp, _, _) = jump::integrate_vertical(
                        air.air.vertical_speed,
                        air.air.planar_x,
                        air.air.planar_z,
                        dt,
                        1.0,
                        false,
                    );
                    air.air.vertical_speed = new_v;
                    disp
                }
                AirVerticalPath::Ballistic => air.air.tick_vertical(dt, false),
            };

            if launching && anim_speed_src > 0.0 && !air.air.launch_lifted {
                air.air.launch_lifted = true;
                air.air.air_time = 0.0;
            }

            if air.air.motion_mode == AirMotionMode::CurveExit
                && air.air.vertical_speed < AirState::CURVE_EXIT_SPEED
            {
                air.air.motion_mode = AirMotionMode::Ballistic;
            }
            let (px, vy, pz, capped_world_disp) = clamp_air_motion_step(
                air.air.planar_x,
                air.air.vertical_speed,
                air.air.planar_z,
                world_disp,
            );
            air.air.planar_x = px;
            air.air.vertical_speed = vy;
            air.air.planar_z = pz;
            let world_disp = capped_world_disp;
            if sys.blocks_motion_sweep(pos, Point3::new(px * dt, world_disp, pz * dt)) {
                air.air.planar_x = 0.0;
                air.air.planar_z = 0.0;
                air.air.vertical_speed = 0.0;
                motion.velocity = Vec3::ZERO;
                continue;
            }
            let requested_x = px * dt;
            let requested_z = pz * dt;
            let (rx, rz, ry, planar_surface) =
                super::character_contact_plugin::resolve_fighter_xz_with_surface(
                    contacts.as_deref_mut(),
                    entity,
                    &sys,
                    pos,
                    requested_x,
                    requested_z,
                    false,
                );
            project_planar_air_velocity_on_contact(&mut air.air, requested_x, requested_z, rx, rz);
            transform.translation.x += rx;
            transform.translation.z += rz;
            let new_y = transform.translation.y + ry;

            let (dy, foot, surface, blocked) = if world_disp >= 0.0 {
                let (dy, hit, blocked) = sys.resolve_ascent_with_surface(
                    Point3::new(transform.translation.x, new_y, transform.translation.z),
                    world_disp,
                );
                (
                    dy,
                    None::<FootHit>,
                    hit.map(|hit| (hit.nx, hit.ny, hit.nz)),
                    blocked,
                )
            } else {
                let (dy, hit, blocked) = sys.resolve_descent_with_surface(
                    Point3::new(transform.translation.x, new_y, transform.translation.z),
                    world_disp,
                    planar_surface,
                );
                (dy, hit, hit.map(|hit| (hit.nx, hit.ny, hit.nz)), blocked)
            };
            air.mover_vertical_contact = surface.is_some() || blocked;
            if air.mover_vertical_contact && !attacking && !switching_weapon_air {
                air.air.cling_armed = false;
            }
            let contact_speed = air.air.vertical_speed;
            update_descent_slide(&mut air, foot, contact_speed, dt);
            record_carry_support(
                &mut air,
                world_disp,
                surface,
                planar_surface.is_some()
                    || (rx - requested_x).abs() > 1e-5
                    || (rz - requested_z).abs() > 1e-5,
            );
            if let Some((nx, ny, nz)) = surface {
                if world_disp <= 0.0 || ny < 0.0 {
                    project_air_velocity_on_surface(&mut air.air, nx, ny, nz);
                }
            }
            if blocked {
                air.air.vertical_speed = 0.0;
            }
            transform.translation.y += ry + dy;
            transform.translation.y = transform.translation.y.max(tick_ground_y);
            commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);

            let ground_y = tick_ground_y;
            let mover_floor_contact =
                world_disp < 0.0 && meteor_physics::collision::descent_has_support(foot, blocked);
            let near_ground =
                transform.translation.y <= ground_y + jump::LAND_EPS || mover_floor_contact;
            air.peak_lift = air
                .peak_lift
                .max((transform.translation.y - air.ground_y).max(0.0));
            air.ground_contact = ordinary_mover_support(
                air.ground_contact,
                planar_surface.is_some()
                    || (rx - requested_x).abs() > 1e-5
                    || (rz - requested_z).abs() > 1e-5,
                surface,
                blocked,
            );
            let character_assets = assets
                .as_ref()
                .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)));
            let authored_link = character_assets
                .and_then(|assets| assets.poses.get(&pose.pose_id))
                .and_then(|pose| pose.link);
            let attack_clip_owns_landing = attack_clip_owns_landing(
                attacking,
                pose.is_fall_hold(),
                authored_link,
                authored_link.is_some_and(|link| {
                    character_assets.is_some_and(|assets| assets.poses.contains_key(&link))
                }),
            );
            let can_land = near_ground
                && !launching
                && jump::lands_on_contact(vertical_speed_before)
                && !attack_clip_owns_landing;

            let timed_out = air.air.air_time >= jump::MAX_AIR_TIME
                && (!launching || air.air.launch_lifted)
                && near_ground
                && !attack_clip_owns_landing;
            if can_land || timed_out {
                if transform.translation.y <= ground_y + jump::LAND_EPS {
                    transform.translation.y = ground_y;
                }
                finish_air_landing(&mut air, &mut wall);
                if attacking {
                    let recovery_pose = authored_link.filter(|&link| {
                        character_assets.is_some_and(|assets| assets.poses.contains_key(&link))
                    });
                    let linked_pose = land_air_attack_state(
                        &mut sm,
                        &mut combat_state,
                        pose.pose_id,
                        recovery_pose,
                    );
                    if let (Some(linked_pose), Some(character_assets)) =
                        (linked_pose, character_assets)
                    {
                        character_assets.apply_pose_with_blend(
                            &mut pose,
                            &mut blend,
                            linked_pose,
                            true,
                            None,
                            EntryBlendKind::Generic,
                            false,
                            false,
                            Some(meteor_combat::weapon_strategy::weapon_to_strategy(
                                fighter.weapon,
                            )),
                        );
                    }
                } else {
                    air.ground_entry_blend_pending = true;
                    land_jump_state(&mut sm, &mut combat_state);
                }
            } else if near_ground {
                air.air.cling_armed = false;
            }

            if air.air.airborne
                && ordinary_locomotion
                && !attacking
                && !switching_weapon_air
                && !wall.cling_active
                && air.air.cling_armed
                && air.air.air_time >= WALL_CLING_DELAY
                && input.z > 0.1
                && air.air.vertical_speed >= 0.0
            {
                let cling_pos = Point3::new(
                    transform.translation.x,
                    transform.translation.y,
                    transform.translation.z,
                );
                if let Some((cling_pose, contact)) = try_enter_wall_cling(
                    &sys,
                    cling_pos,
                    facing,
                    Vec3::new(air.air.planar_x, 0.0, air.air.planar_z),
                    input.x,
                ) {
                    if !apply_wall_cling_pose_now(
                        assets.as_deref(),
                        common_amb.as_deref(),
                        fighter,
                        &mut pose,
                        &mut blend,
                        &mut air,
                        cling_pose,
                    ) {
                        air.motion_root_pin = Some([
                            air.stand_motion_root_x,
                            air.stand_motion_root_y,
                            air.pin_root_z,
                        ]);
                        let accepted = transform.translation - tick_start;
                        motion.velocity =
                            accepted_motion_velocity(accepted.x, accepted.y, accepted.z, dt);
                        continue;
                    }
                    let before_projection = wall_run::Vec3::new(
                        air.air.planar_x,
                        air.air.vertical_speed,
                        air.air.planar_z,
                    );
                    let wall_normal =
                        wall_run::Vec3::new(contact.normal.x, contact.normal.y, contact.normal.z);
                    let external =
                        wall_run::wall_cling_external_accel(before_projection, wall_normal);

                    wall.cling_active = true;
                    wall.cling_pose = cling_pose;
                    wall.wall_normal = contact.normal;
                    wall.contact_seen = false;
                    wall.kick_pose = 0;
                    wall.remaining = wall_run::WALL_CLING_MAX_TIME;
                    wall.external_accel = Vec3::new(external.x, external.y, external.z);
                    air.air.cling_armed = false;
                    air.air.motion_mode = AirMotionMode::Wall;
                    air.ground_contact = false;

                    let into = air.air.planar_x * contact.normal.x
                        + air.air.vertical_speed * contact.normal.y
                        + air.air.planar_z * contact.normal.z;
                    air.air.planar_x -= contact.normal.x * into;
                    air.air.planar_z -= contact.normal.z * into;
                    air.air.vertical_speed -= contact.normal.y * into;
                    motion.movement_mode = MovementMode::WallRun;
                    motion.velocity =
                        Vec3::new(air.air.planar_x, air.air.vertical_speed, air.air.planar_z);
                    continue;
                }
            }

            if air.air.airborne
                && air.air.motion_mode == AirMotionMode::Ballistic
                && !attacking
                && contact_speed < jump::FALL_POSE_SPEED
                && !switching_weapon_air
                && weapon_switch.is_none()
                && !air.attack_ground_pose_pending
                && !jump::fall_pose_keep(pose.pose_id)
                && !pose.is_fall_hold()
            {
                if let Some(character_assets) = assets
                    .as_ref()
                    .and_then(|a| a.0.get(&format!("p{}", fighter.character_id)))
                {
                    let fall_id = jump::fall_pose_for(pose.pose_id);
                    if character_assets.poses.contains_key(&fall_id) && fall_id != pose.pose_id {
                        character_assets.apply_pose_with_blend(
                            &mut pose,
                            &mut blend,
                            fall_id,
                            true,
                            None,
                            crate::components::fighter::EntryBlendKind::Generic,
                            false,
                            false,
                            None,
                        );
                        air.motion_root_pin_pose_id = fall_id;
                        air.late_visual_pose_snap = false;
                    }
                }
            }

            if !air.air.airborne {
                motion.movement_mode = if crouching {
                    MovementMode::Crouch
                } else if input.length_squared() > 0.0 {
                    if running {
                        MovementMode::Run
                    } else {
                        MovementMode::Walk
                    }
                } else {
                    MovementMode::Idle
                };
                air.motion_root_pin = None;
                air.jump_root_xy_started = false;
            } else {
                motion.movement_mode = MovementMode::Airborne;
                air.motion_root_pin = Some([
                    air.stand_motion_root_x,
                    air.stand_motion_root_y,
                    air.pin_root_z,
                ]);
            }
            let accepted = transform.translation - tick_start;
            motion.velocity = if air.air.airborne {
                accepted_motion_velocity(accepted.x, accepted.y, accepted.z, dt)
            } else {
                Vec3::new(air.landed_planar.x, 0.0, air.landed_planar.y)
            };
            continue;
        }

        air.ground_contact = true;

        let ground_mode_updates = weapon_switch.is_none()
            && matches!(sm.current, FighterState::Name | FighterState::Guard);
        if on_ground && (ordinary_locomotion || ground_mode_updates) {
            let external = update_ground_external_motion(&mut air, &sys, pos, dt);

            let drive_dir = if ordinary_locomotion && input.length_squared() > 0.0 {
                world_direction(locomotion_input, basis_yaw)
            } else {
                Vec3::ZERO
            };
            let drive_params = planar_drive::DriveParams {
                dt,
                speed_factor: status_effects.effects.speed_factor(buffs.speed_factor),
                forward: input.z > 0.0 && !crouch_drive,
                ground_support: true,
                air_mode0: false,
                action_vetoed: ranged_gate.wheel_out,
            };
            let driven = planar_drive::tick_ground(
                [motion.velocity.x, 0.0, motion.velocity.z],
                [drive_dir.x, 0.0, drive_dir.z],
                &drive_params,
            );
            let (vx, _, vz, _) = clamp_composed_velocity_3d(
                driven[0] + external.x,
                0.0,
                driven[2] + external.z,
                MOVER_MAX_SPEED,
            );
            let requested_x = vx * dt;
            let requested_z = vz * dt;
            let (rx, rz, ry) = resolve_fighter_xz(
                contacts.as_deref_mut(),
                entity,
                &sys,
                pos,
                requested_x,
                requested_z,
                false,
            );
            transform.translation.x += rx;
            transform.translation.z += rz;
            transform.translation.y += ry;
            commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);
            motion.movement_mode = if crouching {
                MovementMode::Crouch
            } else if ordinary_locomotion && input.length_squared() > 0.0 {
                if running {
                    MovementMode::Run
                } else {
                    MovementMode::Walk
                }
            } else {
                MovementMode::Idle
            };

            let mut kept_x = driven[0];
            let mut kept_z = driven[2];
            if rx == 0.0 && requested_x != 0.0 {
                kept_x = 0.0;
            }
            if rz == 0.0 && requested_z != 0.0 {
                kept_z = 0.0;
            }
            motion.velocity = Vec3::new(kept_x, 0.0, kept_z);

            if !replay_facing_locked {
                if let Some(lock_target) = lock.lock_entity {
                    if let Ok(target_transform) = targets.get(lock_target) {
                        apply_ground_lock_facing(
                            &mut lock,
                            &mut basis,
                            &mut motion,
                            transform.translation,
                            target_transform.translation,
                        );
                    }
                } else if can_turn(&sm) {
                    motion.facing = basis_yaw;
                }
            }
        }
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct AirProbeMesh {
        ground_y: f32,
        ray: Option<meteor_physics::collision::RayHit>,
    }

    impl meteor_physics::collision::MeshCollider for AirProbeMesh {
        fn sweep_foot_segment(
            &self,
            _x: f32,
            _y_top: f32,
            _z: f32,
            _y_bottom: f32,
        ) -> Option<FootHit> {
            None
        }

        fn box_blocked(&self, _cx: f32, _cy: f32, _cz: f32) -> bool {
            false
        }

        fn raycast_hit(
            &self,
            _origin: Point3,
            _direction: Point3,
            _max_dist: f32,
        ) -> Option<meteor_physics::collision::RayHit> {
            self.ray
        }

        fn sample_ground_hit(
            &self,
            _x: f32,
            _z: f32,
            _fallback: f32,
            _hint_y: f32,
        ) -> Option<GroundHit> {
            Some(GroundHit {
                y: self.ground_y,
                nx: 0.0,
                ny: 1.0,
                nz: 0.0,
            })
        }
    }

    #[derive(Resource)]
    struct TestPoseFrame(f32);

    fn set_test_pose_frame(frame: Res<TestPoseFrame>, mut poses: Query<&mut PoseRuntime>) {
        for mut pose in &mut poses {
            pose.frame = frame.0;
        }
    }

    #[derive(Resource, Default)]
    struct HitJudgeAirSnapshot(Option<(Vec3, bool)>);

    fn capture_local_position_at_hit_judge(
        fighters: Query<(&Transform, &FighterAir), With<LocalFighter>>,
        mut snapshot: ResMut<HitJudgeAirSnapshot>,
    ) {
        snapshot.0 = fighters
            .iter()
            .next()
            .map(|(transform, air)| (transform.translation, air.air.airborne));
    }

    #[test]
    fn airborne_ground_sample_rejects_overhead_platforms() {
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(AirProbeMesh {
            ground_y: 5.0,
            ray: None,
        });
        let mut air = FighterAir {
            ground_y: 1.25,
            ..default()
        };
        let hit = sample_air_ground(&mut air, &mover, Point3::new(0.0, 4.0, 0.0));
        assert_eq!(hit.unwrap().y, 1.25);
        assert_eq!(air.ground_y, 1.25);

        mover.set_mesh_collider(AirProbeMesh {
            ground_y: 2.0,
            ray: None,
        });
        let hit = sample_air_ground(&mut air, &mover, Point3::new(0.0, 4.0, 0.0));
        assert_eq!(hit.unwrap().y, 2.0);
        assert_eq!(air.ground_y, 2.0);
    }

    #[test]
    fn wall_probe_retains_the_complete_scene_normal() {
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(AirProbeMesh {
            ground_y: 0.0,
            ray: Some(meteor_physics::collision::RayHit {
                t: 0.4,
                y: 2.0,
                nx: 0.0,
                ny: 0.3,
                nz: -0.953_939_2,
            }),
        });
        let hit = probe_wall(&mover, Point3::ZERO, Vec3::Z, 1.0).expect("wall contact");
        assert!((hit.normal.y - 0.3).abs() < 1e-6);
        assert!((hit.normal.length() - 1.0).abs() < 1e-6);
        assert!(hit.normal.dot(Vec3::Z) <= 0.0);
    }

    #[test]
    fn wall_mode2_matches_executed_meteor_exe_instructions() {
        let mut mesh = crate::plugins::stage_collider::StageMeshCollider::default();
        mesh.append_world_triangles(
            &[[0.0, 0.0, 24.0], [0.0, 0.0, -24.0], [0.0, 24.0, 0.0]],
            &[0, 1, 2],
        );
        for with_wall in [true, false] {
            let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
            mover.set_mesh_collider(if with_wall {
                mesh.clone()
            } else {
                Default::default()
            });
            let mut air = AirState {
                airborne: true,
                vertical_speed: 7.2,
                ..default()
            };
            let mut wall = FighterWall {
                cling_active: true,
                wall_normal: Vec3::X,
                external_accel: Vec3::new(-96.0, 0.0, 0.0),
                ..default()
            };
            let out = tick_wall_cling(
                Entity::PLACEHOLDER,
                None,
                &mut air,
                &mut wall,
                &mover,
                Point3::new(1.44, 4.8, 0.0),
                1.0 / 60.0,
            );
            let expected_x = if with_wall { 2.16 } else { 11.7777777 * 0.12 };
            assert!((out.position.x - expected_x).abs() < 1e-5, "{out:?}");
            assert!((out.position.y - 40.9555016 * 0.12).abs() < 1e-5, "{out:?}");
            assert!((air.vertical_speed - 57.3300018 * 0.12).abs() < 1e-5);
            assert_eq!(out.flags, if with_wall { 34 } else { 0 });
            assert!((air.planar_x - if with_wall { 0.0 } else { -13.333333 * 0.12 }).abs() < 1e-5);
        }
        let mut mesh = crate::plugins::stage_collider::StageMeshCollider::default();
        mesh.append_world_triangles(
            &[[0.0, 0.0, 24.0], [0.0, 0.0, -24.0], [-19.2, 14.4, 0.0]],
            &[0, 1, 2],
        );
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(mesh);
        let mut air = AirState {
            airborne: true,
            vertical_speed: 7.2,
            ..default()
        };
        let mut wall = FighterWall {
            cling_active: true,
            wall_normal: Vec3::new(0.6, 0.8, 0.0),
            external_accel: Vec3::new(-57.6, 0.0, 0.0),
            ..default()
        };
        let out = tick_wall_cling(
            Entity::PLACEHOLDER,
            None,
            &mut air,
            &mut wall,
            &mover,
            Point3::new(-30.4 * 0.12, 42.8 * 0.12, 0.0),
            1.0 / 60.0,
        );
        assert_eq!(out.flags, 2);
        assert!(
            (out.position.x - (-29.28533554 * 0.12)).abs() < 1e-5,
            "{out:?}"
        );
        assert!(
            (out.position.y - 45.41949844 * 0.12).abs() < 1e-5,
            "{out:?}"
        );
        assert!((air.planar_x + 0.96).abs() < 1e-5);
    }

    #[test]
    fn releasing_wall_restarts_ballistic_track_without_dropping_velocity() {
        let mut air = FighterAir {
            ground_y: 2.0,
            peak_lift: 1.0,
            ..default()
        };
        air.air.airborne = true;
        air.air.motion_mode = AirMotionMode::Wall;
        air.air.air_time = 0.8;
        air.air.planar_x = 3.0;
        air.air.vertical_speed = -4.0;
        let mut wall = FighterWall {
            cling_active: true,
            contact_seen: true,
            external_accel: Vec3::ONE,
            ..default()
        };
        release_wall_motion(&mut air, &mut wall, L::WALL_CLING_F, 5.0);
        assert!(!wall.cling_active);
        assert!(!wall.contact_seen);
        assert_eq!(air.air.motion_mode, AirMotionMode::Ballistic);
        assert_eq!(air.air.air_time, 0.8);
        assert_eq!(air.air.planar_x, 3.0);
        assert_eq!(air.air.vertical_speed, -4.0);
        assert_eq!(air.air.vertical_pose_id, L::WALL_CLING_F);
        assert_eq!(air.peak_lift, 3.0);
    }

    #[test]
    fn wall_slide_acceleration_feeds_released_diagonal_jump() {
        for fps in [30, 60, 120, 165] {
            for strong in [false, true] {
                let mut mesh = crate::plugins::stage_collider::StageMeshCollider::default();
                mesh.append_world_triangles(
                    &[
                        [0.0, -100.0, -100.0],
                        [0.0, 100.0, -100.0],
                        [0.0, 100.0, 100.0],
                        [0.0, -100.0, 100.0],
                    ],
                    &[0, 1, 2, 0, 2, 3],
                );
                let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
                mover.set_mesh_collider(mesh);
                let mut air = FighterAir::default();
                air.air.airborne = true;
                air.air.motion_mode = AirMotionMode::Wall;
                air.air.planar_z = 12.0;
                air.air.vertical_speed = 7.2;
                let ext = wall_run::wall_cling_external_accel(
                    wall_run::Vec3::new(0.0, 7.2, 12.0),
                    wall_run::Vec3::new(1.0, 0.0, 0.0),
                );
                let mut wall = FighterWall {
                    cling_active: true,
                    wall_normal: Vec3::X,
                    external_accel: Vec3::new(ext.x, ext.y, ext.z),
                    ..default()
                };
                let mut pos = Point3::new(1.44, 12.0, 0.0);
                for _ in 0..fps / 5 {
                    pos = tick_wall_cling(
                        Entity::PLACEHOLDER,
                        None,
                        &mut air.air,
                        &mut wall,
                        &mover,
                        pos,
                        1.0 / fps as f32,
                    )
                    .position;
                }
                let tangent_speed = air.air.planar_z;
                assert!(tangent_speed > 12.0, "{fps}: wall slide must accelerate");
                let mut direct = air.air.clone();
                assert_eq!(
                    try_wall_jump(
                        &mut direct,
                        &mut wall,
                        &mover,
                        pos,
                        Vec3::Z,
                        L::WALL_CLING_R,
                        strong
                    ),
                    Some(L::WALL_KICK_R)
                );
                assert!(direct.planar_z.abs() < 1e-5, "mode2 replaces tangent speed");

                release_wall_motion(&mut air, &mut wall, L::WALL_CLING_R, pos.y);
                assert_eq!(air.air.planar_z, tangent_speed);
                assert_eq!(wall.external_accel, Vec3::ZERO);

                let facing = Vec3::new(-1.0, 0.0, 1.0).normalize();
                assert!(try_wall_jump(
                    &mut air.air,
                    &mut wall,
                    &mover,
                    pos,
                    facing,
                    L::WALL_CLING_R,
                    strong
                )
                .is_some());
                assert!((air.air.planar_z - tangent_speed).abs() < 1e-5);
                assert!(air.air.planar_x > 0.0, "released jump must also move away");
            }
        }
    }

    #[test]
    fn wall_jump_uses_mode_and_saved_three_dimensional_normal() {
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(AirProbeMesh {
            ground_y: 0.0,
            ray: None,
        });
        let normal = Vec3::new(0.6, 0.8, 0.0);
        let mut wall = FighterWall {
            cling_active: true,
            wall_normal: normal,
            ..default()
        };
        let mut air = AirState {
            airborne: true,
            planar_x: 9.0,
            vertical_speed: -3.0,
            planar_z: 2.0,
            ..default()
        };

        assert_eq!(
            try_wall_jump(
                &mut air,
                &mut wall,
                &mover,
                Point3::ZERO,
                Vec3::Z,
                L::JUMP_F,
                false
            ),
            Some(L::JUMP_F)
        );
        let expected = (normal + Vec3::Y).normalize() * jump::JUMP_VERTICAL_SPEED;
        assert!(
            (Vec3::new(air.planar_x, air.vertical_speed, air.planar_z) - expected).length() < 1e-5
        );
        wall.cling_active = false;
        mover.set_mesh_collider(AirProbeMesh {
            ground_y: 0.0,
            ray: Some(meteor_physics::collision::RayHit {
                t: 0.4,
                y: 0.0,
                nx: 0.0,
                ny: 0.0,
                nz: -1.0,
            }),
        });

        for _ in 0..2 {
            assert_eq!(
                try_wall_jump(
                    &mut air,
                    &mut wall,
                    &mover,
                    Point3::ZERO,
                    Vec3::Z,
                    L::JUMP_B,
                    false
                ),
                Some(L::JUMP_B)
            );
        }
    }

    #[test]
    fn attack_carry_advances_through_authored_still_frames() {
        let dt = 1.0 / 30.0;
        let (carry_x, carry_z, dx, dz) = compose_attack_planar_step(
            Vec2::ZERO,
            Vec2::new(30.0, -15.0),
            Vec2::ZERO,
            CARRY_DECAY_GROUND,
            dt,
        );
        assert!((carry_x - 21.0).abs() < 1e-5);
        assert!((carry_z + 10.5).abs() < 1e-5);
        assert!((dx - carry_x * dt).abs() < 1e-5);
        assert!((dz - carry_z * dt).abs() < 1e-5);
    }

    #[test]
    fn attack_composed_step_clamps_root_and_carry_together() {
        let dt = 1.0 / 60.0;
        let (_, _, dx, dz) = compose_attack_planar_step(
            Vec2::new(4.0, 0.0),
            Vec2::new(120.0, 0.0),
            Vec2::ZERO,
            CARRY_DECAY_GROUND,
            dt,
        );
        assert!((dx / dt - CARRY_MAX_SPEED).abs() < 1e-5);
        assert!(dz.abs() < 1e-6);
    }

    #[test]
    fn action_step_composes_steep_external_velocity_with_slow_carry_decay() {
        let dt = 1.0 / 30.0;
        let (carry_x, _, attack_x, _) = compose_attack_planar_step(
            Vec2::ZERO,
            Vec2::new(12.0, 0.0),
            Vec2::new(3.0, 0.0),
            CARRY_DECAY_STEEP,
            dt,
        );
        let steep_k = 1.0 - CARRY_DECAY_STEEP * dt;
        assert!((carry_x - 12.0 * steep_k).abs() < 1e-6);
        assert!((attack_x - (carry_x + 3.0) * dt).abs() < 1e-6);

        let (residual_x, _, dodge_x, _) = compose_dodge_planar_step(
            Vec2::ZERO,
            Vec2::new(12.0, 0.0),
            Vec2::new(3.0, 0.0),
            CARRY_DECAY_STEEP,
            dt,
        );
        assert!((residual_x - carry_x).abs() < 1e-6);
        assert!((dodge_x - attack_x).abs() < 1e-6);
    }

    #[test]
    fn carry_decay_changes_only_after_a_classifying_mover_solve() {
        for launch_pose in [0, 253, 297] {
            let mut air = FighterAir::default();
            air.air.airborne = true;
            air.air.air_time = 0.8;
            air.air.launch_pose_id = launch_pose;
            record_carry_support(&mut air, 0.0, None, false);
            assert_eq!(
                action_carry_decay(&air),
                CARRY_DECAY_GROUND,
                "frozen charge does not clear support"
            );
            record_carry_support(&mut air, 0.2, None, true);
            assert_eq!(
                action_carry_decay(&air),
                CARRY_DECAY_GROUND,
                "planar contact preserves classification"
            );
            record_carry_support(&mut air, 0.2, None, false);
            assert_eq!(action_carry_decay(&air), CARRY_DECAY_STEEP);
            air.air.launch_pose_id = 0;
            air.air.air_time = 0.0;
            assert_eq!(
                action_carry_decay(&air),
                CARRY_DECAY_STEEP,
                "Pose/clock changes do not invent ground contact"
            );
            record_carry_support(&mut air, -0.2, Some((0.0, 1.0, 0.0)), false);
            assert_eq!(action_carry_decay(&air), CARRY_DECAY_GROUND);
            record_carry_support(&mut air, -0.2, Some((0.8, 0.3, 0.0)), false);
            assert_eq!(action_carry_decay(&air), CARRY_DECAY_STEEP);
        }
    }

    #[test]
    fn steep_support_accumulates_projected_gravity_and_preserves_classification_on_miss() {
        let mut air = FighterAir::default();
        let dt = 1.0 / 30.0;
        let steep = GroundHit {
            y: 0.0,
            nx: 0.8,
            ny: 0.3,
            nz: 0.0,
        };
        let first = update_ground_external_from_hit(&mut air, Some(steep), 0.0, dt);
        assert!(air.carry_decay_steep);
        assert!(first.x > 0.0);
        let second = update_ground_external_from_hit(&mut air, Some(steep), 0.0, dt);
        assert!((second.x - first.x * 2.0).abs() < 1e-5);

        assert_eq!(
            update_ground_external_from_hit(&mut air, None, 0.0, dt),
            Vec3::ZERO
        );
        assert!(
            air.carry_decay_steep,
            "a missing probe preserves char+0x35c"
        );

        let walkable = GroundHit {
            y: 0.0,
            nx: 0.0,
            ny: 1.0,
            nz: 0.0,
        };
        update_ground_external_from_hit(&mut air, Some(walkable), 0.0, dt);
        assert!(!air.carry_decay_steep);
        assert_eq!(air.ground_external_velocity, Vec3::ZERO);
    }

    #[test]
    fn dodge_carry_advances_on_the_root_baseline_tick() {
        let dt = 1.0 / 30.0;
        let (residual_x, residual_z, dx, dz) = compose_dodge_planar_step(
            Vec2::ZERO,
            Vec2::new(12.0, -6.0),
            Vec2::ZERO,
            CARRY_DECAY_GROUND,
            dt,
        );
        assert!((residual_x - 8.4).abs() < 1e-5);
        assert!((residual_z + 4.2).abs() < 1e-5);
        assert!((dx - residual_x * dt).abs() < 1e-5);
        assert!((dz - residual_z * dt).abs() < 1e-5);
    }

    #[test]
    fn dodge_composed_step_clamps_root_and_carry_once() {
        let dt = 1.0 / 60.0;
        let (_, _, dx, dz) = compose_dodge_planar_step(
            Vec2::new(3.0, 4.0),
            Vec2::ZERO,
            Vec2::ZERO,
            CARRY_DECAY_GROUND,
            dt,
        );
        assert!(((dx.hypot(dz) / dt) - CARRY_MAX_SPEED).abs() < 1e-5);
        assert!((dx / dz - 0.75).abs() < 1e-5);
    }

    #[test]
    fn ballistic_air_clamps_all_axes_and_scales_vertical_step() {
        let (vx, vy, vz, dy) = clamp_air_motion_step(60.0, 60.0, 0.0, 2.0);
        assert!(((vx * vx + vy * vy + vz * vz).sqrt() - MOVER_MAX_SPEED).abs() < 1e-5);
        assert!((vx - vy).abs() < 1e-5);
        assert!((dy - 2.0 * MOVER_MAX_SPEED / 60.0_f32.hypot(60.0)).abs() < 1e-5);
    }

    #[test]
    fn airborne_motion_publishes_collision_accepted_three_axis_velocity() {
        let velocity = accepted_motion_velocity(0.6, -0.3, 0.15, 1.0 / 30.0);
        assert!((velocity.x - 18.0).abs() < 1e-5);
        assert!((velocity.y + 9.0).abs() < 1e-5);
        assert!((velocity.z - 4.5).abs() < 1e-5);
        assert_eq!(accepted_motion_velocity(1.0, 1.0, 1.0, 0.0), Vec3::ZERO);
    }

    #[test]
    fn rejected_air_motion_projects_persistent_velocity_instead_of_banking_a_snap() {
        let mut air = AirState::new();
        air.airborne = true;
        air.planar_x = 12.0;
        air.planar_z = 3.0;
        assert!(project_planar_air_velocity_on_contact(
            &mut air, 0.4, 0.0, 0.0, 0.0,
        ));
        assert!(air.planar_x.abs() < 1e-5, "into-wall speed must be removed");
        assert!((air.planar_z - 3.0).abs() < 1e-5, "tangent speed remains");

        air.vertical_speed = 20.0;
        project_air_velocity_on_surface(&mut air, 0.0, -1.0, 0.0);
        assert!(
            air.vertical_speed.abs() < 1e-5,
            "a ceiling hit must not retain upward speed for a later teleport"
        );
    }

    #[test]
    fn wall_cling_caps_the_complete_xyz_request() {
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(crate::plugins::stage_collider::StageMeshCollider::default());
        let mut air = AirState::new();
        air.airborne = true;
        air.planar_x = 160.0;
        air.planar_z = 90.0;
        air.vertical_speed = 120.0;
        let mut wall = FighterWall {
            wall_normal: Vec3::X,
            remaining: wall_run::WALL_CLING_MAX_TIME,
            ..default()
        };
        let dt = 1.0 / 30.0;
        let result = tick_wall_cling(
            Entity::PLACEHOLDER,
            None,
            &mut air,
            &mut wall,
            &mover,
            Point3::ZERO,
            dt,
        );
        assert!(
            Vec3::new(result.position.x, result.position.y, result.position.z).length()
                <= MOVER_MAX_SPEED * dt + 1e-4,
            "mode2 must use the same three-axis mover cap as Web"
        );
    }

    #[test]
    fn jump_takeoff_carries_planar_velocity_verbatim() {
        let impulse = jump::jump_impulse(JumpStartInput {
            carry_x: -4.0,
            carry_z: 7.5,
        });
        assert_eq!(impulse.x, -4.0);
        assert_eq!(impulse.z, 7.5);
    }

    #[test]
    fn attack_contact_release_keeps_free_air_and_normal_jump_holds() {
        for (attacking, contact, released) in [
            (true, true, true),
            (true, false, false),
            (false, true, false),
        ] {
            let mut pose = PoseRuntime {
                pose_id: if attacking { 332 } else { 180 },
                frame: 100.0,
                start: 100,
                end: 103,
                loop_start: 100,
                loop_end: 100,
                loops: true,
                pose_instance_id: 1,
            };
            release_air_attack_hold_on_contact(&mut pose, attacking, contact);
            assert_eq!(pose.loops, !released);
            assert_eq!(pose.frame, 100.0);
        }
    }

    #[test]
    fn every_authored_fall_hold_stays_on_start_while_airborne() {
        let mut app = App::new();
        app.add_systems(Update, hold_airborne_fall_pose);
        let entity = app
            .world_mut()
            .spawn((
                FighterAir {
                    air: AirState {
                        airborne: true,
                        ..default()
                    },
                    ..default()
                },
                PoseRuntime {
                    pose_id: 428,
                    frame: 105.0,
                    start: 100,
                    end: 112,
                    loop_start: 100,
                    loop_end: 100,
                    loops: true,
                    pose_instance_id: 1,
                },
            ))
            .id();
        app.update();
        assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().frame, 100.0);
    }

    #[test]
    fn firearm_aim_ground_drive_matches_crouch_and_recovers_after_exit() {
        for direction in [
            vec![KeyCode::KeyW],
            vec![KeyCode::KeyS],
            vec![KeyCode::KeyA],
            vec![KeyCode::KeyD],
            vec![KeyCode::KeyW, KeyCode::KeyD],
        ] {
            for force_run in [false, true] {
                let mut velocities = Vec::new();
                for stance in 0..3 {
                    let mut app = App::new();
                    app.init_resource::<Time>()
                        .init_resource::<ButtonInput<KeyCode>>()
                        .init_resource::<ButtonInput<MouseButton>>()
                        .init_resource::<CameraBasis>()
                        .init_resource::<LockOnController>()
                        .init_resource::<crate::plugins::item_plugin::ItemBuffs>()
                        .init_resource::<LocalCrouchInput>()
                        .init_resource::<crate::app_state::Keymap>()
                        .insert_resource(StageCollider {
                            mesh: Some({
                                let mut mesh =
                                    crate::plugins::stage_collider::StageMeshCollider::default();
                                mesh.append_world_triangles(
                                    &[
                                        [-600.0, 0.0, -600.0],
                                        [-600.0, 0.0, 600.0],
                                        [600.0, 0.0, 600.0],
                                        [600.0, 0.0, -600.0],
                                    ],
                                    &[0, 1, 2, 0, 2, 3],
                                );
                                mesh
                            }),
                        })
                        .add_systems(Update, move_local_fighter);
                    let entity = app
                        .world_mut()
                        .spawn((
                            LocalFighter,
                            Fighter {
                                character_id: 0,
                                player_slot: 0,
                                weapon: 3,
                                weapon_slots: [3, 0],
                                weapon_slot: 0,
                                weapon_slot_variants: [0, 0],
                                weapon_variant: 0,
                            },
                            Transform::default(),
                            MotionState {
                                movement_mode: MovementMode::Idle,
                                velocity: Vec3::ZERO,
                                facing: 0.0,
                            },
                            FighterAir::default(),
                            FighterDodge::default(),
                            FighterWall::default(),
                            PoseRuntime::idle(),
                            EntryBlend::default(),
                            SmState::default(),
                            CombatState {
                                state: FighterState::Name,
                            },
                        ))
                        .id();
                    app.world_mut()
                        .get_mut::<GunAimState>(entity)
                        .unwrap()
                        .aiming = stance == 2;
                    app.world_mut().resource_mut::<LocalCrouchInput>().held = stance == 1;
                    {
                        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
                        for key in &direction {
                            keys.press(*key);
                        }
                        if force_run {
                            keys.press(KeyCode::ControlLeft);
                        }
                    }
                    app.world_mut()
                        .resource_mut::<Time>()
                        .advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
                    for _ in 0..120 {
                        app.update();
                    }
                    let motion = app.world().get::<MotionState>(entity).unwrap();
                    velocities.push(motion.velocity);
                    if stance != 0 {
                        assert!(
                            motion.velocity.length() > 4.5 && motion.velocity.length() < 5.5,
                            "stance={stance}, direction={direction:?}, velocity={:?}",
                            motion.velocity
                        );
                        assert_eq!(
                            motion.movement_mode,
                            if stance == 1 {
                                MovementMode::Crouch
                            } else {
                                MovementMode::Walk
                            }
                        );
                        if direction.len() == 2 {
                            assert!(
                                (motion.velocity.x.abs() - motion.velocity.z.abs()).abs() < 1e-5
                            );
                        }
                    }
                    if stance == 2 {
                        assert!(!app.world().resource::<LocalCrouchInput>().held);

                        app.world_mut()
                            .get_mut::<GunAimState>(entity)
                            .unwrap()
                            .aiming = false;
                        for _ in 0..120 {
                            app.update();
                        }
                        let recovered = app.world().get::<MotionState>(entity).unwrap().velocity;
                        assert!((recovered - velocities[0]).length() < 1e-3);
                        app.world_mut()
                            .get_mut::<GunAimState>(entity)
                            .unwrap()
                            .aiming = true;
                        app.world_mut()
                            .resource_mut::<ButtonInput<KeyCode>>()
                            .reset_all();
                        app.update();
                        assert_eq!(
                            app.world()
                                .get::<MotionState>(entity)
                                .unwrap()
                                .movement_mode,
                            MovementMode::Idle,
                            "aim idle must not become C-key crouch idle"
                        );
                    }
                }
                assert!((velocities[1] - velocities[2]).length() < 1e-5);
                if direction.contains(&KeyCode::KeyW) {
                    assert!(velocities[0].length() > 3.0 * velocities[2].length());
                }
            }
        }
    }

    #[test]
    fn production_ballistic_air_waits_for_hit_judge_and_integrates_once() {
        let mut app = App::new();
        crate::plugins::configure_fighter_frame_sets(&mut app);
        app.init_resource::<Time>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<CameraBasis>()
            .init_resource::<LockOnController>()
            .init_resource::<crate::plugins::item_plugin::ItemBuffs>()
            .init_resource::<LocalCrouchInput>()
            .init_resource::<LocalGuardInput>()
            .init_resource::<crate::app_state::Keymap>()
            .init_resource::<HitJudgeAirSnapshot>()
            .insert_resource(StageCollider {
                mesh: Some(crate::plugins::stage_collider::StageMeshCollider::default()),
            })
            .add_plugins(MovementPlugin)
            .add_systems(
                Update,
                capture_local_position_at_hit_judge
                    .in_set(crate::plugins::FighterFrameSet::HitJudge),
            );
        let mut air = FighterAir::default();
        air.air.start_fall(0.0, 0.0);
        let start = Vec3::new(0.0, 10.0, 0.0);
        let fighter = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 5,
                    weapon_slots: [5, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::from_translation(start),
                MotionState {
                    movement_mode: MovementMode::Airborne,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                air,
                FighterDodge::default(),
                FighterWall::default(),
                PoseRuntime::idle(),
                EntryBlend::default(),
                SmState {
                    current: FighterState::Jump,
                    ..default()
                },
                CombatState {
                    state: FighterState::Jump,
                },
            ))
            .id();
        let dt = 1.0 / 30.0;
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(dt));

        app.update();

        let (at_hit_judge, airborne_at_hit_judge) =
            app.world().resource::<HitJudgeAirSnapshot>().0.unwrap();
        assert_eq!(at_hit_judge, start, "Locomotion must not move mode0 air");
        assert!(airborne_at_hit_judge);
        let transform = app.world().get::<Transform>(fighter).unwrap();
        assert!(
            transform.translation.y < start.y,
            "RootMotion applies gravity"
        );
        let air = app.world().get::<FighterAir>(fighter).unwrap();
        assert!((air.air.air_time - dt).abs() < 1e-6, "exactly one air tick");
        let motion = app.world().get::<MotionState>(fighter).unwrap();
        assert!((motion.velocity.y - (transform.translation.y - start.y) / dt).abs() < 1e-5);
    }

    #[test]
    fn production_air_direction_change_uses_weak_drive_and_live_buff_veto() {
        let mut app = App::new();
        crate::plugins::configure_fighter_frame_sets(&mut app);
        app.init_resource::<Time>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<CameraBasis>()
            .init_resource::<LockOnController>()
            .init_resource::<crate::plugins::item_plugin::ItemBuffs>()
            .init_resource::<LocalCrouchInput>()
            .init_resource::<LocalGuardInput>()
            .init_resource::<crate::app_state::Keymap>()
            .init_resource::<HitJudgeAirSnapshot>()
            .insert_resource(StageCollider {
                mesh: Some(crate::plugins::stage_collider::StageMeshCollider::default()),
            })
            .add_plugins(MovementPlugin)
            .add_systems(
                Update,
                capture_local_position_at_hit_judge
                    .in_set(crate::plugins::FighterFrameSet::HitJudge),
            );
        let mut air = FighterAir::default();
        air.air.start_fall(0.0, 0.0);
        let start = Vec3::new(0.0, 10.0, 0.0);
        let fighter = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 5,
                    weapon_slots: [5, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::from_translation(start),
                MotionState {
                    movement_mode: MovementMode::Airborne,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                air,
                FighterDodge::default(),
                FighterWall::default(),
                PoseRuntime::idle(),
                EntryBlend::default(),
                SmState {
                    current: FighterState::Jump,
                    ..default()
                },
                CombatState {
                    state: FighterState::Jump,
                },
            ))
            .id();

        for fps in [30.0_f32, 60.0, 120.0] {
            for initial_z in [0.0, 12.0] {
                for (key, ix, iz) in [
                    (KeyCode::KeyA, 1.0_f32, 0.0_f32),
                    (KeyCode::KeyS, 0.0, -1.0),
                ] {
                    for (speed_factor, vetoed) in [(1.0, false), (1.5, false), (1.5, true)] {
                        let dt = 1.0 / fps;
                        {
                            let world = app.world_mut();
                            let mut keys = world.resource_mut::<ButtonInput<KeyCode>>();
                            keys.reset_all();
                            keys.press(key);
                            world
                                .resource_mut::<crate::plugins::item_plugin::ItemBuffs>()
                                .speed_factor = speed_factor;
                            world.get_mut::<Transform>(fighter).unwrap().translation = start;
                            let mut air = world.get_mut::<FighterAir>(fighter).unwrap();
                            *air = FighterAir::default();
                            air.air.start_fall(0.0, initial_z);
                            world.get_mut::<MotionState>(fighter).unwrap().velocity =
                                Vec3::new(0.0, 0.0, initial_z);
                            world.get_mut::<RangedFireGate>(fighter).unwrap().wheel_out = vetoed;
                            world
                                .resource_mut::<Time>()
                                .advance_by(std::time::Duration::from_secs_f32(dt));
                        }
                        app.update();
                        let air = &app.world().get::<FighterAir>(fighter).unwrap().air;
                        let r = initial_z / (288.0 * speed_factor);
                        let gain = if vetoed {
                            0.0
                        } else {
                            12.0 * speed_factor * dt * (1.0 - r.powi(4))
                        };

                        let friction = if initial_z > 0.0 {
                            planar_drive::AIR_RESIST_SCALE * planar_drive::FRICTION_BASE * dt
                        } else {
                            0.0
                        };
                        let px = ix * gain;
                        let pz = initial_z - friction + iz * gain;
                        let vy = -105.84 * dt;
                        let drag = 1.0 - (px * px + pz * pz + vy * vy).sqrt() / 0.12 * dt / 6000.0;
                        assert!(
                            (air.planar_x - px * drag).abs() < 1e-5,
                            "side drive: fps={fps}, initial={initial_z}, veto={vetoed}"
                        );
                        assert!(
                            (air.planar_z - pz * drag).abs() < 1e-5,
                            "reverse drive: fps={fps}, initial={initial_z}, veto={vetoed}"
                        );
                        assert!((air.air_time - dt).abs() < 1e-6);
                        assert_eq!(
                            app.world().resource::<HitJudgeAirSnapshot>().0.unwrap().0,
                            start
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn air_drive_crouch_basis_and_steep_support_match_exe_branches() {
        for crouching in [false, true] {
            let mut air = FighterAir::default();
            air.air.start_fall(0.0, 0.0);
            air.ground_contact = true;
            air.carry_decay_steep = true;
            tick_air_input_drive(
                &mut air,
                1.0 / 30.0,
                Vec3::new(1.0, 0.0, 1.0),
                0.0,
                crouching,
                1.0,
                false,
            );
            let ratio = air.air.planar_x.abs() / air.air.planar_z;
            assert!((ratio - if crouching { 1.0 } else { 0.2 }).abs() < 1e-5);
            assert!(
                (air.air.planar_x.hypot(air.air.planar_z) - 0.4).abs() < 1e-5,
                "steep support must keep the weak air drive"
            );
        }
    }

    #[test]
    fn production_ledge_fall_starts_after_hit_judge_without_same_frame_gravity() {
        let mut app = App::new();
        crate::plugins::configure_fighter_frame_sets(&mut app);
        app.init_resource::<Time>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<CameraBasis>()
            .init_resource::<LockOnController>()
            .init_resource::<crate::plugins::item_plugin::ItemBuffs>()
            .init_resource::<LocalCrouchInput>()
            .init_resource::<LocalGuardInput>()
            .init_resource::<crate::app_state::Keymap>()
            .init_resource::<HitJudgeAirSnapshot>()
            .insert_resource(StageCollider {
                mesh: Some(crate::plugins::stage_collider::StageMeshCollider::default()),
            })
            .add_plugins(MovementPlugin)
            .add_systems(
                Update,
                capture_local_position_at_hit_judge
                    .in_set(crate::plugins::FighterFrameSet::HitJudge),
            );
        let start = Vec3::new(0.0, 10.0, 0.0);
        let fighter = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 5,
                    weapon_slots: [5, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::from_translation(start),
                MotionState {
                    movement_mode: MovementMode::Idle,
                    velocity: Vec3::new(3.0, 0.0, 4.0),
                    facing: 0.0,
                },
                FighterAir::default(),
                FighterDodge::default(),
                FighterWall::default(),
                PoseRuntime::idle(),
                EntryBlend::default(),
                SmState::default(),
                CombatState::default(),
            ))
            .id();
        let dt = 1.0 / 30.0;
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(dt));

        app.update();

        let (at_hit_judge, airborne_at_hit_judge) =
            app.world().resource::<HitJudgeAirSnapshot>().0.unwrap();
        assert_eq!(at_hit_judge, start);
        assert!(
            !airborne_at_hit_judge,
            "startFall is committed after HitJudge"
        );
        let transform = app.world().get::<Transform>(fighter).unwrap();
        assert_eq!(
            transform.translation, start,
            "transition tick does not integrate gravity"
        );
        let air = app.world().get::<FighterAir>(fighter).unwrap();
        assert!(air.air.airborne);
        assert!(air.ledge_fall);
        assert!(!air.pending_ledge_fall);
        assert_eq!(air.air.air_time, 0.0);
    }

    #[test]
    fn attack_states_gate_ordinary_locomotion() {
        let mut sm = SmState::default();
        assert!(ordinary_locomotion_allowed(&sm, false));
        assert!(!ordinary_locomotion_allowed(&sm, true));

        for state in [
            FighterState::Attack1,
            FighterState::Attack2,
            FighterState::Attack3,
            FighterState::Guard,
            FighterState::Dodge,
            FighterState::Burst,
            FighterState::Fake,
            FighterState::Cure,
            FighterState::BeHit,
            FighterState::Down,
            FighterState::Dead,
        ] {
            sm.current = state;
            assert!(!ordinary_locomotion_allowed(&sm, false));
        }

        sm.current = FighterState::Jump;
        assert!(ordinary_locomotion_allowed(&sm, false));
    }

    #[test]
    fn flying_wheel_veto_blocks_dodge_but_not_distinct_burst_action() {
        assert!(!wheel_vetoes_dodge_action(false, false));
        assert!(wheel_vetoes_dodge_action(true, false));
        assert!(!wheel_vetoes_dodge_action(true, true));
    }

    #[test]
    fn guard_accepts_burst_while_attack_and_air_reject_dodge() {
        let mut sm = SmState {
            current: FighterState::Guard,
            locked: true,
            ..default()
        };
        assert!(can_start_dodge_action(&sm, false, false, false, true));

        sm.current = FighterState::Attack1;
        assert!(!can_start_dodge_action(&sm, false, false, false, false));
        sm.current = FighterState::Name;
        assert!(!can_start_dodge_action(&sm, true, false, false, false));
        assert!(!can_start_dodge_action(&sm, false, true, false, false));
        assert!(!can_start_dodge_action(&sm, false, false, true, false));
        assert!(can_start_dodge_action(&sm, false, false, true, true));
    }

    #[test]
    fn preaccepted_dodge_mover_starts_only_if_a_later_action_did_not_replace_it() {
        assert!(accepted_dodge_still_owns_state(FighterState::Dodge, false));
        assert!(accepted_dodge_still_owns_state(FighterState::Burst, true));
        assert!(!accepted_dodge_still_owns_state(
            FighterState::Attack1,
            false
        ));
        assert!(!accepted_dodge_still_owns_state(
            FighterState::Attack1,
            true
        ));
    }

    #[test]
    fn guard_burst_transition_clears_old_action_and_restores_held_guard() {
        let mut sm = SmState {
            current: FighterState::Guard,
            locked: true,
            linked_recovery_pose_id: Some(210),
            exe_ground_mode_ticks: 1,
            ..default()
        };
        let mut combat = CombatState {
            state: FighterState::Guard,
        };

        enter_dodge_state(&mut sm, &mut combat, true);
        assert_eq!(sm.previous, FighterState::Guard);
        assert_eq!(sm.current, FighterState::Burst);
        assert_eq!(combat.state, FighterState::Burst);
        assert!(!sm.locked);
        assert_eq!(sm.linked_recovery_pose_id, None);
        assert_eq!(sm.exe_ground_mode_ticks, 0);

        finish_dodge_state(&mut sm, &mut combat, true);
        assert_eq!(sm.previous, FighterState::Burst);
        assert_eq!(sm.current, FighterState::Guard);
        assert_eq!(combat.state, FighterState::Guard);
    }

    #[test]
    fn dodge_completion_uses_authored_loop_end_or_pose_end_without_fixed_timer() {
        let mut pose = PoseRuntime {
            pose_id: 164,
            frame: 8769.9,
            start: 8760,
            end: 8770,
            loop_start: 8770,
            loop_end: 8770,
            loops: true,
            pose_instance_id: 1,
        };
        assert!(!dodge_pose_finished(&pose));
        pose.frame = 8770.0;
        assert!(dodge_pose_finished(&pose));

        pose.pose_id = 176;
        pose.start = 570;
        pose.end = 581;
        pose.loop_start = 570;
        pose.loop_end = 581;
        pose.loops = false;
        pose.frame = 580.9;
        assert!(!dodge_pose_finished(&pose));
        pose.frame = 581.0;
        assert!(dodge_pose_finished(&pose));
    }

    #[test]
    fn dodge_request_installs_the_target_pose_before_pose_advance() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let character = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root),
            "p1",
        )
        .expect("p1 assets");
        let assets = crate::plugins::character_plugin::CharacterAssetsSet(
            std::collections::HashMap::from([("p1".to_string(), character)]),
        );
        let fighter = Fighter {
            character_id: 1,
            player_slot: 1,
            weapon: 6,
            weapon_slots: [6, 5],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        };
        let mut pose = PoseRuntime::idle();
        let mut blend = EntryBlend::default();

        assert!(apply_dodge_pose_now(
            Some(&assets),
            &fighter,
            &mut pose,
            &mut blend,
            176,
        ));
        assert_eq!(pose.pose_id, 176);
        assert_eq!(blend.pose_id, 176);
    }

    #[test]
    fn buffered_double_tap_direction_selects_pose_after_key_release() {
        assert_eq!(
            dodge_pose_for_axes(Vec2::NEG_X, 1, false),
            mapped_dodge_pose_for_input(-1.0, 0.0, false, false, Some(1))
        );
        assert_eq!(
            dodge_pose_for_axes(Vec2::NEG_Y, 1, true),
            mapped_dodge_pose_for_input(0.0, -1.0, false, true, Some(1))
        );
    }

    #[test]
    fn jump_charge_survives_attack_and_pays_out_in_ground_mode_window() {
        let mut hold = 0.0;
        let mut active = false;
        let mut ticks = 0;

        assert_eq!(
            update_jump_charge(
                &mut hold,
                &mut active,
                &mut ticks,
                true,
                true,
                false,
                true,
                false,
                None,
                0.04,
            ),
            JumpChargeRequest::None
        );

        assert_eq!(
            update_jump_charge(
                &mut hold,
                &mut active,
                &mut ticks,
                false,
                true,
                false,
                false,
                false,
                None,
                0.08,
            ),
            JumpChargeRequest::None
        );
        ticks = 1;
        assert_eq!(
            update_jump_charge(
                &mut hold,
                &mut active,
                &mut ticks,
                false,
                true,
                false,
                true,
                true,
                None,
                0.0,
            ),
            JumpChargeRequest::BufferedGroundMode
        );
        assert!(!active);
        assert_eq!(ticks, 0);
        assert!(hold >= STRONG_JUMP_HOLD);
    }

    #[test]
    fn attack_mode_key_down_does_not_create_buffered_jump() {
        let mut hold = 0.0;
        let mut active = false;
        let mut ticks = 1;
        assert_eq!(
            update_jump_charge(
                &mut hold,
                &mut active,
                &mut ticks,
                true,
                true,
                false,
                false,
                true,
                None,
                0.2,
            ),
            JumpChargeRequest::None
        );
        assert!(!active);
        assert_eq!(ticks, 0);
    }

    #[test]
    fn release_edge_keeps_normal_jump_semantics() {
        let mut hold = 0.05;
        let mut active = true;
        let mut ticks = 1;
        assert_eq!(
            update_jump_charge(
                &mut hold,
                &mut active,
                &mut ticks,
                false,
                false,
                true,
                true,
                true,
                None,
                0.0,
            ),
            JumpChargeRequest::Released
        );
        assert!(!active);
        assert_eq!(ticks, 0);
    }

    #[test]
    fn same_frame_jump_tap_does_not_turn_a_wall_clock_stall_into_strong_hold() {
        let mut hold = 0.0;
        let mut active = false;
        let mut ticks = 0;

        assert_eq!(
            update_jump_charge(
                &mut hold,
                &mut active,
                &mut ticks,
                true,
                false,
                true,
                true,
                false,
                Some(0.0),
                0.3,
            ),
            JumpChargeRequest::Released
        );
        assert_eq!(hold, 0.0);
        assert!(hold < STRONG_JUMP_HOLD);
        assert!(!active);
    }

    #[test]
    fn guard_can_begin_jump_charge_but_locked_actions_cannot() {
        assert!(jump_charge_key_down_allowed(FighterState::Guard, false));
        assert!(jump_charge_key_down_allowed(FighterState::Fake, false));
        assert!(jump_charge_key_down_allowed(FighterState::Down, false));
        assert!(jump_charge_key_down_allowed(FighterState::Jump, true));
        assert!(!jump_charge_key_down_allowed(FighterState::Attack1, false));
        assert!(!jump_charge_key_down_allowed(FighterState::Cure, false));
    }

    #[test]
    fn accepted_jump_survives_a_later_same_frame_attack() {
        assert!(jump_release_reaches_motion(
            false,
            true,
            false,
            JumpChargeRequest::Released,
        ));
        assert!(preserve_post_jump_action(true, FighterState::Attack1));
        assert!(!preserve_post_jump_action(true, FighterState::Jump));

        assert!(!jump_release_reaches_motion(
            false,
            false,
            false,
            JumpChargeRequest::Released,
        ));

        let mut air = AirState::default();
        let mut motion = MotionState {
            movement_mode: MovementMode::Run,
            velocity: Vec3::new(12.0, 0.0, -3.0),
            facing: 0.0,
        };
        start_jump_with_velocity_transfer(
            &mut air,
            &mut motion,
            false,
            jump::Impulse { x: 12.0, z: -3.0 },
        );
        assert_eq!((air.planar_x, air.planar_z), (12.0, -3.0));
        assert_eq!(motion.velocity, Vec3::ZERO);
    }

    #[test]
    fn physical_takeoff_and_landing_publish_jump_state() {
        let mut sm = SmState {
            time_in_state: 0.4,
            locked: true,
            attack_stage: 2,
            exe_ground_mode_ticks: 1,
            ..default()
        };
        let mut combat = CombatState::default();

        enter_jump_state(&mut sm, &mut combat);
        assert_eq!(sm.current, FighterState::Jump);
        assert_eq!(combat.state, FighterState::Jump);
        assert_eq!(sm.previous, FighterState::Name);
        assert_eq!(sm.time_in_state, 0.0);
        assert!(!sm.locked);
        assert_eq!(sm.exe_ground_mode_ticks, 0);

        land_jump_state(&mut sm, &mut combat);
        assert_eq!(sm.current, FighterState::Name);
        assert_eq!(combat.state, FighterState::Name);
        assert_eq!(sm.previous, FighterState::Jump);
        assert_eq!(sm.attack_stage, 0);
    }

    #[test]
    fn wall_kick_installs_pose_and_root_baseline_before_hit_judge() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let character = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root),
            "p1",
        )
        .expect("p1 assets");
        let common = meteor_asset::amb::parse_amb(
            &std::fs::read(root.join("Common/character.amb")).expect("character.amb"),
        )
        .expect("parse character.amb");
        let kick_pose = L::WALL_KICK_L;
        let expected_root =
            character.motion_root_at(&common, kick_pose, character.poses[&kick_pose].start as f32);
        let assets = crate::plugins::character_plugin::CharacterAssetsSet(
            std::collections::HashMap::from([("p1".to_string(), character)]),
        );
        let common = crate::plugins::character_plugin::CommonAmb(common);
        let fighter = Fighter {
            character_id: 1,
            player_slot: 1,
            weapon: 6,
            weapon_slots: [6, 5],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        };
        let mut pose = PoseRuntime::idle();
        let mut blend = EntryBlend::default();
        let mut air = FighterAir::default();
        air.air.airborne = true;

        assert!(apply_wall_kick_pose_now(
            Some(&assets),
            Some(&common),
            &fighter,
            &mut pose,
            &mut blend,
            &mut air,
            kick_pose,
        ));
        assert_eq!(pose.pose_id, kick_pose);
        assert_eq!(pose.frame, pose.start as f32);
        assert_eq!(blend.duration, 0.0, "wall kick is a jump-family hard cut");
        assert_eq!(air.air.vertical_pose_id, kick_pose);
        assert_eq!(air.air.prev_root_z, Some(expected_root[2]));
        assert_eq!(air.motion_root_pin, Some(expected_root));
        assert_eq!(air.motion_root_pin_pose_id, kick_pose);
        assert_eq!(air.pin_root_z, expected_root[2]);

        let cling_pose = L::WALL_CLING_F;
        let cling_root = assets.0["p1"].motion_root_at(
            &common.0,
            cling_pose,
            assets.0["p1"].poses[&cling_pose].start as f32,
        );
        assert!(apply_wall_cling_pose_now(
            Some(&assets),
            Some(&common),
            &fighter,
            &mut pose,
            &mut blend,
            &mut air,
            cling_pose,
        ));
        assert_eq!(pose.pose_id, cling_pose);
        assert_eq!(air.motion_root_pin, Some(cling_root));
        assert_eq!(air.motion_root_pin_pose_id, cling_pose);
        assert!(air.late_visual_pose_snap);
    }

    #[test]
    fn ground_weapon_switch_consumes_existing_carry_once_and_stops_before_tail_tick() {
        const SWITCH_POSE: u16 = meteor_combat::weapon_strategy::WEAPON_SWITCH_POSE_GROUND;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let character = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root),
            "p1",
        )
        .expect("p1 assets");
        let common = meteor_asset::amb::parse_amb(
            &std::fs::read(root.join("Common/character.amb")).expect("character.amb"),
        )
        .expect("parse character.amb");
        let pose_def = character.poses[&SWITCH_POSE].clone();
        let bank = character.amb_for_pose(&common, SWITCH_POSE);
        let base_index = character.dummy_order.get("d_base").copied();
        let sample_root = |frame: f32| {
            let sampled = bank.sample_at(frame);
            base_index
                .and_then(|index| sampled.dummy_pos.get(index).copied())
                .unwrap_or(sampled.root_pivot)
        };
        let start_root = sample_root(pose_def.start as f32);
        let motion_frame = (pose_def.start + 1).min(pose_def.end) as f32;
        let current_root = sample_root(motion_frame);
        let (anim_x, anim_z) = meteor_anim::root_motion::root_delta_to_world_xz(
            current_root[0] - start_root[0],
            current_root[1] - start_root[1],
            0.0,
            1.0,
        );
        let dt = 1.0 / 30.0;
        let (_, _, expected_x, expected_z) = compose_dodge_planar_step(
            Vec2::new(anim_x, anim_z),
            Vec2::new(0.0, 6.0),
            Vec2::ZERO,
            CARRY_DECAY_GROUND,
            dt,
        );
        let mut pose = PoseRuntime::idle();
        character.apply_pose(&mut pose, SWITCH_POSE, true);
        pose.frame = motion_frame;
        let pose_instance_id = pose.pose_instance_id;
        let mut app = App::new();
        app.init_resource::<Time>();
        app.insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
            std::collections::HashMap::from([("p1".to_string(), character)]),
        ));
        app.insert_resource(crate::plugins::character_plugin::CommonAmb(common));
        app.insert_resource(StageCollider {
            mesh: Some(crate::plugins::stage_collider::StageMeshCollider::default()),
        });
        app.add_systems(Update, weapon_switch_root_motion);
        let entity = app
            .world_mut()
            .spawn((
                Fighter {
                    character_id: 1,
                    player_slot: 0,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::default(),
                MotionState {
                    movement_mode: MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                FighterAir::default(),
                WeaponSwitchActive {
                    pose_id: SWITCH_POSE,
                    pose_instance_id,
                    marker_frame: pose_def.loop_start,
                    end_frame: pose_def.end,
                    carry_z: 6.0,
                    root_started: true,
                    prev_root_x: start_root[0],
                    prev_root_y: start_root[1],
                    ..default()
                },
                pose,
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(dt));

        app.update();

        let transform = app.world().get::<Transform>(entity).unwrap();
        assert!((transform.translation.x - expected_x).abs() < 1e-5);
        assert!((transform.translation.z - expected_z).abs() < 1e-5);
        let switching = app.world().get::<WeaponSwitchActive>(entity).unwrap();
        assert!((switching.carry_z - 4.2).abs() < 1e-5);

        let before_tail = transform.translation;
        app.world_mut()
            .get_mut::<WeaponSwitchActive>(entity)
            .unwrap()
            .end_sampled = true;
        app.world_mut()
            .get_mut::<PoseRuntime>(entity)
            .unwrap()
            .frame = pose_def.end as f32;
        app.update();
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            before_tail
        );
    }

    #[test]
    fn ground_self_launch_initializes_curve_and_visual_pin() {
        let mut air = FighterAir {
            ledge_fall: true,
            ground_contact: false,
            ..default()
        };
        let mut wall = FighterWall {
            cling_active: true,
            contact_seen: true,
            external_accel: Vec3::ONE,
            ..default()
        };
        let mut motion = MotionState {
            movement_mode: MovementMode::Idle,
            velocity: Vec3::ZERO,
            facing: 0.0,
        };
        begin_ground_self_launch(
            &mut air,
            &mut wall,
            &mut motion,
            253,
            [1.0, 2.0, -9.0],
            [4.0, 5.0, 0.25],
        );

        assert!(air.air.airborne);
        assert_eq!(air.air.motion_mode, AirMotionMode::Curve);
        assert_eq!(air.air.launch_pose_id, 253);
        assert_eq!(air.air.prev_root_z, Some(-9.0));
        assert_eq!(air.motion_root_pin, Some([4.0, 5.0, 0.25]));
        assert_eq!(
            (air.stand_motion_root_x, air.stand_motion_root_y),
            (4.0, 5.0)
        );
        assert!(air.ground_contact, "launch charge remains grounded");
        assert!(!air.ledge_fall);
        assert!(!wall.cling_active);
        assert!(!wall.contact_seen);
        assert_eq!(wall.external_accel, Vec3::ZERO);
        assert_eq!(motion.movement_mode, MovementMode::Airborne);
    }

    #[test]
    fn self_launch_prepare_gate_is_shared_by_player_and_npc_states() {
        for state in [
            FighterState::Attack1,
            FighterState::Attack2,
            FighterState::Attack3,
        ] {
            assert!(should_begin_ground_self_launch(state, false, 253));
        }
        assert!(!should_begin_ground_self_launch(
            FighterState::Name,
            false,
            253
        ));
        assert!(!should_begin_ground_self_launch(
            FighterState::Attack1,
            true,
            253
        ));
        assert!(!should_begin_ground_self_launch(
            FighterState::Attack1,
            false,
            252
        ));
    }

    #[test]
    fn authored_curve_owner_is_distinct_from_mode0_ballistics() {
        assert!(air_curve_motion_owned(AirMotionMode::Curve, false));
        assert!(air_curve_motion_owned(AirMotionMode::CurveExit, false));
        assert!(air_curve_motion_owned(AirMotionMode::Ballistic, true));
        assert!(!air_curve_motion_owned(AirMotionMode::Ballistic, false));
        assert!(!air_curve_motion_owned(AirMotionMode::Wall, false));
    }

    #[test]
    fn landed_air_weapon_switch_rebases_for_next_ground_tick_and_keeps_carry() {
        let mut switch = WeaponSwitchActive {
            airborne: true,
            carry_x: 3.0,
            carry_z: -6.0,
            root_started: true,
            prev_root_x: 12.0,
            prev_root_y: -4.0,
            ..default()
        };

        assert!(handoff_landed_air_weapon_switch(&mut switch));
        assert!(!switch.airborne);
        assert!(!switch.root_started);
        assert_eq!((switch.carry_x, switch.carry_z), (3.0, -6.0));
        assert_eq!((switch.prev_root_x, switch.prev_root_y), (12.0, -4.0));
        assert!(!handoff_landed_air_weapon_switch(&mut switch));
    }

    #[test]
    fn npc_self_launch_consumes_the_advanced_pose_in_root_motion() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let character = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root),
            "p0",
        )
        .expect("p0 assets");
        let common = meteor_asset::amb::parse_amb(
            &std::fs::read(root.join("Common/character.amb")).expect("character.amb"),
        )
        .expect("parse character.amb");
        let pose_def = character.poses.get(&253).expect("pose 253");
        let bank = character.amb_for_pose(&common, 253);
        let _ = &bank;

        let start_z = character.motion_root_at(&common, 253, pose_def.start as f32)[2];
        let lift_frame = (pose_def.start + 1..=pose_def.end)
            .find(|frame| character.motion_root_at(&common, 253, *frame as f32)[2] > start_z + 0.01)
            .expect("pose 253 authored lift") as f32;
        let mut pose = PoseRuntime::idle();
        character.apply_pose(&mut pose, 253, true);

        let mut app = App::new();
        app.insert_resource(Time::<()>::default());
        app.insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
            std::collections::HashMap::from([("p0".to_string(), character)]),
        ));
        app.insert_resource(crate::plugins::character_plugin::CommonAmb(common));
        app.insert_resource(StageCollider {
            mesh: Some(crate::plugins::stage_collider::StageMeshCollider::default()),
        });
        app.insert_resource(TestPoseFrame(lift_frame));
        app.add_systems(
            Update,
            (
                prepare_air_attack_modes,
                set_test_pose_frame,
                air_curve_root_motion,
            )
                .chain(),
        );
        let npc = app
            .world_mut()
            .spawn((
                crate::plugins::combat_plugin::NpcFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 1,
                    weapon: 5,
                    weapon_slots: [5, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                pose,
                crate::components::fighter::EntryBlend::default(),
                SmState {
                    current: FighterState::Attack1,
                    attack_stage: 1,
                    ..default()
                },
                CombatState {
                    state: FighterState::Attack1,
                },
                FighterAir::default(),
                FighterWall::default(),
                FighterHitReaction::default(),
                MotionState {
                    movement_mode: MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                Transform::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.0 / 30.0));

        app.update();

        let air = app.world().get::<FighterAir>(npc).unwrap();
        assert!(air.air.airborne);
        assert!(air.air.launch_lifted);
        assert_eq!(air.air.launch_pose_id, 253);
        assert!(app.world().get::<Transform>(npc).unwrap().translation.y > 0.0);
    }

    #[test]
    fn self_launch_positive_curve_beats_charge_hold() {
        assert_eq!(
            select_air_vertical_path(true, true, false, false),
            AirVerticalPath::AuthoredCurve
        );
        assert_eq!(
            select_air_vertical_path(false, true, false, false),
            AirVerticalPath::LaunchChargeHold
        );
        assert_eq!(
            select_air_vertical_path(false, true, true, false),
            AirVerticalPath::Ballistic
        );
    }

    #[test]
    fn linked_air_attack_owns_landing_until_its_authored_link() {
        assert!(attack_clip_owns_landing(true, false, Some(342), true));
        assert!(!attack_clip_owns_landing(false, false, Some(342), true));
        assert!(!attack_clip_owns_landing(true, true, Some(342), true));
        assert!(!attack_clip_owns_landing(true, false, None, false));
        assert!(!attack_clip_owns_landing(true, false, Some(999), false));
    }

    #[test]
    fn key_mapping_wasd() {
        let keymap = crate::app_state::Keymap::default();
        assert_eq!(key_to_dir(&KeyCode::KeyW, &keymap), Some(Dir::Up));
        assert_eq!(key_to_dir(&KeyCode::KeyS, &keymap), Some(Dir::Down));
        assert_eq!(key_to_dir(&KeyCode::KeyA, &keymap), Some(Dir::Left));
        assert_eq!(key_to_dir(&KeyCode::KeyD, &keymap), Some(Dir::Right));
        assert_eq!(key_to_dir(&KeyCode::Space, &keymap), None);

        let custom = crate::app_state::Keymap {
            move_up: crate::app_state::InputBinding::Key(KeyCode::KeyI),
            ..default()
        };
        assert_eq!(key_to_dir(&KeyCode::KeyW, &custom), None);
        assert_eq!(key_to_dir(&KeyCode::KeyI, &custom), Some(Dir::Up));
    }

    #[test]
    fn world_direction_rotates_input_by_yaw() {
        let f = world_direction(Vec3::new(0.0, 0.0, 1.0), 0.0);
        assert!((f.z - 1.0).abs() < 1e-5 && f.x.abs() < 1e-5);
        let r = world_direction(Vec3::new(1.0, 0.0, 0.0), 0.0);
        assert!((r.x + 1.0).abs() < 1e-5 && r.z.abs() < 1e-5);

        let f = world_direction(Vec3::new(0.0, 0.0, 1.0), std::f32::consts::FRAC_PI_2);
        assert!((f.x - 1.0).abs() < 1e-5 && f.z.abs() < 1e-5);
        let r = world_direction(Vec3::new(1.0, 0.0, 0.0), std::f32::consts::FRAC_PI_2);
        assert!((r.z - 1.0).abs() < 1e-5 && r.x.abs() < 1e-5);
    }

    #[test]
    fn directional_input_keeps_discrete_axes_and_forward_strafe_weight() {
        let keymap = crate::app_state::Keymap::default();
        let mut keys = ButtonInput::<KeyCode>::default();
        let mouse = ButtonInput::<MouseButton>::default();
        keys.press(KeyCode::KeyW);
        keys.press(KeyCode::KeyD);
        let raw = directional_input(&keys, &mouse, &keymap);
        assert_eq!(raw, Vec3::new(1.0, 0.0, 1.0));
        let loco = compose_loco_axes(raw.x, raw.z, FORWARD_STRAFE_SCALE);
        assert_eq!(loco.local_x, FORWARD_STRAFE_SCALE);
        assert_eq!(loco.local_z, 1.0);
    }

    #[test]
    fn opposing_forward_keys_do_not_upgrade_strafe_to_run_speed() {
        let keymap = crate::app_state::Keymap::default();
        let mut keys = ButtonInput::<KeyCode>::default();
        let mouse = ButtonInput::<MouseButton>::default();
        keys.press(KeyCode::KeyW);
        keys.press(KeyCode::KeyS);
        keys.press(KeyCode::KeyD);
        let raw = directional_input(&keys, &mouse, &keymap);
        let loco = compose_loco_axes(raw.x, raw.z, FORWARD_STRAFE_SCALE);
        assert_eq!(loco.fb, 0);
        assert!(!is_running(loco.running, &keys, &mouse, &keymap));
    }

    #[test]
    fn parse_command_string_maps_dirs() {
        let dirs = meteor_input::command_parser::parse_command_string("uldr");
        assert_eq!(dirs, vec![Dir::Up, Dir::Left, Dir::Down, Dir::Right]);
    }

    #[test]
    fn dodge_pose_matches_input() {
        let keymap = crate::app_state::Keymap::default();

        let mut keys = ButtonInput::<KeyCode>::default();
        let mouse = ButtonInput::<MouseButton>::default();
        assert_eq!(dodge_pose_for_input(&keys, &mouse, &keymap, 0, false), 165);
        keys.press(KeyCode::KeyD);
        assert_eq!(dodge_pose_for_input(&keys, &mouse, &keymap, 0, false), 168);
        keys.release(KeyCode::KeyD);
        keys.press(KeyCode::KeyA);
        assert_eq!(dodge_pose_for_input(&keys, &mouse, &keymap, 0, false), 171);
        keys.release(KeyCode::KeyA);
        keys.press(KeyCode::KeyS);
        assert_eq!(dodge_pose_for_input(&keys, &mouse, &keymap, 0, false), 174);

        assert_eq!(
            dodge_pose_for_input(&keys, &mouse, &keymap, 0, true),
            L::BURST_B
        );
    }
}
