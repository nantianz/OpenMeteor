use crate::app_state::AppState;
use crate::components::fighter::{
    AiIntent, CombatState, EntryBlend, Fighter, FighterHitReaction, Health, MotionState,
    PoseRuntime, RangedFireGate, SmState,
};
use crate::plugins::character_contact_plugin::{
    commit_fighter_position, resolve_fighter_xz, CharacterContactFrame,
};
use crate::plugins::character_plugin::CharacterAssetsSet;
use crate::plugins::combat_plugin::NpcFighter;
use crate::plugins::movement_plugin::{
    accepted_motion_velocity, apply_dodge_pose_now, clamp_air_motion_step, enter_dodge_state,
    finish_air_landing_state, ordinary_locomotion_allowed, project_air_velocity_on_surface,
    project_planar_air_velocity_on_contact, sample_air_ground, update_ground_external_motion,
    wheel_vetoes_dodge_action, FighterAir, FighterDodge, COLLIDER_HEIGHT, COLLIDER_RADIUS,
};
use crate::plugins::network_plugin::RemoteFighter;
use crate::plugins::stage_collider::{StageCollider, StageMeshCollider};
use crate::plugins::stage_plugin::{
    LocalFighter, MissionNpc, NpcCombatProfile, NpcPerformance, StageMissionRuntime,
};
use bevy::prelude::*;
use meteor_ai::behavior::{find_waypoint_path, BehaviorMode, Waypoint};
use meteor_core::FighterState;
use meteor_physics::jump::{RUN_SPEED, WALK_SPEED};
use meteor_physics::{
    collision::{CollisionSystem, FootHit, MeshCollider, Point3},
    jump,
};
use meteor_scene::stage_loader::WaypointGraph;

const NPC_FACE_SMOOTH: f32 = 14.0;

#[cfg(test)]
fn path_target(graph: &WaypointGraph, npc_pos: [f32; 3], target_pos: [f32; 3]) -> Option<[f32; 3]> {
    let start = graph.nearest(npc_pos)?;
    let goal = graph.nearest(target_pos)?;
    if start == goal {
        return None;
    }
    let ai_points: Vec<Waypoint> = graph
        .points
        .iter()
        .map(|p| Waypoint {
            id: p.id,
            links: graph.navigation_links(p.id),
        })
        .collect();
    let path = find_waypoint_path(&ai_points, start, goal);
    if path.is_empty() {
        return None;
    }
    graph.world_position(path[0])
}

#[derive(Component, Debug, Clone)]
pub struct NpcPatrol {
    navigation: super::npc_navigation::Navigation,
    patrol_navigation: super::npc_navigation::Navigation,

    pub route: Vec<usize>,
    pub index: usize,
    think_remaining: f32,
    rng: u64,
    wander_goal: Option<usize>,
    flee_goal: Option<usize>,

    current_waypoint: Option<usize>,
    travel_goal: Option<usize>,
    travel_route: Vec<usize>,

    patrol_goal: Option<usize>,
    patrol_route: Vec<usize>,

    command_memory: meteor_ai::combat::CommandMemory,
    sampled_hp: Option<f32>,

    down_recovery_pose_instance_id: Option<u32>,
    down_recovery_remaining: f32,
}

impl Default for NpcPatrol {
    fn default() -> Self {
        Self {
            navigation: default(),
            patrol_navigation: default(),
            route: Vec::new(),
            index: 0,
            think_remaining: 0.0,
            rng: 0x9e37_79b9_7f4a_7c15,
            wander_goal: None,
            flee_goal: None,
            current_waypoint: None,
            travel_goal: None,
            travel_route: Vec::new(),
            patrol_goal: None,
            patrol_route: Vec::new(),
            command_memory: default(),
            sampled_hp: None,
            down_recovery_pose_instance_id: None,
            down_recovery_remaining: 0.0,
        }
    }
}

impl NpcPatrol {
    pub(crate) fn invalidate_navigation(&mut self) {
        self.navigation = default();
        self.patrol_navigation = default();
        self.current_waypoint = None;
        self.wander_goal = None;
        self.flee_goal = None;
        self.travel_goal = None;
        self.travel_route.clear();
        self.patrol_goal = None;
        self.patrol_route.clear();
    }

    pub(crate) fn reset_behavior_state(&mut self) {
        self.navigation = default();
        self.patrol_navigation = default();
        self.think_remaining = 0.0;
        self.sampled_hp = None;
        self.route.clear();
        self.index = 0;
        self.wander_goal = None;
        self.flee_goal = None;
        self.travel_goal = None;
        self.travel_route.clear();
        self.patrol_goal = None;
        self.patrol_route.clear();
    }

    fn travel_step(
        &mut self,
        graph: Option<&WaypointGraph>,
        mesh: Option<&StageMeshCollider>,
        dt: f32,
        from: Vec3,
        target: Vec3,
    ) -> NpcTravelStep {
        if let Some(mesh) = mesh {
            let (direction, arrived) = self.navigation.step(graph, mesh, from, target, dt);
            return NpcTravelStep {
                direction,
                arrived,
                jump_target: self.navigation.jump_target(),
            };
        }
        cached_travel_step(
            graph,
            &mut self.current_waypoint,
            &mut self.travel_goal,
            &mut self.travel_route,
            from,
            target,
        )
    }

    fn patrol_step(
        &mut self,
        graph: Option<&WaypointGraph>,
        mesh: Option<&StageMeshCollider>,
        dt: f32,
        from: Vec3,
        target: Vec3,
    ) -> NpcTravelStep {
        if let Some(mesh) = mesh {
            let (direction, arrived) = self.patrol_navigation.step(graph, mesh, from, target, dt);
            return NpcTravelStep {
                direction,
                arrived,
                jump_target: self.patrol_navigation.jump_target(),
            };
        }
        cached_travel_step(
            graph,
            &mut self.current_waypoint,
            &mut self.patrol_goal,
            &mut self.patrol_route,
            from,
            target,
        )
    }
}

#[cfg(test)]
const PATROL_ROUTE_LEN: usize = 4;

const PATROL_ARRIVE_DIST: f32 = 1.2;

fn npc_attack_range(weapon: u8) -> f32 {
    meteor_ai::combat::weapon_tactics(weapon).attack_range * meteor_core::MODEL_SCALE
}

fn npc_minimum_range(weapon: u8) -> f32 {
    meteor_ai::combat::weapon_tactics(weapon).minimum_range * meteor_core::MODEL_SCALE
}

fn aim_npc_attack(
    intent: &mut AiIntent,
    patrol: &mut NpcPatrol,
    fighter: &Fighter,
    profile: Option<&NpcCombatProfile>,
    from: Vec3,
    target: Vec3,
) {
    if intent.want_attack && meteor_ai::combat::weapon_tactics(fighter.weapon).ranged {
        let rolls = [
            next_npc_rand(&mut patrol.rng),
            next_npc_rand(&mut patrol.rng),
        ]
        .map(|roll| (roll * 0x0100_0000 as f32) as u32);
        intent.ranged_aim = Some(Vec3::from_array(meteor_ai::combat::ranged_aim(
            fighter.weapon,
            from.to_array(),
            target.to_array(),
            profile.map_or(100.0, |p| p.aim),
            rolls,
        )));
    }
}

fn yaw_towards(delta: Vec3) -> Option<f32> {
    (delta.x * delta.x + delta.z * delta.z > 1e-6).then(|| delta.x.atan2(delta.z))
}

fn npc_combat_turn(facing: f32, target: f32, dt: f32) -> f32 {
    let error = npc_facing_error(facing, target);
    facing + error.clamp(-500.0_f32.to_radians() * dt, 500.0_f32.to_radians() * dt)
}

fn npc_facing_error(facing: f32, target: f32) -> f32 {
    (target - facing + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI
}

fn npc_locomotion_input_for_facing(move_dir: Vec2, facing: f32) -> Vec2 {
    if move_dir.length_squared() <= 0.01 {
        return Vec2::ZERO;
    }
    let world = move_dir.normalize();
    let (sin_yaw, cos_yaw) = facing.sin_cos();
    let local_x = world.x * -cos_yaw + world.y * sin_yaw;
    let local_z = world.x * sin_yaw + world.y * cos_yaw;
    if local_z.abs() >= local_x.abs() {
        Vec2::new(0.0, local_z.signum())
    } else {
        Vec2::new(local_x.signum(), 0.0)
    }
}

#[derive(Debug, Clone, Copy)]
struct NpcTravelStep {
    jump_target: Option<Vec3>,
    direction: Vec3,
    arrived: bool,
}

fn cached_travel_step(
    graph: Option<&WaypointGraph>,
    current_waypoint: &mut Option<usize>,
    cached_goal: &mut Option<usize>,
    cached_route: &mut Vec<usize>,
    from: Vec3,
    target: Vec3,
) -> NpcTravelStep {
    let flat_to_target = Vec3::new(target.x - from.x, 0.0, target.z - from.z);
    let direct = || NpcTravelStep {
        jump_target: None,
        direction: flat_to_target.normalize_or_zero(),
        arrived: flat_to_target.length() <= PATROL_ARRIVE_DIST,
    };
    let Some(graph) = graph else {
        return direct();
    };
    let Some(goal) = graph.nearest(target.to_array()) else {
        return if graph.disabled.is_empty() {
            direct()
        } else {
            NpcTravelStep {
                jump_target: None,
                direction: Vec3::ZERO,
                arrived: false,
            }
        };
    };
    if *cached_goal != Some(goal) || cached_route.is_empty() {
        let Some(start) = graph.nearest(from.to_array()) else {
            return direct();
        };
        *current_waypoint = Some(start);
        *cached_goal = Some(goal);
        *cached_route = if start == goal {
            Vec::new()
        } else {
            let points: Vec<Waypoint> = graph
                .points
                .iter()
                .map(|point| Waypoint {
                    id: point.id,
                    links: graph.navigation_links(point.id),
                })
                .collect();
            let route = find_waypoint_path(&points, start, goal);
            if route.is_empty() && !graph.disabled.is_empty() {
                return NpcTravelStep {
                    jump_target: None,
                    direction: Vec3::ZERO,
                    arrived: false,
                };
            }
            route
        };
    }
    let Some(next_id) = cached_route.first().copied() else {
        return direct();
    };
    let Some(next) = graph.world_position(next_id).map(Vec3::from_array) else {
        cached_route.clear();
        return direct();
    };
    let delta = Vec3::new(next.x - from.x, 0.0, next.z - from.z);
    if delta.length() <= PATROL_ARRIVE_DIST {
        *current_waypoint = Some(next_id);
        cached_route.remove(0);
        return NpcTravelStep {
            jump_target: None,
            direction: Vec3::ZERO,
            arrived: false,
        };
    }
    NpcTravelStep {
        jump_target: None,
        direction: delta.normalize_or_zero(),
        arrived: false,
    }
}

fn next_npc_rand(state: &mut u64) -> f32 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    let value = x.wrapping_mul(0x2545_f491_4f6c_dd1d);
    (value >> 40) as f32 / (1u32 << 24) as f32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NpcOpeningAction {
    Attack,
    Guard,
    Dodge,
    Wait,
}

fn choose_npc_opening_action(profile: &NpcCombatProfile, roll: f32) -> NpcOpeningAction {
    match meteor_ai::combat::attack_or_guard(
        profile.guard,
        [profile.attack1, profile.attack2, profile.attack3],
        (roll.clamp(0.0, 0.999_999) * 100.0) as u32,
    ) {
        Some(true) => NpcOpeningAction::Attack,
        Some(false) => NpcOpeningAction::Guard,
        None => NpcOpeningAction::Wait,
    }
}

fn npc_think_interval(npc: Option<&MissionNpc>) -> f32 {
    meteor_ai::combat::decision_interval(npc.map_or(50.0, |npc| npc.think), 0.2)
}

fn update_npc_down_recovery(
    patrol: &mut NpcPatrol,
    reaction: Option<(meteor_combat::hit_reaction::HitReactKind, f32)>,
    pose_instance_id: u32,
    decision_delay: f32,
    dt: f32,
) -> bool {
    let Some((_, down_remaining)) =
        reaction.filter(|(kind, _)| *kind == meteor_combat::hit_reaction::HitReactKind::Down)
    else {
        patrol.down_recovery_pose_instance_id = None;
        patrol.down_recovery_remaining = 0.0;
        return false;
    };
    if patrol.down_recovery_pose_instance_id != Some(pose_instance_id) {
        patrol.down_recovery_pose_instance_id = Some(pose_instance_id);
        patrol.down_recovery_remaining = down_remaining.max(0.0) + decision_delay.max(0.0);
    }
    patrol.down_recovery_remaining = (patrol.down_recovery_remaining - dt.max(0.0)).max(0.0);
    patrol.down_recovery_remaining <= 0.0
}

fn queue_npc_opening_action(
    intent: &mut AiIntent,
    performance: &mut NpcPerformance,
    profile: Option<&NpcCombatProfile>,
    patrol: &mut NpcPatrol,
    weapon: u8,
    airborne: bool,
) {
    intent.want_attack = false;
    intent.want_guard = false;
    intent.dodge_burst = None;
    intent.attack_directions.clear();
    let Some(profile) = profile else {
        intent.want_attack = true;
        return;
    };

    let choice = if next_npc_rand(&mut patrol.rng) * 100.0 < profile.burst {
        NpcOpeningAction::Dodge
    } else {
        choose_npc_opening_action(profile, next_npc_rand(&mut patrol.rng))
    };
    apply_npc_opening_action(intent, performance, profile, patrol, choice);
    if intent.want_attack {
        let category = meteor_ai::combat::attack_category(
            [profile.attack1, profile.attack2, profile.attack3],
            (next_npc_rand(&mut patrol.rng) * 100.0) as u32,
        );
        let reuse = next_npc_rand(&mut patrol.rng) < 0.5;
        let roll = (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
        let command = if airborne {
            meteor_ai::combat::AIR_COMMANDS[roll as usize % 17]
        } else {
            patrol.command_memory.choose(
                meteor_ai::combat::weapon_tactics(weapon),
                category,
                reuse,
                roll,
            )
        };
        intent.attack_directions = meteor_ai::combat::directions(command);
    }
}

fn apply_npc_opening_action(
    intent: &mut AiIntent,
    performance: &mut NpcPerformance,
    _profile: &NpcCombatProfile,
    patrol: &mut NpcPatrol,
    choice: NpcOpeningAction,
) {
    match choice {
        NpcOpeningAction::Attack => intent.want_attack = true,
        NpcOpeningAction::Guard => {
            intent.want_guard = true;
            performance.guard_remaining = intent.attack_cooldown * 2.0;
        }
        NpcOpeningAction::Dodge => {
            let dir = (next_npc_rand(&mut patrol.rng) * 7.0) as usize;
            intent.dodge_dir = [
                Vec2::Y,
                -Vec2::Y,
                -Vec2::Y,
                -Vec2::X,
                -Vec2::X,
                Vec2::X,
                Vec2::X,
            ][dir.min(6)];
            intent.dodge_burst = Some(false);
        }
        NpcOpeningAction::Wait => {}
    }
}

#[cfg(test)]
fn build_patrol_route(graph: &WaypointGraph, npc_pos: [f32; 3]) -> Vec<usize> {
    let mut route = Vec::with_capacity(PATROL_ROUTE_LEN);
    let Some(mut current) = graph.nearest(npc_pos) else {
        return route;
    };
    route.push(current);
    for _ in 0..PATROL_ROUTE_LEN - 1 {
        let next = graph
            .points
            .get(current)
            .and_then(|p| p.links.first())
            .map(|(target, _)| *target);
        match next {
            Some(target) if target != current => {
                current = target;
                route.push(current);
            }
            _ => break,
        }
    }
    route
}

#[cfg(test)]
fn patrol_next_target(
    graph: &WaypointGraph,
    route: &[usize],
    index: usize,
    npc_pos: [f32; 3],
) -> Option<([f32; 3], usize)> {
    let target = graph.world_position(*route.get(index)?)?;
    let dx = target[0] - npc_pos[0];
    let dz = target[2] - npc_pos[2];
    let next_index = if dx * dx + dz * dz <= PATROL_ARRIVE_DIST * PATROL_ARRIVE_DIST {
        (index + 1) % route.len()
    } else {
        index
    };
    Some((target, next_index))
}

#[derive(Debug, Clone, Copy)]
struct AiActorSnapshot {
    id: i64,
    team: u8,
    position: Vec3,
    alive: bool,
}

fn closest_hostile(
    self_id: i64,
    self_team: u8,
    position: Vec3,
    view_range: f32,
    actors: &[AiActorSnapshot],
    mut visible: impl FnMut(AiActorSnapshot) -> bool,
) -> Option<AiActorSnapshot> {
    actors
        .iter()
        .filter(|actor| {
            actor.id != self_id
                && actor.alive
                && (meteor_combat::hit_resolve::team_mask(i32::from(actor.team))
                    & meteor_combat::hit_resolve::team_mask(i32::from(self_team)))
                    == 0
        })
        .filter_map(|actor| {
            let distance = (actor.position - position).length();
            (distance <= view_range && visible(*actor)).then_some((distance, *actor))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, actor)| actor)
}

fn npc_target_visible<M: MeshCollider + ?Sized>(
    origin: Vec3,
    target: Vec3,
    view_range: f32,
    collider: Option<&M>,
) -> bool {
    let delta = target - origin;
    let distance = delta.length();
    if distance > view_range || delta.y.abs() > 12.0 {
        return false;
    }
    if distance <= 0.001 {
        return true;
    }
    let Some(collider) = collider else {
        return true;
    };
    let direction = delta / distance;
    collider
        .raycast_hit(
            Point3::new(origin.x, origin.y + 1.1, origin.z),
            Point3::new(direction.x, direction.y, direction.z),
            distance,
        )
        .is_none_or(|hit| hit.t >= distance - 0.45)
}

fn behavior_scans_enemies(mode: Option<BehaviorMode>) -> bool {
    !matches!(
        mode,
        Some(BehaviorMode::Idle | BehaviorMode::AttackTarget | BehaviorMode::Dodge)
    )
}

fn farthest_reachable_waypoint(
    graph: &WaypointGraph,
    start_position: Vec3,
    threat: Vec3,
) -> Option<usize> {
    let start = graph.nearest(start_position.to_array())?;
    let mut visited = std::collections::HashSet::from([start]);
    let mut queue = std::collections::VecDeque::from([start]);
    let mut best = (start, -1.0f32);
    while let Some(id) = queue.pop_front() {
        let Some(position) = graph.world_position(id) else {
            continue;
        };
        let dx = position[0] - threat.x;
        let dz = position[2] - threat.z;
        let distance = dx * dx + dz * dz;
        if distance > best.1 {
            best = (id, distance);
        }
        if visited.len() >= 20 {
            continue;
        }
        if let Some(point) = graph.points.get(id) {
            for (target, _) in &point.links {
                if !graph.is_enabled(*target) {
                    continue;
                }
                if visited.len() >= 20 {
                    break;
                }
                if visited.insert(*target) {
                    queue.push_back(*target);
                }
            }
        }
    }
    (best.0 != start).then_some(best.0)
}

type NpcAiQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static mut Health,
        &'static mut MotionState,
        &'static mut AiIntent,
        &'static mut Transform,
        &'static mut FighterAir,
        &'static mut NpcPatrol,
        Option<&'static mut SmState>,
        Option<&'static mut CombatState>,
        Option<&'static mut NpcPerformance>,
        Option<&'static mut MissionNpc>,
        Option<&'static NpcCombatProfile>,
        Option<&'static PoseRuntime>,
        &'static FighterHitReaction,
    ),
    (
        With<Fighter>,
        With<NpcFighter>,
        Without<LocalFighter>,
        Without<RemoteFighter>,
    ),
>;

type NpcRemoteTargets<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static Health,
        &'static RemoteFighter,
    ),
    (Without<LocalFighter>, Without<NpcFighter>),
