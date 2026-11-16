mod decision_space;
mod getitem_route;
mod obstacle_avoidance;
mod perception;
mod pursuit_recovery;
mod weapon_replacement;
use decision_space::{npc_roll, queue_command, DecisionSpace};
pub(crate) use weapon_replacement::NpcWeaponDropRequest;

use crate::components::fighter::{
    AiIntent, CombatState, EntryBlend, Fighter, FighterHitReaction, Health, MotionState,
    PoseRuntime, RangedFireGate, SmState,
};
use crate::plugins::character_contact_plugin::{
    commit_fighter_position, resolve_fighter_xz, CharacterContactFrame,
};
use crate::plugins::character_plugin::CharacterAssetsSet;
use crate::plugins::combat_plugin::{fighter_speed_factor, CombatResourceData, NpcFighter};
use crate::plugins::item_plugin::{npc_getitem_model, StageProp};
use crate::plugins::movement_plugin::{
    accepted_motion_velocity, apply_dodge_pose_now, clamp_air_motion_step, enter_dodge_state,
    finish_air_landing_state, ordinary_locomotion_allowed, project_air_velocity_on_surface,
    project_planar_air_velocity_on_contact, sample_air_ground, update_ground_external_motion,
    wheel_vetoes_dodge_action, FighterAir, FighterDodge, COLLIDER_HEIGHT, COLLIDER_RADIUS,
};
use crate::plugins::network_plugin::RemoteFighter;
use crate::plugins::stage_collider::{StageCollider, StageMeshCollider};
use crate::plugins::stage_plugin::{
    LocalEscortState, LocalFighter, MissionNpc, NpcCombatProfile, NpcPerformance,
    StageMissionRuntime,
};
use bevy::prelude::*;
use meteor_ai::behavior::BehaviorMode;
#[cfg(test)]
use meteor_ai::behavior::{find_waypoint_path, Waypoint};
use meteor_core::FighterState;
use meteor_physics::jump::{RUN_SPEED, WALK_SPEED};
use meteor_physics::{
    collision::{CollisionSystem, FootHit, MeshCollider, Point3},
    jump,
};
use meteor_scene::stage_loader::WaypointGraph;

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

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct StockNpcBrain;

#[derive(Component, Debug, Clone)]
pub struct NpcPatrol {
    behavior_stack: Vec<NpcBehaviorFrame>,
    temporary_target: Option<i64>,
    behavior_ticks: i32,
    pub(crate) attack_target_point: Option<Vec3>,
    navigation: super::npc_navigation::Navigation,
    patrol_navigation: super::npc_navigation::Navigation,
    navigation_frame_step: NpcTravelStep,

    pub route: Vec<usize>,
    patrol_destination: Option<usize>,
    pub index: usize,
    think_remaining: f32,

    first_update_pending: bool,
    init_delay: f32,
    script_tick_due: bool,
    collision_clock: obstacle_avoidance::CollisionClock,
    avoidance: Option<ObstacleCallback>,
    path_heading: Option<f32>,
    path_input: Vec2,
    last_weapon_switch_time: Option<f64>,
    perception: perception::Perception,
    navigation_reset_pending: bool,
    airborne_heading: Option<f32>,
    airborne_input: Vec2,
    combat_spacing: Option<BehaviorMode>,
    combat_ticks: i32,
    combat_strafe: f32,
    rng: u64,
    wander_goal: Option<usize>,

    idle_facing: Option<f32>,
    flee_goal: Option<usize>,

    getitem_goal: Option<Vec3>,
    getitem_entity: Option<Entity>,
    getitem_ticks: i32,

    last_getitem_record: Option<u64>,
    getitem_scan_ticks: i32,
    getitem_direct: bool,

    current_waypoint: Option<usize>,
    travel_goal: Option<usize>,
    travel_route: Vec<usize>,

    patrol_goal: Option<usize>,
    patrol_route: Vec<usize>,

    command_memory: meteor_ai::combat::CommandMemory,
    sampled_hp: Option<f32>,
}

#[derive(Debug, Clone)]
struct NpcBehaviorFrame {
    getitem: (Option<Vec3>, Option<Entity>, i32, bool),
    attack_target_point: Option<Vec3>,
    temporary_target: Option<i64>,
    mode: Option<BehaviorMode>,
    args: Vec<meteor_scene::pst_runtime::PstValue>,
    target_slot: Option<i64>,
    target_remaining: i64,
    route: Vec<usize>,
    index: usize,
    perception: perception::Perception,
    enemy: Option<i64>,
}

#[derive(Debug, Clone)]
struct ObstacleCallback {
    ticks: i32,
    desired_yaw: f32,
    forward: bool,
}

impl Default for NpcPatrol {
    fn default() -> Self {
        Self {
            behavior_stack: Vec::new(),
            temporary_target: None,
            behavior_ticks: -1,
            attack_target_point: None,
            navigation: default(),
            patrol_navigation: default(),
            navigation_frame_step: default(),
            route: Vec::new(),
            patrol_destination: None,
            index: 0,
            think_remaining: 0.0,
            first_update_pending: false,
            init_delay: 0.0,
            script_tick_due: false,
            collision_clock: default(),
            avoidance: None,
            path_heading: None,
            path_input: Vec2::ZERO,
            last_weapon_switch_time: None,
            perception: default(),
            navigation_reset_pending: false,
            airborne_heading: None,
            airborne_input: Vec2::Y,
            combat_spacing: None,
            combat_ticks: -1,
            combat_strafe: 0.0,
            rng: 0x9e37_79b9_7f4a_7c15,
            wander_goal: None,
            idle_facing: None,
            flee_goal: None,
            getitem_goal: None,
            getitem_entity: None,
            getitem_ticks: -1,
            last_getitem_record: None,
            getitem_scan_ticks: -1,
            getitem_direct: false,
            current_waypoint: None,
            travel_goal: None,
            travel_route: Vec::new(),
            patrol_goal: None,
            patrol_route: Vec::new(),
            command_memory: default(),
            sampled_hp: None,
        }
    }
}

impl NpcPatrol {
    #[cfg(test)]
    pub(crate) fn callback_ticks_for_test(&self) -> (i32, i32) {
        (self.behavior_ticks, self.combat_ticks)
    }

    pub(crate) fn finish_callback_install(&mut self) {
        self.behavior_ticks = 0;
        self.combat_ticks = 0;
        if self.getitem_goal.is_some() {
            self.getitem_ticks = 0;
        }
    }
    pub(crate) fn schedule_initial_think(&mut self, think: f32, actor_id: i64) {
        self.first_update_pending = true;
        self.schedule_reset_think(think, actor_id);
    }

    pub(crate) fn schedule_reset_think(&mut self, think: f32, actor_id: i64) {
        if self.rng == 0x9e37_79b9_7f4a_7c15 {
            self.rng ^= (actor_id as u64)
                .wrapping_add(1)
                .wrapping_mul(0xbf58_476d_1ce4_e5b9);
        }
        self.init_delay = (npc_roll(self) % 1000) as f32 * 0.0005;
        self.think_remaining = meteor_ai::combat::decision_interval(think, self.init_delay);
    }

    fn stop_navigation(&mut self) {
        self.navigation_frame_step = default();
        self.navigation.stop();
        self.patrol_navigation.stop();
        self.current_waypoint = None;
        self.travel_goal = None;
        self.travel_route.clear();
        self.patrol_goal = None;
        self.patrol_route.clear();
        self.path_input = Vec2::ZERO;
        self.airborne_input = Vec2::ZERO;
        self.collision_clock = default();
    }
    #[cfg(test)]
    pub(crate) fn set_decision_for_test(&mut self, remaining: f32, rng: u64) {
        self.think_remaining = remaining;
        self.rng = rng;
    }

    #[cfg(test)]
    pub(crate) fn return_combat_to_parent_for_test(&mut self) {
        self.perception.return_from_failed_pursuit(None);
    }

    pub(crate) fn invalidate_navigation(&mut self) {
        self.navigation_frame_step = default();
        self.navigation = default();
        self.patrol_navigation = default();
        self.current_waypoint = None;
        self.wander_goal = None;
        self.idle_facing = None;
        self.flee_goal = None;
        self.getitem_direct = false;
        self.getitem_goal = None;
        self.getitem_entity = None;
        self.getitem_ticks = -1;
        self.travel_goal = None;
        self.travel_route.clear();
        self.patrol_goal = None;
        self.patrol_destination = None;
        self.patrol_route.clear();
    }

    pub(crate) fn prepare_behavior_change(
        &mut self,
        npc: Option<&MissionNpc>,
        intent: &AiIntent,
        mode: Option<BehaviorMode>,
    ) {
        let mut stack = std::mem::take(&mut self.behavior_stack);
        if matches!(
            mode,
            Some(BehaviorMode::Dodge | BehaviorMode::Kill | BehaviorMode::AttackTarget)
        ) {
            stack.push(NpcBehaviorFrame {
                getitem: (
                    self.getitem_goal,
                    self.getitem_entity,
                    self.getitem_ticks,
                    self.getitem_direct,
                ),
                attack_target_point: self.attack_target_point,
                temporary_target: self.temporary_target,
                mode: npc.and_then(|npc| npc.forced_behavior),
                args: npc.map_or_else(Vec::new, |npc| npc.behavior_args.clone()),
                target_slot: npc.and_then(|npc| npc.attack_target_slot),
                target_remaining: npc.map_or(0, |npc| npc.attack_target_remaining),
                route: self.route.clone(),
                index: self.index,
                perception: self.perception.clone(),
                enemy: intent.enemy_id,
            });
        } else {
            stack.clear();
        }
        let sampled_hp = self.sampled_hp;
        self.reset_behavior_state();
        self.sampled_hp = sampled_hp;
        self.behavior_stack = stack;
    }

    fn return_to_parent(&mut self, npc: Option<&mut MissionNpc>, intent: &mut AiIntent) -> bool {
        let Some(frame) = self.behavior_stack.pop() else {
            return false;
        };
        let stack = std::mem::take(&mut self.behavior_stack);
        let remaining = self.think_remaining;
        let sampled_hp = self.sampled_hp;
        self.reset_behavior_state();
        self.behavior_stack = stack;
        self.think_remaining = remaining;
        self.sampled_hp = sampled_hp;
        (
            self.getitem_goal,
            self.getitem_entity,
            self.getitem_ticks,
            self.getitem_direct,
        ) = frame.getitem;
        self.attack_target_point = frame.attack_target_point;
        self.route = frame.route;
        self.index = frame.index;
        self.perception = frame.perception;
        self.temporary_target = frame.temporary_target;
        self.finish_callback_install();
        if let Some(npc) = npc {
            npc.forced_behavior = frame.mode;
            npc.behavior_args = frame.args;
            npc.attack_target_slot = frame.target_slot;
            npc.attack_target_remaining = frame.target_remaining;
        }
        *intent = default();
        intent.enemy_id = frame.enemy;
        intent.attack_cooldown = remaining;
        true
    }

    pub(crate) fn reset_behavior_state(&mut self) {
        self.navigation_frame_step = default();
        self.behavior_stack.clear();
        self.attack_target_point = None;
        self.temporary_target = None;
        self.behavior_ticks = -1;
        self.getitem_scan_ticks = -1;
        self.current_waypoint = None;
        self.navigation = default();
        self.patrol_navigation = default();
        self.think_remaining = 0.0;
        self.perception = default();
        self.navigation_reset_pending = false;
        self.collision_clock = default();
        self.avoidance = None;
        self.path_heading = None;
        self.path_input = Vec2::ZERO;
        self.airborne_heading = None;
        self.airborne_input = Vec2::Y;
        self.combat_spacing = None;
        self.combat_ticks = -1;
        self.combat_strafe = 0.0;
        self.sampled_hp = None;
        self.route.clear();
        self.index = 0;
        self.wander_goal = None;
        self.idle_facing = None;
        self.flee_goal = None;
        self.getitem_direct = false;
        self.getitem_goal = None;
        self.getitem_entity = None;
        self.getitem_ticks = -1;
        self.travel_goal = None;
        self.travel_route.clear();
        self.patrol_goal = None;
        self.patrol_destination = None;
        self.patrol_route.clear();
    }

    fn travel_step(
        &mut self,
        graph: Option<&WaypointGraph>,
        mesh: Option<&StageMeshCollider>,
        from: Vec3,
        target: Vec3,
        replan: bool,
    ) -> NpcTravelStep {
        if replan {
            self.stop_navigation();
            let mut rng = self.rng;
            self.navigation.request_path(graph, mesh, from, target, || {
                (next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32
            });
            self.rng = rng;
        }
        navigation_step(
            &self.navigation,
            from,
            !replan && self.navigation_frame_step.arrived,
        )
    }

    fn patrol_step(
        &mut self,
        graph: Option<&WaypointGraph>,
        mesh: Option<&StageMeshCollider>,
        from: Vec3,
        target: Vec3,
        replan: bool,
    ) -> NpcTravelStep {
        if replan {
            self.stop_navigation();
            let mut rng = self.rng;
            self.patrol_navigation
                .request_path(graph, mesh, from, target, || {
                    (next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32
                });
            self.rng = rng;
        }
        navigation_step(
            &self.patrol_navigation,
            from,
            !replan && self.navigation_frame_step.arrived,
        )
    }

    fn advance_navigation(&mut self, from: Vec3, velocity: Vec3) {
        let mut rng = self.rng;
        let navigation = if self.navigation.in_progress() {
            &mut self.navigation
        } else {
            &mut self.patrol_navigation
        };
        let activation_serial = navigation.activation_serial();
        let (direction, arrived) = navigation.follow(from, velocity, || {
            (next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32
        });
        self.navigation_frame_step = NpcTravelStep {
            direction,
            arrived,
            failed: navigation.failed(),
            jump_target: navigation.jump_target(),
        };
        self.rng = rng;
        let activated =
            navigation.in_progress() && navigation.activation_serial() != activation_serial;
        if arrived || activated {
            self.path_input = Vec2::ZERO;
            self.airborne_input = Vec2::ZERO;
            self.collision_clock = default();
        }
    }
}

#[cfg(test)]
const PATROL_ROUTE_LEN: usize = 4;

#[cfg(test)]
const PATROL_ARRIVE_DIST: f32 = 1.2;

pub(crate) const STOCK_NPC_VIEW: f32 = 10_000.0;
pub(crate) const STOCK_NPC_THINK: f32 = 10_000.0;

pub(crate) fn stock_npc_profile() -> NpcCombatProfile {
    NpcCombatProfile {
        attack1: 30.0,
        attack2: 25.0,
        attack3: 25.0,
        guard: 20.0,
        dodge: 0.0,
        jump: 1.0,
        look: 1000.0,
        burst: 30.0,
        aim: 100.0,
        get_item: 0.0,
    }
}

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
    facing: f32,
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
            facing,
            profile.map_or(100.0, |p| p.aim),
            rolls,
        )));
    }
}

fn yaw_towards(delta: Vec3) -> Option<f32> {
    (delta.x * delta.x + delta.z * delta.z > 1e-6).then(|| delta.x.atan2(delta.z))
}

fn npc_script_callback_delay(mode: Option<BehaviorMode>) -> f32 {
    match mode {
        Some(BehaviorMode::Wait | BehaviorMode::Idle | BehaviorMode::Dodge) => 0.4,
        Some(
            BehaviorMode::Patrol
            | BehaviorMode::Run
            | BehaviorMode::Follow
            | BehaviorMode::Kill
            | BehaviorMode::AttackTarget,
        ) => 0.3,
        _ => 0.2,
    }
}