>;

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        let actor_schedule = super::fighter_frame::schedule(app);
        app.add_systems(
            actor_schedule,
            drive_npc_ai
                .in_set(crate::plugins::FighterFrameSet::Movement)
                .run_if(crate::plugins::simulation_frame)
                .run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            actor_schedule,
            move_npc_fighters
                .after(crate::plugins::movement_plugin::move_local_fighter)
                .in_set(crate::plugins::FighterFrameSet::Locomotion)
                .run_if(crate::plugins::simulation_frame)
                .run_if(in_state(AppState::Playing)),
        );
    }
}

fn resolve_target_point(
    slot_target: Option<(&str, f64)>,
    waypoints: Option<&WaypointGraph>,
    player_pos: Vec3,
    npc_positions: &std::collections::HashMap<i64, Vec3>,
) -> Option<Vec3> {
    let (kind, value) = slot_target?;
    match kind {
        "waypoint" => waypoints
            .and_then(|graph| graph.world_position(value as usize))
            .map(|[x, y, z]| Vec3::new(x, y, z)),
        "char" if value as i64 == 0 => Some(player_pos),
        "char" => npc_positions.get(&(value as i64)).copied(),
        _ => None,
    }
}

fn resolve_behavior_actor_ref(
    value: Option<&meteor_scene::pst_runtime::PstValue>,
    script: Option<&StageMissionRuntime>,
    team: u8,
) -> Option<i64> {
    match value? {
        meteor_scene::pst_runtime::PstValue::Num(value) => Some(*value as i64),
        meteor_scene::pst_runtime::PstValue::Str(name) => script
            .and_then(StageMissionRuntime::script)
            .and_then(|runtime| runtime.host().char_id_for_team(name, team)),
        meteor_scene::pst_runtime::PstValue::Array(_) => None,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn advance_npc_motion<M: MeshCollider>(
    entity: Entity,
    mut contacts: Option<&mut CharacterContactFrame>,
    mover: &CollisionSystem<M>,
    transform: &mut Transform,
    motion: &mut MotionState,
    air: &mut FighterAir,
    desired_velocity: Vec3,
    dt: f32,
    defer_ledge_fall: bool,
) {
    if dt <= 0.0 {
        return;
    }
    if air.air.airborne {
        let pos = Point3::new(
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
        );
        let tick_ground = sample_air_ground(air, mover, pos);
        let ground_y = tick_ground.map_or(f32::NEG_INFINITY, |hit| hit.y);
        let vertical_speed_before = air.air.vertical_speed;
        super::movement_plugin::consume_descent_slide(air);
        let vertical_step = air.air.tick_vertical(dt, false);
        let (vx, vy, vz, vertical_step) = clamp_air_motion_step(
            air.air.planar_x,
            air.air.vertical_speed,
            air.air.planar_z,
            vertical_step,
        );
        air.air.planar_x = vx;
        air.air.vertical_speed = vy;
        air.air.planar_z = vz;
        let requested_x = vx * dt;
        let requested_z = vz * dt;
        let (rx, rz, ry, planar_surface) =
            super::character_contact_plugin::resolve_fighter_xz_with_surface(
                contacts.as_deref_mut(),
                entity,
                mover,
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
        let (dy, foot, surface, blocked) = if vertical_step >= 0.0 {
            let (dy, hit, blocked) = mover.resolve_y(vertical_origin, vertical_step);
            (
                dy,
                None::<FootHit>,
                hit.map(|hit| (hit.nx, hit.ny, hit.nz)),
                blocked,
            )
        } else {
            let (dy, hit, blocked) =
                mover.resolve_descent_with_surface(vertical_origin, vertical_step, planar_surface);
            (dy, hit, hit.map(|hit| (hit.nx, hit.ny, hit.nz)), blocked)
        };
        super::movement_plugin::update_descent_slide(air, foot, air.air.vertical_speed, dt);
        super::movement_plugin::record_carry_support(
            air,
            vertical_step,
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
        transform.translation.y = transform.translation.y.max(ground_y);
        motion.velocity = accepted_motion_velocity(rx, transform.translation.y - pos.y, rz, dt);

        let floor_contact =
            vertical_step < 0.0 && meteor_physics::collision::descent_has_support(foot, blocked);
        let near_ground = transform.translation.y <= ground_y + jump::LAND_EPS || floor_contact;
        air.peak_lift = air
            .peak_lift
            .max((transform.translation.y - air.ground_y).max(0.0));
        air.ground_contact = near_ground;
        let can_land = near_ground && jump::lands_on_contact(vertical_speed_before);
        let timed_out = air.air.air_time >= jump::MAX_AIR_TIME
            && (near_ground || air.air.motion_mode != jump::AirMotionMode::Ballistic);
        if can_land || timed_out {
            if transform.translation.y <= ground_y + jump::LAND_EPS {
                transform.translation.y = ground_y;
            }
            finish_air_landing_state(air);
            air.ground_entry_blend_pending = true;
            motion.movement_mode = meteor_core::MovementMode::Idle;
        }
        commit_fighter_position(contacts, entity, transform.translation);
        return;
    }

    let pos = Point3::new(
        transform.translation.x,
        transform.translation.y,
        transform.translation.z,
    );
    let external = update_ground_external_motion(air, mover, pos, dt);
    let (vx, _, vz, _) = meteor_physics::carry_velocity::clamp_composed_velocity_3d(
        desired_velocity.x + external.x,
        0.0,
        desired_velocity.z + external.z,
        meteor_physics::carry_velocity::CARRY_MAX_SPEED,
    );
    let requested = Vec3::new(vx, 0.0, vz) * dt;
    let (rx, rz, ry) = resolve_fighter_xz(
        contacts.as_deref_mut(),
        entity,
        mover,
        pos,
        requested.x,
        requested.z,
        false,
    );
    transform.translation.x += rx;
    transform.translation.z += rz;
    transform.translation.y += ry;
    let mut accepted_x = rx / dt;
    let mut accepted_z = rz / dt;
    if rx.abs() < requested.x.abs() * 0.5 {
        accepted_x = 0.0;
    }
    if rz.abs() < requested.z.abs() * 0.5 {
        accepted_z = 0.0;
    }
    motion.velocity = Vec3::new(accepted_x, 0.0, accepted_z);

    let settled = Point3::new(
        transform.translation.x,
        transform.translation.y,
        transform.translation.z,
    );
    let ground = mover.sample_ground_hit(settled.x, settled.z, settled.y);
    if let Some(hit) = ground {
        air.ground_y = hit.y;
    }
    let gap = settled.y - air.ground_y;
    if ground.is_some_and(|hit| {
        settled.y - hit.y <= jump::STEP_DOWN
            && mover.can_occupy(Point3::new(settled.x, hit.y, settled.z))
    }) {
        transform.translation.y = ground.expect("checked support").y;
        commit_fighter_position(contacts, entity, transform.translation);
        return;
    }
    let (dy, foot, blocked) = mover.resolve_descent(settled, -jump::STEP_DOWN);
    let walkable = meteor_physics::collision::descent_has_support(foot, blocked);
    if walkable {
        transform.translation.y += dy.min(0.0);
        air.pending_ledge_fall = false;
    } else if defer_ledge_fall {
        air.pending_ledge_fall = true;
    } else {
        air.air.start_fall(motion.velocity.x, motion.velocity.z);
        if let Some(hit) = ground {
            air.ground_y = hit.y;
        }
        air.ground_contact = false;
        air.ledge_fall = true;
        air.peak_lift = gap.max(0.0);
        motion.movement_mode = meteor_core::MovementMode::Airborne;
    }
    commit_fighter_position(contacts, entity, transform.translation);
}

type NpcBallisticAirQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Transform,
        &'static mut MotionState,
        &'static mut FighterAir,
        &'static FighterHitReaction,
        Option<&'static AiIntent>,
        Option<&'static crate::components::fighter::FighterStatusEffects>,
    ),
    (
        With<NpcFighter>,
        Without<LocalFighter>,
        Without<RemoteFighter>,
    ),
>;

pub(crate) fn npc_ballistic_air_root_motion(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    collider: Option<Res<StageCollider>>,
    mut contacts: Option<ResMut<CharacterContactFrame>>,
    mut npcs: NpcBallisticAirQuery<'_, '_>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    if dt <= 0.0 {
        return;
    }
    let mut mover: CollisionSystem<&StageMeshCollider> =
        CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    if let Some(mesh) = collider
        .as_deref()
        .and_then(|collider| collider.mesh.as_ref())
    {
        mover.set_mesh_collider(mesh);
    }
    for (entity, mut transform, mut motion, mut air, hit_reaction, intent, status) in &mut npcs {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        if hit_reaction.current.is_some() {
            continue;
        }
        if air.pending_ledge_fall && !air.air.airborne {
            let ground = mover.sample_ground_hit(
                transform.translation.x,
                transform.translation.z,
                transform.translation.y,
            );
            air.air.start_fall(motion.velocity.x, motion.velocity.z);
            if let Some(hit) = ground {
                air.ground_y = hit.y;
            }
            air.pending_ledge_fall = false;
            air.ground_contact = false;
            air.ledge_fall = true;
            air.peak_lift = (transform.translation.y - air.ground_y).max(0.0);
            motion.movement_mode = meteor_core::MovementMode::Airborne;
            continue;
        }
        if !air.air.airborne {
            continue;
        }
        if crate::plugins::movement_plugin::air_curve_motion_owned(air.air.motion_mode, false) {
            continue;
        }
        if let Some(intent) = intent.filter(|intent| intent.navigation_jump_target.is_some()) {
            super::movement_plugin::tick_air_input_drive(
                &mut air,
                dt,
                Vec3::new(intent.locomotion_input.x, 0.0, intent.locomotion_input.y),
                motion.facing,
                false,
                status.map_or(1.0, |status| status.effects.speed_factor(1.0)),
                false,
            );
        }
        advance_npc_motion(
            entity,
            contacts.as_deref_mut(),
            &mover,
            &mut transform,
            &mut motion,
            &mut air,
            Vec3::ZERO,
            dt,
            false,
        );
    }
}

fn npc_ordinary_locomotion_allowed(sm: Option<&SmState>) -> bool {
    sm.is_none_or(|state| ordinary_locomotion_allowed(state, false))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_npc_ai(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    waypoints: Option<Res<WaypointGraph>>,
    collider: Option<Res<StageCollider>>,
    script: Option<Res<StageMissionRuntime>>,
    net: Option<Res<crate::plugins::network_plugin::NetState>>,
    qi_values: Query<&crate::components::fighter::Qi>,
    mut npcs: NpcAiQuery<'_, '_>,

    players: Query<(Entity, &Transform, &Health), With<LocalFighter>>,
    remotes: NpcRemoteTargets<'_, '_>,
    mut dialogue: Option<ResMut<crate::plugins::ui_plugin::DialogueState>>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(11);
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let performance_dt =
        crate::plugins::simulation_wall_step(frame_clock.as_deref(), time.delta_secs());
    let Some((player_entity, player_pos, player_alive)) = players
        .iter()
        .next()
        .map(|(entity, transform, health)| (entity, transform.translation, health.current > 0.0))
    else {
        return;
    };

    #[cfg(not(target_arch = "wasm32"))]
    let snapshot_perf = super::perf_plugin::Scope::new(19);
    let npc_positions: std::collections::HashMap<i64, Vec3> = npcs
        .iter()
        .filter_map(|(_, _, _, _, _, transform, _, _, _, _, _, npc, _, _, _)| {
            npc.map(|npc| (npc.id, transform.translation))
        })
        .collect();

    let default_net = crate::plugins::network_plugin::NetState::default();
    let net = net.as_deref().unwrap_or(&default_net);
    let player_team =
        crate::plugins::combat_plugin::fighter_combat_team(net, true, None, None) as u8;
    let mut actor_entities = std::collections::HashMap::from([(0, player_entity)]);
    let mut actors = vec![AiActorSnapshot {
        id: 0,
        team: player_team,
        position: player_pos,
        alive: player_alive,
    }];
    actors.extend(npcs.iter().map(
        |(entity, _, health, _, _, transform, _, _, _, _, _, npc, _, _, _)| {
            let id = npc.map_or(-1 - i64::from(entity.index_u32()), |npc| npc.id);
            actor_entities.insert(id, entity);
            AiActorSnapshot {
                id,
                team: crate::plugins::combat_plugin::fighter_combat_team(net, false, None, npc)
                    as u8,
                position: transform.translation,
                alive: health.current > 0.0,
            }
        },
    ));
    for (entity, transform, health, remote) in &remotes {
        let id = -1 - i64::from(entity.index_u32());
        actor_entities.insert(id, entity);
        actors.push(AiActorSnapshot {
            id,
            team: crate::plugins::combat_plugin::fighter_combat_team(net, false, Some(remote), None)
                as u8,
            position: transform.translation,
            alive: health.current > 0.0,
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    drop(snapshot_perf);
    for (
        entity,
        fighter,
        health,
        mut motion,
        mut intent,
        mut transform,
        air,
        mut patrol,
        mut sm_state,
        mut combat_state,
        mut performance,
        mut mission_npc,
        combat_profile,
        pose,
        hit_reaction,
    ) in &mut npcs
    {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }

        intent.locomotion_input = Vec2::ZERO;
        if !air.air.airborne || health.current <= 0.0 {
            intent.navigation_jump_target = None;
        }
        intent.want_qi_burst = false;

        if patrol.rng == 0x9e37_79b9_7f4a_7c15 {
            patrol.rng ^= (u64::from(entity.index_u32()) + 1).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        }

        if health.current <= 0.0 {
            intent.move_dir = Vec2::ZERO;
            intent.want_attack = false;
            intent.want_taunt = false;
            intent.want_guard = false;
            intent.dodge_burst = None;
            intent.enemy_id = None;
            intent.enemy_entity = None;
            intent.lock_target = None;
            intent.ranged_aim = None;
            intent.attack_directions.clear();
            patrol.sampled_hp = None;
            if let Some(performance) = performance.as_deref_mut() {
                performance.crouching = false;
                performance.jump_requested = false;
                performance.use_items.clear();
            }
            continue;
        }

        if intent.attack_cooldown > 0.0 {
            intent.attack_cooldown -= dt;
        }
        let npc_pos = transform.translation;
        let damaged = patrol
            .sampled_hp
            .is_some_and(|previous| health.current < previous);

        if patrol.sampled_hp.is_none() {
            patrol.sampled_hp = Some(health.current);
        }
        let can_react =
            behavior_scans_enemies(mission_npc.as_deref().and_then(|npc| npc.forced_behavior))
                && performance.as_deref().is_none_or(|performance| {
                    performance.pause_remaining <= 0.0 && performance.queue.is_empty()
                });
        if intent.attack_cooldown <= 0.0 && can_react {
            patrol.sampled_hp = Some(health.current);
            let enemy = intent
                .enemy_id
                .and_then(|id| actors.iter().find(|actor| actor.id == id && actor.alive));
            if let Some(enemy) = enemy.filter(|_| damaged) {
                let roll = (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
                if meteor_ai::combat::reacts_with_qi_burst(
                    damaged,
                    qi_values.get(entity).map_or(0.0, |qi| qi.current),
                    (enemy.position - npc_pos).length(),
                    roll,
                ) {
                    intent.want_qi_burst = true;
                    intent.want_attack = false;
                    intent.want_guard = false;
                    intent.dodge_burst = None;
                    intent.move_dir = Vec2::ZERO;
                    intent.attack_cooldown = npc_think_interval(mission_npc.as_deref());
                    continue;
                }
            }
        }
        let request_getup = update_npc_down_recovery(
            &mut patrol,
            hit_reaction
                .current
                .as_ref()
                .map(|reaction| (reaction.kind, reaction.down_remaining)),
            pose.map_or(0, |pose| pose.pose_instance_id),
            npc_think_interval(mission_npc.as_deref()),
            dt,
        );

        let perform_reaction_ready = performance.as_deref().is_some_and(|performance| {
            performance.queue.front().is_some_and(|command| {
                sm_state
                    .as_deref()
                    .is_some_and(|state| match command.action.as_str() {
                        "guard" => matches!(
                            state.current,
                            FighterState::Guard | FighterState::Down | FighterState::Fake
                        ),
                        "aggress" => {
                            matches!(state.current, FighterState::Down | FighterState::Fake)
                        }
                        _ => false,
                    })
            })
        });
        if hit_reaction.current.is_some() && !perform_reaction_ready {
            if let Some(performance) = performance.as_deref_mut() {
                if performance
                    .queue
                    .front()
                    .is_some_and(|command| matches!(command.action.as_str(), "guard" | "aggress"))
                {
                    performance.clock.next_eligible(
                        &mut performance.queue,
                        performance_dt,
                        false,
                        false,
                    );
                }
            }
            if intent.attack_cooldown <= 0.0 {
                intent.attack_cooldown = npc_think_interval(mission_npc.as_deref());
            }
            intent.move_dir = Vec2::ZERO;
            intent.want_attack = false;
            intent.want_guard = request_getup;
            intent.dodge_burst = None;
            motion.movement_mode = if air.air.airborne {
                meteor_core::MovementMode::Airborne
            } else {
                meteor_core::MovementMode::Idle
            };
            continue;
        }

        let mut empty_performance = NpcPerformance::default();
        let performance = performance.as_deref_mut().unwrap_or(&mut empty_performance);
        if performance.pause_remaining > 0.0 {
            performance.pause_remaining = (performance.pause_remaining - dt).max(0.0);
            intent.move_dir = Vec2::ZERO;
            intent.want_attack = false;
            motion.movement_mode = if performance.crouching {
                meteor_core::MovementMode::Crouch
            } else {
                meteor_core::MovementMode::Idle
            };
            continue;
        }
        let mut perform_owns_tick =
            !performance.queue.is_empty() && performance.guard_remaining <= 0.0;
        let mut perform_attack = false;

        let next = if performance.guard_remaining > 0.0 {
            None
        } else {
            performance.clock.next_eligible(
                &mut performance.queue,
                performance_dt,
                !air.air.airborne
                    && sm_state.as_deref().is_none_or(|state| {
                        matches!(
                            state.current,
                            FighterState::Name | FighterState::Down | FighterState::Fake
                        )
                    }),
                !air.air.airborne
                    && sm_state.as_deref().is_none_or(|state| {
                        matches!(
                            state.current,
                            FighterState::Name
                                | FighterState::Guard
                                | FighterState::Down
                                | FighterState::Fake
                        )
                    }),
            )
        };
        if let Some(command) = next {
            match command.action.as_str() {
                "pause" => {}
                "say" => {
                    let text = command
                        .args
                        .iter()
                        .map(|value| match value {
                            meteor_scene::pst_runtime::PstValue::Str(text) => text.clone(),
                            meteor_scene::pst_runtime::PstValue::Num(value) => format!("{value}"),
                            meteor_scene::pst_runtime::PstValue::Array(_) => String::new(),
                        })
                        .collect::<String>();
                    let name = mission_npc
                        .as_deref()
                        .map(|npc| npc.name.as_str())
                        .unwrap_or("");
                    if let Some(dialogue) = dialogue.as_deref_mut() {
                        dialogue.text = Some(format!("{name}：{text}"));
                        dialogue.timer = 3.0;
                    }
                }
                "faceto" => {
                    let target_id = command
                        .args
                        .first()
                        .map(|value| value.as_f64() as i64)
                        .unwrap_or(0);
                    let target = if target_id == 0 {
                        Some(player_pos)
                    } else {
                        npc_positions.get(&target_id).copied()
                    };
                    if let Some(target) = target {
                        let delta = target - transform.translation;
                        motion.facing = delta.x.atan2(delta.z);

                        transform.rotation = Quat::IDENTITY;
                    }
                }
                "guard" => {}
                "aggress" => {
                    intent.want_taunt = true;
                }
                "attack" => perform_attack = true,
                "crouch" => {
                    performance.crouching = command
                        .args
                        .first()
                        .map(meteor_scene::pst_runtime::PstValue::truthy)
                        .unwrap_or(true)
                }
                "jump" => {
                    performance.jump_requested = true;
                    perform_owns_tick = true;
                }
                "use" => {
                    if let Some(id) = command
                        .args
                        .first()
                        .map(meteor_scene::pst_runtime::PstValue::as_f64)
                        .filter(|id| id.is_finite() && *id > 0.0 && id.fract() == 0.0)
                    {
                        performance.use_items.push(id as u32);
                    }
                }
                _ => warn!("unsupported NPC Perform command: {}", command.action),
            }
        }
        if performance.clock.guard_held() {
            intent.want_guard = true;
        }
        if perform_owns_tick {
            intent.move_dir = Vec2::ZERO;
            intent.want_attack = perform_attack;
            motion.movement_mode = if performance.crouching {
                meteor_core::MovementMode::Crouch
            } else {
                meteor_core::MovementMode::Idle
            };
            continue;
        }

        if sm_state.as_deref().is_some_and(|state| {
            matches!(
                state.current,
                FighterState::Attack1 | FighterState::Attack2 | FighterState::Attack3
            )
        }) {
            performance.guard_remaining = 0.0;
        }
        if performance.guard_remaining > 0.0 {
            performance.guard_remaining = (performance.guard_remaining - dt).max(0.0);
            intent.move_dir = Vec2::ZERO;
            intent.want_attack = perform_attack;
            motion.movement_mode = meteor_core::MovementMode::Idle;
            if performance.guard_remaining <= 0.0 {
                if let Some(sm_state) = sm_state.as_deref_mut() {
                    sm_state.current = meteor_core::FighterState::Name;
                    sm_state.locked = false;
                }
                if let Some(combat_state) = combat_state.as_deref_mut() {
                    combat_state.state = meteor_core::FighterState::Name;
                }
            }
            continue;
        }
        let attacking = sm_state.as_deref().is_some_and(|state| {
            matches!(
                state.current,
                FighterState::Attack1 | FighterState::Attack2 | FighterState::Attack3
            )
        });
        let forced = mission_npc.as_deref().and_then(|npc| npc.forced_behavior);
        patrol.think_remaining -= dt;
        let thought_tick = patrol.think_remaining <= 0.0;
        if thought_tick {
            patrol.think_remaining = npc_think_interval(mission_npc.as_deref());
        }

        let actor_ref = matches!(
            forced,
            Some(BehaviorMode::Follow | BehaviorMode::Kill | BehaviorMode::Dodge)
        )
        .then(|| {
            mission_npc.as_deref().and_then(|npc| {
                resolve_behavior_actor_ref(npc.behavior_args.first(), script.as_deref(), npc.team)
            })
        })
        .flatten();
        let configured_target = actor_ref.and_then(|id| {
            actors
                .iter()
                .find(|actor| actor.id == id && actor.alive)
                .map(|actor| (id, actor.position))
        });

        if thought_tick {
            intent.enemy_id = mission_npc.as_deref().and_then(|npc| {
                behavior_scans_enemies(forced).then(|| {
                    let mut view = npc.view.max(0.0) * meteor_core::MODEL_SCALE;
                    if forced == Some(BehaviorMode::Follow) {
                        view = view.min(200.0 * meteor_core::MODEL_SCALE);
                    }
                    closest_hostile(npc.id, npc.team, npc_pos, view, &actors, |actor| {
                        npc_target_visible(
                            npc_pos,
                            actor.position,
                            view,
                            collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
                        )
                    })
                    .map(|enemy| enemy.id)
                    .or_else(|| {
                        (forced == Some(BehaviorMode::Kill))
                            .then_some(configured_target.map(|(id, _)| id))
                            .flatten()
                    })
                })?
            });
            if mission_npc.is_none() {
                let id = -1 - i64::from(entity.index_u32());
                let team =
                    crate::plugins::combat_plugin::fighter_combat_team(net, false, None, None)
                        as u8;
                let view = 400.0 * meteor_core::MODEL_SCALE;
                intent.enemy_id = closest_hostile(id, team, npc_pos, view, &actors, |actor| {
                    npc_target_visible(
                        npc_pos,
                        actor.position,
                        view,
                        collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
                    )
                })
                .map(|actor| actor.id);
            }
        }

        let enemy = intent.enemy_id.and_then(|id| {
            actors
                .iter()
                .find(|actor| actor.id == id && actor.alive)
                .copied()
        });
        intent.enemy_entity = enemy.and_then(|enemy| actor_entities.get(&enemy.id).copied());
        if enemy.is_none() {
            intent.enemy_id = None;
        }
        let tactics = meteor_ai::combat::weapon_tactics(fighter.weapon);
        if let Some(enemy) = enemy {
            if !tactics.ranged
                && intent.lock_target.is_none()
                && (enemy.position - npc_pos).length() <= npc_attack_range(fighter.weapon)
            {
                intent.lock_target = intent.enemy_entity;
            }
        }
        if air.air.airborne && !attacking {
            if let Some(target) = intent.navigation_jump_target {
                let delta = Vec3::new(target.x - npc_pos.x, 0.0, target.z - npc_pos.z);
                intent.move_dir = delta.normalize_or_zero().xz();
                if let Some(yaw) = yaw_towards(delta) {
                    motion.facing =
                        meteor_core::math::lerp_angle(motion.facing, yaw, NPC_FACE_SMOOTH * dt);
                }
                intent.locomotion_input =
                    npc_locomotion_input_for_facing(intent.move_dir, motion.facing);
                intent.want_attack = false;
                continue;
            }
        }
        if attacking || air.air.airborne {
            intent.move_dir = Vec2::ZERO;
            intent.want_attack = perform_attack;
            if enemy.is_some_and(|enemy| {
                (enemy.position - npc_pos).length() <= npc_attack_range(fighter.weapon)
            }) && intent.attack_cooldown <= 0.0
            {
                intent.attack_cooldown = npc_think_interval(mission_npc.as_deref());
                queue_npc_opening_action(
                    &mut intent,
                    performance,
                    combat_profile,
                    &mut patrol,
                    fighter.weapon,
                    air.air.airborne,
                );
                if let Some(enemy) = enemy {
                    aim_npc_attack(
                        &mut intent,
                        &mut patrol,
                        fighter,
                        combat_profile,
                        npc_pos,
                        enemy.position,
                    );
                }

                intent.want_guard = false;
                intent.dodge_burst = None;
                performance.guard_remaining = 0.0;
            }
            continue;
        }
        let configured_position = configured_target.map(|(_, position)| position);
        let player_is_hostile = mission_npc.as_deref().is_none_or(|npc| {
            (meteor_combat::hit_resolve::team_mask(i32::from(player_team))
                & meteor_combat::hit_resolve::team_mask(i32::from(npc.team)))
                == 0
        });
        let live_player = (player_alive && player_is_hostile).then_some(player_pos);
        let fallback_target = match forced {
            Some(BehaviorMode::Follow | BehaviorMode::Dodge) => configured_position,
            Some(BehaviorMode::Kill) => configured_position,
            Some(BehaviorMode::Attack | BehaviorMode::Defend) => live_player,
            _ => None,
        };
        let target_position = enemy.map(|enemy| enemy.position).or(fallback_target);
        let delta = target_position.unwrap_or(npc_pos) - npc_pos;
        let flat = Vec3::new(delta.x, 0.0, delta.z);
        let distance = flat.length();
        let attack_range = npc_attack_range(fighter.weapon);
        let minimum_range = npc_minimum_range(fighter.weapon);

        let mode = if enemy.is_some() {
            if distance > attack_range {
                BehaviorMode::Follow
            } else if distance < minimum_range {
                BehaviorMode::Defend
            } else {
                BehaviorMode::Attack
            }
        } else {
            match forced {
                Some(BehaviorMode::Follow) if configured_target.is_none() => BehaviorMode::Run,
                Some(BehaviorMode::Follow) if distance <= 100.0 * meteor_core::MODEL_SCALE => {
                    BehaviorMode::Idle
                }
                Some(BehaviorMode::Kill) if target_position.is_none() => BehaviorMode::Wait,
                Some(BehaviorMode::Kill) if distance > attack_range => BehaviorMode::Follow,
                Some(BehaviorMode::Kill) if distance < minimum_range => BehaviorMode::Defend,
                Some(BehaviorMode::Kill) => BehaviorMode::Attack,
                Some(BehaviorMode::Dodge) if configured_target.is_none() => BehaviorMode::Wait,
                Some(BehaviorMode::Dodge) if distance < 250.0 * meteor_core::MODEL_SCALE => {
                    BehaviorMode::Dodge
                }
                Some(BehaviorMode::Dodge) => BehaviorMode::Idle,
                Some(mode) => mode,
                None if target_position.is_some() && distance > attack_range => {
                    BehaviorMode::Follow
                }
                None if target_position.is_some() && distance < minimum_range => {
                    BehaviorMode::Defend
                }
                None if target_position.is_some() => BehaviorMode::Attack,
                None if !patrol.route.is_empty() => BehaviorMode::Patrol,
                None => BehaviorMode::Idle,
            }
        };
        let mut desired_facing = None;
        match mode {
            BehaviorMode::Attack | BehaviorMode::AttackTarget => {
                let attack_target = mode == BehaviorMode::AttackTarget;

                let target = if attack_target {
                    mission_npc
                        .as_deref()
                        .and_then(|npc| npc.attack_target_slot)
                        .and_then(|slot| {
                            script
                                .as_deref()
                                .and_then(|s| s.script())
                                .and_then(|runtime| runtime.host().target(slot))
                        })
                        .and_then(|slot_target| {
                            resolve_target_point(
                                Some(slot_target),
                                waypoints.as_deref(),
                                player_pos,
                                &npc_positions,
                            )
                        })
                } else {
                    target_position
                };
                if let Some(target) = target {
                    let delta = target - npc_pos;
                    let flat = Vec3::new(delta.x, 0.0, delta.z);
                    let dist = flat.length();
                    let remaining = mission_npc
                        .as_deref()
                        .map(|npc| npc.attack_target_remaining)
                        .unwrap_or(0);
                    let arrive = if attack_target {
                        if remaining > 0 {
                            80.0 * meteor_core::MODEL_SCALE
                        } else {
                            40.0 * meteor_core::MODEL_SCALE
                        }
                    } else {
                        attack_range
                    };
                    if dist > arrive {
                        let step = patrol.travel_step(
                            waypoints.as_deref(),
                            collider.as_deref().and_then(|c| c.mesh.as_ref()),
                            dt,
                            npc_pos,
                            target,
                        );
                        intent.move_dir = Vec2::new(step.direction.x, step.direction.z);
                        intent.navigation_jump_target = step.jump_target;
                        intent.want_attack = false;
                        motion.movement_mode = if step.direction.length_squared() > 0.01 {
                            meteor_core::MovementMode::Run
                        } else {
                            meteor_core::MovementMode::Idle
                        };
                    } else {
                        intent.move_dir = Vec2::ZERO;
                        if let Some(yaw) = yaw_towards(flat) {
                            motion.facing = npc_combat_turn(motion.facing, yaw, dt);
                        }
                        let in_angle = yaw_towards(flat).is_none_or(|yaw| {
                            npc_facing_error(motion.facing, yaw).abs()
                                <= tactics.angle_degrees.to_radians()
                        });
                        if (!attack_target || remaining > 0)
                            && intent.attack_cooldown <= 0.0
                            && in_angle
                        {
                            intent.attack_cooldown = npc_think_interval(mission_npc.as_deref());
                            if attack_target {
                                intent.want_attack = true;
                                let category = (next_npc_rand(&mut patrol.rng) * 3.0) as usize;
                                let roll =
                                    (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
                                let command =
                                    patrol.command_memory.choose(tactics, category, false, roll);
                                intent.attack_directions = meteor_ai::combat::directions(command);
                            } else {
                                queue_npc_opening_action(
                                    &mut intent,
                                    performance,
                                    combat_profile,
                                    &mut patrol,
                                    fighter.weapon,
                                    false,
                                );
                            }
                            aim_npc_attack(
                                &mut intent,
                                &mut patrol,
                                fighter,
                                combat_profile,
                                npc_pos,
                                target,
                            );
                            if attack_target {
                                if let Some(npc) = mission_npc.as_deref_mut() {
                                    npc.attack_target_remaining =
                                        npc.attack_target_remaining.saturating_sub(1);
                                }
                            }
                        }
                        motion.movement_mode = meteor_core::MovementMode::Idle;
                    }
                } else {
                    intent.move_dir = Vec2::ZERO;
                    intent.want_attack = false;
                    motion.movement_mode = meteor_core::MovementMode::Idle;
                }
            }
            BehaviorMode::Defend => {
                let dir = if distance > 0.01 {
                    -flat / distance
                } else {
                    Vec3::ZERO
                };
                intent.move_dir = Vec2::new(dir.x, dir.z);
                intent.want_attack = false;
                motion.movement_mode = meteor_core::MovementMode::Walk;
                desired_facing = yaw_towards(flat);
            }
            BehaviorMode::Follow => {
                let target_position = target_position.unwrap_or(npc_pos);
                let step = patrol.travel_step(
                    waypoints.as_deref(),
                    collider.as_deref().and_then(|c| c.mesh.as_ref()),
                    dt,
                    npc_pos,
                    target_position,
                );
                intent.move_dir = Vec2::new(step.direction.x, step.direction.z);
                intent.navigation_jump_target = step.jump_target;
                intent.want_attack = false;
                motion.movement_mode = if step.direction.length_squared() > 0.01 {
                    meteor_core::MovementMode::Run
                } else {
                    meteor_core::MovementMode::Idle
                };
            }
            BehaviorMode::Patrol => {
                if let Some(graph) = waypoints.as_deref() {
                    for _ in 0..patrol.route.len() {
                        if patrol
                            .route
                            .get(patrol.index)
                            .is_some_and(|id| graph.is_enabled(*id))
                        {
                            break;
                        }
                        patrol.index = (patrol.index + 1) % patrol.route.len();
                    }
                    let target_id = patrol
                        .route
                        .get(patrol.index)
                        .copied()
                        .filter(|id| graph.is_enabled(*id));
                    if let Some(target) = target_id.and_then(|id| graph.world_position(id)) {
                        let step = patrol.patrol_step(
                            Some(graph),
                            collider.as_deref().and_then(|c| c.mesh.as_ref()),
                            dt,
                            npc_pos,
                            Vec3::from_array(target),
                        );
                        if step.arrived && !patrol.route.is_empty() {
                            patrol.index = (patrol.index + 1) % patrol.route.len();
                        }
                        intent.move_dir = Vec2::new(step.direction.x, step.direction.z);
                        intent.navigation_jump_target = step.jump_target;
                        intent.want_attack = false;
                        motion.movement_mode = if step.direction.length_squared() > 0.01 {
                            meteor_core::MovementMode::Run
                        } else {
                            meteor_core::MovementMode::Idle
                        };
                    } else {
                        intent.move_dir = Vec2::ZERO;
                        intent.want_attack = false;
                        motion.movement_mode = meteor_core::MovementMode::Idle;
                    }
                } else {
                    intent.move_dir = Vec2::ZERO;
                    motion.movement_mode = meteor_core::MovementMode::Idle;
                }
            }
            BehaviorMode::Idle => {
                intent.move_dir = Vec2::ZERO;
                intent.want_attack = false;
                motion.movement_mode = meteor_core::MovementMode::Idle;
                let face_chance = match forced {
                    Some(BehaviorMode::Follow) => Some(1.0 / 6.0),
                    Some(BehaviorMode::Dodge) => Some(1.0 / 8.0),
                    _ => None,
                };
                if thought_tick {
                    if let (Some(target), Some(chance)) = (configured_position, face_chance) {
                        if next_npc_rand(&mut patrol.rng) < chance {
                            motion.facing = yaw_towards(target - npc_pos).unwrap_or(motion.facing);
                        }
                    }
                }
            }
            BehaviorMode::Wait => {
                intent.move_dir = Vec2::ZERO;
                intent.want_attack = false;
                motion.movement_mode = meteor_core::MovementMode::Idle;
                if thought_tick && next_npc_rand(&mut patrol.rng) < 0.1 {
                    motion.facing = next_npc_rand(&mut patrol.rng) * std::f32::consts::TAU;
                }
            }
            BehaviorMode::Run => {
                if thought_tick
                    && patrol.wander_goal.is_none()
                    && next_npc_rand(&mut patrol.rng) < 1.0 / 7.0
                {
                    if let Some(graph) = waypoints.as_deref() {
                        let enabled = graph
                            .points
                            .iter()
                            .filter(|p| graph.is_enabled(p.id))
                            .map(|p| p.id)
                            .collect::<Vec<_>>();
                        if !enabled.is_empty() {
                            let index =
                                (next_npc_rand(&mut patrol.rng) * enabled.len() as f32) as usize;
                            patrol.wander_goal = Some(enabled[index.min(enabled.len() - 1)]);
                        }
                    }
                }
                let wander_target = patrol
                    .wander_goal
                    .and_then(|goal| waypoints.as_deref()?.world_position(goal))
                    .map(Vec3::from_array);
                if let Some(target) = wander_target {
                    let to_goal = target - npc_pos;
                    let goal_flat = Vec3::new(to_goal.x, 0.0, to_goal.z);
                    if goal_flat.length() <= PATROL_ARRIVE_DIST {
                        patrol.wander_goal = None;
                        intent.move_dir = Vec2::ZERO;
                        motion.movement_mode = meteor_core::MovementMode::Idle;
                    } else {
                        let step = patrol.travel_step(
                            waypoints.as_deref(),
                            collider.as_deref().and_then(|c| c.mesh.as_ref()),
                            dt,
                            npc_pos,
                            target,
                        );
                        intent.move_dir = Vec2::new(step.direction.x, step.direction.z);
                        intent.navigation_jump_target = step.jump_target;
                        intent.want_attack = false;
                        motion.movement_mode = if step.direction.length_squared() > 0.01 {
                            meteor_core::MovementMode::Run
                        } else {
                            meteor_core::MovementMode::Idle
                        };
                    }
                } else {
                    intent.move_dir = Vec2::ZERO;
                    intent.want_attack = false;
                    motion.movement_mode = meteor_core::MovementMode::Idle;

                    if thought_tick && next_npc_rand(&mut patrol.rng) < 0.1 {
                        motion.facing = next_npc_rand(&mut patrol.rng) * std::f32::consts::TAU;
                    }
                }
            }
            BehaviorMode::Dodge => {
                let threat = configured_position.unwrap_or(npc_pos);
                if patrol.flee_goal.is_none() {
                    patrol.flee_goal = waypoints
                        .as_deref()
                        .and_then(|graph| farthest_reachable_waypoint(graph, npc_pos, threat));
                }
                let flee_target = patrol
                    .flee_goal
                    .and_then(|goal| waypoints.as_deref()?.world_position(goal))
                    .map(Vec3::from_array);
                let destination = flee_target.unwrap_or_else(|| npc_pos - (threat - npc_pos));
                let step = patrol.travel_step(
                    waypoints.as_deref(),
                    collider.as_deref().and_then(|c| c.mesh.as_ref()),
                    dt,
                    npc_pos,
                    destination,
                );
                if flee_target.is_some_and(|target| {
                    (Vec3::new(target.x, 0.0, target.z) - Vec3::new(npc_pos.x, 0.0, npc_pos.z))
                        .length()
                        <= PATROL_ARRIVE_DIST
                }) {
                    patrol.flee_goal = None;
                }
                intent.move_dir = Vec2::new(step.direction.x, step.direction.z);
                intent.navigation_jump_target = step.jump_target;
                intent.want_attack = false;
                motion.movement_mode = if step.direction.length_squared() > 0.01 {
                    meteor_core::MovementMode::Run
                } else {
                    meteor_core::MovementMode::Idle
                };
            }
            BehaviorMode::Kill => {
                unreachable!("kill is normalized into idle/follow/attack")
            }
        }
        intent.want_attack |= perform_attack;
        if !npc_ordinary_locomotion_allowed(sm_state.as_deref()) {
            intent.navigation_jump_target = None;

            intent.move_dir = Vec2::ZERO;
            intent.locomotion_input = Vec2::ZERO;
            continue;
        }
        let dir = Vec3::new(intent.move_dir.x, 0.0, intent.move_dir.y);
        if desired_facing.is_none() && intent.move_dir.length_squared() > 0.01 {
            desired_facing = yaw_towards(dir);
        }
        if let Some(target_yaw) = desired_facing {
            motion.facing =
                meteor_core::math::lerp_angle(motion.facing, target_yaw, NPC_FACE_SMOOTH * dt);
        }
        if performance.crouching {
            motion.movement_mode = meteor_core::MovementMode::Crouch;
        }
        intent.locomotion_input = npc_locomotion_input_for_facing(intent.move_dir, motion.facing);

        if motion.movement_mode == meteor_core::MovementMode::Run
            && intent.locomotion_input != Vec2::Y
        {
            motion.movement_mode = meteor_core::MovementMode::Walk;
        }
    }
}

type NpcMoveQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static mut AiIntent,
        &'static mut Transform,
        &'static mut MotionState,
        &'static mut FighterAir,
        &'static mut FighterDodge,
        &'static mut PoseRuntime,
        &'static mut EntryBlend,
        &'static mut SmState,
        &'static mut CombatState,
        (
            &'static RangedFireGate,
            &'static crate::components::fighter::FighterStatusEffects,
            Option<&'static mut NpcPerformance>,
        ),
    ),
    (
        With<Fighter>,
        With<NpcFighter>,
        Without<LocalFighter>,
        Without<RemoteFighter>,
    ),
>;

pub(crate) fn move_npc_fighters(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    collider: Option<Res<StageCollider>>,
    assets: Option<Res<CharacterAssetsSet>>,
    common: Option<Res<crate::plugins::character_plugin::CommonAmb>>,
    mut contacts: Option<ResMut<CharacterContactFrame>>,
    mut npcs: NpcMoveQuery<'_, '_>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(13);
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let mut mover: CollisionSystem<&StageMeshCollider> =
        CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    if let Some(mesh) = collider
        .as_deref()
        .and_then(|collider| collider.mesh.as_ref())
    {
        mover.set_mesh_collider(mesh);
    }

    for (
        entity,
        fighter,
        mut intent,
        mut transform,
        mut motion,
        mut air,
        mut dodge,
        mut pose,
        mut blend,
        mut sm,
        mut combat,
        (ranged_gate, status_effects, performance),
    ) in &mut npcs
    {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        if let Some(burst) = intent.dodge_burst.take() {
            if !wheel_vetoes_dodge_action(ranged_gate.wheel_out, burst) {
                let dodge_pose = meteor_physics::locomotion::dodge_pose_for_input(
                    intent.dodge_dir.x,
                    intent.dodge_dir.y,
                    burst,
                    burst,
                    Some(meteor_combat::weapon_strategy::weapon_to_strategy(
                        fighter.weapon,
                    )),
                );
                dodge
                    .state
                    .start(dodge_pose, motion.facing, burst, 0.0, 0.0);
                dodge.pose = dodge_pose;
                dodge.pose_entered = false;
                motion.movement_mode = meteor_core::MovementMode::Dodge;
                enter_dodge_state(&mut sm, &mut combat, burst);
                let _ = apply_dodge_pose_now(
                    assets.as_deref(),
                    fighter,
                    &mut pose,
                    &mut blend,
                    dodge.pose,
                );
            }
        }
        let navigation_jump = intent.navigation_jump_target.is_some_and(|target| {
            yaw_towards(target - transform.translation).is_some_and(|yaw| {
                npc_facing_error(motion.facing, yaw).abs() <= 10.0_f32.to_radians()
                    && yaw_towards(motion.velocity).is_some_and(|moving| {
                        npc_facing_error(moving, yaw).abs() <= 40.0_f32.to_radians()
                    })
            })
        });
        let scripted_jump = performance.as_deref().is_some_and(|p| p.jump_requested);
        if (navigation_jump || scripted_jump)
            && !air.air.airborne
            && !dodge.state.active
            && npc_ordinary_locomotion_allowed(Some(&sm))
        {
            if let (Some(assets), Some(common)) = (assets.as_deref(), common.as_deref()) {
                if crate::plugins::movement_plugin::start_scripted_jump(
                    fighter,
                    assets,
                    common,
                    &mut air,
                    &mut motion,
                    &mut pose,
                    &mut blend,
                    &mut sm,
                    &mut combat,
                    navigation_jump,
                ) {
                    if let Some(mut performance) = performance {
                        performance.jump_requested = false;
                    }
                }
            }
        }
        if dodge.state.active {
            continue;
        }
        if air.air.airborne {
            motion.movement_mode = meteor_core::MovementMode::Airborne;
            continue;
        }
        let action_owned =
            !npc_ordinary_locomotion_allowed(Some(&sm)) && sm.current != FighterState::Guard;
        if action_owned && !air.air.airborne {
            continue;
        }
        let speed = if air.air.airborne {
            0.0
        } else {
            match motion.movement_mode {
                meteor_core::MovementMode::Run => RUN_SPEED,
                meteor_core::MovementMode::Walk => WALK_SPEED,
                meteor_core::MovementMode::Crouch => WALK_SPEED,
                _ => 0.0,
            }
        };
        let dir = Vec3::new(intent.move_dir.x, 0.0, intent.move_dir.y);
        advance_npc_motion(
            entity,
            contacts.as_deref_mut(),
            &mover,
            &mut transform,
            &mut motion,
            &mut air,
            dir * speed * status_effects.effects.speed_factor(1.0),
            dt,
            true,
        );
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;
    use meteor_scene::stage_loader::parse_waypoint_graph;

    #[derive(Resource, Default)]
    struct NpcHitJudgeSnapshot(Option<(Vec3, bool)>);

    fn capture_npc_position_at_hit_judge(
        npcs: Query<(&Transform, &FighterAir), With<NpcFighter>>,
        mut snapshot: ResMut<NpcHitJudgeSnapshot>,
    ) {
        snapshot.0 = npcs
            .iter()
            .next()
            .map(|(transform, air)| (transform.translation, air.air.airborne));
    }

    struct NpcTestMesh {
        wall_x: Option<f32>,
        ledge_x: Option<f32>,
    }

    struct NpcSteepMesh;

    struct NpcVisionMesh(Option<f32>);

    impl MeshCollider for NpcVisionMesh {
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
            self.0.map(|t| meteor_physics::collision::RayHit {
                t,
                y: 0.0,
                nx: 0.0,
                ny: 0.0,
                nz: 1.0,
            })
        }
    }

    impl MeshCollider for NpcSteepMesh {
        fn sweep_foot_segment(
            &self,
            _x: f32,
            y_top: f32,
            _z: f32,
            y_bottom: f32,
        ) -> Option<FootHit> {
            (y_top >= 0.0 && y_bottom <= 0.0).then_some(FootHit {
                y: 0.0,
                nx: 0.8,
                ny: 0.3,
                nz: 0.0,
            })
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
            None
        }

        fn sample_ground_hit(
            &self,
            _x: f32,
            _z: f32,
            _fallback: f32,
            _hint_y: f32,
        ) -> Option<meteor_physics::collision::GroundHit> {
            Some(meteor_physics::collision::GroundHit {
                y: 0.0,
                nx: 0.8,
                ny: 0.3,
                nz: 0.0,
            })
        }
    }

    impl MeshCollider for NpcTestMesh {
        fn sweep_foot_segment(
            &self,
            x: f32,
            y_top: f32,
            _z: f32,
            y_bottom: f32,
        ) -> Option<FootHit> {
            let supported = self.ledge_x.is_none_or(|ledge| x <= ledge);
            (supported && y_top >= 0.0 && y_bottom <= 0.0).then_some(FootHit {
                y: 0.0,
                nx: 0.0,
                ny: 1.0,
                nz: 0.0,
            })
        }

        fn box_blocked(&self, cx: f32, _cy: f32, _cz: f32) -> bool {
            self.wall_x.is_some_and(|wall| cx >= wall)
        }

        fn raycast_hit(
            &self,
            _origin: Point3,
            _direction: Point3,
            _max_dist: f32,
        ) -> Option<meteor_physics::collision::RayHit> {
            None
        }

        fn sample_ground_hit(
            &self,
            x: f32,
            _z: f32,
            _fallback: f32,
            _hint_y: f32,
        ) -> Option<meteor_physics::collision::GroundHit> {
            Some(meteor_physics::collision::GroundHit {
                y: if self.ledge_x.is_some_and(|ledge| x > ledge) {
                    -10.0
                } else {
                    0.0
                },
                nx: 0.0,
                ny: 1.0,
                nz: 0.0,
            })
        }
    }

    fn line_graph() -> WaypointGraph {
        parse_waypoint_graph(
            "WayPoints 3\nPos 0 0 0\nSize 1\nLink 1\n1 0 5.0\nPos 5 0 0\nSize 1\nLink 2\n0 0 5.0\n2 0 5.0\nPos 10 0 0\nSize 1\nLink 1\n1 0 5.0\n",
            [0.0, 0.0, 0.0],
        )
    }

    #[test]
    fn sn09_lengyan_uses_authored_jump_to_reach_central_platform() {
        for start in [5, 7, 8, 10] {
            for fps in [30, 60, 120] {
                sn09_jump_case(start, fps);
            }
        }
    }

    fn sn09_jump_case(start_id: usize, fps: u32) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let center = meteor_scene::stage_loader::get_stage_center("sn09", None);
        let graph = parse_waypoint_graph(
            &std::fs::read_to_string(root.join("Level/sn09.wp")).unwrap(),
            center,
        );
        assert_eq!(graph.link_types.get(&(7, 0)), Some(&1));
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn09/sn09.gmb")).unwrap())
                .unwrap();
        let mut des = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn09/sn09.des")).unwrap(),
        );
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn09/sn09.cob")).unwrap())
            .unwrap();
        let mut mesh = StageMeshCollider::from_gmdl(&gmdl, &des, Some(&cob.face_ids), center);

        use meteor_scene::scene_script_runtime::{SceneItemInstance, SceneScriptRuntime};
        let sources = ["PScriptLib/Def.h", "sn09/sn09.pst"]
            .map(|path| crate::text_codec::decode_text(&std::fs::read(root.join(path)).unwrap()))
            .join("\n");
        let instances = des
            .dummies
            .iter()
            .enumerate()
            .map(|(index, dummy)| SceneItemInstance {
                name: dummy.name.clone(),
                index,
                props: default(),
            })
            .collect();
        let mut scene = SceneScriptRuntime::load(&sources, instances, 250_000).unwrap();
        scene.call("Scene_OnLoad", &[]).unwrap();
        for (index, dummy) in des.dummies.iter_mut().enumerate() {
            if scene.host().instance(index).unwrap().collision_enabled() {
                dummy.collision_attr = Some(1);
            }
        }
        super::super::render_plugin::append_solid_props_for_test(&root, &des, center, &mut mesh);
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(&mesh);
        let grounded = |id| {
            let p = Vec3::from_array(graph.world_position(id).unwrap());
            let hit = mover.sample_ground_hit(p.x, p.z, p.y).unwrap();
            Vec3::new(p.x, hit.y, p.z)
        };
        let start = grounded(start_id);
        let target = grounded(4);
        let character = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root),
            "p1",
        )
        .unwrap();
        let common = meteor_asset::amb::parse_amb(
            &std::fs::read(root.join("Common/character.amb")).unwrap(),
        )
        .unwrap();
        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(graph)
            .insert_resource(StageCollider { mesh: Some(mesh) })
            .insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
                "p1".into(),
                character,
            )])))
            .insert_resource(crate::plugins::character_plugin::CommonAmb(common))
            .add_systems(
                Update,
                (
                    drive_npc_ai,
                    move_npc_fighters,
                    npc_ballistic_air_root_motion,
                )
                    .chain(),
            );
        app.world_mut().spawn((
            LocalFighter,
            Health::default(),
            Transform::from_translation(target),
        ));
        let npc = app
            .world_mut()
            .spawn((
                NpcFighter,
                Fighter {
                    character_id: 1,
                    player_slot: 1,
                    weapon: crate::plugins::character_plugin::weapon_slot_from_item(33)
                        .unwrap()
                        .0,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: yaw_towards(target - start).unwrap(),
                },
                AiIntent::default(),
                Transform::from_translation(start),
                FighterAir::default(),
                FighterDodge::default(),
                NpcPatrol::default(),
                SmState::default(),
                CombatState::default(),
                PoseRuntime::idle(),
                EntryBlend::default(),
                NpcPerformance::default(),
                MissionNpc {
                    id: 1,
                    name: "冷燕".into(),
                    team: 2,
                    in_scene_roster: true,
                    view: 500.0,
                    think: 100.0,
                    forced_behavior: Some(BehaviorMode::Follow),
                    is_vip: false,
                    attack_target_slot: None,
                    behavior_args: vec![meteor_scene::pst_runtime::PstValue::Num(0.0)],
                    attack_target_remaining: 0,
                },
            ))
            .id();
        let mut jumped = false;
        for _ in 0..10 * fps {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / fps as f32));
            app.update();
            let air = app.world().get::<FighterAir>(npc).unwrap();
            jumped |= air.air.airborne && air.air.vertical_speed > 0.0;
            let p = app.world().get::<Transform>(npc).unwrap().translation;
            if jumped && !air.air.airborne && (p.y - target.y).abs() < 0.1 {
                return;
            }
        }
        panic!(
            "start_id={start_id}, fps={fps}, jumped={jumped}, start={start:?}, target={target:?}, final={:?}, nav={:?}",
            app.world().get::<Transform>(npc).unwrap().translation,
            app.world().get::<NpcPatrol>(npc).unwrap()
        );
    }

    #[test]
    fn pst_npc_perform_jump_reaches_shared_air_state_and_consumes_once() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root),
            "p1",
        )
        .unwrap();
        let common = meteor_asset::amb::parse_amb(
            &std::fs::read(root.join("Common/character.amb")).unwrap(),
        )
        .unwrap();
        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
                "p1".into(),
                character,
            )])))
            .insert_resource(crate::plugins::character_plugin::CommonAmb(common))
            .add_systems(Update, (drive_npc_ai, move_npc_fighters).chain());
        let fighter = Fighter {
            character_id: 1,
            player_slot: 1,
            weapon: 6,
            weapon_slots: [6, 5],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        };
        app.world_mut().spawn((
            LocalFighter,
            fighter.clone(),
            Health::default(),
            Transform::from_xyz(50.0, 0.0, 0.0),
        ));
        let mut performance = NpcPerformance::default();
        performance
            .queue
            .push_back(crate::plugins::stage_plugin::NpcPerformCommand {
                action: "jump".into(),
                args: vec![],
            });
        let npc = app
            .world_mut()
            .spawn((
                NpcFighter,
                fighter,
                Health::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                AiIntent::default(),
                Transform::default(),
                FighterAir::default(),
                FighterDodge::default(),
                NpcPatrol::default(),
                SmState::default(),
                CombatState::default(),
                PoseRuntime::idle(),
                EntryBlend::default(),
                performance,
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        for _ in 0..21 {
            app.update();
        }
        assert!(app.world().get::<FighterAir>(npc).unwrap().air.airborne);
        assert_eq!(
            app.world().get::<SmState>(npc).unwrap().current,
            FighterState::Jump
        );
        assert!(
            !app.world()
                .get::<NpcPerformance>(npc)
                .unwrap()
                .jump_requested
        );
        let instance = app
            .world()
            .get::<PoseRuntime>(npc)
            .unwrap()
            .pose_instance_id;
        app.update();
        assert_eq!(
            app.world()
                .get::<PoseRuntime>(npc)
                .unwrap()
                .pose_instance_id,
            instance
        );
    }

    #[test]
    fn pst_disabled_waypoint_invalidates_routes_and_can_be_reenabled() {
        let mut graph = line_graph();
        for point in &mut graph.points {
            for axis in &mut point.position {
                *axis *= 10.0;
            }
        }
        let start = Vec3::from_array(graph.world_position(0).unwrap());
        let target = Vec3::from_array(graph.world_position(2).unwrap());
        let mut patrol = NpcPatrol::default();
        assert!(
            patrol
                .travel_step(Some(&graph), None, 0.016, start, target)
                .direction
                .length_squared()
                > 0.0
        );
        assert!(graph.set_enabled(1, false));
        patrol.invalidate_navigation();
        let blocked = patrol.travel_step(Some(&graph), None, 0.016, start, target);
        assert_eq!(blocked.direction, Vec3::ZERO);
        assert!(!blocked.arrived);
        assert!(graph.set_enabled(1, true));
        patrol.invalidate_navigation();
        assert!(
            patrol
                .travel_step(Some(&graph), None, 0.016, start, target)
                .direction
                .length_squared()
                > 0.0
        );
    }

    #[test]
    fn npc_ground_motion_uses_body_box_collision_and_publishes_accepted_velocity() {
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(NpcTestMesh {
            wall_x: Some(1.0),
            ledge_x: None,
        });
        let mut transform = Transform::from_xyz(0.0, 0.0, 0.0);
        let mut motion = MotionState {
            movement_mode: meteor_core::MovementMode::Idle,
            velocity: Vec3::ZERO,
            facing: 0.0,
        };
        let mut air = FighterAir::default();
        advance_npc_motion(
            Entity::PLACEHOLDER,
            None,
            &mover,
            &mut transform,
            &mut motion,
            &mut air,
            Vec3::new(12.0, 0.0, 0.0),
            0.1,
            false,
        );
        assert!(
            transform.translation.x < 1.0,
            "NPC cannot cross body-box wall"
        );
        assert!(
            motion.velocity.x < 12.0,
            "velocity reports accepted displacement"
        );
        assert_eq!(transform.translation.y, 0.0);
        assert!(!air.air.airborne);
    }

    #[test]
    fn npc_ground_motion_consumes_steep_projected_gravity_without_input() {
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(NpcSteepMesh);
        let mut transform = Transform::default();
        let mut motion = MotionState {
            movement_mode: meteor_core::MovementMode::Idle,
            velocity: Vec3::ZERO,
            facing: 0.0,
        };
        let mut air = FighterAir::default();

        advance_npc_motion(
            Entity::PLACEHOLDER,
            None,
            &mover,
            &mut transform,
            &mut motion,
            &mut air,
            Vec3::ZERO,
            1.0 / 30.0,
            false,
        );

        assert!(air.carry_decay_steep);
        assert!(air.ground_external_velocity.x > 0.0);
        assert!(transform.translation.x > 0.0);
        assert!(motion.velocity.x > 0.0);
    }

    #[test]
    fn npc_action_states_retain_their_own_motion_owner() {
        let mut state = SmState::default();
        assert!(npc_ordinary_locomotion_allowed(Some(&state)));
        for action in [
            meteor_core::FighterState::Attack1,
            meteor_core::FighterState::Attack2,
            meteor_core::FighterState::Attack3,
            meteor_core::FighterState::Guard,
            meteor_core::FighterState::Dodge,
            meteor_core::FighterState::Burst,
            meteor_core::FighterState::BeHit,
            meteor_core::FighterState::Down,
            meteor_core::FighterState::Dead,
        ] {
            state.current = action;
            assert!(!npc_ordinary_locomotion_allowed(Some(&state)), "{action:?}");
        }
    }

    #[test]
    fn npc_walk_off_ledge_enters_shared_ballistic_fall() {
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(NpcTestMesh {
            wall_x: None,
            ledge_x: Some(0.5),
        });
        let mut transform = Transform::from_xyz(0.4, 0.0, 0.0);
        let mut motion = MotionState {
            movement_mode: meteor_core::MovementMode::Run,
            velocity: Vec3::ZERO,
            facing: 0.0,
        };
        let mut air = FighterAir::default();
        advance_npc_motion(
            Entity::PLACEHOLDER,
            None,
            &mover,
            &mut transform,
            &mut motion,
            &mut air,
            Vec3::new(6.0, 0.0, 0.0),
            0.1,
            false,
        );
        assert!(air.air.airborne, "unsupported destination starts fall");
        assert!(air.ledge_fall);
        assert_eq!(motion.movement_mode, meteor_core::MovementMode::Airborne);

        let before = transform.translation.y;
        advance_npc_motion(
            Entity::PLACEHOLDER,
            None,
            &mover,
            &mut transform,
            &mut motion,
            &mut air,
            Vec3::ZERO,
            0.1,
            false,
        );
        assert!(
            transform.translation.y < before,
            "gravity advances NPC downward"
        );
        assert!(motion.velocity.y < 0.0);
    }

    #[test]
    fn npc_ballistic_motion_clamps_and_publishes_the_accepted_three_axis_step() {
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(NpcTestMesh {
            wall_x: None,
            ledge_x: None,
        });
        let mut transform = Transform::from_xyz(0.0, 100.0, 0.0);
        let start = transform.translation;
        let mut motion = MotionState {
            movement_mode: meteor_core::MovementMode::Airborne,
            velocity: Vec3::ZERO,
            facing: 0.0,
        };
        let mut air = FighterAir::default();
        air.air.airborne = true;
        air.air.planar_x = 100.0;
        air.air.vertical_speed = 100.0;
        air.air.motion_mode = jump::AirMotionMode::Ballistic;
        let dt = 1.0 / 30.0;

        advance_npc_motion(
            Entity::PLACEHOLDER,
            None,
            &mover,
            &mut transform,
            &mut motion,
            &mut air,
            Vec3::ZERO,
            dt,
            false,
        );

        assert!(motion.velocity.length() <= meteor_physics::carry_velocity::CARRY_MAX_SPEED + 1e-4);
        let accepted = (transform.translation - start) / dt;
        assert!((motion.velocity - accepted).length() < 1e-4);
        assert!(
            motion.velocity.y > 0.0,
            "accepted vertical speed is published"
        );
    }

    #[test]
    fn production_npc_ballistic_air_waits_for_hit_judge_and_integrates_once() {
        let mut app = App::new();
        crate::plugins::configure_fighter_frame_sets(&mut app);
        app.init_resource::<Time>()
            .init_resource::<NpcHitJudgeSnapshot>()
            .insert_resource(StageCollider {
                mesh: Some(StageMeshCollider::default()),
            })
            .add_systems(
                Update,
                move_npc_fighters.in_set(crate::plugins::FighterFrameSet::Locomotion),
            )
            .add_systems(
                Update,
                capture_npc_position_at_hit_judge.in_set(crate::plugins::FighterFrameSet::HitJudge),
            )
            .add_systems(
                Update,
                npc_ballistic_air_root_motion.in_set(crate::plugins::FighterFrameSet::RootMotion),
            );
        let mut air = FighterAir::default();
        air.air.start_fall(0.0, 0.0);
        let start = Vec3::new(0.0, 10.0, 0.0);
        let npc = app
            .world_mut()
            .spawn((
                NpcFighter,
                Fighter {
                    character_id: 1,
                    player_slot: 1,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                AiIntent::default(),
                Transform::from_translation(start),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Airborne,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                air,
                FighterDodge::default(),
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

        assert_eq!(
            app.world().resource::<NpcHitJudgeSnapshot>().0,
            Some((start, true)),
            "NPC Locomotion must not move mode0 air"
        );
        let transform = app.world().get::<Transform>(npc).unwrap();
        assert!(transform.translation.y < start.y);
        let air = app.world().get::<FighterAir>(npc).unwrap();
        assert!(
            (air.air.air_time - dt).abs() < 1e-6,
            "exactly one NPC air tick"
        );
    }

    #[test]
    fn production_npc_ledge_fall_commits_after_hit_judge_without_gravity() {
        let mut app = App::new();
        crate::plugins::configure_fighter_frame_sets(&mut app);
        app.init_resource::<Time>()
            .init_resource::<NpcHitJudgeSnapshot>()
            .insert_resource(StageCollider {
                mesh: Some(StageMeshCollider::default()),
            })
            .add_systems(
                Update,
                move_npc_fighters.in_set(crate::plugins::FighterFrameSet::Locomotion),
            )
            .add_systems(
                Update,
                capture_npc_position_at_hit_judge.in_set(crate::plugins::FighterFrameSet::HitJudge),
            )
            .add_systems(
                Update,
                npc_ballistic_air_root_motion.in_set(crate::plugins::FighterFrameSet::RootMotion),
            );
        let start = Vec3::new(0.0, 10.0, 0.0);
        let npc = app
            .world_mut()
            .spawn((
                NpcFighter,
                Fighter {
                    character_id: 1,
                    player_slot: 1,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                AiIntent::default(),
                Transform::from_translation(start),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::new(3.0, 0.0, 4.0),
                    facing: 0.0,
                },
                FighterAir::default(),
                FighterDodge::default(),
                PoseRuntime::idle(),
                EntryBlend::default(),
                SmState::default(),
                CombatState::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.0 / 30.0));

        app.update();

        assert_eq!(
            app.world().resource::<NpcHitJudgeSnapshot>().0,
            Some((start, false))
        );
        assert_eq!(
            app.world().get::<Transform>(npc).unwrap().translation,
            start
        );
        let air = app.world().get::<FighterAir>(npc).unwrap();
        assert!(air.air.airborne);
        assert!(air.ledge_fall);
        assert!(!air.pending_ledge_fall);
        assert_eq!(air.air.air_time, 0.0);
    }

    #[test]
    fn origin_sn01_npc_ballistic_motion_lands_on_real_stage_floor() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("sn01/sn01.gmb").is_file() {
            return;
        }
        let gmdl = meteor_asset::gmdl::decode_gmdl(
            &std::fs::read(root.join("sn01/sn01.gmb")).expect("sn01.gmb"),
        )
        .expect("decode sn01.gmb");
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn01/sn01.des")).expect("sn01.des"),
        );
        let cob = meteor_asset::cob::parse_cob(
            &std::fs::read(root.join("sn01/sn01.cob")).expect("sn01.cob"),
        )
        .expect("decode sn01.cob");
        let mesh = crate::plugins::stage_collider::StageMeshCollider::from_gmdl(
            &gmdl,
            &desc,
            Some(&cob.face_ids),
            meteor_scene::stage_loader::get_stage_center("sn01", None),
        );
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(mesh);
        let ground = mover.sample_ground_hit(0.0, 0.0, 3.0).unwrap().y;
        let mut transform = Transform::from_xyz(0.0, ground + 3.0, 0.0);
        let mut motion = MotionState {
            movement_mode: meteor_core::MovementMode::Idle,
            velocity: Vec3::ZERO,
            facing: 0.0,
        };
        let mut air = FighterAir::default();
        for _ in 0..240 {
            advance_npc_motion(
                Entity::PLACEHOLDER,
                None,
                &mover,
                &mut transform,
                &mut motion,
                &mut air,
                Vec3::ZERO,
                1.0 / 60.0,
                false,
            );
        }
        assert!(!air.air.airborne, "NPC settles after real mesh descent");
        assert!(
            (transform.translation.y - ground).abs() < 1e-3,
            "NPC lands on sampled sn01 support: y={} ground={ground}",
            transform.translation.y
        );
    }

    #[test]
    fn resolve_target_point_waypoint_and_char() {
        let graph = line_graph();
        let player = Vec3::new(0.0, 0.9, 3.0);
        let mut npc_positions = std::collections::HashMap::new();
        npc_positions.insert(7, Vec3::new(-2.0, 0.9, 1.0));

        let wp = resolve_target_point(
            Some(("waypoint", 1.0)),
            Some(&graph),
            player,
            &npc_positions,
        );
        let wp = wp.unwrap();
        assert!((wp.x - 0.6).abs() < 1e-5);
        assert!(wp.z.abs() < 1e-5);

        let player_target =
            resolve_target_point(Some(("char", 0.0)), Some(&graph), player, &npc_positions);
        assert_eq!(player_target, Some(player));

        let npc_target =
            resolve_target_point(Some(("char", 7.0)), Some(&graph), player, &npc_positions);
        assert_eq!(npc_target, Some(Vec3::new(-2.0, 0.9, 1.0)));

        assert_eq!(
            resolve_target_point(None, Some(&graph), player, &npc_positions),
            None
        );
        assert_eq!(
            resolve_target_point(Some(("bogus", 1.0)), Some(&graph), player, &npc_positions),
            None
        );

        assert_eq!(
            resolve_target_point(
                Some(("waypoint", 99.0)),
                Some(&graph),
                player,
                &npc_positions
            ),
            None
        );
    }

    #[test]
    fn path_target_follows_waypoint_route() {
        let graph = line_graph();

        let target = path_target(&graph, [0.0, 0.0, 0.0], [1.2, 0.0, 0.0]).unwrap();
        assert!((target[0] - 0.6).abs() < 1e-5);
        assert!((target[2]).abs() < 1e-5);
    }

    #[test]
    fn patrol_route_builds_along_links() {
        let graph = line_graph();

        let route = build_patrol_route(&graph, [0.0, 0.0, 0.0]);
        assert_eq!(route, vec![0, 1, 0, 1]);

        assert!(build_patrol_route(&WaypointGraph::default(), [0.0, 0.0, 0.0]).is_empty());
    }

    #[test]
    fn patrol_next_target_advances_on_arrival() {
        let graph = line_graph();
        let route = vec![1, 2];

        let (target, index) = patrol_next_target(&graph, &route, 0, [2.0, 0.0, 0.0]).unwrap();
        assert!((target[0] - 0.6).abs() < 1e-5);
        assert_eq!(index, 0);

        let (target, index) = patrol_next_target(&graph, &route, 0, [0.6, 0.0, 0.0]).unwrap();
        assert!((target[0] - 0.6).abs() < 1e-5);
        assert_eq!(index, 1);

        let (target, index) = patrol_next_target(&graph, &route, 1, [1.2, 0.0, 0.0]).unwrap();
        assert!((target[0] - 1.2).abs() < 1e-5);
        assert_eq!(index, 0);
    }

    #[test]
    fn path_target_same_waypoint_returns_none() {
        let graph = line_graph();
        assert_eq!(path_target(&graph, [0.05, 0.0, 0.0], [0.1, 0.0, 0.0]), None);
    }

    #[test]
    fn path_target_empty_graph_returns_none() {
        let graph = WaypointGraph::default();
        assert_eq!(path_target(&graph, [0.0, 0.0, 0.0], [5.0, 0.0, 0.0]), None);
    }

    #[test]
    fn path_target_disconnected_returns_none() {
        let graph = parse_waypoint_graph(
            "WayPoints 2\nPos 0 0 0\nSize 1\nLink 0\nPos 10 0 0\nSize 1\nLink 0\n",
            [0.0, 0.0, 0.0],
        );
        assert_eq!(path_target(&graph, [0.0, 0.0, 0.0], [1.2, 0.0, 0.0]), None);
    }

    #[test]
    fn eight_script_behaviors_keep_distinct_web_distance_gates() {
        for mode in [
            BehaviorMode::Wait,
            BehaviorMode::Run,
            BehaviorMode::Follow,
            BehaviorMode::Patrol,
            BehaviorMode::Kill,
        ] {
            assert!(behavior_scans_enemies(Some(mode)), "{mode:?} must scan");
        }
        for mode in [
            BehaviorMode::Idle,
            BehaviorMode::AttackTarget,
            BehaviorMode::Dodge,
        ] {
            assert!(
                !behavior_scans_enemies(Some(mode)),
                "{mode:?} must stay target-pinned"
            );
        }

        let actors = [
            AiActorSnapshot {
                id: 0,
                team: 1,
                position: Vec3::new(0.0, 0.0, 2.0),
                alive: true,
            },
            AiActorSnapshot {
                id: 2,
                team: 2,
                position: Vec3::new(0.0, 0.0, 4.0),
                alive: true,
            },
        ];

        assert_eq!(
            closest_hostile(1, 1, Vec3::ZERO, 10.0, &actors, |_| true).map(|actor| actor.position),
            Some(Vec3::new(0.0, 0.0, 4.0))
        );

        assert_eq!(
            closest_hostile(2, 2, Vec3::ZERO, 10.0, &actors, |_| true).map(|actor| actor.position),
            Some(Vec3::new(0.0, 0.0, 2.0))
        );

        let unaffiliated = [
            AiActorSnapshot {
                id: 1,
                team: 0,
                position: Vec3::ZERO,
                alive: true,
            },
            AiActorSnapshot {
                id: 2,
                team: 0,
                position: Vec3::new(1.0, 0.0, 0.0),
                alive: true,
            },
        ];
        assert_eq!(
            closest_hostile(1, 0, Vec3::ZERO, 10.0, &unaffiliated, |_| true).map(|actor| actor.id),
            Some(2)
        );
    }

    #[test]
    fn npc_weapon_spacing_uses_original_tables() {
        let debug_profile = NpcCombatProfile::web_debug();
        assert_eq!(
            (
                debug_profile.attack1,
                debug_profile.attack2,
                debug_profile.attack3,
                debug_profile.guard,
                debug_profile.dodge,
                debug_profile.burst,
            ),
            (40.0, 30.0, 20.0, 20.0, 15.0, 10.0),
            "room/debug NPCs keep the Web-authored default personality"
        );
        assert!(
            (npc_attack_range(6) - 7.2).abs() < 1e-6,
            "sword uses melee range"
        );
        assert!(
            (npc_attack_range(3) - 240.0).abs() < 1e-5,
            "gun keeps long range"
        );
        assert!(
            (npc_attack_range(1) - 120.0).abs() < 1e-5,
            "dart keeps ranged spacing"
        );
        assert!(
            (npc_attack_range(2) - 36.0).abs() < 1e-6,
            "wheel keeps ranged spacing"
        );
        assert_eq!(npc_minimum_range(6), 0.0);
        assert_eq!(npc_minimum_range(3), 12.0);

        for target in [Vec3::Z, Vec3::X, Vec3::NEG_X, Vec3::NEG_Z] {
            let yaw = yaw_towards(target).unwrap();
            let forward = Vec3::new(yaw.sin(), 0.0, yaw.cos());
            assert!(
                forward.dot(target) > 0.999,
                "yaw {yaw} must face target {target:?}, got {forward:?}"
            );
        }
    }

    #[test]
    fn npc_perception_uses_height_and_stage_occlusion_gates() {
        let origin = Vec3::ZERO;
        let target = Vec3::new(0.0, 0.0, 5.0);
        assert!(npc_target_visible(
            origin,
            target,
            10.0,
            Some(&NpcVisionMesh(None))
        ));
        assert!(!npc_target_visible(
            origin,
            Vec3::new(0.0, 12.1, 1.0),
            20.0,
            Some(&NpcVisionMesh(None))
        ));
        assert!(!npc_target_visible(
            origin,
            target,
            10.0,
            Some(&NpcVisionMesh(Some(1.0)))
        ));
        assert!(
            npc_target_visible(origin, target, 10.0, Some(&NpcVisionMesh(Some(4.8)))),
            "a hit inside Web's target-end tolerance does not occlude"
        );
    }

    #[test]
    fn authored_patrol_target_uses_linked_route_instead_of_wall_cutting() {
        let graph = parse_waypoint_graph(
            "WayPoints 3\nPos 0 0 0\nSize 1\nLink 1\n1 0 20\nPos 0 -20 0\nSize 1\nLink 2\n0 0 20\n2 0 20\nPos 20 -20 0\nSize 1\nLink 1\n1 0 20\n",
            [0.0, 0.0, 0.0],
        );
        let target = Vec3::from_array(graph.world_position(2).unwrap());
        let mut patrol = NpcPatrol::default();
        let step = patrol.travel_step(Some(&graph), None, 0.016, Vec3::ZERO, target);
        let dir = step.direction;
        assert!(dir.z > 0.999, "first leg follows waypoint 1: {dir:?}");
        assert!(
            dir.x.abs() < 1e-5,
            "patrol must not cut diagonally to waypoint 2"
        );

        let step = patrol.travel_step(Some(&graph), None, 0.016, Vec3::new(0.01, 0.0, 0.0), target);
        assert!(step.direction.z > 0.999, "cached leg flipped: {step:?}");
    }

    #[test]
    fn authored_attack_weights_choose_command_categories() {
        let weights = [15.0, 25.0, 30.0];
        assert_eq!(meteor_ai::combat::attack_category(weights, 14), 0);
        assert_eq!(meteor_ai::combat::attack_category(weights, 15), 1);
        assert_eq!(meteor_ai::combat::attack_category(weights, 39), 1);
        assert_eq!(meteor_ai::combat::attack_category(weights, 40), 2);
    }

    #[test]
    fn authored_opening_uses_fixed_percentiles_and_keeps_wait_probability() {
        let profile = NpcCombatProfile {
            attack1: 20.0,
            guard: 25.0,
            dodge: 60.0,
            burst: 20.0,
            ..default()
        };
        assert_eq!(
            choose_npc_opening_action(&profile, 0.10),
            NpcOpeningAction::Guard
        );
        assert_eq!(
            choose_npc_opening_action(&profile, 0.25),
            NpcOpeningAction::Attack
        );
        assert_eq!(
            choose_npc_opening_action(&profile, 0.60),
            NpcOpeningAction::Wait
        );
        assert_eq!(
            choose_npc_opening_action(&profile, 0.92),
            NpcOpeningAction::Wait
        );
    }

    #[test]
    fn npc_down_recovery_waits_for_authored_hold_plus_decision_delay() {
        let mut patrol = NpcPatrol::default();

        assert!(!update_npc_down_recovery(
            &mut patrol,
            Some((meteor_combat::hit_reaction::HitReactKind::Down, 0.8)),
            7,
            0.3,
            0.5,
        ));
        assert!((patrol.down_recovery_remaining - 0.6).abs() < 1e-6);
        assert!(update_npc_down_recovery(
            &mut patrol,
            Some((meteor_combat::hit_reaction::HitReactKind::Down, 0.8)),
            7,
            0.3,
            0.6,
        ));

        assert!(!update_npc_down_recovery(&mut patrol, None, 7, 0.3, 0.0,));
        assert_eq!(patrol.down_recovery_pose_instance_id, None);
        assert_eq!(patrol.down_recovery_remaining, 0.0);
    }

    #[test]
    fn queued_guard_uses_authored_duration_and_dodge_uses_existing_intent_channel() {
        let profile = NpcCombatProfile {
            guard: 65.0,
            ..default()
        };
        let mut intent = AiIntent {
            attack_cooldown: 0.2,
            ..default()
        };
        let mut performance = NpcPerformance::default();
        let mut patrol = NpcPatrol::default();
        apply_npc_opening_action(
            &mut intent,
            &mut performance,
            &profile,
            &mut patrol,
            NpcOpeningAction::Guard,
        );
        assert!(intent.want_guard);
        assert!((performance.guard_remaining - 0.4).abs() < 1e-6);

        apply_npc_opening_action(
            &mut intent,
            &mut performance,
            &profile,
            &mut patrol,
            NpcOpeningAction::Dodge,
        );
        assert_eq!(intent.dodge_burst, Some(false));
        assert!(intent.dodge_dir.length_squared() > 0.9);
    }

    #[test]
    fn production_ai_sees_the_local_fighter_and_drives_an_npc() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, drive_npc_ai);

        app.world_mut().spawn((
            LocalFighter,
            Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 6,
                weapon_slots: [6, 5],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Health::default(),
            Transform::from_xyz(10.0, 0.9, 0.0),
        ));
        let npc = app
            .world_mut()
            .spawn((
                NpcFighter,
                Fighter {
                    character_id: 1,
                    player_slot: 1,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                FighterAir::default(),
                AiIntent::default(),
                Transform::from_xyz(0.0, 0.9, 0.0),
                NpcPatrol::default(),
            ))
            .id();

        app.update();

        let intent = app.world().get::<AiIntent>(npc).unwrap();
        assert!(intent.move_dir.x > 0.9, "NPC AI did not follow the player");
        assert_eq!(
            intent.locomotion_input,
            Vec2::NEG_X,
            "a 90-degree route turn must use the matching side-step pose"
        );
        assert_eq!(intent.enemy_id, Some(0));
        assert_eq!(
            app.world().get::<MotionState>(npc).unwrap().movement_mode,
            meteor_core::MovementMode::Walk,
            "side-step pose must not move at forward-run speed"
        );

        for _ in 0..12 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(16));
            app.update();
        }
        let intent = app.world().get::<AiIntent>(npc).unwrap();
        assert_eq!(intent.locomotion_input, Vec2::Y);
        assert_eq!(
            app.world().get::<MotionState>(npc).unwrap().movement_mode,
            meteor_core::MovementMode::Run,
            "once facing catches the route, the NPC returns to forward run"
        );
    }

    #[test]
    fn production_ai_retains_targets_until_think_and_does_not_flee_on_low_hp() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_systems(Update, drive_npc_ai);
        let player = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health::default(),
                Transform::from_xyz(0.0, 0.9, 2.0),
            ))
            .id();
        let npc = app
            .world_mut()
            .spawn((
                NpcFighter,
                Fighter {
                    character_id: 1,
                    player_slot: 1,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health {
                    current: 1.0,
                    max: 100.0,
                },
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                FighterAir::default(),
                AiIntent::default(),
                Transform::from_xyz(0.0, 0.9, 0.0),
                NpcPatrol::default(),
                MissionNpc {
                    id: 7,
                    name: "guard".into(),
                    team: 2,
                    in_scene_roster: true,
                    view: 400.0,
                    think: 0.0,
                    forced_behavior: Some(BehaviorMode::Wait),
                    is_vip: false,
                    attack_target_slot: None,
                    behavior_args: Vec::new(),
                    attack_target_remaining: 0,
                },
            ))
            .id();
        let other = app
            .world_mut()
            .spawn((
                NpcFighter,
                Fighter {
                    character_id: 2,
                    player_slot: 2,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                FighterAir::default(),
                AiIntent::default(),
                Transform::from_xyz(0.0, 0.9, 8.0),
                NpcPatrol::default(),
                MissionNpc {
                    id: 8,
                    name: "other".into(),
                    team: 0,
                    in_scene_roster: true,
                    view: 0.0,
                    think: 50.0,
                    forced_behavior: Some(BehaviorMode::Idle),
                    is_vip: false,
                    attack_target_slot: None,
                    behavior_args: Vec::new(),
                    attack_target_remaining: 0,
                },
            ))
            .id();

        app.update();
        let intent = app.world().get::<AiIntent>(npc).unwrap();
        assert_eq!(intent.enemy_id, Some(0));
        assert!(intent.want_attack, "low HP melee NPC still fights like Web");

        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation
            .z = 8.0;
        app.world_mut()
            .get_mut::<Transform>(other)
            .unwrap()
            .translation
            .z = 1.0;
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        app.update();
        assert_eq!(
            app.world().get::<AiIntent>(npc).unwrap().enemy_id,
            Some(0),
            "target is stable between Think ticks"
        );

        for _ in 0..42 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(50));
            app.update();
        }
        let think_remaining = app.world().get::<NpcPatrol>(npc).unwrap().think_remaining;
        assert_eq!(
            app.world().get::<AiIntent>(npc).unwrap().enemy_id,
            Some(8),
            "next Think tick selects the now-closest visible hostile; remaining={think_remaining}"
        );
    }

    #[test]
    fn production_ai_retries_the_same_attack_pose_on_the_next_decision() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_systems(Update, drive_npc_ai);
        app.world_mut().spawn((
            LocalFighter,
            Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 6,
                weapon_slots: [6, 5],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Health::default(),
            Transform::from_xyz(0.0, 0.9, 4.0),
        ));
        let npc = app
            .world_mut()
            .spawn((
                NpcFighter,
                Fighter {
                    character_id: 1,
                    player_slot: 1,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                FighterAir::default(),
                AiIntent::default(),
                Transform::from_xyz(0.0, 0.9, 0.0),
                NpcPatrol::default(),
                SmState {
                    current: FighterState::Attack1,
                    attack_stage: 1,
                    ..default()
                },
                PoseRuntime::idle(),
                NpcCombatProfile {
                    attack2: 100.0,
                    ..default()
                },
            ))
            .id();

        app.update();
        assert!(app.world().get::<AiIntent>(npc).unwrap().want_attack);

        app.world_mut()
            .get_mut::<AiIntent>(npc)
            .unwrap()
            .want_attack = false;
        app.update();
        assert!(!app.world().get::<AiIntent>(npc).unwrap().want_attack);
        let mut retried = false;
        for _ in 0..10 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(50));
            app.update();
            retried |= app.world().get::<AiIntent>(npc).unwrap().want_attack;
        }
        assert!(
            retried,
            "a rejected attack is retried without requiring a new pose"
        );
    }

    #[test]
    fn production_npc_mover_consumes_dodge_and_burst_requests_into_dodge_state() {
        for burst in [false, true] {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_systems(Update, move_npc_fighters);
            let direction = Vec2::new(0.0, -1.0);
            let fighter = Fighter {
                character_id: 1,
                player_slot: 1,
                weapon: 6,
                weapon_slots: [6, 5],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            };
            let expected_pose = meteor_physics::locomotion::dodge_pose_for_input(
                direction.x,
                direction.y,
                burst,
                burst,
                Some(meteor_combat::weapon_strategy::weapon_to_strategy(
                    fighter.weapon,
                )),
            );
            let entity = app
                .world_mut()
                .spawn((
                    NpcFighter,
                    fighter,
                    AiIntent {
                        dodge_burst: Some(burst),
                        dodge_dir: direction,
                        ..default()
                    },
                    Transform::default(),
                    MotionState {
                        movement_mode: meteor_core::MovementMode::Idle,
                        velocity: Vec3::ZERO,
                        facing: 0.0,
                    },
                    FighterAir::default(),
                    FighterDodge::default(),
                    PoseRuntime::idle(),
                    EntryBlend::default(),
                    SmState::default(),
                    CombatState::default(),
                ))
                .id();

            app.update();

            let intent = app.world().get::<AiIntent>(entity).unwrap();
            let dodge = app.world().get::<FighterDodge>(entity).unwrap();
            let state = app.world().get::<SmState>(entity).unwrap();
            let combat = app.world().get::<CombatState>(entity).unwrap();
            assert_eq!(intent.dodge_burst, None);
            assert!(dodge.state.active);
            assert_eq!(dodge.pose, expected_pose);
            assert_eq!(
                state.current,
                if burst {
                    FighterState::Burst
                } else {
                    FighterState::Dodge
                }
            );
            assert_eq!(combat.state, state.current);
        }
    }

    #[test]
    fn production_npc_wheel_out_rejects_dodge_but_keeps_burst() {
        for burst in [false, true] {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_systems(Update, move_npc_fighters);
            let entity = app
                .world_mut()
                .spawn((
                    NpcFighter,
                    Fighter {
                        character_id: 1,
                        player_slot: 1,
                        weapon: 2,
                        weapon_slots: [2, 5],
                        weapon_slot: 0,
                        weapon_slot_variants: [0, 0],
                        weapon_variant: 0,
                    },
                    RangedFireGate {
                        wheel_out: true,
                        ..default()
                    },
                    AiIntent {
                        dodge_burst: Some(burst),
                        dodge_dir: Vec2::new(0.0, -1.0),
                        ..default()
                    },
                    Transform::default(),
                    MotionState {
                        movement_mode: meteor_core::MovementMode::Idle,
                        velocity: Vec3::ZERO,
                        facing: 0.0,
                    },
                    FighterAir::default(),
                    FighterDodge::default(),
                    PoseRuntime::idle(),
                    EntryBlend::default(),
                    SmState::default(),
                    CombatState::default(),
                ))
                .id();

            app.update();

            let dodge = app.world().get::<FighterDodge>(entity).unwrap();
            let state = app.world().get::<SmState>(entity).unwrap();
            let combat = app.world().get::<CombatState>(entity).unwrap();
            assert_eq!(dodge.state.active, burst);
            assert_eq!(
                state.current,
                if burst {
                    FighterState::Burst
                } else {
                    FighterState::Name
                }
            );
            assert_eq!(combat.state, state.current);
        }
    }

    #[test]
    fn production_npc_dodge_finish_clears_both_state_projections() {
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
        let mut app = App::new();
        app.init_resource::<Time>();
        app.insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
            "p1".to_string(),
            character,
        )])));
        app.insert_resource(crate::plugins::character_plugin::CommonAmb(common));
        app.insert_resource(StageCollider {
            mesh: Some(StageMeshCollider::default()),
        });
        app.init_resource::<crate::plugins::input_plugin::LocalGuardInput>();
        app.add_systems(
            Update,
            (
                move_npc_fighters,
                crate::plugins::movement_plugin::dodge_root_motion,
            )
                .chain(),
        );
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.0 / 30.0));
        let mut dodge = FighterDodge::default();
        dodge.state.start(176, 0.0, true, 0.0, 0.0);
        dodge.pose = 176;
        dodge.pose_entered = true;
        let entity = app
            .world_mut()
            .spawn((
                NpcFighter,
                Fighter {
                    character_id: 1,
                    player_slot: 1,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                AiIntent::default(),
                Transform::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Dodge,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                FighterAir::default(),
                dodge,
                PoseRuntime {
                    pose_id: 176,
                    frame: 581.0,
                    start: 570,
                    end: 581,
                    loop_start: 570,
                    loop_end: 581,
                    loops: false,
                    pose_instance_id: 1,
                },
                EntryBlend::default(),
                SmState {
                    current: FighterState::Burst,
                    linked_recovery_pose_id: Some(210),
                    exe_ground_mode_ticks: 1,
                    ..default()
                },
                CombatState {
                    state: FighterState::Burst,
                },
            ))
            .id();

        app.update();

        let state = app.world().get::<SmState>(entity).unwrap();
        let combat = app.world().get::<CombatState>(entity).unwrap();
        assert_eq!(state.current, FighterState::Name);
        assert_eq!(combat.state, FighterState::Name);
        assert_eq!(state.linked_recovery_pose_id, None);
        assert_eq!(state.exe_ground_mode_ticks, 0);
        assert!(
            !app.world()
                .get::<FighterDodge>(entity)
                .unwrap()
                .state
                .active
        );
    }

    #[test]
    fn production_perform_queue_holds_commands_behind_pause_then_faces_target() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.init_resource::<crate::plugins::ui_plugin::DialogueState>();
        app.add_systems(Update, drive_npc_ai);
        app.world_mut().spawn((
            LocalFighter,
            Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 6,
                weapon_slots: [6, 5],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Health::default(),
            Transform::from_xyz(4.0, 0.9, 0.0),
        ));
        let mut performance = NpcPerformance::default();
        performance.queue.extend([
            crate::plugins::stage_plugin::NpcPerformCommand {
                action: "say".into(),
                args: vec![meteor_scene::pst_runtime::PstValue::Str("先说".into())],
            },
            crate::plugins::stage_plugin::NpcPerformCommand {
                action: "pause".into(),
                args: vec![meteor_scene::pst_runtime::PstValue::Num(2.0)],
            },
            crate::plugins::stage_plugin::NpcPerformCommand {
                action: "faceto".into(),
                args: vec![meteor_scene::pst_runtime::PstValue::Num(0.0)],
            },
        ]);
        let npc = app
            .world_mut()
            .spawn((
                NpcFighter,
                Fighter {
                    character_id: 1,
                    player_slot: 1,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                FighterAir::default(),
                AiIntent::default(),
                Transform::from_xyz(0.0, 0.9, 0.0),
                NpcPatrol::default(),
                SmState::default(),
                CombatState::default(),
                performance,
                MissionNpc {
                    id: 7,
                    name: "守卫".into(),
                    team: 2,
                    in_scene_roster: true,
                    view: 400.0,
                    think: 50.0,
                    forced_behavior: Some(BehaviorMode::Idle),
                    is_vip: false,
                    attack_target_slot: None,
                    behavior_args: Vec::new(),
                    attack_target_remaining: 0,
                },
            ))
            .id();

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        for _ in 0..21 {
            app.update();
        }
        assert_eq!(
            app.world()
                .resource::<crate::plugins::ui_plugin::DialogueState>()
                .text
                .as_deref(),
            Some("守卫：先说")
        );
        assert_eq!(
            app.world().get::<NpcPerformance>(npc).unwrap().queue.len(),
            2
        );

        for _ in 0..140 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(16));
            app.update();
        }
        assert!(app
            .world()
            .get::<NpcPerformance>(npc)
            .unwrap()
            .queue
            .is_empty());
        let facing = app.world().get::<MotionState>(npc).unwrap().facing;
        assert!((facing - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        app.world_mut()
            .get_mut::<NpcPerformance>(npc)
            .unwrap()
            .queue
            .extend([
                crate::plugins::stage_plugin::NpcPerformCommand {
                    action: "guard".into(),
                    args: vec![meteor_scene::pst_runtime::PstValue::Num(0.2)],
                },
                crate::plugins::stage_plugin::NpcPerformCommand {
                    action: "say".into(),
                    args: vec![meteor_scene::pst_runtime::PstValue::Str("防御结束".into())],
                },
            ]);
        for _ in 0..7 {
            app.update();
        }
        assert_eq!(
            app.world().get::<NpcPerformance>(npc).unwrap().queue.len(),
            1
        );
        assert_eq!(
            app.world()
                .resource::<crate::plugins::ui_plugin::DialogueState>()
                .text
                .as_deref(),
            Some("守卫：先说")
        );
        for _ in 0..40 {
            app.update();
        }
        assert_eq!(
            app.world()
                .resource::<crate::plugins::ui_plugin::DialogueState>()
                .text
                .as_deref(),
            Some("守卫：防御结束")
        );
        app.world_mut()
            .get_mut::<MissionNpc>(npc)
            .unwrap()
            .forced_behavior = Some(BehaviorMode::Dodge);
        app.world_mut().get_mut::<AiIntent>(npc).unwrap().enemy_id = None;
        app.world_mut().get_mut::<SmState>(npc).unwrap().current = FighterState::BeHit;
        app.world_mut()
            .get_mut::<NpcPerformance>(npc)
            .unwrap()
            .queue
            .push_back(crate::plugins::stage_plugin::NpcPerformCommand {
                action: "aggress".into(),
                args: vec![],
            });
        for _ in 0..40 {
            app.update();
        }
        assert_eq!(
            app.world().get::<NpcPerformance>(npc).unwrap().queue.len(),
            1
        );
        assert!(!app.world().get::<AiIntent>(npc).unwrap().want_taunt);
        app.world_mut().get_mut::<SmState>(npc).unwrap().current = FighterState::Name;
        for _ in 0..21 {
            app.update();
        }
        let intent = app.world().get::<AiIntent>(npc).unwrap();
        assert!(intent.want_taunt);
        assert_eq!(intent.enemy_id, None);
        assert!(!intent.want_attack);
        assert_eq!(
            app.world().get::<MissionNpc>(npc).unwrap().forced_behavior,
            Some(BehaviorMode::Dodge)
        );
    }
}