fn npc_combat_turn(facing: f32, target: f32, dt: f32) -> f32 {
    let direct_error = target - facing;
    let error = if direct_error.abs() < std::f32::consts::PI {
        direct_error
    } else {
        npc_facing_error(facing, target)
    };

    if error.abs() < 1.0_f32.to_radians() {
        return facing;
    }
    if error.abs() < 3.0_f32.to_radians() {
        return facing + error;
    }
    let whole_error = error.to_degrees().trunc().abs().max(3.0).to_radians();
    let step = (500.0_f32.to_radians() * dt).min(whole_error);
    facing + error.signum() * step
}

fn npc_facing_error(facing: f32, target: f32) -> f32 {
    (target - facing + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI
}

fn npc_can_turn(state: Option<&SmState>, airborne: bool) -> bool {
    airborne || state.is_none_or(|state| state.current == FighterState::Name)
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

#[derive(Debug, Clone, Copy, Default)]
struct NpcTravelStep {
    failed: bool,
    jump_target: Option<Vec3>,
    direction: Vec3,
    arrived: bool,
}

fn navigation_step(
    navigation: &super::npc_navigation::Navigation,
    from: Vec3,
    arrived: bool,
) -> NpcTravelStep {
    NpcTravelStep {
        direction: navigation.steering_target().map_or(Vec3::ZERO, |target| {
            Vec3::new(target.x - from.x, 0.0, target.z - from.z).normalize_or_zero()
        }),
        arrived,
        failed: navigation.failed(),
        jump_target: navigation.jump_target(),
    }
}

fn route_reconsider(active: bool, remaining: usize, roll: u32) -> bool {
    !active || roll % (remaining as u32 + 10) == 0
}

fn attack_goal_reconsider(tick: i32, remaining: usize) -> bool {
    (tick as u32) % (remaining as u32 + 10) == 0
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

fn npc_think_interval(think: f32) -> f32 {
    meteor_ai::combat::decision_interval(think, 0.2)
}

fn queue_npc_down_recovery(
    intent: &mut AiIntent,
    roll: u32,
    attack_allowed: bool,
    facing: f32,
    position: Vec3,
    mesh: Option<&StageMeshCollider>,
) {
    let direction = match roll % 8 {
        0 => Vec2::Y,
        1 => Vec2::NEG_Y,
        2 => Vec2::NEG_X,
        3 => Vec2::X,
        4 => {
            intent.want_attack = attack_allowed;
            return;
        }
        _ => return,
    };
    if (DecisionSpace {
        position,
        facing,
        ground_controllable: true,
        mesh,
    })
    .supported(direction, 50.0)
    {
        intent.dodge_dir = direction;
        intent.dodge_burst = Some(false);
    }
}

fn queue_npc_opening_action(
    intent: &mut AiIntent,
    performance: &mut NpcPerformance,
    profile: Option<&NpcCombatProfile>,
    patrol: &mut NpcPatrol,
    weapon: u8,
    airborne: bool,
    attack_allowed: bool,
    space: DecisionSpace<'_>,
) {
    intent.want_attack = false;
    intent.want_guard = false;
    intent.dodge_burst = None;
    intent.attack_directions.clear();
    let Some(profile) = profile else {
        intent.want_attack = attack_allowed;
        return;
    };

    if ((npc_roll(patrol) % 100) as f32) < profile.burst {
        let direction = [
            Vec2::Y,
            -Vec2::Y,
            -Vec2::Y,
            -Vec2::X,
            -Vec2::X,
            Vec2::X,
            Vec2::X,
        ][npc_roll(patrol) as usize % 7];
        if space.supported(direction, 70.0) {
            intent.dodge_dir = direction;
            intent.dodge_burst = Some(false);
            patrol.think_remaining = intent.attack_cooldown * 2.5;
            intent.attack_cooldown = patrol.think_remaining;
            return;
        }
    }

    if patrol.combat_ticks % 3 == 0 {
        let direction = match npc_roll(patrol) % 10 {
            0 => 1.0,
            1 => -1.0,
            _ => 0.0,
        };
        patrol.combat_strafe = if direction != 0.0 && space.supported(Vec2::X * direction, 50.0) {
            direction
        } else {
            0.0
        };
    }
    let choice = choose_npc_opening_action(profile, (npc_roll(patrol) % 100) as f32 / 100.0);
    apply_npc_opening_action(intent, performance, profile, patrol, choice);
    if intent.want_attack {
        if !attack_allowed {
            intent.want_attack = false;
            return;
        }
        let category = meteor_ai::combat::attack_category(
            [profile.attack1, profile.attack2, profile.attack3],
            npc_roll(patrol),
        );
        queue_command(intent, patrol, weapon, category, airborne, attack_allowed);
    }
}

fn apply_npc_opening_action(
    intent: &mut AiIntent,
    performance: &mut NpcPerformance,
    _profile: &NpcCombatProfile,
    patrol: &mut NpcPatrol,
    choice: NpcOpeningAction,
) {
    performance.crouching = false;
    match choice {
        NpcOpeningAction::Attack => intent.want_attack = true,
        NpcOpeningAction::Guard => {
            performance.crouching = npc_roll(patrol) % 5 == 0;
            intent.want_guard = !performance.crouching;
            patrol.think_remaining = intent.attack_cooldown * 2.0;
            intent.attack_cooldown = patrol.think_remaining;
            performance.guard_remaining = if intent.want_guard {
                patrol.think_remaining
            } else {
                0.0
            };
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

fn npc_target_visible<M: MeshCollider + ?Sized>(
    origin: Vec3,
    target: Vec3,
    weapon: u8,
    view_range: f32,
    collider: Option<&M>,
) -> bool {
    let origin = origin
        + Vec3::Y * meteor_ai::combat::weapon_tactics(weapon).aim_height * meteor_core::MODEL_SCALE;
    let target = target + Vec3::Y * (20.0 * meteor_core::MODEL_SCALE);
    let delta = target - origin;
    let distance = delta.length();
    if distance > view_range {
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
            Point3::new(origin.x, origin.y, origin.z),
            Point3::new(direction.x, direction.y, direction.z),
            distance,
        )
        .is_none_or(|hit| hit.t >= distance)
}

fn behavior_scans_enemies(mode: Option<BehaviorMode>) -> bool {
    !matches!(
        mode,
        Some(BehaviorMode::Idle | BehaviorMode::AttackTarget | BehaviorMode::Dodge)
    )
}

fn getitem_seek_range_world(view_model_units: f32) -> f32 {
    (view_model_units * 0.5).min(2000.0).max(0.0) * meteor_core::MODEL_SCALE
}

fn getitem_arrived(npc_pos: Vec3, item_pos: Vec3) -> bool {
    let dx = (npc_pos.x - item_pos.x).abs();
    let dy = (npc_pos.y - item_pos.y).abs();
    let dz = (npc_pos.z - item_pos.z).abs();
    dx < 20.0 * meteor_core::MODEL_SCALE
        && dz < 20.0 * meteor_core::MODEL_SCALE
        && dy < 60.0 * meteor_core::MODEL_SCALE
}

fn behavior_may_seek_getitem(mode: BehaviorMode) -> bool {
    matches!(
        mode,
        BehaviorMode::Wait
            | BehaviorMode::Patrol
            | BehaviorMode::Run
            | BehaviorMode::Follow
            | BehaviorMode::Kill
            | BehaviorMode::Attack
    )
}

#[cfg(test)]
fn npc_seekable_prop(prop: &StageProp, visibility: &Visibility) -> bool {
    npc_seekable_prop_active(prop, visibility, None)
}

fn npc_seekable_prop_active(
    prop: &StageProp,
    visibility: &Visibility,
    active: Option<bool>,
) -> bool {
    active.unwrap_or(*visibility != Visibility::Hidden && prop.interactive)
        && !prop.broken
        && prop.respawn_timer <= 0.0
        && npc_getitem_model(&prop.model)
}

fn getitem_scan_points(npc_pos: Vec3, item_pos: Vec3) -> (Vec3, Vec3) {
    (
        npc_pos + Vec3::Y * (40.0 * meteor_core::MODEL_SCALE),
        item_pos + Vec3::Y * (20.0 * meteor_core::MODEL_SCALE),
    )
}

fn getitem_visible<M: MeshCollider + ?Sized>(
    npc_pos: Vec3,
    item_pos: Vec3,
    collider: Option<&M>,
) -> bool {
    let Some(collider) = collider else {
        return true;
    };
    let (origin, target) = getitem_scan_points(npc_pos, item_pos);
    let delta = target - origin;
    let distance = delta.length();
    if distance <= 0.001 {
        return true;
    }
    let direction = delta / distance;
    collider
        .raycast_hit(
            Point3::new(origin.x, origin.y, origin.z),
            Point3::new(direction.x, direction.y, direction.z),
            distance,
        )
        .is_none_or(|hit| hit.t >= distance)
}

#[derive(Clone, Copy, Debug)]
struct GetItemCandidate {
    record: u64,
    entity: Entity,
    position: Vec3,
}

fn getitem_record(
    entity: Entity,
    order: Option<&crate::plugins::item_plugin::SceneRecordOrder>,
) -> u64 {
    order.map_or((1_u64 << 63) | entity.to_bits(), |order| order.0 as u64)
}

fn first_getitem_target(
    npc_pos: Vec3,
    range: f32,
    previous: Option<u64>,
    candidates: impl IntoIterator<Item = GetItemCandidate>,
) -> Option<GetItemCandidate> {
    candidates
        .into_iter()
        .filter(|candidate| {
            let (origin, target) = getitem_scan_points(npc_pos, candidate.position);
            Some(candidate.record) != previous && target.distance_squared(origin) <= range * range
        })
        .min_by_key(|candidate| (candidate.record, candidate.entity.to_bits()))
        .map(|mut candidate| {
            candidate.position.y += 20.0 * meteor_core::MODEL_SCALE;
            candidate
        })
}

fn try_start_getitem_seek(
    patrol: &mut NpcPatrol,
    profile: Option<&NpcCombatProfile>,
    view_model_units: f32,
    npc_pos: Vec3,
    candidates: impl IntoIterator<Item = GetItemCandidate>,
) {
    if patrol.getitem_goal.is_some() {
        return;
    }
    let tick = patrol.getitem_scan_ticks;
    patrol.getitem_scan_ticks = tick.wrapping_add(1);
    if tick % 4 != 3 {
        return;
    }
    let Some(profile) = profile else {
        return;
    };

    let roll = (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
    if (roll % 100) as f32 > profile.get_item {
        return;
    }
    let range = getitem_seek_range_world(view_model_units);
    if let Some(candidate) =
        first_getitem_target(npc_pos, range, patrol.last_getitem_record, candidates)
    {
        patrol.last_getitem_record = Some(candidate.record);
        patrol.getitem_entity = Some(candidate.entity);
        patrol.getitem_goal = Some(candidate.position);
    }
}

fn npc_stock_drop_ready(net: &crate::plugins::network_plugin::NetState, entity: Entity) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        net.is_stock_host() && net.round_active && net.stock_npc_ready.contains(&entity)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (net, entity);
        false
    }
}

fn concealed_escape_waypoint(
    graph: &WaypointGraph,
    mesh: &StageMeshCollider,
    start_position: Vec3,
    threat: Vec3,
    mut roll: impl FnMut() -> u32,
) -> Option<usize> {
    let start = super::npc_navigation::nearest_original(graph, Some(mesh), start_position)?;
    let mut visited = std::collections::HashSet::from([start]);
    let mut queue = std::collections::VecDeque::from([start]);
    let target = threat + Vec3::Y * (30.0 * meteor_core::MODEL_SCALE);
    for _ in 0..20 {
        let Some(id) = queue.pop_front() else {
            break;
        };
        let Some(position) = graph.world_position(id).map(Vec3::from_array) else {
            continue;
        };
        let delta = target - position;
        let distance = delta.length();
        if roll() % 4 != 0
            && distance > 0.001
            && mesh
                .raycast_hit(
                    Point3::new(position.x, position.y, position.z),
                    Point3::new(delta.x / distance, delta.y / distance, delta.z / distance),
                    distance,
                )
                .is_some_and(|hit| hit.t < distance)
        {
            return Some(id);
        }
        if let Some(point) = graph.points.get(id) {
            for (next, _) in &point.links {
                if graph.is_enabled(*next) && visited.insert(*next) {
                    queue.push_back(*next);
                }
            }
        }
    }
    None
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
        (&'static FighterHitReaction, Option<&'static StockNpcBrain>),
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
            (drive_npc_ai, finish_npc_script_tick)
                .chain()
                .in_set(crate::plugins::FighterFrameSet::Movement)
                .run_if(crate::plugins::simulation_frame)
                .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
        )
        .add_systems(
            actor_schedule,
            move_npc_fighters
                .after(crate::plugins::movement_plugin::move_local_fighter)
                .in_set(crate::plugins::FighterFrameSet::Locomotion)
                .run_if(crate::plugins::simulation_frame)
                .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
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
        &'static Fighter,
        &'static mut Transform,
        &'static mut MotionState,
        &'static mut FighterAir,
        &'static FighterHitReaction,
        Option<&'static AiIntent>,
        Option<&'static crate::components::fighter::FighterStatusEffects>,
        &'static mut SmState,
        &'static mut CombatState,
    ),
    (
        With<NpcFighter>,
        Without<LocalFighter>,
        Without<RemoteFighter>,
    ),
>;

pub(crate) fn npc_ballistic_air_root_motion(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    combat_resources: Option<Res<CombatResourceData>>,
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
    for (
        entity,
        fighter,
        mut transform,
        mut motion,
        mut air,
        hit_reaction,
        intent,
        status,
        mut sm,
        mut combat,
    ) in &mut npcs
    {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        if air.dodge_owns_motion_tick || hit_reaction.current.is_some() {
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
        if let Some(intent) = intent.filter(|intent| intent.locomotion_input != Vec2::ZERO) {
            super::movement_plugin::tick_air_input_drive(
                &mut air,
                dt,
                Vec3::new(intent.locomotion_input.x, 0.0, intent.locomotion_input.y),
                motion.facing,
                false,
                fighter_speed_factor(combat_resources.as_deref(), fighter, 1.0, status),
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
        if !air.air.airborne {
            super::movement_plugin::land_jump_state(&mut sm, &mut combat);
        }
    }
}

fn npc_ordinary_locomotion_allowed(sm: Option<&SmState>) -> bool {
    sm.is_none_or(|state| ordinary_locomotion_allowed(state, false))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_npc_ai(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    (time, frame_clock): (Res<Time>, Option<Res<crate::plugins::FrameClock>>),
    waypoints: Option<Res<WaypointGraph>>,
    (collider, contacts): (
        Option<Res<StageCollider>>,
        Option<Res<CharacterContactFrame>>,
    ),
    (mut script, scene_script): (
        Option<ResMut<StageMissionRuntime>>,
        Option<Res<crate::plugins::stage_plugin::StageScriptRuntime>>,
    ),
    net: Option<Res<crate::plugins::network_plugin::NetState>>,
    qi_values: Query<&crate::components::fighter::Qi>,
    switching: Query<(), With<crate::components::fighter::WeaponSwitchActive>>,
    concealment: Query<&crate::components::fighter::Concealment>,
    mut npcs: NpcAiQuery<'_, '_>,

    players: Query<(Entity, &Transform, &Health), With<LocalFighter>>,
    remotes: NpcRemoteTargets<'_, '_>,
    (cargos, network_flags): (
        Query<
            (
                Entity,
                &LocalEscortState,
                &Transform,
                Option<&crate::plugins::item_plugin::SceneRecordOrder>,
            ),
            Without<Fighter>,
        >,
        Query<
            (
                Entity,
                &Transform,
                Option<&crate::plugins::item_plugin::SceneRecordOrder>,
            ),
            (
                With<crate::plugins::stage_plugin::NetworkEscortFlag>,
                Without<Fighter>,
            ),
        >,
    ),
    props: Query<
        (
            Entity,
            &StageProp,
            &Transform,
            &Visibility,
            Option<&crate::plugins::item_plugin::SceneRecordOrder>,
            Option<&crate::plugins::item_plugin::StockSceneItemView>,
        ),
        Without<Fighter>,
    >,
    (mut commands, mut dialogue): (
        Commands,
        Option<ResMut<crate::plugins::ui_plugin::DialogueState>>,
    ),
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(11);
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let performance_dt =
        crate::plugins::simulation_wall_step(frame_clock.as_deref(), time.delta_secs());
    let peace = script.as_deref().map_or(0, StageMissionRuntime::peace);
    let attack_allowed = peace <= 0;
    let Some((player_entity, player_pos, player_alive)) = players
        .iter()
        .next()
        .map(|(entity, transform, health)| (entity, transform.translation, health.current > 0.0))
    else {
        return;
    };
    let prop_active: std::collections::HashMap<_, _> = scene_script
        .as_deref()
        .and_then(|script| script.script())
        .map(|script| {
            script
                .host()
                .instances()
                .iter()
                .map(|instance| {
                    (
                        instance.name.to_ascii_lowercase(),
                        instance
                            .props
                            .get("active")
                            .is_none_or(|value| value.as_f64() != 0.0),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let prop_available =
        |prop: &StageProp,
         visibility: &Visibility,
         view: Option<&crate::plugins::item_plugin::StockSceneItemView>| {
            view.is_none_or(|view| view.available)
                && npc_seekable_prop_active(
                    prop,
                    visibility,
                    prop_active.get(&prop.name.to_ascii_lowercase()).copied(),
                )
        };

    #[cfg(not(target_arch = "wasm32"))]
    let snapshot_perf = super::perf_plugin::Scope::new(19);
    let npc_positions: std::collections::HashMap<i64, Vec3> = npcs
        .iter()
        .filter_map(
            |(_, _, _, _, _, transform, _, _, _, _, _, npc, _, _, (_, _))| {
                npc.map(|npc| (npc.id, transform.translation))
            },
        )
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
        |(entity, _, health, _, _, transform, _, _, _, _, _, npc, _, _, (_, _))| {
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

    let concealed_ids: std::collections::HashSet<i64> = actor_entities
        .iter()
        .filter(|(_, entity)| concealment.get(**entity).is_ok_and(|c| c.remaining > 0.0))
        .map(|(id, _)| *id)
        .collect();
    let perceived_actors: Vec<_> = actors
        .iter()
        .copied()
        .filter(|actor| !concealed_ids.contains(&actor.id))
        .collect();
    #[cfg(not(target_arch = "wasm32"))]
    drop(snapshot_perf);
    for (
        entity,
        fighter,
        mut health,
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
        _pose,
        (hit_reaction, stock_brain),
    ) in &mut npcs
    {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        let stock_brain = stock_brain.is_some();
        patrol.script_tick_due = false;
        let mut base_behavior = mission_npc
            .as_deref()
            .and_then(|npc| npc.forced_behavior)
            .or(stock_brain.then_some(BehaviorMode::Run));
        let base_think = mission_npc
            .as_deref()
            .map_or(if stock_brain { STOCK_NPC_THINK } else { 100.0 }, |npc| {
                npc.think
            });
        let base_view = mission_npc
            .as_deref()
            .map_or(if stock_brain { STOCK_NPC_VIEW } else { 400.0 }, |npc| {
                npc.view.max(0.0)
            });

        intent.locomotion_input = Vec2::ZERO;
        if !air.air.airborne || health.current <= 0.0 {
            intent.navigation_jump_target = None;
            patrol.airborne_heading = None;
            patrol.airborne_input = Vec2::Y;
        }
        intent.want_qi_burst = false;
        intent.want_weapon_switch = false;
        if let Some(performance) = performance.as_deref_mut() {
            performance.ai_jump = None;
        }

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
            patrol.perception = default();
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
        patrol.advance_navigation(npc_pos, motion.velocity);
        let mut route_recovery = obstacle_avoidance::FollowRecovery::Normal;
        if patrol.avoidance.is_none()
            && (patrol.navigation.in_progress() || patrol.patrol_navigation.in_progress())
        {
            let mut rng = patrol.rng;
            route_recovery = patrol.collision_clock.follow_recovery(
                obstacle_avoidance::ObstacleSpace {
                    position: npc_pos,
                    facing: motion.facing,
                    mesh: collider.as_deref().and_then(|c| c.mesh.as_ref()),
                },
                || (next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32,
            );
            patrol.rng = rng;
            match route_recovery {
                obstacle_avoidance::FollowRecovery::AvoidObstacle => {
                    let heading = patrol.path_heading.unwrap_or(motion.facing);
                    patrol.stop_navigation();
                    patrol.avoidance = Some(ObstacleCallback {
                        ticks: -1,
                        desired_yaw: heading,
                        forward: false,
                    });
                }
                obstacle_avoidance::FollowRecovery::StrongJump
                | obstacle_avoidance::FollowRecovery::WeakJump => {
                    if let Some(performance) = performance.as_deref_mut() {
                        performance.ai_jump =
                            Some(route_recovery == obstacle_avoidance::FollowRecovery::StrongJump);
                    }
                }
                _ => {}
            }
        }

        let blocked = sm_state
            .as_deref()
            .is_none_or(|state| !matches!(state.current, FighterState::Down | FighterState::Fake))
            && contacts
                .as_deref()
                .is_some_and(|contacts| contacts.last_mover_scene_contact(entity));
        patrol.collision_clock.record_previous_mover(blocked, dt);
        let damaged = patrol
            .sampled_hp
            .is_some_and(|previous| health.current < previous);

        if patrol.sampled_hp.is_none() {
            patrol.sampled_hp = Some(health.current);
        }

        patrol.think_remaining -= dt;
        let thought_tick = patrol.think_remaining <= 0.0;
        let mut performing = performance
            .as_deref()
            .is_some_and(|p| !p.queue.is_empty() || p.pause_remaining > 0.0);
        patrol.script_tick_due = if performing {
            performance
                .as_deref()
                .is_some_and(|p| p.clock.pulse_due(performance_dt))
        } else {
            thought_tick
        };
        let stock_init_tick = stock_brain && thought_tick && patrol.first_update_pending;
        if thought_tick && patrol.first_update_pending {
            patrol.first_update_pending = false;
            patrol.reset_behavior_state();
            let id = mission_npc
                .as_deref()
                .map_or(-1 - i64::from(entity.index_u32()), |npc| npc.id);
            patrol.schedule_reset_think(base_think, id);
            if let Some(npc) = mission_npc.as_deref_mut() {
                npc.forced_behavior = None;
                npc.behavior_args.clear();
                npc.attack_target_slot = None;
                npc.attack_target_remaining = 0;
            }
            let loaded_hp = script
                .as_deref()
                .and_then(|s| s.script())
                .and_then(|runtime| runtime.npc_initial_hp(id))
                .or_else(|| stock_brain.then_some(health.max));
            if let Some(hp) = loaded_hp {
                health.current = hp;
                health.max = hp;
            }
            if let Some(runtime) = script.as_deref_mut().and_then(|s| s.script_mut()) {
                runtime.reset_npc_init(id);
                if let Some(hp) = loaded_hp {
                    let team = mission_npc
                        .as_deref()
                        .map_or(0.0, |npc| f64::from(npc.team));
                    runtime
                        .host_mut()
                        .sync_fighter(id, f64::from(hp), f64::from(hp), team);
                }
            }
            base_behavior = None;
            if let Some(performance) = performance.as_deref_mut() {
                *performance = default();
            }
            patrol.script_tick_due = true;
            performing = false;
            *intent = default();
        }
        if stock_brain && (patrol.first_update_pending || stock_init_tick) {
            if stock_init_tick {
                let id = -1 - i64::from(entity.index_u32());
                patrol.schedule_reset_think(base_think, id);
                patrol.finish_callback_install();
            }
            *intent = default();
            motion.movement_mode = meteor_core::MovementMode::Idle;
            continue;
        }
        if thought_tick {
            let mode = if patrol.temporary_target.is_some() {
                Some(BehaviorMode::Dodge)
            } else {
                base_behavior
            };
            let delay = if patrol.avoidance.is_some() {
                obstacle_avoidance::AVOID_CALLBACK_DELAY
            } else if patrol.getitem_goal.is_some() {
                0.3
            } else if (intent.enemy_id.is_some() || patrol.perception.passive)
                && !matches!(mode, Some(BehaviorMode::Idle | BehaviorMode::Dodge))
            {
                patrol.perception.callback_delay()
            } else {
                npc_script_callback_delay(mode)
            };
            patrol.think_remaining = meteor_ai::combat::decision_interval(base_think, delay);
        }
        let init_id = mission_npc
            .as_deref()
            .filter(|npc| npc.forced_behavior.is_none())
            .map(|npc| npc.id)
            .filter(|id| {
                script
                    .as_deref()
                    .and_then(|s| s.script())
                    .is_some_and(|runtime| runtime.npc_init_pending(*id))
            });
        if let Some(id) = init_id.filter(|_| !performing && patrol.avoidance.is_none()) {
            if thought_tick {
                if let Some(runtime) = script.as_deref_mut().and_then(|s| s.script_mut()) {
                    if let Err(error) = runtime.run_npc_init(id) {
                        warn!("NPC {id} OnStart: {error}");
                    }
                }
                patrol.think_remaining =
                    meteor_ai::combat::decision_interval(base_think, patrol.init_delay);
            }
            intent.move_dir = Vec2::ZERO;
            intent.want_attack = false;
            intent.want_guard = false;
            intent.dodge_burst = None;
            motion.movement_mode = meteor_core::MovementMode::Idle;
            continue;
        }
        if switching.contains(entity) {
            intent.want_attack = false;
            intent.want_guard = false;
            intent.want_taunt = false;
            intent.dodge_burst = None;
            intent.move_dir = Vec2::ZERO;
            continue;
        }
        let can_react = behavior_scans_enemies(base_behavior)
            && patrol.avoidance.is_none()
            && !patrol.perception.in_combat()
            && patrol.temporary_target.is_none()
            && patrol.getitem_goal.is_none()
            && performance.as_deref().is_none_or(|performance| {
                performance.pause_remaining <= 0.0 && performance.queue.is_empty()
            });
        if thought_tick && can_react {
            patrol.sampled_hp = Some(health.current);
            let enemy = intent
                .enemy_id
                .and_then(|id| actors.iter().find(|actor| actor.id == id && actor.alive));
            if damaged {
                if let Some(enemy) = enemy {
                    let dodge_roll = (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
                    if ((dodge_roll % 100) as f32)
                        < combat_profile.map_or(0.0, |profile| profile.dodge)
                    {
                        patrol.prepare_behavior_change(
                            mission_npc.as_deref(),
                            &intent,
                            Some(BehaviorMode::Dodge),
                        );
                        patrol.finish_callback_install();
                        if let Some(npc) = mission_npc.as_deref_mut() {
                            npc.forced_behavior = Some(BehaviorMode::Dodge);
                            npc.behavior_args =
                                vec![meteor_scene::pst_runtime::PstValue::Num(enemy.id as f64)];
                        } else {
                            patrol.temporary_target = Some(enemy.id);
                        }
                        *intent = default();
                        continue;
                    }
                    let qi = qi_values.get(entity).map_or(0.0, |qi| qi.current);
                    let distance = (enemy.position - npc_pos).length();
                    let roll = if qi > 60.0 && distance < 80.0 * meteor_core::MODEL_SCALE {
                        (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32
                    } else {
                        1
                    };
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
                        intent.attack_cooldown = npc_think_interval(base_think);
                        continue;
                    }
                }
                let roll = (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
                let spare = fighter.weapon_slots[1 - usize::from(fighter.weapon_slot.min(1))];
                if roll % 5 == 0 && spare != 0 {
                    patrol.last_weapon_switch_time = Some(crate::plugins::simulation_now(
                        frame_clock.as_deref(),
                        time.elapsed_secs_f64(),
                    ));
                    let delay = meteor_ai::combat::decision_interval(base_think, 0.6);
                    patrol.think_remaining = delay;
                    *intent = default();
                    intent.want_weapon_switch = true;
                    intent.attack_cooldown = delay;
                    if let Some(performance) = performance.as_deref_mut() {
                        performance.guard_remaining = 0.0;
                    }
                    continue;
                }
                let guard_roll = (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
                intent.want_guard = ((guard_roll % 100) as f32)
                    < combat_profile.map_or(0.0, |p| p.guard)
                    && !meteor_ai::combat::weapon_tactics(fighter.weapon).ranged;
                intent.want_attack = false;
                intent.move_dir = Vec2::ZERO;
                intent.dodge_burst = None;
                if intent.want_guard {
                    if let Some(performance) = performance.as_deref_mut() {
                        performance.guard_remaining = patrol.think_remaining;
                    }
                }
                if !intent.want_guard {
                    if let Some(performance) = performance.as_deref_mut() {
                        performance.guard_remaining = 0.0;
                    }
                    let following = mission_npc
                        .as_deref()
                        .is_some_and(|npc| npc.forced_behavior == Some(BehaviorMode::Follow));
                    if enemy.is_none() && !following {
                        let roll = (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
                        if ((roll % 100) as f32) < 2.0 * combat_profile.map_or(0.0, |p| p.dodge) {
                            patrol.perception.start_search();
                        }
                    }
                }
                continue;
            }
        }
        let perform_reaction_ready = performance.as_deref().is_some_and(|performance| {
            performance.queue.front().is_some_and(|command| {
                sm_state
                    .as_deref()
                    .is_some_and(|state| match command.action.as_str() {
                        "guard" => matches!(
                            state.current,
                            FighterState::Guard | FighterState::Down | FighterState::Fake
                        ),
                        "aggress" | "attack" | "jump" | "crouch" | "pause" => {
                            matches!(state.current, FighterState::Down | FighterState::Fake)
                        }
                        "use" | "faceto" => true,
                        "say" => matches!(
                            state.current,
                            FighterState::Name
                                | FighterState::Down
                                | FighterState::Fake
                                | FighterState::Guard
                                | FighterState::BeHit
                        ),
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
                intent.attack_cooldown = npc_think_interval(base_think);
            }
            intent.move_dir = Vec2::ZERO;
            intent.want_attack = false;
            intent.want_guard = false;
            intent.dodge_burst = None;
            intent.locomotion_input = Vec2::ZERO;
            intent.attack_directions.clear();
            if thought_tick
                && performance
                    .as_deref()
                    .is_none_or(|p| p.pause_remaining.is_finite())
                && sm_state
                    .as_deref()
                    .is_some_and(|state| state.current == FighterState::Down)
                && hit_reaction.current.as_ref().is_some_and(|reaction| {
                    reaction.kind == meteor_combat::hit_reaction::HitReactKind::Down
                })
            {
                let roll = (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
                queue_npc_down_recovery(
                    &mut intent,
                    roll,
                    attack_allowed,
                    motion.facing,
                    npc_pos,
                    collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
                );
            }
            motion.movement_mode = if air.air.airborne {
                meteor_core::MovementMode::Airborne
            } else {
                meteor_core::MovementMode::Idle
            };
            continue;
        }

        let mut down_recovery_attack = false;
        if thought_tick
            && hit_reaction.current.as_ref().is_some_and(|reaction| {
                reaction.kind == meteor_combat::hit_reaction::HitReactKind::Down
            })
            && sm_state
                .as_deref()
                .is_some_and(|state| state.current == FighterState::Down)
            && performance
                .as_deref()
                .is_none_or(|p| p.pause_remaining.is_finite())
        {
            let roll = npc_roll(&mut patrol);
            queue_npc_down_recovery(
                &mut intent,
                roll,
                attack_allowed,
                motion.facing,
                npc_pos,
                collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
            );
            down_recovery_attack = intent.want_attack;
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

        let faceto_target = performance
            .queue
            .front()
            .filter(|cmd| cmd.action == "faceto")
            .and_then(|cmd| {
                let id = cmd.args.first().map_or(0, |value| value.as_f64() as i64);
                if id == 0 {
                    Some(player_pos)
                } else {
                    npc_positions.get(&id).copied()
                }
            });
        if let Some(target) = faceto_target {
            patrol.idle_facing = yaw_towards(target - npc_pos);
        }
        if !performance.queue.is_empty() && npc_can_turn(sm_state.as_deref(), air.air.airborne) {
            if let Some(yaw) = patrol.idle_facing {
                motion.facing = npc_combat_turn(motion.facing, yaw, dt);
            }
        }

        if !performance.queue.is_empty()
            && performance.guard_remaining <= 0.0
            && performance.clock.pulse_due(performance_dt)
        {
            if intent.enemy_id.is_some_and(|id| {
                !perceived_actors
                    .iter()
                    .any(|actor| actor.id == id && actor.alive)
            }) {
                intent.enemy_id = None;
                intent.enemy_entity = None;
            }
            if intent.enemy_id.is_none() {
                let scan = perception::Scan {
                    peace,
                    self_id: mission_npc
                        .as_deref()
                        .map_or(-1 - i64::from(entity.index_u32()), |npc| npc.id),
                    team: crate::plugins::combat_plugin::fighter_combat_team(
                        net,
                        false,
                        None,
                        mission_npc.as_deref(),
                    ) as u8,
                    origin: npc_pos,
                    forward: Vec3::new(motion.facing.sin(), 0.0, motion.facing.cos()),
                    center: npc_pos,
                    radius: (3000.0
                        + meteor_ai::combat::weapon_tactics(fighter.weapon).attack_range * 0.5)
                        * meteor_core::MODEL_SCALE,
                };
                intent.enemy_id = perception::select_hostile(
                    scan,
                    &perceived_actors,
                    || npc_roll(&mut patrol),
                    |actor| {
                        npc_target_visible(
                            npc_pos,
                            actor.position,
                            fighter.weapon,
                            base_view * meteor_core::MODEL_SCALE,
                            collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
                        )
                    },
                )
                .map(|actor| actor.id);
                intent.enemy_entity = intent
                    .enemy_id
                    .and_then(|id| actor_entities.get(&id).copied());
            }
        }
        let missing_faceto = performance
            .queue
            .front()
            .is_some_and(|cmd| cmd.action == "faceto")
            && faceto_target.is_none();
        let next = if performance.guard_remaining > 0.0 {
            None
        } else {
            performance.clock.next_npc(
                &mut performance.queue,
                performance_dt,
                missing_faceto
                    || !air.air.airborne
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
                sm_state.as_deref().is_none_or(|state| {
                    matches!(
                        state.current,
                        FighterState::Name
                            | FighterState::Down
                            | FighterState::Fake
                            | FighterState::Guard
                            | FighterState::BeHit
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
                        patrol.idle_facing = yaw_towards(delta);

                        transform.rotation = Quat::IDENTITY;
                    }
                }
                "guard" => {}
                "aggress" => {
                    intent.want_taunt = true;
                }
                "attack" => {
                    perform_attack = attack_allowed;
                    let category = (npc_roll(&mut patrol) % 3) as usize;
                    queue_command(
                        &mut intent,
                        &mut patrol,
                        fighter.weapon,
                        category,
                        air.air.airborne,
                        attack_allowed,
                    );
                }
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
            intent.want_attack = perform_attack || down_recovery_attack;
            motion.movement_mode = if performance.crouching {
                meteor_core::MovementMode::Crouch
            } else {
                meteor_core::MovementMode::Idle
            };
            continue;
        }
        if let Some(mut avoidance) = patrol.avoidance.take() {
            let callback_facing = motion.facing;
            if npc_can_turn(sm_state.as_deref(), air.air.airborne) {
                motion.facing = npc_combat_turn(motion.facing, avoidance.desired_yaw, dt);
            }
            let held_forward = avoidance.forward;
            if thought_tick {
                let step = obstacle_avoidance::avoid_obstacle_tick(
                    obstacle_avoidance::ObstacleSpace {
                        position: npc_pos,
                        facing: callback_facing,
                        mesh: collider.as_deref().and_then(|c| c.mesh.as_ref()),
                    },
                    avoidance.desired_yaw,
                    avoidance.ticks,
                    || npc_roll(&mut patrol),
                );
                avoidance.ticks = avoidance.ticks.wrapping_add(1);
                avoidance.desired_yaw = step.desired_yaw;
                avoidance.forward = step.forward;
                patrol.think_remaining = meteor_ai::combat::decision_interval(
                    base_think,
                    obstacle_avoidance::AVOID_CALLBACK_DELAY,
                );
                if step.return_to_parent {
                    patrol.stop_navigation();
                    patrol.finish_callback_install();

                    if patrol.getitem_goal.is_some() {
                        patrol.getitem_ticks = 0;
                    }
                    patrol.wander_goal = None;
                    patrol.patrol_destination = None;
                    patrol.idle_facing = Some(avoidance.desired_yaw);
                    intent.move_dir = Vec2::ZERO;
                    intent.want_attack = false;
                    intent.want_guard = false;
                    intent.navigation_jump_target = None;
                    motion.movement_mode = meteor_core::MovementMode::Idle;
                    continue;
                }
            }
            intent.want_attack = false;
            intent.want_guard = false;
            intent.dodge_burst = None;
            intent.navigation_jump_target = None;
            intent.locomotion_input = if held_forward { Vec2::Y } else { Vec2::ZERO };
            intent.move_dir = decision_space::world_input(intent.locomotion_input, motion.facing);
            motion.movement_mode = if held_forward {
                meteor_core::MovementMode::Run
            } else {
                meteor_core::MovementMode::Idle
            };
            patrol.avoidance = Some(avoidance);
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
        let forced = if patrol.temporary_target.is_some() {
            Some(BehaviorMode::Dodge)
        } else {
            base_behavior
        };

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
        let configured_target = patrol.temporary_target.or(actor_ref).and_then(|id| {
            actors
                .iter()
                .find(|actor| {
                    actor.id == id
                        && actor.alive
                        && (forced == Some(BehaviorMode::Follow) || !concealed_ids.contains(&id))
                })
                .map(|actor| (id, actor.position))
        });

        if thought_tick && matches!(forced, Some(BehaviorMode::Dodge | BehaviorMode::Kill)) {
            let mut finish = configured_target.is_none();
            let mut exit_facing = None;
            if forced == Some(BehaviorMode::Dodge) {
                patrol.think_remaining = meteor_ai::combat::decision_interval(base_think, 0.4);
                if let Some((_, position)) = configured_target {
                    if patrol.behavior_ticks != 0 {
                        let roll = (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
                        finish |= roll % 8 == 0
                            || npc_pos.distance_squared(position)
                                > (250.0 * meteor_core::MODEL_SCALE).powi(2);
                    }
                    let view = base_view * meteor_core::MODEL_SCALE;
                    finish |= !npc_target_visible(
                        npc_pos,
                        position,
                        fighter.weapon,
                        view,
                        collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
                    );
                    if !finish {
                        patrol.flee_goal = match (
                            waypoints.as_deref(),
                            collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
                        ) {
                            (Some(graph), Some(mesh)) => {
                                concealed_escape_waypoint(graph, mesh, npc_pos, position, || {
                                    (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32
                                })
                            }
                            _ => None,
                        };
                        finish = patrol.flee_goal.is_none();
                    }
                    if finish {
                        exit_facing = yaw_towards(position - npc_pos);
                        if let Some(yaw) = exit_facing {
                            motion.facing = npc_combat_turn(motion.facing, yaw, dt);
                        }
                    }
                }
            }
            patrol.behavior_ticks = patrol.behavior_ticks.saturating_add(1);
            if finish {
                if patrol.return_to_parent(mission_npc.as_deref_mut(), &mut intent) {
                    if forced == Some(BehaviorMode::Dodge) {
                        patrol.idle_facing = exit_facing;
                    }
                    motion.movement_mode = meteor_core::MovementMode::Idle;
                    continue;
                }
            }
        }

        let previous_enemy = intent.enemy_id;
        if thought_tick && patrol.getitem_goal.is_none() {
            let id = mission_npc
                .as_deref()
                .map_or(-1 - i64::from(entity.index_u32()), |npc| npc.id);
            let team = crate::plugins::combat_plugin::fighter_combat_team(
                net,
                false,
                None,
                mission_npc.as_deref(),
            ) as u8;
            let view = base_view * meteor_core::MODEL_SCALE;
            let navigating = patrol.navigation.in_progress()
                || !patrol.travel_route.is_empty()
                || intent.navigation_jump_target.is_some();
            let context = perception::Context {
                peace,
                self_id: id,
                team,
                position: npc_pos,
                forward: Vec3::new(motion.facing.sin(), 0.0, motion.facing.cos()),
                weapon: fighter.weapon,
                mode: forced,
                configured: configured_target,
                navigating,
                look: combat_profile.map_or(0.0, |profile| profile.look),
            };
            let mut rng = patrol.rng;
            intent.enemy_id = patrol
                .perception
                .decide(
                    context,
                    previous_enemy,
                    &perceived_actors,
                    || (next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32,
                    |actor, multiplier| {
                        npc_target_visible(
                            npc_pos,
                            actor.position,
                            fighter.weapon,
                            view * multiplier,
                            collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
                        )
                    },
                )
                .map(|actor| actor.id);
            patrol.rng = rng;
            patrol.navigation_reset_pending |= patrol.perception.clear_navigation;
            patrol.think_remaining = meteor_ai::combat::decision_interval(
                base_think,
                if patrol.perception.passive
                    || (intent.enemy_id.is_some() && forced != Some(BehaviorMode::Idle))
                {
                    patrol.perception.callback_delay()
                } else {
                    npc_script_callback_delay(forced)
                },
            );
        }

        if patrol.navigation_reset_pending {
            patrol.stop_navigation();
            patrol.navigation_reset_pending = false;
        }

        if patrol.perception.parent_returned {
            intent.move_dir = Vec2::ZERO;
            intent.locomotion_input = Vec2::ZERO;
            intent.navigation_jump_target = None;
            intent.want_attack = false;
            intent.want_guard = false;
            intent.lock_target = None;
            intent.ranged_aim = None;
            motion.movement_mode = if air.air.airborne {
                meteor_core::MovementMode::Airborne
            } else {
                meteor_core::MovementMode::Idle
            };
            continue;
        }

        let enemy = intent.enemy_id.and_then(|id| {
            actors
                .iter()
                .find(|actor| actor.id == id && actor.alive && !concealed_ids.contains(&id))
                .copied()
        });
        intent.enemy_entity = enemy.and_then(|enemy| actor_entities.get(&enemy.id).copied());
        if enemy.is_none() {
            intent.enemy_id = None;
            intent.lock_target = None;
            intent.ranged_aim = None;
            intent.attack_directions.clear();
        }

        let enemy =
            enemy.filter(|_| !patrol.perception.parent_returned && behavior_scans_enemies(forced));
        if intent.enemy_id != previous_enemy {
            patrol.combat_spacing = None;
            patrol.combat_ticks = -1;

            intent.lock_target = None;
        }
        let ground_controllable = !air.air.airborne
            && sm_state.as_deref().is_none_or(|state| {
                matches!(
                    state.current,
                    FighterState::Name | FighterState::Down | FighterState::Fake
                )
            });
        let space = DecisionSpace {
            position: npc_pos,
            facing: motion.facing,
            ground_controllable,
            mesh: collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
        };
        let target_in_range = enemy.is_some_and(|enemy| {
            meteor_ai::combat::in_attack_range(
                fighter.weapon,
                (enemy.position - npc_pos).to_array(),
                ground_controllable,
            )
        });
        if enemy.is_none()
            || !target_in_range
            || meteor_ai::combat::weapon_tactics(fighter.weapon).ranged
        {
            intent.lock_target = None;
        }
        if thought_tick && enemy.is_some() && !patrol.perception.passive {
            patrol.combat_ticks = patrol.combat_ticks.saturating_add(1);
            let jump = combat_profile.map_or(0, |profile| profile.jump as i32);
            let yaw = yaw_towards(enemy.unwrap().position - npc_pos).unwrap_or(motion.facing);
            let forward = ((npc_roll(&mut patrol) % 100) as i32) < jump * 3 / 4;
            if forward && space.jump(&mut patrol, &mut intent, performance, yaw, true) {
                let delay = 0.1 + 0.05 * (npc_roll(&mut patrol) % 10) as f32;
                patrol.think_remaining = meteor_ai::combat::decision_interval(base_think, delay);
                intent.attack_cooldown = patrol.think_remaining;
                motion.movement_mode = meteor_core::MovementMode::Run;
                continue;
            }
            if target_in_range
                && enemy.is_some_and(|enemy| {
                    enemy.position.distance(npc_pos) < npc_minimum_range(fighter.weapon)
                })
                && forced != Some(BehaviorMode::Follow)
                && ((npc_roll(&mut patrol) % 100) as f32) < combat_profile.map_or(0.0, |p| p.dodge)
            {
                let id = enemy.unwrap().id;
                patrol.prepare_behavior_change(
                    mission_npc.as_deref(),
                    &intent,
                    Some(BehaviorMode::Dodge),
                );
                patrol.finish_callback_install();
                if let Some(npc) = mission_npc.as_deref_mut() {
                    npc.forced_behavior = Some(BehaviorMode::Dodge);
                    npc.behavior_args = vec![meteor_scene::pst_runtime::PstValue::Num(id as f64)];
                } else {
                    patrol.temporary_target = Some(id);
                }
                *intent = default();
                performance.guard_remaining = 0.0;
                performance.crouching = false;
                continue;
            }
        }
        if thought_tick
            && !air.air.airborne
            && sm_state
                .as_deref()
                .is_some_and(|sm| sm.current == FighterState::Name)
        {
            if let Some(enemy) = enemy {
                let delta = enemy.position - npc_pos;
                let elapsed = patrol
                    .last_weapon_switch_time
                    .map_or(f64::INFINITY, |last| {
                        crate::plugins::simulation_now(
                            frame_clock.as_deref(),
                            time.elapsed_secs_f64(),
                        ) - last
                    });
                let spare = fighter.weapon_slots[1 - usize::from(fighter.weapon_slot.min(1))];
                if meteor_ai::combat::wants_weapon_switch(
                    fighter.weapon,
                    spare,
                    delta.length() / meteor_core::MODEL_SCALE,
                    delta.y / meteor_core::MODEL_SCALE,
                    elapsed,
                    || (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32,
                ) {
                    patrol.last_weapon_switch_time = Some(crate::plugins::simulation_now(
                        frame_clock.as_deref(),
                        time.elapsed_secs_f64(),
                    ));
                    let delay = meteor_ai::combat::decision_interval(base_think, 0.6);
                    patrol.think_remaining = delay;
                    intent.attack_cooldown = delay;
                    performance.guard_remaining = 0.0;
                    intent.want_weapon_switch = true;
                    intent.want_attack = false;
                    intent.want_guard = false;
                    intent.dodge_burst = None;
                    intent.attack_directions.clear();
                    intent.lock_target = None;
                    intent.ranged_aim = None;
                    intent.move_dir = Vec2::ZERO;
                    motion.movement_mode = meteor_core::MovementMode::Idle;
                    continue;
                }
            }
        }
        let tactics = meteor_ai::combat::weapon_tactics(fighter.weapon);
        if thought_tick && target_in_range && enemy.is_some() && !patrol.perception.passive {
            let jump = combat_profile.map_or(0, |profile| profile.jump as i32);
            let yaw = yaw_towards(enemy.unwrap().position - npc_pos).unwrap_or(motion.facing);
            if ((npc_roll(&mut patrol) % 100) as i32) < jump / 4
                && space.jump(&mut patrol, &mut intent, performance, yaw, false)
            {
                motion.movement_mode = meteor_core::MovementMode::Run;
                continue;
            }
        }
        if enemy.is_some() {
            if !tactics.ranged && intent.lock_target.is_none() && target_in_range {
                intent.lock_target = intent.enemy_entity;
            }
        }
        if air.air.airborne && !attacking {
            if patrol.navigation_frame_step.arrived {
                intent.navigation_jump_target = None;
                intent.move_dir = Vec2::ZERO;
                intent.locomotion_input = Vec2::ZERO;
            }
            if patrol.navigation.in_progress()
                || patrol.patrol_navigation.in_progress()
                || intent.navigation_jump_target.is_some()
            {
                let target = patrol
                    .navigation
                    .steering_target()
                    .or_else(|| patrol.patrol_navigation.steering_target());
                if let Some(target) = target {
                    intent.navigation_jump_target = patrol.navigation_frame_step.jump_target;
                    let delta = Vec3::new(target.x - npc_pos.x, 0.0, target.z - npc_pos.z);
                    patrol.airborne_heading = yaw_towards(delta).or(patrol.airborne_heading);
                    intent.move_dir = delta.normalize_or_zero().xz();
                    if !thought_tick {
                        if let Some(yaw) = patrol.airborne_heading {
                            motion.facing = npc_combat_turn(motion.facing, yaw, dt);
                        }
                    }
                    patrol.airborne_input =
                        npc_locomotion_input_for_facing(intent.move_dir, motion.facing);
                } else {
                    let yaw = patrol.airborne_heading.unwrap_or(motion.facing);
                    if !thought_tick {
                        motion.facing = npc_combat_turn(motion.facing, yaw, dt);
                    }
                    intent.move_dir =
                        decision_space::world_input(patrol.airborne_input, motion.facing);
                }
                intent.locomotion_input = patrol.airborne_input;
                intent.want_attack = false;
                if !thought_tick {
                    continue;
                }
            } else if patrol.airborne_heading.is_some() {
                intent.locomotion_input = patrol.airborne_input;
                intent.move_dir = decision_space::world_input(patrol.airborne_input, motion.facing);
                if thought_tick && npc_roll(&mut patrol) % 4 == 0 {
                    patrol.airborne_input.y = 0.0;
                    intent.locomotion_input = patrol.airborne_input;
                }
                if !thought_tick {
                    continue;
                }
            }
        }
        if (attacking || (air.air.airborne && enemy.is_some() && target_in_range))
            && forced != Some(BehaviorMode::AttackTarget)
        {
            if !air.air.airborne {
                intent.move_dir = Vec2::ZERO;
            }
            intent.want_attack = perform_attack;
            if enemy.is_some_and(|enemy| {
                let delta = enemy.position - npc_pos;

                meteor_ai::combat::in_attack_range(
                    fighter.weapon,
                    delta.to_array(),
                    ground_controllable,
                ) && (!tactics.ranged
                    || yaw_towards(delta).is_none_or(|yaw| {
                        npc_facing_error(motion.facing, yaw).abs()
                            <= tactics.angle_degrees.to_radians()
                    }))
            }) && thought_tick
                && intent.attack_cooldown <= 0.0
            {
                intent.attack_cooldown = npc_think_interval(base_think);
                queue_npc_opening_action(
                    &mut intent,
                    performance,
                    combat_profile,
                    &mut patrol,
                    fighter.weapon,
                    air.air.airborne,
                    attack_allowed,
                    space,
                );
                if let Some(enemy) = enemy {
                    aim_npc_attack(
                        &mut intent,
                        &mut patrol,
                        fighter,
                        combat_profile,
                        npc_pos,
                        enemy.position,
                        motion.facing,
                    );
                }

                intent.want_guard = false;
                intent.dodge_burst = None;
                performance.guard_remaining = 0.0;
                performance.crouching = false;
            }
            continue;
        }
        let configured_position = configured_target.map(|(_, position)| position);
        let player_is_hostile = mission_npc.as_deref().is_none_or(|npc| {
            (meteor_combat::hit_resolve::team_mask(i32::from(player_team))
                & meteor_combat::hit_resolve::team_mask(i32::from(npc.team)))
                == 0
        });
        let live_player = (player_alive && player_is_hostile && !concealed_ids.contains(&0))
            .then_some(player_pos);
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

        let view_model = base_view;
        if thought_tick && patrol.getitem_goal.is_some() {
            let present = patrol.getitem_entity.is_some_and(|entity| {
                cargos
                    .get(entity)
                    .is_ok_and(|(_, state, _, _)| state.carrier.is_none())
                    || (network_flags.get(entity).is_ok()
                        && net
                            .escort_flag
                            .as_ref()
                            .is_some_and(|flag| flag.carrier_id.is_none()))
                    || props
                        .get(entity)
                        .is_ok_and(|(_, prop, _, visibility, _, view)| {
                            prop_available(prop, visibility, view)
                        })
            });
            let arrived = patrol
                .getitem_goal
                .is_some_and(|goal| getitem_arrived(npc_pos, goal));
            if !present || arrived || patrol.getitem_ticks > 10 {
                patrol.invalidate_navigation();
                intent.move_dir = Vec2::ZERO;
                intent.want_attack = false;
                motion.movement_mode = meteor_core::MovementMode::Idle;
                continue;
            }
        }
        let mode = if patrol.perception.passive {
            BehaviorMode::Idle
        } else if patrol.getitem_goal.is_some() {
            BehaviorMode::GetItem
        } else if enemy.is_some() {
            let mode = if thought_tick || patrol.combat_spacing.is_none() {
                if target_in_range {
                    BehaviorMode::Attack
                } else {
                    BehaviorMode::Follow
                }
            } else {
                patrol.combat_spacing.unwrap()
            };
            patrol.combat_spacing = Some(mode);
            mode
        } else {
            let mode = match forced {
                Some(BehaviorMode::Follow) if configured_target.is_none() => BehaviorMode::Run,
                Some(BehaviorMode::Follow)
                    if delta.length() <= 100.0 * meteor_core::MODEL_SCALE
                        && !patrol.navigation.in_progress() =>
                {
                    BehaviorMode::Idle
                }
                Some(BehaviorMode::Kill) if target_position.is_none() => BehaviorMode::Wait,

                Some(BehaviorMode::Kill) => BehaviorMode::Follow,
                Some(BehaviorMode::Dodge) => BehaviorMode::Dodge,
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
            };
            mode
        };
        let seek_mode = if enemy.is_some() {
            BehaviorMode::Attack
        } else if forced == Some(BehaviorMode::Follow) {
            BehaviorMode::Follow
        } else if forced == Some(BehaviorMode::Kill) {
            BehaviorMode::Kill
        } else {
            mode
        };
        let mode = if thought_tick
            && !patrol.perception.passive
            && mode != BehaviorMode::GetItem
            && behavior_may_seek_getitem(seek_mode)
        {
            try_start_getitem_seek(
                &mut patrol,
                combat_profile,
                if matches!(seek_mode, BehaviorMode::Attack | BehaviorMode::Follow) {
                    view_model.min(400.0)
                } else {
                    view_model
                },
                npc_pos,
                cargos
                    .iter()
                    .filter(|(_, state, transform, _)| {
                        state.carrier.is_none()
                            && getitem_visible(
                                npc_pos,
                                transform.translation,
                                collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
                            )
                    })
                    .map(|(entity, _, transform, order)| GetItemCandidate {
                        record: getitem_record(entity, order),
                        entity,
                        position: transform.translation,
                    })
                    .chain(props.iter().filter_map(
                        |(entity, prop, transform, visibility, order, view)| {
                            let position = prop.world_center(transform);
                            (prop_available(prop, visibility, view)
                                && getitem_visible(
                                    npc_pos,
                                    position,
                                    collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
                                ))
                            .then_some(GetItemCandidate {
                                record: getitem_record(entity, order),
                                entity,
                                position,
                            })
                        },
                    ))
                    .chain(
                        network_flags
                            .iter()
                            .filter_map(|(entity, transform, order)| {
                                (net.escort_flag
                                    .as_ref()
                                    .is_some_and(|flag| flag.carrier_id.is_none())
                                    && getitem_visible(
                                        npc_pos,
                                        transform.translation,
                                        collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
                                    ))
                                .then_some(GetItemCandidate {
                                    record: getitem_record(entity, order),
                                    entity,
                                    position: transform.translation,
                                })
                            }),
                    ),
            );
            if let Some(goal) = patrol.getitem_goal {
                let target_entity = patrol.getitem_entity;
                patrol.invalidate_navigation();
                let weapon = target_entity
                    .and_then(|entity| props.get(entity).ok())
                    .and_then(|(_, prop, _, _, _, _)| {
                        crate::plugins::item_plugin::weapon_from_model(&prop.model)
                    });
                let admission = if let Some(weapon) = weapon {
                    weapon_replacement::admit_weapon_getitem(
                        &mut patrol,
                        fighter,
                        weapon,
                        npc_pos,
                        goal,
                        waypoints.as_deref(),
                        collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
                        dt,
                        net.status != crate::plugins::network_plugin::NetStatus::InRoom
                            || npc_stock_drop_ready(net, entity),
                    )
                } else {
                    let step = patrol.travel_step(
                        waypoints.as_deref(),
                        collider.as_deref().and_then(|stage| stage.mesh.as_ref()),
                        npc_pos,
                        goal,
                        true,
                    );
                    if step.failed {
                        Err(())
                    } else {
                        Ok(None)
                    }
                };
                match admission {
                    Ok(drop) => {
                        patrol.getitem_direct = patrol.navigation.remaining_waypoints() == 0;
                        if let Some(mut drop) = drop {
                            drop.facing = motion.facing;
                            commands.entity(entity).insert(drop);
                        }
                        patrol.getitem_goal = Some(goal);
                        patrol.getitem_entity = target_entity;
                        intent.lock_target = None;
                        intent.want_guard = false;
                        performance.guard_remaining = 0.0;
                        performance.crouching = false;
                        patrol.think_remaining =
                            meteor_ai::combat::decision_interval(base_think, 0.3);
                        BehaviorMode::GetItem
                    }
                    Err(()) => {
                        patrol.invalidate_navigation();
                        mode
                    }
                }
            } else {
                mode
            }
        } else {
            mode
        };
        if !matches!(
            mode,
            BehaviorMode::Wait | BehaviorMode::Run | BehaviorMode::Patrol
        ) && !(mode == BehaviorMode::Idle
            && (forced == Some(BehaviorMode::Follow) || patrol.perception.passive))
        {
            patrol.idle_facing = None;
        }
        let mut desired_facing = patrol.perception.search_yaw.or_else(|| {
            patrol
                .perception
                .passive
                .then_some(patrol.idle_facing)
                .flatten()
        });
        match mode {
            BehaviorMode::Attack | BehaviorMode::AttackTarget => {
                let attack_target = mode == BehaviorMode::AttackTarget;
                let callback_tick = patrol.behavior_ticks;
                if attack_target && thought_tick {
                    patrol.behavior_ticks = callback_tick.wrapping_add(1);
                }

                let target = if attack_target {
                    if patrol.attack_target_point.is_none() {
                        patrol.attack_target_point = mission_npc
                            .as_deref()
                            .and_then(|npc| npc.attack_target_slot)
                            .and_then(|slot| {
                                script.as_deref()?.script()?.host().target_position(slot)
                            })
                            .map(|point| {
                                Vec3::new(point[0] as f32, point[1] as f32, point[2] as f32)
                            });
                    }
                    patrol.attack_target_point
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
                    let arrived = if attack_target {
                        flat.x.abs() < arrive
                            && flat.z.abs() < arrive
                            && delta.y.abs() < arrive + 30.0 * meteor_core::MODEL_SCALE
                    } else {
                        dist <= arrive
                    };
                    if !arrived && (attack_target || enemy.is_none()) {
                        let replan = thought_tick
                            && (!attack_target
                                || attack_goal_reconsider(
                                    callback_tick,
                                    patrol.navigation.remaining_waypoints(),
                                ));
                        let step = patrol.travel_step(
                            waypoints.as_deref(),
                            collider.as_deref().and_then(|c| c.mesh.as_ref()),
                            npc_pos,
                            target,
                            replan,
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
                        if attack_target && remaining <= 0 && thought_tick {
                            patrol.return_to_parent(mission_npc.as_deref_mut(), &mut intent);
                            intent.move_dir = Vec2::ZERO;
                            intent.want_attack = false;
                            motion.movement_mode = meteor_core::MovementMode::Idle;
                            continue;
                        }
                        intent.move_dir = Vec2::ZERO;
                        if let Some(yaw) = yaw_towards(flat)
                            .filter(|_| npc_can_turn(sm_state.as_deref(), air.air.airborne))
                        {
                            motion.facing = npc_combat_turn(motion.facing, yaw, dt);
                        }
                        let in_angle = yaw_towards(flat).is_none_or(|yaw| {
                            npc_facing_error(motion.facing, yaw).abs()
                                <= if attack_target {
                                    30.0_f32.to_radians()
                                } else {
                                    tactics.angle_degrees.to_radians()
                                }
                        });

                        let spare =
                            fighter.weapon_slots[1 - usize::from(fighter.weapon_slot.min(1))];
                        if attack_target
                            && remaining > 0
                            && thought_tick
                            && !in_angle
                            && tactics.ranged
                            && spare != 0
                        {
                            let delay = meteor_ai::combat::decision_interval(base_think, 0.6);
                            patrol.think_remaining = delay;
                            patrol.last_weapon_switch_time = Some(crate::plugins::simulation_now(
                                frame_clock.as_deref(),
                                time.elapsed_secs_f64(),
                            ));
                            *intent = default();
                            intent.want_weapon_switch = true;
                            intent.attack_cooldown = delay;
                            performance.guard_remaining = 0.0;
                            continue;
                        }
                        if (!attack_target || remaining > 0)
                            && thought_tick
                            && (attack_target || intent.attack_cooldown <= 0.0)
                            && (!attack_target || in_angle)
                        {
                            intent.attack_cooldown = if attack_target {
                                meteor_ai::combat::decision_interval(base_think, 0.3)
                            } else {
                                npc_think_interval(base_think)
                            };
                            if attack_target {
                                intent.want_attack = attack_allowed;
                                let category = (npc_roll(&mut patrol) % 3) as usize;

                                if attack_allowed {
                                    queue_command(
                                        &mut intent,
                                        &mut patrol,
                                        fighter.weapon,
                                        category,
                                        air.air.airborne,
                                        attack_allowed,
                                    );
                                }
                            } else {
                                queue_npc_opening_action(
                                    &mut intent,
                                    performance,
                                    combat_profile,
                                    &mut patrol,
                                    fighter.weapon,
                                    false,
                                    attack_allowed && in_angle,
                                    DecisionSpace {
                                        facing: motion.facing,
                                        ..space
                                    },
                                );
                            }
                            if attack_target {
                                if intent.want_attack && tactics.ranged {
                                    intent.ranged_aim =
                                        Some(Vec3::from_array(meteor_ai::combat::point_aim(
                                            fighter.weapon,
                                            npc_pos.to_array(),
                                            target.to_array(),
                                            motion.facing,
                                        )));
                                }
                            } else {
                                aim_npc_attack(
                                    &mut intent,
                                    &mut patrol,
                                    fighter,
                                    combat_profile,
                                    npc_pos,
                                    target,
                                    motion.facing,
                                );
                            }
                            if attack_target {
                                if let Some(npc) = mission_npc.as_deref_mut() {
                                    npc.attack_target_remaining =
                                        npc.attack_target_remaining.saturating_sub(1);
                                }
                            }
                        }
                        motion.movement_mode = meteor_core::MovementMode::Idle;
                        if !attack_target
                            && ground_controllable
                            && !intent.want_attack
                            && !intent.want_guard
                            && intent.dodge_burst.is_none()
                        {
                            intent.move_dir = decision_space::world_input(
                                Vec2::X * patrol.combat_strafe,
                                motion.facing,
                            );
                            if patrol.combat_strafe != 0.0 {
                                motion.movement_mode = meteor_core::MovementMode::Walk;
                            }
                            desired_facing = Some(motion.facing);
                        }
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
                let mut replan = false;
                if thought_tick {
                    let active = patrol.navigation.in_progress();
                    if enemy.is_some() {
                        if !active || npc_roll(&mut patrol) % 30 == 0 {
                            if target_position.distance(npc_pos) <= 120.0 * meteor_core::MODEL_SCALE
                                && npc_roll(&mut patrol) % 2 != 0
                            {
                                desired_facing = yaw_towards(target_position - npc_pos);
                            } else {
                                patrol.stop_navigation();
                                replan = ground_controllable;
                            }
                        }
                    } else {
                        let remaining = patrol.navigation.remaining_waypoints();
                        let reconsider =
                            !active || route_reconsider(true, remaining, npc_roll(&mut patrol));
                        if reconsider {
                            if forced != Some(BehaviorMode::Follow)
                                || target_position.distance(npc_pos)
                                    > 100.0 * meteor_core::MODEL_SCALE
                            {
                                replan = true;
                            } else if npc_roll(&mut patrol) % 6 == 0 {
                                desired_facing = yaw_towards(target_position - npc_pos);
                                patrol.idle_facing = desired_facing;
                            } else if ((npc_roll(&mut patrol) % 100) as f32)
                                < combat_profile.map_or(0.0, |profile| profile.look)
                            {
                                patrol.idle_facing = Some(
                                    (npc_roll(&mut patrol) % 360) as f32 * std::f32::consts::PI
                                        / 180.0,
                                );
                                desired_facing = patrol.idle_facing;
                            }
                        }
                        if forced == Some(BehaviorMode::Follow) {
                            patrol.behavior_ticks = patrol.behavior_ticks.wrapping_add(1);
                        }
                    }
                }
                let step = patrol.travel_step(
                    waypoints.as_deref(),
                    collider.as_deref().and_then(|c| c.mesh.as_ref()),
                    npc_pos,
                    target_position + Vec3::Y * (30.0 * meteor_core::MODEL_SCALE),
                    replan,
                );
                intent.move_dir = step.direction.xz();
                intent.navigation_jump_target = step.jump_target;
                intent.want_attack = false;
                motion.movement_mode = if patrol.navigation.in_progress() {
                    meteor_core::MovementMode::Run
                } else {
                    meteor_core::MovementMode::Idle
                };
                if replan && step.failed {
                    desired_facing = yaw_towards(flat);
                    if enemy.is_some() {
                        desired_facing = pursuit_recovery::recover(
                            &mut patrol,
                            &mut intent,
                            motion.facing,
                            target_position,
                            npc_pos,
                            combat_profile.map_or(0.0, |profile| profile.look),
                            base_think,
                        );
                    } else if forced == Some(BehaviorMode::Follow) {
                        patrol.think_remaining =
                            meteor_ai::combat::decision_interval(base_think, 0.5);
                        if npc_roll(&mut patrol) % 6 == 0
                            && desired_facing.is_none_or(|yaw| {
                                npc_facing_error(motion.facing, yaw).abs() <= 5.0_f32.to_radians()
                            })
                        {
                            intent.want_taunt = true;
                        }
                    }
                }
            }
            BehaviorMode::Patrol => {
                let mut replan = false;
                if thought_tick {
                    if !patrol.patrol_navigation.in_progress() || patrol.behavior_ticks % 40 == 20 {
                        if npc_roll(&mut patrol) % 10 == 0 && !patrol.route.is_empty() {
                            patrol.index %= patrol.route.len();
                            patrol.patrol_destination = Some(patrol.route[patrol.index]);
                            patrol.index += 1;
                            if npc_roll(&mut patrol) % 4 == 0 {
                                patrol.index += 1;
                            }
                            replan = true;
                        } else if ((npc_roll(&mut patrol) % 100) as f32)
                            < combat_profile.map_or(0.0, |profile| profile.look)
                        {
                            patrol.idle_facing = Some(
                                (npc_roll(&mut patrol) % 360) as f32 * std::f32::consts::PI / 180.0,
                            );
                            desired_facing = patrol.idle_facing;
                        }
                    }
                    patrol.behavior_ticks = patrol.behavior_ticks.wrapping_add(1);
                }
                let target = patrol
                    .patrol_destination
                    .and_then(|id| waypoints.as_deref()?.world_position(id));
                intent.want_attack = false;
                intent.move_dir = Vec2::ZERO;
                motion.movement_mode = meteor_core::MovementMode::Idle;
                if let Some(target) = target {
                    let step = patrol.patrol_step(
                        waypoints.as_deref(),
                        collider.as_deref().and_then(|c| c.mesh.as_ref()),
                        npc_pos,
                        Vec3::from_array(target),
                        replan,
                    );
                    intent.move_dir = step.direction.xz();
                    intent.navigation_jump_target = step.jump_target;
                    if patrol.patrol_navigation.in_progress() {
                        motion.movement_mode = meteor_core::MovementMode::Run;
                    }
                }
                if intent.move_dir == Vec2::ZERO {
                    desired_facing = desired_facing.or(patrol.idle_facing);
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
                if thought_tick && !patrol.perception.passive {
                    if let (Some(target), Some(chance)) = (configured_position, face_chance) {
                        if next_npc_rand(&mut patrol.rng) < chance {
                            patrol.idle_facing = yaw_towards(target - npc_pos);
                        }
                    }
                }
                if forced == Some(BehaviorMode::Follow) && !patrol.perception.passive {
                    desired_facing = patrol.idle_facing;
                }
            }
            BehaviorMode::GetItem => {
                if let Some(target) = patrol.getitem_goal {
                    let replan = thought_tick && patrol.getitem_ticks == 0;
                    let step = patrol.travel_step(
                        waypoints.as_deref(),
                        collider.as_deref().and_then(|c| c.mesh.as_ref()),
                        npc_pos,
                        target,
                        replan,
                    );
                    if thought_tick {
                        let finished = getitem_arrived(npc_pos, target)
                            || !patrol.navigation.in_progress()
                            || patrol.getitem_ticks > 10;
                        patrol.getitem_ticks = patrol.getitem_ticks.saturating_add(1);
                        if finished {
                            patrol.invalidate_navigation();
                            intent.move_dir = Vec2::ZERO;
                            intent.want_attack = false;
                            motion.movement_mode = meteor_core::MovementMode::Idle;
                            continue;
                        }
                    }
                    intent.move_dir = Vec2::new(step.direction.x, step.direction.z);
                    intent.navigation_jump_target = step.jump_target;
                    intent.want_attack = false;
                    motion.movement_mode = if step.direction.length_squared() > 0.01 {
                        meteor_core::MovementMode::Run
                    } else {
                        meteor_core::MovementMode::Idle
                    };
                    if !patrol.navigation.in_progress() {
                        desired_facing = yaw_towards(target - npc_pos);
                    }
                } else {
                    intent.move_dir = Vec2::ZERO;
                    intent.want_attack = false;
                    motion.movement_mode = meteor_core::MovementMode::Idle;
                }
            }
            BehaviorMode::Wait => {
                intent.move_dir = Vec2::ZERO;
                intent.want_attack = false;
                motion.movement_mode = meteor_core::MovementMode::Idle;

                if thought_tick
                    && next_npc_rand(&mut patrol.rng) * 100.0
                        < combat_profile.map_or(0.0, |p| p.look)
                {
                    patrol.idle_facing =
                        Some(next_npc_rand(&mut patrol.rng) * std::f32::consts::TAU);
                }
                desired_facing = patrol.idle_facing;
            }
            BehaviorMode::Run => {
                let mut replan = false;
                if thought_tick {
                    if !patrol.navigation.in_progress() || patrol.behavior_ticks % 40 == 20 {
                        if npc_roll(&mut patrol) % 7 == 0 {
                            if let Some(graph) = waypoints
                                .as_deref()
                                .filter(|graph| !graph.points.is_empty())
                            {
                                patrol.wander_goal =
                                    Some(npc_roll(&mut patrol) as usize % graph.points.len());
                                replan = true;
                            }
                        } else if ((npc_roll(&mut patrol) % 100) as f32)
                            < combat_profile.map_or(0.0, |profile| profile.look)
                        {
                            patrol.idle_facing = Some(
                                (npc_roll(&mut patrol) % 360) as f32 * std::f32::consts::PI / 180.0,
                            );
                            desired_facing = patrol.idle_facing;
                        }
                    }
                    patrol.behavior_ticks = patrol.behavior_ticks.wrapping_add(1);
                }
                intent.move_dir = Vec2::ZERO;
                intent.want_attack = false;
                motion.movement_mode = meteor_core::MovementMode::Idle;
                if let Some(target) = patrol
                    .wander_goal
                    .and_then(|id| waypoints.as_deref()?.world_position(id))
                {
                    let step = patrol.travel_step(
                        waypoints.as_deref(),
                        collider.as_deref().and_then(|c| c.mesh.as_ref()),
                        npc_pos,
                        Vec3::from_array(target),
                        replan,
                    );
                    intent.move_dir = step.direction.xz();
                    intent.navigation_jump_target = step.jump_target;
                    if patrol.navigation.in_progress() {
                        motion.movement_mode = meteor_core::MovementMode::Run;
                    }
                }
                if intent.move_dir == Vec2::ZERO {
                    desired_facing = desired_facing.or(patrol.idle_facing);
                }
            }
            BehaviorMode::Dodge => {
                let flee_target = patrol
                    .flee_goal
                    .and_then(|goal| waypoints.as_deref()?.world_position(goal))
                    .map(Vec3::from_array);
                let Some(destination) = flee_target else {
                    intent.move_dir = Vec2::ZERO;
                    intent.want_attack = false;
                    motion.movement_mode = meteor_core::MovementMode::Idle;
                    continue;
                };
                let step = patrol.travel_step(
                    waypoints.as_deref(),
                    collider.as_deref().and_then(|c| c.mesh.as_ref()),
                    npc_pos,
                    destination,
                    thought_tick,
                );
                if thought_tick
                    && step.failed
                    && patrol.return_to_parent(mission_npc.as_deref_mut(), &mut intent)
                {
                    motion.movement_mode = meteor_core::MovementMode::Idle;
                    continue;
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
        if !air.air.airborne && !npc_ordinary_locomotion_allowed(sm_state.as_deref()) {
            intent.navigation_jump_target = None;

            intent.move_dir = Vec2::ZERO;
            intent.locomotion_input = Vec2::ZERO;
            continue;
        }
        let following_path =
            patrol.navigation.in_progress() || patrol.patrol_navigation.in_progress();
        if following_path
            && intent.move_dir == Vec2::ZERO
            && matches!(
                mode,
                BehaviorMode::Follow
                    | BehaviorMode::Patrol
                    | BehaviorMode::Run
                    | BehaviorMode::Dodge
                    | BehaviorMode::GetItem
                    | BehaviorMode::AttackTarget
            )
            && patrol
                .navigation
                .steering_target()
                .or_else(|| patrol.patrol_navigation.steering_target())
                .is_some_and(|target| (target - npc_pos).xz().length_squared() <= 1e-10)
        {
            intent.move_dir = Vec2::new(motion.facing.sin(), motion.facing.cos());
        }

        let height_failed_held = !thought_tick
            && patrol.path_input != Vec2::ZERO
            && (patrol.navigation.failed() || patrol.patrol_navigation.failed());
        let held_path = height_failed_held
            || (following_path
                && route_recovery == obstacle_avoidance::FollowRecovery::KeepPathInputs);
        let obstacle_jump = following_path
            && matches!(
                route_recovery,
                obstacle_avoidance::FollowRecovery::StrongJump
                    | obstacle_avoidance::FollowRecovery::WeakJump
            );
        if held_path {
            desired_facing = patrol.path_heading;
            intent.move_dir = decision_space::world_input(patrol.path_input, motion.facing);
            if height_failed_held {
                motion.movement_mode = meteor_core::MovementMode::Run;
            }
        }
        let dir = Vec3::new(intent.move_dir.x, 0.0, intent.move_dir.y);
        if desired_facing.is_none() && intent.move_dir.length_squared() > 0.01 {
            desired_facing = yaw_towards(dir);
        }
        if let Some(target_yaw) = desired_facing {
            if intent.navigation_jump_target.is_some() {
                patrol.airborne_heading = Some(target_yaw);
            }
            motion.facing = npc_combat_turn(motion.facing, target_yaw, dt);

            if !held_path
                && !obstacle_jump
                && intent.move_dir.length_squared() > 0.01
                && matches!(
                    mode,
                    BehaviorMode::Follow
                        | BehaviorMode::Patrol
                        | BehaviorMode::Run
                        | BehaviorMode::Dodge
                        | BehaviorMode::GetItem
                        | BehaviorMode::AttackTarget
                )
            {
                let tolerance = if intent.navigation_jump_target.is_some() {
                    5.0_f32
                } else {
                    45.0_f32
                };
                if npc_facing_error(motion.facing, target_yaw).abs() <= tolerance.to_radians() {
                    intent.move_dir = Vec2::new(motion.facing.sin(), motion.facing.cos());
                } else {
                    intent.move_dir = Vec2::ZERO;
                    motion.movement_mode = meteor_core::MovementMode::Idle;
                }
            }
        }
        if performance.crouching {
            motion.movement_mode = meteor_core::MovementMode::Crouch;
        }
        intent.locomotion_input = npc_locomotion_input_for_facing(intent.move_dir, motion.facing);
        if obstacle_jump {
            intent.locomotion_input = Vec2::Y;
            intent.move_dir = decision_space::world_input(Vec2::Y, motion.facing);
            motion.movement_mode = meteor_core::MovementMode::Run;
        } else if held_path {
            intent.locomotion_input = patrol.path_input;
            intent.move_dir = decision_space::world_input(patrol.path_input, motion.facing);
        }
        if following_path {
            patrol.path_heading = desired_facing.or(patrol.path_heading);
            patrol.path_input = intent.locomotion_input;
        }

        if motion.movement_mode == meteor_core::MovementMode::Run
            && intent.locomotion_input != Vec2::Y
        {
            motion.movement_mode = meteor_core::MovementMode::Walk;
        }
    }
}

pub(crate) fn finish_npc_script_tick(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    mut script: Option<ResMut<StageMissionRuntime>>,
    mut npcs: Query<(Entity, &mut NpcPatrol, Option<&MissionNpc>), With<NpcFighter>>,
) {
    for (entity, mut patrol, npc) in &mut npcs {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        if std::mem::take(&mut patrol.script_tick_due) {
            if let (Some(npc), Some(runtime)) =
                (npc, script.as_deref_mut().and_then(|s| s.script_mut()))
            {
                if let Err(error) = runtime.update_npc_script(npc.id) {
                    warn!("NPC {} OnUpdate: {error}", npc.id);
                }
            }
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
            &'static mut FighterHitReaction,
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
    combat_resources: Option<Res<CombatResourceData>>,
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
        (ranged_gate, status_effects, performance, mut hit_reaction),
    ) in &mut npcs
    {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        if let Some(burst) = intent.dodge_burst.take() {
            if !wheel_vetoes_dodge_action(ranged_gate.wheel_out, burst) {
                let down_roll = matches!(pose.pose_id, 112 | 113 | 117);
                let roll = burst || down_roll;
                let dodge_pose = meteor_physics::locomotion::dodge_pose_for_input(
                    intent.dodge_dir.x,
                    intent.dodge_dir.y,
                    burst,
                    roll,
                    Some(meteor_combat::weapon_strategy::weapon_to_strategy(
                        fighter.weapon,
                    )),
                );

                let carry = if down_roll {
                    motion.velocity
                } else {
                    Vec3::ZERO
                };
                if down_roll {
                    hit_reaction.clear();
                }
                dodge
                    .state
                    .start(dodge_pose, motion.facing, roll, carry.x, carry.z);
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
        let ai_jump = performance.as_deref().and_then(|p| p.ai_jump);
        if (navigation_jump || scripted_jump || ai_jump.is_some())
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
                    navigation_jump || scripted_jump || ai_jump == Some(true),
                ) {
                    if let Some(mut performance) = performance {
                        performance.jump_requested = false;
                        performance.ai_jump = None;
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
            dir * speed
                * fighter_speed_factor(
                    combat_resources.as_deref(),
                    fighter,
                    1.0,
                    Some(status_effects),
                ),
            dt,
            true,
        );
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    include!("ai_plugin/init_tests.rs");
    include!("ai_plugin/basic_parity_tests.rs");
    include!("ai_plugin/river_audit_tests.rs");
    include!("ai_plugin/behavior_tests.rs");
    include!("ai_plugin/weapon_replacement_tests.rs");
    include!("ai_plugin/idle_facing_tests.rs");
    include!("ai_plugin/peace_tests.rs");
    include!("ai_plugin/review_r5_tests.rs");
    include!("ai_plugin/obstacle_integration_tests.rs");
    use super::*;
    use meteor_scene::stage_loader::parse_waypoint_graph;

    fn test_hostile(
        self_id: i64,
        team: u8,
        origin: Vec3,
        actors: &[AiActorSnapshot],
        visible: impl FnMut(AiActorSnapshot) -> bool,
    ) -> Option<AiActorSnapshot> {
        perception::select_hostile(
            perception::Scan {
                peace: 0,
                self_id,
                team,
                origin,
                forward: Vec3::Z,
                center: origin,
                radius: 1000.0,
            },
            actors,
            || 0,
            visible,
        )
    }

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

    include!("ai_plugin/campaign_regression_tests.rs");

    fn sn09_jump_case(start_id: usize, fps: u32) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let center = meteor_scene::stage_loader::get_stage_center("sn09", None);
        let mut graph = parse_waypoint_graph(
            &std::fs::read_to_string(root.join("Level/sn09.wp")).unwrap(),
            center,
        );
        assert_eq!(graph.link_types.get(&(7, 0)), Some(&1));
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn09/sn09.gmb")).unwrap())
                .unwrap();
        graph.navigation_bounds = super::super::npc_navigation::source_bounds_for_test(&gmdl);
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
            .init_resource::<CharacterContactFrame>()
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
                    super::super::character_contact_plugin::begin_character_contact_frame,
                    drive_npc_ai,
                    finish_npc_script_tick,
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

                    team: 1,
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
    fn height_failed_route_keeps_production_input_between_callbacks_until_stop() {
        let (mut app, npc) =
            perception_test_app(BehaviorMode::AttackTarget, Vec3::splat(1000.0), None);
        let graph = app.world().resource::<WaypointGraph>().clone();
        let target = Vec3::new(1.0, 3.6, 0.0);
        let active;
        {
            let mut patrol = app.world_mut().get_mut::<NpcPatrol>(npc).unwrap();
            assert!(
                !patrol
                    .travel_step(Some(&graph), None, Vec3::ZERO, target, true)
                    .failed
            );
            active = patrol.navigation.steering_target().unwrap();
            patrol.attack_target_point = Some(target);
            patrol.path_input = Vec2::Y;
            patrol.path_heading = Some(0.0);
            patrol.think_remaining = 100.0;
        }
        app.world_mut()
            .get_mut::<Transform>(npc)
            .unwrap()
            .translation = Vec3::new(active.x, -10.0, active.z);
        app.world_mut()
            .get_mut::<MotionState>(npc)
            .unwrap()
            .velocity = Vec3::X * 10.0;
        for _ in 0..2 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(16));
            app.update();
            let patrol = app.world().get::<NpcPatrol>(npc).unwrap();
            assert!(!patrol.navigation.in_progress());
            assert!(patrol.navigation.failed());
            let intent = app.world().get::<AiIntent>(npc).unwrap();
            assert_eq!(intent.locomotion_input, Vec2::Y);
            assert_eq!(intent.move_dir, Vec2::Y);
        }
        app.world_mut()
            .get_mut::<NpcPatrol>(npc)
            .unwrap()
            .stop_navigation();
        app.update();
        assert_eq!(
            app.world().get::<AiIntent>(npc).unwrap().locomotion_input,
            Vec2::ZERO
        );
    }

    #[test]
    fn path_completion_releases_held_inputs_but_height_failure_only_clears_route() {
        for height_failure in [false, true] {
            let graph = line_graph();
            let mut patrol = NpcPatrol::default();
            let goal = Vec3::new(1.0, 3.6, 0.0);
            assert!(
                !patrol
                    .travel_step(Some(&graph), None, Vec3::ZERO, goal, true)
                    .failed
            );
            let active = patrol.navigation.steering_target().unwrap();
            patrol.path_input = Vec2::Y;
            patrol.airborne_input = Vec2::Y;
            patrol.collision_clock = obstacle_avoidance::CollisionClock {
                blocked_seconds: 0.8,
                clear_seconds: 0.2,
            };
            let feet = Vec3::new(active.x, if height_failure { -10.0 } else { 0.0 }, active.z);
            patrol.advance_navigation(feet, Vec3::X * 10.0);
            assert!(!patrol.navigation.in_progress());
            assert_eq!(patrol.navigation_frame_step.arrived, !height_failure);
            assert_eq!(
                patrol.path_input,
                if height_failure { Vec2::Y } else { Vec2::ZERO }
            );
            assert_eq!(patrol.airborne_input, patrol.path_input);
            assert_eq!(
                patrol.collision_clock.blocked_seconds,
                if height_failure { 0.8 } else { 0.0 }
            );
        }
    }

    #[test]
    fn pst_disabled_waypoint_preserves_active_route_and_applies_to_next_request() {
        let mut graph = line_graph();
        for point in &mut graph.points {
            for axis in &mut point.position {
                *axis *= 60.0;
            }
        }
        let start = Vec3::from_array(graph.world_position(0).unwrap());
        let target = Vec3::from_array(graph.world_position(2).unwrap());
        let mut patrol = NpcPatrol::default();
        assert!(
            patrol
                .travel_step(Some(&graph), None, start, target, true)
                .failed
                == false
        );
        assert!(graph.set_enabled(1, false));
        assert!(
            patrol.navigation.in_progress(),
            "enabled writes do not clear an active route"
        );
        let held = patrol.travel_step(Some(&graph), None, start, target, false);
        assert!(!held.failed);
        let blocked = patrol.travel_step(Some(&graph), None, start, target, true);
        assert_eq!(blocked.direction, Vec3::ZERO);
        assert!(!blocked.arrived);
        assert!(graph.set_enabled(1, true));
        patrol.invalidate_navigation();
        assert!(
            patrol
                .travel_step(Some(&graph), None, start, target, true)
                .failed
                == false
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
    fn getitem_seek_range_uses_half_view_capped_at_2000_model_units() {
        assert!((getitem_seek_range_world(400.0) - 200.0 * meteor_core::MODEL_SCALE).abs() < 1e-4);
        assert!(
            (getitem_seek_range_world(5000.0) - 2000.0 * meteor_core::MODEL_SCALE).abs() < 1e-4
        );
    }

    #[test]
    fn getitem_scene_scan_rejects_occluded_props() {
        let npc = Vec3::ZERO;
        let item = Vec3::X * 4.0;
        assert!(getitem_visible(npc, item, Some(&NpcVisionMesh(None))));
        assert!(!getitem_visible(npc, item, Some(&NpcVisionMesh(Some(1.0)))));
    }

    #[test]
    fn getitem_skips_hidden_broken_and_recharging_props() {
        let mut prop = StageProp {
            name: "D_it01".into(),
            model: "item01".into(),
            center_offset: Vec3::ZERO,
            half_extents: Vec3::ONE,
            hit_count: 0,
            respawn_timer: 0.0,
            interactive: true,
            collision_enabled: true,
            breakable: false,
            broken: false,
        };
        assert!(npc_seekable_prop(&prop, &Visibility::Visible));
        assert!(!npc_seekable_prop(&prop, &Visibility::Hidden));
        prop.respawn_timer = 1.0;
        assert!(!npc_seekable_prop(&prop, &Visibility::Visible));
        prop.respawn_timer = 0.0;
        prop.broken = true;
        assert!(!npc_seekable_prop(&prop, &Visibility::Visible));
    }

    #[test]
    fn getitem_target_keeps_entity_and_respects_three_dimensional_view() {
        let mut world = World::new();
        let near = world.spawn_empty().id();
        let high = world.spawn_empty().id();
        let selected = first_getitem_target(
            Vec3::ZERO,
            5.0,
            None,
            [
                GetItemCandidate {
                    record: 0,
                    entity: high,
                    position: Vec3::new(1.0, 12.0, 0.0),
                },
                GetItemCandidate {
                    record: 1,
                    entity: near,
                    position: Vec3::new(3.0, 0.0, 0.0),
                },
            ],
        )
        .map(|candidate| (candidate.entity, candidate.position));
        assert_eq!(
            selected,
            Some((near, Vec3::new(3.0, 20.0 * meteor_core::MODEL_SCALE, 0.0)))
        );
        assert_eq!(
            first_getitem_target(
                Vec3::ZERO,
                2.0,
                None,
                [GetItemCandidate {
                    record: 0,
                    entity: near,
                    position: Vec3::X * 3.0
                }]
            )
            .map(|candidate| candidate.entity),
            None
        );
    }

    #[test]
    fn getitem_arrive_box_matches_exe_20_20_60_model_units() {
        let npc = Vec3::ZERO;
        let inside = Vec3::new(19.0 * meteor_core::MODEL_SCALE, 0.0, 0.0);
        let outside = Vec3::new(21.0 * meteor_core::MODEL_SCALE, 0.0, 0.0);
        assert!(getitem_arrived(npc, inside));
        assert!(!getitem_arrived(npc, outside));
    }

    #[test]
    fn parent_and_combat_callbacks_may_seek_getitem_but_idle_does_not() {
        assert!(behavior_may_seek_getitem(BehaviorMode::Wait));
        assert!(behavior_may_seek_getitem(BehaviorMode::Patrol));
        assert!(behavior_may_seek_getitem(BehaviorMode::Run));
        assert!(behavior_may_seek_getitem(BehaviorMode::Follow));
        assert!(!behavior_may_seek_getitem(BehaviorMode::Idle));
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
            test_hostile(1, 1, Vec3::ZERO, &actors, |_| true).map(|actor| actor.position),
            Some(Vec3::new(0.0, 0.0, 4.0))
        );

        assert_eq!(
            test_hostile(2, 2, Vec3::ZERO, &actors, |_| true).map(|actor| actor.position),
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
            test_hostile(1, 0, Vec3::ZERO, &unaffiliated, |_| true).map(|actor| actor.id),
            Some(2)
        );
    }

    #[test]
    fn npc_weapon_spacing_uses_original_tables() {
        let stock_profile = stock_npc_profile();
        assert_eq!(
            (
                stock_profile.attack1,
                stock_profile.attack2,
                stock_profile.attack3,
                stock_profile.guard,
                stock_profile.dodge,
                stock_profile.burst,
            ),
            (30.0, 25.0, 25.0, 20.0, 0.0, 30.0),
            "room/debug NPC decisions use the original stock PST weights"
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
    fn npc_perception_uses_original_height_range_and_occlusion() {
        let origin = Vec3::ZERO;
        let target = Vec3::new(0.0, 0.0, 5.0);
        assert!(npc_target_visible(
            origin,
            target,
            3,
            10.0,
            Some(&NpcVisionMesh(None))
        ));
        assert!(
            npc_target_visible(
                origin,
                Vec3::new(0.0, 12.1, 1.0),
                3,
                20.0,
                Some(&NpcVisionMesh(None)),
            ),
            "high ground inside View is not rejected by a separate height cap"
        );
        assert!(!npc_target_visible(
            origin,
            target,
            3,
            10.0,
            Some(&NpcVisionMesh(Some(1.0)))
        ));
        assert!(
            !npc_target_visible(origin, target, 3, 10.0, Some(&NpcVisionMesh(Some(4.8)))),
            "a wall near the target must not be ignored by Web's 0.45 tolerance"
        );
        assert!(!npc_target_visible(
            origin,
            target,
            3,
            4.0,
            Some(&NpcVisionMesh(None))
        ));

        let high = Vec3::new(0.0, 10.5, 1.0);
        assert!(npc_target_visible(
            origin,
            high,
            3,
            10.0,
            Some(&NpcVisionMesh(None))
        ));
        let actors = [AiActorSnapshot {
            id: 2,
            team: 2,
            position: high,
            alive: true,
        }];
        assert_eq!(
            test_hostile(1, 1, origin, &actors, |actor| {
                npc_target_visible(origin, actor.position, 3, 10.0, Some(&NpcVisionMesh(None)))
            })
            .map(|actor| actor.id),
            Some(2),
            "root-distance prefilter must not undo sight geometry"
        );
    }

    fn perception_wall(height: f32, z: f32) -> super::super::stage_collider::StageMeshCollider {
        let mut mesh = super::super::stage_collider::StageMeshCollider::default();
        mesh.append_world_triangles(
            &[
                [-10.0, -1.0, z],
                [10.0, -1.0, z],
                [10.0, height, z],
                [-10.0, height, z],
            ],
            &[0, 1, 2, 0, 2, 3],
        );
        mesh
    }

    #[test]
    fn npc_perception_sees_over_low_wall_but_not_solid_wall() {
        let target = Vec3::new(0.0, 0.0, 10.0);
        for weapon in [1, 3, 6] {
            assert!(
                npc_target_visible(
                    Vec3::ZERO,
                    target,
                    weapon,
                    20.0,
                    Some(&perception_wall(2.0, 5.0))
                ),
                "weapon {weapon} sees above a low wall"
            );
            assert!(
                !npc_target_visible(
                    Vec3::ZERO,
                    target,
                    weapon,
                    20.0,
                    Some(&perception_wall(8.0, 5.0))
                ),
                "weapon {weapon} cannot see through a wall"
            );
        }

        let wall = perception_wall(4.0, 1.0);
        assert!(!npc_target_visible(
            Vec3::ZERO,
            target,
            3,
            20.0,
            Some(&wall)
        ));
        assert!(npc_target_visible(Vec3::ZERO, target, 6, 20.0, Some(&wall)));
    }

    #[test]
    fn authored_long_route_is_advanced_once_before_callback_requests() {
        let graph = parse_waypoint_graph(
            "WayPoints 3\nPos 0 0 30\nSize 10\nLink 1\n1 0 600\nPos 0 -600 30\nSize 10\nLink 2\n0 0 600\n2 0 600\nPos 600 -600 30\nSize 10\nLink 1\n1 0 600\n",
            [0.0; 3],
        );
        let target = Vec3::from_array(graph.world_position(2).unwrap());
        let mut patrol = NpcPatrol::default();
        let first = patrol.travel_step(Some(&graph), None, Vec3::ZERO, target, true);
        assert!(!first.failed);
        let remaining = patrol.navigation.remaining_waypoints();

        patrol.travel_step(Some(&graph), None, Vec3::ZERO, target, false);
        assert_eq!(patrol.navigation.remaining_waypoints(), remaining);
        patrol.advance_navigation(Vec3::ZERO, Vec3::X * 12.0);
        patrol.advance_navigation(Vec3::ZERO, Vec3::X * 12.0);
        let step = patrol.travel_step(Some(&graph), None, Vec3::ZERO, target, false);
        assert!(step.direction.z > 0.999 && step.direction.x.abs() < 1e-5);
        let old_target = patrol.navigation.steering_target();
        patrol.travel_step(Some(&graph), None, Vec3::new(0.01, 0.0, 0.0), target, false);
        assert_eq!(patrol.navigation.steering_target(), old_target);
    }

    #[test]
    fn path_callback_cadence_distinguishes_rng_and_unsigned_tick() {
        assert!(route_reconsider(false, 3, 12));
        assert!(route_reconsider(true, 3, 26));
        assert!(!route_reconsider(true, 3, 25));
        assert!(attack_goal_reconsider(0, 3));
        assert!(attack_goal_reconsider(13, 3));
        assert!(!attack_goal_reconsider(-1, 0));
        assert!(attack_goal_reconsider(-1, 5), "u32::MAX is divisible by 15");
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
    fn npc_down_recovery_uses_original_direction_and_attack_choices() {
        for roll in 0..8 {
            let mut intent = AiIntent::default();
            queue_npc_down_recovery(&mut intent, roll, true, 0.0, Vec3::ZERO, None);
            assert_eq!(intent.want_attack, roll == 4);
            assert_eq!(intent.dodge_burst, (roll < 4).then_some(false));
            assert!(!intent.want_guard);
            if roll < 4 {
                assert_eq!(
                    intent.dodge_dir,
                    [Vec2::Y, Vec2::NEG_Y, Vec2::NEG_X, Vec2::X][roll as usize]
                );
            }
        }
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
        app.init_resource::<Time>()
            .insert_resource(fixture_navigation_graph());
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
            Transform::from_xyz(9.0, 0.9, 0.0),
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
        assert_eq!(
            intent.move_dir,
            Vec2::ZERO,
            "the NPC must turn before moving"
        );
        assert_eq!(
            intent.locomotion_input,
            Vec2::ZERO,
            "a 90-degree route turn must not slide sideways"
        );
        assert_eq!(intent.enemy_id, Some(0));
        assert_eq!(
            app.world().get::<MotionState>(npc).unwrap().movement_mode,
            meteor_core::MovementMode::Idle,
            "navigation waits until the heading is within 45 degrees"
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

    fn fixture_navigation_graph() -> WaypointGraph {
        let side = 13usize;
        let mut graph = WaypointGraph::default();
        for z in 0..side {
            for x in 0..side {
                let id = z * side + x;
                let mut links = Vec::new();
                for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let nx = x as i32 + dx;
                    let nz = z as i32 + dz;
                    if nx >= 0 && nz >= 0 && nx < side as i32 && nz < side as i32 {
                        links.push((
                            nz as usize * side + nx as usize,
                            40.0 / meteor_core::MODEL_SCALE,
                        ));
                    }
                }
                graph
                    .points
                    .push(meteor_scene::stage_loader::WaypointPoint {
                        id,
                        position: [
                            (x as f32 - 6.0) * 40.0 / meteor_core::MODEL_SCALE,
                            -(z as f32 - 6.0) * 40.0 / meteor_core::MODEL_SCALE,
                            30.0,
                        ],
                        size: 10.0,
                        links,
                    });
            }
        }
        graph
    }

    fn perception_test_app(mode: BehaviorMode, target: Vec3, wall: Option<f32>) -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(fixture_navigation_graph())
            .init_resource::<Time>()
            .add_systems(Update, drive_npc_ai);
        let player = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 3,
                    weapon_slots: [3, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health::default(),
                Transform::from_translation(target),
            ))
            .id();
        let npc = app
            .world_mut()
            .spawn((
                NpcFighter,
                Fighter {
                    character_id: 1,
                    player_slot: 1,
                    weapon: 3,
                    weapon_slots: [3, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health {
                    current: 100.0,
                    max: 100.0,
                },
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                FighterAir::default(),
                AiIntent::default(),
                Transform::default(),
                NpcPatrol::default(),
                MissionNpc {
                    id: 7,
                    name: "guard".into(),
                    team: 2,
                    in_scene_roster: true,
                    view: 400.0,
                    think: 0.0,
                    forced_behavior: Some(mode),
                    is_vip: false,
                    attack_target_slot: None,
                    behavior_args: vec![meteor_scene::pst_runtime::PstValue::Num(0.0)],
                    attack_target_remaining: 0,
                },
            ))
            .id();
        if let Some(height) = wall {
            app.insert_resource(StageCollider {
                mesh: Some(perception_wall(height, 5.0)),
            });
        }
        let _ = player;
        (app, npc)
    }

    #[test]
    fn production_ai_acquires_targets_over_low_walls_and_on_high_ground() {
        for (target, wall, visible) in [
            (Vec3::new(0.0, 0.0, 20.0), Some(2.0), true),
            (Vec3::new(0.0, 13.0, 20.0), None, true),
            (Vec3::new(0.0, 0.0, 20.0), Some(8.0), false),
            (Vec3::new(0.0, 0.0, 60.0), None, false),
            (Vec3::new(0.0, 0.0, -20.0), None, false),
            (Vec3::new(0.0, 0.0, -5.0), None, true),
        ] {
            let (mut app, npc) = perception_test_app(BehaviorMode::Wait, target, wall);
            app.update();
            assert_eq!(
                app.world().get::<AiIntent>(npc).unwrap().enemy_id,
                visible.then_some(0),
                "target={target:?}, wall={wall:?}"
            );
        }
    }

    #[test]
    fn production_kill_target_requires_sight_before_combat_and_reacquires() {
        let (mut app, npc) =
            perception_test_app(BehaviorMode::Kill, Vec3::new(0.0, 0.0, 30.0), Some(8.0));
        app.update();
        let intent = app.world().get::<AiIntent>(npc).unwrap();
        assert_eq!(
            intent.enemy_id, None,
            "configured target is not automatically visible"
        );
        assert!(!intent.want_attack);
        assert_eq!(intent.ranged_aim, None);

        app.world_mut().resource_mut::<StageCollider>().mesh = None;
        app.world_mut()
            .get_mut::<NpcPatrol>(npc)
            .unwrap()
            .think_remaining = 0.0;
        app.update();
        assert_eq!(app.world().get::<AiIntent>(npc).unwrap().enemy_id, Some(0));

        app.world_mut().resource_mut::<StageCollider>().mesh = Some(perception_wall(8.0, 5.0));
        app.world_mut()
            .get_mut::<NpcPatrol>(npc)
            .unwrap()
            .think_remaining = 0.0;
        app.update();
        let intent = app.world().get::<AiIntent>(npc).unwrap();
        assert_eq!(intent.enemy_id, None);
        assert!(!intent.want_attack);
    }

    #[test]
    fn production_follow_acquires_beyond_old_self_view_cap() {
        let (mut app, npc) =
            perception_test_app(BehaviorMode::Follow, Vec3::new(0.0, 0.0, 20.0), None);
        app.world_mut().get_mut::<MissionNpc>(npc).unwrap().team = 1;
        let mut hostile = app.world().get::<MissionNpc>(npc).unwrap().clone();
        hostile.id = 8;
        hostile.team = 2;
        hostile.forced_behavior = Some(BehaviorMode::Idle);
        let fighter = app.world().get::<Fighter>(npc).unwrap().clone();
        let motion = app.world().get::<MotionState>(npc).unwrap().clone();
        app.world_mut().spawn((
            NpcFighter,
            fighter,
            motion,
            Health::default(),
            FighterAir::default(),
            AiIntent::default(),
            NpcPatrol::default(),
            hostile,
            Transform::from_xyz(0.0, 0.0, 40.0),
        ));
        app.update();
        assert_eq!(app.world().get::<AiIntent>(npc).unwrap().enemy_id, Some(8));
    }

    #[test]
    fn production_search_holds_fire_and_resets_on_behavior_change() {
        let (mut app, npc) =
            perception_test_app(BehaviorMode::Wait, Vec3::new(0.0, 0.0, 35.0), None);
        app.update();
        assert_eq!(app.world().get::<AiIntent>(npc).unwrap().enemy_id, Some(0));
        app.insert_resource(StageCollider {
            mesh: Some(perception_wall(8.0, 5.0)),
        });
        {
            let mut patrol = app.world_mut().get_mut::<NpcPatrol>(npc).unwrap();
            patrol.think_remaining = 0.0;
            patrol.rng = (1..100)
                .find(|seed| {
                    let mut seed = *seed;
                    ((next_npc_rand(&mut seed) * 0x0100_0000 as f32) as u32) % 30 != 0
                })
                .unwrap();
        }
        app.update();
        assert!(app
            .world()
            .get::<NpcPatrol>(npc)
            .unwrap()
            .perception
            .searching());
        assert!(!app.world().get::<AiIntent>(npc).unwrap().want_attack);

        assert_eq!(
            app.world().get::<NpcPatrol>(npc).unwrap().think_remaining,
            2.5
        );
        app.world_mut()
            .get_mut::<NpcPatrol>(npc)
            .unwrap()
            .reset_behavior_state();
        assert!(!app
            .world()
            .get::<NpcPatrol>(npc)
            .unwrap()
            .perception
            .searching());
        assert!(
            !app.world()
                .get::<NpcPatrol>(npc)
                .unwrap()
                .perception
                .passive
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
                Transform::from_xyz(0.0, 0.9, 60.0),
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

        app.world_mut().get_mut::<NpcPatrol>(npc).unwrap().rng = (1..100)
            .find(|seed| {
                let mut seed = *seed;
                ((next_npc_rand(&mut seed) * 0x0100_0000 as f32) as u32) % 30 != 0
            })
            .unwrap();
        for _ in 0..42 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(50));
            app.update();
        }
        let think_remaining = app.world().get::<NpcPatrol>(npc).unwrap().think_remaining;
        assert_eq!(
            app.world().get::<AiIntent>(npc).unwrap().enemy_id,
            Some(0),
            "valid target stays stable across Think ticks; remaining={think_remaining}"
        );
        app.world_mut()
            .get_mut::<crate::components::fighter::Concealment>(player)
            .unwrap()
            .remaining = 60.0;
        app.update();
        let intent = app.world().get::<AiIntent>(npc).unwrap();
        assert_eq!(
            intent.enemy_id, None,
            "concealment invalidates between Think ticks"
        );
        assert_eq!(intent.lock_target, None);
        for _ in 0..42 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(50));
            app.update();
        }
        assert_eq!(app.world().get::<AiIntent>(npc).unwrap().enemy_id, Some(8));
        app.world_mut().get_mut::<Health>(other).unwrap().current = 0.0;
        app.update();
        assert_eq!(app.world().get::<AiIntent>(npc).unwrap().enemy_id, None);
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
                    action_id: None,
                    pose_id: 176,
                    frame: 581.0,
                    start: 570,
                    end: 581,
                    loop_start: 570,
                    loop_end: 581,
                    loops: false,
                    link_past_end: false,
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

    #[test]
    fn search_resamples_original_look_but_turns_with_original_speed_limit() {
        let (mut app, npc) =
            perception_test_app(BehaviorMode::Wait, Vec3::new(0.0, 0.0, 35.0), None);
        app.world_mut().entity_mut(npc).insert(NpcCombatProfile {
            look: 50.0,
            ..default()
        });
        app.world_mut().get_mut::<MissionNpc>(npc).unwrap().think = 100.0;
        app.update();
        app.insert_resource(StageCollider {
            mesh: Some(perception_wall(8.0, 5.0)),
        });
        app.world_mut()
            .get_mut::<NpcPatrol>(npc)
            .unwrap()
            .think_remaining = 0.0;

        let mut chosen: Option<f32> = None;
        let mut search_frames = 0;
        let mut changes = 0;
        let mut previous_facing = app.world().get::<MotionState>(npc).unwrap().facing;
        for _ in 0..300 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(16));
            app.update();
            let facing = app.world().get::<MotionState>(npc).unwrap().facing;
            assert!(
                npc_facing_error(previous_facing, facing).abs()
                    <= 500.0_f32.to_radians() * 0.016 + 1e-5
            );
            previous_facing = facing;
            let patrol = app.world().get::<NpcPatrol>(npc).unwrap();
            if patrol.perception.searching() {
                search_frames += 1;
                if let Some(yaw) = patrol.perception.search_yaw {
                    if chosen.is_some_and(|old| npc_facing_error(old, yaw).abs() > 1e-6) {
                        changes += 1;
                    }
                    chosen = Some(yaw);
                }
            }
        }
        assert!(changes > 1, "original search resamples Look every callback");
        assert!(search_frames > 0, "test must exercise the search state");
        assert!(chosen.is_some(), "look=50 must choose one search direction");
    }
}
