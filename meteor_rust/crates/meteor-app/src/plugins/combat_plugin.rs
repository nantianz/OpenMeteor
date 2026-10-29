use crate::app_state::{AppState, Settings};
use crate::components::fighter::{
    AiIntent, CombatState, DeathRespawn, EntryBlend, EntryBlendKind, Fighter, FighterHitReaction,
    GunAimState, HeadTrackState, Health, LockOnController, MatchEnd, MotionState, PoseRuntime, Qi,
    RangedFireGate, SmState, TargetHitClock, WeaponCollisionGeometry, WeaponForm,
    WeaponSwitchActive,
};
use crate::plugins::character_contact_plugin::{
    commit_fighter_position, resolve_fighter_xz, CharacterContactFrame,
};
use crate::plugins::character_plugin::{
    model_bones, model_dummies, CharacterAssets, CharacterAssetsSet, CommonAmb, FighterBone,
    FighterDummy, FighterModel, FighterSkeletonQuery,
};
use crate::plugins::input_plugin::{
    DesktopDodgeInput, DodgeGesture, LocalCombatInput, LocalCombatInputEdge, LocalCrouchInput,
    LocalGuardInput, LocalJumpInput,
};
use crate::plugins::movement_plugin::{
    directional_input, FighterAir, FighterAttackMotion, FighterDodge, FighterWall, COLLIDER_HEIGHT,
    COLLIDER_RADIUS,
};
use crate::plugins::network_plugin::{
    NetState, NetStatus, PendingNetActions, PendingRemoteHit, RemoteFighter, RemoteHitMotionWindow,
    RemoteTransformTarget,
};
use crate::plugins::stage_collider::StageCollider;
use crate::plugins::stage_plugin::{LocalFighter, MissionNpc, NpcCombatProfile, NpcPerformance};
use crate::{
    components::projectile::{PendingRangedHits, Projectile, ProjectileKind, RangedHitSource},
    plugins::camera_plugin::GameplayCamera,
};
use bevy::prelude::*;
use bevy::transform::helper::TransformHelper;
use meteor_anim::pose_controller::{advance_pose_loop_cursor, pose_playback_speed_at_frame};
use meteor_asset::res::{parse_meteor_res, CombatResourceTable, WeaponCombatStats};
use meteor_combat::combo_engine::{
    pick_act_successor, resolve_affordable_weapon_command, weapon_form_for_pose, Dir as CombatDir,
};
use meteor_combat::hit_reaction::{
    begin_be_hit, begin_death, begin_guard_hit, facing_bucket, getup_reaction_for_pose,
    pose_duration_seconds, tick_hit_react, HitReactKind, ReactionFighter,
};
use meteor_combat::hit_resolve::{
    guard_anger_gain, resolve_hit, sum_attack_power, team_mask, CombatStats, HitOptions,
};
use meteor_combat::hitbox::{
    attack_hit_obbs, body_hurt_obbs, bone_world_obb, normalized_bone_name, obbs_intersect,
    BoneLocalBox, Obb, WeaponCollisionPart,
};
use meteor_combat::lock_on::{
    locked_attack_facing_yaw, tracked_head_yaw, LockCandidate, TRACK_CURVE_EXIT_TURN_LIMIT,
    TRACK_TURN_LIMIT,
};
use meteor_combat::qi_burst::{
    can_qi_burst, QI_BURST_ACTION_ID, QI_BURST_ANGER_COST, QI_BURST_POSE_ID,
};
use meteor_combat::ranged_weapons::{
    bypasses_melee_hitboxes, ranged_attack_profile, ranged_target_box, segment_hits_box_distance,
    Point3 as RangedPoint3, RangedKind, RangedMode, DART_SPAWN_HEIGHT, DART_SPEED,
    GUN_SPAWN_HEIGHT, WHEEL_SPAWN_HEIGHT,
};
use meteor_combat::state_machine::{
    Action, FighterView, QueuedAttack, QueuedAttackCommit, StateMachine,
};
use meteor_combat::weapon_strategy::{
    weapon_to_strategy, WEAPON_SWITCH_POSE_AIR, WEAPON_SWITCH_POSE_GROUND,
};
use meteor_core::FighterState;
use meteor_network::modes::{evaluate_round_result, RoundMember};
use meteor_physics::carry_velocity::{
    bank_carry_velocity, clamp_composed_delta_3d, CarryVelocity, CARRY_MAX_SPEED,
};
use meteor_physics::jump::AirMotionMode;
use meteor_physics::locomotion::pose as L;
use meteor_physics::locomotion::{
    can_keep_locomotion_phase, guard_pose_for_weapon, is_jump_pose_id, walk_pose_for_input,
    WalkOptions,
};

#[cfg(test)]
const ATTACK_REACH: f32 = 1.7;

const NPC_SPAWN_OFFSET: f32 = 3.0;

pub(crate) const MISSION_SETTLE_SECONDS: f32 = 10.0;

const NINJA_BLADE_WEAPON_ID: u8 = 12;
const NINJA_BLADE_CARRY_SCALE: f32 = 0.67;

const GUN_AIM_READY_POSE: u16 = 212;
const GUN_AIM_EXEMPT_POSES: [u16; 2] = [338, 215];

fn gun_aim_intercepts(strategy_id: u8, aiming: bool, airborne: bool, entry_pose: u16) -> bool {
    strategy_id == meteor_physics::locomotion::FIREARM_STRATEGY
        && !aiming
        && !airborne
        && !GUN_AIM_EXEMPT_POSES.contains(&entry_pose)
}

fn alternate_weapon_slot(active_slot: u8, slots: [u8; 2]) -> Option<usize> {
    let target = other_weapon_slot(active_slot);
    (slots[target] != 0).then_some(target)
}

fn other_weapon_slot(active_slot: u8) -> usize {
    1 - usize::from(active_slot.min(1))
}

fn combat_dir(dir: meteor_input::command_parser::Dir) -> CombatDir {
    match dir {
        meteor_input::command_parser::Dir::Up => CombatDir::Up,
        meteor_input::command_parser::Dir::Down => CombatDir::Down,
        meteor_input::command_parser::Dir::Left => CombatDir::Left,
        meteor_input::command_parser::Dir::Right => CombatDir::Right,
    }
}

fn dodge_direction(dir: meteor_input::command_parser::Dir) -> Vec2 {
    use meteor_input::command_parser::Dir;
    match dir {
        Dir::Up => Vec2::Y,
        Dir::Down => Vec2::NEG_Y,
        Dir::Left => Vec2::NEG_X,
        Dir::Right => Vec2::X,
    }
}

fn dodge_roll_at_event(
    burst: bool,
    crouch_held: bool,
    gun_aiming: bool,
    state: FighterState,
) -> bool {
    burst || crouch_held || gun_aiming || matches!(state, FighterState::Fake | FighterState::Down)
}

fn publish_accepted_dodge(
    dodge: Option<&mut DesktopDodgeInput>,
    direction: Vec2,
    burst: bool,
    roll: bool,
) {
    let Some(dodge) = dodge else {
        return;
    };
    if dodge.pending.is_none() {
        dodge.pending = Some(DodgeGesture { direction, burst });
        dodge.pending_state_accepted = true;
        dodge.pending_roll = Some(roll);
    }
}

fn project_state_machine<F: FighterView>(sm: &StateMachine<F>) -> SmState {
    SmState {
        current: sm.current,
        previous: sm.previous,
        time_in_state: sm.time_in_state,
        locked: sm.locked,
        attack_stage: sm.attack_stage,
        attack_queued: sm.attack_queued.clone(),
        linked_recovery_pose_id: sm.linked_recovery_pose_id,
        exe_ground_mode_ticks: sm.exe_ground_mode_ticks,
    }
}

fn release_reaction_after_accepted_jump(
    reaction: &mut FighterHitReaction,
    accepted: bool,
    state: FighterState,
) -> bool {
    if accepted && state == FighterState::Jump {
        reaction.clear();
        true
    } else {
        false
    }
}

fn install_requested_getup_reaction(
    reaction: &mut FighterHitReaction,
    state: &mut SmState,
    pose: &PoseRuntime,
    airborne: bool,
) -> bool {
    if state.current != FighterState::BeHit || state.linked_recovery_pose_id != Some(pose.pose_id) {
        return false;
    }
    reaction.begin(
        getup_reaction_for_pose(pose.pose_id, pose.start, pose.end),
        airborne,
    );

    state.linked_recovery_pose_id = None;
    true
}

pub struct CombatPlugin;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CombatPoseResolveSet;

#[derive(Resource, Default)]
pub(crate) struct CombatResourceData(pub(crate) CombatResourceTable);

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        let actor_schedule = super::fighter_frame::schedule(app);
        configure_lock_updates(app);
        app.add_systems(
            actor_schedule,
            super::status_plugin::apply_self_attack_effect
                .in_set(crate::plugins::FighterFrameSet::SkeletonSample)
                .run_if(in_state(AppState::Playing))
                .run_if(crate::plugins::simulation_frame),
        );
        configure_periodic_status_updates(app);
        app.add_systems(
            actor_schedule,
            npc_lock_on_tick
                .before(local_attack_lock_facing)
                .in_set(crate::plugins::FighterFrameSet::AttackFacing)
                .run_if(in_state(AppState::Playing)),
        );
        app.init_resource::<super::hazard_plugin::StageHazardRuntime>()
            .init_resource::<crate::components::fighter::KillScore>()
            .init_resource::<LockOnController>()
            .init_resource::<crate::plugins::camera_plugin::CameraBasis>()
            .init_resource::<MatchEnd>()
            .add_systems(
                PreUpdate,
                advance_match_settlement.run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                OnEnter(AppState::Playing),
                (load_combat_resource_data, spawn_training_npc)
                    .run_if(crate::plugins::starts_fresh_playing_session),
            )
            .add_systems(
                OnExit(AppState::Playing),
                (
                    despawn_npc_fighters,
                    clear_lock_on_state,
                    super::status_plugin::clear_status_effects,
                )
                    .run_if(crate::plugins::ends_playing_session),
            )
            .add_systems(
                Update,
                (
                    tick_target_hit_clocks.run_if(crate::plugins::simulation_frame),
                    apply_weapon_strategy_equip_hooks,
                    init_fighter_poses,
                )
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::PoseDecision)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                actor_schedule,
                (
                    resolve_completed_pose_links.run_if(crate::plugins::simulation_frame),
                    combat_tick,
                    sync_weapon_form_state,
                )
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::PoseDecision)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                actor_schedule,
                preadvance_local_ground_locomotion_pose
                    .run_if(super::fighter_frame::current_is_local)
                    .after(crate::plugins::movement_plugin::move_local_fighter)
                    .in_set(crate::plugins::FighterFrameSet::Locomotion)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                actor_schedule,
                advance_poses
                    .in_set(crate::plugins::FighterFrameSet::PoseAdvance)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                actor_schedule,
                attack_windows
                    .in_set(crate::plugins::FighterFrameSet::HitJudge)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                actor_schedule,
                hit_reaction_root_motion
                    .in_set(crate::plugins::FighterFrameSet::RootMotion)
                    .after(crate::plugins::movement_plugin::attack_root_motion)
                    .after(crate::plugins::movement_plugin::dodge_root_motion)
                    .after(crate::plugins::ai_plugin::npc_ballistic_air_root_motion)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                actor_schedule,
                (
                    sync_attacks_after_animation,
                    tick_hit_reactions,
                    fist_weapon_swap,
                    default_poses,
                    commit_weapon_switch,
                )
                    .chain()
                    .in_set(CombatPoseResolveSet)
                    .in_set(crate::plugins::FighterFrameSet::PoseResolve)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                Update,
                (
                    super::hazard_plugin::sync_hazard_machines
                        .after(super::stage_plugin::tick_stage_script),
                    super::hazard_plugin::publish_stock_environment_geometry,
                    stage_hazard_damage.run_if(crate::plugins::simulation_frame),
                    evaluate_match_end,
                    death_respawn,
                )
                    .chain()
                    .in_set(CombatPoseResolveSet)
                    .in_set(crate::plugins::FighterFrameSet::PoseResolve)
                    .run_if(in_state(AppState::Playing)),
            );
    }
}

fn configure_periodic_status_updates(app: &mut App) {
    app.add_systems(
        Update,
        super::status_plugin::tick_status_effects
            .in_set(crate::plugins::FighterFrameSet::Movement)
            .run_if(crate::plugins::simulation_frame)
            .run_if(in_state(AppState::Playing)),
    );
    app.add_systems(
        Update,
        apply_periodic_status_hits
            .after(super::status_plugin::tick_status_effects)
            .after(super::item_plugin::tick_item_buffs)
            .before(super::fighter_frame::SerialFighterSet)
            .before(crate::plugins::FighterFrameSet::PoseDecision)
            .run_if(crate::plugins::simulation_frame)
            .run_if(in_state(AppState::Playing)),
    );
}

#[allow(clippy::type_complexity)]
fn apply_periodic_status_hits(
    assets: Option<Res<CharacterAssetsSet>>,
    net: Option<Res<NetState>>,
    mut fighters: Query<
        (
            &Fighter,
            &mut crate::components::fighter::FighterStatusEffects,
            &mut Health,
            &mut Qi,
            &mut PoseRuntime,
            &mut EntryBlend,
            &mut CombatState,
            &mut SmState,
            &mut FighterHitReaction,
            &MotionState,
            &FighterAir,
            (
                Option<&DeathRespawn>,
                Option<&mut FighterAttackMotion>,
                Option<&mut FighterDodge>,
                Option<&mut AiIntent>,
                Option<&LocalFighter>,
            ),
        ),
        Without<RemoteFighter>,
    >,
) {
    let online = net
        .as_ref()
        .is_some_and(|net| net.status == NetStatus::InRoom);
    for (
        fighter,
        mut status,
        mut health,
        mut qi,
        mut pose,
        mut blend,
        mut combat,
        mut sm,
        mut reaction,
        motion,
        air,
        (death, attack, dodge, intent, local),
    ) in &mut fighters
    {
        if !std::mem::take(&mut status.pending_periodic_hit) {
            continue;
        }
        if online
            || health.current <= 0.0
            || sm.current == FighterState::Dead
            || death.is_some_and(|death| death.active)
        {
            continue;
        }
        let Some(asset) = assets
            .as_ref()
            .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
        else {
            continue;
        };
        if !asset
            .poses
            .contains_key(&meteor_combat::hit_reaction::ENVIRONMENT_HIT_POSE)
        {
            continue;
        }
        let mut anger_spend = None;
        let mut view = CombatView {
            pose: &mut pose,
            blend: &mut blend,
            combat: &mut combat,
            health: &mut health,
            qi: &mut qi,
            air,
            assets: asset,
            weapon: fighter.weapon,
            guard_held_input: false,
            in_guard_hit_reaction: false,
            attack_directions: Vec::new(),
            weapon_form: 0,
            anger_spend: &mut anger_spend,
            strategy_id: weapon_to_strategy(fighter.weapon),
            weapon_action_vetoed: false,
        };
        let Some(hit) = meteor_combat::hit_reaction::begin_environmental_hit(&mut view, &[90])
        else {
            continue;
        };
        drop(view);
        reaction.begin(hit, air.air.airborne);
        reaction.bank_be_hit_carry(motion.velocity, local.is_some());
        combat.state = FighterState::BeHit;
        sm.previous = sm.current;
        sm.current = FighterState::BeHit;
        sm.time_in_state = 0.0;
        sm.locked = true;
        sm.attack_stage = 0;
        sm.attack_queued = None;
        sm.linked_recovery_pose_id = None;
        sm.exe_ground_mode_ticks = 0;
        if let Some(mut attack) = attack {
            *attack = default();
        }
        if let Some(mut dodge) = dodge {
            *dodge = default();
        }
        if let Some(mut intent) = intent {
            *intent = default();
        }
    }
}

fn stage_hazard_damage(
    time: Res<Time>,
    clock: Option<Res<crate::plugins::FrameClock>>,
    mut hazards: ResMut<super::hazard_plugin::StageHazardRuntime>,
    assets: Option<Res<CharacterAssetsSet>>,
    net: Res<NetState>,
    mut script: ResMut<super::stage_plugin::StageScriptRuntime>,
    mut drops: MessageWriter<super::render_plugin::OfflineWeaponDeathDrop>,
    mut fighters: Query<
        (
            Entity,
            &Fighter,
            &Transform,
            &mut Health,
            &mut Qi,
            &mut PoseRuntime,
            &mut EntryBlend,
            &mut CombatState,
            &mut SmState,
            &mut FighterHitReaction,
            &mut DeathRespawn,
            &mut MotionState,
            (
                &FighterAir,
                Option<&mut FighterAttackMotion>,
                Option<&TargetHitClock>,
                Option<&super::debug_plugin::CheatInvincible>,
                Option<&LocalFighter>,
                Option<&MissionNpc>,
            ),
        ),
        Without<RemoteFighter>,
    >,
) {
    let Some(assets) = assets else {
        return;
    };

    if net.status == NetStatus::InRoom {
        return;
    }
    let positions = fighters
        .iter()
        .filter(
            |(_, _, _, _, _, _, _, _, _, _, _, _, (_, _, _, _, local, _))| {
                net.status != NetStatus::InRoom || local.is_some()
            },
        )
        .map(
            |(entity, _, transform, health, _, _, _, _, _, _, _, _, (_, _, clock, _, _, _))| {
                (
                    entity.to_bits().to_string(),
                    transform.translation.to_array(),
                    health.current <= 0.0,
                    clock.is_some_and(|c| c.spawn_elapsed <= 3.0),
                )
            },
        )
        .collect::<Vec<_>>();
    let mut hits = Vec::new();
    hazards.0.update(
        crate::plugins::simulation_dt(clock.as_deref(), time.delta_secs()),
        &positions,
        &mut |id, _, damage, _| {
            hits.push((id.to_owned(), damage));
        },
    );
    for (id, damage) in hits {
        let Some(entity) = id.parse::<u64>().ok().and_then(Entity::try_from_bits) else {
            continue;
        };
        let Ok((
            _,
            fighter,
            transform,
            mut health,
            mut qi,
            mut pose,
            mut blend,
            mut combat,
            mut sm,
            mut reaction,
            mut death,
            mut motion,
            (air, attack, hit_clock, cheat, local, npc),
        )) = fighters.get_mut(entity)
        else {
            continue;
        };
        if damage <= 0
            || health.current <= 0.0
            || cheat.is_some()
            || hit_clock.is_some_and(|c| c.spawn_protected())
        {
            continue;
        }
        let Some(asset) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        health.current = (health.current - damage as f32).max(0.0);
        let killed = health.current <= 0.0;
        if let Some(mut attack) = attack {
            *attack = FighterAttackMotion::default();
        }
        let mut anger_spend = None;
        let mut view = CombatView {
            pose: &mut pose,
            blend: &mut blend,
            combat: &mut combat,
            health: &mut health,
            qi: &mut qi,
            air,
            assets: asset,
            weapon: fighter.weapon,
            guard_held_input: false,
            in_guard_hit_reaction: false,
            attack_directions: Vec::new(),
            weapon_form: 0,
            anger_spend: &mut anger_spend,
            strategy_id: weapon_to_strategy(fighter.weapon),
            weapon_action_vetoed: false,
        };
        if killed {
            reaction.begin(begin_death(&mut view, None), air.air.airborne);
        } else if let Some(hit) = meteor_combat::hit_reaction::begin_environmental_hit(
            &mut view,
            &asset.poses.keys().copied().collect::<Vec<_>>(),
        ) {
            reaction.begin(hit, air.air.airborne);
            reaction.bank_be_hit_carry(motion.velocity, local.is_some());
        }
        drop(view);
        let state = if killed {
            FighterState::Dead
        } else {
            FighterState::BeHit
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
        if !killed {
            continue;
        }
        clear_composed_velocity_on_death(&mut motion);
        death.active = true;
        death.timer = meteor_combat::death::RESPAWN_DELAY_SECONDS;
        if net.status != NetStatus::InRoom {
            if let Some(model) =
                super::character_plugin::weapon_model_stem(fighter.weapon, fighter.weapon_variant)
            {
                let yaw = transform.rotation.to_euler(EulerRot::YXZ).0;
                drops.write(super::render_plugin::OfflineWeaponDeathDrop {
                    model,
                    position: (transform.translation + Vec3::Y * 6.0).to_array(),
                    velocity: [yaw.sin() * 12.0, 0.0, yaw.cos() * 12.0],
                });
            }
        }
        let character_id = npc.map(|n| n.id).or_else(|| local.is_some().then_some(0));
        if let (Some(id), Some(runtime)) = (character_id, script.script_mut()) {
            use meteor_scene::scene_script_runtime::PstValue;
            if let Err(error) = runtime.call(
                "Scene_OnCharacterEvent",
                &[PstValue::Num(id as f64), PstValue::Num(202.0)],
            ) {
                warn!("environment death callback failed: {error}");
            }
        }
    }
}

fn load_combat_resource_data(mut commands: Commands) {
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let table = meteor_asset::runtime_assets::read_legacy_path("Meteor.res")
        .and_then(|bytes| parse_meteor_res(&bytes).ok())
        .map(|resource| resource.combat_table())
        .or_else(|| {
            crate::plugins::audio_plugin::resource_roots(&root)
                .into_iter()
                .find_map(|resource_root| {
                    std::fs::read(resource_root.join("Meteor.res"))
                        .ok()
                        .and_then(|bytes| parse_meteor_res(&bytes).ok())
                        .map(|resource| resource.combat_table())
                })
        })
        .unwrap_or_else(|| {
            warn!("Meteor.res unavailable; local combat uses the compatibility base-ATK fallback");
            CombatResourceTable::default()
        });
    info!(
        "combat resource table ready: {} pose segments, {} weapon variants",
        table.pose_attack_percent_count(),
        table.weapon_stats_count()
    );
    commands.insert_resource(CombatResourceData(table));
}

#[derive(Component)]
pub(crate) struct NpcFighter;

#[derive(Component)]
struct PoseInitialized;

fn tick_target_hit_clocks(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    impact: Option<Res<crate::plugins::impact_fx_plugin::ImpactSimulationScale>>,
    mut query: Query<&mut crate::components::fighter::TargetHitClock>,
) {
    let dt = crate::plugins::simulation_wall_step(frame_clock.as_deref(), time.delta_secs());
    for mut clock in &mut query {
        clock.tick_spawn(dt);
        if !impact.as_deref().is_some_and(|impact| impact.frozen) {
            clock.tick_target(dt);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntersectingTargetAdmission {
    IntervalBlocked,
    SpawnProtected,
    Accepted,
}

fn admit_intersecting_target(clock: &mut TargetHitClock) -> IntersectingTargetAdmission {
    if !clock.accepts_target_hit() {
        return IntersectingTargetAdmission::IntervalBlocked;
    }
    clock.reset();
    if clock.spawn_protected() {
        IntersectingTargetAdmission::SpawnProtected
    } else {
        IntersectingTargetAdmission::Accepted
    }
}

fn sync_weapon_form_state(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    mut query: Query<(Entity, &Fighter, &PoseRuntime, &mut WeaponForm)>,
) {
    for (entity, fighter, pose, mut form) in &mut query {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        if fighter.weapon != 10 {
            form.current = 0;
        } else if let Some(next) = weapon_form_for_pose(fighter.weapon, pose.pose_id) {
            form.current = next;
        }
    }
}

type PoseInitQuery<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static Fighter, &'static mut PoseRuntime),
    (Without<PoseInitialized>, Without<RemoteFighter>),
>;

fn init_fighter_poses(
    mut commands: Commands,
    assets: Option<Res<CharacterAssetsSet>>,
    mut query: PoseInitQuery<'_, '_>,
) {
    let Some(assets) = assets else {
        return;
    };
    for (entity, fighter, mut pose) in &mut query {
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        let pose_id = if character_assets.poses.contains_key(&0) {
            0
        } else {
            character_assets.poses.keys().next().copied().unwrap_or(0)
        };
        apply_pose(character_assets, &mut pose, pose_id, true);
        commands.entity(entity).insert(PoseInitialized);
    }
}

fn spawn_training_npc(
    mut commands: Commands,
    request: Res<crate::app_state::LaunchRequest>,
    room: Res<crate::app_state::RoomState>,
) {
    if !request.spawn_room_npcs {
        return;
    }
    let mut offset = NPC_SPAWN_OFFSET;
    for (slot, member) in (1u8..).zip(room.members.iter().filter(|m| m.is_npc)) {
        commands
            .spawn((
                NpcFighter,
                Fighter {
                    character_id: member.character_id,
                    player_slot: slot,
                    weapon: member.weapon,
                    weapon_slots: [member.weapon, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health::default(),
                Qi::default(),
                CombatState::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,

                    facing: std::f32::consts::PI,
                },
                PoseRuntime::idle(),
                SmState::default(),
                DeathRespawn {
                    spawn_pos: Vec3::new(0.0, 0.9, offset),
                    ..default()
                },
                FighterAir::default(),
                FighterDodge::default(),
                crate::components::fighter::AiIntent::default(),
                crate::plugins::ai_plugin::NpcPatrol::default(),
                NpcCombatProfile::web_debug(),
                Transform::from_xyz(0.0, 0.9, offset),
            ))
            .insert(Visibility::default())
            .insert(InheritedVisibility::default())
            .insert(EntryBlend::default())
            .insert(FighterAttackMotion::default())
            .insert(crate::components::fighter::VisualRootPin::default())
            .insert(crate::components::fighter::TargetHitClock::default());
        offset += 2.0;
    }
}

fn despawn_npc_fighters(mut commands: Commands, query: Query<Entity, With<NpcFighter>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

fn configure_lock_updates(app: &mut App) {
    let actor_schedule = super::fighter_frame::schedule(app);
    app.add_systems(
        actor_schedule,
        lock_on_tick
            .run_if(super::fighter_frame::current_is_local)
            .after(combat_tick)
            .before(sync_weapon_form_state)
            .in_set(crate::plugins::FighterFrameSet::PoseDecision)
            .run_if(crate::plugins::simulation_frame)
            .run_if(in_state(AppState::Playing)),
    )
    .add_systems(
        actor_schedule,
        (
            local_attack_lock_facing.run_if(super::fighter_frame::current_is_local),
            crate::plugins::character_plugin::sync_fighter_model_facing,
        )
            .chain()
            .in_set(crate::plugins::FighterFrameSet::AttackFacing)
            .run_if(crate::plugins::simulation_frame)
            .run_if(in_state(AppState::Playing)),
    );
}

fn clear_lock_targets(controller: &mut LockOnController) -> bool {
    let had_lock = controller.system.lock_target.is_some() || controller.lock_entity.is_some();
    controller.system.clear();
    controller.lock_entity = None;
    controller.track_entity = None;
    had_lock
}

fn clear_lock_on_state(mut controller: ResMut<LockOnController>) {
    let _ = clear_lock_targets(&mut controller);
}

fn apply_weapon_strategy_equip_hooks(
    mut controller: ResMut<LockOnController>,
    mut camera_basis: ResMut<crate::plugins::camera_plugin::CameraBasis>,
    mut query: Query<(&Fighter, &MotionState, &mut RangedFireGate), With<LocalFighter>>,
) {
    for (fighter, motion, mut gate) in &mut query {
        let equipped = (fighter.weapon, fighter.weapon_slot, fighter.weapon_variant);
        if gate.equipped_weapon == Some(equipped) {
            continue;
        }
        gate.equipped_weapon = Some(equipped);
        if meteor_combat::lock_on::weapon_equip_clears_lock(weapon_to_strategy(fighter.weapon)) {
            let had_lock = controller.system.clear_lock();
            controller.lock_entity = None;
            if had_lock {
                camera_basis.yaw = motion.facing;
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct OrderedLockCandidate {
    entity: Entity,
    candidate: LockCandidate,
}

fn stable_lock_candidates(mut candidates: Vec<OrderedLockCandidate>) -> Vec<LockCandidate> {
    candidates.sort_by_key(|candidate| candidate.entity.to_bits());
    candidates
        .into_iter()
        .map(|candidate| candidate.candidate)
        .collect()
}

type LockCandidateQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Health,
        &'static Transform,
        Option<&'static MissionNpc>,
        Option<&'static RemoteFighter>,
        Option<&'static LocalFighter>,
        Option<&'static Visibility>,
    ),
    With<Fighter>,
>;
type LockPlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Health,
        &'static SmState,
        &'static PoseRuntime,
        &'static EntryBlend,
        &'static MotionState,
        &'static Transform,
        &'static mut HeadTrackState,
        Option<&'static FighterAir>,
        Option<&'static WeaponSwitchActive>,
    ),
    With<LocalFighter>,
>;

fn attack_transition_facing_yaw(
    is_attacking: bool,
    transition_active: bool,
    player: Vec3,
    target: Vec3,
) -> Option<f32> {
    if !is_attacking || !transition_active {
        return None;
    }
    locked_attack_facing_yaw(
        (player.x, player.y, player.z),
        (target.x, target.y, target.z),
    )
}

type NpcLockQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static Health,
        &'static SmState,
        &'static PoseRuntime,
        &'static EntryBlend,
        &'static Transform,
        &'static mut MotionState,
        &'static mut AiIntent,
        Option<&'static MissionNpc>,
    ),
    (
        With<NpcFighter>,
        Without<LocalFighter>,
        Without<RemoteFighter>,
    ),
>;

fn npc_lock_on_tick(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    net: Res<NetState>,
    targets: LockCandidateQuery<'_, '_>,
    mut npcs: NpcLockQuery<'_, '_>,
) {
    for (entity, fighter, health, sm, pose, blend, transform, mut motion, mut intent, mission) in
        &mut npcs
    {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        if health.current <= 0.0 || meteor_ai::combat::weapon_tactics(fighter.weapon).ranged {
            intent.lock_target = None;
            continue;
        }
        let Some(target) = intent.lock_target else {
            continue;
        };
        let valid =
            targets
                .get(target)
                .ok()
                .filter(|(_, hp, _, npc, remote, local, visibility)| {
                    target != entity
                        && hp.current > 0.0
                        && visibility.is_none_or(|visibility| *visibility != Visibility::Hidden)
                        && teams_are_hostile(
                            fighter_combat_team(&net, false, None, mission),
                            fighter_combat_team(&net, local.is_some(), *remote, *npc),
                        )
                });
        let Some((_, _, target_transform, ..)) = valid else {
            intent.lock_target = None;
            continue;
        };
        if is_attacking_state(sm.current)
            && entry_blend_is_active(blend.duration, blend.pose_id, pose.pose_id)
        {
            if let Some(yaw) = attack_transition_facing_yaw(
                true,
                true,
                transform.translation,
                target_transform.translation,
            ) {
                motion.facing = yaw;
            } else {
                intent.lock_target = None;
            }
        }
    }
}

fn head_track_turn_limit(
    state: FighterState,
    air: Option<&FighterAir>,
    switching_weapon: bool,
) -> Option<f32> {
    if switching_weapon {
        return None;
    }
    let ballistic =
        air.is_some_and(|air| air.air.airborne && air.air.motion_mode == AirMotionMode::Ballistic);
    let curve_exit =
        air.is_some_and(|air| air.air.airborne && air.air.motion_mode == AirMotionMode::CurveExit);
    if curve_exit {
        Some(TRACK_CURVE_EXIT_TURN_LIMIT)
    } else if state == FighterState::Name || ballistic {
        Some(TRACK_TURN_LIMIT)
    } else {
        None
    }
}

#[derive(Default)]
struct LockVisibility {
    target: Option<u64>,
    hidden_seconds: f32,
}

impl LockVisibility {
    fn expired(&mut self, target: Option<u64>, visible: bool, dt: f32) -> bool {
        if self.target != target || target.is_none() || visible {
            self.hidden_seconds = 0.0;
        }
        self.target = target;
        if target.is_some() && !visible {
            self.hidden_seconds += dt.max(0.0);
        }
        self.hidden_seconds >= 0.6
    }
}

#[allow(clippy::too_many_arguments)]
fn lock_on_tick(
    mut controller: ResMut<LockOnController>,
    net: Res<NetState>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut camera_basis: Option<ResMut<crate::plugins::camera_plugin::CameraBasis>>,
    fighters: LockCandidateQuery<'_, '_>,
    mut player: LockPlayerQuery<'_, '_>,
    targets: Query<&Transform, Without<LocalFighter>>,
    collider: Option<Res<StageCollider>>,
    cameras: Query<(&Transform, &Projection), With<crate::plugins::camera_plugin::GameplayCamera>>,
    mut visibility: Local<LockVisibility>,
) {
    let Ok((
        player_entity,
        player_health,
        player_sm,
        _player_pose,
        _player_blend,
        player_motion,
        player_transform,
        mut head_state,
        player_air,
        switching_weapon,
    )) = player.single_mut()
    else {
        let _ = clear_lock_targets(&mut controller);
        *visibility = LockVisibility::default();
        return;
    };
    let p = player_transform.translation;
    if player_health.current <= 0.0 {
        *visibility = LockVisibility::default();
        if clear_lock_targets(&mut controller) {
            if let Some(basis) = camera_basis.as_deref_mut() {
                basis.yaw = player_motion.facing;
            }
        }
        return;
    }
    let player_team = fighter_combat_team(&net, true, None, None);
    let candidates = stable_lock_candidates(
        fighters
            .iter()
            .filter_map(
                |(entity, health, transform, mission, remote, local, visibility)| {
                    let team = fighter_combat_team(&net, local.is_some(), remote, mission);
                    (health.current > 0.0
                        && entity != player_entity
                        && visibility.is_none_or(|visibility| *visibility != Visibility::Hidden)
                        && teams_are_hostile(player_team, team))
                    .then_some(OrderedLockCandidate {
                        entity,
                        candidate: LockCandidate {
                            id: entity.to_bits(),
                            x: transform.translation.x,
                            y: transform.translation.y,
                            z: transform.translation.z,
                            team,
                        },
                    })
                },
            )
            .collect(),
    );
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let defer_attack_facing = is_attacking_state(player_sm.current);
    let had_lock = controller.system.lock_target.is_some();
    if let Some(target) = controller
        .system
        .lock_target
        .and_then(|id| candidates.iter().find(|candidate| candidate.id == id))
    {
        use meteor_physics::collision::{MeshCollider, Point3};
        let target_pos = Vec3::new(target.x, target.y, target.z);
        let eye = p + Vec3::Y * 3.4;
        let clear_from = |origin: Vec3, aim: Vec3| {
            let delta = aim - origin;
            collider
                .as_ref()
                .and_then(|c| c.mesh.as_ref())
                .is_none_or(|mesh| {
                    let direction = delta.normalize_or_zero();
                    mesh.raycast_hit(
                        Point3::new(origin.x, origin.y, origin.z),
                        Point3::new(direction.x, direction.y, direction.z),
                        (delta.length() - 0.1).max(0.0),
                    )
                    .is_none()
                })
        };
        let visible = [2.0, 4.5].into_iter().any(|height| {
            let aim = target_pos + Vec3::Y * height;
            let clear = clear_from(eye, aim);

            let on_screen = cameras.single().ok().is_none_or(|(camera, projection)| {
                let Projection::Perspective(projection) = projection else {
                    return true;
                };
                let local = camera.rotation.inverse() * (aim - camera.translation);
                let half_height = -local.z * (projection.fov * 0.5).tan() * 1.15;
                local.z < 0.0
                    && local.y.abs() <= half_height
                    && local.x.abs() <= half_height * projection.aspect_ratio
            });
            clear && on_screen
        });
        let hidden = visibility.expired(Some(target.id), visible, dt);
        if (!defer_attack_facing
            && meteor_combat::lock_on::locked_attack_gate_failed(
                (p.x, p.y, p.z),
                (target.x, target.y, target.z),
            ))
            || hidden
        {
            controller.system.clear_lock();
            *visibility = LockVisibility::default();
        }
    } else {
        *visibility = LockVisibility::default();
    }
    controller.system.update(
        dt,
        (p.x, p.y, p.z),
        player_motion.facing,
        &candidates,
        false,
        false,
    );
    if had_lock && controller.system.lock_target.is_none() {
        if let Some(basis) = camera_basis.as_deref_mut() {
            basis.yaw = player_motion.facing;
        }
    }
    controller.lock_entity = controller.system.lock_target.map(Entity::from_bits);
    controller.track_entity = controller.system.track_target.map(Entity::from_bits);

    let head_target = controller
        .system
        .track_target
        .map(Entity::from_bits)
        .and_then(|entity| targets.get(entity).ok())
        .map(|t| (t.translation.x, t.translation.z));
    if let Some(turn_limit) =
        head_track_turn_limit(player_sm.current, player_air, switching_weapon.is_some())
    {
        if let Some(next) = tracked_head_yaw(
            p.x,
            p.z,
            player_motion.facing,
            head_target,
            head_state.current,
            dt,
            turn_limit,
        ) {
            head_state.current = next;
        }
    }
}

type AttackFacingPlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Health,
        &'static SmState,
        &'static PoseRuntime,
        &'static EntryBlend,
        &'static mut MotionState,
        &'static Transform,
    ),
    With<LocalFighter>,
>;

fn local_attack_lock_facing(
    mut controller: ResMut<LockOnController>,
    mut camera_basis: Option<ResMut<crate::plugins::camera_plugin::CameraBasis>>,
    targets: LockCandidateQuery<'_, '_>,
    mut player: AttackFacingPlayerQuery<'_, '_>,
) {
    let Ok((entity, health, sm, pose, blend, mut motion, transform)) = player.single_mut() else {
        return;
    };
    if health.current <= 0.0
        || !is_attacking_state(sm.current)
        || !entry_blend_is_active(blend.duration, blend.pose_id, pose.pose_id)
    {
        return;
    }
    let Some(target) = controller.system.lock_target.map(Entity::from_bits) else {
        return;
    };
    let target_position = targets
        .get(target)
        .ok()
        .and_then(|(_, hp, pos, _, _, _, visibility)| {
            (target != entity
                && hp.current > 0.0
                && visibility.is_none_or(|v| *v != Visibility::Hidden))
            .then_some(pos.translation)
        });
    if let Some(yaw) = target_position.and_then(|position| {
        attack_transition_facing_yaw(true, true, transform.translation, position)
    }) {
        motion.facing = yaw;
    } else if target_position.is_none_or(|position| {
        meteor_combat::lock_on::locked_attack_gate_failed(
            transform.translation.to_array().into(),
            position.to_array().into(),
        )
    }) {
        controller.system.clear_lock();
        controller.lock_entity = None;
        if let Some(basis) = camera_basis.as_deref_mut() {
            basis.yaw = motion.facing;
        }
    }
}

struct StatsView<'a> {
    hp: &'a mut f32,
    qi: &'a mut f32,

    invincible_flag: bool,
}

impl CombatStats for StatsView<'_> {
    fn hp(&self) -> f32 {
        *self.hp
    }
    fn alive(&self) -> bool {
        *self.hp > 0.0
    }
    fn invincible(&self) -> bool {
        self.invincible_flag
    }
    fn anger(&self) -> f32 {
        *self.qi
    }
    fn anger_max(&self) -> f32 {
        meteor_core::DEFAULT_QI_MAX
    }
    fn def(&self) -> f32 {
        0.0
    }
    fn attack_bonus(&self) -> f32 {
        0.0
    }
    fn take_damage(&mut self, amount: i32) -> i32 {
        let before = *self.hp;
        *self.hp = (*self.hp - amount as f32).max(0.0);
        (before - *self.hp) as i32
    }
    fn gain_anger_from_damage(&mut self, damage: i32) -> i32 {
        let gain = (14 * damage / 100).min((meteor_core::DEFAULT_QI_MAX - *self.qi) as i32);
        *self.qi += gain as f32;
        gain
    }
    fn gain_anger_on_guard(&mut self, raw_atk: i32) -> i32 {
        let gain = guard_anger_gain(raw_atk);
        let before = *self.qi;
        *self.qi = (*self.qi + gain as f32).min(meteor_core::DEFAULT_QI_MAX);
        (*self.qi - before).max(0.0) as i32
    }
    fn reset_spawn_clock(&mut self) {}
}

fn apply_pose(assets: &CharacterAssets, pose: &mut PoseRuntime, pose_id: u16, hard: bool) {
    assets.apply_pose(pose, pose_id, hard);
}

struct CombatView<'a> {
    pose: &'a mut PoseRuntime,

    blend: &'a mut EntryBlend,
    combat: &'a mut CombatState,
    health: &'a mut Health,
    qi: &'a mut Qi,
    air: &'a FighterAir,
    assets: &'a CharacterAssets,
    weapon: u8,
    guard_held_input: bool,

    in_guard_hit_reaction: bool,

    attack_directions: Vec<CombatDir>,

    weapon_form: u8,

    anger_spend: &'a mut Option<(u16, u16)>,

    strategy_id: u8,

    weapon_action_vetoed: bool,
}

fn queued_qi_burst(
    source_pose_id: u16,
    frame: u32,
    transition_time: f32,
    qi: f32,
    has_pose: bool,
) -> Option<QueuedAttack> {
    if qi < QI_BURST_ANGER_COST || !has_pose {
        return None;
    }
    Some(QueuedAttack {
        source_pose_id,
        next_pose_id: QI_BURST_POSE_ID,
        last_evaluated_frame: frame,
        transition_time: transition_time.max(0.0),
        anger_cost: QI_BURST_ANGER_COST as u16,
        action_id: QI_BURST_ACTION_ID,
    })
}

fn queued_attack_commit_gate(
    can_commit: bool,
    has_target_pose: bool,
    qi: f32,
    anger_cost: u16,
) -> QueuedAttackCommit {
    if !can_commit {
        QueuedAttackCommit::Pending
    } else if !has_target_pose || !qi.is_finite() || qi < anger_cost as f32 {
        QueuedAttackCommit::Rejected
    } else {
        QueuedAttackCommit::Committed
    }
}

impl CombatView<'_> {
    fn play_pose_entry(
        &mut self,
        pose_id: u16,
        hard: bool,
        pending_blend_seconds: Option<f32>,
        kind: EntryBlendKind,
        skip_blend: bool,
        force_blend: bool,
    ) {
        self.assets.apply_pose_with_blend(
            self.pose,
            self.blend,
            pose_id,
            hard,
            pending_blend_seconds,
            kind,
            skip_blend,
            force_blend,
            Some(self.strategy_id),
        );
    }
}

impl FighterView for CombatView<'_> {
    fn guard_held(&self) -> bool {
        self.guard_held_input
    }
    fn in_guard_state(&self) -> bool {
        self.combat.state == FighterState::Guard
    }
    fn pose_id(&self) -> u16 {
        self.pose.pose_id
    }
    fn is_pose_finished(&self) -> bool {
        self.pose.finished()
    }
    fn is_airborne(&self) -> bool {
        self.air.air.airborne
    }
    fn ground_contact(&self) -> Option<bool> {
        Some(!self.air.air.airborne || self.air.ground_contact)
    }
    fn play_pose(&mut self, pose_id: u16, hard: bool) {
        self.play_pose_entry(pose_id, hard, None, EntryBlendKind::Generic, false, false);
    }
    fn play_pose_hard_cut(&mut self, pose_id: u16) {
        self.play_pose_entry(pose_id, true, None, EntryBlendKind::Generic, true, false);
    }
    fn set_in_guard(&mut self, v: bool) {
        let _ = v;
    }
    fn has_pose(&self, pose_id: u16) -> bool {
        self.assets.poses.contains_key(&pose_id)
    }
    fn pose_link(&self, pose_id: u16) -> Option<u16> {
        self.assets.poses.get(&pose_id).and_then(|pose| pose.link)
    }
    fn pose_group(&self, pose_id: u16) -> Option<u16> {
        self.assets.act.groups.get(&pose_id).copied()
    }
    fn weapon_action_vetoed(&self) -> bool {
        self.weapon_action_vetoed
    }
    fn can_qi_burst(&self) -> bool {
        can_qi_burst(
            self.combat.state,
            self.in_guard_hit_reaction,
            self.air.air.airborne,
            self.health.current > 0.0,
            self.qi.current,
            FighterView::has_pose(self, QI_BURST_POSE_ID),
        )
    }
    fn start_qi_burst(&mut self) -> bool {
        if self.qi.current < QI_BURST_ANGER_COST || !FighterView::has_pose(self, QI_BURST_POSE_ID) {
            return false;
        }
        self.play_pose_entry(
            QI_BURST_POSE_ID,
            true,
            None,
            EntryBlendKind::Generic,
            true,
            false,
        );
        self.qi.current -= QI_BURST_ANGER_COST;
        *self.anger_spend = Some((QI_BURST_ACTION_ID, QI_BURST_ANGER_COST as u16));
        true
    }
    fn queue_qi_burst(&mut self) -> Option<QueuedAttack> {
        let current = FighterView::pose_id(self);
        let pose_def = self.assets.poses.get(&current)?;
        let window = pose_def.next_pose.as_ref()?;
        let start = window.start?;
        let end = window.end?;
        let frame = self.pose.frame.floor() as u32;
        if frame < start || frame > end {
            return None;
        }

        queued_qi_burst(
            current,
            frame,
            window.time,
            self.qi.current,
            FighterView::has_pose(self, QI_BURST_POSE_ID),
        )
    }
    fn start_neutral_attack(&mut self, airborne: bool) -> bool {
        let Some(mut resolved) = resolve_affordable_weapon_command(
            self.weapon,
            &self.attack_directions,
            airborne,
            self.weapon_form,
            |cost| self.qi.current >= cost as f32,
        ) else {
            return false;
        };
        if !FighterView::has_pose(self, resolved.pose_id) {
            let Some(neutral) = resolve_affordable_weapon_command(
                self.weapon,
                &[],
                airborne,
                self.weapon_form,
                |cost| self.qi.current >= cost as f32,
            ) else {
                return false;
            };
            resolved = neutral;
            if !FighterView::has_pose(self, resolved.pose_id) {
                return false;
            }
        }

        self.play_pose_entry(
            resolved.pose_id,
            true,
            None,
            EntryBlendKind::Generic,
            true,
            false,
        );
        self.qi.current -= resolved.anger_cost as f32;
        if resolved.anger_cost > 0 {
            if let Some(action_id) = resolved.action_id {
                *self.anger_spend = Some((action_id, resolved.anger_cost));
            }
        }
        true
    }
    fn queue_attack(&mut self, airborne: bool) -> Option<QueuedAttack> {
        let current = FighterView::pose_id(self);
        let pose_def = self.assets.poses.get(&current)?;
        let window = pose_def.next_pose.as_ref()?;
        let start = window.start?;
        let end = window.end?;
        let frame = self.pose.frame.floor() as u32;
        if frame < start || frame > end {
            return None;
        }

        let resolved = resolve_affordable_weapon_command(
            self.weapon,
            &self.attack_directions,
            airborne,
            self.weapon_form,
            |cost| self.qi.current >= cost as f32,
        )?;
        let requested_action = self.assets.act.groups.get(&resolved.pose_id).copied()?;
        let candidates = self.assets.act.edges.get(&current)?;
        let poses: Vec<u16> = self.assets.poses.keys().copied().collect();
        let next_pose_id = pick_act_successor(
            requested_action,
            candidates,
            &poses,
            &self.assets.act.groups,
        )?;
        let anger_cost = resolved.anger_cost;

        if !FighterView::has_pose(self, next_pose_id) {
            return None;
        }

        Some(QueuedAttack {
            source_pose_id: current,
            next_pose_id,
            last_evaluated_frame: frame,
            transition_time: window.time.max(0.0),
            anger_cost,
            action_id: requested_action,
        })
    }
    fn consume_queued_attack(&mut self, queued: &mut QueuedAttack) -> QueuedAttackCommit {
        let Some(source) = self.assets.poses.get(&queued.source_pose_id) else {
            return QueuedAttackCommit::Rejected;
        };
        let frame = self.pose.frame.floor() as u32;
        let can_commit = if frame >= source.end {
            true
        } else {
            let first = queued.last_evaluated_frame.min(frame);
            (first..=frame).any(|candidate| {
                !source.actions.iter().any(|action| {
                    action.start.is_some_and(|start| candidate >= start)
                        && action.end.is_some_and(|end| candidate <= end)
                })
            })
        };
        let commit = queued_attack_commit_gate(
            can_commit,
            FighterView::has_pose(self, queued.next_pose_id),
            self.qi.current,
            queued.anger_cost,
        );
        if commit == QueuedAttackCommit::Pending {
            queued.last_evaluated_frame = frame;
            return commit;
        }
        if commit == QueuedAttackCommit::Rejected {
            return commit;
        }
        self.play_pose_entry(
            queued.next_pose_id,
            true,
            Some(queued.transition_time),
            EntryBlendKind::Chained,
            false,
            false,
        );

        self.blend.pending_local_chain = Some((
            self.pose.pose_instance_id,
            self.pose.pose_id,
            self.blend.duration,
        ));
        self.qi.current -= queued.anger_cost as f32;
        if queued.anger_cost > 0 {
            *self.anger_spend = Some((queued.action_id, queued.anger_cost));
        }
        QueuedAttackCommit::Committed
    }
}

impl ReactionFighter for CombatView<'_> {
    fn pose_id(&self) -> u16 {
        self.pose.pose_id
    }
    fn is_pose_finished(&self) -> bool {
        self.pose.finished()
    }
    fn has_pose(&self, pose_id: u16) -> bool {
        self.assets.poses.contains_key(&pose_id)
    }
    fn play_pose(&mut self, pose_id: u16, hard: bool) {
        self.play_pose_entry(pose_id, hard, None, EntryBlendKind::Generic, false, false);
    }
    fn play_pose_hard_cut(&mut self, pose_id: u16) {
        self.play_pose_entry(pose_id, true, None, EntryBlendKind::Generic, true, false);
    }
    fn set_in_guard(&mut self, v: bool) {
        let _ = v;
    }
    fn current_state(&self) -> FighterState {
        self.combat.state
    }
    fn set_state(&mut self, state: FighterState, locked: bool) {
        self.combat.state = state;
        let _ = locked;
    }
    fn alive(&self) -> bool {
        self.health.current > 0.0
    }
    fn guard_held(&self) -> bool {
        self.guard_held_input
    }
    fn pose_link(&self, pose_id: u16) -> Option<u16> {
        self.assets.poses.get(&pose_id).and_then(|pose| pose.link)
    }
    fn is_airborne(&self) -> bool {
        self.air.air.airborne
    }
    fn pose_range(&self, pose_id: u16) -> Option<(u32, u32)> {
        self.assets.pose_frames.get(&pose_id).copied()
    }
    fn release_pose_loop_to_end(&mut self, pose_id: u16) {
        if self.pose.pose_id == pose_id {
            self.pose.loops = false;
        }
    }
    fn rewind_reaction_pose(&mut self, pose_id: u16) {
        if self.pose.pose_id == pose_id {
            self.pose.frame = self.pose.loop_start as f32;
            self.pose.loops = true;
        }
    }
}

fn is_attacking_state(state: FighterState) -> bool {
    matches!(
        state,
        FighterState::Attack1 | FighterState::Attack2 | FighterState::Attack3
    )
}

fn attack_reaches_command_parser(state: FighterState) -> bool {
    is_attacking_state(state)
        || matches!(
            state,
            FighterState::Name | FighterState::Jump | FighterState::Dodge | FighterState::Burst
        )
}

fn attack_input_reaches_auto_lock(
    attack_pressed: bool,
    switch_blocks_attack: bool,
    hit_reaction_active: bool,
    state: FighterState,
) -> bool {
    attack_pressed
        && !switch_blocks_attack
        && !hit_reaction_active
        && !matches!(
            state,
            FighterState::BeHit | FighterState::Down | FighterState::Dead
        )
}

fn weapon_switch_context_pose(
    state: FighterState,
    linked_recovery: bool,
    airborne: bool,
    motion_mode: meteor_physics::jump::AirMotionMode,
) -> Option<u16> {
    if state == FighterState::Name && !linked_recovery && !airborne {
        Some(WEAPON_SWITCH_POSE_GROUND)
    } else if state == FighterState::Jump
        && airborne
        && motion_mode == meteor_physics::jump::AirMotionMode::Ballistic
    {
        Some(WEAPON_SWITCH_POSE_AIR)
    } else {
        None
    }
}

fn weapon_switch_entry_allowed(
    switch_pressed: bool,
    switch_active: bool,
    gun_aiming: bool,
    weapon_action_vetoed: bool,
    context_pose: Option<u16>,
    has_pose: bool,
) -> bool {
    switch_pressed
        && !switch_active
        && !gun_aiming
        && !weapon_action_vetoed
        && context_pose.is_some()
        && has_pose
}

fn actor_guard_held(
    local: bool,
    local_guard_pressed: bool,
    npc_guard_remaining: Option<f32>,
) -> bool {
    if local {
        local_guard_pressed
    } else {
        npc_guard_remaining.is_some_and(|remaining| remaining > 0.0)
    }
}

fn actor_uses_local_lock(local: bool, local_lock_active: bool) -> bool {
    local && local_lock_active
}

fn hit_result_reaches_auto_lock(killed: bool, remote_projection: bool) -> bool {
    remote_projection || !killed
}

fn clear_composed_velocity_on_death(motion: &mut MotionState) {
    motion.velocity = Vec3::ZERO;
}

fn should_consume_attack_command(
    attack_pressed: bool,
    aim_consumed: bool,
    switch_active: bool,
    switch_started: bool,
    state: FighterState,
) -> bool {
    attack_pressed
        && !aim_consumed
        && !switch_active
        && !switch_started
        && attack_reaches_command_parser(state)
}

fn dispatch_local_attack(
    sm: &mut StateMachine<CombatView<'_>>,
    command_input: &mut crate::plugins::input_plugin::LocalCommandInput,
    gun_aim: &mut GunAimState,
) -> (bool, bool) {
    let state = sm.current;
    let peek_directions: Vec<CombatDir> =
        command_input.0.peek().into_iter().map(combat_dir).collect();
    let aim_intercepted = {
        let view = sm.fighter_mut();
        let entry_pose = resolve_affordable_weapon_command(
            view.weapon,
            &peek_directions,
            false,
            view.weapon_form,
            |cost| view.qi.current >= cost as f32,
        )
        .map(|resolved| resolved.pose_id);
        matches!(
            state,
            FighterState::Name | FighterState::Dodge | FighterState::Burst
        ) && entry_pose.is_some_and(|entry_pose| {
            gun_aim_intercepts(
                view.strategy_id,
                gun_aim.aiming,
                view.air.air.airborne,
                entry_pose,
            )
        })
    };
    if aim_intercepted {
        gun_aim.aiming = true;
        let view = sm.fighter_mut();
        if view.assets.poses.contains_key(&GUN_AIM_READY_POSE) {
            view.play_pose_entry(
                GUN_AIM_READY_POSE,
                true,
                None,
                EntryBlendKind::Generic,
                true,
                false,
            );
        }
        return (false, true);
    }

    let directions = if should_consume_attack_command(true, false, false, false, state) {
        command_input.0.take().into_iter().map(combat_dir).collect()
    } else {
        Vec::new()
    };
    sm.fighter_mut().attack_directions = directions;
    (sm.handle_input(Action::Attack), false)
}

fn acquire_local_attack_lock(
    controller: &mut LockOnController,
    auto_camera: bool,
    strategy_id: u8,
    player: Vec3,
    facing: f32,
    candidates: &[LockCandidate],
) {
    if controller.system.acquire_on_attack(
        auto_camera,
        strategy_id,
        (player.x, player.y, player.z),
        facing,
        candidates,
    ) {
        controller.lock_entity = controller.system.lock_target.map(Entity::from_bits);
    }
}

fn dispatch_local_lock_toggle(
    controller: &mut LockOnController,
    camera_basis: &mut crate::plugins::camera_plugin::CameraBasis,
    gun_aim: &mut GunAimState,
    strategy_id: u8,
    player: Vec3,
    facing: f32,
    candidates: &[LockCandidate],
) {
    if gun_aim.aiming {
        gun_aim.aiming = false;
        return;
    }
    let had_lock = controller.system.lock_target.is_some();
    controller.system.toggle(
        strategy_id,
        (player.x, player.y, player.z),
        facing,
        candidates,
    );
    controller.lock_entity = controller.system.lock_target.map(Entity::from_bits);
    controller.track_entity = controller.system.track_target.map(Entity::from_bits);
    if had_lock && controller.system.lock_target.is_none() {
        camera_basis.yaw = facing;
    }
}

#[allow(clippy::too_many_arguments)]
fn begin_local_weapon_switch(
    sm: &mut StateMachine<CombatView<'_>>,
    switch_active: bool,
    gun_aiming: bool,
    fighter: &Fighter,
    motion: &MotionState,
    attack_motion: &FighterAttackMotion,
    common_amb: Option<&CommonAmb>,
) -> Option<WeaponSwitchActive> {
    let current = sm.current;
    let linked_recovery = sm.linked_recovery_pose_id.is_some();
    let view = sm.fighter_mut();
    let switch_pose = weapon_switch_context_pose(
        current,
        linked_recovery,
        view.air.air.airborne,
        view.air.air.motion_mode,
    );
    if !weapon_switch_entry_allowed(
        true,
        switch_active,
        gun_aiming,
        view.weapon_action_vetoed,
        switch_pose,
        switch_pose.is_some_and(|pose| view.assets.poses.contains_key(&pose)),
    ) {
        return None;
    }

    let switch_pose = switch_pose.expect("switch entry gate requires a context pose");
    let next_slot = other_weapon_slot(fighter.weapon_slot);
    let next = fighter.weapon_slots[next_slot];
    let start_root = common_amb.and_then(|common| {
        view.assets.poses.get(&switch_pose).map(|pose_def| {
            view.assets
                .amb_for_pose(&common.0, switch_pose)
                .sample_at(pose_def.start as f32)
                .root_pivot
        })
    });
    view.play_pose_entry(
        switch_pose,
        true,
        Some(0.12),
        EntryBlendKind::Generic,
        false,
        true,
    );

    view.pose.loops = false;

    Some(WeaponSwitchActive {
        to_weapon: next,
        to_slot: next_slot as u8,
        to_variant: fighter.weapon_slot_variants[next_slot],
        empty_target: next == 0,
        pose_id: view.pose.pose_id,
        pose_instance_id: view.pose.pose_instance_id,
        marker_frame: view.pose.loop_start,
        end_frame: view.pose.end,
        committed: false,
        end_sampled: false,
        airborne: view.air.air.airborne,
        carry_x: if view.air.air.airborne {
            motion.velocity.x
        } else {
            attack_motion.carry_x
        },
        carry_z: if view.air.air.airborne {
            motion.velocity.z
        } else {
            attack_motion.carry_z
        },
        root_started: start_root.is_some(),
        prev_root_x: start_root.map_or(0.0, |root| root[0]),
        prev_root_y: start_root.map_or(0.0, |root| root[1]),
    })
}

fn advance_pose_cursor(pose: &meteor_asset::pos::Pose, runtime: &PoseRuntime, dt: f32) -> f32 {
    let frame = runtime.frame;
    let frame_budget = dt * meteor_core::ANIM_FPS * pose_playback_speed_at_frame(pose, frame);
    if !runtime.plays_to_end() {
        let cursor = frame - runtime.start as f32;
        let advanced = advance_pose_loop_cursor(
            cursor,
            frame_budget,
            runtime.start,
            runtime.loop_start,
            runtime.loop_end,
        );
        runtime.start as f32 + advanced
    } else if frame >= runtime.end as f32 {
        frame
    } else {
        let count = (runtime.end - runtime.start + 1) as f32;
        let cursor = frame - runtime.start as f32;
        let room = count - cursor;
        if frame_budget < room {
            (frame + frame_budget).min(runtime.end as f32)
        } else {
            runtime.end as f32
        }
    }
}

fn attack_window_active(frame: f32, start: u32, end: u32) -> bool {
    let f = frame.floor() as u32;
    f >= start && f <= end
}

pub(crate) fn active_attack_window(
    frame: f32,
    attacks: &[meteor_asset::pos::AttackBlock],
) -> Option<(u32, &meteor_asset::pos::AttackBlock)> {
    attacks
        .iter()
        .enumerate()
        .filter(|(_, window)| attack_window_active(frame, window.start, window.end))
        .map(|(index, window)| (index as u32, window))
        .next_back()
}

fn bevy_mat4_to_core(m: bevy::math::Mat4) -> meteor_core::Mat4 {
    meteor_core::Mat4 {
        m: m.to_cols_array(),
    }
}

type FighterModelQuery<'w, 's> = Query<'w, 's, (Entity, &'static FighterModel)>;
type FighterModelWorldQuery<'w, 's> =
    Query<'w, 's, (Entity, &'static FighterModel, &'static GlobalTransform)>;
type FighterBoneWorldQuery<'w, 's> =
    Query<'w, 's, (Entity, &'static FighterBone, &'static GlobalTransform)>;
type FighterDummyWorldQuery<'w, 's> =
    Query<'w, 's, (Entity, &'static FighterDummy, &'static GlobalTransform)>;
type WeaponCollisionWorldQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static WeaponCollisionGeometry,
        &'static GlobalTransform,
        &'static Visibility,
    ),
>;

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct HitGeometryQueries<'w, 's> {
    models: FighterModelQuery<'w, 's>,
    model_worlds: FighterModelWorldQuery<'w, 's>,
    bones: FighterBoneWorldQuery<'w, 's>,
    skeletons: FighterSkeletonQuery<'w, 's>,
    dummies: FighterDummyWorldQuery<'w, 's>,
    weapons: WeaponCollisionWorldQuery<'w, 's>,
    transforms: TransformHelper<'w, 's>,
    stage: Option<Res<'w, StageCollider>>,
    effects: Query<
        'w,
        's,
        (
            &'static crate::plugins::effect_plugin::PonEfInstance,
            &'static ChildOf,
        ),
    >,
}

impl HitGeometryQueries<'_, '_> {
    pub(crate) fn hurt_obbs(&self, entity: Entity, assets: &CharacterAssets) -> Vec<Obb> {
        fighter_body_hurt_obbs(entity, assets, self)
    }

    pub(crate) fn live_effect_obbs(&self, fighter: Entity, assets: &CharacterAssets) -> Vec<Obb> {
        let Some(model) = find_fighter_model(fighter, &self.models) else {
            return Vec::new();
        };
        self.effects
            .iter()
            .filter(|(effect, parent)| {
                parent.parent() == model
                    && !effect.looping
                    && effect.elapsed <= effect.duration + 1e-4
            })
            .flat_map(|(effect, _)| {
                effect_attack_obbs(fighter, assets, &effect.ef, effect.elapsed, self)
            })
            .collect()
    }

    pub(crate) fn attack_obbs(
        &self,
        entity: Entity,
        fighter: &Fighter,
        form: &WeaponForm,
        pose_id: u16,
        assets: &CharacterAssets,
        bone_names: &[String],
    ) -> Vec<Obb> {
        let bones = build_attacker_bone_resolver(entity, assets, self);
        let weapons = build_weapon_collision_parts(
            entity,
            fighter.weapon,
            fighter.weapon_slot,
            fighter.weapon_variant,
            meteor_combat::combo_engine::weapon_visual_form(fighter.weapon, form.current, pose_id),
            &self.weapons,
            &self.transforms,
        );
        attack_hit_obbs(bone_names, &weapons, |name| bones.get(name).cloned())
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct HitFeedback<'w> {
    effects: Option<Res<'w, crate::plugins::effect_plugin::EffectAssets>>,
    impacts: MessageWriter<'w, crate::plugins::impact_fx_plugin::CombatImpactEvent>,
    settings: Res<'w, Settings>,
}

#[derive(bevy::ecs::system::SystemParam)]
struct RangedRuntimeParams<'w, 's> {
    statuses: Query<'w, 's, &'static mut crate::components::fighter::FighterStatusEffects>,
    net_tx: Option<Res<'w, crate::plugins::network_plugin::NetTx>>,
    pending: ResMut<'w, PendingRangedHits>,
    camera: Query<'w, 's, &'static GlobalTransform, With<GameplayCamera>>,
}

fn bone_world_matrix(
    character_assets: &CharacterAssets,
    bone: &FighterBone,
    global: &GlobalTransform,
) -> Option<meteor_core::Mat4> {
    let bind_inverse = character_assets
        .skeleton
        .skin_world_bind_inverse
        .get(bone.skin_index)?;
    Some(bevy_mat4_to_core(global.to_matrix()).mul(bind_inverse))
}

fn current_global_transform(
    entity: Entity,
    propagated: &GlobalTransform,
    transforms: &TransformHelper<'_, '_>,
) -> GlobalTransform {
    transforms
        .compute_global_transform(entity)
        .unwrap_or(*propagated)
}

fn find_fighter_model(fighter: Entity, models: &FighterModelQuery) -> Option<Entity> {
    models
        .iter()
        .find_map(|(entity, m)| (m.fighter == fighter).then_some(entity))
}

fn build_attacker_bone_resolver(
    fighter: Entity,
    character_assets: &CharacterAssets,
    geometry: &HitGeometryQueries<'_, '_>,
) -> std::collections::HashMap<String, (meteor_core::Mat4, BoneLocalBox)> {
    let mut map = std::collections::HashMap::new();
    let Some(model_entity) = find_fighter_model(fighter, &geometry.models) else {
        return map;
    };
    for (entity, bone, propagated) in geometry
        .bones
        .iter_many(model_bones(model_entity, &geometry.skeletons))
    {
        let Some(Some(local)) = character_assets.bone_local_bounds.get(bone.skin_index) else {
            continue;
        };
        let global = current_global_transform(entity, propagated, &geometry.transforms);
        let Some(world) = bone_world_matrix(character_assets, bone, &global) else {
            continue;
        };
        map.insert(normalized_bone_name(&bone.name), (world, *local));
    }
    map
}

fn weapon_collision_part_is_active(
    geometry: &WeaponCollisionGeometry,
    visibility: &Visibility,
    fighter: Entity,
    weapon_id: u8,
    weapon_slot: u8,
    weapon_variant: u8,
    weapon_form: u8,
) -> bool {
    geometry.fighter == fighter
        && geometry.weapon == weapon_id
        && geometry.slot == weapon_slot
        && geometry.variant == weapon_variant
        && *visibility != Visibility::Hidden
        && geometry
            .forms
            .as_ref()
            .is_none_or(|forms| forms.contains(&weapon_form))
}

fn build_weapon_collision_parts(
    fighter: Entity,
    weapon_id: u8,
    weapon_slot: u8,
    weapon_variant: u8,
    weapon_form: u8,
    weapons: &WeaponCollisionWorldQuery<'_, '_>,
    transforms: &TransformHelper<'_, '_>,
) -> Vec<WeaponCollisionPart> {
    weapons
        .iter()
        .filter(|(_, geometry, _, visibility)| {
            weapon_collision_part_is_active(
                geometry,
                visibility,
                fighter,
                weapon_id,
                weapon_slot,
                weapon_variant,
                weapon_form,
            )
        })
        .map(|(entity, geometry, propagated, _)| WeaponCollisionPart {
            world_matrix: bevy_mat4_to_core(
                current_global_transform(entity, propagated, transforms).to_matrix(),
            ),
            boxes: geometry
                .boxes
                .iter()
                .map(|bounds| BoneLocalBox {
                    center: meteor_core::Vec3::new(
                        bounds.center[0],
                        bounds.center[1],
                        bounds.center[2],
                    ),
                    half_size: meteor_core::Vec3::new(
                        bounds.half_size[0],
                        bounds.half_size[1],
                        bounds.half_size[2],
                    ),
                })
                .collect(),
        })
        .collect()
}

fn effect_attack_obbs(
    fighter: Entity,
    character_assets: &CharacterAssets,
    ef: &meteor_asset::pon_ef::PonEf,
    time_secs: f32,
    geometry: &HitGeometryQueries<'_, '_>,
) -> Vec<Obb> {
    let Some((model_entity, _)) = geometry
        .models
        .iter()
        .find(|(_, model)| model.fighter == fighter)
    else {
        return Vec::new();
    };
    let Some(root) = geometry
        .model_worlds
        .iter()
        .find_map(|(entity, model, propagated)| {
            (model.fighter == fighter).then(|| {
                bevy_mat4_to_core(
                    current_global_transform(entity, propagated, &geometry.transforms).to_matrix(),
                )
            })
        })
    else {
        return Vec::new();
    };
    let mut external = std::collections::HashMap::<String, meteor_core::Mat4>::new();
    external.insert("character".into(), root);
    external.insert("channel".into(), root);
    external.insert("root".into(), root);
    for (entity, bone, propagated) in geometry
        .bones
        .iter_many(model_bones(model_entity, &geometry.skeletons))
    {
        let global = current_global_transform(entity, propagated, &geometry.transforms);
        external.insert(
            bone.name.to_ascii_lowercase(),
            bevy_mat4_to_core(global.to_matrix()),
        );
    }
    for (name, index) in &character_assets.dummy_order {
        if let Some((entity, _, propagated)) = geometry
            .dummies
            .iter_many(model_dummies(model_entity, &geometry.skeletons))
            .find(|(_, dummy, _)| dummy.dummy_index == *index)
        {
            let global = current_global_transform(entity, propagated, &geometry.transforms);
            external.insert(
                name.to_ascii_lowercase(),
                bevy_mat4_to_core(global.to_matrix()),
            );
        }
    }

    let mut memo = std::collections::HashMap::new();
    let mut visiting = std::collections::HashSet::new();
    let mut resolve_external = |name: &str| external.get(&name.to_ascii_lowercase()).copied();
    ef.nodes
        .iter()
        .filter(|node| {
            node.is_attack_node()
                && time_secs >= node.start_time - 1e-4
                && time_secs <= node.end_time + 1e-4
        })
        .filter_map(|node| {
            let world = meteor_asset::pon_ef::node_world_matrix(
                ef,
                node.index,
                time_secs,
                &mut memo,
                &mut visiting,
                &mut resolve_external,
            )?;
            let half = node.half_extents()?;
            bone_world_obb(
                &world,
                &BoneLocalBox {
                    center: meteor_core::Vec3::ZERO,
                    half_size: meteor_core::Vec3::new(half[0], half[1], half[2]),
                },
            )
        })
        .collect()
}

fn fighter_body_hurt_obbs(
    fighter: Entity,
    character_assets: &CharacterAssets,
    geometry: &HitGeometryQueries<'_, '_>,
) -> Vec<Obb> {
    let Some(model_entity) = find_fighter_model(fighter, &geometry.models) else {
        return Vec::new();
    };
    let entries = geometry
        .bones
        .iter_many(model_bones(model_entity, &geometry.skeletons))
        .filter_map(|(entity, bone, propagated)| {
            if !meteor_combat::hitbox::is_hurt_bone_name(&bone.name) {
                return None;
            }
            let local = character_assets
                .bone_local_bounds
                .get(bone.skin_index)?
                .as_ref()?;
            let global = current_global_transform(entity, propagated, &geometry.transforms);
            let world = bone_world_matrix(character_assets, bone, &global)?;
            Some((bone.name.as_str(), world, Some(*local)))
        });
    body_hurt_obbs(entries)
}

fn fighter_whole_body_obb(
    fighter: Entity,
    assets: &CharacterAssets,
    geometry: &HitGeometryQueries<'_, '_>,
) -> Option<Obb> {
    let model = find_fighter_model(fighter, &geometry.models)?;
    let boxes = geometry
        .bones
        .iter_many(model_bones(model, &geometry.skeletons))
        .filter_map(|(entity, bone, propagated)| {
            let local = assets.bone_local_bounds.get(bone.skin_index)?.as_ref()?;
            let world = current_global_transform(entity, propagated, &geometry.transforms);
            let matrix = bone_world_matrix(assets, bone, &world)?;
            bone_world_obb(&matrix, local)
        });
    whole_body_aabb(boxes)
}

fn effect_body_occluded(stage: Option<&StageCollider>, source: Option<&Obb>, target: &Obb) -> bool {
    use meteor_physics::collision::{MeshCollider, Point3};
    let (Some(mesh), Some(source)) = (stage.and_then(|stage| stage.mesh.as_ref()), source) else {
        return false;
    };
    let from = Vec3::new(source.center.x, source.center.y, source.center.z);
    let to = Vec3::new(target.center.x, target.center.y, target.center.z);
    let delta = to - from;
    let distance = delta.length();
    distance > 1e-6
        && mesh
            .raycast_hit(
                Point3::new(from.x, from.y, from.z),
                Point3::new(delta.x, delta.y, delta.z),
                distance,
            )
            .is_some_and(|hit| hit.t < distance)
}

fn whole_body_aabb(boxes: impl Iterator<Item = Obb>) -> Option<Obb> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for obb in boxes {
        let center = Vec3::new(obb.center.x, obb.center.y, obb.center.z);
        let mut extent = Vec3::ZERO;
        for (axis, half) in obb
            .axes
            .iter()
            .zip([obb.half_size.x, obb.half_size.y, obb.half_size.z])
        {
            extent += Vec3::new(axis.x, axis.y, axis.z).abs() * half;
        }
        min = min.min(center - extent);
        max = max.max(center + extent);
    }
    if !min.is_finite() || !max.is_finite() {
        return None;
    }
    let center = (min + max) * 0.5;
    let half = (max - min) * 0.5;
    Some(Obb {
        center: meteor_core::Vec3::new(center.x, center.y, center.z),
        half_size: meteor_core::Vec3::new(half.x, half.y, half.z),
        axes: [
            meteor_core::Vec3::new(1.0, 0.0, 0.0),
            meteor_core::Vec3::new(0.0, 1.0, 0.0),
            meteor_core::Vec3::new(0.0, 0.0, 1.0),
        ],
    })
}

pub(crate) fn entry_blend_is_active(
    blend_duration: f32,
    blend_pose_id: u16,
    current_pose_id: u16,
) -> bool {
    blend_duration > 0.0 && blend_pose_id == current_pose_id
}

fn jump_transition_is_hard_cut(from_pose_id: u16, to_pose_id: u16) -> bool {
    is_jump_pose_id(from_pose_id) || is_jump_pose_id(to_pose_id)
}

fn take_ground_entry_blend(
    air: Option<&mut crate::plugins::movement_plugin::FighterAir>,
    movement_mode: meteor_core::MovementMode,
) -> bool {
    let Some(air) = air else {
        return false;
    };
    if !air.ground_entry_blend_pending || movement_mode == meteor_core::MovementMode::Airborne {
        return false;
    }
    air.ground_entry_blend_pending = false;
    true
}

pub(super) fn advance_poses(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    assets: Option<Res<CharacterAssetsSet>>,
    mut query: Query<(Entity, &Fighter, &mut PoseRuntime, &mut EntryBlend)>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let Some(assets) = assets else {
        return;
    };
    for (entity, fighter, mut pose, mut blend) in &mut query {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        let Some(pose_data) = character_assets.poses.get(&pose.pose_id) else {
            continue;
        };

        let blend_active = entry_blend_is_active(blend.duration, blend.pose_id, pose.pose_id);
        if blend_active {
            pose.frame = blend.to_frame.clamp(pose.start as f32, pose.end as f32);
            continue;
        }
        pose.frame = advance_pose_cursor(pose_data, &pose, dt);

        if matches!(pose.pose_id, 152 | 154 | 156 | 158) && pose.finished() {
            if let meteor_anim::pose_controller::PoseLinkDecision::Link {
                link_pose_id,
                entry_blend_seconds,
            } = meteor_anim::pose_controller::resolve_completed_pose(
                pose.pose_id,
                Some(pose_data),
                |id| character_assets.poses.contains_key(&id),
            ) {
                let instance = pose.pose_instance_id;
                character_assets.apply_pose_with_blend(
                    &mut pose,
                    &mut blend,
                    link_pose_id,
                    true,
                    entry_blend_seconds,
                    EntryBlendKind::Generic,
                    false,
                    false,
                    None,
                );
                pose.pose_instance_id = instance;
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn resolve_completed_pose_links(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    assets: Option<Res<CharacterAssetsSet>>,
    mut query: Query<
        (
            Entity,
            &Fighter,
            &mut PoseRuntime,
            &mut EntryBlend,
            &mut SmState,
            &mut CombatState,
            &mut Health,
            &mut Qi,
            &FighterAir,
            Option<&WeaponSwitchActive>,
            &WeaponForm,
            Option<&RemoteFighter>,
        ),
        With<Fighter>,
    >,
) {
    let Some(assets) = assets else {
        return;
    };
    for (
        entity,
        fighter,
        mut pose,
        mut blend,
        mut sm_state,
        mut combat,
        mut health,
        mut qi,
        air,
        switching,
        weapon_form,
        remote,
    ) in &mut query
    {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }

        if switching.is_some() || !pose.finished() {
            continue;
        }
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        let Some(completed) = character_assets.poses.get(&pose.pose_id) else {
            continue;
        };
        let decision = meteor_anim::pose_controller::resolve_completed_pose(
            pose.pose_id,
            Some(completed),
            |id| character_assets.poses.contains_key(&id),
        );
        let meteor_anim::pose_controller::PoseLinkDecision::Link {
            link_pose_id,
            entry_blend_seconds,
        } = decision
        else {
            continue;
        };

        if remote.is_none() && state_machine_tracks_pose_link(sm_state.current) {
            combat.state = sm_state.current;
            let mut anger_spend = None;
            let view = CombatView {
                pose: &mut pose,
                blend: &mut blend,
                combat: &mut combat,
                health: &mut health,
                qi: &mut qi,
                air,
                assets: character_assets,
                weapon: fighter.weapon,
                guard_held_input: false,
                in_guard_hit_reaction: false,
                attack_directions: Vec::new(),
                weapon_form: weapon_form.current,
                anger_spend: &mut anger_spend,
                strategy_id: weapon_to_strategy(fighter.weapon),
                weapon_action_vetoed: false,
            };
            let mut sm = StateMachine::new(view);
            sm.current = sm_state.current;
            sm.previous = sm_state.previous;
            sm.time_in_state = sm_state.time_in_state;
            sm.locked = sm_state.locked;
            sm.attack_stage = sm_state.attack_stage;
            sm.attack_queued = sm_state.attack_queued.clone();
            sm.linked_recovery_pose_id = sm_state.linked_recovery_pose_id;
            sm.exe_ground_mode_ticks = sm_state.exe_ground_mode_ticks;
            sm.on_pose_linked(completed.id, link_pose_id);
            let projected = project_state_machine(&sm);
            drop(sm);
            *sm_state = projected;
            combat.state = sm_state.current;
        }

        let skip_blend = jump_transition_is_hard_cut(completed.id, link_pose_id);

        let pose_instance_id = pose.pose_instance_id;
        character_assets.apply_pose_with_blend(
            &mut pose,
            &mut blend,
            link_pose_id,
            true,
            entry_blend_seconds,
            EntryBlendKind::Generic,
            skip_blend,
            false,
            None,
        );
        pose.pose_instance_id = pose_instance_id;
    }
}

fn state_machine_tracks_pose_link(state: FighterState) -> bool {
    is_attacking_state(state) || state == FighterState::Cure
}

type CombatTickQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static mut PoseRuntime,
        &'static mut EntryBlend,
        &'static mut CombatState,
        &'static mut Health,
        &'static mut Qi,
        &'static mut SmState,
        &'static DeathRespawn,
        &'static FighterAir,
        &'static MotionState,
        &'static Transform,
        Option<&'static LocalFighter>,
        &'static mut GunAimState,
        (
            &'static mut AiIntent,
            &'static WeaponForm,
            &'static mut FighterAttackMotion,
            Option<&'static MissionNpc>,
            Option<&'static Visibility>,
            &'static mut FighterHitReaction,
            Option<&'static NpcPerformance>,
        ),
    ),
    (With<Fighter>, Without<RemoteFighter>),
>;

type RemoteLockCandidateQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Health,
        &'static Transform,
        &'static RemoteFighter,
        Option<&'static Visibility>,
    ),
    With<RemoteFighter>,
>;

fn bank_attack_commit_motion(
    attack_motion: &mut FighterAttackMotion,
    composed_velocity: Vec3,
    locally_driven: bool,
    weapon: u8,
) {
    let mut carry = CarryVelocity::new(attack_motion.carry_x, attack_motion.carry_z);
    let composed = CarryVelocity::new(composed_velocity.x, composed_velocity.z);
    let _ = bank_carry_velocity(&mut carry, composed, locally_driven);
    if weapon == NINJA_BLADE_WEAPON_ID {
        carry.scale(NINJA_BLADE_CARRY_SCALE);
    }
    attack_motion.carry_x = carry.x;
    attack_motion.carry_z = carry.z;
    attack_motion.carry_banked = true;
}

fn release_attack_motion_after_pose_resolve(
    motion: &mut FighterAttackMotion,
    was_attacking: bool,
    is_attacking: bool,
) -> bool {
    if !was_attacking || is_attacking || !motion.active {
        return false;
    }
    motion.active = false;
    motion.stopping = false;
    true
}

type AttackerWindowQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static PoseRuntime,
        &'static EntryBlend,
        &'static MotionState,
        &'static Transform,
        &'static SmState,
        &'static Health,
        &'static DeathRespawn,
        &'static FighterHitReaction,
        Option<&'static MissionNpc>,
        Option<&'static LocalFighter>,
        Option<&'static RemoteFighter>,
        &'static mut RangedFireGate,
        (&'static WeaponForm, Option<&'static AiIntent>),
    ),
    With<Fighter>,
>;

type DefenderWindowQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Health,
        &'static mut Qi,
        &'static mut PoseRuntime,
        &'static mut EntryBlend,
        &'static mut MotionState,
        &'static mut CombatState,
        &'static mut SmState,
        &'static mut DeathRespawn,
        &'static mut crate::components::fighter::TargetHitClock,
        &'static Fighter,
        &'static Transform,
        Option<&'static RemoteFighter>,
        Option<&'static MissionNpc>,
        (
            Option<&'static LocalFighter>,
            &'static WeaponForm,
            Option<&'static Visibility>,
            &'static mut FighterHitReaction,
            Option<&'static mut FighterAttackMotion>,
            Option<&'static mut FighterAir>,
            Option<&'static crate::plugins::debug_plugin::CheatInvincible>,
        ),
    ),
    With<Fighter>,
>;

type DefenderPositionQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static Health,
        &'static SmState,
        &'static DeathRespawn,
        &'static FighterHitReaction,
    ),
    With<Fighter>,
>;

struct AttackSnapshot {
    entity: Entity,
    weapon: u8,
    weapon_stats: Option<WeaponCombatStats>,
    pose_id: u16,
    pose_frame: f32,
    pose_start: u32,
    player_slot: u8,
    combat_team: i32,

    script_character_id: i64,

    script_team: Option<i64>,
    atk_pos: Vec3,

    forward: Vec3,

    active_window: Option<(u32, meteor_asset::pos::AttackBlock)>,

    requires_live_attack_owner: bool,

    props_only: bool,
    geometry: AttackGeometry,
    effect_obbs: Vec<Obb>,
    effect_origin: Option<Obb>,
}

enum AttackGeometry {
    Obbs(Vec<Obb>),
    Segment {
        start: Vec3,
        end: Vec3,
        hit_scale: f32,
        projectile: Entity,
        stop_after_first: bool,
        despawn_after: bool,
    },
}

fn sort_ranged_defender_candidates(candidates: &mut [(Entity, f32)]) {
    candidates.sort_by(|a, b| a.1.total_cmp(&b.1));
}

#[cfg(test)]
fn attack_window_owner_active(
    health: f32,
    death_active: bool,
    state: FighterState,
    reaction_active: bool,
) -> bool {
    health > 0.0 && !death_active && is_attacking_state(state) && !reaction_active
}

fn attack_window_owner_active_for_scope(
    health: f32,
    death_active: bool,
    state: FighterState,
    reaction_active: bool,
    props_only: bool,
) -> bool {
    health > 0.0 && !death_active && !reaction_active && (props_only || is_attacking_state(state))
}

fn attack_forward_from_facing(facing: f32) -> Vec3 {
    let (sin_yaw, cos_yaw) = facing.sin_cos();
    Vec3::new(sin_yaw, 0.0, cos_yaw)
}

fn sort_attack_snapshots(snapshots: &mut [AttackSnapshot]) {
    snapshots.sort_by(
        |a, b| match (a.requires_live_attack_owner, b.requires_live_attack_owner) {
            (true, true) => a.player_slot.cmp(&b.player_slot),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => std::cmp::Ordering::Equal,
        },
    );
}

fn combat_weapon_stats(
    table: &CombatResourceTable,
    fighter: &Fighter,
) -> Option<WeaponCombatStats> {
    crate::plugins::character_plugin::weapon_model_stem(fighter.weapon, fighter.weapon_variant)
        .as_deref()
        .and_then(|model| table.weapon_stats_for_model(model))
        .or_else(|| table.weapon_stats(fighter.weapon))
}

fn scene_on_attack_args(
    character_id: i64,
    damage: i32,
) -> [meteor_scene::scene_script_runtime::PstValue; 2] {
    [
        meteor_scene::scene_script_runtime::PstValue::Num(character_id as f64),
        meteor_scene::scene_script_runtime::PstValue::Num(damage.max(0) as f64),
    ]
}

pub(crate) fn script_allows_prop_hit(
    runtime: &meteor_scene::scene_script_runtime::SceneScriptRuntime,
    name: &str,
) -> bool {
    let host = runtime.host();
    let Some(instance) = host
        .resolve_name(name)
        .and_then(|index| host.instance(index))
    else {
        return true;
    };

    ["active", "interactive"].iter().all(|key| {
        instance
            .props
            .get(*key)
            .is_none_or(|value| value.as_f64() != 0.0)
    })
}

fn stage_prop_hit_obb(prop: &crate::plugins::item_plugin::StageProp, transform: &Transform) -> Obb {
    let core = |v: Vec3| meteor_core::Vec3::new(v.x, v.y, v.z);
    let rotation = transform.rotation;
    Obb {
        center: core(prop.world_center(transform)),
        axes: [
            core(rotation * Vec3::X),
            core(rotation * Vec3::Y),
            core(rotation * Vec3::Z),
        ],
        half_size: core(prop.half_extents),
    }
}

pub(crate) fn fighter_combat_team(
    net: &NetState,
    local: bool,
    remote: Option<&RemoteFighter>,
    mission_npc: Option<&MissionNpc>,
) -> i32 {
    if net.status == NetStatus::InRoom {
        if !local && remote.is_none() {
            return if net.mode == "leader" {
                0
            } else {
                mission_npc.map_or(0, |npc| i32::from(npc.team))
            };
        }
        let player_id = if local {
            Some(net.player_id.as_str())
        } else {
            remote.map(|remote| remote.player_id.as_str())
        };
        return player_id
            .and_then(|player_id| {
                net.members
                    .iter()
                    .find(|member| member.player_id == player_id)
                    .and_then(|member| member.team.map(|team| team as i32))
                    .or_else(|| net.remote_actor_teams.get(player_id).copied())
            })
            .unwrap_or(0);
    }
    mission_npc
        .map(|npc| i32::from(npc.team))
        .unwrap_or(if local { 1 } else { 0 })
}

fn teams_are_hostile(a: i32, b: i32) -> bool {
    (team_mask(a) & team_mask(b)) == 0
}

type DeathRespawnQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static mut DeathRespawn,
        &'static mut Health,
        &'static mut Qi,
        &'static mut CombatState,
        &'static mut PoseRuntime,
        &'static mut SmState,
        &'static mut Transform,
        &'static mut crate::components::fighter::TargetHitClock,
        (
            &'static mut EntryBlend,
            &'static mut MotionState,
            &'static mut FighterAir,
            &'static mut FighterDodge,
            &'static mut FighterAttackMotion,
            &'static mut FighterHitReaction,
            &'static mut GunAimState,
            Option<&'static mut AiIntent>,
            Option<&'static mut FighterWall>,
        ),
        Option<&'static MissionNpc>,
        Option<&'static LocalFighter>,
    ),
    (With<Fighter>, Without<RemoteFighter>),
>;

#[allow(clippy::too_many_arguments)]
pub(crate) fn reset_fighter_runtime_for_respawn(
    character_assets: Option<&CharacterAssets>,
    combat: &mut CombatState,
    pose: &mut PoseRuntime,
    blend: &mut EntryBlend,
    sm: &mut SmState,
    motion: &mut MotionState,
    air: &mut FighterAir,
    dodge: &mut FighterDodge,
    attack_motion: &mut FighterAttackMotion,
    hit_reaction: &mut FighterHitReaction,
    gun_aim: &mut GunAimState,
    ai_intent: Option<&mut AiIntent>,
    wall: Option<&mut FighterWall>,
) {
    *combat = CombatState::default();
    *blend = EntryBlend::default();
    if let Some(character_assets) = character_assets.filter(|assets| assets.poses.contains_key(&0))
    {
        character_assets.apply_pose_with_blend(
            pose,
            blend,
            0,
            true,
            None,
            EntryBlendKind::Generic,
            false,
            false,
            None,
        );
    } else {
        let next_instance = pose.pose_instance_id.wrapping_add(1);
        *pose = PoseRuntime::idle();
        pose.pose_instance_id = next_instance;
    }
    *sm = SmState::default();
    motion.movement_mode = meteor_core::MovementMode::Idle;
    motion.velocity = Vec3::ZERO;
    *air = FighterAir::default();
    *dodge = FighterDodge::default();
    *attack_motion = FighterAttackMotion::default();
    hit_reaction.clear();
    *gun_aim = GunAimState::default();
    if let Some(intent) = ai_intent {
        *intent = AiIntent::default();
    }
    if let Some(wall) = wall {
        *wall = FighterWall::default();
    }
}

#[derive(bevy::ecs::system::SystemParam)]
struct CombatNetwork<'w> {
    #[cfg(not(target_arch = "wasm32"))]
    stock_anger: Option<ResMut<'w, crate::plugins::network_plugin::PendingStockAnger>>,
    net_tx: Option<Res<'w, crate::plugins::network_plugin::NetTx>>,
    net: Res<'w, NetState>,
    pending_actions: ResMut<'w, PendingNetActions>,
    camera_basis: ResMut<'w, crate::plugins::camera_plugin::CameraBasis>,
    guard_input: Option<Res<'w, LocalGuardInput>>,
    crouch_input: Option<Res<'w, LocalCrouchInput>>,
    combat_input: Option<Res<'w, LocalCombatInput>>,
    desktop_dodge: Option<ResMut<'w, DesktopDodgeInput>>,
    jump_input: Option<ResMut<'w, LocalJumpInput>>,
    common_amb: Option<Res<'w, CommonAmb>>,
}

fn report_committed_anger(
    network: &mut CombatNetwork<'_>,
    entity: Entity,
    local: bool,
    generation: u32,
    spend: Option<(u16, u16)>,
) {
    let Some((action_id, cost)) = spend else {
        return;
    };
    if network.net.status != NetStatus::InRoom {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if network.net.is_stock_udp() {
        if local || (network.net.is_stock_host() && network.net.stock_npc_ready.contains(&entity)) {
            if let (Some(tx), Some(ledger)) = (
                network.net_tx.as_deref(),
                network.stock_anger.as_deref_mut(),
            ) {
                ledger.send(tx, (!local).then_some(entity), generation, action_id, cost);
            }
        }
        return;
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (entity, &network.net_tx);
    if local {
        if action_id == QI_BURST_ACTION_ID {
            network.pending_actions.push("qi_burst");
        } else if let Some(action) =
            meteor_network::protocol::encode_anger_spend_action(generation, action_id, cost)
        {
            network.pending_actions.push(action);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn combat_tick(
    mut commands: Commands,
    (active_fighter, time): (Option<Res<super::fighter_frame::ActiveFighter>>, Res<Time>),
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keymap: Res<crate::app_state::Keymap>,
    settings: Res<Settings>,
    mut network: CombatNetwork<'_>,
    player_performance: Option<Res<crate::plugins::stage_plugin::PlayerPerformance>>,
    assets: Option<Res<CharacterAssetsSet>>,
    mut controller: ResMut<LockOnController>,
    mut command_input: ResMut<crate::plugins::input_plugin::LocalCommandInput>,
    switching: Query<(), With<WeaponSwitchActive>>,
    ranged_gates: Query<&RangedFireGate>,
    remotes: RemoteLockCandidateQuery<'_, '_>,
    mut query: CombatTickQuery<'_, '_>,
) {
    let Some(assets) = assets else {
        return;
    };
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let local_guard_held = network
        .guard_input
        .as_deref()
        .is_some_and(|guard| guard.held);
    let local_crouch_held = network
        .crouch_input
        .as_deref()
        .is_some_and(|crouch| crouch.held);

    let mut all_candidates: Vec<OrderedLockCandidate> = Vec::new();
    for (
        other,
        _fighter,
        _,
        _,
        _,
        health,
        _,
        _,
        _,
        _,
        _,
        other_transform,
        local,
        _,
        (_, _, _, mission, visibility, _, _),
    ) in query.iter()
    {
        if health.current <= 0.0
            || visibility.is_some_and(|visibility| *visibility == Visibility::Hidden)
        {
            continue;
        }
        let team = fighter_combat_team(&network.net, local.is_some(), None, mission);
        all_candidates.push(OrderedLockCandidate {
            entity: other,
            candidate: LockCandidate {
                id: other.to_bits(),
                x: other_transform.translation.x,
                y: other_transform.translation.y,
                z: other_transform.translation.z,
                team,
            },
        });
    }
    all_candidates.extend(remotes.iter().filter_map(
        |(entity, health, transform, remote, visibility)| {
            if health.current <= 0.0
                || visibility.is_some_and(|visibility| *visibility == Visibility::Hidden)
            {
                return None;
            }
            let team = fighter_combat_team(&network.net, false, Some(remote), None);
            Some(OrderedLockCandidate {
                entity,
                candidate: LockCandidate {
                    id: entity.to_bits(),
                    x: transform.translation.x,
                    y: transform.translation.y,
                    z: transform.translation.z,
                    team,
                },
            })
        },
    ));
    all_candidates.sort_by_key(|candidate| candidate.entity.to_bits());
    for (
        entity,
        fighter,
        mut pose,
        mut blend,
        mut combat,
        mut health,
        mut qi,
        mut sm_state,
        death,
        air,
        motion,
        transform,
        local,
        mut gun_aim,
        (
            mut intent,
            weapon_form_state,
            mut attack_motion,
            mission,
            _visibility,
            mut hit_reaction,
            npc_performance,
        ),
    ) in query.iter_mut()
    {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        let strategy_id = weapon_to_strategy(fighter.weapon);
        if death.active
            || air.air.airborne
            || strategy_id != meteor_physics::locomotion::FIREARM_STRATEGY
        {
            gun_aim.aiming = false;
        }

        if death.active {
            continue;
        }
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };

        combat.state = sm_state.current;
        let is_local = local.is_some();
        let input_blocked = player_performance
            .as_deref()
            .is_some_and(|performance| performance.input_blocked);
        let accept_local_input = is_local && !input_blocked;
        let raw_bindings = if is_local {
            network
                .combat_input
                .as_deref()
                .map(|input| input.raw_bindings.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let command_update_ms = network
            .combat_input
            .as_deref()
            .and_then(|input| input.command_update_ms);
        let initial_crouch_held = network
            .combat_input
            .as_deref()
            .map_or(local_crouch_held, |input| input.initial_crouch_held);
        let ordered_actions = network
            .combat_input
            .as_deref()
            .map(|input| input.edges.clone())
            .unwrap_or_default();
        let has_ordered_input = !ordered_actions.is_empty();
        let saw_raw_binding = |binding| raw_bindings.contains(&binding);
        let attack_pressed = accept_local_input
            && !saw_raw_binding(keymap.attack)
            && keymap.attack.just_pressed(&keys, &mouse);
        let switch_active = switching.contains(entity);
        let switch_pressed = accept_local_input
            && !saw_raw_binding(keymap.weapon_switch)
            && keymap.weapon_switch.just_pressed(&keys, &mouse);
        let weapon_action_vetoed = ranged_gates.get(entity).is_ok_and(|gate| gate.wheel_out);
        let npc_attack_requested = !is_local && intent.want_attack;
        let actor_team = fighter_combat_team(&network.net, is_local, None, mission);
        let candidates: Vec<LockCandidate> = all_candidates
            .iter()
            .filter(|ordered| {
                ordered.entity != entity && teams_are_hostile(actor_team, ordered.candidate.team)
            })
            .map(|ordered| ordered.candidate)
            .collect();
        let burst_pressed = accept_local_input
            && !saw_raw_binding(keymap.qi_burst)
            && keymap.qi_burst.just_pressed(&keys, &mouse);
        let fake_pressed = accept_local_input
            && !saw_raw_binding(keymap.fake)
            && keymap.fake.just_pressed(&keys, &mouse);
        let jump_released = accept_local_input
            && !saw_raw_binding(keymap.jump)
            && network.jump_input.as_deref().map_or_else(
                || keymap.jump.just_released(&keys, &mouse),
                |jump| jump.released,
            );
        let weapon_form = weapon_form_state.current;
        let guard_held = actor_guard_held(
            is_local,
            local_guard_held,
            npc_performance.map(|performance| performance.guard_hold()),
        );

        let npc_aim_consumed = npc_attack_requested
            && matches!(
                sm_state.current,
                FighterState::Name | FighterState::Dodge | FighterState::Burst
            )
            && resolve_affordable_weapon_command(
                fighter.weapon,
                &intent.attack_directions,
                false,
                weapon_form_state.current,
                |cost| qi.current >= cost as f32,
            )
            .is_some_and(|resolved| {
                gun_aim_intercepts(
                    strategy_id,
                    gun_aim.aiming,
                    air.air.airborne,
                    resolved.pose_id,
                )
            });
        if npc_aim_consumed {
            intent.want_attack = false;
            gun_aim.aiming = true;
            if character_assets.poses.contains_key(&GUN_AIM_READY_POSE) {
                character_assets.apply_pose_with_blend(
                    &mut pose,
                    &mut blend,
                    GUN_AIM_READY_POSE,
                    true,
                    None,
                    EntryBlendKind::Generic,
                    true,
                    false,
                    Some(strategy_id),
                );
            }
        }
        let mut anger_spend = None;
        let view = CombatView {
            pose: &mut pose,
            blend: &mut blend,
            combat: &mut combat,
            health: &mut health,
            qi: &mut qi,
            air,
            assets: character_assets,
            weapon: fighter.weapon,
            guard_held_input: guard_held,
            in_guard_hit_reaction: hit_reaction
                .current
                .as_ref()
                .is_some_and(|reaction| reaction.kind == HitReactKind::Guard),
            attack_directions: if is_local {
                Vec::new()
            } else {
                std::mem::take(&mut intent.attack_directions)
            },
            weapon_form,
            anger_spend: &mut anger_spend,
            strategy_id,
            weapon_action_vetoed,
        };

        let mut sm = StateMachine::new(view);

        sm.current = sm_state.current;
        sm.previous = sm_state.previous;
        sm.time_in_state = sm_state.time_in_state;
        sm.locked = sm_state.locked;
        sm.attack_stage = sm_state.attack_stage;
        sm.attack_queued = sm_state.attack_queued.clone();
        sm.linked_recovery_pose_id = sm_state.linked_recovery_pose_id;
        sm.exe_ground_mode_ticks = sm_state.exe_ground_mode_ticks;

        let mut attack_commit_accepted = false;
        let mut switch_started = false;
        if is_local {
            let down_has_direction =
                !input_blocked && directional_input(&keys, &mouse, &keymap).length_squared() > 0.01;
            let mut event_crouch_held = initial_crouch_held;
            if has_ordered_input {
                for edge in ordered_actions {
                    match edge {
                        LocalCombatInputEdge::DirectionPress(direction) => {
                            command_input.0.push_direction(
                                direction,
                                command_update_ms
                                    .unwrap_or_else(|| time.elapsed_secs_f64() * 1_000.0),
                            );
                        }
                        LocalCombatInputEdge::DirectionRelease(direction) => {
                            command_input.0.release_direction(direction);
                        }
                        LocalCombatInputEdge::Dodge { direction, burst } => {
                            if !accept_local_input
                                || switch_active
                                || switch_started
                                || hit_reaction.current.is_some()
                                || matches!(sm.current, FighterState::BeHit | FighterState::Down)
                            {
                                continue;
                            }
                            let roll = dodge_roll_at_event(
                                burst,
                                event_crouch_held,
                                gun_aim.aiming,
                                sm.current,
                            );
                            let accepted =
                                sm.handle_input(if burst { Action::Burst } else { Action::Dodge });
                            if accepted {
                                publish_accepted_dodge(
                                    network.desktop_dodge.as_deref_mut(),
                                    dodge_direction(direction),
                                    burst,
                                    roll,
                                );
                            }
                        }
                        LocalCombatInputEdge::DirectDodge { burst } => {
                            if !accept_local_input
                                || switch_active
                                || switch_started
                                || hit_reaction.current.is_some()
                                || matches!(sm.current, FighterState::BeHit | FighterState::Down)
                            {
                                continue;
                            }
                            let raw = directional_input(&keys, &mouse, &keymap);
                            let direction = if raw.x != 0.0 || raw.z != 0.0 {
                                Vec2::new(raw.x, raw.z)
                            } else {
                                Vec2::Y
                            };
                            let roll = dodge_roll_at_event(
                                burst,
                                event_crouch_held,
                                gun_aim.aiming,
                                sm.current,
                            );
                            let accepted =
                                sm.handle_input(if burst { Action::Burst } else { Action::Dodge });
                            if accepted {
                                publish_accepted_dodge(
                                    network.desktop_dodge.as_deref_mut(),
                                    direction,
                                    burst,
                                    roll,
                                );
                            }
                        }
                        LocalCombatInputEdge::CrouchPress => {
                            if !input_blocked {
                                event_crouch_held = true;
                                sm.handle_input(Action::Crouch);
                            }
                        }
                        LocalCombatInputEdge::CrouchRelease => {
                            event_crouch_held = false;
                            sm.handle_input(Action::CrouchRelease);
                        }
                        LocalCombatInputEdge::Attack => {
                            if !accept_local_input || switch_active || switch_started {
                                continue;
                            }
                            if attack_input_reaches_auto_lock(
                                true,
                                false,
                                hit_reaction.current.is_some(),
                                sm.current,
                            ) && fighter.weapon != 0
                            {
                                acquire_local_attack_lock(
                                    &mut controller,
                                    settings.auto_camera,
                                    strategy_id,
                                    transform.translation,
                                    motion.facing,
                                    &candidates,
                                );
                            }
                            let (accepted, _aim_consumed) =
                                dispatch_local_attack(&mut sm, &mut command_input, &mut gun_aim);
                            attack_commit_accepted |= accepted;
                            if !accepted && fighter.weapon == 0 {
                                warn!(
                                    "attack ignored: weapon 0 (空手) has no command table; \
                                     press E to switch to 匕首 or select a weapon in the launcher"
                                );
                            }
                        }
                        LocalCombatInputEdge::GuardPress => {
                            if !input_blocked
                                && !(matches!(sm.current, FighterState::Down | FighterState::Fake)
                                    && down_has_direction)
                            {
                                sm.handle_input(Action::Guard);
                            }
                        }
                        LocalCombatInputEdge::GuardRelease => {
                            let guard_reaction_active = hit_reaction
                                .current
                                .as_ref()
                                .is_some_and(|reaction| reaction.kind == HitReactKind::Guard);
                            if !guard_reaction_active {
                                sm.handle_input(Action::GuardRelease);
                            }
                        }
                        LocalCombatInputEdge::LockToggle => {
                            dispatch_local_lock_toggle(
                                &mut controller,
                                &mut network.camera_basis,
                                &mut gun_aim,
                                strategy_id,
                                transform.translation,
                                motion.facing,
                                &candidates,
                            );
                        }
                        LocalCombatInputEdge::WeaponSwitch => {
                            if !accept_local_input || switch_started {
                                continue;
                            }
                            if let Some(active) = begin_local_weapon_switch(
                                &mut sm,
                                switch_active,
                                gun_aim.aiming,
                                fighter,
                                motion,
                                &attack_motion,
                                network.common_amb.as_deref(),
                            ) {
                                commands.entity(entity).insert(active);
                                switch_started = true;
                            }
                        }
                        LocalCombatInputEdge::QiBurst => {
                            if accept_local_input && !switch_active && !switch_started {
                                attack_commit_accepted |= sm.handle_input(Action::QiBurst);
                            }
                        }
                        LocalCombatInputEdge::Fake => {
                            if accept_local_input && !switch_active && !switch_started {
                                sm.handle_input(Action::Fake);
                            }
                        }
                        LocalCombatInputEdge::Jump => {
                            if accept_local_input && !switch_active && !switch_started {
                                let accepted = sm.handle_input(Action::Jump);
                                if accepted {
                                    if let Some(jump) = network.jump_input.as_deref_mut() {
                                        jump.action_accepted = true;
                                    }
                                }
                                let _ = release_reaction_after_accepted_jump(
                                    &mut hit_reaction,
                                    accepted,
                                    sm.current,
                                );
                            }
                        }
                        LocalCombatInputEdge::Cure => {
                            if accept_local_input && !switch_active && !switch_started {
                                sm.handle_input(Action::Cure);
                            }
                        }

                        LocalCombatInputEdge::DropWeapon | LocalCombatInputEdge::DropItem => {}
                    }
                }
            }

            let fallback_switch_pose = weapon_switch_context_pose(
                sm.current,
                sm.linked_recovery_pose_id.is_some(),
                air.air.airborne,
                air.air.motion_mode,
            );
            let fallback_switch_will_start = weapon_switch_entry_allowed(
                switch_pressed,
                switch_active || switch_started,
                gun_aim.aiming,
                weapon_action_vetoed,
                fallback_switch_pose,
                fallback_switch_pose.is_some_and(|pose| character_assets.poses.contains_key(&pose)),
            );
            if attack_input_reaches_auto_lock(
                attack_pressed,
                switch_active || switch_started || fallback_switch_will_start,
                hit_reaction.current.is_some(),
                sm.current,
            ) && fighter.weapon != 0
            {
                acquire_local_attack_lock(
                    &mut controller,
                    settings.auto_camera,
                    strategy_id,
                    transform.translation,
                    motion.facing,
                    &candidates,
                );
            }
            let lock_pressed = !saw_raw_binding(keymap.lock_toggle)
                && keymap.lock_toggle.just_pressed(&keys, &mouse);
            if lock_pressed {
                dispatch_local_lock_toggle(
                    &mut controller,
                    &mut network.camera_basis,
                    &mut gun_aim,
                    strategy_id,
                    transform.translation,
                    motion.facing,
                    &candidates,
                );
            }
            if fallback_switch_will_start {
                if let Some(active) = begin_local_weapon_switch(
                    &mut sm,
                    switch_active || switch_started,
                    gun_aim.aiming,
                    fighter,
                    motion,
                    &attack_motion,
                    network.common_amb.as_deref(),
                ) {
                    commands.entity(entity).insert(active);
                    switch_started = true;
                }
            }
            if accept_local_input && !switch_active && !switch_started && attack_pressed {
                let (accepted, _aim_consumed) =
                    dispatch_local_attack(&mut sm, &mut command_input, &mut gun_aim);
                attack_commit_accepted |= accepted;
                if !accepted && fighter.weapon == 0 {
                    warn!(
                        "attack ignored: weapon 0 (空手) has no command table; \
                         press E to switch to 匕首 or select a weapon in the launcher"
                    );
                }
            }
            if !saw_raw_binding(keymap.guard)
                && !input_blocked
                && keymap.guard.just_pressed(&keys, &mouse)
                && !(matches!(sm.current, FighterState::Down | FighterState::Fake)
                    && down_has_direction)
            {
                sm.handle_input(Action::Guard);
            }
            let guard_reaction_active = hit_reaction
                .current
                .as_ref()
                .is_some_and(|reaction| reaction.kind == HitReactKind::Guard);
            if !saw_raw_binding(keymap.guard)
                && keymap.guard.just_released(&keys, &mouse)
                && !guard_reaction_active
            {
                sm.handle_input(Action::GuardRelease);
            }
            if accept_local_input && !switch_active && !switch_started {
                if burst_pressed {
                    attack_commit_accepted |= sm.handle_input(Action::QiBurst);
                }
                if fake_pressed {
                    sm.handle_input(Action::Fake);
                }
                if jump_released {
                    let accepted = sm.handle_input(Action::Jump);
                    if accepted {
                        if let Some(jump) = network.jump_input.as_deref_mut() {
                            jump.action_accepted = true;
                        }
                    }

                    let _ = release_reaction_after_accepted_jump(
                        &mut hit_reaction,
                        accepted,
                        sm.current,
                    );
                }
                if !saw_raw_binding(keymap.cure) && keymap.cure.just_pressed(&keys, &mouse) {
                    sm.handle_input(Action::Cure);
                }
            }

            if !input_blocked
                && matches!(sm.current, FighterState::Down | FighterState::Fake)
                && guard_held
                && !down_has_direction
            {
                sm.handle_input(Action::Guard);
            }
            if let Some(now_ms) = command_update_ms {
                command_input.0.update(now_ms);
            }
        } else if !is_local {
            if std::mem::take(&mut intent.want_qi_burst) {
                attack_commit_accepted |= sm.handle_input(Action::QiBurst);
            }
            if sm.current == FighterState::Guard && !guard_held {
                sm.handle_input(Action::GuardRelease);
            }
            if intent.want_guard {
                intent.want_guard = false;
                sm.handle_input(Action::Guard);
            }
            if std::mem::take(&mut intent.want_taunt) {
                sm.taunt();
            }
            if intent.want_attack && !npc_aim_consumed {
                intent.want_attack = false;
                attack_commit_accepted |= sm.handle_input(Action::Attack);
            }
        }

        if guard_held
            && (!is_local || !input_blocked)
            && sm.current == FighterState::Name
            && !air.air.airborne
            && !switch_active
            && !switch_started
            && hit_reaction.current.is_none()
            && !weapon_action_vetoed
        {
            sm.handle_input(Action::Guard);
        }
        if attack_commit_accepted {
            bank_attack_commit_motion(
                &mut attack_motion,
                motion.velocity,
                is_local,
                fighter.weapon,
            );
        }

        sm.update(dt);
        let mut projected = project_state_machine(&sm);
        drop(sm);
        let _ = install_requested_getup_reaction(
            &mut hit_reaction,
            &mut projected,
            &pose,
            air.air.airborne,
        );
        report_committed_anger(
            &mut network,
            entity,
            is_local,
            pose.pose_instance_id,
            anger_spend,
        );
        *sm_state = projected;
        combat.state = sm_state.current;
        if hit_reaction.current.is_some()
            && !matches!(
                sm_state.current,
                FighterState::BeHit | FighterState::Down | FighterState::Guard | FighterState::Dead
            )
        {
            hit_reaction.clear();
        }
    }
}

fn sync_attacks_after_animation(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    assets: Option<Res<CharacterAssetsSet>>,
    mut network: CombatNetwork<'_>,
    mut query: CombatTickQuery<'_, '_>,
) {
    let Some(assets) = assets else {
        return;
    };
    let local_guard_held = network
        .guard_input
        .as_deref()
        .is_some_and(|guard| guard.held);
    for (
        entity,
        fighter,
        mut pose,
        mut blend,
        mut combat,
        mut health,
        mut qi,
        mut sm_state,
        death,
        air,
        _motion,
        _transform,
        local,
        _gun_aim,
        (
            _intent,
            weapon_form_state,
            mut attack_motion,
            _mission,
            _visibility,
            _hit_reaction,
            npc_performance,
        ),
    ) in query.iter_mut()
    {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        if death.active
            || (!is_attacking_state(sm_state.current) && sm_state.attack_queued.is_none())
        {
            continue;
        }
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        combat.state = sm_state.current;
        let weapon_form = weapon_form_state.current;
        let mut anger_spend = None;
        let view = CombatView {
            pose: &mut pose,
            blend: &mut blend,
            combat: &mut combat,
            health: &mut health,
            qi: &mut qi,
            air,
            assets: character_assets,
            weapon: fighter.weapon,
            guard_held_input: actor_guard_held(
                local.is_some(),
                local_guard_held,
                npc_performance.map(|performance| performance.guard_hold()),
            ),
            in_guard_hit_reaction: false,
            attack_directions: Vec::new(),
            weapon_form,
            anger_spend: &mut anger_spend,
            strategy_id: weapon_to_strategy(fighter.weapon),
            weapon_action_vetoed: false,
        };
        let mut sm = StateMachine::new(view);
        sm.current = sm_state.current;
        sm.previous = sm_state.previous;
        sm.time_in_state = sm_state.time_in_state;
        sm.locked = sm_state.locked;
        sm.attack_stage = sm_state.attack_stage;
        sm.attack_queued = sm_state.attack_queued.clone();
        sm.linked_recovery_pose_id = sm_state.linked_recovery_pose_id;
        sm.exe_ground_mode_ticks = sm_state.exe_ground_mode_ticks;
        let was_attacking = is_attacking_state(sm.current);
        sm.sync_after_animation();
        let is_attacking = is_attacking_state(sm.current);
        let projected = project_state_machine(&sm);
        drop(sm);
        let _ = release_attack_motion_after_pose_resolve(
            &mut attack_motion,
            was_attacking,
            is_attacking,
        );
        report_committed_anger(
            &mut network,
            entity,
            local.is_some(),
            pose.pose_instance_id,
            anger_spend,
        );
        *sm_state = projected;
        combat.state = sm_state.current;
    }
}

const REACTION_ROOT_MAX_SUBSTEP: f32 = 0.5;
const REACTION_LAND_EPS: f32 = 0.005;

fn advance_reaction_carry(
    runtime: &mut FighterHitReaction,
    air: &FighterAir,
    dt: f32,
) -> (f32, f32) {
    if dt <= 0.0 || !dt.is_finite() {
        return (0.0, 0.0);
    }
    let decay = super::movement_plugin::action_carry_decay(air);
    let k = (1.0 - decay * dt).max(0.0);
    runtime.carry_x *= k;
    runtime.carry_z *= k;
    (runtime.carry_x * dt, runtime.carry_z * dt)
}

fn advance_down_carry(runtime: &mut FighterHitReaction, air: &FighterAir, dt: f32) -> (f32, f32) {
    if dt <= 0.0 || !dt.is_finite() {
        return (0.0, 0.0);
    }
    let (carry_x, carry_z) = advance_reaction_carry(runtime, air, dt);
    let dx = carry_x + air.ground_external_velocity.x * dt;
    let dz = carry_z + air.ground_external_velocity.z * dt;
    let (dx, _, dz, _) = clamp_composed_delta_3d(dx, 0.0, dz, dt, CARRY_MAX_SPEED);
    (dx, dz)
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
fn hit_reaction_root_motion(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    assets: Option<Res<CharacterAssetsSet>>,
    common_amb: Option<Res<crate::plugins::character_plugin::CommonAmb>>,
    collider: Res<StageCollider>,
    mut contacts: Option<ResMut<CharacterContactFrame>>,
    mut query: Query<(
        Entity,
        &Fighter,
        &PoseRuntime,
        Option<&EntryBlend>,
        &mut Transform,
        &mut MotionState,
        &mut FighterAir,
        &mut FighterHitReaction,
        Option<&RemoteFighter>,
        Option<&mut RemoteTransformTarget>,
        Option<&mut RemoteHitMotionWindow>,
    )>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    if !dt.is_finite() || dt <= 0.0 {
        return;
    }
    let (Some(assets), Some(common_amb), Some(mesh)) = (assets, common_amb, collider.mesh.as_ref())
    else {
        return;
    };
    let mut collision =
        meteor_physics::collision::CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    collision.set_mesh_collider(mesh);

    for (
        entity,
        fighter,
        pose,
        blend,
        mut transform,
        mut motion,
        mut air,
        mut runtime,
        remote,
        mut remote_target,
        mut remote_window,
    ) in &mut query
    {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        let Some(reaction) = runtime.current.as_ref() else {
            continue;
        };
        if remote.is_some() {
            let Some(window) = remote_window.as_deref_mut() else {
                runtime.clear();
                continue;
            };
            window.remaining = (window.remaining - dt.max(0.0)).max(0.0);
            if window.remaining <= 0.0 {
                runtime.clear();
                continue;
            }
        }

        if reaction.kind == HitReactKind::Down {
            let (tx, tz) = advance_down_carry(&mut runtime, &air, dt);
            let mut accepted_x = 0.0;
            let mut accepted_y = 0.0;
            let mut accepted_z = 0.0;
            if tx.abs() > 1e-9 || tz.abs() > 1e-9 {
                let pos = meteor_physics::collision::Point3::new(
                    transform.translation.x,
                    transform.translation.y,
                    transform.translation.z,
                );
                let (rx, rz, ry) = resolve_fighter_xz(
                    contacts.as_deref_mut(),
                    entity,
                    &collision,
                    pos,
                    tx,
                    tz,
                    false,
                );
                transform.translation.x += rx;
                transform.translation.z += rz;
                transform.translation.y += ry;
                accepted_x = rx;
                accepted_y = ry;
                accepted_z = rz;
                if rx == 0.0 && tx != 0.0 {
                    runtime.carry_x = 0.0;
                }
                if rz == 0.0 && tz != 0.0 {
                    runtime.carry_z = 0.0;
                }
                commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);
            }
            if let Some(target) = remote_target.as_deref_mut() {
                target.translation.x += accepted_x;
                target.translation.y += accepted_y;
                target.translation.z += accepted_z;
            }
            motion.velocity = if dt > 0.0 {
                Vec3::new(accepted_x / dt, 0.0, accepted_z / dt)
            } else {
                Vec3::ZERO
            };
            continue;
        }
        let motion_scale = if reaction.motion_scale.is_finite() {
            reaction.motion_scale
        } else {
            1.0
        };
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        let sampled = character_assets
            .amb_for_pose(&common_amb.0, pose.pose_id)
            .sample_at(pose.frame);
        let root = character_assets
            .dummy_order
            .get("d_base")
            .and_then(|index| sampled.dummy_pos.get(*index))
            .copied()
            .unwrap_or(sampled.root_pivot);

        if !runtime.root_started {
            if runtime
                .root_expected_pose_id
                .is_some_and(|id| id != pose.pose_id)
            {
                continue;
            }
            runtime.root_prev = Some(root);
            runtime.root_pose_id = Some(pose.pose_id);
            runtime.root_started = true;
            runtime.airborne |= air.air.airborne;
        }
        if runtime.root_pose_id != Some(pose.pose_id) {
            runtime.root_pose_id = Some(pose.pose_id);
            runtime.root_expected_pose_id = Some(pose.pose_id);
            runtime.root_prev = Some(root);
        }
        runtime.root_prev = Some(root);
        let delta =
            if blend.is_some_and(|blend| blend.duration > 0.0 && blend.pose_id == pose.pose_id) {
                [0.0; 3]
            } else {
                character_assets.authored_root_step(&common_amb.0, pose, dt)
            };
        let [dx, dy, dz] = delta.map(|value| value * motion_scale);
        let (world_x, world_z) =
            meteor_anim::root_motion::root_delta_to_world_xz(dx, dy, motion.facing, 1.0);
        let (carry_x, carry_z) = advance_reaction_carry(&mut runtime, &air, dt);
        let world_x = world_x + carry_x + air.ground_external_velocity.x * dt;
        let world_z = world_z + carry_z + air.ground_external_velocity.z * dt;
        air.ground_external_velocity = Vec3::ZERO;

        let entry_ground = collision
            .sample_ground_hit(
                transform.translation.x,
                transform.translation.z,
                transform.translation.y,
            )
            .map_or(f32::NEG_INFINITY, |hit| hit.y);
        let entry_ground = if runtime.airborne && entry_ground > transform.translation.y {
            runtime.ground_y.unwrap_or(entry_ground)
        } else {
            entry_ground
        };
        let was_airborne =
            runtime.airborne || transform.translation.y > entry_ground + REACTION_LAND_EPS;
        let anim_speed_source = if dt > 0.0 { dz / dt } else { 0.0 };
        let anim_driven =
            anim_speed_source.abs() >= meteor_combat::hit_reaction::HIT_REACTION_ANIM_MIN_SPEED;
        let requested_y = if anim_driven {
            runtime.vertical_velocity = anim_speed_source * meteor_core::MODEL_SCALE;
            dz * meteor_core::MODEL_SCALE
        } else if was_airborne && dt > 0.0 {
            runtime.vertical_velocity += meteor_combat::hit_reaction::HIT_REACTION_GRAVITY * dt;
            runtime.vertical_velocity * dt
        } else {
            runtime.vertical_velocity = 0.0;
            0.0
        };
        let (world_x, mut requested_y, world_z, scale) =
            clamp_composed_delta_3d(world_x, requested_y, world_z, dt, CARRY_MAX_SPEED);
        runtime.vertical_velocity *= scale;

        let distance = world_x.hypot(world_z);
        let steps = (distance / REACTION_ROOT_MAX_SUBSTEP).ceil().max(1.0) as usize;
        let before_y = transform.translation.y;
        let mut planar_surface = None;
        let mut accepted_x = 0.0;
        let mut accepted_z = 0.0;
        for _ in 0..steps {
            let pos = meteor_physics::collision::Point3::new(
                transform.translation.x,
                transform.translation.y,
                transform.translation.z,
            );
            let (rx, rz, ry, swept) =
                super::character_contact_plugin::resolve_fighter_xz_with_surface(
                    contacts.as_deref_mut(),
                    entity,
                    &collision,
                    pos,
                    world_x / steps as f32,
                    world_z / steps as f32,
                    false,
                );
            planar_surface = swept.or(planar_surface);
            transform.translation.x += rx;
            transform.translation.z += rz;

            transform.translation.y += ry;
            accepted_x += rx;
            accepted_z += rz;
        }

        if world_x != 0.0 && accepted_x == 0.0 {
            runtime.carry_x = 0.0;
        }
        if world_z != 0.0 && accepted_z == 0.0 {
            runtime.carry_z = 0.0;
        }
        let mut accepted_y = 0.0;
        if dt > 0.0 {
            let sampled_ground = collision
                .sample_ground_hit(
                    transform.translation.x,
                    transform.translation.z,
                    transform.translation.y,
                )
                .map_or(f32::NEG_INFINITY, |hit| hit.y);
            let ground_y = if runtime.airborne
                && runtime.ground_y.is_some()
                && sampled_ground > transform.translation.y
            {
                runtime.ground_y.unwrap_or(sampled_ground)
            } else {
                sampled_ground
            };
            runtime.ground_y = ground_y.is_finite().then_some(ground_y);
            let pos = meteor_physics::collision::Point3::new(
                transform.translation.x,
                transform.translation.y,
                transform.translation.z,
            );
            let mut mover_floor_contact = false;
            let vertical_request = requested_y;
            let mut carry_surface = None;
            if requested_y > 0.0 {
                let (resolved, hit, blocked) = collision.resolve_y(pos, requested_y);
                carry_surface = hit.map(|hit| (hit.nx, hit.ny, hit.nz));
                requested_y = resolved;
                if hit.is_some() || blocked {
                    runtime.vertical_velocity = runtime.vertical_velocity.min(0.0);
                }
            } else if requested_y < 0.0 {
                let (resolved, hit, blocked) =
                    collision.resolve_descent_with_surface(pos, requested_y, planar_surface);
                carry_surface = hit.map(|hit| (hit.nx, hit.ny, hit.nz));
                super::movement_plugin::update_descent_slide(
                    &mut air,
                    hit,
                    runtime.vertical_velocity,
                    dt,
                );
                requested_y = resolved;
                if hit.is_some() || blocked {
                    runtime.vertical_velocity = runtime.vertical_velocity.max(0.0);
                }
                mover_floor_contact = meteor_physics::collision::descent_has_support(hit, blocked);
            }
            super::movement_plugin::record_carry_support(
                &mut air,
                vertical_request,
                carry_surface,
                planar_surface.is_some()
                    || (accepted_x - world_x).abs() > 1e-5
                    || (accepted_z - world_z).abs() > 1e-5,
            );
            let room = transform.translation.y - ground_y;
            if requested_y < -room {
                requested_y = -room;
            }
            transform.translation.y = (transform.translation.y + requested_y).max(ground_y);
            accepted_y = requested_y;

            if transform.translation.y > ground_y + REACTION_LAND_EPS && !mover_floor_contact {
                runtime.airborne = true;
                air.air.airborne = true;
                air.ground_contact = false;
                air.air.vertical_speed = runtime.vertical_velocity;
            } else if was_airborne && requested_y <= 0.0 {
                runtime.airborne = false;
                runtime.vertical_velocity = 0.0;
                air.air.airborne = false;
                air.ground_contact = true;
                air.carry_no_support = false;
                air.air.vertical_speed = 0.0;
            }
        }
        motion.velocity = if dt > 0.0 {
            Vec3::new(accepted_x / dt, accepted_y / dt, accepted_z / dt)
        } else {
            Vec3::ZERO
        };
        if let Some(target) = remote_target.as_deref_mut() {
            target.translation +=
                Vec3::new(accepted_x, transform.translation.y - before_y, accepted_z);
        }
        commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);
    }
}

#[allow(clippy::type_complexity)]
fn tick_hit_reactions(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    guard_input: Option<Res<LocalGuardInput>>,
    assets: Option<Res<CharacterAssetsSet>>,
    mut query: Query<
        (
            Entity,
            &Fighter,
            &mut PoseRuntime,
            &mut EntryBlend,
            &mut CombatState,
            &mut Health,
            &mut Qi,
            &mut SmState,
            &FighterAir,
            &WeaponForm,
            &mut FighterHitReaction,
            Option<&LocalFighter>,
            Option<&NpcPerformance>,
        ),
        Without<RemoteFighter>,
    >,
) {
    let Some(assets) = assets else {
        return;
    };
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let local_guard_held = guard_input.as_deref().is_some_and(|guard| guard.held);
    for (
        entity,
        fighter,
        mut pose,
        mut blend,
        mut combat,
        mut health,
        mut qi,
        mut sm,
        air,
        weapon_form,
        mut runtime,
        local,
        npc_performance,
    ) in &mut query
    {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        let Some(mut reaction) = runtime.current.take() else {
            continue;
        };
        let owns_state = match reaction.kind {
            HitReactKind::Guard => sm.current == FighterState::Guard,
            HitReactKind::BeHit | HitReactKind::GetUp => sm.current == FighterState::BeHit,
            HitReactKind::Down => sm.current == FighterState::Down,
            HitReactKind::Roll => {
                matches!(sm.current, FighterState::Dodge | FighterState::Burst)
            }
            HitReactKind::Dead => sm.current == FighterState::Dead,
        };
        if !owns_state {
            runtime.clear();
            continue;
        }
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            runtime.current = Some(reaction);
            continue;
        };

        if reaction.pose_id != Some(pose.pose_id) {
            reaction.pose_id = Some(pose.pose_id);
            reaction.minimum_remaining = runtime.authored_minimum;
            reaction.lock_remaining = runtime
                .authored_minimum
                .max(pose_duration_seconds(pose.start, pose.end));
            reaction.play_to_end_pose_id = None;
            runtime.root_expected_pose_id = Some(pose.pose_id);
            runtime.root_pose_id = None;
            runtime.root_prev = None;
            runtime.root_started = false;
        }

        combat.state = sm.current;
        let guard_held = actor_guard_held(
            local.is_some(),
            local_guard_held,
            npc_performance.map(|performance| performance.guard_hold()),
        );
        let mut anger_spend = None;
        let mut view = CombatView {
            pose: &mut pose,
            blend: &mut blend,
            combat: &mut combat,
            health: &mut health,
            qi: &mut qi,
            air,
            assets: character_assets,
            weapon: fighter.weapon,
            guard_held_input: guard_held,
            in_guard_hit_reaction: reaction.kind == HitReactKind::Guard,
            attack_directions: Vec::new(),
            weapon_form: weapon_form.current,
            anger_spend: &mut anger_spend,
            strategy_id: weapon_to_strategy(fighter.weapon),
            weapon_action_vetoed: false,
        };
        let previous_kind = reaction.kind;
        let next = tick_hit_react(&mut view, reaction, dt);
        drop(view);

        if let Some(next) = next {
            let (state, locked) = match next.kind {
                HitReactKind::Guard => (FighterState::Guard, true),
                HitReactKind::BeHit | HitReactKind::GetUp => (FighterState::BeHit, true),
                HitReactKind::Down => (FighterState::Down, true),
                HitReactKind::Roll => (FighterState::Dodge, false),
                HitReactKind::Dead => (FighterState::Dead, true),
            };
            if sm.current != state {
                sm.previous = sm.current;
                sm.current = state;
                sm.time_in_state = 0.0;
            }
            sm.locked = locked;
            combat.state = state;
            runtime.current = Some(next);
        } else {
            runtime.clear();
            let state = if guard_held {
                FighterState::Guard
            } else {
                FighterState::Name
            };
            if matches!(
                sm.current,
                FighterState::BeHit | FighterState::Down | FighterState::Guard
            ) {
                sm.previous = sm.current;
                sm.current = state;
                sm.time_in_state = 0.0;
                sm.locked = false;
                sm.attack_stage = 0;
                sm.attack_queued = None;
                sm.linked_recovery_pose_id = None;
                sm.exe_ground_mode_ticks = 0;
                combat.state = state;
            }
            debug!("hit reaction {:?} completed", previous_kind);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn attack_windows(
    mut commands: Commands,
    (active_fighter, time, mut self_effect_commits): (
        Option<Res<super::fighter_frame::ActiveFighter>>,
        Res<Time>,
        Local<std::collections::HashMap<Entity, u32>>,
    ),
    net: Res<NetState>,
    assets: Option<Res<CharacterAssetsSet>>,
    combat_resources: Res<CombatResourceData>,
    mut score: ResMut<crate::components::fighter::KillScore>,
    mut feedback: HitFeedback,
    mut lock: ResMut<LockOnController>,
    mut death_weapon_drops: MessageWriter<crate::plugins::render_plugin::OfflineWeaponDeathDrop>,
    buffs: Res<crate::plugins::item_plugin::ItemBuffs>,
    mut gate: ResMut<crate::plugins::item_plugin::PropHitGate>,
    mut script: ResMut<crate::plugins::stage_plugin::StageScriptRuntime>,
    geometry: HitGeometryQueries,
    mut ranged: RangedRuntimeParams,
    mut queries: ParamSet<(
        AttackerWindowQuery<'_, '_>,
        DefenderWindowQuery<'_, '_>,
        DefenderPositionQuery<'_, '_>,
    )>,

    mut props: Query<
        (
            Entity,
            &mut crate::plugins::item_plugin::StageProp,
            &Transform,
            &Visibility,
        ),
        Without<Fighter>,
    >,
) {
    #[cfg(target_arch = "wasm32")]
    let _ = &ranged.net_tx;
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(14);
    let Some(assets) = assets else {
        return;
    };

    let mut snapshots: Vec<AttackSnapshot> = Vec::new();
    let mut self_effects = Vec::new();
    {
        let mut attackers = queries.p0();
        self_effect_commits.retain(|entity, _| attackers.contains(*entity));
        for (
            attacker_entity,
            fighter,
            pose,
            blend,
            motion,
            atk_pos,
            sm_state,
            health,
            death,
            hit_reaction,
            mission_npc,
            local,
            remote,
            mut ranged_gate,
            (weapon_form_state, ai_intent),
        ) in &mut attackers
        {
            if !super::fighter_frame::accepts(active_fighter.as_deref(), attacker_entity) {
                continue;
            }
            let local_authority = local_attack_window_authority(net.status, local.is_some())
                || (net.is_stock_host() && net.stock_npc_ready.contains(&attacker_entity));
            let props_only = remote_prop_authority(&net, remote.is_some());
            if !local_authority && !props_only {
                continue;
            }
            if !attack_window_owner_active_for_scope(
                health.current,
                death.active,
                sm_state.current,
                hit_reaction.current.is_some(),
                props_only,
            ) || entry_blend_is_active(blend.duration, blend.pose_id, pose.pose_id)
            {
                continue;
            }
            let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
                continue;
            };
            let Some(pose_data) = character_assets.poses.get(&pose.pose_id) else {
                continue;
            };

            if !props_only
                && pose.frame.floor() >= pose.end.saturating_sub(1) as f32
                && self_effect_commits.get(&attacker_entity) != Some(&pose.pose_instance_id)
            {
                if let Some(effect) = combat_resources
                    .0
                    .pose_self_status_effect(pose.pose_id)
                    .filter(|effect| effect.health < 0 && effect.duration == 0.0)
                {
                    self_effects.push((
                        attacker_entity,
                        pose.pose_id,
                        pose.pose_instance_id,
                        effect,
                    ));
                }
            }
            let forward = attack_forward_from_facing(motion.facing);

            let Some((index, window)) = active_attack_window(pose.frame, &pose_data.attacks) else {
                continue;
            };
            let (script_character_id, script_team) = if let Some(npc) = mission_npc {
                (npc.id, Some(i64::from(npc.team)))
            } else if local.is_some() {
                (0, None)
            } else if let Some(remote) = remote {
                remote_scene_character(&net, remote)
            } else {
                (-1, None)
            };
            let combat_team = fighter_combat_team(&net, local.is_some(), remote, mission_npc);
            let profile = ranged_attack_profile(
                pose.pose_id,
                fighter.weapon,
                Some(weapon_to_strategy(fighter.weapon)),
            );
            if bypasses_melee_hitboxes(profile) {
                let profile = profile.expect("checked above");
                let default_origin = atk_pos.translation;
                let mut spawn_origin = default_origin;
                let mut shot_forward = if local.is_none() && remote.is_none() {
                    ai_intent
                        .and_then(|intent| intent.ranged_aim)
                        .unwrap_or(forward)
                } else {
                    forward
                };
                if profile.kind == RangedKind::Gun {
                    if local.is_some() {
                        if let Ok(camera) = ranged.camera.single() {
                            spawn_origin = camera.translation();
                            shot_forward = camera.forward().into();
                        } else {
                            spawn_origin.y += GUN_SPAWN_HEIGHT;
                        }
                    } else {
                        spawn_origin.y += GUN_SPAWN_HEIGHT;
                    }
                } else if profile.kind == RangedKind::Dart {
                    spawn_origin.y += DART_SPAWN_HEIGHT;

                    if local.is_some() {
                        if let Ok(camera) = ranged.camera.single() {
                            shot_forward = camera.forward().into();
                        }
                    }
                } else {
                    spawn_origin.y += WHEEL_SPAWN_HEIGHT;
                }
                let source = RangedHitSource {
                    owner: attacker_entity,
                    weapon: fighter.weapon,
                    weapon_stats: combat_weapon_stats(&combat_resources.0, fighter),
                    pose_id: pose.pose_id,
                    pose_frame: pose.frame,
                    pose_start: pose.start,
                    pose_instance_id: pose.pose_instance_id,
                    attack_index: index,
                    attack: window.clone(),
                    player_slot: fighter.player_slot,
                    combat_team,
                    script_character_id,
                    script_team,
                    attack_origin: spawn_origin,
                    attack_forward: shot_forward,
                };

                if !props_only
                    && profile.entity
                    && ranged_gate.entity_pose_instance != Some(pose.pose_instance_id)
                    && !(profile.kind == RangedKind::Wheel && ranged_gate.wheel_out)
                {
                    ranged_gate.entity_pose_instance = Some(pose.pose_instance_id);
                    if profile.kind == RangedKind::Wheel {
                        ranged_gate.wheel_out = true;
                    }
                    let (kind, velocity, speed) = match profile.kind {
                        RangedKind::Dart => {
                            let velocity = (shot_forward
                                + Vec3::Y * meteor_combat::ranged_weapons::DART_VERTICAL_BIAS)
                                * DART_SPEED;
                            (ProjectileKind::Dart, velocity, DART_SPEED)
                        }
                        RangedKind::Wheel => {
                            let speed = profile.speed.unwrap_or(42.0);
                            (ProjectileKind::FlyingWheel, shot_forward * speed, speed)
                        }
                        RangedKind::Gun => unreachable!(),
                    };
                    commands.spawn((
                        Projectile {
                            kind,
                            source: source.clone(),
                            velocity,
                            age: 0.0,
                            hit_scale: profile.hit_scale,
                            speed,
                            launch_point: spawn_origin,
                            guide_target: None,
                            guide_initialized: false,
                            route_to_target: false,
                            collision_return: false,
                        },
                        Transform::from_translation(spawn_origin)
                            .with_rotation(Quat::from_rotation_y(motion.facing)),
                        Visibility::Visible,
                    ));
                }
                if !props_only
                    && profile.ray
                    && ranged_gate.ray_pose_instance != Some(pose.pose_instance_id)
                {
                    ranged_gate.ray_pose_instance = Some(pose.pose_instance_id);
                    commands.spawn((
                        Projectile {
                            kind: ProjectileKind::GunRay,
                            source,
                            velocity: shot_forward,
                            age: 0.0,
                            hit_scale: profile.hit_scale,
                            speed: 0.0,
                            launch_point: spawn_origin,
                            guide_target: None,
                            guide_initialized: false,
                            route_to_target: false,
                            collision_return: false,
                        },
                        Transform::from_translation(spawn_origin),
                    ));
                }
                if profile.mode == RangedMode::Effect {
                    let effect_obbs = geometry.live_effect_obbs(attacker_entity, character_assets);
                    if !effect_obbs.is_empty() {
                        snapshots.push(AttackSnapshot {
                            entity: attacker_entity,
                            weapon: fighter.weapon,
                            weapon_stats: combat_weapon_stats(&combat_resources.0, fighter),
                            pose_id: pose.pose_id,
                            pose_frame: pose.frame,
                            pose_start: pose.start,
                            player_slot: fighter.player_slot,
                            combat_team,
                            script_character_id,
                            script_team,
                            atk_pos: atk_pos.translation,
                            forward,
                            active_window: Some((index, window.clone())),
                            requires_live_attack_owner: true,
                            props_only,
                            geometry: AttackGeometry::Obbs(Vec::new()),
                            effect_obbs,
                            effect_origin: fighter_whole_body_obb(
                                attacker_entity,
                                character_assets,
                                &geometry,
                            ),
                        });
                    }
                }
                continue;
            }

            let attack_obbs = geometry.attack_obbs(
                attacker_entity,
                fighter,
                weapon_form_state,
                pose.pose_id,
                character_assets,
                &window.bones,
            );
            let effect_obbs = if window
                .bones
                .iter()
                .any(|name| name.eq_ignore_ascii_case("effect"))
            {
                geometry.live_effect_obbs(attacker_entity, character_assets)
            } else {
                Vec::new()
            };
            let effect_origin = if effect_obbs.is_empty() {
                None
            } else {
                fighter_whole_body_obb(attacker_entity, character_assets, &geometry)
            };
            snapshots.push(AttackSnapshot {
                entity: attacker_entity,
                weapon: fighter.weapon,
                weapon_stats: combat_weapon_stats(&combat_resources.0, fighter),
                pose_id: pose.pose_id,
                pose_frame: pose.frame,
                pose_start: pose.start,
                player_slot: fighter.player_slot,
                combat_team,
                script_character_id,
                script_team,
                atk_pos: atk_pos.translation,
                forward,
                active_window: Some((index, window.clone())),
                requires_live_attack_owner: true,
                props_only,
                effect_origin,
                effect_obbs,
                geometry: AttackGeometry::Obbs(attack_obbs),
            });
        }
    }
    for hit in ranged.pending.0.drain(..) {
        snapshots.push(AttackSnapshot {
            entity: hit.source.owner,
            weapon: hit.source.weapon,
            weapon_stats: hit.source.weapon_stats,
            pose_id: hit.source.pose_id,
            pose_frame: hit.source.pose_frame,
            pose_start: hit.source.pose_start,
            player_slot: hit.source.player_slot,
            combat_team: hit.source.combat_team,
            script_character_id: hit.source.script_character_id,
            script_team: hit.source.script_team,
            atk_pos: hit.source.attack_origin,
            forward: hit.source.attack_forward,
            active_window: Some((hit.source.attack_index, hit.source.attack)),
            requires_live_attack_owner: false,
            props_only: false,
            effect_origin: None,
            effect_obbs: Vec::new(),
            geometry: AttackGeometry::Segment {
                start: hit.start,
                end: hit.end,
                hit_scale: hit.hit_scale,
                projectile: hit.projectile,
                stop_after_first: hit.stop_after_first,
                despawn_after: hit.despawn_after,
            },
        });
    }
    sort_attack_snapshots(&mut snapshots);

    for snapshot in &snapshots {
        if net.status == NetStatus::InRoom && !snapshot.props_only {
            let owned = queries.p0().get(snapshot.entity).is_ok_and(
                |(_, _, _, _, _, _, _, _, _, _, _, local, _, _, _)| {
                    local.is_some()
                        || (net.is_stock_host() && net.stock_npc_ready.contains(&snapshot.entity))
                },
            );
            if !owned {
                continue;
            }
        }
        let Some((attack_index, window)) = snapshot.active_window.as_ref() else {
            continue;
        };
        if snapshot.requires_live_attack_owner {
            let still_active = {
                let fighters = queries.p2();
                fighters
                    .get(snapshot.entity)
                    .is_ok_and(|(_, _, health, state, death, reaction)| {
                        attack_window_owner_active_for_scope(
                            health.current,
                            death.active,
                            state.current,
                            reaction.current.is_some(),
                            snapshot.props_only,
                        )
                    })
            };
            if !still_active {
                continue;
            }
        }

        let mut defender_order = {
            let positions = queries.p2();
            positions
                .iter()
                .filter_map(|(entity, transform, ..)| {
                    if entity == snapshot.entity {
                        return None;
                    }
                    let along = match &snapshot.geometry {
                        AttackGeometry::Segment {
                            start,
                            end,
                            hit_scale,
                            ..
                        } => segment_hits_box_distance(
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
                            ranged_target_box(
                                RangedPoint3 {
                                    x: transform.translation.x,
                                    y: transform.translation.y,
                                    z: transform.translation.z,
                                },
                                *hit_scale,
                            ),
                        )?,
                        AttackGeometry::Obbs(_) => 0.0,
                    };
                    Some((entity, along))
                })
                .collect::<Vec<_>>()
        };
        if matches!(&snapshot.geometry, AttackGeometry::Segment { .. }) {
            sort_ranged_defender_candidates(&mut defender_order);
        }
        let mut ranged_hit_landed = false;
        if !snapshot.props_only {
            let mut defenders = queries.p1();
            for (defender_entity, _) in defender_order {
                let Ok((
                    defender_entity,
                    mut health,
                    mut qi,
                    mut def_pose,
                    mut def_blend,
                    mut def_motion,
                    mut def_combat,
                    mut def_sm,
                    mut def_death,
                    mut target_hit_clock,
                    defender,
                    def_pos,
                    defender_remote,
                    defender_mission_npc,
                    (
                        defender_local,
                        defender_weapon_form,
                        defender_visibility,
                        mut defender_reaction,
                        defender_attack_motion,
                        defender_air,
                        defender_cheat,
                    ),
                )) = defenders.get_mut(defender_entity)
                else {
                    continue;
                };
                if defender_entity == snapshot.entity
                    || def_death.active
                    || defender_visibility
                        .is_some_and(|visibility| *visibility == Visibility::Hidden)
                {
                    continue;
                }
                let defender_team = fighter_combat_team(
                    &net,
                    defender_local.is_some(),
                    defender_remote,
                    defender_mission_npc,
                );
                if net.status == NetStatus::InRoom
                    && defender_local.is_none()
                    && defender_remote.is_none()
                    && !net.stock_npc_ready.contains(&defender_entity)
                {
                    continue;
                }

                if !meteor_combat::attack_window::attack_can_reach_team_with_friendly_fire(
                    snapshot.combat_team,
                    defender_team,
                    net.friendly_fire_enabled(),
                ) {
                    continue;
                }
                let Some(def_assets) = assets.0.get(&format!("p{}", defender.character_id)) else {
                    continue;
                };
                let effect_hit = !snapshot.effect_obbs.is_empty()
                    && fighter_whole_body_obb(defender_entity, def_assets, &geometry).is_some_and(
                        |body| {
                            snapshot
                                .effect_obbs
                                .iter()
                                .any(|attack| obbs_intersect(attack, &body))
                                && !effect_body_occluded(
                                    geometry.stage.as_deref(),
                                    snapshot.effect_origin.as_ref(),
                                    &body,
                                )
                        },
                    );
                let hit_geometry = match &snapshot.geometry {
                    AttackGeometry::Obbs(attack_obbs) => {
                        if attack_obbs.is_empty() {
                            false
                        } else {
                            let hurt_obbs =
                                fighter_body_hurt_obbs(defender_entity, def_assets, &geometry);
                            attack_obbs
                                .iter()
                                .any(|a| hurt_obbs.iter().any(|b| obbs_intersect(a, b)))
                        }
                    }
                    AttackGeometry::Segment {
                        start,
                        end,
                        hit_scale,
                        ..
                    } => segment_hits_box_distance(
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
                        ranged_target_box(
                            RangedPoint3 {
                                x: def_pos.translation.x,
                                y: def_pos.translation.y,
                                z: def_pos.translation.z,
                            },
                            *hit_scale,
                        ),
                    )
                    .is_some(),
                };
                if !hit_geometry && !effect_hit {
                    continue;
                }

                if admit_intersecting_target(&mut target_hit_clock)
                    != IntersectingTargetAdmission::Accepted
                {
                    continue;
                }

                #[cfg(not(target_arch = "wasm32"))]
                if net.is_stock_host()
                    && (net.stock_npc_ready.contains(&snapshot.entity)
                        || net.stock_npc_ready.contains(&defender_entity))
                {
                    let target_id =
                        net.stock_npc_actors
                            .get(&defender_entity)
                            .copied()
                            .or_else(|| {
                                let id = if defender_local.is_some() {
                                    Some(net.player_id.as_str())
                                } else {
                                    defender_remote.map(|remote| remote.player_id.as_str())
                                };
                                id?.strip_prefix("stock-")?.parse().ok()
                            });
                    if let (Some(target_id), Some(tx)) = (target_id, ranged.net_tx.as_deref()) {
                        let poses: Vec<u16> = def_assets.poses.keys().copied().collect();
                        let bucket = facing_bucket(
                            snapshot.atk_pos.x,
                            snapshot.atk_pos.z,
                            def_pos.translation.x,
                            def_pos.translation.z,
                            def_motion.facing,
                        );
                        let request = meteor_network::stock_protocol::StockHitRequest::from_attack(
                            target_id,
                            snapshot.pose_id,
                            snapshot.weapon,
                            window,
                            bucket,
                            &poses,
                        );
                        tx.send_stock_actor_hit(
                            net.stock_npc_ready
                                .contains(&snapshot.entity)
                                .then_some(snapshot.entity),
                            request,
                        );

                        if !net.stock_npc_ready.contains(&snapshot.entity)
                            && teams_are_hostile(snapshot.combat_team, defender_team)
                        {
                            let candidate = LockCandidate {
                                id: defender_entity.to_bits(),
                                x: def_pos.translation.x,
                                y: def_pos.translation.y,
                                z: def_pos.translation.z,
                                team: defender_team,
                            };
                            if lock.system.acquire_hit_target(
                                feedback.settings.auto_camera,
                                weapon_to_strategy(snapshot.weapon),
                                defender_entity.to_bits(),
                                &[candidate],
                            ) {
                                lock.lock_entity = Some(defender_entity);
                            }
                        }
                        ranged_hit_landed |=
                            matches!(&snapshot.geometry, AttackGeometry::Segment { .. });
                        if matches!(
                            &snapshot.geometry,
                            AttackGeometry::Segment {
                                stop_after_first: true,
                                ..
                            }
                        ) {
                            break;
                        }
                    }
                    continue;
                }

                let mut remote_hp = health.current;
                let mut remote_qi = qi.current;
                let mut stats = if defender_remote.is_some() {
                    StatsView {
                        hp: &mut remote_hp,
                        qi: &mut remote_qi,
                        invincible_flag: false,
                    }
                } else {
                    StatsView {
                        hp: &mut health.current,
                        qi: &mut qi.current,
                        invincible_flag: defender_cheat.is_some(),
                    }
                };
                let attacker_bonus = if snapshot.player_slot == 0 && buffs.remaining > 0.0 {
                    buffs.attack_bonus
                } else {
                    0
                };
                let attacker_bonus = attacker_bonus
                    + ranged
                        .statuses
                        .get(snapshot.entity)
                        .map_or(0, |status| status.effects.attack_bonus());
                let defender_bonus = if defender.player_slot == 0 && buffs.remaining > 0.0 {
                    buffs.defense_bonus
                } else {
                    0
                };
                let defender_bonus = defender_bonus
                    + ranged
                        .statuses
                        .get(defender_entity)
                        .map_or(0, |status| status.effects.defense_bonus());
                let defender_weapon_def = combat_weapon_stats(&combat_resources.0, defender)
                    .map(|stats| stats.defense)
                    .unwrap_or(15)
                    + defender_bonus;
                let pose_attack_percent = |pose_id| combat_resources.0.pose_attack_percent(pose_id);
                let result = resolve_hit(
                    &mut stats,
                    &HitOptions {
                        attacker_team: snapshot.combat_team,
                        defender_team,
                        defender_in_guard: def_combat.state == FighterState::Guard,
                        defender_weapon_def: Some(defender_weapon_def),
                        weapon_id: snapshot.weapon,
                        pose_id: snapshot.pose_id,
                        attack_power: snapshot.weapon_stats.map(|stats| stats.attack),
                        attack_bonus: attacker_bonus,
                        hit_key: None,
                        force_hit_flags: None,
                        attack_type: window.attack_type as i32,
                        dword1: None,
                        attack: Some(window),
                        pose_attack_percent: &pose_attack_percent,
                    },
                );
                if !result.guarded
                    && !result.killed
                    && defender_remote.is_none()
                    && net.status != NetStatus::InRoom
                {
                    if let Some(effect) = combat_resources.0.pose_status_effect(snapshot.pose_id) {
                        if let Ok(mut statuses) = ranged.statuses.get_mut(defender_entity) {
                            statuses.apply(effect);
                        }
                    }
                }
                ranged_hit_landed |= matches!(&snapshot.geometry, AttackGeometry::Segment { .. });
                let stop_after_first = matches!(
                    &snapshot.geometry,
                    AttackGeometry::Segment {
                        stop_after_first: true,
                        ..
                    }
                );
                let killed = result.killed;
                let chest = def_pos.translation + Vec3::Y * 2.2;
                let weapon_point = match &snapshot.geometry {
                    AttackGeometry::Obbs(obbs) => obbs
                        .first()
                        .or_else(|| snapshot.effect_obbs.first())
                        .map(|obb| Vec3::new(obb.center.x, obb.center.y, obb.center.z)),
                    AttackGeometry::Segment { end, .. } => Some(*end),
                }
                .unwrap_or(snapshot.atk_pos + Vec3::Y * 2.2);
                let impact_kind = if result.guarded {
                    crate::plugins::impact_fx_plugin::CombatImpactKind::Guard
                } else if killed {
                    crate::plugins::impact_fx_plugin::CombatImpactKind::Kill
                } else {
                    crate::plugins::impact_fx_plugin::CombatImpactKind::Hit
                };
                let predicted_reaction = defender_remote.is_some().then(|| {
                    use meteor_combat::hit_reaction::{
                        guard_reaction_pose_id, hit_direction_bits, pick_reaction_pose_id,
                        DEATH_POSES,
                    };
                    let poses: Vec<u16> = def_assets.poses.keys().copied().collect();
                    let bucket = facing_bucket(
                        snapshot.atk_pos.x,
                        snapshot.atk_pos.z,
                        def_pos.translation.x,
                        def_pos.translation.z,
                        def_motion.facing,
                    );
                    if result.guarded {
                        let strategy = weapon_to_strategy(defender.weapon);
                        guard_reaction_pose_id(
                            strategy,
                            hit_direction_bits(bucket) >> 12,
                            &poses,
                            (u32::from(snapshot.pose_id)
                                ^ snapshot.pose_frame.floor() as u32
                                ^ *attack_index)
                                & 3,
                        )
                        .or_else(|| guard_pose_for_weapon(strategy))
                    } else if killed {
                        DEATH_POSES.into_iter().find(|id| poses.contains(id))
                    } else {
                        pick_reaction_pose_id(window, bucket, &poses)
                    }
                    .unwrap_or(def_pose.pose_id)
                });
                feedback
                    .impacts
                    .write(crate::plugins::impact_fx_plugin::CombatImpactEvent {
                        kind: impact_kind,
                        attacker: snapshot.entity,
                        defender: defender_entity,
                        reaction_pose: predicted_reaction,
                        effect_strategy: None,
                        contact: (weapon_point + chest) * 0.5,
                        direction: def_pos.translation - snapshot.atk_pos,
                    });

                if snapshot.player_slot == 0
                    && teams_are_hostile(snapshot.combat_team, defender_team)
                    && hit_result_reaches_auto_lock(killed, defender_remote.is_some())
                {
                    let candidate = LockCandidate {
                        id: defender_entity.to_bits(),
                        x: def_pos.translation.x,
                        y: def_pos.translation.y,
                        z: def_pos.translation.z,
                        team: defender_team,
                    };
                    if lock.system.acquire_hit_target(
                        feedback.settings.auto_camera,
                        weapon_to_strategy(snapshot.weapon),
                        defender_entity.to_bits(),
                        &[candidate],
                    ) {
                        lock.lock_entity = Some(defender_entity);
                    }
                }

                if let Some(_remote) = defender_remote.as_ref() {
                    commands.entity(defender_entity).insert(PendingRemoteHit {
                        damage: result.damage as f32,
                        pose_id: snapshot.pose_id,
                        attack_index: *attack_index,
                        frame: snapshot.pose_frame,
                        cursor: (snapshot.pose_frame - snapshot.pose_start as f32).max(0.0),
                        attack_origin: snapshot.atk_pos,
                        attack_forward: snapshot.forward,
                    });
                    if stop_after_first {
                        break;
                    }
                    continue;
                }
                let poses: Vec<u16> = def_assets.poses.keys().copied().collect();
                let bucket = facing_bucket(
                    snapshot.atk_pos.x,
                    snapshot.atk_pos.z,
                    def_pos.translation.x,
                    def_pos.translation.z,
                    def_motion.facing,
                );

                if let Some(mut attack_motion) = defender_attack_motion {
                    *attack_motion = FighterAttackMotion::default();
                }

                let fallback_air = FighterAir::default();
                let defender_air = defender_air.as_deref().unwrap_or(&fallback_air);
                let mut anger_spend = None;
                let mut view = CombatView {
                    pose: &mut def_pose,
                    blend: &mut def_blend,
                    combat: &mut def_combat,
                    health: &mut health,
                    qi: &mut qi,
                    air: defender_air,
                    assets: def_assets,
                    weapon: defender.weapon,
                    guard_held_input: false,
                    in_guard_hit_reaction: false,
                    attack_directions: Vec::new(),
                    weapon_form: defender_weapon_form.current,
                    anger_spend: &mut anger_spend,
                    strategy_id: weapon_to_strategy(defender.weapon),
                    weapon_action_vetoed: false,
                };
                if result.guarded {
                    let strategy = weapon_to_strategy(defender.weapon);
                    if let Some(reaction) = begin_guard_hit(
                        &mut view,
                        Some(window),
                        bucket,
                        &poses,
                        strategy,
                        guard_pose_for_weapon(strategy),
                        (u32::from(snapshot.pose_id)
                            ^ snapshot.pose_frame.floor() as u32
                            ^ *attack_index)
                            & 3,
                    ) {
                        defender_reaction.begin(reaction, defender_air.air.airborne);
                    }
                    drop(view);
                    def_combat.state = FighterState::Guard;
                    def_sm.current = FighterState::Guard;
                    def_sm.locked = true;
                    if stop_after_first {
                        break;
                    }
                    continue;
                }

                if killed {
                    let reaction = begin_death(&mut view, Some(window));
                    defender_reaction.begin(reaction, defender_air.air.airborne);

                    clear_composed_velocity_on_death(&mut def_motion);
                    drop(view);
                    def_combat.state = FighterState::Dead;
                    def_sm.previous = def_sm.current;
                    def_sm.current = FighterState::Dead;
                    def_sm.time_in_state = 0.0;
                    def_sm.locked = true;
                    def_sm.attack_stage = 0;
                    def_sm.attack_queued = None;
                    def_sm.linked_recovery_pose_id = None;
                    def_sm.exe_ground_mode_ticks = 0;
                    if let Some(model) = crate::plugins::character_plugin::weapon_model_stem(
                        defender.weapon,
                        defender.weapon_variant,
                    ) {
                        let yaw = def_pos.rotation.to_euler(EulerRot::YXZ).0;
                        death_weapon_drops.write(
                            crate::plugins::render_plugin::OfflineWeaponDeathDrop {
                                model,
                                position: (def_pos.translation + Vec3::Y * (50.0 * 0.12))
                                    .to_array(),
                                velocity: [yaw.sin() * 12.0, 0.0, yaw.cos() * 12.0],
                            },
                        );
                    }
                    def_death.active = true;
                    def_death.timer = meteor_combat::death::RESPAWN_DELAY_SECONDS;

                    if snapshot.player_slot == 0 {
                        score.player_kills += 1;
                    } else {
                        score.npc_kills += 1;
                    }
                    info!(
                        "kill! attacker slot {} → defender slot {}",
                        snapshot.player_slot, defender.player_slot
                    );

                    let character_id = defender_mission_npc
                        .map(|npc| npc.id)
                        .or_else(|| defender_local.is_some().then_some(0));
                    if let (Some(character_id), Some(runtime)) = (character_id, script.script_mut())
                    {
                        if let Err(error) = runtime.call(
                            "Scene_OnCharacterEvent",
                            &[
                                meteor_scene::scene_script_runtime::PstValue::Num(
                                    character_id as f64,
                                ),
                                meteor_scene::scene_script_runtime::PstValue::Num(202.0),
                            ],
                        ) {
                            warn!("script Scene_OnCharacterEvent failed: {error}");
                        }
                    }
                } else {
                    if let Some(reaction) = begin_be_hit(&mut view, Some(window), bucket, &poses) {
                        defender_reaction.begin(reaction, defender_air.air.airborne);
                        defender_reaction
                            .bank_be_hit_carry(def_motion.velocity, defender_local.is_some());
                    }
                    drop(view);
                    def_combat.state = FighterState::BeHit;
                    def_sm.previous = def_sm.current;
                    def_sm.current = FighterState::BeHit;
                    def_sm.time_in_state = 0.0;
                    def_sm.locked = true;
                    def_sm.attack_stage = 0;
                    def_sm.attack_queued = None;
                    def_sm.linked_recovery_pose_id = None;
                    def_sm.exe_ground_mode_ticks = 0;
                }
                if stop_after_first {
                    break;
                }
            }

            if matches!(&snapshot.geometry, AttackGeometry::Obbs(_))
                && gate.accepts(snapshot.entity, time.elapsed_secs_f64())
            {
                let prop_attack_bonus = if snapshot.player_slot == 0 && buffs.remaining > 0.0 {
                    buffs.attack_bonus
                } else {
                    0
                };
                let prop_pose_attack_percent =
                    |pose_id| combat_resources.0.pose_attack_percent(pose_id);

                let prop_attack_bonus = prop_attack_bonus
                    + ranged
                        .statuses
                        .get(snapshot.entity)
                        .map_or(0, |status| status.effects.attack_bonus());
                let prop_damage = sum_attack_power(
                    snapshot.weapon,
                    snapshot.pose_id,
                    u32::from(snapshot.pose_id),
                    snapshot.weapon_stats.map(|stats| stats.attack),
                    prop_attack_bonus,
                    &prop_pose_attack_percent,
                );
                let AttackGeometry::Obbs(attack_obbs) = &snapshot.geometry else {
                    continue;
                };
                let mut prop_hit = false;

                let mut reported_props = std::collections::HashSet::new();
                for (_, mut prop, prop_transform, visibility) in &mut props {
                    if prop.broken || *visibility == Visibility::Hidden {
                        continue;
                    }
                    if script
                        .script()
                        .is_some_and(|runtime| !script_allows_prop_hit(runtime, &prop.name))
                    {
                        continue;
                    }
                    let prop_box = stage_prop_hit_obb(&prop, prop_transform);
                    if !attack_obbs
                        .iter()
                        .chain(snapshot.effect_obbs.iter())
                        .any(|attack_box| obbs_intersect(attack_box, &prop_box))
                    {
                        continue;
                    }
                    if !reported_props.insert(prop.name.to_ascii_lowercase()) {
                        continue;
                    }
                    prop_hit = true;
                    if forward_network_prop_hit(
                        &net,
                        ranged.net_tx.as_deref(),
                        &prop.name,
                        snapshot.pose_id,
                        snapshot.pose_frame,
                    ) {
                        continue;
                    }
                    prop.hit_count += 1;

                    if let Some(effect_assets) = feedback.effects.as_deref() {
                        crate::plugins::effect_plugin::spawn_hit_burst(
                            &mut commands,
                            effect_assets,
                            Vec3::new(prop_box.center.x, prop_box.center.y, prop_box.center.z),
                            snapshot.forward,
                        );
                    }

                    if let Some(runtime) = script.script_mut() {
                        if let Some(team) = snapshot.script_team {
                            runtime
                                .host_mut()
                                .set_character_team(snapshot.script_character_id, team);
                        }
                        if let Err(error) = runtime.call_item(
                            &prop.name,
                            "OnAttack",
                            &scene_on_attack_args(snapshot.script_character_id, prop_damage),
                        ) {
                            warn!("script callItem OnAttack failed: {error}");
                        }
                    }
                    info!(
                        "prop hit: {} at ({:.2}, {:.2}, {:.2})",
                        prop.model,
                        prop_transform.translation.x,
                        prop_transform.translation.y,
                        prop_transform.translation.z
                    );
                }
                if prop_hit {
                    gate.reset(snapshot.entity, time.elapsed_secs_f64());
                }
            }
            if let AttackGeometry::Segment {
                projectile,
                stop_after_first,
                despawn_after,
                ..
            } = &snapshot.geometry
            {
                if *despawn_after || (*stop_after_first && ranged_hit_landed) {
                    commands.entity(*projectile).despawn();
                }
            }
        }
    }
    for (entity, source_pose, instance, effect) in self_effects {
        let mut defenders = queries.p1();
        let Ok((
            _,
            mut health,
            mut qi,
            mut pose,
            mut blend,
            mut motion,
            mut combat,
            mut sm,
            mut death,
            _,
            fighter,
            transform,
            remote,
            npc,
            (local, form, _, mut reaction, attack, air, _),
        )) = defenders.get_mut(entity)
        else {
            continue;
        };
        if remote.is_some()
            || health.current <= 0.0
            || death.active
            || pose.pose_id != source_pose
            || pose.pose_instance_id != instance
        {
            continue;
        }
        self_effect_commits.insert(entity, instance);
        if net.status == NetStatus::InRoom {
            commands
                .entity(entity)
                .insert(super::network_plugin::PendingSelfEffect {
                    resource_id: effect.id,
                    pose_id: source_pose,
                    pose_instance: instance,
                });
            continue;
        }
        health.current = (health.current + effect.health as f32).clamp(0.0, health.max);
        if health.current > 0.0 {
            continue;
        }
        let Some(asset) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        let fallback_air = FighterAir::default();
        let air = air.as_deref().unwrap_or(&fallback_air);
        let mut anger_spend = None;
        let mut view = CombatView {
            pose: &mut pose,
            blend: &mut blend,
            combat: &mut combat,
            health: &mut health,
            qi: &mut qi,
            air,
            assets: asset,
            weapon: fighter.weapon,
            guard_held_input: false,
            in_guard_hit_reaction: false,
            attack_directions: Vec::new(),
            weapon_form: form.current,
            anger_spend: &mut anger_spend,
            strategy_id: weapon_to_strategy(fighter.weapon),
            weapon_action_vetoed: false,
        };
        reaction.begin(begin_death(&mut view, None), air.air.airborne);
        drop(view);
        if let Some(mut attack) = attack {
            *attack = FighterAttackMotion::default();
        }
        clear_composed_velocity_on_death(&mut motion);
        combat.state = FighterState::Dead;
        sm.previous = sm.current;
        sm.current = FighterState::Dead;
        sm.time_in_state = 0.0;
        sm.locked = true;
        sm.attack_stage = 0;
        sm.attack_queued = None;
        sm.linked_recovery_pose_id = None;
        sm.exe_ground_mode_ticks = 0;
        death.active = true;
        death.timer = meteor_combat::death::RESPAWN_DELAY_SECONDS;

        if let Some(model) =
            super::character_plugin::weapon_model_stem(fighter.weapon, fighter.weapon_variant)
        {
            let yaw = transform.rotation.to_euler(EulerRot::YXZ).0;
            death_weapon_drops.write(super::render_plugin::OfflineWeaponDeathDrop {
                model,
                position: (transform.translation + Vec3::Y * 6.0).to_array(),
                velocity: [yaw.sin() * 12.0, 0.0, yaw.cos() * 12.0],
            });
        }
        let character_id = npc.map(|n| n.id).or_else(|| local.is_some().then_some(0));
        if let (Some(id), Some(runtime)) = (character_id, script.script_mut()) {
            use meteor_scene::scene_script_runtime::PstValue;
            if let Err(error) = runtime.call(
                "Scene_OnCharacterEvent",
                &[PstValue::Num(id as f64), PstValue::Num(202.0)],
            ) {
                warn!("self effect death callback failed: {error}");
            }
        }
    }
}

type WeaponSwitchFighterQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Fighter,
        &'static mut WeaponSwitchActive,
        &'static PoseRuntime,
        Option<&'static LocalFighter>,
    ),
>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WeaponSwitchStep {
    commit: bool,
    remove: bool,
}

fn weapon_switch_step(switch: &mut WeaponSwitchActive, pose: &PoseRuntime) -> WeaponSwitchStep {
    if pose.pose_id != switch.pose_id || pose.pose_instance_id != switch.pose_instance_id {
        return WeaponSwitchStep {
            commit: false,
            remove: true,
        };
    }

    let commit = !switch.committed && pose.frame >= switch.marker_frame as f32;
    if commit {
        switch.committed = true;
    }

    let mut remove = false;
    if pose.frame >= switch.end_frame as f32 {
        if switch.end_sampled {
            remove = true;
        } else {
            switch.end_sampled = true;
        }
    }
    WeaponSwitchStep { commit, remove }
}

fn commit_carried_weapon(fighter: &mut Fighter, switch: &WeaponSwitchActive) -> bool {
    let target_slot = usize::from(switch.to_slot.min(1));
    if switch.empty_target
        || fighter.weapon_slots[target_slot] != switch.to_weapon
        || fighter.weapon_slot_variants[target_slot] != switch.to_variant
    {
        return false;
    }
    fighter.weapon = switch.to_weapon;
    fighter.weapon_slot = switch.to_slot.min(1);
    fighter.weapon_variant = switch.to_variant;
    true
}

fn commit_weapon_switch(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    mut commands: Commands,
    mut fighters: WeaponSwitchFighterQuery<'_, '_>,
    net: Res<NetState>,
    mut pending_actions: ResMut<PendingNetActions>,
) {
    for (entity, mut fighter, mut switch, pose, local) in &mut fighters {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        let step = weapon_switch_step(&mut switch, pose);
        if step.remove {
            commands.entity(entity).remove::<WeaponSwitchActive>();
            continue;
        }
        if step.commit {
            let switched = commit_carried_weapon(&mut fighter, &switch);
            if switched && local.is_some() && net.status == NetStatus::InRoom {
                pending_actions.push("switch_weapon");
            }
        }
    }
}

fn fist_weapon_swap(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    mut fighters: Query<
        (Entity, &mut Fighter, &PoseRuntime, Option<&LocalFighter>),
        Without<RemoteFighter>,
    >,
    net: Res<NetState>,
    mut pending_actions: ResMut<PendingNetActions>,
    mut committed: Local<std::collections::HashMap<Entity, (u16, u32)>>,
) {
    committed.retain(|entity, _| fighters.contains(*entity));
    for (entity, mut fighter, pose, local) in &mut fighters {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }

        let instance = (pose.pose_id, pose.pose_instance_id);
        if fighter.weapon != 11 || committed.get(&entity) == Some(&instance) {
            continue;
        }
        if !meteor_combat::weapon_strategy::is_fist_weapon_swap_pose(
            meteor_combat::weapon_strategy::FIST_STRATEGY_ID,
            pose.pose_id,
        ) {
            continue;
        }
        committed.insert(entity, instance);
        let Some(target_slot) = alternate_weapon_slot(fighter.weapon_slot, fighter.weapon_slots)
        else {
            continue;
        };

        fighter.weapon_slot = target_slot as u8;
        fighter.weapon = fighter.weapon_slots[target_slot];
        fighter.weapon_variant = fighter.weapon_slot_variants[target_slot];
        if local.is_some() && net.status == NetStatus::InRoom {
            pending_actions.push("switch_weapon");
        }
        info!(
            "fist weapon swap: pose {} → weapon {}",
            pose.pose_id, fighter.weapon
        );
    }
}

type MatchEndQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Fighter,
        &'static DeathRespawn,
        Option<&'static crate::plugins::stage_plugin::MissionNpc>,
    ),
    With<Fighter>,
>;

fn advance_match_settlement(
    mut end: ResMut<MatchEnd>,
    time: Res<Time<bevy::time::Real>>,
    request: Res<crate::app_state::LaunchRequest>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if request.mode == "replay" || !end.settled {
        return;
    }
    end.settle_elapsed += time.delta_secs();
    if end.settle_elapsed >= MISSION_SETTLE_SECONDS {
        next_state.set(AppState::Result);
    }
}

#[allow(clippy::too_many_arguments)]
fn evaluate_match_end(
    mut end: ResMut<MatchEnd>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    request: Res<crate::app_state::LaunchRequest>,
    fighters: MatchEndQuery<'_, '_>,
    mission: Res<crate::plugins::stage_plugin::StageMissionRuntime>,
    scene_script: Res<crate::plugins::stage_plugin::StageScriptRuntime>,
    mut net: ResMut<crate::plugins::network_plugin::NetState>,
    net_tx: Option<Res<crate::plugins::network_plugin::NetTx>>,
) {
    if request.mode == "replay" {
        return;
    }
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    if end.settled {
        return;
    }
    end.elapsed += dt;

    if let Some(winner_team) = scene_script
        .script()
        .and_then(|runtime| runtime.host().game_end())
    {
        if net.status == crate::plugins::network_plugin::NetStatus::InRoom {
            if net.is_scene_authority() && net.pending_objective_end != Some(winner_team as i32) {
                if let Some(tx) = net_tx {
                    tx.send_control(meteor_network::protocol::Message::ObjectiveEnd {
                        winner_team: winner_team as i32,
                    });
                    net.pending_objective_end = Some(winner_team as i32);
                }
            }
            return;
        }
        end.result = scene_game_end_result(winner_team);
        end.settled = true;
        end.settle_elapsed = 0.0;
        info!("scene script game end: team {winner_team}");
        return;
    }
    if !local_match_end_enabled(net.status) {
        return;
    }
    let members: Vec<RoundMember> = fighters
        .iter()
        .filter(|(_, _, mission_npc)| mission_npc.is_none_or(|npc| npc.in_scene_roster))
        .map(|(fighter, death, mission_npc)| RoundMember {
            team: mission_npc
                .map(|npc| npc.team as i32)
                .unwrap_or(if fighter.player_slot == 0 { 1 } else { 2 }),
            dead: death.active,
        })
        .collect();
    if members.is_empty() {
        return;
    }

    if request.mode == "assassin" {
        let enemy_vip_dead = fighters.iter().any(|(_, death, npc)| {
            npc.map(|npc| npc.in_scene_roster && npc.team == 2 && npc.is_vip)
                .unwrap_or(false)
                && death.active
        });
        let player_dead = fighters
            .iter()
            .any(|(fighter, death, _)| fighter.player_slot == 0 && death.active);
        if enemy_vip_dead {
            end.result = meteor_network::modes::ROUND_RESULT_TEAM2_WIPED;
            end.settled = true;
            end.settle_elapsed = 0.0;
            info!("assassin: enemy VIP eliminated — victory");
            return;
        }
        if player_dead {
            end.result = meteor_network::modes::ROUND_RESULT_TEAM1_WIPED;
            end.settled = true;
            end.settle_elapsed = 0.0;
            info!("assassin: player down — defeat");
            return;
        }
    }

    let round_time = (request.mode == "single")
        .then(|| {
            mission
                .script()
                .and_then(|script| script.values().get("RoundTime"))
        })
        .flatten()
        .map(|value| value.as_f64() as i32);
    let rule = if request.mode == "single" {
        mission
            .script()
            .and_then(|script| script.values().get("Rule"))
            .map_or(meteor_network::modes::DEFAULT_LEVEL_RULE, |value| {
                value.as_f64() as i32
            })
    } else {
        meteor_network::modes::mode_rule_id(&request.mode)
            .unwrap_or(meteor_network::modes::DEFAULT_LEVEL_RULE)
    };
    let result = match rule {
        4 => {
            let timeout = evaluate_round_result(end.elapsed, round_time, &[]);
            if timeout != 0 {
                timeout
            } else {
                let mut result = 0;
                if let Some(script) = mission.script() {
                    for team in [1_u8, 2] {
                        if let Some(id) = script.host().vip(team) {
                            let alive = fighters.iter().any(|(fighter, death, npc)| {
                                let matches = if id == 0 {
                                    fighter.player_slot == 0 && npc.is_none()
                                } else {
                                    npc.is_some_and(|npc| npc.id == id && npc.in_scene_roster)
                                };
                                matches && !death.active
                            });
                            if !alive {
                                result |= 1 << team;
                            }
                        }
                    }
                }
                result
            }
        }
        1..=3 => evaluate_round_result(end.elapsed, round_time, &[]),
        _ => evaluate_round_result(end.elapsed, round_time, &members),
    };
    let game_over = mission.script().and_then(|m| m.host().game_over());
    let decision = solo_round_decision(result, game_over);
    if decision != 0 {
        end.result = decision;
        end.settled = true;
        end.settle_elapsed = 0.0;
        info!("match end: result bits {decision} (round={result} game_over={game_over:?})");
    }
}

fn scene_game_end_result(winner_team: i64) -> i32 {
    if winner_team == 1 {
        meteor_network::modes::ROUND_RESULT_TEAM2_WIPED
    } else {
        meteor_network::modes::ROUND_RESULT_TEAM1_WIPED
    }
}

fn local_match_end_enabled(status: crate::plugins::network_plugin::NetStatus) -> bool {
    status != crate::plugins::network_plugin::NetStatus::InRoom
}

fn local_attack_window_authority(status: NetStatus, local: bool) -> bool {
    status != NetStatus::InRoom || local
}

pub(crate) fn forward_network_prop_hit(
    net: &NetState,
    tx: Option<&crate::plugins::network_plugin::NetTx>,
    reference: &str,
    pose_id: u16,
    frame: f32,
) -> bool {
    if net.status != NetStatus::InRoom || net.is_stock_udp() || net.is_scene_authority() {
        return false;
    }

    if net.round_active {
        if let (Some(tx), Some(match_epoch), Some(round_epoch)) =
            (tx, net.match_epoch, net.round_epoch)
        {
            tx.send_control(meteor_network::protocol::Message::ScenePropHit {
                room_id: net.room_id.clone(),
                match_epoch,
                round_epoch,
                player_id: String::new(),
                reference: reference.into(),
                pose_id,
                frame,
                damage: 0,
            });
        }
    }
    true
}

fn remote_scene_character(net: &NetState, remote: &RemoteFighter) -> (i64, Option<i64>) {
    let actor_id = if net.is_stock_udp() {
        remote
            .player_id
            .strip_prefix("stock-")
            .and_then(|id| id.parse().ok())
            .unwrap_or(-1)
    } else {
        net.members
            .iter()
            .position(|member| member.player_id == remote.player_id)
            .map_or(-1, |index| index as i64)
    };
    (actor_id, net.player_team(&remote.player_id).map(i64::from))
}

fn remote_prop_authority(net: &NetState, remote: bool) -> bool {
    remote
        && net.status == NetStatus::InRoom
        && net.round_active
        && net.is_stock_host()
        && net.mode == "siege"
}

fn solo_round_decision(round_result: i32, game_over: Option<i64>) -> i32 {
    if round_result != 0 {
        return round_result;
    }
    match game_over {
        Some(game_over) if game_over < 0 => meteor_network::modes::ROUND_RESULT_TEAM1_WIPED,
        Some(_) => meteor_network::modes::ROUND_RESULT_TEAM2_WIPED,
        None => 0,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn death_respawn(
    mut commands: Commands,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    end: Res<MatchEnd>,
    request: Res<crate::app_state::LaunchRequest>,
    net: Res<NetState>,
    mission: Option<Res<crate::plugins::stage_plugin::StageMissionRuntime>>,
    assets: Option<Res<CharacterAssetsSet>>,
    mut lock: ResMut<LockOnController>,
    mut camera_basis: ResMut<crate::plugins::camera_plugin::CameraBasis>,
    mut query: DeathRespawnQuery<'_, '_>,
    mut statuses: Query<(
        &mut crate::components::fighter::FighterStatusEffects,
        &mut crate::components::fighter::WornEffect,
    )>,
) {
    if request.mode == "replay" {
        return;
    }

    if end.settled || net.status == NetStatus::InRoom {
        return;
    }

    let rule = if request.mode == "single" {
        Some(mission.as_deref().map_or(
            meteor_network::modes::DEFAULT_LEVEL_RULE,
            crate::plugins::stage_plugin::StageMissionRuntime::rule,
        ))
    } else {
        meteor_network::modes::mode_rule_id(&request.mode)
    };
    if !meteor_network::modes::can_respawn(rule) {
        return;
    }
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    for (
        entity,
        fighter,
        mut death,
        mut health,
        mut qi,
        mut combat,
        mut pose,
        mut sm_state,
        mut transform,
        mut target_hit_clock,
        (
            mut blend,
            mut motion,
            mut air,
            mut dodge,
            mut attack_motion,
            mut hit_reaction,
            mut gun_aim,
            mut ai_intent,
            mut wall,
        ),
        mission_npc,
        local,
    ) in &mut query
    {
        if mission_npc.is_some() {
            continue;
        }
        if !death.active {
            continue;
        }
        death.timer -= dt;
        if death.timer > 0.0 {
            continue;
        }

        if let Ok((mut status, mut worn)) = statuses.get_mut(entity) {
            status.clear(&mut worn);
        }
        death.active = false;
        death.timer = 0.0;
        health.current = health.max;
        qi.current = 0.0;
        let character_assets = assets
            .as_deref()
            .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)));
        reset_fighter_runtime_for_respawn(
            character_assets,
            &mut combat,
            &mut pose,
            &mut blend,
            &mut sm_state,
            &mut motion,
            &mut air,
            &mut dodge,
            &mut attack_motion,
            &mut hit_reaction,
            &mut gun_aim,
            ai_intent.as_deref_mut(),
            wall.as_deref_mut(),
        );
        commands.entity(entity).remove::<WeaponSwitchActive>();
        transform.translation = death.spawn_pos;
        if local.is_some() {
            let _ = clear_lock_targets(&mut lock);
            camera_basis.yaw = motion.facing;
        }

        target_hit_clock.reset_spawn();
        target_hit_clock.reset();
    }
}

type DefaultPosesQuery<'w, 's> = Query<
    'w,
    's,
    (
        (Entity, &'static Fighter),
        &'static mut PoseRuntime,
        &'static mut EntryBlend,
        &'static SmState,
        &'static MotionState,
        &'static FighterDodge,
        Option<&'static crate::plugins::movement_plugin::FighterAttackMotion>,
        Option<&'static mut crate::plugins::movement_plugin::FighterWall>,
        Option<&'static WeaponSwitchActive>,
        Option<&'static LocalFighter>,
        &'static GunAimState,
        &'static WeaponForm,
        Option<&'static AiIntent>,
        &'static FighterHitReaction,
        Option<&'static mut crate::plugins::movement_plugin::FighterAir>,
    ),
    (With<Fighter>, Without<RemoteFighter>),
>;

fn guard_stance_pose(character_assets: &CharacterAssets, strategy_id: u8) -> u16 {
    guard_pose_for_weapon(strategy_id)
        .into_iter()
        .chain([77, 80, 83, 86])
        .find(|pose_id| character_assets.poses.contains_key(pose_id))
        .unwrap_or(L::IDLE)
}

fn guard_reaction_owns_pose(hit_reaction: &FighterHitReaction) -> bool {
    hit_reaction
        .current
        .as_ref()
        .is_some_and(|reaction| reaction.kind == HitReactKind::Guard)
}

fn preadvance_local_ground_locomotion_pose(
    assets: Option<Res<CharacterAssetsSet>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keymap: Res<crate::app_state::Keymap>,
    player_performance: Option<Res<crate::plugins::stage_plugin::PlayerPerformance>>,
    lock: Res<LockOnController>,
    statuses: Query<&crate::components::fighter::FighterStatusEffects>,
    ranged: Query<&RangedFireGate>,
    buffs: Option<Res<crate::plugins::item_plugin::ItemBuffs>>,
    mut query: DefaultPosesQuery<'_, '_>,
) {
    let Some(assets) = assets else {
        return;
    };
    for (
        (entity, fighter),
        mut pose,
        mut blend,
        sm_state,
        motion,
        dodge,
        attack_motion,
        wall,
        switching,
        local,
        gun_aim,
        weapon_form,
        _ai,
        hit_reaction,
        mut air,
    ) in &mut query
    {
        let base_speed = if local.is_some() {
            buffs.as_ref().map_or(1.0, |buffs| buffs.speed_factor)
        } else {
            1.0
        };
        let slow = statuses
            .get(entity)
            .map_or(base_speed, |status| status.effects.speed_factor(base_speed))
            < 0.7;
        if (sm_state.current == FighterState::Name
            && ranged.get(entity).is_ok_and(|gate| gate.wheel_out))
            || local.is_none()
            || switching.is_some()
            || attack_motion.is_some_and(|motion| motion.active)
            || dodge.state.active
            || wall.as_ref().is_some_and(|wall| {
                wall.cling_active || (wall.kick_pose != 0 && wall.kick_pose == pose.pose_id)
            })
            || air.as_ref().is_some_and(|air| air.air.airborne)
            || !matches!(
                motion.movement_mode,
                meteor_core::MovementMode::Idle
                    | meteor_core::MovementMode::Walk
                    | meteor_core::MovementMode::Run
                    | meteor_core::MovementMode::Crouch
            )
        {
            continue;
        }
        if sm_state.current == FighterState::Guard {
            if guard_reaction_owns_pose(hit_reaction) {
                continue;
            }
            let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
                continue;
            };
            let strategy_id = weapon_to_strategy(fighter.weapon);
            let head = guard_stance_pose(character_assets, strategy_id);
            if pose.pose_id != head {
                character_assets.apply_pose_with_blend(
                    &mut pose,
                    &mut blend,
                    head,
                    true,
                    None,
                    EntryBlendKind::Generic,
                    false,
                    false,
                    Some(strategy_id),
                );
            }
            continue;
        }
        if sm_state.current != FighterState::Name {
            continue;
        }
        if sm_state.linked_recovery_pose_id == Some(pose.pose_id) && !pose.finished() {
            continue;
        }
        if gun_aim.aiming && pose.pose_id == GUN_AIM_READY_POSE && !pose.finished() {
            continue;
        }
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        let input = if player_performance
            .as_deref()
            .is_some_and(|performance| performance.input_blocked)
        {
            Vec3::ZERO
        } else {
            crate::plugins::movement_plugin::directional_input(&keys, &mouse, &keymap)
        };
        let strategy_id = weapon_to_strategy(fighter.weapon);
        let mut wanted = walk_pose_for_input(
            input.x,
            input.z,
            WalkOptions {
                slow,
                crouching: motion.movement_mode == meteor_core::MovementMode::Crouch,
                strategy_id: Some(strategy_id),
                locked: lock.lock_entity.is_some(),
                aiming: gun_aim.aiming,
                weapon_form: weapon_form.current,
                ..WalkOptions::default()
            },
        );
        if !character_assets.poses.contains_key(&wanted) {
            wanted = walk_pose_for_input(
                input.x,
                input.z,
                WalkOptions {
                    slow,
                    crouching: motion.movement_mode == meteor_core::MovementMode::Crouch,
                    ..WalkOptions::default()
                },
            );
        }
        let force_ground_entry = take_ground_entry_blend(air.as_deref_mut(), motion.movement_mode);
        if pose.pose_id != wanted {
            let hard = !can_keep_locomotion_phase(pose.pose_id, wanted);
            character_assets.apply_pose_with_blend(
                &mut pose,
                &mut blend,
                wanted,
                hard,
                None,
                EntryBlendKind::Generic,
                false,
                force_ground_entry,
                Some(strategy_id),
            );
        }
    }
}

fn airborne_locomotion_pose(
    current_pose: u16,
    fall_hold: bool,
    ledge_fall: bool,
    vertical_speed: f32,
    has_jump_arc: bool,
) -> u16 {
    if ledge_fall && vertical_speed < meteor_physics::jump::FALL_POSE_SPEED && !fall_hold {
        return meteor_physics::jump::fall_pose_for(current_pose);
    }
    if is_jump_pose_id(current_pose)
        || ledge_fall
        || current_pose == WEAPON_SWITCH_POSE_AIR
        || fall_hold
    {
        current_pose
    } else if has_jump_arc {
        L::JUMP_ARC
    } else {
        current_pose
    }
}

fn default_poses(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    assets: Option<Res<CharacterAssetsSet>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keymap: Res<crate::app_state::Keymap>,
    player_performance: Option<Res<crate::plugins::stage_plugin::PlayerPerformance>>,
    lock: Res<LockOnController>,
    statuses: Query<&crate::components::fighter::FighterStatusEffects>,
    ranged: Query<&RangedFireGate>,
    buffs: Option<Res<crate::plugins::item_plugin::ItemBuffs>>,
    mut query: DefaultPosesQuery<'_, '_>,
) {
    let Some(assets) = assets else {
        return;
    };
    for (
        (entity, fighter),
        mut pose,
        mut blend,
        sm_state,
        motion,
        dodge,
        attack_motion,
        mut wall,
        switching,
        local,
        gun_aim,
        weapon_form,
        ai,
        hit_reaction,
        mut air,
    ) in &mut query
    {
        let base_speed = if local.is_some() {
            buffs.as_ref().map_or(1.0, |buffs| buffs.speed_factor)
        } else {
            1.0
        };
        let slow = statuses
            .get(entity)
            .map_or(base_speed, |status| status.effects.speed_factor(base_speed))
            < 0.7;
        if !super::fighter_frame::accepts(active_fighter.as_deref(), entity) {
            continue;
        }
        if sm_state.current == FighterState::Name
            && !air.as_ref().is_some_and(|air| air.air.airborne)
            && ranged.get(entity).is_ok_and(|gate| gate.wheel_out)
        {
            continue;
        }

        if switching.is_some() {
            continue;
        }

        if attack_motion.is_some_and(|motion| motion.active) {
            continue;
        }
        if is_attacking_state(sm_state.current)
            || matches!(
                sm_state.current,
                FighterState::BeHit
                    | FighterState::Dead
                    | FighterState::Down
                    | FighterState::Fake
                    | FighterState::Cure
            )
        {
            continue;
        }

        if sm_state.linked_recovery_pose_id == Some(pose.pose_id) && !pose.finished() {
            continue;
        }
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        let strategy_id = weapon_to_strategy(fighter.weapon);

        if sm_state.current == FighterState::Guard {
            if guard_reaction_owns_pose(hit_reaction) {
                continue;
            }
            let head = guard_stance_pose(character_assets, strategy_id);
            if pose.pose_id != head {
                character_assets.apply_pose_with_blend(
                    &mut pose,
                    &mut blend,
                    head,
                    true,
                    None,
                    EntryBlendKind::Generic,
                    false,
                    false,
                    Some(strategy_id),
                );
            }
            continue;
        }

        if gun_aim.aiming && pose.pose_id == GUN_AIM_READY_POSE && !pose.finished() {
            continue;
        }

        if let Some(wall) = wall.as_deref_mut() {
            if wall.kick_pose != 0 {
                if pose.pose_id == wall.kick_pose && !pose.finished() {
                    continue;
                }
                wall.kick_pose = 0;
            }
        }
        if dodge.state.active {
            if pose.pose_id != dodge.pose {
                character_assets.apply_pose_with_blend(
                    &mut pose,
                    &mut blend,
                    dodge.pose,
                    true,
                    None,
                    EntryBlendKind::Generic,
                    false,
                    false,
                    Some(strategy_id),
                );
            }
            continue;
        }
        if let Some(wall) = wall.as_ref() {
            if wall.cling_active {
                if pose.pose_id != wall.cling_pose {
                    character_assets.apply_pose_with_blend(
                        &mut pose,
                        &mut blend,
                        wall.cling_pose,
                        true,
                        None,
                        EntryBlendKind::Generic,
                        true,
                        false,
                        Some(strategy_id),
                    );
                }
                continue;
            }
        }

        if air.as_deref().is_some_and(|air| {
            air.air_attack_recovery_pose == Some(pose.pose_id) && pose.finished()
        }) {
            continue;
        }
        let attack_ground_pose = air
            .as_deref()
            .is_some_and(|air| air.attack_ground_pose_pending)
            && sm_state.current == FighterState::Name;
        if let Some(air) = air.as_deref_mut() {
            air.attack_ground_pose_pending = false;
        }
        let ledge_fall = air.as_deref().is_some_and(|air| air.ledge_fall);
        let strategy_sample = air.as_deref().and_then(|air| air.locomotion_pose_sample);
        let vertical_speed = strategy_sample.map_or_else(
            || air.as_deref().map_or(0.0, |air| air.air.vertical_speed),
            |(_, speed)| speed,
        );
        let pose_mode = if attack_ground_pose {
            meteor_core::MovementMode::Idle
        } else if strategy_sample.is_some_and(|(ground, _)| !ground) {
            meteor_core::MovementMode::Airborne
        } else {
            motion.movement_mode
        };
        let pose_id = match pose_mode {
            meteor_core::MovementMode::Airborne => airborne_locomotion_pose(
                pose.pose_id,
                pose.is_fall_hold(),
                ledge_fall,
                vertical_speed,
                character_assets.poses.contains_key(&L::JUMP_ARC),
            ),
            meteor_core::MovementMode::WallRun | meteor_core::MovementMode::Dodge => L::IDLE,
            meteor_core::MovementMode::Idle
            | meteor_core::MovementMode::Walk
            | meteor_core::MovementMode::Run
            | meteor_core::MovementMode::Crouch => {
                let (ix, iz) = if local.is_some()
                    && !player_performance
                        .as_deref()
                        .is_some_and(|performance| performance.input_blocked)
                {
                    let input =
                        crate::plugins::movement_plugin::directional_input(&keys, &mouse, &keymap);
                    (input.x, input.z)
                } else if let Some(ai) = ai {
                    (ai.locomotion_input.x, ai.locomotion_input.y)
                } else {
                    (0.0, 0.0)
                };
                let mut want = walk_pose_for_input(
                    ix,
                    iz,
                    WalkOptions {
                        slow,
                        crouching: motion.movement_mode == meteor_core::MovementMode::Crouch,
                        strategy_id: Some(weapon_to_strategy(fighter.weapon)),
                        locked: actor_uses_local_lock(local.is_some(), lock.lock_entity.is_some()),
                        aiming: gun_aim.aiming,
                        weapon_form: weapon_form.current,
                        ..WalkOptions::default()
                    },
                );

                if !character_assets.poses.contains_key(&want) {
                    want = walk_pose_for_input(
                        ix,
                        iz,
                        WalkOptions {
                            slow,
                            crouching: motion.movement_mode == meteor_core::MovementMode::Crouch,
                            ..WalkOptions::default()
                        },
                    );
                }
                want
            }
        };
        let force_ground_entry = take_ground_entry_blend(air.as_deref_mut(), motion.movement_mode);
        if pose.pose_id != pose_id {
            let hard = !can_keep_locomotion_phase(pose.pose_id, pose_id);
            let skip_blend = jump_transition_is_hard_cut(pose.pose_id, pose_id);
            character_assets.apply_pose_with_blend(
                &mut pose,
                &mut blend,
                pose_id,
                hard,
                None,
                EntryBlendKind::Generic,
                skip_blend,
                force_ground_entry,
                Some(strategy_id),
            );
        }
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;
    mod story_rule_tests {
        include!("combat_plugin/story_rule_tests.rs");
    }

    mod sn13_slope_tests {
        include!("combat_plugin/sn13_slope_tests.rs");
    }

    mod crowd_schedule_tests {
        include!("combat_plugin/crowd_schedule_tests.rs");
    }

    fn settlement_test_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time<bevy::time::Real>>()
            .init_resource::<Time>()
            .init_resource::<crate::plugins::FrameClock>()
            .init_resource::<MatchEnd>()
            .init_resource::<crate::app_state::LaunchRequest>()
            .init_resource::<NextState<AppState>>()
            .insert_resource(State::new(AppState::Playing))
            .init_resource::<crate::plugins::stage_plugin::StageMissionRuntime>()
            .init_resource::<crate::plugins::stage_plugin::StageScriptRuntime>()
            .init_resource::<NetState>()
            .add_systems(
                PreUpdate,
                advance_match_settlement.run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                Update,
                evaluate_match_end.run_if(crate::plugins::simulation_frame),
            );
        app
    }

    #[test]
    fn match_settlement_uses_authored_round_time_and_preserves_network_authority() {
        use crate::plugins::stage_plugin::StageMissionRuntime;
        use bevy::ecs::system::RunSystemOnce;
        use meteor_scene::mission_runtime::{parse_mission_definition, MissionScriptRuntime};

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        for (stage, limit) in [("sn01", 600.0), ("sn15", 1500.0), ("", 900.0)] {
            let source = if stage.is_empty() {
                String::new()
            } else {
                crate::text_codec::decode_text(
                    &std::fs::read(root.join(format!("Level/{stage}_.pst"))).unwrap(),
                )
            };
            let definition = parse_mission_definition(&source);
            let mut app = settlement_test_app();
            app.insert_resource(StageMissionRuntime::from_script(MissionScriptRuntime::new(
                &source,
                definition.values,
            )));
            for slot in [0, 1] {
                app.world_mut().spawn((
                    Fighter {
                        character_id: 0,
                        player_slot: slot,
                        weapon: 5,
                        weapon_slots: [5, 0],
                        weapon_slot: 0,
                        weapon_slot_variants: [0, 0],
                        weapon_variant: 0,
                    },
                    DeathRespawn::default(),
                ));
            }
            let mut times = vec![limit - 0.01, limit, limit + 0.01];
            if stage == "sn15" {
                times.insert(0, 900.0);
            }
            for elapsed in times {
                *app.world_mut().resource_mut::<MatchEnd>() = MatchEnd {
                    elapsed,
                    ..default()
                };
                app.world_mut().run_system_once(evaluate_match_end).unwrap();
                let end = app.world().resource::<MatchEnd>();
                assert_eq!(end.settled, elapsed >= limit, "{stage} at {elapsed}");
                assert_eq!(end.result, if elapsed >= limit { 1 } else { 0 });
            }
            *app.world_mut().resource_mut::<MatchEnd>() = MatchEnd {
                elapsed: limit + 1.0,
                ..default()
            };
            app.world_mut().resource_mut::<NetState>().status = NetStatus::InRoom;
            app.world_mut().run_system_once(evaluate_match_end).unwrap();
            assert!(!app.world().resource::<MatchEnd>().settled);
        }
    }

    #[test]
    fn match_settlement_uses_real_time_during_long_frames_slow_motion_and_hitstop() {
        for sim_dt in [0.05, 0.00625, 0.0] {
            let mut app = settlement_test_app();
            app.world_mut()
                .resource_mut::<crate::plugins::FrameClock>()
                .sim_dt = sim_dt;
            *app.world_mut().resource_mut::<MatchEnd>() = MatchEnd {
                result: 4,
                settled: true,
                elapsed: 123.0,
                ..default()
            };

            for _ in 0..79 {
                app.world_mut()
                    .resource_mut::<Time<bevy::time::Real>>()
                    .advance_by(std::time::Duration::from_millis(125));
                app.update();
            }
            assert_eq!(app.world().resource::<MatchEnd>().settle_elapsed, 9.875);
            assert!(matches!(
                app.world().resource::<NextState<AppState>>(),
                NextState::Unchanged
            ));
            app.world_mut()
                .resource_mut::<Time<bevy::time::Real>>()
                .advance_by(std::time::Duration::from_millis(125));
            app.update();
            assert_eq!(app.world().resource::<MatchEnd>().settle_elapsed, 10.0);
            assert_eq!(app.world().resource::<MatchEnd>().elapsed, 123.0);
            assert!(matches!(
                app.world().resource::<NextState<AppState>>(),
                NextState::Pending(AppState::Result)
            ));
        }
    }

    #[test]
    fn match_settlement_waits_for_decision_and_respects_pause_and_replay() {
        let mut app = settlement_test_app();
        app.world_mut()
            .resource_mut::<Time<bevy::time::Real>>()
            .advance_by(std::time::Duration::from_secs(20));
        app.update();
        assert_eq!(app.world().resource::<MatchEnd>().settle_elapsed, 0.0);

        app.world_mut().resource_mut::<MatchEnd>().settled = true;
        app.world_mut()
            .insert_resource(State::new(AppState::Paused));
        app.update();
        assert_eq!(app.world().resource::<MatchEnd>().settle_elapsed, 0.0);

        app.world_mut()
            .insert_resource(State::new(AppState::Playing));
        app.world_mut()
            .resource_mut::<crate::app_state::LaunchRequest>()
            .mode = "replay".into();
        app.update();
        assert_eq!(app.world().resource::<MatchEnd>().settle_elapsed, 0.0);
        assert!(matches!(
            app.world().resource::<NextState<AppState>>(),
            NextState::Unchanged
        ));

        app.world_mut()
            .resource_mut::<crate::app_state::LaunchRequest>()
            .mode = "single".into();
        app.world_mut()
            .resource_mut::<Time<bevy::time::Real>>()
            .advance_by(std::time::Duration::from_secs(1));
        app.update();
        assert_eq!(app.world().resource::<MatchEnd>().settle_elapsed, 1.0);
    }

    #[derive(Component)]
    struct LiveTransformProbe;

    #[derive(Resource, Default)]
    struct LiveTransformResult(Vec3);

    fn sample_live_transform(
        transforms: TransformHelper<'_, '_>,
        probe: Query<(Entity, &GlobalTransform), With<LiveTransformProbe>>,
        mut result: ResMut<LiveTransformResult>,
    ) {
        let (entity, propagated) = probe.single().unwrap();
        result.0 = current_global_transform(entity, propagated, &transforms).translation();
    }

    #[test]
    fn lock_visibility_grace_resets_on_recovery_target_change_and_release() {
        let mut state = LockVisibility::default();
        assert!(!state.expired(Some(1), false, 0.3));
        assert!(!state.expired(Some(1), true, 0.1));
        assert!(!state.expired(Some(1), false, 0.3));
        assert!(!state.expired(Some(2), false, 0.3));
        assert!(state.expired(Some(2), false, 0.31));
        assert!(!state.expired(None, false, 1.0));
        assert!(!state.expired(Some(2), false, 0.1));
    }

    #[test]
    fn hit_geometry_uses_current_local_hierarchy_not_previous_propagated_frame() {
        let mut app = App::new();
        app.init_resource::<LiveTransformResult>()
            .add_systems(Update, sample_live_transform);
        let parent = app
            .world_mut()
            .spawn((
                Transform::from_xyz(10.0, 0.0, 0.0),
                GlobalTransform::from_translation(Vec3::new(100.0, 0.0, 0.0)),
            ))
            .id();
        app.world_mut().spawn((
            LiveTransformProbe,
            Transform::from_xyz(2.0, 0.0, 0.0),
            GlobalTransform::from_translation(Vec3::new(200.0, 0.0, 0.0)),
            ChildOf(parent),
        ));

        app.update();

        assert_eq!(
            app.world().resource::<LiveTransformResult>().0,
            Vec3::new(12.0, 0.0, 0.0),
            "judge must see this frame's sampled bone/weapon hierarchy"
        );
    }

    #[test]
    fn hidden_weapon_collision_part_never_builds_an_attack_obb() {
        let fighter = Entity::from_bits(7);
        let geometry = WeaponCollisionGeometry {
            fighter,
            weapon: 2,
            slot: 0,
            variant: 0,
            part: "w1_0".into(),
            left: false,
            boxes: Vec::new(),
            forms: None,
        };

        assert!(weapon_collision_part_is_active(
            &geometry,
            &Visibility::Visible,
            fighter,
            2,
            0,
            0,
            0,
        ));
        assert!(!weapon_collision_part_is_active(
            &geometry,
            &Visibility::Hidden,
            fighter,
            2,
            0,
            0,
            0,
        ));
    }

    #[test]
    fn prop_hit_stops_real_box_break_replay_before_render_sync() {
        use meteor_scene::scene_script_runtime::{PstValue, SceneItemInstance, SceneScriptRuntime};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut source = String::new();
        for file in ["Def.h", "box.pst"] {
            source.push_str(&meteor_scene::text::decode_cp936(
                &std::fs::read(root.join("PScriptLib").join(file)).unwrap(),
            ));
            source.push('\n');
        }
        source.push_str("Scene_OnInit() { InitBoxes(1); InitBBoxes(1); }");
        let instances = ["D_BBox01", "D_BBBox01"]
            .iter()
            .enumerate()
            .map(|(index, name)| SceneItemInstance {
                name: (*name).into(),
                index,
                props: std::collections::HashMap::from([
                    ("active".into(), PstValue::Num(1.0)),
                    ("interactive".into(), PstValue::Num(1.0)),
                    ("_has_fmc".into(), PstValue::Num(1.0)),
                ]),
            })
            .collect();
        let mut runtime = SceneScriptRuntime::load(&source, instances, 250_000).unwrap();
        runtime.start().unwrap();
        runtime.host_mut().take_net_ops();
        for name in ["D_BBox01", "D_BBBox01"] {
            assert!(script_allows_prop_hit(&runtime, name));
            runtime
                .call_item(
                    name,
                    "OnAttack",
                    &[PstValue::Num(0.0), PstValue::Num(1000.0)],
                )
                .unwrap();
            let index = runtime.host().resolve_name(name).unwrap();
            let revision = runtime
                .host()
                .instance(index)
                .unwrap()
                .get("_pose_revision");
            runtime.host_mut().take_net_ops();
            assert_eq!(runtime.host_mut().take_effects().len(), 1);
            runtime.set_item_property(name, "anim_time", PstValue::Num(0.25));

            for _ in 0..4 {
                assert!(!script_allows_prop_hit(&runtime, name));
                if script_allows_prop_hit(&runtime, name) {
                    runtime
                        .call_item(
                            name,
                            "OnAttack",
                            &[PstValue::Num(0.0), PstValue::Num(1000.0)],
                        )
                        .unwrap();
                }
            }
            let instance = runtime.host().instance(index).unwrap();
            assert_eq!(instance.get("active"), PstValue::Num(1.0));
            assert_eq!(instance.get("state"), PstValue::Num(1.0));
            assert_eq!(instance.get("anim_time"), PstValue::Num(0.25));
            assert_eq!(instance.get("_pose_revision"), revision);
            assert!(runtime.host_mut().take_effects().is_empty());
            assert!(runtime.host_mut().take_net_ops().is_empty());
            runtime.set_item_property(name, "state", PstValue::Num(3.0));
            runtime.call_item(name, "OnIdle", &[]).unwrap();
            assert_eq!(
                runtime.host().instance(index).unwrap().get("active"),
                PstValue::Num(0.0)
            );
            assert!(!script_allows_prop_hit(&runtime, name));
        }
    }

    #[test]
    fn prop_hit_obb_rotates_the_authored_center_offset_into_world_space() {
        let prop = crate::plugins::item_plugin::StageProp {
            name: "D_BBox01".into(),
            model: "bbox02".into(),

            center_offset: Vec3::new(0.0, 0.0, 1.593_381_4),
            half_extents: Vec3::new(1.592_666, 1.592_666, 1.579_031),
            hit_count: 0,
            respawn_timer: 0.0,
            interactive: true,
            collision_enabled: true,
            breakable: true,
            broken: false,
        };
        let transform =
            Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2));
        let prop_box = stage_prop_hit_obb(&prop, &transform);
        assert!(
            (prop_box.center.y - 1.593_381_4).abs() < 1e-4,
            "Z-up model center must become the box's world Y center"
        );

        let attack = Obb {
            center: meteor_core::Vec3::new(0.0, 3.0, 0.0),
            axes: [
                meteor_core::Vec3::new(1.0, 0.0, 0.0),
                meteor_core::Vec3::new(0.0, 1.0, 0.0),
                meteor_core::Vec3::new(0.0, 0.0, 1.0),
            ],
            half_size: meteor_core::Vec3::new(0.2, 0.2, 0.2),
        };
        assert!(obbs_intersect(&attack, &prop_box));
        let old_origin_centered = Obb {
            center: meteor_core::Vec3::new(0.0, 0.0, 0.0),
            ..prop_box
        };
        assert!(!obbs_intersect(&attack, &old_origin_centered));
    }

    fn mission_npc_with_team(team: u8) -> MissionNpc {
        MissionNpc {
            id: 1,
            name: "npc".into(),
            team,
            in_scene_roster: true,
            view: 100.0,
            think: 50.0,
            forced_behavior: None,
            is_vip: false,
            attack_target_slot: None,
            behavior_args: Vec::new(),
            attack_target_remaining: 0,
        }
    }

    #[test]
    fn production_combat_teams_follow_mission_and_network_authority() {
        let offline = NetState::default();
        let ally = mission_npc_with_team(1);
        let enemy = mission_npc_with_team(2);
        let player_team = fighter_combat_team(&offline, true, None, None);
        assert_eq!(player_team, 1);
        assert!(!teams_are_hostile(
            player_team,
            fighter_combat_team(&offline, false, None, Some(&ally))
        ));
        assert!(teams_are_hostile(
            player_team,
            fighter_combat_team(&offline, false, None, Some(&enemy))
        ));
        assert!(teams_are_hostile(
            player_team,
            fighter_combat_team(&offline, false, None, None)
        ));

        let mut network = NetState {
            status: NetStatus::InRoom,
            player_id: "local".into(),
            ..default()
        };
        network.members = vec![
            meteor_network::protocol::RoomMemberWire {
                connected: true,
                player_id: "local".into(),
                name: "local".into(),
                character_id: 0,
                weapon: 6,
                team: Some(2),
                ready: true,
            },
            meteor_network::protocol::RoomMemberWire {
                connected: true,
                player_id: "peer".into(),
                name: "peer".into(),
                character_id: 1,
                weapon: 6,
                team: Some(2),
                ready: true,
            },
        ];
        let remote = RemoteFighter {
            player_id: "peer".into(),
        };
        assert_eq!(fighter_combat_team(&network, true, None, None), 2);
        assert_eq!(fighter_combat_team(&network, false, Some(&remote), None), 2);
        let network_npc = RemoteFighter {
            player_id: "npc:7".into(),
        };
        network.remote_actor_teams.insert("npc:7".into(), 1);
        assert_eq!(
            fighter_combat_team(&network, false, Some(&network_npc), None),
            1
        );
        assert!(!teams_are_hostile(2, 2));
    }

    #[test]
    fn ws_sn04_remote_attacks_unlock_buns_and_end_siege() {
        use meteor_scene::scene_script_runtime::{PstValue, SceneItemInstance, SceneScriptRuntime};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut source = String::new();
        for path in [
            "PScriptLib/Def.h",
            "PScriptLib/box.pst",
            "PScriptLib/desk.pst",
            "PScriptLib/chair.pst",
            "PScriptLib/jug.pst",
            "sn04/sn04.pst",
        ] {
            source.push_str(&meteor_scene::text::decode_cp936(
                &std::fs::read(root.join(path)).unwrap(),
            ));
            source.push('\n');
        }
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn04/sn04.des")).unwrap(),
        );
        let instances = desc
            .dummies
            .iter()
            .enumerate()
            .map(|(index, dummy)| SceneItemInstance {
                name: dummy.name.clone(),
                index,
                props: [
                    ("active".into(), PstValue::Num(1.0)),
                    ("interactive".into(), PstValue::Num(1.0)),
                    ("state".into(), PstValue::Num(3.0)),
                ]
                .into_iter()
                .collect(),
            })
            .collect();
        let mut runtime = SceneScriptRuntime::load(&source, instances, 250_000).unwrap();
        runtime.host_mut().set_game_mode(3);
        runtime.start().unwrap();
        let mut net = NetState {
            status: NetStatus::InRoom,
            player_id: "host".into(),
            address: "ws://localhost".into(),
            room_id: "room1".into(),
            match_epoch: Some(1),
            round_epoch: Some(1),
            mode: "siege".into(),
            round_active: true,
            ..Default::default()
        };
        for (player_id, team) in [("host", 1), ("guest", 2)] {
            net.members.push(meteor_network::protocol::RoomMemberWire {
                player_id: player_id.into(),
                name: player_id.into(),
                character_id: 0,
                weapon: 6,
                team: Some(team),
                ready: true,
                connected: true,
            });
        }
        let remote = RemoteFighter {
            player_id: "guest".into(),
        };
        assert!(
            !remote_prop_authority(&net, true),
            "WS uses reliable prop reports"
        );
        let (id, team) = remote_scene_character(&net, &remote);
        assert_eq!((id, team), (1, Some(2)));
        runtime.host_mut().set_character_team(id, team.unwrap());
        runtime.host_mut().take_net_ops();
        let bun = runtime.host().resolve_name("D_itRJug0101").unwrap();
        assert_eq!(
            runtime.host().instance(bun).unwrap().get("interactive"),
            PstValue::Num(0.0)
        );
        let report = |reference: &str| meteor_network::protocol::Message::ScenePropHit {
            room_id: "room1".into(),
            match_epoch: 1,
            round_epoch: 1,
            player_id: "guest".into(),
            reference: reference.into(),
            pose_id: 262,
            frame: 9770.0,
            damage: 10001,
        };
        crate::plugins::stage_plugin::apply_network_prop_hit(
            &mut runtime,
            &net,
            report("D_RJug01"),
        );
        assert_eq!(
            runtime.host().instance(bun).unwrap().get("interactive"),
            PstValue::Num(1.0)
        );
        assert!(!runtime.host_mut().take_net_ops().is_empty());

        crate::plugins::stage_plugin::apply_network_prop_hit(
            &mut runtime,
            &net,
            report("D_BPdoor"),
        );
        assert!(runtime.host().game_end().is_none());
        crate::plugins::stage_plugin::apply_network_prop_hit(
            &mut runtime,
            &net,
            report("D_APdoor"),
        );
        runtime.set_item_property("D_APdoor", "state", PstValue::Num(3.0));
        runtime.call_item("D_APdoor", "OnIdle", &[]).unwrap();
        assert_eq!(runtime.host().game_end(), Some(2));

        {
            use bevy::ecs::system::RunSystemOnce;
            let (tx, drain) = crate::plugins::network_plugin::control_test_channel();
            let mut app = App::new();
            app.init_resource::<Time>()
                .init_resource::<MatchEnd>()
                .init_resource::<crate::app_state::LaunchRequest>()
                .init_resource::<crate::plugins::stage_plugin::StageMissionRuntime>()
                .insert_resource(
                    crate::plugins::stage_plugin::StageScriptRuntime::from_script(runtime),
                )
                .insert_resource(tx);
            app.insert_resource(net);
            app.world_mut().run_system_once(evaluate_match_end).unwrap();
            assert!(matches!(
                drain().as_slice(),
                [meteor_network::protocol::Message::ObjectiveEnd { winner_team: 2 }]
            ));
            assert!(!app.world().resource::<MatchEnd>().settled);
            app.world_mut().run_system_once(evaluate_match_end).unwrap();
            assert!(drain().is_empty(), "objective result is submitted once");
            net = app.world_mut().remove_resource::<NetState>().unwrap();
        }
        net.player_id = "guest".into();
        assert!(!remote_prop_authority(&net, true));
        net.player_id = "host".into();
        net.mode = "deathmatch".into();
        assert!(
            !remote_prop_authority(&net, true),
            "WS must not double-count reports through replay"
        );
        net.round_active = false;
        assert!(!remote_prop_authority(&net, true));
    }

    #[test]
    fn ranged_defenders_are_processed_nearest_first() {
        let near = Entity::from_bits(11);
        let middle = Entity::from_bits(12);
        let far = Entity::from_bits(13);
        let mut candidates = vec![(far, 8.0), (near, 1.5), (middle, 4.0)];

        sort_ranged_defender_candidates(&mut candidates);

        assert_eq!(
            candidates
                .into_iter()
                .map(|(entity, _)| entity)
                .collect::<Vec<_>>(),
            vec![near, middle, far]
        );
    }

    #[test]
    fn lock_candidate_ties_follow_creation_order_not_query_order() {
        let first_created = Entity::from_bits(11);
        let second_created = Entity::from_bits(12);
        let third_created = Entity::from_bits(13);
        let make = |entity, z| OrderedLockCandidate {
            entity,
            candidate: LockCandidate {
                id: entity.to_bits(),
                x: 0.0,
                y: 0.0,
                z,
                team: 2,
            },
        };
        let candidates = stable_lock_candidates(vec![
            make(third_created, 2.0),
            make(second_created, 3.0),
            make(first_created, 4.0),
        ]);

        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>(),
            vec![
                first_created.to_bits(),
                second_created.to_bits(),
                third_created.to_bits(),
            ]
        );
        assert_eq!(
            meteor_combat::lock_on::pick_lock_target((0.0, 0.0, 0.0), 0.0, &candidates)
                .unwrap()
                .id,
            first_created.to_bits(),
            "equal-angle targets retain the first stable actor"
        );
    }

    #[test]
    fn production_track_candidates_include_hostile_network_remote() {
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
        let mut net = NetState {
            status: NetStatus::InRoom,
            player_id: "local".into(),
            ..default()
        };
        net.members = vec![
            meteor_network::protocol::RoomMemberWire {
                connected: true,
                player_id: "local".into(),
                name: "local".into(),
                character_id: 0,
                weapon: 6,
                team: Some(1),
                ready: true,
            },
            meteor_network::protocol::RoomMemberWire {
                connected: true,
                player_id: "peer".into(),
                name: "peer".into(),
                character_id: 1,
                weapon: 6,
                team: Some(2),
                ready: true,
            },
        ];
        let mut app = App::new();
        app.insert_resource(time)
            .insert_resource(net)
            .init_resource::<LockOnController>()
            .init_resource::<crate::plugins::camera_plugin::CameraBasis>()
            .add_systems(Update, (lock_on_tick, local_attack_lock_facing).chain());
        let local = app
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
                SmState::default(),
                PoseRuntime::default(),
                EntryBlend::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                Transform::default(),
                HeadTrackState::default(),
            ))
            .id();
        let remote = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "peer".into(),
                },
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
                Transform::from_xyz(3.0, 0.0, 0.1),
            ))
            .id();

        app.update();

        let lock = app.world().resource::<LockOnController>();
        assert_eq!(lock.track_entity, Some(remote));
        assert_eq!(lock.system.track_target, Some(remote.to_bits()));
        assert!(
            (app.world().get::<HeadTrackState>(local).unwrap().current
                - meteor_combat::lock_on::TRACK_TURN_LIMIT / 3.0)
                .abs()
                < 1e-5
        );
    }

    #[test]
    fn production_idle_lock_releases_at_range_or_after_losing_sight() {
        for (distance, occluded) in [(29.0, false), (3.0, true), (3.0, false)] {
            let mut time: Time = Time::default();
            time.advance_by(std::time::Duration::from_secs_f32(0.05));
            let mut app = App::new();
            app.insert_resource(time)
                .init_resource::<NetState>()
                .init_resource::<LockOnController>()
                .init_resource::<crate::plugins::camera_plugin::CameraBasis>()
                .add_systems(Update, (lock_on_tick, local_attack_lock_facing).chain());
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
                Health {
                    current: 100.0,
                    ..default()
                },
                SmState::default(),
                PoseRuntime::default(),
                EntryBlend::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.75,
                },
                Transform::default(),
                HeadTrackState::default(),
            ));
            let target = app
                .world_mut()
                .spawn((
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
                    Transform::from_xyz(0.0, 0.0, distance),
                ))
                .id();
            {
                let mut lock = app.world_mut().resource_mut::<LockOnController>();
                lock.system.lock_target = Some(target.to_bits());
                lock.system.track_target = Some(target.to_bits());
                lock.lock_entity = Some(target);
                lock.track_entity = Some(target);
            }
            app.world_mut()
                .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw = -1.0;

            if occluded {
                let mut mesh = crate::plugins::stage_collider::StageMeshCollider::default();
                mesh.append_world_triangles(
                    &[
                        [-20.0, -1.0, 1.5],
                        [20.0, -1.0, 1.5],
                        [20.0, 20.0, 1.5],
                        [-20.0, 20.0, 1.5],
                    ],
                    &[0, 1, 2, 0, 2, 3],
                );
                app.insert_resource(StageCollider { mesh: Some(mesh) });
            } else if distance < 10.0 {
                app.world_mut().spawn((
                    crate::plugins::camera_plugin::GameplayCamera,
                    Transform::from_xyz(0.0, 3.0, 10.0).looking_to(Vec3::Z, Vec3::Y),
                    Projection::Perspective(PerspectiveProjection::default()),
                ));
            }
            if distance < 10.0 {
                for _ in 0..11 {
                    app.update();
                    assert_eq!(
                        app.world().resource::<LockOnController>().lock_entity,
                        Some(target)
                    );
                }
            }
            app.update();
            let lock = app.world().resource::<LockOnController>();
            assert_eq!(lock.system.lock_target, None);
            assert_eq!(lock.lock_entity, None);
            assert_eq!(
                app.world()
                    .resource::<crate::plugins::camera_plugin::CameraBasis>()
                    .yaw,
                0.75
            );
        }
    }

    #[test]
    fn production_player_death_clears_lock_track_and_restores_free_basis() {
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
        let mut app = App::new();
        app.insert_resource(time)
            .init_resource::<NetState>()
            .init_resource::<LockOnController>()
            .init_resource::<crate::plugins::camera_plugin::CameraBasis>()
            .add_systems(Update, (lock_on_tick, local_attack_lock_facing).chain());
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
            Health {
                current: 0.0,
                ..default()
            },
            SmState::default(),
            PoseRuntime::default(),
            EntryBlend::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.75,
            },
            Transform::default(),
            HeadTrackState::default(),
        ));
        let target = app
            .world_mut()
            .spawn((
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
                Transform::from_xyz(0.0, 0.0, 3.0),
            ))
            .id();
        {
            let mut lock = app.world_mut().resource_mut::<LockOnController>();
            lock.system.lock_target = Some(target.to_bits());
            lock.system.track_target = Some(target.to_bits());
            lock.lock_entity = Some(target);
            lock.track_entity = Some(target);
        }
        app.world_mut()
            .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
            .yaw = -1.0;

        app.update();

        let lock = app.world().resource::<LockOnController>();
        assert_eq!(lock.system.lock_target, None);
        assert_eq!(lock.system.track_target, None);
        assert_eq!(lock.lock_entity, None);
        assert_eq!(lock.track_entity, None);
        assert_eq!(
            app.world()
                .resource::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw,
            0.75
        );
    }

    #[test]
    fn ranged_weapon_equip_hook_clears_lock_but_preserves_track() {
        let mut app = App::new();
        app.init_resource::<LockOnController>()
            .init_resource::<crate::plugins::camera_plugin::CameraBasis>()
            .add_systems(Update, apply_weapon_strategy_equip_hooks);
        let tracked = app.world_mut().spawn_empty().id();
        let fighter = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 1,
                    weapon_slots: [1, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.75,
                },
            ))
            .id();
        {
            let mut lock = app.world_mut().resource_mut::<LockOnController>();
            lock.system.lock_target = Some(tracked.to_bits());
            lock.system.track_target = Some(tracked.to_bits());
            lock.lock_entity = Some(tracked);
            lock.track_entity = Some(tracked);
        }

        app.update();

        let lock = app.world().resource::<LockOnController>();
        assert_eq!(lock.system.lock_target, None);
        assert_eq!(lock.lock_entity, None);
        assert_eq!(lock.system.track_target, Some(tracked.to_bits()));
        assert_eq!(lock.track_entity, Some(tracked));
        assert_eq!(
            app.world()
                .get::<RangedFireGate>(fighter)
                .unwrap()
                .equipped_weapon,
            Some((1, 0, 0))
        );
        assert!(
            (app.world()
                .resource::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw
                - 0.75)
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn production_missing_player_drops_stale_entity_targets() {
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
        let mut app = App::new();
        app.insert_resource(time)
            .init_resource::<NetState>()
            .init_resource::<LockOnController>()
            .init_resource::<crate::plugins::camera_plugin::CameraBasis>()
            .add_systems(Update, (lock_on_tick, local_attack_lock_facing).chain());
        let stale = app.world_mut().spawn_empty().id();
        {
            let mut lock = app.world_mut().resource_mut::<LockOnController>();
            lock.system.lock_target = Some(stale.to_bits());
            lock.system.track_target = Some(stale.to_bits());
            lock.lock_entity = Some(stale);
            lock.track_entity = Some(stale);
        }

        app.update();

        let lock = app.world().resource::<LockOnController>();
        assert_eq!(lock.system.lock_target, None);
        assert_eq!(lock.system.track_target, None);
        assert_eq!(lock.lock_entity, None);
        assert_eq!(lock.track_entity, None);
    }

    #[test]
    fn production_lock_faces_only_during_attack_entry_blend() {
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
        let mut net = NetState {
            status: NetStatus::InRoom,
            player_id: "local".into(),
            ..default()
        };
        net.members = vec![
            meteor_network::protocol::RoomMemberWire {
                connected: true,
                player_id: "local".into(),
                name: "local".into(),
                character_id: 0,
                weapon: 6,
                team: Some(1),
                ready: true,
            },
            meteor_network::protocol::RoomMemberWire {
                connected: true,
                player_id: "peer".into(),
                name: "peer".into(),
                character_id: 1,
                weapon: 6,
                team: Some(2),
                ready: true,
            },
        ];
        let mut app = App::new();
        app.insert_resource(time)
            .insert_resource(net)
            .init_resource::<LockOnController>()
            .init_resource::<crate::plugins::camera_plugin::CameraBasis>()
            .add_systems(Update, (lock_on_tick, local_attack_lock_facing).chain());
        let local = app
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
                SmState {
                    current: FighterState::Attack1,
                    attack_stage: 1,
                    ..default()
                },
                PoseRuntime {
                    pose_id: 200,
                    frame: 10.0,
                    start: 0,
                    end: 20,
                    loop_start: 0,
                    loop_end: 20,
                    loops: false,
                    pose_instance_id: 1,
                },
                EntryBlend {
                    duration: 0.12,
                    pose_id: 200,
                    ..default()
                },
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                Transform::default(),
                HeadTrackState::default(),
            ))
            .id();
        let remote = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "peer".into(),
                },
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
                Transform::from_xyz(3.0, 0.0, 0.0),
            ))
            .id();
        {
            let mut lock = app.world_mut().resource_mut::<LockOnController>();
            lock.system.lock_target = Some(remote.to_bits());
            lock.lock_entity = Some(remote);
        }

        app.update();
        assert!(
            (app.world().get::<MotionState>(local).unwrap().facing - std::f32::consts::FRAC_PI_2)
                .abs()
                < 1e-5
        );

        app.world_mut()
            .get_mut::<Transform>(remote)
            .unwrap()
            .translation
            .y = meteor_combat::lock_on::LOCK_ATTACK_FACE_HEIGHT;
        app.world_mut()
            .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
            .yaw = -1.0;
        app.update();
        assert!(app
            .world()
            .resource::<LockOnController>()
            .lock_entity
            .is_none());
        assert!(
            (app.world()
                .resource::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw
                - std::f32::consts::FRAC_PI_2)
                .abs()
                < 1e-5
        );

        app.world_mut()
            .get_mut::<MotionState>(local)
            .unwrap()
            .facing = 0.0;
        app.world_mut()
            .get_mut::<Transform>(remote)
            .unwrap()
            .translation
            .y = 0.0;
        app.world_mut()
            .get_mut::<EntryBlend>(local)
            .unwrap()
            .duration = 0.0;
        {
            let mut lock = app.world_mut().resource_mut::<LockOnController>();
            lock.system.lock_target = Some(remote.to_bits());
            lock.lock_entity = Some(remote);
        }
        app.update();
        assert_eq!(app.world().get::<MotionState>(local).unwrap().facing, 0.0);
    }

    #[test]
    fn track_head_uses_web_motion_ownership_and_turn_limits() {
        let grounded = FighterAir::default();
        assert_eq!(
            head_track_turn_limit(FighterState::Name, Some(&grounded), false),
            Some(meteor_combat::lock_on::TRACK_TURN_LIMIT)
        );
        assert_eq!(
            head_track_turn_limit(FighterState::Attack1, Some(&grounded), false),
            None,
            "a grounded attack must preserve the sampled head override"
        );
        assert_eq!(
            head_track_turn_limit(FighterState::Name, Some(&grounded), true),
            None,
            "weapon-switch mode owns the head even while SM remains Name"
        );

        let mut ballistic = FighterAir::default();
        ballistic.air.airborne = true;
        ballistic.air.motion_mode = AirMotionMode::Ballistic;
        assert_eq!(
            head_track_turn_limit(FighterState::Attack2, Some(&ballistic), false),
            Some(meteor_combat::lock_on::TRACK_TURN_LIMIT),
            "free-air mode updates Track independently of the combat state"
        );

        ballistic.air.motion_mode = AirMotionMode::CurveExit;
        assert_eq!(
            head_track_turn_limit(FighterState::Jump, Some(&ballistic), false),
            Some(meteor_combat::lock_on::TRACK_CURVE_EXIT_TURN_LIMIT)
        );
        ballistic.air.motion_mode = AirMotionMode::Curve;
        assert_eq!(
            head_track_turn_limit(FighterState::Jump, Some(&ballistic), false),
            None
        );
        ballistic.air.motion_mode = AirMotionMode::Wall;
        assert_eq!(
            head_track_turn_limit(FighterState::Jump, Some(&ballistic), false),
            None
        );
    }

    #[test]
    fn ledge_fall_switches_run_pose_only_after_original_velocity_gate() {
        assert_eq!(airborne_locomotion_pose(144, false, true, 0.0, true), 144);
        assert_eq!(
            airborne_locomotion_pose(
                144,
                false,
                true,
                meteor_physics::jump::FALL_POSE_SPEED - 0.01,
                true,
            ),
            152
        );
        assert_eq!(
            airborne_locomotion_pose(
                180,
                true,
                true,
                meteor_physics::jump::FALL_POSE_SPEED - 0.01,
                true,
            ),
            180,
            "authored fall hold remains pinned"
        );
    }

    #[test]
    fn production_room_npc_has_attack_motion_state() {
        let mut app = App::new();
        app.insert_resource(crate::app_state::LaunchRequest {
            spawn_room_npcs: true,
            ..default()
        })
        .insert_resource(crate::app_state::RoomState::default())
        .add_systems(Update, spawn_training_npc);
        app.update();

        let mut query = app.world_mut().query_filtered::<(
            &NpcFighter,
            &FighterAttackMotion,
            &crate::components::fighter::VisualRootPin,
            &NpcCombatProfile,
        ), With<Fighter>>();
        assert_eq!(query.iter(app.world()).count(), 1);
        let (_, _, _, profile) = query.single(app.world()).unwrap();
        assert_eq!(
            (
                profile.attack1,
                profile.attack2,
                profile.attack3,
                profile.guard,
                profile.dodge,
                profile.burst,
            ),
            (40.0, 30.0, 20.0, 20.0, 15.0, 10.0)
        );
    }

    #[test]
    fn production_solo_mission_death_respects_rule_and_keeps_mission_npc_dead() {
        fn spawn_dead_fighter(world: &mut World, slot: u8) -> Entity {
            let sm = SmState {
                current: FighterState::Dead,
                locked: true,
                ..default()
            };
            world
                .spawn((
                    (
                        Fighter {
                            character_id: 0,
                            player_slot: slot,
                            weapon: 5,
                            weapon_slots: [5, 0],
                            weapon_slot: 0,
                            weapon_slot_variants: [0, 0],
                            weapon_variant: 0,
                        },
                        DeathRespawn {
                            active: true,
                            timer: 0.0,
                            spawn_pos: Vec3::new(slot as f32, 0.0, 0.0),
                        },
                        Health {
                            current: 0.0,
                            max: 100.0,
                        },
                        Qi::default(),
                        CombatState {
                            state: FighterState::Dead,
                        },
                        PoseRuntime::idle(),
                        sm,
                        Transform::default(),
                    ),
                    (
                        crate::components::fighter::TargetHitClock::default(),
                        EntryBlend::default(),
                        MotionState {
                            movement_mode: meteor_core::MovementMode::Idle,
                            velocity: Vec3::ZERO,
                            facing: 0.0,
                        },
                        FighterAir::default(),
                        FighterDodge::default(),
                        FighterAttackMotion::default(),
                        FighterHitReaction::default(),
                        GunAimState::default(),
                    ),
                ))
                .id()
        }

        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(crate::app_state::LaunchRequest::default())
            .init_resource::<NetState>()
            .insert_resource(MatchEnd::default())
            .insert_resource(LockOnController::default())
            .insert_resource(crate::plugins::camera_plugin::CameraBasis::default())
            .add_systems(Update, death_respawn);

        let player = spawn_dead_fighter(app.world_mut(), 0);
        app.world_mut().entity_mut(player).insert(LocalFighter);
        let npc = spawn_dead_fighter(app.world_mut(), 1);
        app.world_mut().entity_mut(npc).insert((
            NpcFighter,
            MissionNpc {
                id: 1,
                name: "mission-dead".into(),
                team: 2,
                in_scene_roster: true,
                view: 0.0,
                think: 0.0,
                forced_behavior: None,
                is_vip: false,
                attack_target_slot: None,
                behavior_args: Vec::new(),
                attack_target_remaining: 0,
            },
        ));

        app.update();

        let player_death = app.world().get::<DeathRespawn>(player).unwrap();
        let player_health = app.world().get::<Health>(player).unwrap();
        assert!(player_death.active, "single defaults to Rule 5: no respawn");
        assert_eq!(player_health.current, 0.0);
        assert_eq!(
            app.world().get::<SmState>(player).unwrap().current,
            FighterState::Dead
        );

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let sn15 =
            crate::text_codec::decode_text(&std::fs::read(root.join("Level/sn15_.pst")).unwrap());
        for source in [sn15.as_str(), "int Rule = 5;", "int Rule = 6;", ""] {
            let definition = meteor_scene::mission_runtime::parse_mission_definition(source);
            let script =
                meteor_scene::mission_runtime::MissionScriptRuntime::new(source, definition.values);
            app.insert_resource(
                crate::plugins::stage_plugin::StageMissionRuntime::from_script(script),
            );

            for _ in 0..4 {
                app.update();
                assert!(app.world().get::<DeathRespawn>(player).unwrap().active);
                assert_eq!(app.world().get::<Health>(player).unwrap().current, 0.0);
                assert_eq!(
                    app.world().get::<SmState>(player).unwrap().current,
                    FighterState::Dead
                );
            }
        }

        let source = "int Rule = 2;";
        let definition = meteor_scene::mission_runtime::parse_mission_definition(source);
        app.insert_resource(
            crate::plugins::stage_plugin::StageMissionRuntime::from_script(
                meteor_scene::mission_runtime::MissionScriptRuntime::new(source, definition.values),
            ),
        );
        app.update();
        let player_death = app.world().get::<DeathRespawn>(player).unwrap();
        let player_health = app.world().get::<Health>(player).unwrap();
        assert!(!player_death.active);
        assert_eq!(player_health.current, player_health.max);

        let npc_death = app.world().get::<DeathRespawn>(npc).unwrap();
        let npc_health = app.world().get::<Health>(npc).unwrap();
        assert!(npc_death.active, "任务 NPC 必须保持死亡，等待任务脚本处理");
        assert_eq!(npc_health.current, 0.0);
        assert_eq!(
            app.world().get::<SmState>(npc).unwrap().current,
            FighterState::Dead
        );
    }

    #[test]
    fn attack_commit_banks_composed_velocity_and_ninja_scale() {
        let mut normal = FighterAttackMotion::default();
        bank_attack_commit_motion(&mut normal, Vec3::new(30.0, 4.0, -12.0), true, 6);
        assert!((normal.carry_x - 30.0).abs() < 1e-6);
        assert!((normal.carry_z + 12.0).abs() < 1e-6);
        assert!(normal.carry_banked);

        let mut ninja = FighterAttackMotion::default();
        bank_attack_commit_motion(
            &mut ninja,
            Vec3::new(30.0, 4.0, -12.0),
            true,
            NINJA_BLADE_WEAPON_ID,
        );
        assert!((ninja.carry_x - 20.1).abs() < 1e-5);
        assert!((ninja.carry_z + 8.04).abs() < 1e-5);

        bank_attack_commit_motion(&mut ninja, Vec3::new(30.0, 0.0, 10.0), false, 6);
        assert_eq!((ninja.carry_x, ninja.carry_z), (0.0, 0.0));
    }

    #[test]
    fn staff_weapon_form_persists_until_authored_transition_or_weapon_change() {
        let mut app = App::new();
        app.add_systems(Update, sync_weapon_form_state);
        let mut pose = PoseRuntime::idle();
        pose.pose_id = 430;
        let entity = app
            .world_mut()
            .spawn((
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 10,
                    weapon_slots: [10, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [3, 0],
                    weapon_variant: 3,
                },
                pose,
                WeaponForm::default(),
            ))
            .id();
        app.update();
        assert_eq!(app.world().get::<WeaponForm>(entity).unwrap().current, 1);

        app.world_mut()
            .get_mut::<PoseRuntime>(entity)
            .unwrap()
            .pose_id = 432;
        app.update();
        assert_eq!(app.world().get::<WeaponForm>(entity).unwrap().current, 1);

        app.world_mut().get_mut::<Fighter>(entity).unwrap().weapon = 6;
        app.update();
        assert_eq!(app.world().get::<WeaponForm>(entity).unwrap().current, 0);
    }

    #[test]
    fn entry_blend_active_when_duration_positive_and_pose_matches() {
        assert!(entry_blend_is_active(0.12, 5, 5));
    }

    #[test]
    fn entry_blend_inactive_when_duration_zero() {
        assert!(!entry_blend_is_active(0.0, 5, 5));
    }

    #[test]
    fn entry_blend_inactive_when_pose_id_mismatches() {
        assert!(!entry_blend_is_active(0.12, 5, 6));
    }

    #[test]
    fn jump_transition_matrix_keeps_web_hard_cuts() {
        for (from, to) in [
            (0, 151),
            (151, 152),
            (152, 180),
            (153, 154),
            (155, 156),
            (157, 158),
            (0, 161),
            (161, 159),
            (180, 0),
        ] {
            assert!(jump_transition_is_hard_cut(from, to), "{from}->{to}");
        }
        assert!(!jump_transition_is_hard_cut(0, 140));
        assert!(!jump_transition_is_hard_cut(140, 10));
    }

    #[test]
    fn ground_entry_blend_is_consumed_once_after_landing() {
        let mut air = crate::plugins::movement_plugin::FighterAir {
            ground_entry_blend_pending: true,
            ..default()
        };
        assert!(!take_ground_entry_blend(
            Some(&mut air),
            meteor_core::MovementMode::Airborne
        ));
        assert!(air.ground_entry_blend_pending);
        assert!(take_ground_entry_blend(
            Some(&mut air),
            meteor_core::MovementMode::Idle
        ));
        assert!(!air.ground_entry_blend_pending);
        assert!(!take_ground_entry_blend(
            Some(&mut air),
            meteor_core::MovementMode::Walk
        ));
    }

    #[test]
    fn engine_round_result_takes_priority_over_game_over() {
        assert_eq!(
            solo_round_decision(meteor_network::modes::ROUND_RESULT_TIMEOUT, Some(-1)),
            meteor_network::modes::ROUND_RESULT_TIMEOUT
        );
        assert_eq!(
            solo_round_decision(meteor_network::modes::ROUND_RESULT_TEAM2_WIPED, Some(-1)),
            meteor_network::modes::ROUND_RESULT_TEAM2_WIPED
        );
    }

    #[test]
    fn game_over_negative_is_defeat_positive_is_victory() {
        assert_eq!(
            solo_round_decision(0, Some(-1)),
            meteor_network::modes::ROUND_RESULT_TEAM1_WIPED
        );
        assert_eq!(
            solo_round_decision(0, Some(0)),
            meteor_network::modes::ROUND_RESULT_TEAM2_WIPED
        );
        assert_eq!(
            solo_round_decision(0, Some(1)),
            meteor_network::modes::ROUND_RESULT_TEAM2_WIPED
        );

        assert_eq!(solo_round_decision(0, None), 0);
    }

    #[test]
    fn scene_script_end_maps_winner_team_to_local_result() {
        assert_eq!(
            scene_game_end_result(1),
            meteor_network::modes::ROUND_RESULT_TEAM2_WIPED
        );
        assert_eq!(
            scene_game_end_result(2),
            meteor_network::modes::ROUND_RESULT_TEAM1_WIPED
        );
    }

    #[test]
    fn scene_on_attack_keeps_character_and_raw_damage_arguments() {
        assert_eq!(
            scene_on_attack_args(8, 126),
            [
                meteor_scene::scene_script_runtime::PstValue::Num(8.0),
                meteor_scene::scene_script_runtime::PstValue::Num(126.0),
            ]
        );
        assert_eq!(
            scene_on_attack_args(-1, -4),
            [
                meteor_scene::scene_script_runtime::PstValue::Num(-1.0),
                meteor_scene::scene_script_runtime::PstValue::Num(0.0),
            ]
        );
    }

    #[test]
    fn network_room_waits_for_authoritative_match_lifecycle() {
        use crate::plugins::network_plugin::NetStatus;

        assert!(local_match_end_enabled(NetStatus::Disconnected));
        assert!(local_match_end_enabled(NetStatus::Connecting));
        assert!(!local_match_end_enabled(NetStatus::InRoom));
    }

    #[test]
    fn network_room_does_not_replay_remote_attack_windows_locally() {
        assert!(local_attack_window_authority(
            NetStatus::Disconnected,
            false
        ));
        assert!(local_attack_window_authority(NetStatus::InRoom, true));
        assert!(!local_attack_window_authority(NetStatus::InRoom, false));

        let mut net = NetState {
            status: NetStatus::InRoom,
            address: "udp-host://0.0.0.0:37105".into(),
            mode: "siege".into(),
            round_active: true,
            ..Default::default()
        };
        assert!(remote_prop_authority(&net, true));
        assert!(!remote_prop_authority(&net, false));
        net.address = "udp://127.0.0.1:37105".into();
        assert!(!remote_prop_authority(&net, true));
        net.address = "udp-host://0.0.0.0:37105".into();
        net.mode = "deathmatch".into();
        assert!(!remote_prop_authority(&net, true));
    }

    fn core(v: bevy::prelude::Vec3) -> meteor_core::Vec3 {
        meteor_core::Vec3::new(v.x, v.y, v.z)
    }

    #[test]
    fn attack_box_sat_hits_prop_in_front_only() {
        let forward = Vec3::Z;
        let attack_box = Obb {
            center: core(Vec3::new(0.0, 0.0, ATTACK_REACH / 2.0)),
            axes: [core(Vec3::new(1.0, 0.0, 0.0)), core(Vec3::Y), core(forward)],
            half_size: core(Vec3::new(0.6, 0.9, ATTACK_REACH / 2.0)),
        };
        let prop_box = |pos: [f32; 3]| Obb {
            center: core(Vec3::from_array(pos)),
            axes: [core(Vec3::X), core(Vec3::Y), core(Vec3::Z)],
            half_size: core(Vec3::splat(0.4)),
        };

        assert!(obbs_intersect(&attack_box, &prop_box([0.0, 0.0, 1.0])));

        assert!(!obbs_intersect(&attack_box, &prop_box([2.5, 0.0, 0.0])));

        assert!(!obbs_intersect(&attack_box, &prop_box([0.0, 0.0, -2.0])));

        assert!(!obbs_intersect(&attack_box, &prop_box([0.0, 2.0, 0.0])));
    }

    #[test]
    fn alternate_slot_uses_explicit_active_index_and_allows_same_class_variants() {
        assert_eq!(alternate_weapon_slot(0, [6, 5]), Some(1));
        assert_eq!(alternate_weapon_slot(1, [6, 5]), Some(0));

        assert_eq!(alternate_weapon_slot(0, [6, 6]), Some(1));
        assert_eq!(alternate_weapon_slot(0, [6, 0]), None);
        assert_eq!(other_weapon_slot(0), 1);
        assert_eq!(other_weapon_slot(1), 0);
    }

    #[test]
    fn switch_commit_preserves_target_slot_variant_even_for_same_weapon_class() {
        let mut fighter = Fighter {
            character_id: 0,
            player_slot: 0,
            weapon: 6,
            weapon_slots: [6, 6],
            weapon_slot: 0,
            weapon_slot_variants: [0, 2],
            weapon_variant: 0,
        };
        let switch = WeaponSwitchActive {
            to_weapon: 6,
            to_slot: 1,
            to_variant: 2,
            pose_id: 24,
            pose_instance_id: 1,
            marker_frame: 0,
            end_frame: 1,
            committed: true,
            end_sampled: false,
            ..default()
        };
        assert!(commit_carried_weapon(&mut fighter, &switch));
        assert_eq!(fighter.weapon_slot, 1);
        assert_eq!(fighter.weapon, 6);
        assert_eq!(fighter.weapon_variant, 2);
        assert_eq!(fighter.weapon_slots, [6, 6]);
        assert_eq!(fighter.weapon_slot_variants, [0, 2]);
    }

    #[test]
    fn empty_weapon_switch_target_keeps_the_current_loadout_at_marker() {
        let mut fighter = Fighter {
            character_id: 0,
            player_slot: 0,
            weapon: 6,
            weapon_slots: [6, 0],
            weapon_slot: 0,
            weapon_slot_variants: [2, 0],
            weapon_variant: 2,
        };
        let switch = WeaponSwitchActive {
            to_weapon: 0,
            to_slot: 1,
            empty_target: true,
            committed: true,
            ..default()
        };

        assert!(!commit_carried_weapon(&mut fighter, &switch));

        assert_eq!(fighter.weapon, 6);
        assert_eq!(fighter.weapon_slot, 0);
        assert_eq!(fighter.weapon_variant, 2);
        assert_eq!(fighter.weapon_slots, [6, 0]);
    }

    #[test]
    fn g_during_e_does_not_reequip_the_removed_spare_at_marker() {
        let mut fighter = Fighter {
            character_id: 0,
            player_slot: 0,
            weapon: 6,
            weapon_slots: [6, 5],
            weapon_slot: 0,
            weapon_slot_variants: [2, 3],
            weapon_variant: 2,
        };
        let switch = WeaponSwitchActive {
            to_weapon: 5,
            to_slot: 1,
            to_variant: 3,
            ..default()
        };
        assert_eq!(
            crate::plugins::item_plugin::drop_spare_weapon(&mut fighter),
            Some((5, 3))
        );
        assert!(!commit_carried_weapon(&mut fighter, &switch));
        assert_eq!(
            (fighter.weapon, fighter.weapon_variant, fighter.weapon_slot),
            (6, 2, 0)
        );
        assert_eq!(fighter.weapon_slots, [6, 0]);
    }

    #[test]
    fn fist_swap_toggles_local_inventory_but_never_replays_for_remote_snapshot() {
        let mut app = App::new();
        app.init_resource::<NetState>();
        app.init_resource::<PendingNetActions>();
        app.add_systems(Update, fist_weapon_swap);
        let fighter = || Fighter {
            character_id: 0,
            player_slot: 0,
            weapon: 11,
            weapon_slots: [11, 6],
            weapon_slot: 0,
            weapon_slot_variants: [1, 3],
            weapon_variant: 1,
        };
        let pose = PoseRuntime {
            pose_id: 412,
            frame: 1.0,
            start: 0,
            end: 1,
            loop_start: 0,
            loop_end: 1,
            loops: false,
            pose_instance_id: 1,
        };
        let local = app.world_mut().spawn((fighter(), pose.clone())).id();
        let remote = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "remote".into(),
                },
                fighter(),
                pose,
            ))
            .id();

        app.update();

        let local = app.world().get::<Fighter>(local).unwrap();
        assert_eq!(local.weapon_slot, 1);
        assert_eq!(local.weapon, 6);
        assert_eq!(local.weapon_variant, 3);
        assert_eq!(local.weapon_slots, [11, 6]);
        let remote = app.world().get::<Fighter>(remote).unwrap();
        assert_eq!(remote.weapon_slot, 0);
        assert_eq!(remote.weapon, 11);
        assert_eq!(remote.weapon_variant, 1);
    }

    #[test]
    fn fist_swap_commits_once_per_pose_instance_with_two_fists() {
        let mut app = App::new();
        app.init_resource::<NetState>();
        app.init_resource::<PendingNetActions>();
        app.add_systems(Update, fist_weapon_swap);
        let entity = app
            .world_mut()
            .spawn((
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 11,
                    weapon_slots: [11, 11],
                    weapon_slot: 0,
                    weapon_slot_variants: [1, 3],
                    weapon_variant: 1,
                },
                PoseRuntime {
                    pose_id: 412,
                    pose_instance_id: 1,
                    ..PoseRuntime::idle()
                },
            ))
            .id();
        app.update();
        app.update();
        assert_eq!(app.world().get::<Fighter>(entity).unwrap().weapon_slot, 1);
        app.world_mut()
            .get_mut::<PoseRuntime>(entity)
            .unwrap()
            .pose_instance_id += 1;
        app.update();
        assert_eq!(app.world().get::<Fighter>(entity).unwrap().weapon_slot, 0);
    }

    #[test]
    fn gun_aim_intercepts_only_grounded_non_exempt_first_attack() {
        let firearm = meteor_physics::locomotion::FIREARM_STRATEGY;
        assert!(gun_aim_intercepts(firearm, false, false, 206));
        assert!(!gun_aim_intercepts(firearm, true, false, 206));
        assert!(!gun_aim_intercepts(firearm, false, true, 206));
        assert!(!gun_aim_intercepts(firearm, false, false, 338));
        assert!(!gun_aim_intercepts(firearm, false, false, 215));
        assert!(!gun_aim_intercepts(0, false, false, 206));
    }

    fn switch_runtime(frame: f32, pose_id: u16, instance: u32) -> PoseRuntime {
        PoseRuntime {
            pose_id,
            frame,
            start: 10,
            end: 30,
            loop_start: 20,
            loop_end: 20,
            loops: false,
            pose_instance_id: instance,
        }
    }

    fn active_switch() -> WeaponSwitchActive {
        WeaponSwitchActive {
            to_weapon: 5,
            to_slot: 1,
            to_variant: 2,
            pose_id: WEAPON_SWITCH_POSE_GROUND,
            pose_instance_id: 7,
            marker_frame: 20,
            end_frame: 30,
            committed: false,
            end_sampled: false,
            ..default()
        }
    }

    #[test]
    fn weapon_switch_commits_once_at_stored_marker() {
        let mut switch = active_switch();
        let before = switch_runtime(19.9, WEAPON_SWITCH_POSE_GROUND, 7);
        assert_eq!(
            weapon_switch_step(&mut switch, &before),
            WeaponSwitchStep {
                commit: false,
                remove: false
            }
        );
        let marker = switch_runtime(20.0, WEAPON_SWITCH_POSE_GROUND, 7);
        assert!(weapon_switch_step(&mut switch, &marker).commit);
        assert!(!weapon_switch_step(&mut switch, &marker).commit);
    }

    #[test]
    fn weapon_switch_interruption_cancels_before_marker() {
        let mut switch = active_switch();
        let interrupted = switch_runtime(12.0, 95, 8);
        assert_eq!(
            weapon_switch_step(&mut switch, &interrupted),
            WeaponSwitchStep {
                commit: false,
                remove: true
            }
        );
        assert!(!switch.committed);
    }

    #[test]
    fn weapon_switch_keeps_first_terminal_sample_then_releases() {
        let mut switch = active_switch();
        let end = switch_runtime(30.0, WEAPON_SWITCH_POSE_GROUND, 7);
        assert_eq!(
            weapon_switch_step(&mut switch, &end),
            WeaponSwitchStep {
                commit: true,
                remove: false
            }
        );
        assert!(switch.end_sampled);
        assert_eq!(
            weapon_switch_step(&mut switch, &end),
            WeaponSwitchStep {
                commit: false,
                remove: true
            }
        );
    }

    #[test]
    fn combat_weapon_stats_tracks_original_refined_variant() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Meteor.res");
        if !path.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let table = parse_meteor_res(&std::fs::read(path).unwrap())
            .unwrap()
            .combat_table();
        let fighter = Fighter {
            character_id: 0,
            player_slot: 0,
            weapon: 6,
            weapon_slots: [6, 5],
            weapon_slot: 0,
            weapon_slot_variants: [3, 0],
            weapon_variant: 3,
        };

        assert_eq!(
            combat_weapon_stats(&table, &fighter),
            Some(WeaponCombatStats {
                attack: 95,
                defense: 7,
            })
        );
    }

    #[test]
    fn stats_view_take_damage_clamps_hp() {
        let mut hp = 100.0f32;
        let mut qi = 0.0f32;
        {
            let mut stats = StatsView {
                hp: &mut hp,
                qi: &mut qi,
                invincible_flag: false,
            };
            assert_eq!(stats.take_damage(30), 30);
            assert_eq!(stats.take_damage(999), 70);
        }
        assert!(hp <= 0.0);
    }

    #[test]
    fn stats_view_gain_anger_from_damage() {
        let mut hp = 100.0f32;
        let mut qi = 0.0f32;
        {
            let mut stats = StatsView {
                hp: &mut hp,
                qi: &mut qi,
                invincible_flag: false,
            };
            assert_eq!(stats.gain_anger_from_damage(100), 14);
        }
        assert!((qi - 14.0).abs() < 1e-5);
    }

    #[test]
    fn stats_view_guard_anger_uses_web_divisor_and_actual_capped_gain() {
        let mut hp = 100.0f32;
        let mut qi = 0.0f32;
        {
            let mut stats = StatsView {
                hp: &mut hp,
                qi: &mut qi,
                invincible_flag: false,
            };
            assert_eq!(stats.gain_anger_on_guard(35), 0);
            assert_eq!(stats.gain_anger_on_guard(36), 1);
        }
        assert_eq!(qi, 1.0);

        qi = meteor_core::DEFAULT_QI_MAX - 1.0;
        let actual_gain;
        {
            let mut stats = StatsView {
                hp: &mut hp,
                qi: &mut qi,
                invincible_flag: false,
            };
            actual_gain = stats.gain_anger_on_guard(1_000);
        }
        assert_eq!(actual_gain, 1);
        assert_eq!(qi, meteor_core::DEFAULT_QI_MAX);
    }

    #[test]
    fn stats_view_anger_capped_at_max() {
        let mut hp = 100.0f32;
        let mut qi = 99.0f32;
        let gain;
        {
            let mut stats = StatsView {
                hp: &mut hp,
                qi: &mut qi,
                invincible_flag: false,
            };
            gain = stats.gain_anger_from_damage(1000);
        }
        assert!(gain <= 1);
        assert!((qi - meteor_core::DEFAULT_QI_MAX).abs() < 1e-5);
    }

    #[test]
    fn attacking_state_helper() {
        assert!(is_attacking_state(FighterState::Attack1));
        assert!(is_attacking_state(FighterState::Attack2));
        assert!(!is_attacking_state(FighterState::Name));
        assert!(!is_attacking_state(FighterState::BeHit));
    }

    #[test]
    fn cure_links_are_forwarded_to_the_state_machine() {
        assert!(state_machine_tracks_pose_link(FighterState::Cure));
        assert!(state_machine_tracks_pose_link(FighterState::Attack1));
        assert!(!state_machine_tracks_pose_link(FighterState::Name));
        assert!(!state_machine_tracks_pose_link(FighterState::BeHit));
    }

    #[test]
    fn accepted_down_jump_releases_old_reaction_ownership() {
        let mut reaction = FighterHitReaction {
            root_started: true,
            airborne: true,
            authored_minimum: 0.8,
            ..default()
        };
        assert!(release_reaction_after_accepted_jump(
            &mut reaction,
            true,
            FighterState::Jump
        ));
        assert!(!reaction.root_started);
        assert!(!reaction.airborne);
        assert_eq!(reaction.authored_minimum, 0.0);

        let mut rejected = FighterHitReaction {
            root_started: true,
            ..default()
        };
        assert!(!release_reaction_after_accepted_jump(
            &mut rejected,
            false,
            FighterState::Down
        ));
        assert!(rejected.root_started);
    }

    #[test]
    fn requested_getup_installs_reaction_ownership_until_pose_end() {
        let mut reaction = FighterHitReaction::default();
        let mut state = SmState {
            current: FighterState::BeHit,
            linked_recovery_pose_id: Some(120),
            ..default()
        };
        let pose = PoseRuntime {
            pose_id: 120,
            start: 100,
            end: 107,
            ..default()
        };

        assert!(install_requested_getup_reaction(
            &mut reaction,
            &mut state,
            &pose,
            false,
        ));
        let getup = reaction.current.as_ref().unwrap();
        assert_eq!(getup.kind, HitReactKind::GetUp);
        assert_eq!(getup.pose_id, Some(120));
        assert!(getup.lock_remaining >= 0.35);
        assert_eq!(state.linked_recovery_pose_id, None);

        assert!(!install_requested_getup_reaction(
            &mut reaction,
            &mut state,
            &pose,
            false,
        ));
    }

    #[test]
    fn attack_command_is_consumed_only_when_parser_is_reached() {
        for state in [
            FighterState::Name,
            FighterState::Jump,
            FighterState::Dodge,
            FighterState::Burst,
            FighterState::Attack1,
            FighterState::Attack2,
            FighterState::Attack3,
        ] {
            assert!(should_consume_attack_command(
                true, false, false, false, state
            ));
        }
        for state in [
            FighterState::Guard,
            FighterState::Fake,
            FighterState::Cure,
            FighterState::BeHit,
            FighterState::Down,
            FighterState::Dead,
        ] {
            assert!(!should_consume_attack_command(
                true, false, false, false, state
            ));
        }
        assert!(!should_consume_attack_command(
            true,
            true,
            false,
            false,
            FighterState::Name
        ));
        assert!(!should_consume_attack_command(
            true,
            false,
            true,
            false,
            FighterState::Name
        ));
        assert!(!should_consume_attack_command(
            true,
            false,
            false,
            true,
            FighterState::Name
        ));
    }

    #[test]
    fn production_dispatch_observes_guard_attack_event_order() {
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

        for guard_first in [true, false] {
            let mut pose = PoseRuntime::idle();
            character.apply_pose(&mut pose, 0, true);
            let mut blend = EntryBlend::default();
            let mut combat = CombatState::default();
            let mut health = Health::default();
            let mut qi = Qi::default();
            let air = FighterAir::default();
            let mut anger_spend = None;
            let view = CombatView {
                pose: &mut pose,
                blend: &mut blend,
                combat: &mut combat,
                health: &mut health,
                qi: &mut qi,
                air: &air,
                assets: &character,
                weapon: 6,
                guard_held_input: true,
                in_guard_hit_reaction: false,
                attack_directions: Vec::new(),
                weapon_form: 0,
                anger_spend: &mut anger_spend,
                strategy_id: weapon_to_strategy(6),
                weapon_action_vetoed: false,
            };
            let mut sm = StateMachine::new(view);
            let mut command_input = crate::plugins::input_plugin::LocalCommandInput::default();
            let mut gun_aim = GunAimState::default();

            let attack_accepted = if guard_first {
                assert!(sm.handle_input(Action::Guard));
                dispatch_local_attack(&mut sm, &mut command_input, &mut gun_aim).0
            } else {
                let accepted = dispatch_local_attack(&mut sm, &mut command_input, &mut gun_aim).0;
                assert!(sm.handle_input(Action::Guard));
                accepted
            };

            if guard_first {
                assert!(!attack_accepted);
                assert_eq!(sm.current, FighterState::Guard);
            } else {
                assert!(attack_accepted);
                assert_eq!(sm.current, FighterState::Attack1);
            }
        }
    }

    #[test]
    fn production_attack_consumes_only_directions_that_precede_its_event() {
        use meteor_input::command_parser::Dir;

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

        for direction_first in [true, false] {
            let mut pose = PoseRuntime::idle();
            character.apply_pose(&mut pose, 0, true);
            let mut blend = EntryBlend::default();
            let mut combat = CombatState::default();
            let mut health = Health::default();
            let mut qi = Qi::default();
            let air = FighterAir::default();
            let mut anger_spend = None;
            let view = CombatView {
                pose: &mut pose,
                blend: &mut blend,
                combat: &mut combat,
                health: &mut health,
                qi: &mut qi,
                air: &air,
                assets: &character,
                weapon: 6,
                guard_held_input: false,
                in_guard_hit_reaction: false,
                attack_directions: Vec::new(),
                weapon_form: 0,
                anger_spend: &mut anger_spend,
                strategy_id: weapon_to_strategy(6),
                weapon_action_vetoed: false,
            };
            let mut sm = StateMachine::new(view);
            let mut command_input = crate::plugins::input_plugin::LocalCommandInput::default();
            let mut gun_aim = GunAimState::default();

            if direction_first {
                command_input.0.push_direction(Dir::Up, 1_000.0);
            }
            assert!(dispatch_local_attack(&mut sm, &mut command_input, &mut gun_aim).0);
            if !direction_first {
                command_input.0.push_direction(Dir::Up, 1_000.0);
            }

            assert_eq!(
                command_input.0.peek(),
                if direction_first {
                    Vec::new()
                } else {
                    vec![Dir::Up]
                }
            );
        }
    }

    #[test]
    fn production_attack_missing_command_pose_falls_back_to_plain_web_entry() {
        use meteor_input::command_parser::Dir;

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let mut character = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root),
            "p0",
        )
        .expect("p0 assets");
        assert!(
            character.poses.remove(&266).is_some(),
            "remove sword Up+A entry"
        );

        let mut pose = PoseRuntime::idle();
        character.apply_pose(&mut pose, 0, true);
        let mut blend = EntryBlend::default();
        let mut combat = CombatState::default();
        let mut health = Health::default();
        let mut qi = Qi {
            current: 100.0,
            ..Default::default()
        };
        let air = FighterAir::default();
        let mut anger_spend = None;
        let view = CombatView {
            pose: &mut pose,
            blend: &mut blend,
            combat: &mut combat,
            health: &mut health,
            qi: &mut qi,
            air: &air,
            assets: &character,
            weapon: 6,
            guard_held_input: false,
            in_guard_hit_reaction: false,
            attack_directions: Vec::new(),
            weapon_form: 0,
            anger_spend: &mut anger_spend,
            strategy_id: weapon_to_strategy(6),
            weapon_action_vetoed: false,
        };
        let mut sm = StateMachine::new(view);
        let mut command_input = crate::plugins::input_plugin::LocalCommandInput::default();
        command_input.0.push_direction(Dir::Up, 1_000.0);
        let mut gun_aim = GunAimState::default();

        assert!(dispatch_local_attack(&mut sm, &mut command_input, &mut gun_aim).0);
        assert_eq!(sm.fighter_mut().pose.pose_id, 262);
        assert_eq!(sm.fighter_mut().qi.current, 100.0);
    }

    #[test]
    fn dodge_roll_branch_uses_the_event_time_hold_and_aim_state() {
        assert!(!dodge_roll_at_event(
            false,
            false,
            false,
            FighterState::Name
        ));
        assert!(dodge_roll_at_event(false, true, false, FighterState::Name));
        assert!(dodge_roll_at_event(false, false, true, FighterState::Name));
        assert!(dodge_roll_at_event(true, false, false, FighterState::Name));
        assert!(dodge_roll_at_event(false, false, false, FighterState::Down));
    }

    #[test]
    fn production_dispatch_observes_attack_weapon_switch_event_order() {
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
        let fighter = Fighter {
            character_id: 0,
            player_slot: 0,
            weapon: 6,
            weapon_slots: [6, 5],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        };
        let motion = MotionState {
            movement_mode: meteor_core::MovementMode::Idle,
            velocity: Vec3::ZERO,
            facing: 0.0,
        };
        let attack_motion = FighterAttackMotion::default();

        for attack_first in [true, false] {
            let mut pose = PoseRuntime::idle();
            character.apply_pose(&mut pose, 0, true);
            let mut blend = EntryBlend::default();
            let mut combat = CombatState::default();
            let mut health = Health::default();
            let mut qi = Qi::default();
            let air = FighterAir::default();
            let mut anger_spend = None;
            let view = CombatView {
                pose: &mut pose,
                blend: &mut blend,
                combat: &mut combat,
                health: &mut health,
                qi: &mut qi,
                air: &air,
                assets: &character,
                weapon: fighter.weapon,
                guard_held_input: false,
                in_guard_hit_reaction: false,
                attack_directions: Vec::new(),
                weapon_form: 0,
                anger_spend: &mut anger_spend,
                strategy_id: weapon_to_strategy(fighter.weapon),
                weapon_action_vetoed: false,
            };
            let mut sm = StateMachine::new(view);
            let mut command_input = crate::plugins::input_plugin::LocalCommandInput::default();
            let mut gun_aim = GunAimState::default();

            if attack_first {
                assert!(dispatch_local_attack(&mut sm, &mut command_input, &mut gun_aim).0);
                assert_eq!(sm.current, FighterState::Attack1);
                assert!(begin_local_weapon_switch(
                    &mut sm,
                    false,
                    gun_aim.aiming,
                    &fighter,
                    &motion,
                    &attack_motion,
                    None,
                )
                .is_none());
            } else {
                let switch = begin_local_weapon_switch(
                    &mut sm,
                    false,
                    gun_aim.aiming,
                    &fighter,
                    &motion,
                    &attack_motion,
                    None,
                );
                assert!(switch.is_some());
                assert_eq!(sm.fighter_mut().pose.pose_id, WEAPON_SWITCH_POSE_GROUND);

                assert_eq!(sm.current, FighterState::Name);
            }
        }
    }

    #[test]
    fn attack_lock_toggle_order_changes_the_final_lock_slot() {
        let candidate = LockCandidate {
            id: 7,
            x: 0.0,
            y: 0.0,
            z: 3.0,
            team: 1,
        };
        let player = Vec3::ZERO;

        let mut attack_first = LockOnController::default();
        acquire_local_attack_lock(&mut attack_first, true, 0, player, 0.0, &[candidate]);
        assert_eq!(attack_first.system.lock_target, Some(7));
        dispatch_local_lock_toggle(
            &mut attack_first,
            &mut crate::plugins::camera_plugin::CameraBasis::default(),
            &mut GunAimState::default(),
            0,
            player,
            0.0,
            &[candidate],
        );
        assert_eq!(attack_first.system.lock_target, None);

        let mut lock_first = LockOnController::default();
        dispatch_local_lock_toggle(
            &mut lock_first,
            &mut crate::plugins::camera_plugin::CameraBasis::default(),
            &mut GunAimState::default(),
            0,
            player,
            0.0,
            &[candidate],
        );
        acquire_local_attack_lock(&mut lock_first, true, 0, player, 0.0, &[candidate]);
        assert_eq!(lock_first.system.lock_target, Some(7));
    }

    #[test]
    fn rejected_attack_does_not_reach_auto_lock_before_input_gates() {
        assert!(attack_input_reaches_auto_lock(
            true,
            false,
            false,
            FighterState::Name
        ));

        assert!(attack_input_reaches_auto_lock(
            true,
            false,
            false,
            FighterState::Guard
        ));
        assert!(!attack_input_reaches_auto_lock(
            true,
            true,
            false,
            FighterState::Name
        ));
        assert!(!attack_input_reaches_auto_lock(
            true,
            false,
            true,
            FighterState::Guard
        ));
        for state in [FighterState::BeHit, FighterState::Down, FighterState::Dead] {
            assert!(!attack_input_reaches_auto_lock(true, false, false, state));
        }
        assert!(!attack_input_reaches_auto_lock(
            false,
            false,
            false,
            FighterState::Name
        ));

        let ground_context = weapon_switch_context_pose(
            FighterState::Name,
            false,
            false,
            meteor_physics::jump::AirMotionMode::Ballistic,
        );
        let empty_slot_switch =
            weapon_switch_entry_allowed(true, false, false, false, ground_context, true);
        assert!(empty_slot_switch);
        assert!(!attack_input_reaches_auto_lock(
            true,
            empty_slot_switch,
            false,
            FighterState::Name,
        ));
    }

    #[test]
    fn weapon_switch_context_matches_ground_and_free_air_ownership() {
        use meteor_physics::jump::AirMotionMode;

        assert_eq!(
            weapon_switch_context_pose(FighterState::Name, false, false, AirMotionMode::Ballistic),
            Some(WEAPON_SWITCH_POSE_GROUND)
        );
        assert_eq!(
            weapon_switch_context_pose(FighterState::Name, true, false, AirMotionMode::Ballistic),
            None,
            "linked recovery still owns ground mode"
        );
        assert_eq!(
            weapon_switch_context_pose(FighterState::Jump, false, true, AirMotionMode::Ballistic),
            Some(WEAPON_SWITCH_POSE_AIR)
        );
        assert_eq!(
            weapon_switch_context_pose(FighterState::Name, false, true, AirMotionMode::Ballistic),
            None,
            "ledge-fall Name is not the Jump mode-0 switch entry"
        );
        for mode in [
            AirMotionMode::Wall,
            AirMotionMode::Curve,
            AirMotionMode::CurveExit,
        ] {
            assert_eq!(
                weapon_switch_context_pose(FighterState::Jump, false, true, mode),
                None,
                "only free ballistic air owns Pose 36"
            );
        }

        let air_context = Some(WEAPON_SWITCH_POSE_AIR);
        assert!(weapon_switch_entry_allowed(
            true,
            false,
            false,
            false,
            air_context,
            true,
        ));
        assert!(!weapon_switch_entry_allowed(
            true,
            false,
            false,
            true,
            air_context,
            true,
        ));
    }

    #[test]
    fn guard_hold_uses_the_actor_owner_instead_of_the_players_keyboard_for_everyone() {
        assert!(actor_guard_held(true, true, None));
        assert!(!actor_guard_held(true, false, Some(1.0)));

        assert!(!actor_guard_held(false, true, None));

        assert!(actor_guard_held(false, false, Some(0.25)));
        assert!(!actor_guard_held(false, false, Some(0.0)));
    }

    #[test]
    fn local_lock_never_remaps_an_npc_locomotion_pose() {
        assert!(actor_uses_local_lock(true, true));
        assert!(!actor_uses_local_lock(true, false));
        assert!(!actor_uses_local_lock(false, true));
        let pose_for = |local| {
            walk_pose_for_input(
                0.0,
                0.0,
                WalkOptions {
                    strategy_id: Some(0),
                    locked: actor_uses_local_lock(local, true),
                    ..WalkOptions::default()
                },
            )
        };
        assert_eq!(pose_for(true), 6);
        assert_eq!(pose_for(false), L::IDLE);
    }

    #[test]
    fn pose_resolve_releases_already_integrated_attack_motion_without_an_extra_carry_tick() {
        let mut motion = FighterAttackMotion {
            active: true,
            stopping: false,
            carry_x: 3.0,
            carry_z: 4.0,
            ..default()
        };
        assert!(release_attack_motion_after_pose_resolve(
            &mut motion,
            true,
            false,
        ));
        assert!(!motion.active);
        assert!(!motion.stopping);

        assert_eq!((motion.carry_x, motion.carry_z), (3.0, 4.0));

        let mut continuing = FighterAttackMotion {
            active: true,
            ..default()
        };
        assert!(!release_attack_motion_after_pose_resolve(
            &mut continuing,
            true,
            true,
        ));
        assert!(continuing.active);
    }

    #[test]
    fn local_be_hit_banks_velocity_and_down_hold_spends_it_through_decay() {
        let mut runtime = FighterHitReaction::default();
        runtime.bank_be_hit_carry(Vec3::new(12.0, 7.0, -6.0), true);
        assert_eq!((runtime.carry_x, runtime.carry_z), (12.0, -6.0));

        let dt = 1.0 / 30.0;
        let (dx, dz) = advance_down_carry(&mut runtime, &FighterAir::default(), dt);
        let k = 1.0 - 9.0 * dt;
        assert!((runtime.carry_x - 12.0 * k).abs() < 1e-6);
        assert!((runtime.carry_z + 6.0 * k).abs() < 1e-6);
        assert!((dx - runtime.carry_x * dt).abs() < 1e-6);
        assert!((dz - runtime.carry_z * dt).abs() < 1e-6);

        runtime.bank_be_hit_carry(Vec3::new(12.0, 0.0, -6.0), false);
        assert_eq!((runtime.carry_x, runtime.carry_z), (0.0, 0.0));

        let steep = FighterAir {
            ground_external_velocity: Vec3::new(3.0, 0.0, -1.5),
            carry_decay_steep: true,
            ..default()
        };
        runtime.carry_x = 12.0;
        runtime.carry_z = -6.0;
        let (dx, dz) = advance_down_carry(&mut runtime, &steep, dt);
        let steep_k = 1.0 - 0.2 * dt;
        assert!((runtime.carry_x - 12.0 * steep_k).abs() < 1e-6);
        assert!((dx - (runtime.carry_x + 3.0) * dt).abs() < 1e-6);
        assert!((dz - (runtime.carry_z - 1.5) * dt).abs() < 1e-6);
    }

    #[test]
    fn production_remote_pose_advances_between_role_state_edges() {
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
        let mut pose = PoseRuntime::idle();
        character.apply_pose(&mut pose, 0, true);
        pose.pose_instance_id = 17;
        let start_frame = pose.frame;

        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
                "p0".to_string(),
                character,
            )])))
            .add_systems(Update, advance_poses);
        let entity = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "remote".into(),
                },
                Fighter {
                    character_id: 0,
                    player_slot: 1,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                pose,
                EntryBlend::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.0 / 30.0));
        app.update();

        let advanced = app.world().get::<PoseRuntime>(entity).unwrap();
        assert!(advanced.frame > start_frame);
        assert_eq!(advanced.pose_instance_id, 17);
    }

    #[test]
    fn production_remote_natural_link_preserves_pose_instance() {
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
        let (source_pose_id, link_pose_id) = character
            .poses
            .iter()
            .find_map(|(&pose_id, pose)| {
                (!pose.loops)
                    .then_some(pose.link)
                    .flatten()
                    .filter(|link| character.poses.contains_key(link))
                    .map(|link| (pose_id, link))
            })
            .expect("p0 has a non-looping authored Link");
        let mut pose = PoseRuntime::idle();
        character.apply_pose(&mut pose, source_pose_id, true);
        pose.frame = pose.end as f32;
        pose.pose_instance_id = 41;

        let mut app = App::new();
        app.insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
            "p0".to_string(),
            character,
        )])))
        .add_systems(Update, resolve_completed_pose_links);
        let entity = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "remote".into(),
                },
                Fighter {
                    character_id: 0,
                    player_slot: 1,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                pose,
                EntryBlend::default(),
                SmState::default(),
                CombatState::default(),
                Health::default(),
                Qi::default(),
                FighterAir::default(),
                WeaponForm::default(),
            ))
            .id();
        app.update();

        let linked = app.world().get::<PoseRuntime>(entity).unwrap();
        assert_eq!(linked.pose_id, link_pose_id);
        assert_eq!(linked.frame, linked.start as f32);
        assert_eq!(linked.pose_instance_id, 41);
    }

    #[test]
    fn original_sword_directional_hits_drive_the_matching_world_root_motion() {
        use meteor_combat::hit_reaction::{pick_reaction_pose_id, FacingBucket};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root),
            "p0",
        )
        .expect("original p0 assets");
        let common = meteor_asset::amb::parse_amb(
            &std::fs::read(root.join("Common/character.amb")).unwrap(),
        )
        .unwrap();
        let attack = character.poses[&262].attacks[0].clone();
        let poses: Vec<u16> = character.poses.keys().copied().collect();
        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(StageCollider {
                mesh: Some(harness_ground()),
            })
            .add_systems(Update, hit_reaction_root_motion);
        let mut cases = Vec::new();

        for yaw in [0.0, std::f32::consts::FRAC_PI_2, std::f32::consts::PI, -0.7] {
            for (angle, bucket, pose_id, delta) in [
                (0.0, FacingBucket::Front, 93, Vec3::new(-3.0, 0.0, 0.0)),
                (
                    std::f32::consts::PI,
                    FacingBucket::Back,
                    92,
                    Vec3::new(3.0, 0.0, 0.0),
                ),
                (
                    std::f32::consts::FRAC_PI_2,
                    FacingBucket::Left,
                    94,
                    Vec3::new(0.0, 0.0, 3.0),
                ),
                (
                    -std::f32::consts::FRAC_PI_2,
                    FacingBucket::Right,
                    91,
                    Vec3::new(0.0, 0.0, -3.0),
                ),
            ] {
                let incoming = yaw + angle;
                let actual_bucket =
                    facing_bucket(incoming.sin() * 10.0, incoming.cos() * 10.0, 0.0, 0.0, yaw);
                assert_eq!(actual_bucket, bucket);
                assert_eq!(
                    pick_reaction_pose_id(&attack, actual_bucket, &poses),
                    Some(pose_id)
                );
                let packet = meteor_network::stock_protocol::StockHitRequest::from_attack(
                    2,
                    262,
                    6,
                    &attack,
                    actual_bucket,
                    &poses,
                );
                assert_eq!(packet.target_pose_word & 0xfff, u32::from(pose_id));
                let mut pose = PoseRuntime::idle();
                let mut blend = EntryBlend::default();
                let mut combat = CombatState::default();
                let mut health = Health::default();
                let mut qi = Qi::default();
                let air = FighterAir::default();
                let mut anger_spend = None;
                let reaction = begin_be_hit(
                    &mut CombatView {
                        pose: &mut pose,
                        blend: &mut blend,
                        combat: &mut combat,
                        health: &mut health,
                        qi: &mut qi,
                        air: &air,
                        assets: &character,
                        weapon: 5,
                        guard_held_input: false,
                        in_guard_hit_reaction: false,
                        attack_directions: Vec::new(),
                        weapon_form: 0,
                        anger_spend: &mut anger_spend,
                        strategy_id: weapon_to_strategy(5),
                        weapon_action_vetoed: false,
                    },
                    Some(&attack),
                    actual_bucket,
                    &poses,
                )
                .unwrap();
                assert_eq!(pose.pose_id, pose_id);
                let mut runtime = FighterHitReaction::default();
                runtime.begin(reaction, false);
                let initial_carry = if yaw == -0.7 {
                    Vec3::new(12.0, 0.0, -6.0)
                } else {
                    Vec3::ZERO
                };
                runtime.bank_be_hit_carry(initial_carry, true);
                let carry_distance = 0.7 * (1.0 - 0.7_f32.powi(31)) / (1.0 - 0.7) / 30.0;
                let entity = app
                    .world_mut()
                    .spawn((
                        Fighter {
                            character_id: 0,
                            player_slot: 0,
                            weapon: 5,
                            weapon_slots: [5, 0],
                            weapon_slot: 0,
                            weapon_slot_variants: [0, 0],
                            weapon_variant: 0,
                        },
                        pose,
                        Transform::default(),
                        MotionState {
                            facing: yaw,
                            velocity: Vec3::ZERO,
                            movement_mode: meteor_core::MovementMode::Idle,
                        },
                        air,
                        runtime,
                    ))
                    .id();
                cases.push((
                    entity,
                    Quat::from_rotation_y(yaw) * delta + initial_carry * carry_distance,
                    character.poses[&pose_id].end,
                ));
            }
        }
        app.insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
            "p0".to_string(),
            character,
        )])))
        .insert_resource(CommonAmb(common));
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.0 / 30.0));
        app.update();
        for _ in 0..30 {
            for (entity, _, end) in &cases {
                let mut pose = app.world_mut().get_mut::<PoseRuntime>(*entity).unwrap();
                pose.frame = (pose.frame + 1.0).min(*end as f32);
            }
            app.update();
        }
        for (entity, expected, _) in cases {
            let actual = app.world().get::<Transform>(entity).unwrap().translation;
            assert!(
                (actual.x - expected.x).abs() < 0.01 && (actual.z - expected.z).abs() < 0.01,
                "authored reaction direction: {actual:?} != {expected:?}"
            );
        }
    }

    #[test]
    fn remote_hit_root_motion_advances_the_observer_correction_target() {
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
        let pose_id = 93;
        let pose_def = character.poses[&pose_id].clone();
        let bank = character.amb_for_pose(&common, pose_id);
        let base_index = character.dummy_order.get("d_base").copied();
        let sample_root = |frame: f32| {
            let sampled = bank.sample_at(frame);
            base_index
                .and_then(|index| sampled.dummy_pos.get(index).copied())
                .unwrap_or(sampled.root_pivot)
        };
        let start_root = sample_root(pose_def.start as f32);
        let motion_frame = ((pose_def.start + 1)..=pose_def.end)
            .find(|frame| sample_root(*frame as f32) != start_root)
            .expect("pose 93 authored d_base motion") as f32;
        let fighter = Fighter {
            character_id: 0,
            player_slot: 1,
            weapon: 6,
            weapon_slots: [6, 5],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        };
        let mut pose = PoseRuntime::idle();
        let mut blend = EntryBlend::default();
        let mut combat = CombatState::default();
        let mut health = Health::default();
        let mut qi = Qi::default();
        let air = FighterAir::default();
        let attack = meteor_asset::pos::AttackBlock {
            start: 0,
            end: 0,
            attack_type: 0,
            check_friend: 0,
            defense_value: 0.0,
            defense_move: 0.0,
            target_value: 0.0,
            target_move: 1.0,
            target_pose: Some(pose_id),
            target_pose_front: Some(pose_id),
            target_pose_back: Some(pose_id),
            target_pose_left: Some(pose_id),
            target_pose_right: Some(pose_id),
            bones: Vec::new(),
        };
        let poses = character.poses.keys().copied().collect::<Vec<_>>();
        let reaction = {
            let mut anger_spend = None;
            let mut view = CombatView {
                pose: &mut pose,
                blend: &mut blend,
                combat: &mut combat,
                health: &mut health,
                qi: &mut qi,
                air: &air,
                assets: &character,
                weapon: fighter.weapon,
                guard_held_input: false,
                in_guard_hit_reaction: false,
                attack_directions: Vec::new(),
                weapon_form: 0,
                anger_spend: &mut anger_spend,
                strategy_id: weapon_to_strategy(fighter.weapon),
                weapon_action_vetoed: false,
            };
            begin_be_hit(
                &mut view,
                Some(&attack),
                meteor_combat::hit_reaction::FacingBucket::Front,
                &poses,
            )
            .unwrap()
        };
        let mut reaction_runtime = FighterHitReaction::default();
        reaction_runtime.begin(reaction, false);
        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
                "p0".to_string(),
                character,
            )])))
            .insert_resource(CommonAmb(common))
            .insert_resource(StageCollider {
                mesh: Some(crate::plugins::stage_collider::StageMeshCollider::default()),
            })
            .add_systems(Update, hit_reaction_root_motion);
        let entity = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "remote".into(),
                },
                fighter,
                pose,
                Transform::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                air,
                reaction_runtime,
                RemoteTransformTarget {
                    translation: Vec3::ZERO,
                    yaw: 0.0,
                },
                RemoteHitMotionWindow { remaining: 0.4 },
            ))
            .id();
        let dt = 1.0 / 60.0;
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(dt));
        app.update();
        app.world_mut()
            .get_mut::<PoseRuntime>(entity)
            .unwrap()
            .frame = motion_frame;
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(dt));
        app.update();

        let position = app.world().get::<Transform>(entity).unwrap().translation;
        assert!(position.length_squared() > 1e-8);
        assert_eq!(
            app.world()
                .get::<RemoteTransformTarget>(entity)
                .unwrap()
                .translation,
            position
        );
        assert!(
            app.world()
                .get::<RemoteHitMotionWindow>(entity)
                .unwrap()
                .remaining
                < 0.4
        );

        for _ in 0..30 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(dt));
            app.update();
        }
        assert_eq!(
            app.world()
                .get::<RemoteHitMotionWindow>(entity)
                .unwrap()
                .remaining,
            0.0
        );
        assert!(app
            .world()
            .get::<FighterHitReaction>(entity)
            .unwrap()
            .current
            .is_none());
    }

    #[test]
    fn death_clears_published_velocity_without_destroying_facing_or_mode() {
        let mut motion = MotionState {
            movement_mode: meteor_core::MovementMode::Airborne,
            velocity: Vec3::new(8.0, -3.0, 5.0),
            facing: 1.2,
        };

        clear_composed_velocity_on_death(&mut motion);

        assert_eq!(motion.velocity, Vec3::ZERO);
        assert_eq!(motion.movement_mode, meteor_core::MovementMode::Airborne);
        assert_eq!(motion.facing, 1.2);
    }

    #[test]
    fn lethal_local_hit_does_not_reach_auto_lock_but_remote_claim_does() {
        assert!(hit_result_reaches_auto_lock(false, false));
        assert!(!hit_result_reaches_auto_lock(true, false));

        assert!(hit_result_reaches_auto_lock(true, true));
    }

    #[test]
    fn queued_qi_burst_uses_next_pose_window_without_source_act_edge() {
        let queued = queued_qi_burst(262, 9432, 0.15, 60.0, true).unwrap();
        assert_eq!(queued.source_pose_id, 262);
        assert_eq!(queued.next_pose_id, QI_BURST_POSE_ID);
        assert_eq!(queued.action_id, QI_BURST_ACTION_ID);
        assert_eq!(queued.last_evaluated_frame, 9432);
        assert_eq!(queued.transition_time, 0.15);
        assert_eq!(queued.anger_cost, 60);
    }

    #[test]
    fn queued_qi_burst_keeps_resource_and_pose_gates() {
        assert!(queued_qi_burst(262, 9432, 0.15, 59.0, true).is_none());
        assert!(queued_qi_burst(262, 9432, 0.15, 100.0, false).is_none());
    }

    #[test]
    fn queued_attack_gate_distinguishes_pending_from_terminal_rejection() {
        assert_eq!(
            queued_attack_commit_gate(false, true, 60.0, 60),
            QueuedAttackCommit::Pending
        );
        assert_eq!(
            queued_attack_commit_gate(true, false, 60.0, 60),
            QueuedAttackCommit::Rejected
        );
        assert_eq!(
            queued_attack_commit_gate(true, true, 59.0, 60),
            QueuedAttackCommit::Rejected
        );
        assert_eq!(
            queued_attack_commit_gate(true, true, f32::NAN, 60),
            QueuedAttackCommit::Rejected
        );
        assert_eq!(
            queued_attack_commit_gate(true, true, 60.0, 60),
            QueuedAttackCommit::Committed
        );
    }

    fn plain_pose() -> meteor_asset::pos::Pose {
        meteor_asset::pos::Pose {
            id: 1,
            source: 1,
            start: 0,
            end: 0,
            loop_start: 0,
            loop_end: 0,
            loops: false,
            link: None,
            note: String::new(),
            effect_type: None,
            effect_id: None,
            attacks: vec![],
            next_pose: None,
            blends: vec![],
            actions: vec![],
            action_entry_frame: 0,
            drags: vec![],
        }
    }

    fn loop_runtime(
        start: u32,
        end: u32,
        loop_start: u32,
        loop_end: u32,
        loops: bool,
    ) -> PoseRuntime {
        PoseRuntime {
            pose_id: 1,
            frame: 0.0,
            start,
            end,
            loop_start,
            loop_end,
            loops,
            pose_instance_id: 0,
        }
    }

    fn speed_pose(segments: Vec<(u32, u32, f32)>) -> meteor_asset::pos::Pose {
        let mut pose = plain_pose();
        pose.actions = segments
            .into_iter()
            .map(|(s, e, sp)| meteor_asset::pos::SpeedBlock {
                start: Some(s),
                end: Some(e),
                speed: sp,
            })
            .collect();
        pose
    }

    #[test]
    fn pose_cursor_is_frame_rate_independent() {
        let pose = speed_pose(vec![(0, 59, 1.0)]);
        let mut f60 = 0.0_f32;
        for _ in 0..60 {
            let mut rt = loop_runtime(0, 59, 0, 59, true);
            rt.frame = f60;
            f60 = advance_pose_cursor(&pose, &rt, 1.0 / 60.0);
        }
        let mut f144 = 0.0_f32;
        for _ in 0..144 {
            let mut rt = loop_runtime(0, 59, 0, 59, true);
            rt.frame = f144;
            f144 = advance_pose_cursor(&pose, &rt, 1.0 / 144.0);
        }

        assert!((f60 - 30.0).abs() < 1.0);
        assert!((f144 - 30.0).abs() < 1.0);

        assert!((f60 - f144).abs() < 1e-3);
    }

    #[test]
    fn pose_cursor_speed_segment_scales_budget() {
        let pose = speed_pose(vec![(0, 9, 0.5), (10, 19, 2.0)]);

        let mut f = 0.0_f32;
        for _ in 0..10 {
            let mut rt = loop_runtime(0, 19, 0, 19, true);
            rt.frame = f;
            f = advance_pose_cursor(&pose, &rt, 1.0 / 60.0);
        }
        assert!((f - 2.5).abs() < 1e-3);

        let mut f = 10.0_f32;
        for _ in 0..9 {
            let mut rt = loop_runtime(0, 19, 0, 19, true);
            rt.frame = f;
            f = advance_pose_cursor(&pose, &rt, 1.0 / 60.0);
        }
        assert!((f - 19.0).abs() < 1e-3);
    }

    #[test]
    fn pose_cursor_loop_wraps_to_integer_loop_start() {
        let pose = plain_pose();

        let mut rt = loop_runtime(100, 120, 100, 120, true);
        rt.frame = 118.0;
        let f = advance_pose_cursor(&pose, &rt, 1.0 / 60.0);
        assert!((f - 118.5).abs() < 1e-4);

        rt.frame = 119.8;
        let f = advance_pose_cursor(&pose, &rt, 1.0 / 60.0);
        assert!((f - 100.0).abs() < 1e-4);
    }

    #[test]
    fn pose_cursor_non_loop_freezes_at_end() {
        let pose = plain_pose();
        let mut f = 0.0_f32;
        for _ in 0..10 {
            let mut rt = loop_runtime(0, 5, 0, 5, false);
            rt.frame = f;
            f = advance_pose_cursor(&pose, &rt, 1.0 / 60.0);
        }

        assert!((f - 5.0).abs() < 1e-4);

        let mut rt = loop_runtime(0, 5, 0, 5, false);
        rt.frame = f;
        let f = advance_pose_cursor(&pose, &rt, 1.0 / 60.0);
        assert!((f - 5.0).abs() < 1e-4);
    }

    #[test]
    fn attack_window_truncated_frame_semantics() {
        assert!(attack_window_active(10.0, 10, 12));
        assert!(attack_window_active(10.9, 10, 12));
        assert!(attack_window_active(12.9, 10, 12));
        assert!(!attack_window_active(13.0, 10, 12));
        assert!(!attack_window_active(9.9, 10, 12));

        assert!(attack_window_active(0.5, 0, 0));
        assert!(!attack_window_active(1.0, 0, 0));
    }

    fn attack_block(start: u32, end: u32, attack_type: u32) -> meteor_asset::pos::AttackBlock {
        meteor_asset::pos::AttackBlock {
            start,
            end,
            attack_type,
            check_friend: 0,
            defense_value: 0.0,
            defense_move: 0.0,
            target_value: 0.0,
            target_move: 0.0,
            target_pose: None,
            target_pose_front: None,
            target_pose_back: None,
            target_pose_left: None,
            target_pose_right: None,
            bones: vec![],
        }
    }

    #[test]
    fn active_attack_window_picks_the_frame_matching_block() {
        let attacks = vec![attack_block(10, 12, 1), attack_block(20, 22, 2)];
        let (index, window) = active_attack_window(11.0, &attacks).unwrap();
        assert_eq!(index, 0);
        assert_eq!(window.attack_type, 1);
        let (index, window) = active_attack_window(21.5, &attacks).unwrap();
        assert_eq!(index, 1);
        assert_eq!(window.attack_type, 2);
    }

    #[test]
    fn active_attack_window_none_outside_all_ranges() {
        let attacks = vec![attack_block(10, 12, 1)];
        assert!(active_attack_window(9.0, &attacks).is_none());
        assert!(active_attack_window(13.0, &attacks).is_none());
        assert!(active_attack_window(0.0, &[]).is_none());
    }

    #[test]
    fn active_attack_window_last_declaration_wins_on_overlap() {
        let attacks = vec![attack_block(10, 20, 1), attack_block(15, 25, 2)];

        let (index, window) = active_attack_window(18.0, &attacks).unwrap();
        assert_eq!(index, 1);
        assert_eq!(window.attack_type, 2);

        let (index, _) = active_attack_window(12.0, &attacks).unwrap();
        assert_eq!(index, 0);

        let (index, _) = active_attack_window(24.0, &attacks).unwrap();
        assert_eq!(index, 1);
    }

    #[test]
    fn target_hit_clock_accepts_first_hit_immediately() {
        use crate::components::fighter::TargetHitClock;

        let clock = TargetHitClock::default();
        assert!(clock.accepts_target_hit());
    }

    #[test]
    fn target_hit_clock_blocks_within_interval_and_reopens_after() {
        use crate::components::fighter::{TargetHitClock, TARGET_HIT_INTERVAL_SECONDS};
        let mut clock = TargetHitClock::default();
        clock.reset();
        assert!(!clock.accepts_target_hit());

        clock.tick(TARGET_HIT_INTERVAL_SECONDS);
        assert!(!clock.accepts_target_hit());

        clock.tick(0.01);
        assert!(clock.accepts_target_hit());
    }

    #[test]
    fn target_hit_clock_spawn_protection_expires() {
        use crate::components::fighter::{TargetHitClock, SPAWN_INVULNERABLE_SECONDS};
        let mut clock = TargetHitClock::default();

        assert!(clock.spawn_protected());
        clock.tick(SPAWN_INVULNERABLE_SECONDS);
        assert!(!clock.spawn_protected());
    }

    #[test]
    fn target_hit_clock_reset_spawn_restarts_protection() {
        use crate::components::fighter::TargetHitClock;
        let mut clock = TargetHitClock::default();
        clock.tick(10.0);
        assert!(!clock.spawn_protected());

        clock.reset_spawn();
        assert!(clock.spawn_protected());
    }

    #[test]
    fn spawn_protected_intersection_resets_target_interval_before_rejecting_damage() {
        use crate::components::fighter::{SPAWN_INVULNERABLE_SECONDS, TARGET_HIT_INTERVAL_SECONDS};

        let mut clock = TargetHitClock::default();
        assert_eq!(
            admit_intersecting_target(&mut clock),
            IntersectingTargetAdmission::SpawnProtected
        );
        assert_eq!(clock.elapsed, 0.0);

        clock.tick_target(TARGET_HIT_INTERVAL_SECONDS + 0.01);
        assert_eq!(
            admit_intersecting_target(&mut clock),
            IntersectingTargetAdmission::SpawnProtected
        );
        assert_eq!(clock.elapsed, 0.0);

        clock.spawn_elapsed = SPAWN_INVULNERABLE_SECONDS;
        clock.tick_target(TARGET_HIT_INTERVAL_SECONDS);
        assert_eq!(
            admit_intersecting_target(&mut clock),
            IntersectingTargetAdmission::IntervalBlocked,
            "the interval is strict even after spawn protection expires"
        );
        clock.tick_target(0.01);
        assert_eq!(
            admit_intersecting_target(&mut clock),
            IntersectingTargetAdmission::Accepted
        );
        assert_eq!(clock.elapsed, 0.0);
    }

    #[test]
    fn pose_attack_snapshots_revalidate_interrupts_before_persistent_projectiles() {
        assert!(attack_window_owner_active(
            100.0,
            false,
            FighterState::Attack1,
            false
        ));
        assert!(!attack_window_owner_active(
            100.0,
            false,
            FighterState::Attack1,
            true
        ));
        assert!(!attack_window_owner_active(
            100.0,
            false,
            FighterState::BeHit,
            false
        ));
        assert!(!attack_window_owner_active(
            0.0,
            false,
            FighterState::Attack1,
            false
        ));
        assert!(attack_window_owner_active_for_scope(
            100.0,
            false,
            FighterState::Name,
            false,
            true,
        ));
        assert!(!attack_window_owner_active_for_scope(
            100.0,
            false,
            FighterState::Name,
            true,
            true,
        ));

        let snapshot = |id: u64, slot: u8, live| AttackSnapshot {
            entity: Entity::from_bits(id),
            weapon: 6,
            weapon_stats: None,
            pose_id: 262,
            pose_frame: 0.0,
            pose_start: 0,
            player_slot: slot,
            combat_team: 1,
            script_character_id: 0,
            script_team: None,
            atk_pos: Vec3::ZERO,
            forward: Vec3::Z,
            active_window: None,
            requires_live_attack_owner: live,
            props_only: false,
            effect_origin: None,
            effect_obbs: Vec::new(),
            geometry: AttackGeometry::Obbs(Vec::new()),
        };
        let mut snapshots = vec![
            snapshot(40, 0, false),
            snapshot(12, 2, true),
            snapshot(10, 0, true),
            snapshot(41, 1, false),
            snapshot(11, 1, true),
        ];
        sort_attack_snapshots(&mut snapshots);
        assert_eq!(
            snapshots
                .iter()
                .map(|snapshot| snapshot.entity.to_bits())
                .collect::<Vec<_>>(),
            vec![10, 11, 12, 40, 41],
            "local/NPC pose windows resolve first by slot; projectiles keep emission order"
        );
    }

    #[test]
    fn attack_window_forward_is_the_actor_yaw_after_lock_entry_has_finished() {
        assert_eq!(attack_forward_from_facing(0.0), Vec3::Z);
        let east = attack_forward_from_facing(std::f32::consts::FRAC_PI_2);
        assert!((east.x - 1.0).abs() < 1e-6);
        assert!(east.z.abs() < 1e-6);
        let west = attack_forward_from_facing(-std::f32::consts::FRAC_PI_2);
        assert!((west.x + 1.0).abs() < 1e-6);
        assert!(west.z.abs() < 1e-6);
    }

    #[test]
    fn dart_release_waits_for_authored_attack_window_and_freezes_yaw() {
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
        let pose = character.poses.get(&200).expect("dart light pose");
        let authored = pose.attacks.first().expect("authored release window");
        assert!(
            active_attack_window(pose.start as f32, &pose.attacks).is_none(),
            "attack commit/start frame must not release the dart"
        );
        let (index, active) = active_attack_window(authored.start as f32, &pose.attacks)
            .expect("first authored Attack frame releases the dart");
        assert_eq!(index, 0);
        assert_eq!(active.start, 3517);

        let release_forward = attack_forward_from_facing(std::f32::consts::FRAC_PI_2);
        assert!((release_forward.x - 1.0).abs() < 1e-5);
        assert!(release_forward.z.abs() < 1e-5);
        let later_facing = -std::f32::consts::FRAC_PI_2;
        assert_ne!(
            release_forward,
            attack_forward_from_facing(later_facing),
            "the projectile keeps its release-frame direction after later turns"
        );
    }

    use crate::plugins::input_plugin::{
        DesktopDodgeInput, LocalCombatInput, LocalCombatInputEdge as Edge, LocalCommandInput,
        LocalCrouchInput, LocalGuardInput, LocalJumpInput, SyntheticInputStream,
    };
    use crate::plugins::movement_plugin::{FighterAir, FighterDodge, FighterWall};
    use crate::plugins::stage_collider::{StageCollider, StageMeshCollider};
    use bevy::input::keyboard::{Key, KeyboardInput};
    use bevy::input::mouse::MouseButtonInput;
    use bevy::input::ButtonState;
    use bevy::window::WindowEvent;

    const HARNESS_DT: f32 = 1.0 / 30.0;

    const DAGGER: u8 = 5;

    #[derive(Resource, Default)]
    struct HarnessPoseBeforeAdvance(Option<u16>);

    fn capture_harness_pose_before_advance(
        query: Query<&PoseRuntime, With<LocalFighter>>,
        mut captured: ResMut<HarnessPoseBeforeAdvance>,
    ) {
        captured.0 = query.iter().next().map(|pose| pose.pose_id);
    }

    fn harness_retire_entry_blend(
        time: Res<Time<bevy::time::Real>>,
        mut query: Query<(&PoseRuntime, &mut EntryBlend)>,
    ) {
        let now = time.elapsed_secs_f64();
        for (pose, mut blend) in &mut query {
            if blend.duration <= 0.0 || blend.pose_id != pose.pose_id {
                continue;
            }
            if blend.started_at_seconds.is_none() {
                blend.started_at_seconds = Some(now);
            }
            let started = blend.started_at_seconds.unwrap_or(now);
            blend.elapsed = (now - started).max(0.0) as f32;
            if blend.elapsed / blend.duration >= 1.0 {
                blend.duration = 0.0;
                blend.elapsed = 0.0;
                blend.started_at_seconds = None;
            }
        }
    }

    struct GameplayHarness {
        app: App,
        fighter: Entity,
    }

    impl GameplayHarness {
        fn key_edge(code: KeyCode, pressed: bool) -> WindowEvent {
            WindowEvent::KeyboardInput(KeyboardInput {
                key_code: code,
                logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
                state: if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                },
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            })
        }

        fn mouse_edge(button: MouseButton, pressed: bool) -> WindowEvent {
            WindowEvent::MouseButtonInput(MouseButtonInput {
                button,
                state: if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                },
                window: Entity::PLACEHOLDER,
            })
        }

        fn press_key(&mut self, code: KeyCode) {
            self.app
                .world_mut()
                .write_message(Self::key_edge(code, true));
            self.app
                .world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(code);
        }

        fn release_key(&mut self, code: KeyCode) {
            self.app
                .world_mut()
                .write_message(Self::key_edge(code, false));
            self.app
                .world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .release(code);
        }

        fn press_binding(&mut self, binding: crate::app_state::InputBinding) {
            match binding {
                crate::app_state::InputBinding::Key(code) => self.press_key(code),
                crate::app_state::InputBinding::Mouse(button) => self.press_mouse(button),
            }
        }

        fn release_binding(&mut self, binding: crate::app_state::InputBinding) {
            match binding {
                crate::app_state::InputBinding::Key(code) => self.release_key(code),
                crate::app_state::InputBinding::Mouse(button) => self.release_mouse(button),
            }
        }

        fn press_mouse(&mut self, button: MouseButton) {
            self.app
                .world_mut()
                .write_message(Self::mouse_edge(button, true));
            self.app
                .world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .press(button);
        }

        fn release_mouse(&mut self, button: MouseButton) {
            self.app
                .world_mut()
                .write_message(Self::mouse_edge(button, false));
            self.app
                .world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .release(button);
        }

        fn mobile_binding(
            &mut self,
            binding: crate::app_state::InputBinding,
            pressed: bool,
            action: Option<crate::plugins::input_plugin::SyntheticAction>,
        ) {
            match binding {
                crate::app_state::InputBinding::Key(key) => {
                    let mut input = self.app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
                    if pressed {
                        input.press(key);
                    } else {
                        input.release(key);
                    }
                }
                crate::app_state::InputBinding::Mouse(button) => {
                    let mut input = self
                        .app
                        .world_mut()
                        .resource_mut::<ButtonInput<MouseButton>>();
                    if pressed {
                        input.press(button);
                    } else {
                        input.release(button);
                    }
                }
            }
            let mut stream = self.app.world_mut().resource_mut::<SyntheticInputStream>();
            if let Some(action) = action {
                stream.push_action(binding, action, pressed, None);
            } else {
                stream.push_mobile_direction(binding, pressed);
            }
        }

        fn touch(&mut self, id: u64, phase: bevy::input::touch::TouchPhase, position: Vec2) {
            self.app
                .world_mut()
                .write_message(bevy::input::touch::TouchInput {
                    phase,
                    position,
                    window: Entity::PLACEHOLDER,
                    force: None,
                    id,
                });
        }

        fn step(&mut self) {
            self.step_dt(HARNESS_DT);
        }

        fn step_dt(&mut self, dt: f32) {
            let dt = std::time::Duration::from_secs_f32(dt);
            self.app.world_mut().resource_mut::<Time>().advance_by(dt);
            self.app
                .world_mut()
                .resource_mut::<Time<bevy::time::Real>>()
                .advance_by(dt);
            self.app.update();

            self.app
                .world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .clear();
            self.app
                .world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
        }

        fn steps(&mut self, count: usize) {
            for _ in 0..count {
                self.step();
            }
        }

        fn position(&self) -> Vec3 {
            self.app
                .world()
                .get::<Transform>(self.fighter)
                .unwrap()
                .translation
        }

        fn state(&self) -> FighterState {
            self.app
                .world()
                .get::<SmState>(self.fighter)
                .unwrap()
                .current
        }

        fn pose_id(&self) -> u16 {
            self.app
                .world()
                .get::<PoseRuntime>(self.fighter)
                .unwrap()
                .pose_id
        }

        fn airborne(&self) -> bool {
            self.app
                .world()
                .get::<FighterAir>(self.fighter)
                .unwrap()
                .air
                .airborne
        }

        fn replace_stage_mesh(&mut self, mesh: StageMeshCollider) {
            self.app.world_mut().resource_mut::<StageCollider>().mesh = Some(mesh);
        }

        fn edges(&self) -> Vec<Edge> {
            self.app
                .world()
                .resource::<LocalCombatInput>()
                .edges
                .clone()
        }

        fn attack_trace(&self, tag: &str) {
            let pose = self.app.world().get::<PoseRuntime>(self.fighter).unwrap();
            let am = self
                .app
                .world()
                .get::<crate::plugins::movement_plugin::FighterAttackMotion>(self.fighter)
                .unwrap();
            let motion = self.app.world().get::<MotionState>(self.fighter).unwrap();
            eprintln!(
                "  [{tag}] pose={}@{:.2} am(active={},hold={},prev_root=({:.3},{:.3}),carry=({:.3},{:.3})) vel=({:.3},{:.3}) pos=({:.3},{:.3},{:.3})",
                pose.pose_id,
                pose.frame,
                am.active,
                am.hold_delta,
                am.prev_root_x,
                am.prev_root_y,
                am.carry_x,
                am.carry_z,
                motion.velocity.x,
                motion.velocity.z,
                self.position().x,
                self.position().y,
                self.position().z,
            );
        }

        fn trace(&self, tag: &str) {
            let pose = self.app.world().get::<PoseRuntime>(self.fighter).unwrap();
            let blend = self.app.world().get::<EntryBlend>(self.fighter).unwrap();
            let dodge = self.app.world().get::<FighterDodge>(self.fighter).unwrap();
            let motion = self.app.world().get::<MotionState>(self.fighter).unwrap();
            eprintln!(
                "[{tag}] state={:?} pose={}@{:.2}[{}..{}] loop={}({}..{}) inst={} blend(d={:.3},pose={}) dodge(active={},pose={},prog={:.4},raw={:.4}) mode={:?} airborne={} pos=({:.3},{:.3},{:.3}) edges={:?}",
                self.state(),
                pose.pose_id,
                pose.frame,
                pose.start,
                pose.end,
                pose.loops,
                pose.loop_start,
                pose.loop_end,
                pose.pose_instance_id,
                blend.duration,
                blend.pose_id,
                dodge.state.active,
                dodge.pose,
                dodge.state.progress,
                dodge.state.raw_progress,
                motion.movement_mode,
                self.airborne(),
                self.position().x,
                self.position().y,
                self.position().z,
                self.edges(),
            );
        }
    }

    fn harness_ground() -> StageMeshCollider {
        let mut mesh = StageMeshCollider::default();
        const R: f32 = 600.0;

        mesh.append_world_triangles(
            &[[-R, 0.0, -R], [-R, 0.0, R], [R, 0.0, R], [R, 0.0, -R]],
            &[0, 1, 2, 0, 2, 3],
        );
        mesh
    }

    fn harness_ground_with_front_wall(wall_z: f32) -> StageMeshCollider {
        let mut mesh = harness_ground();
        mesh.append_world_triangles(
            &[
                [-20.0, 0.0, wall_z],
                [20.0, 0.0, wall_z],
                [20.0, 20.0, wall_z],
                [-20.0, 20.0, wall_z],
            ],
            &[0, 2, 1, 0, 3, 2],
        );
        mesh
    }

    fn harness_ground_with_side_wall(wall_x: f32) -> StageMeshCollider {
        let mut mesh = harness_ground();
        mesh.append_world_triangles(
            &[
                [wall_x, 0.0, -20.0],
                [wall_x, 20.0, -20.0],
                [wall_x, 20.0, 40.0],
                [wall_x, 0.0, 40.0],
            ],
            if wall_x < 0.0 {
                &[0, 1, 2, 0, 2, 3]
            } else {
                &[0, 2, 1, 0, 3, 2]
            },
        );
        mesh
    }

    fn harness_ground_with_side_corridor(half_width: f32) -> StageMeshCollider {
        let mut mesh = harness_ground();
        for x in [-half_width, half_width] {
            mesh.append_world_triangles(
                &[
                    [x, 0.0, -20.0],
                    [x, 20.0, -20.0],
                    [x, 20.0, 40.0],
                    [x, 0.0, 40.0],
                ],
                if x < 0.0 {
                    &[0, 1, 2, 0, 2, 3]
                } else {
                    &[0, 2, 1, 0, 3, 2]
                },
            );
        }
        mesh
    }

    fn spawn_gameplay_harness(weapon: u8) -> Option<GameplayHarness> {
        spawn_gameplay_harness_with_order(weapon, false)
    }

    fn spawn_gameplay_harness_with_order(weapon: u8, serial: bool) -> Option<GameplayHarness> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            return None;
        }
        let character = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root),
            "p1",
        )?;
        let common_amb = std::fs::read(root.join("Common/character.amb"))
            .ok()
            .and_then(|bytes| meteor_asset::amb::parse_amb(&bytes).ok())?;

        let mut pose = PoseRuntime::idle();
        character.apply_pose(&mut pose, 0, true);

        let mut app = App::new();
        crate::plugins::configure_fighter_frame_sets(&mut app);
        if serial {
            super::super::fighter_frame::install(&mut app);
        }
        let actor_schedule = super::super::fighter_frame::schedule(&app);
        app.init_resource::<Time>()
            .init_resource::<Time<bevy::time::Real>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<crate::app_state::Keymap>()
            .init_resource::<Settings>()
            .init_resource::<NetState>()
            .init_resource::<PendingNetActions>()
            .init_resource::<crate::plugins::camera_plugin::CameraBasis>()
            .init_resource::<LockOnController>()
            .init_resource::<LocalCommandInput>()
            .init_resource::<LocalCrouchInput>()
            .init_resource::<LocalGuardInput>()
            .init_resource::<LocalCombatInput>()
            .init_resource::<LocalJumpInput>()
            .init_resource::<DesktopDodgeInput>()
            .init_resource::<SyntheticInputStream>()
            .init_resource::<crate::plugins::item_plugin::ItemBuffs>()
            .init_resource::<crate::components::fighter::KillScore>()
            .init_resource::<HarnessPoseBeforeAdvance>()
            .insert_resource(StageCollider {
                mesh: Some(harness_ground()),
            })
            .insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
                "p1".to_string(),
                character,
            )])))
            .insert_resource(crate::plugins::character_plugin::CommonAmb(common_amb))
            .add_message::<WindowEvent>()
            .add_plugins(crate::plugins::MovementPlugin)
            .add_systems(
                Update,
                (
                    crate::plugins::input_plugin::capture_local_command_input,
                    crate::plugins::input_plugin::capture_local_hold_inputs,
                )
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::Movement),
            )
            .add_systems(
                actor_schedule,
                (
                    resolve_completed_pose_links,
                    combat_tick,
                    sync_weapon_form_state,
                )
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::PoseDecision),
            )
            .add_systems(
                actor_schedule,
                preadvance_local_ground_locomotion_pose
                    .run_if(super::super::fighter_frame::current_is_local)
                    .after(crate::plugins::movement_plugin::move_local_fighter)
                    .in_set(crate::plugins::FighterFrameSet::Locomotion),
            )
            .add_systems(
                actor_schedule,
                (
                    capture_harness_pose_before_advance
                        .run_if(super::super::fighter_frame::current_is_local),
                    advance_poses,
                )
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::PoseAdvance),
            )
            .add_systems(
                Update,
                harness_retire_entry_blend
                    .run_if(move || !serial)
                    .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
            )
            .add_systems(
                actor_schedule,
                (
                    sync_attacks_after_animation,
                    tick_hit_reactions,
                    fist_weapon_swap,
                    default_poses,
                    commit_weapon_switch,
                )
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::PoseResolve),
            );

        let fighter = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 1,
                    player_slot: 0,
                    weapon,
                    weapon_slots: [weapon, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::from_xyz(0.0, 0.0, 0.0),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                FighterAir::default(),
                FighterDodge::default(),
                FighterWall::default(),
                pose,
                EntryBlend::default(),
                SmState::default(),
                CombatState::default(),
                (
                    GunAimState::default(),
                    RangedFireGate::default(),
                    FighterHitReaction::default(),
                    Health::default(),
                    Qi::default(),
                    DeathRespawn::default(),
                    AiIntent::default(),
                    WeaponForm::default(),
                    FighterAttackMotion::default(),
                    crate::components::fighter::TargetHitClock::default(),
                ),
            ))
            .id();

        let mut harness = GameplayHarness { app, fighter };
        harness.steps(2);
        Some(harness)
    }

    #[test]
    fn sn13_perform_release_accepts_attack_on_same_frame_with_or_without_lock() {
        use crate::plugins::stage_plugin::{
            drive_player_performance, NpcPerformCommand, PlayerPerformance,
        };
        use meteor_scene::pst_runtime::PstValue;
        for (locked, fresh_attack) in [(false, false), (false, true), (true, false), (true, true)] {
            let mut h =
                spawn_gameplay_harness_with_order(6, true).expect("original p1 resources required");
            let mut performance = PlayerPerformance::default();
            for (action, value) in [("block", 0.0), ("pause", 20.0), ("block", 1.0)] {
                performance.queue.push_back(NpcPerformCommand {
                    action: action.into(),
                    args: vec![PstValue::Num(value)],
                });
            }
            h.app.insert_resource(performance).add_systems(
                Update,
                drive_player_performance
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .before(crate::plugins::input_plugin::capture_local_command_input),
            );
            let target = h
                .app
                .world_mut()
                .spawn((Health::default(), Transform::from_xyz(0.0, 0.0, 5.0)))
                .id();
            if locked {
                let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                lock.system.lock_target = Some(target.to_bits());
                lock.lock_entity = Some(target);
            }

            for _ in 0..42 * 20 {
                h.step_dt(1.0 / 60.0);
            }
            assert!(h.app.world().resource::<PlayerPerformance>().input_blocked);

            h.press_mouse(MouseButton::Left);
            h.step_dt(1.0 / 60.0);
            assert!(!is_attacking_state(h.state()));
            h.release_mouse(MouseButton::Left);
            for _ in 0..18 {
                h.step_dt(1.0 / 60.0);
            }
            assert!(h.app.world().resource::<PlayerPerformance>().input_blocked);
            if fresh_attack {
                h.press_mouse(MouseButton::Left);
            }
            h.step_dt(1.0 / 60.0);
            assert!(!h.app.world().resource::<PlayerPerformance>().input_blocked);
            assert_eq!(
                is_attacking_state(h.state()),
                fresh_attack,
                "locked={locked}"
            );
        }
    }

    #[test]
    fn production_pointer_turn_is_committed_before_unlocked_attack_entry() {
        use bevy::input::mouse::MouseMotion;
        for serial in [false, true] {
            for airborne in [false, true] {
                let mut h = spawn_gameplay_harness_with_order(5, serial)
                    .expect("original p1 resources required");
                crate::plugins::camera_plugin::install_camera_input_for_test(&mut h.app);
                h.app.world_mut().spawn((
                    crate::plugins::camera_plugin::GameplayCamera,
                    crate::plugins::camera_plugin::ThirdPersonState::default(),
                ));
                h.app.world_mut().spawn((
                    bevy::window::PrimaryWindow,
                    bevy::window::CursorOptions {
                        grab_mode: bevy::window::CursorGrabMode::Locked,
                        ..default()
                    },
                ));
                if airborne {
                    h.press_key(KeyCode::Space);
                    h.steps(5);
                    h.release_key(KeyCode::Space);
                    h.step();
                    assert!(h.airborne());
                }
                let before = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                h.app
                    .world_mut()
                    .resource_mut::<Messages<MouseMotion>>()
                    .write(MouseMotion {
                        delta: Vec2::new(126.0, 0.0),
                    });
                h.press_mouse(MouseButton::Left);
                h.step();
                assert!(
                    is_attacking_state(h.state()),
                    "serial={serial} airborne={airborne}"
                );
                let after = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                assert!((after - before + std::f32::consts::FRAC_PI_2).abs() < 1e-5,
                    "entry must retain the full pointer turn: serial={serial} airborne={airborne} before={before} after={after}");
            }
        }
    }

    #[test]
    fn firearm_aim_shot_recovery_keeps_visible_motion_root_neutral() {
        use crate::components::fighter::VisualRootPin;
        use crate::plugins::character_plugin::{
            drive_fighter_animation_for_test, spawn_fighter_model_for_test, FighterDummy,
        };
        let mut h = spawn_gameplay_harness(3).expect("original p1 resources required");
        h.app
            .world_mut()
            .entity_mut(h.fighter)
            .insert(VisualRootPin::default());
        h.app
            .world_mut()
            .get_mut::<GunAimState>(h.fighter)
            .unwrap()
            .aiming = true;
        spawn_fighter_model_for_test(h.app.world_mut(), h.fighter);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let assets = crate::plugins::character_plugin::load_character_asset_for_test(&[root], "p1")
            .expect("original p1 assets");
        let common = &h.app.world().resource::<CommonAmb>().0;
        let aim = &assets.poses[&213];
        assert!(assets.motion_root_at(common, 213, aim.start as f32)[1].abs() > 12.0);

        for id in [212, 213, 206, 213, 207, 213, 208, 213, 145, 213] {
            for endpoint in [false, true] {
                let mut pose = h.app.world_mut().get_mut::<PoseRuntime>(h.fighter).unwrap();
                assets.apply_pose(&mut pose, id, true);
                if endpoint {
                    pose.frame = pose.end as f32;
                }
                h.app
                    .world_mut()
                    .get_mut::<SmState>(h.fighter)
                    .unwrap()
                    .current = if matches!(id, 206..=208) {
                    FighterState::Attack1
                } else {
                    FighterState::Name
                };
                drive_fighter_animation_for_test(h.app.world_mut());
                let world = h.app.world_mut();
                let mut query = world.query::<(&FighterDummy, &Transform)>();
                let (_, root) = query
                    .iter(world)
                    .find(|(dummy, _)| dummy.name == "d_base")
                    .unwrap();
                assert_eq!(root.translation, Vec3::ZERO, "pose={id} end={endpoint}");
            }
        }
    }

    #[test]
    fn production_air_pointer_turn_after_real_jump_in_serial_schedule() {
        use crate::components::fighter::VisualRootPin;
        use crate::plugins::camera_plugin::{
            install_camera_input_for_test, install_follow_camera_for_test,
        };
        use bevy::input::mouse::MouseMotion;
        for serial in [false, true] {
            let mut h = spawn_gameplay_harness_with_order(5, serial)
                .expect("original p1 resources required");
            install_camera_input_for_test(&mut h.app);
            install_follow_camera_for_test(&mut h.app);
            h.app
                .world_mut()
                .entity_mut(h.fighter)
                .insert((HeadTrackState::default(), VisualRootPin::default()));
            h.app.insert_resource(State::new(AppState::Playing));
            configure_lock_updates(&mut h.app);
            crate::plugins::character_plugin::spawn_fighter_model_for_test(
                h.app.world_mut(),
                h.fighter,
            );
            let actor_schedule = crate::plugins::fighter_frame::schedule(&h.app);
            h.app.add_systems(
                actor_schedule,
                crate::plugins::character_plugin::drive_fighter_animation_for_test
                    .before(harness_retire_entry_blend)
                    .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
            );
            h.app.world_mut().spawn((
                bevy::window::PrimaryWindow,
                bevy::window::CursorOptions {
                    grab_mode: bevy::window::CursorGrabMode::Locked,
                    ..default()
                },
            ));
            h.press_key(KeyCode::KeyW);
            h.steps(10);
            h.press_key(KeyCode::Space);
            h.steps(5);
            h.release_key(KeyCode::Space);
            h.step();
            assert!(h.airborne());
            for frame in 0..15 {
                h.app
                    .world_mut()
                    .resource_mut::<Messages<MouseMotion>>()
                    .write(MouseMotion {
                        delta: Vec2::new(1.4, 0.0),
                    });
                let before = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                h.step();
                let after = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                let air = &h.app.world().get::<FighterAir>(h.fighter).unwrap().air;
                assert!((after - before + 1.0_f32.to_radians()).abs() < 1e-5,
                    "serial={serial} frame={frame} state={:?} mode={:?} airborne={} before={before} after={after}",
                    h.state(), air.motion_mode, air.airborne);
                assert!(air.planar_x < 0.0, "W drive must follow the turned facing");
                let expected = Quat::from_rotation_y(after)
                    * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
                let mut models = h
                    .app
                    .world_mut()
                    .query::<(&crate::plugins::character_plugin::FighterModel, &Transform)>();
                let (_, transform) = models
                    .iter(h.app.world())
                    .find(|(model, _)| model.fighter == h.fighter)
                    .expect("visible fighter model");
                assert!(transform.rotation.angle_between(expected).abs() < 1e-3);
            }
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn attack_facing_schedule_with_network_and_contact_is_acyclic() {
        use bevy::ecs::schedule::Schedules;
        use bevy::state::app::AppExtStates;
        for serial in [false, true] {
            let mut app = App::new();
            app.add_plugins(bevy::state::app::StatesPlugin)
                .init_state::<AppState>();
            crate::plugins::configure_fighter_frame_sets(&mut app);
            if serial {
                crate::plugins::fighter_frame::install(&mut app);
            }
            app.add_plugins((
                crate::plugins::InputPlugin,
                crate::plugins::NetworkPlugin,
                crate::plugins::CharacterContactPlugin,
                crate::plugins::MovementPlugin,
                crate::plugins::AiPlugin,
                crate::plugins::ItemPlugin,
                CombatPlugin,
                crate::plugins::CharacterPlugin,
                crate::plugins::CameraPlugin,
            ));
            let mut update = app
                .world_mut()
                .resource_mut::<Schedules>()
                .remove(Update)
                .unwrap();
            update.initialize(app.world_mut()).expect(
                "attack facing must remain after movement and before camera/network publication",
            );
            app.world_mut().resource_mut::<Schedules>().insert(update);
            if serial {
                let mut actors = app
                    .world_mut()
                    .resource_mut::<Schedules>()
                    .remove(crate::plugins::fighter_frame::FighterUpdate)
                    .unwrap();
                actors
                    .initialize(app.world_mut())
                    .expect("serial actor schedule must be acyclic and conflict-free");
            }
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn fist_slide_uses_post_move_position_and_post_sample_blend_gate() {
        #[derive(Resource)]
        struct FacingBeforeMove {
            fighter: Entity,
            yaw: f32,
        }
        fn sample_hit_facing(
            active: Option<Res<super::super::fighter_frame::ActiveFighter>>,
            mut sample: ResMut<FacingBeforeMove>,
            fighters: Query<&MotionState>,
        ) {
            if !super::super::fighter_frame::accepts(active.as_deref(), sample.fighter) {
                return;
            }
            sample.yaw = fighters.get(sample.fighter).unwrap().facing;
        }
        for serial in [false, true] {
            for locally_driven in [true, false] {
                for fps in [30, 60, 120] {
                    for finish_blend in [false, true] {
                        for unlock in [false, true] {
                            if !locally_driven && unlock {
                                continue;
                            }
                            let (mut h, target, _) =
                                spawn_local_stock_hit_harness_with_order(11, serial);
                            let actor_schedule = super::super::fighter_frame::schedule(&h.app);
                            h.app.insert_resource(State::new(AppState::Playing));
                            configure_lock_updates(&mut h.app);
                            h.app.insert_resource(FacingBeforeMove {
                                fighter: h.fighter,
                                yaw: f32::NAN,
                            });
                            h.app.add_systems(
                                actor_schedule,
                                sample_hit_facing.in_set(crate::plugins::FighterFrameSet::HitJudge),
                            );
                            if !locally_driven {
                                h.app
                                    .world_mut()
                                    .entity_mut(h.fighter)
                                    .remove::<LocalFighter>()
                                    .insert(NpcFighter);
                                let mut intent =
                                    h.app.world_mut().get_mut::<AiIntent>(h.fighter).unwrap();
                                intent.lock_target = Some(target);
                                intent.attack_cooldown = f32::INFINITY;

                                h.app
                                    .world_mut()
                                    .get_mut::<FighterAir>(h.fighter)
                                    .unwrap()
                                    .ground_external_velocity = Vec3::new(12.0, 0.0, 0.0);
                            }
                            h.app.add_systems(
                                actor_schedule,
                                crate::plugins::character_plugin::drive_fighter_animation_for_test
                                    .before(harness_retire_entry_blend)
                                    .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
                            );
                            h.app
                                .world_mut()
                                .entity_mut(h.fighter)
                                .insert(HeadTrackState::default());
                            h.app
                                .world_mut()
                                .get_mut::<Transform>(target)
                                .unwrap()
                                .translation = Vec3::new(0.0, 0.0, 3.0);
                            let initial_facing = 0.2;
                            h.app
                                .world_mut()
                                .get_mut::<MotionState>(h.fighter)
                                .unwrap()
                                .facing = initial_facing;
                            let mut pose = PoseRuntime::idle();
                            h.app.world().resource::<CharacterAssetsSet>().0["p1"]
                                .apply_pose(&mut pose, 417, true);
                            let now = h
                                .app
                                .world()
                                .resource::<Time<bevy::time::Real>>()
                                .elapsed_secs_f64();
                            let frame = pose.frame;
                            h.app.world_mut().entity_mut(h.fighter).insert((
                                pose,
                                SmState {
                                    current: FighterState::Attack1,
                                    attack_stage: 1,
                                    locked: true,
                                    ..default()
                                },
                                EntryBlend {
                                    pose_id: 417,
                                    duration: if finish_blend { 0.001 } else { 0.12 },
                                    started_at_seconds: Some(now),
                                    to_frame: frame,
                                    kind: EntryBlendKind::Chained,
                                    ..default()
                                },
                                FighterAttackMotion {
                                    carry_x: if locally_driven { 12.0 } else { 0.0 },
                                    carry_banked: true,
                                    ..default()
                                },
                            ));
                            {
                                let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                                lock.system.lock_target = Some(target.to_bits());
                                lock.lock_entity = Some(target);
                            }
                            if unlock {
                                h.press_key(KeyCode::KeyQ);
                            }
                            let before = h.position();
                            let dt = 1.0 / fps as f32;
                            h.step_dt(dt);
                            let after = h.position();
                            let expected_dx =
                                12.0 * (if locally_driven { 1.0 - 9.0 * dt } else { 1.0 }) * dt;
                            assert!(
                                (h.app.world().resource::<FacingBeforeMove>().yaw - initial_facing)
                                    .abs()
                                    < 1e-5,
                                "HitJudge must keep the pre-mover facing, local={locally_driven}"
                            );
                            assert!(
                                (after.x - before.x - expected_dx).abs() < 1e-4,
                                "{fps} FPS: carry must move once in world X: {before:?} -> {after:?}"
                            );
                            let expected = if unlock || finish_blend {
                                initial_facing
                            } else {
                                let target_pos =
                                    h.app.world().get::<Transform>(target).unwrap().translation;
                                locked_attack_facing_yaw(
                                    after.to_array().into(),
                                    target_pos.to_array().into(),
                                )
                                .unwrap()
                            };
                            let facing =
                                h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                            assert!(
                                (facing - expected).abs() < 1e-5,
                                "{fps} FPS local={locally_driven} finish={finish_blend} unlock={unlock}: facing={facing}, expected={expected}, pos={after:?}"
                            );
                            let world = h.app.world_mut();
                            let mut models = world
                            .query::<(&crate::plugins::character_plugin::FighterModel, &Transform)>(
                            );
                            let model = models
                                .iter(world)
                                .find(|(model, _)| model.fighter == h.fighter)
                                .unwrap()
                                .1;
                            let forward = model.rotation * Vec3::NEG_Y;
                            assert!(
                                (forward.x.atan2(forward.z) - expected).abs() < 1e-5,
                                "the rendered model must use this tick's accepted facing"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn fist_slide_unlock_preserves_angle_before_root_motion() {
        for fps in [30, 60, 120] {
            for unlock in [false, true] {
                let (mut h, target, _) = spawn_local_stock_hit_harness(11);
                h.app.insert_resource(State::new(AppState::Playing));
                configure_lock_updates(&mut h.app);
                h.app
                    .world_mut()
                    .entity_mut(h.fighter)
                    .insert(HeadTrackState::default());
                h.app
                    .world_mut()
                    .get_mut::<Transform>(target)
                    .unwrap()
                    .translation = Vec3::new(0.0, 0.0, 3.0);
                let angle = 0.2;
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .facing = angle;
                let assets = h.app.world().resource::<CharacterAssetsSet>();
                let mut pose = PoseRuntime::idle();
                assets.0["p1"].apply_pose(&mut pose, 417, true);
                let frame = pose.frame;
                h.app.world_mut().entity_mut(h.fighter).insert((
                    pose,
                    SmState {
                        current: FighterState::Attack1,
                        attack_stage: 1,
                        locked: true,
                        ..default()
                    },
                    EntryBlend {
                        pose_id: 417,
                        duration: 0.12,
                        to_frame: frame,
                        kind: EntryBlendKind::Chained,
                        ..default()
                    },
                ));
                {
                    let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                    lock.system.lock_target = Some(target.to_bits());
                    lock.lock_entity = Some(target);
                }
                if unlock {
                    h.press_key(KeyCode::KeyQ);
                }
                h.step_dt(1.0 / fps as f32);
                let facing = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                let expected = if unlock { angle } else { 0.0 };
                assert!((facing - expected).abs() < 1e-5,
                    "417 chained fade at {fps} FPS, unlock={unlock}: facing={facing}, expected={expected}");
                assert_eq!(
                    h.app.world().resource::<LockOnController>().lock_entity,
                    if unlock { None } else { Some(target) }
                );
            }
        }
    }

    #[test]
    fn hazard_room_clients_wait_for_authority_instead_of_dying_locally() {
        use super::super::hazard_plugin::StageHazardRuntime;
        use bevy::ecs::system::RunSystemOnce;
        use meteor_scene::hazards::{Aabb, HazardVolume};
        for address in ["ws://127.0.0.1:9001", "udp://127.0.0.1:5230"] {
            let mut h = spawn_gameplay_harness(1).expect("original character assets");
            h.app
                .init_resource::<StageHazardRuntime>()
                .init_resource::<super::super::stage_plugin::StageScriptRuntime>()
                .add_message::<super::super::render_plugin::OfflineWeaponDeathDrop>();
            h.app.world_mut().resource_mut::<NetState>().status = NetStatus::InRoom;
            h.app.world_mut().resource_mut::<NetState>().address = address.into();
            h.app
                .world_mut()
                .resource_mut::<StageHazardRuntime>()
                .0
                .volumes
                .push(HazardVolume {
                    name: "hell".into(),
                    damage: 100000,
                    period: 0.1,
                    box_: Aabb {
                        min: [-5.0; 3],
                        max: [5.0; 3],
                    },
                });
            h.app
                .world_mut()
                .get_mut::<Health>(h.fighter)
                .unwrap()
                .current = 100.0;
            h.app
                .world_mut()
                .get_mut::<TargetHitClock>(h.fighter)
                .unwrap()
                .tick_spawn(10.0);
            for _ in 0..10 {
                h.app
                    .world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::from_secs_f32(0.2));
                h.app
                    .world_mut()
                    .run_system_once(stage_hazard_damage)
                    .unwrap();
                assert_eq!(
                    h.app.world().get::<Health>(h.fighter).unwrap().current,
                    100.0,
                    "{address}"
                );
                assert!(!h.app.world().get::<DeathRespawn>(h.fighter).unwrap().active);
            }
        }
    }

    #[test]
    fn hazard_production_damage_protection_death_and_cooldown() {
        use super::super::hazard_plugin::StageHazardRuntime;
        use bevy::ecs::system::RunSystemOnce;
        use meteor_scene::hazards::{Aabb, HazardVolume};
        let mut h = spawn_gameplay_harness(1).expect("original character assets");
        h.app
            .init_resource::<StageHazardRuntime>()
            .init_resource::<super::super::stage_plugin::StageScriptRuntime>()
            .add_message::<super::super::render_plugin::OfflineWeaponDeathDrop>();
        h.app
            .world_mut()
            .resource_mut::<StageHazardRuntime>()
            .0
            .volumes
            .push(HazardVolume {
                name: "DaBox".into(),
                damage: 30,
                period: 1.0,
                box_: Aabb {
                    min: [-5.0; 3],
                    max: [5.0; 3],
                },
            });
        h.app
            .world_mut()
            .get_mut::<Health>(h.fighter)
            .unwrap()
            .current = 100.0;
        h.app
            .world_mut()
            .run_system_once(stage_hazard_damage)
            .unwrap();
        assert_eq!(
            h.app.world().get::<Health>(h.fighter).unwrap().current,
            100.0,
            "spawn protection"
        );
        h.app
            .world_mut()
            .get_mut::<TargetHitClock>(h.fighter)
            .unwrap()
            .spawn_elapsed = 3.0;
        h.app
            .world_mut()
            .run_system_once(stage_hazard_damage)
            .unwrap();
        assert_eq!(
            h.app.world().get::<Health>(h.fighter).unwrap().current,
            100.0,
            "ordinary environment damage requires strictly more than three seconds since spawn"
        );
        h.app
            .world_mut()
            .get_mut::<TargetHitClock>(h.fighter)
            .unwrap()
            .tick_spawn(0.001);
        h.app
            .world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.05));
        for _ in 0..10 {
            h.app
                .world_mut()
                .run_system_once(stage_hazard_damage)
                .unwrap();
        }
        assert_eq!(
            h.app.world().get::<Health>(h.fighter).unwrap().current,
            70.0
        );
        assert_eq!(
            h.app.world().get::<PoseRuntime>(h.fighter).unwrap().pose_id,
            90
        );
        assert_eq!(
            h.app.world().get::<SmState>(h.fighter).unwrap().current,
            FighterState::BeHit
        );
        h.app
            .world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.016));
        h.app
            .world_mut()
            .run_system_once(stage_hazard_damage)
            .unwrap();
        assert_eq!(
            h.app.world().get::<Health>(h.fighter).unwrap().current,
            70.0,
            "cooldown"
        );
        h.app
            .world_mut()
            .resource_mut::<StageHazardRuntime>()
            .0
            .volumes[0]
            .damage = 100000;
        h.app
            .world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.05));
        for _ in 0..10 {
            h.app
                .world_mut()
                .run_system_once(stage_hazard_damage)
                .unwrap();
        }
        assert_eq!(h.app.world().get::<Health>(h.fighter).unwrap().current, 0.0);
        assert_eq!(
            h.app.world().get::<PoseRuntime>(h.fighter).unwrap().pose_id,
            117
        );
        assert_eq!(
            h.app.world().get::<SmState>(h.fighter).unwrap().current,
            FighterState::Dead
        );
        assert!(h.app.world().get::<DeathRespawn>(h.fighter).unwrap().active);
        assert_eq!(
            h.app
                .world()
                .get::<MotionState>(h.fighter)
                .unwrap()
                .velocity,
            Vec3::ZERO
        );
        h.app
            .world_mut()
            .run_system_once(stage_hazard_damage)
            .unwrap();
        let drops = h
            .app
            .world()
            .resource::<Messages<super::super::render_plugin::OfflineWeaponDeathDrop>>();
        assert_eq!(drops.len(), 1, "dead actors must not drop twice");
    }

    fn spawn_npc_combat_harness(weapon: u8, attacks: [f32; 3]) -> (GameplayHarness, Entity) {
        spawn_npc_combat_harness_with_order(weapon, attacks, false)
    }

    fn spawn_npc_combat_harness_with_order(
        weapon: u8,
        attacks: [f32; 3],
        serial: bool,
    ) -> (GameplayHarness, Entity) {
        let mut harness = spawn_gameplay_harness_with_order(weapon, serial)
            .expect("original NPC combat resources");
        harness
            .app
            .world_mut()
            .entity_mut(harness.fighter)
            .remove::<LocalFighter>()
            .insert((
                NpcFighter,
                crate::plugins::ai_plugin::NpcPatrol::default(),
                NpcPerformance::default(),
                NpcCombatProfile {
                    attack1: attacks[0],
                    attack2: attacks[1],
                    attack3: attacks[2],
                    ..default()
                },
            ));
        let target = harness
            .app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 1,
                    player_slot: 0,
                    weapon: 6,
                    weapon_slots: [6, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health::default(),
                Transform::from_xyz(
                    0.0,
                    0.0,
                    if meteor_ai::combat::weapon_tactics(weapon).ranged {
                        16.0
                    } else {
                        3.0
                    },
                ),
            ))
            .id();
        let actor_schedule = super::super::fighter_frame::schedule(&harness.app);
        harness.app.add_systems(
            actor_schedule,
            crate::plugins::ai_plugin::drive_npc_ai
                .in_set(crate::plugins::FighterFrameSet::Movement),
        );
        harness.app.add_systems(
            actor_schedule,
            npc_lock_on_tick
                .in_set(crate::plugins::FighterFrameSet::AttackFacing)
                .before(local_attack_lock_facing),
        );
        harness.app.add_systems(
            actor_schedule,
            crate::plugins::ai_plugin::move_npc_fighters
                .in_set(crate::plugins::FighterFrameSet::Locomotion)
                .after(crate::plugins::movement_plugin::move_local_fighter),
        );
        (harness, target)
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn serial_locked_forward_push_keeps_target_ahead_at_each_frame_rate() {
        use crate::plugins::fighter_frame::FighterUpdate;
        let mut distances = Vec::new();
        for fps in [30, 60, 120] {
            let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(11, true);
            h.app
                .insert_resource(State::new(AppState::Playing))
                .insert_resource(NetState::default());
            h.app.add_plugins(crate::plugins::CharacterContactPlugin);
            configure_lock_updates(&mut h.app);
            h.app.add_systems(
                FighterUpdate,
                crate::plugins::character_plugin::drive_fighter_animation_for_test
                    .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
            );
            h.app
                .world_mut()
                .entity_mut(h.fighter)
                .insert(HeadTrackState::default());
            h.app.world_mut().entity_mut(target).insert((
                NpcFighter,
                AiIntent::default(),
                crate::plugins::ai_plugin::NpcPatrol::default(),
                NpcPerformance {
                    pause_remaining: 100.0,
                    ..default()
                },
            ));
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = Vec3::new(-0.6, 0.0, 0.0);
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = 0.0;
            let target_before = h.app.world().get::<Transform>(target).unwrap().translation;
            {
                let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                lock.system.lock_target = Some(target.to_bits());
                lock.lock_entity = Some(target);
            }
            h.press_key(KeyCode::KeyW);
            for _ in 0..fps * 3 {
                h.step_dt(1.0 / fps as f32);
            }
            let target_after = h.app.world().get::<Transform>(target).unwrap().translation;
            let offset = target_after - h.position();
            let yaw = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
            assert!(
                offset.length() < 2.3,
                "locked W ran away from the target at {fps} FPS: {offset:?}"
            );
            assert!(
                attack_forward_from_facing(yaw).dot(offset.normalize()) > 0.98,
                "contact commit must not reverse lock facing at {fps} FPS"
            );
            let distance = (target_after - target_before).length();
            assert!(
                (5.5..7.3).contains(&distance),
                "three-second push speed at {fps} FPS: {distance}"
            );
            distances.push(distance);
        }
        let range = distances.iter().copied().fold(f32::NEG_INFINITY, f32::max)
            - distances.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(
            range < 0.3,
            "push distance must not grow with FPS: {distances:?}"
        );
    }

    #[test]
    fn serial_render_only_frames_preserve_fist_command_edges_without_moving() {
        let mut h = spawn_gameplay_harness_with_order(11, true).expect("original fist resources");
        h.app.insert_resource(crate::plugins::FrameClock {
            sim_dt: 0.0,
            wall_dt: 1.0 / 120.0,
            wall_step: 0.0,
            ..default()
        });
        let before = h.position();
        h.press_key(KeyCode::KeyS);
        h.step_dt(1.0 / 120.0);
        h.release_key(KeyCode::KeyS);
        h.press_key(KeyCode::KeyW);
        h.step_dt(1.0 / 120.0);
        h.release_key(KeyCode::KeyW);
        h.press_mouse(MouseButton::Left);
        h.step_dt(1.0 / 120.0);
        h.release_mouse(MouseButton::Left);
        assert_eq!(
            h.pose_id(),
            417,
            "render-only event edges must reach the local controller"
        );
        assert!(
            (h.position() - before).length() < 1e-6,
            "zero simulation step must not move"
        );
        let frame = h.app.world().get::<PoseRuntime>(h.fighter).unwrap().frame;
        h.step_dt(1.0 / 120.0);
        assert_eq!(h.pose_id(), 417);
        assert_eq!(
            h.app.world().get::<PoseRuntime>(h.fighter).unwrap().frame,
            frame
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn serial_actor_frames_expose_prior_movement_and_interrupt_before_sampling() {
        use crate::plugins::fighter_frame::{ActiveFighter, FighterUpdate};
        #[derive(Resource, Default)]
        struct SeenPositions(Vec<(Entity, Vec<(Entity, Vec3)>)>);
        fn observe(
            active: Res<ActiveFighter>,
            actors: Query<(Entity, &Transform), With<Fighter>>,
            mut seen: ResMut<SeenPositions>,
        ) {
            seen.0.push((
                active.0.unwrap(),
                actors.iter().map(|(e, t)| (e, t.translation)).collect(),
            ));
        }
        for fps in [30, 60, 120] {
            for attacker_first in [true, false] {
                let dt = 1.0 / fps as f32;
                let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(7, true);
                let source = h.fighter;
                h.app
                    .insert_resource(NetState::default())
                    .init_resource::<SeenPositions>();
                for entity in [source, target] {
                    h.app
                        .world_mut()
                        .entity_mut(entity)
                        .insert(crate::components::fighter::VisualRootPin::default());
                }
                if !attacker_first {
                    let fighter = h.app.world().get::<Fighter>(source).unwrap().clone();
                    h.app
                        .world_mut()
                        .entity_mut(source)
                        .remove::<Fighter>()
                        .insert(fighter);
                }
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(source)
                    .unwrap()
                    .facing = 0.0;
                h.app
                    .world_mut()
                    .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                    .yaw = 0.0;
                h.app
                    .world_mut()
                    .get_mut::<Transform>(target)
                    .unwrap()
                    .translation = Vec3::new(0.0, 0.0, 4.0);
                crate::plugins::character_plugin::drive_fighter_animation_for_test(
                    h.app.world_mut(),
                );
                h.app.add_systems(
                    Update,
                    tick_target_hit_clocks.in_set(crate::plugins::FighterFrameSet::PoseDecision),
                );
                h.app.add_systems(
                    FighterUpdate,
                    crate::plugins::character_plugin::drive_fighter_animation_for_test
                        .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
                );
                h.app.add_systems(
                    FighterUpdate,
                    (observe, attack_windows)
                        .chain()
                        .in_set(crate::plugins::FighterFrameSet::HitJudge),
                );
                h.app.add_systems(
                    FighterUpdate,
                    hit_reaction_root_motion
                        .after(crate::plugins::movement_plugin::attack_root_motion)
                        .in_set(crate::plugins::FighterFrameSet::RootMotion),
                );
                h.press_key(KeyCode::KeyW);
                h.press_mouse(MouseButton::Left);
                let mut hit = false;
                let mut moved = false;
                for _ in 0..fps {
                    h.app.world_mut().resource_mut::<SeenPositions>().0.clear();
                    let before = h.position();
                    let clock_before = h
                        .app
                        .world()
                        .get::<TargetHitClock>(target)
                        .unwrap()
                        .spawn_elapsed;
                    h.step_dt(dt);
                    h.release_key(KeyCode::KeyW);
                    h.release_mouse(MouseButton::Left);
                    let after = h.position();
                    moved |= (after - before).length() > 1e-4;
                    let seen = &h.app.world().resource::<SeenPositions>().0;
                    assert_eq!(seen.len(), 2, "one controller tick per registered actor");
                    assert_eq!(seen[0].0, if attacker_first { source } else { target });
                    let target_view = &seen.iter().find(|(e, _)| *e == target).unwrap().1;
                    let observed_source = target_view.iter().find(|(e, _)| *e == source).unwrap().1;
                    assert!(
                        (observed_source - if attacker_first { after } else { before }).length()
                            < 1e-5,
                        "later HitJudge must observe earlier RootMotion, {fps} FPS order={attacker_first}"
                    );
                    let clock_after = h
                        .app
                        .world()
                        .get::<TargetHitClock>(target)
                        .unwrap()
                        .spawn_elapsed;
                    assert!(
                        (clock_after - clock_before - dt).abs() < 1e-5,
                        "global clock advanced twice"
                    );
                    if h.app
                        .world()
                        .get::<FighterHitReaction>(target)
                        .unwrap()
                        .current
                        .is_some()
                    {
                        let blend = h.app.world().get::<EntryBlend>(target).unwrap();
                        assert_eq!(
                            h.app.world().get::<PoseRuntime>(target).unwrap().pose_id,
                            100
                        );
                        assert!(blend.duration > 0.0);
                        assert_eq!(
                            blend.started_at_seconds.is_some(),
                            attacker_first,
                            "first state3 update follows registration order, not a global sample barrier"
                        );
                        hit = true;
                        break;
                    }
                }
                assert!(
                    hit && moved,
                    "real spear input, assets and hit geometry at {fps} FPS order={attacker_first}"
                );
            }
        }
    }

    #[test]
    fn production_npc_combat_retries_into_real_next_pose_windows() {
        let (mut h, _) = spawn_npc_combat_harness(6, [100.0, 0.0, 0.0]);
        let mut poses = std::collections::BTreeSet::new();
        let mut max_stage = 0;
        for _ in 0..120 {
            h.step();
            poses.insert(h.pose_id());
            max_stage = max_stage.max(
                h.app
                    .world()
                    .get::<SmState>(h.fighter)
                    .unwrap()
                    .attack_stage,
            );
        }
        assert!(
            poses.contains(&263),
            "AI must actually execute sword second attack: {poses:?}"
        );
        assert!(
            max_stage >= 2,
            "a boolean request alone does not prove a combo"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn production_stock_npc_melee_geometry_emits_identity_claim_without_local_damage() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut h, target) = spawn_npc_combat_harness(6, [100.0, 0.0, 0.0]);
        let (tx, take_hits) = crate::plugins::network_plugin::stock_hit_test_channel();
        h.app
            .insert_resource(tx)
            .init_resource::<CombatResourceData>()
            .init_resource::<crate::plugins::audio_plugin::SfxCache>()
            .init_resource::<Assets<AudioSource>>()
            .init_resource::<crate::plugins::effect_plugin::EfCache>()
            .init_resource::<crate::plugins::item_plugin::PropHitGate>()
            .init_resource::<crate::plugins::stage_plugin::StageScriptRuntime>()
            .init_resource::<PendingRangedHits>()
            .add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>()
            .add_message::<crate::plugins::render_plugin::OfflineWeaponDeathDrop>();
        h.app.world_mut().entity_mut(target).insert((
            Qi::default(),
            PoseRuntime::idle(),
            EntryBlend::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: std::f32::consts::PI,
            },
            SmState::default(),
            CombatState::default(),
            DeathRespawn::default(),
            FighterAir::default(),
            FighterDodge::default(),
            FighterAttackMotion::default(),
            crate::components::fighter::TargetHitClock {
                elapsed: f32::INFINITY,
                spawn_elapsed: 3.0,
            },
        ));
        {
            let mut net = h.app.world_mut().resource_mut::<NetState>();
            net.status = NetStatus::InRoom;
            net.address = "udp-host://0.0.0.0:5230".into();
            net.player_id = "stock-1".into();
            net.mode = "leader".into();
            net.round_active = true;
            net.stock_npc_actors.insert(h.fighter, 2);
            net.stock_npc_ready.insert(h.fighter);
        }
        for entity in [h.fighter, target] {
            crate::plugins::character_plugin::spawn_fighter_model_for_test(
                h.app.world_mut(),
                entity,
            );
        }
        let hp = h.app.world().get::<Health>(target).unwrap().current;
        let mut claims = Vec::new();
        for _ in 0..150 {
            h.step();
            h.app
                .world_mut()
                .get_mut::<crate::components::fighter::TargetHitClock>(target)
                .unwrap()
                .tick(HARNESS_DT);
            crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
            h.app
                .world_mut()
                .run_system_once(attack_windows)
                .expect("production melee hit judge");
            claims.extend(take_hits());
        }
        assert!(
            !claims.is_empty(),
            "real sword POS/AMB/weapon geometry must produce NPC -> host requests"
        );
        assert!(claims
            .iter()
            .all(|(source, hit)| *source == Some(h.fighter) && hit.target_actor_id == 1));
        assert_eq!(
            h.app.world().get::<Health>(target).unwrap().current,
            hp,
            "must wait for host vitals, not predict HP locally"
        );
        assert!(
            h.app
                .world()
                .get::<FighterHitReaction>(target)
                .unwrap()
                .current
                .is_none(),
            "must wait for host BeHit"
        );
        assert_eq!(
            h.app.world().resource::<LockOnController>().lock_entity,
            None,
            "a hosted NPC attack must not write the local player's global Lock"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_local_stock_hit_harness(
        weapon: u8,
    ) -> (
        GameplayHarness,
        Entity,
        impl Fn() -> Vec<(
            Option<Entity>,
            meteor_network::stock_protocol::StockHitRequest,
        )>,
    ) {
        spawn_local_stock_hit_harness_with_order(weapon, false)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_local_stock_hit_harness_with_order(
        weapon: u8,
        serial: bool,
    ) -> (
        GameplayHarness,
        Entity,
        impl Fn() -> Vec<(
            Option<Entity>,
            meteor_network::stock_protocol::StockHitRequest,
        )>,
    ) {
        let (mut h, target) =
            spawn_npc_combat_harness_with_order(weapon, [100.0, 0.0, 0.0], serial);
        h.app
            .world_mut()
            .entity_mut(h.fighter)
            .remove::<NpcFighter>()
            .insert(LocalFighter);
        h.app
            .world_mut()
            .entity_mut(target)
            .remove::<LocalFighter>();

        let (tx, take_hits) = crate::plugins::network_plugin::stock_hit_test_channel();
        h.app
            .insert_resource(tx)
            .init_resource::<CombatResourceData>()
            .init_resource::<crate::plugins::audio_plugin::SfxCache>()
            .init_resource::<Assets<AudioSource>>()
            .init_resource::<crate::plugins::effect_plugin::EfCache>()
            .init_resource::<crate::plugins::item_plugin::PropHitGate>()
            .init_resource::<crate::plugins::stage_plugin::StageScriptRuntime>()
            .init_resource::<PendingRangedHits>()
            .add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>()
            .add_message::<crate::plugins::render_plugin::OfflineWeaponDeathDrop>();
        h.app.world_mut().entity_mut(target).insert((
            Qi::default(),
            PoseRuntime::idle(),
            EntryBlend::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: std::f32::consts::PI,
            },
            SmState::default(),
            CombatState::default(),
            DeathRespawn::default(),
            FighterAir::default(),
            FighterDodge::default(),
            FighterAttackMotion::default(),
            crate::components::fighter::TargetHitClock {
                elapsed: f32::INFINITY,
                spawn_elapsed: 3.0,
            },
        ));
        {
            let mut net = h.app.world_mut().resource_mut::<NetState>();
            net.status = NetStatus::InRoom;
            net.address = "udp-host://0.0.0.0:5230".into();
            net.player_id = "stock-1".into();
            net.mode = "leader".into();
            net.round_active = true;
            net.stock_npc_actors.insert(target, 2);
            net.stock_npc_ready.insert(target);
        }
        for entity in [h.fighter, target] {
            crate::plugins::character_plugin::spawn_fighter_model_for_test(
                h.app.world_mut(),
                entity,
            );
        }
        (h, target, take_hits)
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn production_local_melee_hit_on_stock_npc_acquires_actual_target_lock() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut h, target, take_hits) = spawn_local_stock_hit_harness(6);

        h.press_mouse(MouseButton::Left);
        let mut claims = Vec::new();
        for tick in 0..150 {
            h.step();
            if tick == 0 {
                h.release_mouse(MouseButton::Left);
            }
            h.app
                .world_mut()
                .get_mut::<crate::components::fighter::TargetHitClock>(target)
                .unwrap()
                .tick(HARNESS_DT);
            crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
            h.app
                .world_mut()
                .run_system_once(attack_windows)
                .expect("production melee hit judge");
            let batch = take_hits();
            if !batch.is_empty() {
                claims.extend(batch);
                break;
            }
        }
        assert!(
            !claims.is_empty(),
            "real sword POS/AMB/weapon geometry must produce player -> NPC requests"
        );
        assert!(claims
            .iter()
            .all(|(source, hit)| source.is_none() && hit.target_actor_id == 2));
        let lock = h.app.world().resource::<LockOnController>();
        assert_eq!(lock.system.lock_target, Some(target.to_bits()));
        assert_eq!(lock.lock_entity, Some(target));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn production_dart_camera_aim_reaches_target_after_release() {
        use bevy::ecs::system::RunSystemOnce;
        for fps in [30, 60, 120] {
            let (mut h, target, take_hits) = spawn_local_stock_hit_harness(1);
            h.app
                .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>();
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = Vec3::ZERO;
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = Vec3::new(0.0, 0.0, 12.0);

            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = std::f32::consts::FRAC_PI_2;
            let aim = Vec3::new(0.0, -0.3, 1.0).normalize();
            let camera = h
                .app
                .world_mut()
                .spawn((
                    GameplayCamera,
                    GlobalTransform::from(Transform::default().looking_to(aim, Vec3::Y)),
                ))
                .id();
            h.app
                .world_mut()
                .resource_scope(|world, assets: Mut<CharacterAssetsSet>| {
                    let character = &assets.0["p1"];
                    let mut pose = world.get_mut::<PoseRuntime>(h.fighter).unwrap();
                    character.apply_pose(&mut pose, 200, true);
                    pose.frame = character.poses[&200].attacks[0].start as f32;
                });
            h.app
                .world_mut()
                .get_mut::<SmState>(h.fighter)
                .unwrap()
                .current = FighterState::Attack1;
            h.app
                .world_mut()
                .get_mut::<CombatState>(h.fighter)
                .unwrap()
                .state = FighterState::Attack1;
            h.app.world_mut().run_system_once(attack_windows).unwrap();
            let (projectile, velocity) = {
                let world = h.app.world_mut();
                let mut query = world.query::<(Entity, &Projectile)>();
                let (entity, shot) = query.single(world).expect("one dart at authored release");
                assert!((shot.source.attack_forward - aim).length() < 1e-5);
                assert!((shot.velocity - (aim + Vec3::Y * 0.13) * DART_SPEED).length() < 1e-5);
                (entity, shot.velocity)
            };

            h.app
                .world_mut()
                .get_mut::<PoseRuntime>(h.fighter)
                .unwrap()
                .frame = 0.0;
            *h.app
                .world_mut()
                .get_mut::<GlobalTransform>(camera)
                .unwrap() = GlobalTransform::default();
            let mut claims = Vec::new();
            for _ in 0..fps {
                h.app
                    .world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::from_secs_f32(1.0 / fps as f32));
                h.app
                    .world_mut()
                    .run_system_once(crate::plugins::ranged_plugin::update_projectiles)
                    .unwrap();
                h.app.world_mut().run_system_once(attack_windows).unwrap();
                claims.extend(take_hits());
                if h.app.world().get::<Projectile>(projectile).is_none() {
                    break;
                }
                assert_eq!(
                    h.app
                        .world()
                        .get::<Projectile>(projectile)
                        .unwrap()
                        .velocity
                        .z,
                    velocity.z
                );
            }
            assert_eq!(
                claims.len(),
                1,
                "{fps} FPS dart must reach and consume on the first accepted target"
            );
            assert_eq!(claims[0].1.target_actor_id, 2);
            assert!(h.app.world().get::<Projectile>(projectile).is_none());
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn production_ranged_hit_applies_authored_status_only_after_accepted_unguarded_hit() {
        use crate::components::fighter::FighterStatusEffects;
        use bevy::ecs::system::RunSystemOnce;
        for (pose_id, weapon, guarded, expected) in [
            (210, 1, false, Some(11)),
            (210, 1, true, None),
            (200, 1, false, None),
            (224, 2, false, Some(13)),
        ] {
            let (mut h, target, _) = spawn_local_stock_hit_harness(weapon);
            *h.app.world_mut().resource_mut::<NetState>() = NetState::default();
            h.app
                .world_mut()
                .run_system_once(load_combat_resource_data)
                .unwrap();
            let attack = h.app.world().resource::<CharacterAssetsSet>().0["p1"].poses[&pose_id]
                .attacks[0]
                .clone();
            if guarded {
                assert_eq!(
                    attack.attack_type, 0,
                    "guard case must use the original non-breaking attack"
                );
            }
            if guarded {
                h.app
                    .world_mut()
                    .get_mut::<CombatState>(target)
                    .unwrap()
                    .state = FighterState::Guard;
            }
            let point = h.app.world().get::<Transform>(target).unwrap().translation;
            let projectile = h.app.world_mut().spawn_empty().id();
            h.app
                .world_mut()
                .resource_mut::<PendingRangedHits>()
                .0
                .push(crate::components::projectile::RangedSegmentHit {
                    projectile,
                    source: RangedHitSource {
                        owner: h.fighter,
                        weapon,
                        weapon_stats: None,
                        pose_id,
                        pose_frame: attack.start as f32,
                        pose_start: attack.start,
                        pose_instance_id: 1,
                        attack_index: 0,
                        attack,
                        player_slot: 0,
                        combat_team: 0,
                        script_character_id: 0,
                        script_team: None,
                        attack_origin: Vec3::ZERO,
                        attack_forward: Vec3::Z,
                    },
                    start: point + Vec3::new(0.0, 2.0, -2.0),
                    end: point + Vec3::new(0.0, 2.0, 2.0),
                    hit_scale: 1.0,
                    stop_after_first: true,
                    despawn_after: true,
                });
            h.app.world_mut().run_system_once(attack_windows).unwrap();
            let status = h.app.world().get::<FighterStatusEffects>(target).unwrap();
            assert_eq!(
                status
                    .effects
                    .active
                    .first()
                    .map(|effect| effect.definition.id),
                expected,
                "pose {pose_id}, guarded={guarded}"
            );
            let before = h.app.world().get::<Health>(target).unwrap().current;
            h.app
                .world_mut()
                .run_system_once(super::super::status_plugin::tick_status_effects)
                .unwrap();
            assert_eq!(
                h.app.world().get::<Health>(target).unwrap().current,
                before - if expected == Some(11) { 10.0 } else { 0.0 }
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn production_dagger_sword_and_hammer_melee_reach_stock_npc() {
        use bevy::ecs::system::RunSystemOnce;

        for weapon in [5, 6, 9] {
            let (mut h, target, take_hits) = spawn_local_stock_hit_harness(weapon);
            h.press_mouse(MouseButton::Left);
            let mut claims = Vec::new();
            for tick in 0..180 {
                h.step();
                if tick == 0 {
                    h.release_mouse(MouseButton::Left);
                }
                h.app
                    .world_mut()
                    .get_mut::<crate::components::fighter::TargetHitClock>(target)
                    .unwrap()
                    .tick(HARNESS_DT);
                crate::plugins::character_plugin::drive_fighter_animation_for_test(
                    h.app.world_mut(),
                );
                h.app
                    .world_mut()
                    .run_system_once(attack_windows)
                    .expect("production melee hit judge");
                claims.extend(take_hits());
                if !claims.is_empty() {
                    break;
                }
            }
            assert!(
                claims.iter().any(|(source, hit)| {
                    source.is_none() && hit.target_actor_id == 2
                }),
                "weapon {weapon} must reach the hosted stock NPC through its real POS/AMB/model geometry"
            );
        }
    }

    fn install_poison_test_status(h: &mut GameplayHarness) -> meteor_asset::res::HitStatusEffect {
        let table = parse_meteor_res(include_bytes!("../../../../../origin_res/Meteor.res"))
            .unwrap()
            .combat_table();
        let poison = table.status_effect(14).unwrap();
        h.app.insert_resource(CombatResourceData(table));
        h.app.insert_resource(State::new(AppState::Playing));
        configure_periodic_status_updates(&mut h.app);
        if super::super::fighter_frame::installed(&h.app) {
            h.app.add_systems(
                Update,
                harness_retire_entry_blend.after(super::super::fighter_frame::SerialFighterSet),
            );
        }
        let schedule = super::super::fighter_frame::schedule(&h.app);
        h.app.add_systems(
            schedule,
            hit_reaction_root_motion.in_set(crate::plugins::FighterFrameSet::RootMotion),
        );
        poison
    }

    #[test]
    fn production_periodic_poison_interrupts_attack_once_and_recovers() {
        use crate::components::fighter::FighterStatusEffects;
        for serial in [false, true] {
            let mut h = spawn_gameplay_harness_with_order(6, serial).unwrap();
            let poison = install_poison_test_status(&mut h);
            h.app
                .world_mut()
                .get_mut::<FighterStatusEffects>(h.fighter)
                .unwrap()
                .apply(poison);
            h.step();
            h.press_mouse(MouseButton::Left);
            h.step();
            h.release_mouse(MouseButton::Left);
            assert!(is_attacking_state(h.state()));
            let hp = h.app.world().get::<Health>(h.fighter).unwrap().current;
            let qi = h.app.world().get::<Qi>(h.fighter).unwrap().current;
            h.app
                .world_mut()
                .get_mut::<FighterStatusEffects>(h.fighter)
                .unwrap()
                .clock = 5.0;
            h.step();
            assert_eq!(h.pose_id(), 90, "serial={serial}");
            assert_eq!(h.state(), FighterState::BeHit);
            assert_eq!(
                h.app.world().get::<Health>(h.fighter).unwrap().current,
                hp - 10.
            );
            assert_eq!(h.app.world().get::<Qi>(h.fighter).unwrap().current, qi);
            let instance = h
                .app
                .world()
                .get::<PoseRuntime>(h.fighter)
                .unwrap()
                .pose_instance_id;
            h.step();
            assert_eq!(
                h.app
                    .world()
                    .get::<PoseRuntime>(h.fighter)
                    .unwrap()
                    .pose_instance_id,
                instance
            );
            h.steps(60);
            assert_eq!(
                h.state(),
                FighterState::Name,
                "serial={serial} pose={:?} air={:?} reaction={:?}",
                h.app.world().get::<PoseRuntime>(h.fighter),
                h.airborne(),
                h.app.world().get::<FighterHitReaction>(h.fighter)
            );
            assert_eq!(
                h.app.world().get::<Health>(h.fighter).unwrap().current,
                hp - 10.
            );
        }
    }

    #[test]
    fn production_periodic_poison_air_attack_recovers() {
        use crate::components::fighter::FighterStatusEffects;
        for weapon in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] {
            for (delay, height) in [0, 5, 15, 30]
                .into_iter()
                .flat_map(|delay| [0.0, 1000.0].into_iter().map(move |height| (delay, height)))
            {
                let mut h = spawn_gameplay_harness_with_order(weapon, true).unwrap();
                let poison = install_poison_test_status(&mut h);
                h.app
                    .world_mut()
                    .get_mut::<FighterStatusEffects>(h.fighter)
                    .unwrap()
                    .apply(poison);
                h.step();
                h.press_key(KeyCode::Space);
                h.step();
                h.release_key(KeyCode::Space);
                h.steps(5);
                h.press_mouse(MouseButton::Left);
                h.step();
                h.release_mouse(MouseButton::Left);
                h.steps(delay);
                let source = h.pose_id();

                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation
                    .y += height;
                h.app
                    .world_mut()
                    .get_mut::<FighterStatusEffects>(h.fighter)
                    .unwrap()
                    .clock = 5.;
                h.step();
                assert_eq!(h.pose_id(), 90);
                h.steps(120);
                assert_eq!(h.airborne(), height > 0.0, "weapon={weapon} delay={delay}");
                if height > 0.0 {
                    assert!(h.position().y < height, "must continue descending");
                }
                assert_eq!(
                    h.state(),
                    FighterState::Name,
                    "weapon={weapon} delay={delay} source={source} pos={:?} air={:?} pose={:?}",
                    h.position(),
                    h.airborne(),
                    h.app.world().get::<PoseRuntime>(h.fighter)
                );
            }
        }
    }

    #[test]
    fn production_periodic_poison_interrupts_npc_air_down_fake_and_guard() {
        use crate::components::fighter::FighterStatusEffects;
        use bevy::ecs::system::RunSystemOnce;
        for npc in [false, true] {
            for state in [
                FighterState::Jump,
                FighterState::Down,
                FighterState::Fake,
                FighterState::Guard,
            ] {
                let mut h = if npc {
                    spawn_npc_combat_harness(6, [0.; 3]).0
                } else {
                    spawn_gameplay_harness(6).unwrap()
                };
                let poison = install_poison_test_status(&mut h);
                h.app
                    .world_mut()
                    .get_mut::<FighterStatusEffects>(h.fighter)
                    .unwrap()
                    .apply(poison);
                h.step();
                h.app
                    .world_mut()
                    .get_mut::<SmState>(h.fighter)
                    .unwrap()
                    .current = state;
                h.app
                    .world_mut()
                    .get_mut::<FighterAir>(h.fighter)
                    .unwrap()
                    .air
                    .airborne = state == FighterState::Jump;
                h.app
                    .world_mut()
                    .get_mut::<FighterStatusEffects>(h.fighter)
                    .unwrap()
                    .clock = 5.;
                let hp = h.app.world().get::<Health>(h.fighter).unwrap().current;
                h.app
                    .world_mut()
                    .run_system_once(super::super::status_plugin::tick_status_effects)
                    .unwrap();
                h.app
                    .world_mut()
                    .run_system_once(apply_periodic_status_hits)
                    .unwrap();
                assert_eq!(h.pose_id(), 90, "npc={npc} state={state:?}");
                assert_eq!(h.state(), FighterState::BeHit);
                assert_eq!(
                    h.app.world().get::<Health>(h.fighter).unwrap().current,
                    hp - 10.
                );
                assert_eq!(
                    h.app
                        .world()
                        .get::<FighterHitReaction>(h.fighter)
                        .unwrap()
                        .airborne,
                    state == FighterState::Jump
                );
            }
        }
    }

    #[test]
    fn production_periodic_poison_does_not_revive_dead_or_predict_online_hits() {
        use crate::components::fighter::FighterStatusEffects;
        use bevy::ecs::system::RunSystemOnce;
        for online in [false, true] {
            let mut h = spawn_gameplay_harness(6).unwrap();
            let poison = install_poison_test_status(&mut h);
            h.app
                .world_mut()
                .get_mut::<FighterStatusEffects>(h.fighter)
                .unwrap()
                .apply(poison);
            h.step();
            h.app
                .world_mut()
                .get_mut::<Health>(h.fighter)
                .unwrap()
                .current = 5.;
            h.app
                .world_mut()
                .get_mut::<FighterStatusEffects>(h.fighter)
                .unwrap()
                .clock = 5.;
            if online {
                h.app.world_mut().resource_mut::<NetState>().status = NetStatus::InRoom;
            }
            let before = h.pose_id();
            h.app
                .world_mut()
                .run_system_once(super::super::status_plugin::tick_status_effects)
                .unwrap();
            h.app
                .world_mut()
                .run_system_once(apply_periodic_status_hits)
                .unwrap();
            if online {
                assert_eq!(h.app.world().get::<Health>(h.fighter).unwrap().current, 5.);
                assert_eq!(h.pose_id(), before);
            } else {
                assert_eq!(h.app.world().get::<Health>(h.fighter).unwrap().current, 0.);
                assert_eq!(h.state(), FighterState::Dead);
                assert_ne!(h.pose_id(), 90);
            }
        }
    }

    #[test]
    fn production_periodic_poison_item_uses_same_reaction_without_double_damage() {
        let mut h = spawn_gameplay_harness(6).unwrap();
        install_poison_test_status(&mut h);
        h.app
            .add_message::<super::super::render_plugin::OfflineWeaponDeathDrop>();
        h.app.add_systems(
            Update,
            super::super::item_plugin::tick_item_buffs
                .before(super::super::fighter_frame::SerialFighterSet),
        );
        h.press_mouse(MouseButton::Left);
        h.step();
        h.release_mouse(MouseButton::Left);
        let hp = h.app.world().get::<Health>(h.fighter).unwrap().current;
        {
            let mut buffs = h
                .app
                .world_mut()
                .resource_mut::<super::super::item_plugin::ItemBuffs>();
            buffs.poison_total = 30.;
            buffs.poison_tick_every = 5.;
            buffs.poison_tick = 0.;
            buffs.poison_per_tick = 10;
        }
        h.step();
        assert_eq!(h.pose_id(), 90);
        assert_eq!(
            h.app.world().get::<Health>(h.fighter).unwrap().current,
            hp - 10.
        );
        h.step();
        assert_eq!(
            h.app.world().get::<Health>(h.fighter).unwrap().current,
            hp - 10.
        );
    }

    #[test]
    fn production_status_slow_changes_player_and_npc_running_distance() {
        use crate::components::fighter::FighterStatusEffects;
        use bevy::ecs::system::RunSystemOnce;
        let table = parse_meteor_res(include_bytes!("../../../../../origin_res/Meteor.res"))
            .unwrap()
            .combat_table();
        for npc in [false, true] {
            let mut distances = Vec::new();
            for slow in [false, true] {
                let mut h = if npc {
                    spawn_npc_combat_harness(6, [0.0; 3]).0
                } else {
                    spawn_gameplay_harness(6).unwrap()
                };
                if slow {
                    h.app
                        .world_mut()
                        .get_mut::<FighterStatusEffects>(h.fighter)
                        .unwrap()
                        .apply(table.pose_status_effect(224).unwrap());
                }
                let start = h.position();
                if npc {
                    h.app
                        .world_mut()
                        .get_mut::<AiIntent>(h.fighter)
                        .unwrap()
                        .move_dir = Vec2::Y;
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(h.fighter)
                        .unwrap()
                        .movement_mode = meteor_core::MovementMode::Run;
                    for _ in 0..60 {
                        h.app
                            .world_mut()
                            .resource_mut::<Time>()
                            .advance_by(std::time::Duration::from_secs_f32(HARNESS_DT));
                        h.app
                            .world_mut()
                            .run_system_once(crate::plugins::ai_plugin::move_npc_fighters)
                            .unwrap();
                    }
                } else {
                    h.press_binding(crate::app_state::Keymap::default().move_up);
                    h.steps(60);
                }
                distances.push(h.position().distance(start));
            }
            assert!(
                distances[0] > 5.0 && distances[1] > 0.0 && distances[1] < distances[0] * 0.65,
                "npc={npc}: {distances:?}"
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn production_sword_small_ultimate_pose_560_emits_two_hits() {
        use bevy::ecs::system::RunSystemOnce;

        let (mut h, target, take_hits) = spawn_local_stock_hit_harness(6);
        h.app
            .world_mut()
            .resource_scope(|world, assets: Mut<CharacterAssetsSet>| {
                let character = assets.0.get("p1").expect("p1 assets");
                let mut pose = world
                    .get_mut::<PoseRuntime>(h.fighter)
                    .expect("local pose runtime");
                character.apply_pose(&mut pose, 560, true);
            });
        {
            let mut state = h.app.world_mut().get_mut::<SmState>(h.fighter).unwrap();
            state.current = FighterState::Attack1;
        }
        h.app
            .world_mut()
            .get_mut::<CombatState>(h.fighter)
            .unwrap()
            .state = FighterState::Attack1;

        let (start, end) = {
            let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
            (pose.start, pose.end)
        };
        let mut claims = Vec::new();
        for frame in start..=end {
            h.app
                .world_mut()
                .get_mut::<PoseRuntime>(h.fighter)
                .unwrap()
                .frame = frame as f32;
            h.app
                .world_mut()
                .get_mut::<crate::components::fighter::TargetHitClock>(target)
                .unwrap()
                .tick(HARNESS_DT);
            crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
            h.app
                .world_mut()
                .run_system_once(attack_windows)
                .expect("production sword ultimate hit judge");
            claims.extend(take_hits());
        }

        assert_eq!(
            claims.len(),
            2,
            "Pose 560 owns two authored Attack blocks separated by the original >0.2s target gate"
        );
        assert!(claims.iter().all(|(source, hit)| {
            source.is_none() && hit.target_actor_id == 2 && hit.hit_word & 0x3ff == 560
        }));
    }

    #[test]
    fn production_npc_combat_uses_own_commands_and_spends_anger() {
        for weapon in 1..=12 {
            let weights = if weapon == 10 {
                [0.0, 50.0, 50.0]
            } else {
                [0.0, 0.0, 100.0]
            };
            let (mut h, _) = spawn_npc_combat_harness(weapon, weights);
            h.app.world_mut().get_mut::<Qi>(h.fighter).unwrap().current = 100.0;
            h.app
                .world_mut()
                .resource_mut::<LocalCommandInput>()
                .0
                .push_direction(meteor_input::command_parser::Dir::Left, 0.0);
            let pending = h.app.world().resource::<LocalCommandInput>().0.peek();
            let mut poses = std::collections::BTreeSet::new();
            for _ in 0..600 {
                h.step();
                poses.insert(h.pose_id());
                if h.app.world().get::<Qi>(h.fighter).unwrap().current < 100.0 {
                    break;
                }
            }
            let qi = h.app.world().get::<Qi>(h.fighter).unwrap().current;
            assert!(
                (0.0..100.0).contains(&qi),
                "NPC weapon {weapon} must execute an authored anger move: {poses:?}, qi={qi}"
            );

            assert_eq!(
                h.app.world().resource::<LocalCommandInput>().0.peek(),
                pending
            );
        }
    }

    #[test]
    fn production_npc_combat_has_independent_lock_and_freezes_action_facing() {
        let (mut h, target) = spawn_npc_combat_harness(6, [100.0, 0.0, 0.0]);
        h.step();
        assert_eq!(
            h.app
                .world()
                .get::<AiIntent>(h.fighter)
                .unwrap()
                .lock_target,
            Some(target)
        );
        assert_eq!(
            h.app.world().resource::<LockOnController>().lock_entity,
            None
        );
        h.app
            .world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::new(3.0, 0.0, 0.0);
        let pose_id = h.pose_id();
        h.app
            .world_mut()
            .get_mut::<EntryBlend>(h.fighter)
            .unwrap()
            .duration = 0.0;
        let facing = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
        h.step();
        assert_eq!(
            h.app.world().get::<MotionState>(h.fighter).unwrap().facing,
            facing,
            "Action frames cannot freely spin attacks"
        );
        h.app
            .world_mut()
            .get_mut::<EntryBlend>(h.fighter)
            .unwrap()
            .duration = 0.2;
        h.app
            .world_mut()
            .get_mut::<EntryBlend>(h.fighter)
            .unwrap()
            .pose_id = pose_id;
        h.step();
        assert!(
            h.app.world().get::<MotionState>(h.fighter).unwrap().facing > 1.0,
            "entry blend must follow the NPC's own enemy"
        );
        h.app.world_mut().get_mut::<Health>(target).unwrap().current = 0.0;
        h.step();
        assert_eq!(
            h.app
                .world()
                .get::<AiIntent>(h.fighter)
                .unwrap()
                .lock_target,
            None
        );
    }

    #[test]
    fn production_npc_combat_reacts_to_damage_with_real_qi_burst() {
        let (mut h, _) = spawn_npc_combat_harness(6, [0.0; 3]);
        h.app.world_mut().get_mut::<Qi>(h.fighter).unwrap().current = 100.0;
        h.step();
        for _ in 0..60 {
            h.app
                .world_mut()
                .get_mut::<Health>(h.fighter)
                .unwrap()
                .current -= 1.0;
            h.app
                .world_mut()
                .get_mut::<AiIntent>(h.fighter)
                .unwrap()
                .attack_cooldown = 0.0;
            h.step();
            if h.pose_id() == QI_BURST_POSE_ID {
                break;
            }
        }
        assert_eq!(h.pose_id(), QI_BURST_POSE_ID);
        assert_eq!(
            h.app.world().get::<Qi>(h.fighter).unwrap().current,
            40.0,
            "general qi burst spends exactly 60"
        );
        assert!(
            !h.app
                .world()
                .get::<FighterDodge>(h.fighter)
                .unwrap()
                .state
                .active
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn production_stock_npc_anger_reports_only_paid_commits_and_burst_once() {
        use crate::plugins::network_plugin::{stock_anger_test_channel, PendingStockAnger};
        use meteor_network::stock_protocol::StockAngerSpend;
        for weapon in [5, 6, 11] {
            let (mut h, _) = spawn_npc_combat_harness(weapon, [0.0, 0.0, 100.0]);
            let (tx, take) = stock_anger_test_channel();
            h.app
                .insert_resource(tx)
                .init_resource::<PendingStockAnger>();
            {
                let mut net = h.app.world_mut().resource_mut::<NetState>();
                net.status = NetStatus::InRoom;
                net.address = "udp-host://0.0.0.0:5230".into();
                net.stock_npc_ready.insert(h.fighter);
            }
            h.app.world_mut().get_mut::<Qi>(h.fighter).unwrap().current = 100.0;
            h.steps(180);
            let spent = take();
            assert!(
                !spent.is_empty(),
                "weapon {weapon} must report real paid commits"
            );
            assert!(spent.iter().all(|(actor, _)| *actor == Some(h.fighter)));
            let sum: u32 = spent.iter().map(|(_, spend)| spend.cost()).sum();
            assert_eq!(
                sum as f32,
                100.0 - h.app.world().get::<Qi>(h.fighter).unwrap().current
            );
            h.app.world_mut().get_mut::<Qi>(h.fighter).unwrap().current = 0.0;
            h.steps(120);
            assert!(
                take().is_empty(),
                "failed/unaffordable commands and free attacks cannot report spend"
            );
        }
        let (mut h, _) = spawn_npc_combat_harness(6, [0.0; 3]);
        let (tx, take) = stock_anger_test_channel();
        h.app
            .insert_resource(tx)
            .init_resource::<PendingStockAnger>();
        {
            let mut net = h.app.world_mut().resource_mut::<NetState>();
            net.status = NetStatus::InRoom;
            net.address = "udp-host://0.0.0.0:5230".into();
            net.stock_npc_ready.insert(h.fighter);
        }
        h.app.world_mut().get_mut::<Qi>(h.fighter).unwrap().current = 100.0;
        h.step();
        for _ in 0..60 {
            h.app
                .world_mut()
                .get_mut::<Health>(h.fighter)
                .unwrap()
                .current -= 1.0;
            h.app
                .world_mut()
                .get_mut::<AiIntent>(h.fighter)
                .unwrap()
                .attack_cooldown = 0.0;
            h.step();
            if h.pose_id() == QI_BURST_POSE_ID {
                break;
            }
        }
        assert_eq!(h.pose_id(), QI_BURST_POSE_ID);
        assert_eq!(take(), [(Some(h.fighter), StockAngerSpend::Burst)]);
        assert_eq!(h.app.world().get::<Qi>(h.fighter).unwrap().current, 40.0);
        h.steps(30);
        assert!(take().is_empty());
    }

    #[test]
    fn production_npc_combat_room_targets_include_other_npcs() {
        let (mut h, player) = spawn_npc_combat_harness(6, [0.0; 3]);
        h.app
            .world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation
            .z = 20.0;
        let fighter = h.app.world().get::<Fighter>(h.fighter).unwrap().clone();
        let other = h
            .app
            .world_mut()
            .spawn((
                NpcFighter,
                fighter,
                Health::default(),
                Transform::from_xyz(0.0, 0.0, 3.0),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                FighterAir::default(),
                AiIntent::default(),
                crate::plugins::ai_plugin::NpcPatrol::default(),
            ))
            .id();
        h.step();
        assert_eq!(
            h.app
                .world()
                .get::<AiIntent>(h.fighter)
                .unwrap()
                .enemy_entity,
            Some(other)
        );
        assert_eq!(
            h.app
                .world()
                .get::<AiIntent>(h.fighter)
                .unwrap()
                .lock_target,
            Some(other)
        );
        assert_eq!(
            h.app.world().get::<AiIntent>(other).unwrap().enemy_entity,
            Some(h.fighter)
        );
        h.app.world_mut().get_mut::<Health>(other).unwrap().current = 0.0;
        h.steps(16);
        assert_eq!(
            h.app
                .world()
                .get::<AiIntent>(h.fighter)
                .unwrap()
                .enemy_entity,
            Some(player)
        );
        assert_ne!(
            h.app
                .world()
                .get::<AiIntent>(h.fighter)
                .unwrap()
                .lock_target,
            Some(other)
        );
    }

    #[test]
    fn production_npc_combat_cannot_spend_missing_anger() {
        let (mut h, _) = spawn_npc_combat_harness(6, [0.0, 0.0, 100.0]);
        for _ in 0..300 {
            h.step();
            assert_eq!(h.app.world().get::<Qi>(h.fighter).unwrap().current, 0.0);
            assert_ne!(h.pose_id(), QI_BURST_POSE_ID);
        }
    }

    #[test]
    fn production_npc_combat_ranged_aim_tracks_height_without_camera_lock() {
        let (mut h, target) = spawn_npc_combat_harness(3, [100.0, 0.0, 0.0]);
        h.app
            .world_mut()
            .get_mut::<NpcCombatProfile>(h.fighter)
            .unwrap()
            .aim = 100.0;
        h.app
            .world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation
            .y = 5.0;
        h.app
            .world_mut()
            .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
            .yaw = std::f32::consts::PI;
        h.step();
        let intent = h.app.world().get::<AiIntent>(h.fighter).unwrap();
        let aim = intent
            .ranged_aim
            .expect("NPC stores its own aim at attack decision");
        assert!(aim.y > 0.0 && aim.z > 0.9);
        assert_eq!(
            intent.lock_target, None,
            "original ranged weapons do not acquire melee Lock"
        );
        assert_eq!(
            h.app.world().resource::<LockOnController>().lock_entity,
            None
        );
    }

    #[test]
    fn production_npc_combat_qi_burst_releases_hit_and_down_ownership() {
        use meteor_combat::hit_reaction::{FacingBucket, HitReact};
        for (state, kind, pose_id) in [
            (FighterState::BeHit, HitReactKind::BeHit, 93),
            (FighterState::Down, HitReactKind::Down, 113),
        ] {
            let (mut h, _) = spawn_npc_combat_harness(6, [0.0; 3]);
            h.app.world_mut().get_mut::<Qi>(h.fighter).unwrap().current = 100.0;
            h.step();
            h.app
                .world_mut()
                .get_mut::<NpcPerformance>(h.fighter)
                .unwrap()
                .guard_remaining = 5.0;
            let mut pose = PoseRuntime::idle();
            h.app.world().resource::<CharacterAssetsSet>().0["p1"]
                .apply_pose(&mut pose, pose_id, true);
            h.app.world_mut().entity_mut(h.fighter).insert(pose);
            h.app
                .world_mut()
                .get_mut::<SmState>(h.fighter)
                .unwrap()
                .current = state;
            h.app
                .world_mut()
                .get_mut::<FighterHitReaction>(h.fighter)
                .unwrap()
                .begin(
                    HitReact {
                        kind,
                        pose_id: Some(pose_id),
                        head_pose_id: None,
                        lock_remaining: 10.0,
                        minimum_remaining: 10.0,
                        authored_minimum: 10.0,
                        motion_scale: 1.0,
                        down_remaining: 10.0,
                        bucket: FacingBucket::Front,
                        environmental: false,
                        selector: 0,
                        rewind_count: 0,
                        play_to_end_pose_id: None,
                    },
                    false,
                );
            for _ in 0..60 {
                h.app
                    .world_mut()
                    .get_mut::<Health>(h.fighter)
                    .unwrap()
                    .current -= 1.0;
                h.app
                    .world_mut()
                    .get_mut::<AiIntent>(h.fighter)
                    .unwrap()
                    .attack_cooldown = 0.0;
                h.step();
                if h.pose_id() == QI_BURST_POSE_ID {
                    break;
                }
            }
            assert_eq!(
                h.pose_id(),
                QI_BURST_POSE_ID,
                "NPC must escape {state:?} through the common QiBurst action"
            );
            assert!(h
                .app
                .world()
                .get::<FighterHitReaction>(h.fighter)
                .unwrap()
                .current
                .is_none());
            assert_eq!(h.app.world().get::<Qi>(h.fighter).unwrap().current, 40.0);
            h.step();
            assert_eq!(
                h.app
                    .world()
                    .get::<NpcPerformance>(h.fighter)
                    .unwrap()
                    .guard_remaining,
                0.0,
                "old guard hold cannot overwrite the accepted QiBurst"
            );
        }
    }

    #[test]
    fn production_wheel_return_wait_preserves_pose_for_every_movement_direction() {
        let keys = crate::app_state::Keymap::default();
        for binding in [
            keys.move_up,
            keys.move_down,
            keys.move_left,
            keys.move_right,
        ] {
            let mut h = spawn_gameplay_harness(2).expect("original wheel resources");

            let definition =
                h.app.world().resource::<CharacterAssetsSet>().0["p1"].poses[&220].clone();
            {
                let mut pose = h.app.world_mut().get_mut::<PoseRuntime>(h.fighter).unwrap();
                pose.pose_id = 220;
                pose.start = definition.start;
                pose.end = definition.end;
                pose.frame = definition.end as f32;
            }
            h.app
                .world_mut()
                .get_mut::<RangedFireGate>(h.fighter)
                .unwrap()
                .wheel_out = true;
            let start = h.position();
            h.press_binding(binding);
            h.steps(6);
            assert_eq!(h.pose_id(), 220, "{binding:?}");
            assert!(h.position().distance(start) < 0.001, "{binding:?}");
            h.app
                .world_mut()
                .get_mut::<RangedFireGate>(h.fighter)
                .unwrap()
                .wheel_out = false;
            h.steps(3);
            assert_ne!(h.pose_id(), 220, "return releases locomotion");
            assert!(h.position().distance(start) > 0.0);
        }
    }

    #[test]
    fn production_slow_pose_tracks_speed_threshold_and_recovers() {
        use crate::components::fighter::FighterStatusEffects;
        use bevy::ecs::system::RunSystemOnce;
        let table = parse_meteor_res(include_bytes!("../../../../../origin_res/Meteor.res"))
            .unwrap()
            .combat_table();
        for npc in [false, true] {
            let mut h = if npc {
                spawn_npc_combat_harness(6, [0.0; 3]).0
            } else {
                spawn_gameplay_harness(6).unwrap()
            };
            h.app
                .world_mut()
                .get_mut::<FighterStatusEffects>(h.fighter)
                .unwrap()
                .apply(table.pose_status_effect(224).unwrap());
            if npc {
                h.app
                    .world_mut()
                    .get_mut::<AiIntent>(h.fighter)
                    .unwrap()
                    .locomotion_input = Vec2::Y;
            } else {
                h.press_binding(crate::app_state::Keymap::default().move_up);
            }
            for expected in [L::SLOW_WALK, L::RUN_F] {
                h.app
                    .world_mut()
                    .run_system_once(preadvance_local_ground_locomotion_pose)
                    .unwrap();
                h.app.world_mut().run_system_once(default_poses).unwrap();
                assert_eq!(h.pose_id(), expected, "npc={npc}");
                *h.app
                    .world_mut()
                    .get_mut::<FighterStatusEffects>(h.fighter)
                    .unwrap() = FighterStatusEffects::default();
            }
        }
        let mut h = spawn_gameplay_harness(6).unwrap();
        h.press_binding(crate::app_state::Keymap::default().move_up);
        for (factor, expected) in [(0.699, L::SLOW_WALK), (0.7, L::RUN_F), (1.0, L::RUN_F)] {
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::item_plugin::ItemBuffs>()
                .speed_factor = factor;
            h.app
                .world_mut()
                .run_system_once(preadvance_local_ground_locomotion_pose)
                .unwrap();
            assert_eq!(h.pose_id(), expected);
        }
    }

    #[test]
    fn ground_locomotion_pose_is_selected_before_the_same_frame_animation_sample() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };

        harness.press_key(KeyCode::KeyW);
        harness.step();
        assert_eq!(
            harness.app.world().resource::<HarnessPoseBeforeAdvance>().0,
            Some(L::RUN_F),
            "Web selects run before Fighter.update samples this frame"
        );

        harness.release_key(KeyCode::KeyW);
        harness.step();
        assert_eq!(
            harness.app.world().resource::<HarnessPoseBeforeAdvance>().0,
            Some(L::IDLE),
            "key-up must select idle before the same-frame animation sample"
        );
    }

    #[test]
    fn landed_wall_kick_advances_once_and_releases_without_restarting_at_pose_start() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let fighter = harness.fighter;
        let kick_pose = L::WALL_KICK_L;
        harness
            .app
            .world_mut()
            .resource_scope(|world, assets: Mut<CharacterAssetsSet>| {
                let character = assets.0.get("p1").expect("p1 assets");
                let mut entity = world.entity_mut(fighter);
                let mut pose = entity.get_mut::<PoseRuntime>().expect("fighter pose");
                character.apply_pose(&mut pose, kick_pose, true);
            });
        harness
            .app
            .world_mut()
            .get_mut::<FighterWall>(fighter)
            .expect("fighter wall state")
            .kick_pose = kick_pose;
        let start = harness
            .app
            .world()
            .get::<PoseRuntime>(fighter)
            .expect("fighter pose")
            .start as f32;

        harness.step();
        assert_eq!(
            harness.app.world().resource::<HarnessPoseBeforeAdvance>().0,
            Some(kick_pose),
            "ground locomotion must not replace an unfinished wall kick before animation advance"
        );
        assert!(
            harness
                .app
                .world()
                .get::<PoseRuntime>(fighter)
                .expect("fighter pose")
                .frame
                > start,
            "wall kick cursor must advance instead of being reset to Start every frame"
        );

        harness.steps(12);
        assert_eq!(
            harness
                .app
                .world()
                .get::<FighterWall>(fighter)
                .expect("fighter wall state")
                .kick_pose,
            0,
            "completed or linked wall kick must release its one-shot ownership"
        );
        assert_ne!(
            harness.pose_id(),
            kick_pose,
            "ground locomotion must take over after the authored wall-kick clip"
        );
    }

    #[test]
    fn production_down_recovery_consumes_held_guard_and_prioritizes_direction() {
        use meteor_combat::hit_reaction::HitReact;
        for (direction, fresh_guard) in [(false, false), (true, false), (true, true)] {
            let mut h = spawn_gameplay_harness(6).expect("original character resources");
            let guard = h.app.world().resource::<crate::app_state::Keymap>().guard;
            if !fresh_guard {
                h.press_binding(guard);
                h.step();
            }
            let fighter = h.fighter;
            h.app
                .world_mut()
                .get_mut::<SmState>(fighter)
                .unwrap()
                .current = FighterState::Down;
            h.app
                .world_mut()
                .get_mut::<CombatState>(fighter)
                .unwrap()
                .state = FighterState::Down;
            let mut pose = PoseRuntime::idle();
            h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, 113, true);
            h.app.world_mut().entity_mut(fighter).insert(pose);
            h.app
                .world_mut()
                .get_mut::<FighterHitReaction>(fighter)
                .unwrap()
                .begin(
                    HitReact {
                        kind: HitReactKind::Down,
                        pose_id: Some(113),
                        head_pose_id: None,
                        lock_remaining: 0.0,
                        minimum_remaining: 0.0,
                        authored_minimum: 0.0,
                        motion_scale: 1.0,
                        down_remaining: 0.0,
                        bucket: meteor_combat::hit_reaction::FacingBucket::Front,
                        environmental: false,
                        selector: 0,
                        rewind_count: 0,
                        play_to_end_pose_id: None,
                    },
                    false,
                );
            if direction {
                let forward = h.app.world().resource::<crate::app_state::Keymap>().move_up;
                h.press_binding(forward);
            }
            if fresh_guard {
                h.press_binding(guard);
            }
            h.step();
            if direction {
                assert!(
                    matches!(h.pose_id(), 176..=179),
                    "direction must select a ground roll"
                );
                assert!(
                    h.app
                        .world()
                        .get::<FighterDodge>(fighter)
                        .unwrap()
                        .state
                        .active
                );
            } else {
                assert_eq!(h.pose_id(), 120, "held Guard must recover from supine down");
                assert_eq!(
                    h.app
                        .world()
                        .get::<FighterHitReaction>(fighter)
                        .unwrap()
                        .current
                        .as_ref()
                        .unwrap()
                        .kind,
                    HitReactKind::GetUp
                );
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn production_fist_416_hits_stock_npc_with_live_effect() {
        use crate::plugins::effect_plugin as fx;
        use bevy::ecs::system::RunSystemOnce;
        for fps in [30, 60, 120] {
            for (chained, down, distance) in
                [(false, false, 6.0), (true, false, 6.0), (true, true, 12.0)]
            {
                let dt = 1.0 / fps as f32;
                let (mut h, target, take_hits) = spawn_local_stock_hit_harness(11);
                let attacker = h.fighter;
                for entity in [attacker, target] {
                    h.app
                        .world_mut()
                        .entity_mut(entity)
                        .insert(crate::components::fighter::VisualRootPin::default());
                }
                h.app
                    .init_resource::<Assets<fx::PonEfMaterial>>()
                    .init_resource::<Assets<Mesh>>()
                    .init_resource::<Assets<Image>>()
                    .init_resource::<Assets<StandardMaterial>>()
                    .init_resource::<fx::EfTextureCache>()
                    .init_resource::<Assets<crate::plugins::stage_material::GmdlMaterial>>();
                h.app
                    .world_mut()
                    .run_system_once(fx::setup_effect_assets)
                    .unwrap();
                h.app.add_systems(
                    Update,
                    fx::advance_pon_ef_clocks
                        .after(crate::plugins::FighterFrameSet::SkeletonSample)
                        .before(crate::plugins::FighterFrameSet::HitJudge),
                );
                h.app.add_systems(
                    Update,
                    (fx::pose_effect_spawn, fx::tick_pon_ef_players)
                        .chain()
                        .after(crate::plugins::FighterFrameSet::CameraFinalize),
                );
                h.app.add_systems(
                    Update,
                    (|world: &mut World| {
                        crate::plugins::character_plugin::drive_fighter_animation_for_test(world);
                    })
                    .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
                );
                h.app.add_systems(
                    Update,
                    (tick_target_hit_clocks, attack_windows)
                        .chain()
                        .in_set(crate::plugins::FighterFrameSet::HitJudge),
                );
                h.app
                    .world_mut()
                    .get_mut::<Transform>(target)
                    .unwrap()
                    .translation = Vec3::new(0.0, 0.0, distance);
                if down {
                    let mut pose = PoseRuntime::idle();
                    h.app.world().resource::<CharacterAssetsSet>().0["p1"]
                        .apply_pose(&mut pose, 113, true);
                    h.app.world_mut().entity_mut(target).insert((
                        pose,
                        SmState {
                            current: FighterState::Down,
                            ..default()
                        },
                        CombatState {
                            state: FighterState::Down,
                        },
                    ));
                }
                h.app.world_mut().get_mut::<Qi>(attacker).unwrap().current = 60.0;
                if chained {
                    h.press_mouse(MouseButton::Left);
                    h.step_dt(dt);
                    h.release_mouse(MouseButton::Left);
                    for _ in 0..fps {
                        h.step_dt(dt);
                        if h.app.world().get::<PoseRuntime>(attacker).unwrap().frame >= 13708.0 {
                            break;
                        }
                    }
                    assert_eq!(h.pose_id(), 407);
                }
                for key in [KeyCode::KeyA, KeyCode::KeyD, KeyCode::KeyW] {
                    h.press_key(key);
                    h.step_dt(dt);
                    h.release_key(key);
                }
                h.press_mouse(MouseButton::Left);
                h.step_dt(dt);
                h.release_mouse(MouseButton::Left);
                for _ in 0..fps {
                    if h.pose_id() == 416 {
                        break;
                    }
                    h.step_dt(dt);
                }
                assert_eq!(h.pose_id(), 416);
                if chained {
                    assert_eq!(
                        h.app.world().get::<EntryBlend>(attacker).unwrap().kind,
                        EntryBlendKind::Chained
                    );
                }
                take_hits();
                let mut hits = Vec::new();
                for _ in 0..fps * 3 {
                    h.step_dt(dt);
                    hits.extend(take_hits());
                    if down {
                        assert_eq!(
                            h.app.world().get::<PoseRuntime>(target).unwrap().pose_id,
                            113
                        );
                    }
                }
                eprintln!(
                    "FIST416 fps={fps} chained={chained} down={down} distance={distance} hits={}",
                    hits.len()
                );
                assert!(!hits.is_empty(), "60-Qi fist special must hit stock NPC: fps={fps} chained={chained} down={down} distance={distance}");
                assert!(hits
                    .iter()
                    .all(|(_, hit)| hit.target_actor_id == 2 && hit.hit_word & 0x03ff == 416));
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn production_sword_wave_hits_at_range_on_live_playback_clock() {
        use crate::plugins::effect_plugin as fx;
        use bevy::ecs::system::RunSystemOnce;
        for fps in [20, 30, 60, 120] {
            let dt = 1.0 / fps as f32;
            let (mut h, target, take_hits) = spawn_local_stock_hit_harness(6);
            h.app
                .init_resource::<Assets<fx::PonEfMaterial>>()
                .init_resource::<Assets<Mesh>>()
                .init_resource::<Assets<Image>>()
                .init_resource::<Assets<StandardMaterial>>()
                .init_resource::<fx::EfTextureCache>()
                .init_resource::<Assets<crate::plugins::stage_material::GmdlMaterial>>();
            h.app
                .world_mut()
                .run_system_once(fx::setup_effect_assets)
                .unwrap();
            h.app.add_systems(
                Update,
                fx::advance_pon_ef_clocks
                    .after(crate::plugins::FighterFrameSet::SkeletonSample)
                    .before(crate::plugins::FighterFrameSet::HitJudge),
            );
            h.app.add_systems(
                Update,
                (fx::pose_effect_spawn, fx::tick_pon_ef_players)
                    .chain()
                    .after(crate::plugins::FighterFrameSet::CameraFinalize),
            );
            h.app.add_systems(
                Update,
                (|world: &mut World| {
                    crate::plugins::character_plugin::drive_fighter_animation_for_test(world);
                })
                .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
            );
            h.app.add_systems(
                Update,
                (tick_target_hit_clocks, attack_windows)
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::HitJudge),
            );
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = Vec3::new(0.0, 0.0, 6.0);
            h.app.world_mut().get_mut::<Qi>(h.fighter).unwrap().current = 100.0;
            h.press_key(KeyCode::KeyA);
            h.step_dt(dt);
            h.release_key(KeyCode::KeyA);
            h.press_key(KeyCode::KeyD);
            h.step_dt(dt);
            h.release_key(KeyCode::KeyD);
            h.press_mouse(MouseButton::Left);
            h.step_dt(dt);
            h.release_mouse(MouseButton::Left);
            assert_eq!(h.pose_id(), 560);
            let mut hits = Vec::new();
            for _ in 0..fps * 3 {
                h.step_dt(dt);
                hits.extend(take_hits());
            }
            assert_eq!(
                hits.len(),
                2,
                "both sword waves must hit a target 6 units ahead at {fps} FPS"
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn place_target_in_current_attack(h: &mut GameplayHarness, target: Entity) {
        use bevy::ecs::system::RunSystemOnce;
        let attacker = h.fighter;
        let offset = h
            .app
            .world_mut()
            .run_system_once(
                move |geometry: HitGeometryQueries,
                      assets: Res<CharacterAssetsSet>,
                      fighters: Query<(&Fighter, &WeaponForm, &PoseRuntime)>| {
                    let (fighter, form, pose) = fighters.get(attacker).unwrap();
                    let character = &assets.0["p1"];
                    let attacks = &character.poses[&pose.pose_id].attacks;
                    let (_, attack) = active_attack_window(pose.frame, attacks).unwrap();
                    let attacks = geometry.attack_obbs(
                        attacker,
                        fighter,
                        form,
                        pose.pose_id,
                        character,
                        &attack.bones,
                    );
                    let hurts = geometry.hurt_obbs(target, character);
                    assert!(!attacks.is_empty(), "authored attack geometry must exist");
                    assert!(!hurts.is_empty(), "authored target geometry must exist");
                    let delta = attacks[0].center - hurts[0].center;
                    Vec3::new(delta.x, delta.y, delta.z)
                },
            )
            .unwrap();
        h.app
            .world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation += offset;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn production_ninja_self_destruct_hits_before_death_and_also_dies_on_miss() {
        use bevy::ecs::system::RunSystemOnce;
        for (hit_target, frame_offset) in [(true, 1), (false, 1), (true, 0)] {
            let (mut h, target, _) = spawn_local_stock_hit_harness(12);
            h.app.insert_resource(NetState::default());
            h.app.insert_resource(CombatResourceData(
                parse_meteor_res(include_bytes!("../../../../../origin_res/Meteor.res"))
                    .unwrap()
                    .combat_table(),
            ));
            let mut pose = PoseRuntime::idle();
            h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, 468, true);
            pose.frame = pose.end.saturating_sub(2) as f32;
            h.app
                .world_mut()
                .entity_mut(h.fighter)
                .insert((pose, EntryBlend::default()));
            h.app
                .world_mut()
                .get_mut::<SmState>(h.fighter)
                .unwrap()
                .current = FighterState::Attack1;
            h.app
                .world_mut()
                .get_mut::<CombatState>(h.fighter)
                .unwrap()
                .state = FighterState::Attack1;
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = if hit_target {
                Vec3::ZERO
            } else {
                Vec3::splat(1000.0)
            };
            h.app.world_mut().get_mut::<Health>(target).unwrap().current = 2000.0;
            h.app.world_mut().get_mut::<Health>(target).unwrap().max = 2000.0;
            crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
            h.app.world_mut().run_system_once(attack_windows).unwrap();
            assert!(
                h.app.world().get::<Health>(h.fighter).unwrap().current > 0.0,
                "self effect must not fire before the last two frames"
            );
            assert_eq!(h.app.world().get::<Health>(target).unwrap().current, 2000.0);
            {
                let mut pose = h.app.world_mut().get_mut::<PoseRuntime>(h.fighter).unwrap();
                pose.frame = (pose.end - frame_offset) as f32;
            }
            crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
            if hit_target {
                place_target_in_current_attack(&mut h, target);
            }
            h.app.world_mut().run_system_once(attack_windows).unwrap();
            let target_health = h.app.world().get::<Health>(target).unwrap().current;
            assert_eq!(
                target_health < 2000.0,
                hit_target,
                "attack must resolve before killing its owner: {target_health}"
            );
            assert_eq!(h.app.world().get::<Health>(h.fighter).unwrap().current, 0.0);
            assert_eq!(
                h.app.world().get::<SmState>(h.fighter).unwrap().current,
                FighterState::Dead
            );
            assert!(h.app.world().get::<DeathRespawn>(h.fighter).unwrap().active);
            assert_eq!(
                h.app.world().get::<PoseRuntime>(h.fighter).unwrap().pose_id,
                117
            );
            assert_eq!(
                h.app
                    .world()
                    .resource::<crate::components::fighter::KillScore>()
                    .player_kills,
                0
            );
            h.app.world_mut().run_system_once(attack_windows).unwrap();
            assert_eq!(
                h.app.world().get::<Health>(target).unwrap().current,
                target_health
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn production_blade_back_attack_releases_after_speed_two_reaction_tail() {
        use bevy::ecs::system::RunSystemOnce;
        for fps in [30, 60, 120] {
            let (mut h, target, _) = spawn_local_stock_hit_harness(8);
            h.app.insert_resource(NetState::default());
            let mut pose = PoseRuntime::idle();
            let character = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
            character.apply_pose(&mut pose, 300, true);
            pose.frame = character.poses[&300].attacks[0].start as f32;
            h.app
                .world_mut()
                .entity_mut(h.fighter)
                .insert((pose, EntryBlend::default()));
            h.app
                .world_mut()
                .get_mut::<SmState>(h.fighter)
                .unwrap()
                .current = FighterState::Attack1;
            h.app
                .world_mut()
                .get_mut::<CombatState>(h.fighter)
                .unwrap()
                .state = FighterState::Attack1;
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = Vec3::ZERO;
            crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
            place_target_in_current_attack(&mut h, target);
            h.app.world_mut().run_system_once(attack_windows).unwrap();
            assert_eq!(
                h.app.world().get::<PoseRuntime>(target).unwrap().pose_id,
                109
            );
            let mut elapsed = 0.0;
            while elapsed < 3.0
                && h.app
                    .world()
                    .get::<FighterHitReaction>(target)
                    .unwrap()
                    .current
                    .is_some()
            {
                h.step_dt(1.0 / fps as f32);
                elapsed += 1.0 / fps as f32;
            }
            assert!((1.2..1.65).contains(&elapsed), "109 must finish its Speed=2 tail without a 2.53s extra gate at {fps} FPS: {elapsed}");
            assert_ne!(
                h.app.world().get::<SmState>(target).unwrap().current,
                FighterState::BeHit
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn production_area_ultimates_use_live_effects_beyond_melee_bones() {
        use crate::plugins::effect_plugin::{EfCache, PonEfInstance};
        use bevy::ecs::system::RunSystemOnce;
        for (weapon, pose_id, effect_name) in [
            (11, 421, "pos421"),
            (5, 259, "pos259"),
            (9, 327, "pos326"),
            (9, 373, "pos324"),
            (10, 487, "pos456"),
        ] {
            let (mut h, target, take_hits) = spawn_local_stock_hit_harness(weapon);
            let attacker = h.fighter;
            let def =
                h.app.world().resource::<CharacterAssetsSet>().0["p1"].poses[&pose_id].clone();
            let window = def
                .attacks
                .iter()
                .find(|attack| {
                    attack
                        .bones
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case("effect"))
                })
                .expect("authored effect attack");
            let mut runtime = PoseRuntime::idle();
            h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(
                &mut runtime,
                pose_id,
                true,
            );
            runtime.frame = window.start as f32;
            h.app
                .world_mut()
                .entity_mut(attacker)
                .insert((runtime, EntryBlend::default()));
            h.app
                .world_mut()
                .get_mut::<SmState>(attacker)
                .unwrap()
                .current = FighterState::Attack1;
            h.app
                .world_mut()
                .get_mut::<CombatState>(attacker)
                .unwrap()
                .state = FighterState::Attack1;
            let ef = h
                .app
                .world_mut()
                .resource_mut::<EfCache>()
                .load(effect_name)
                .expect("original EF");
            let duration = ef
                .nodes
                .iter()
                .map(|node| node.end_time)
                .fold(0.0f32, f32::max);
            let node = ef
                .nodes
                .iter()
                .find(|node| node.is_attack_node())
                .expect("attack shape");
            let elapsed = (node.start_time + node.end_time) * 0.5;
            let model = h
                .app
                .world_mut()
                .query::<(Entity, &FighterModel)>()
                .iter(h.app.world())
                .find_map(|(entity, model)| (model.fighter == attacker).then_some(entity))
                .unwrap();
            let effect = h
                .app
                .world_mut()
                .spawn((
                    PonEfInstance {
                        ef: ef.clone(),
                        elapsed,
                        duration,
                        looping: false,
                        audio_playing: vec![None; ef.nodes.len()],
                        audio_triggered: vec![false; ef.nodes.len()],
                    },
                    ChildOf(model),
                ))
                .id();
            crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
            let shapes = h
                .app
                .world_mut()
                .run_system_once(
                    move |geometry: HitGeometryQueries, assets: Res<CharacterAssetsSet>| {
                        geometry.live_effect_obbs(attacker, &assets.0["p1"])
                    },
                )
                .unwrap();
            assert!(
                !shapes.is_empty(),
                "pose {pose_id} must see live {effect_name}"
            );

            let mut chosen = None;
            'search: for y in [0, 2, 4, 6, 8] {
                for x in -16..=16 {
                    for z in -16..=16 {
                        if x * x + z * z + y * y < 16 {
                            continue;
                        }
                        let position = Vec3::new(x as f32, y as f32, z as f32);
                        h.app
                            .world_mut()
                            .get_mut::<Transform>(target)
                            .unwrap()
                            .translation = position;
                        let names = window.bones.clone();
                        let area_only =
                            h.app
                                .world_mut()
                                .run_system_once(
                                    move |geometry: HitGeometryQueries,
                                          assets: Res<CharacterAssetsSet>,
                                          fighters: Query<(
                                        &Fighter,
                                        &WeaponForm,
                                        &PoseRuntime,
                                    )>| {
                                        let character = &assets.0["p1"];
                                        let (fighter, form, pose) = fighters.get(attacker).unwrap();
                                        let bone_boxes = geometry.attack_obbs(
                                            attacker,
                                            fighter,
                                            form,
                                            pose.pose_id,
                                            character,
                                            &names,
                                        );
                                        let hurt = geometry.hurt_obbs(target, character);
                                        let body =
                                            fighter_whole_body_obb(target, character, &geometry)
                                                .unwrap();
                                        !bone_boxes
                                            .iter()
                                            .any(|a| hurt.iter().any(|b| obbs_intersect(a, b)))
                                            && geometry
                                                .live_effect_obbs(attacker, character)
                                                .iter()
                                                .any(|a| obbs_intersect(a, &body))
                                    },
                                )
                                .unwrap();
                        if area_only {
                            chosen = Some(position);
                            break 'search;
                        }
                    }
                }
            }
            assert!(
                chosen.is_some(),
                "pose {pose_id} needs a point beyond bone range"
            );
            h.app.world_mut().run_system_once(attack_windows).unwrap();
            let claims = take_hits();
            assert_eq!(claims.len(), 1, "pose {pose_id}, point {chosen:?}");
            assert_eq!(claims[0].1.target_actor_id, 2);
            h.app
                .world_mut()
                .get_mut::<TargetHitClock>(target)
                .unwrap()
                .elapsed = f32::INFINITY;

            let (from, to) = h
                .app
                .world_mut()
                .run_system_once(
                    move |geometry: HitGeometryQueries, assets: Res<CharacterAssetsSet>| {
                        let a = fighter_whole_body_obb(attacker, &assets.0["p1"], &geometry)
                            .unwrap()
                            .center;
                        let b = fighter_whole_body_obb(target, &assets.0["p1"], &geometry)
                            .unwrap()
                            .center;
                        (Vec3::new(a.x, a.y, a.z), Vec3::new(b.x, b.y, b.z))
                    },
                )
                .unwrap();
            let normal = (to - from).normalize();
            let tangent = normal.any_orthonormal_vector() * 50.0;
            let vertical = normal.cross(tangent);
            let middle = (from + to) * 0.5 + tangent * 0.17;
            let mut wall = StageMeshCollider::default();
            wall.append_world_triangles(
                &[
                    (middle - tangent - vertical).to_array(),
                    (middle + tangent - vertical).to_array(),
                    (middle + tangent + vertical).to_array(),
                    (middle - tangent + vertical).to_array(),
                ],
                &[0, 1, 2, 0, 2, 3],
            );
            h.replace_stage_mesh(wall);
            let blocked = h
                .app
                .world_mut()
                .run_system_once(
                    move |geometry: HitGeometryQueries, assets: Res<CharacterAssetsSet>| {
                        let a =
                            fighter_whole_body_obb(attacker, &assets.0["p1"], &geometry).unwrap();
                        let b = fighter_whole_body_obb(target, &assets.0["p1"], &geometry).unwrap();
                        effect_body_occluded(geometry.stage.as_deref(), Some(&a), &b)
                    },
                )
                .unwrap();
            assert!(blocked, "wall fixture {from:?} -> {to:?}");
            h.app.world_mut().run_system_once(attack_windows).unwrap();
            assert!(
                take_hits().is_empty(),
                "{effect_name} blocked by scene wall"
            );
            h.replace_stage_mesh(harness_ground());

            h.app
                .world_mut()
                .get_mut::<PonEfInstance>(effect)
                .unwrap()
                .elapsed = duration + 1.0;
            h.app
                .world_mut()
                .get_mut::<TargetHitClock>(target)
                .unwrap()
                .elapsed = f32::INFINITY;
            h.app.world_mut().run_system_once(attack_windows).unwrap();
            assert!(take_hits().is_empty(), "expired {effect_name} cannot hit");
        }
    }

    #[test]
    fn production_blade_ultimate_preserves_visual_root_through_launch_and_recovery() {
        use crate::components::fighter::VisualRootPin;
        let mut h = spawn_gameplay_harness(8).expect("original resources");
        h.app
            .world_mut()
            .entity_mut(h.fighter)
            .insert(VisualRootPin::default());
        crate::plugins::character_plugin::spawn_fighter_model_for_test(
            h.app.world_mut(),
            h.fighter,
        );
        crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
        h.app.world_mut().get_mut::<Qi>(h.fighter).unwrap().current = 100.0;
        for key in [KeyCode::KeyA, KeyCode::KeyD] {
            h.press_key(key);
            h.step();
            h.release_key(key);
            h.step();
        }
        h.press_key(KeyCode::KeyS);
        h.press_mouse(MouseButton::Left);
        h.step();
        h.release_key(KeyCode::KeyS);
        h.release_mouse(MouseButton::Left);
        assert_eq!(h.pose_id(), 310);
        let baseline = h
            .app
            .world()
            .get::<VisualRootPin>(h.fighter)
            .unwrap()
            .baseline;
        let mut previous = 0;
        let mut visited = Vec::new();
        for tick in 0..450 {
            h.step();
            crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
            if previous != h.pose_id() {
                h.attack_trace(&format!("blade/{tick}"));
                visited.push(h.pose_id());
                previous = h.pose_id();
            }
            if (310..=314).contains(&h.pose_id()) || h.pose_id() == 331 {
                let pin = h.app.world().get::<VisualRootPin>(h.fighter).unwrap();
                assert!(
                    pin.active,
                    "blade pose {} lost motion-root ownership at tick {tick}",
                    h.pose_id()
                );
                assert!(
                    (pin.baseline[0] - baseline[0]).abs() < 0.01
                        && (pin.baseline[1] - baseline[1]).abs() < 0.01,
                    "blade pose {} teleported visual root from {baseline:?} to {:?} at tick {tick}",
                    h.pose_id(),
                    pin.baseline
                );
            }
        }
        assert_eq!(visited, [310, 311, 312, 313, 314, 331, 0]);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn production_spear_hit_locks_immediately_and_freezes_entry_frames() {
        use bevy::ecs::system::RunSystemOnce;
        for fps in [30, 60, 120] {
            let dt = 1.0 / fps as f32;
            let (mut h, target, _) = spawn_local_stock_hit_harness(7);
            h.app.insert_resource(NetState::default());
            for entity in [h.fighter, target] {
                h.app
                    .world_mut()
                    .entity_mut(entity)
                    .insert(crate::components::fighter::VisualRootPin::default());
            }
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = 0.0;
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw = 0.0;
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = Vec3::new(0.0, 0.0, 4.0);
            h.press_key(KeyCode::KeyW);
            h.press_mouse(MouseButton::Left);
            let mut accepted = false;
            for tick in 0..fps {
                h.step_dt(dt);
                if tick == 0 {
                    h.release_key(KeyCode::KeyW);
                    h.release_mouse(MouseButton::Left);
                }
                crate::plugins::character_plugin::drive_fighter_animation_for_test(
                    h.app.world_mut(),
                );
                h.app.world_mut().run_system_once(attack_windows).unwrap();
                if h.app
                    .world()
                    .get::<FighterHitReaction>(target)
                    .unwrap()
                    .current
                    .is_some()
                {
                    accepted = true;
                    break;
                }
            }
            assert!(accepted, "spear front A must hit at {fps} FPS");
            let state = h.app.world().get::<SmState>(target).unwrap();
            assert_eq!(state.current, FighterState::BeHit);
            assert!(state.locked);
            let pose = h.app.world().get::<PoseRuntime>(target).unwrap();
            assert_eq!(pose.pose_id, 100);
            let start = pose.start as f32;
            assert_eq!(pose.frame, start);
            assert!(
                (h.app.world().get::<EntryBlend>(target).unwrap().duration - 0.12).abs() < 1e-6
            );
            for _ in 0..(0.1 * fps as f32) as usize {
                h.step_dt(dt);
                assert_eq!(
                    h.app.world().get::<PoseRuntime>(target).unwrap().frame,
                    start,
                    "BeHit must not advance authored launch during entry fade at {fps} FPS"
                );
            }
            for _ in 0..(0.12 * fps as f32).ceil() as usize + 3 {
                h.step_dt(dt);
            }
            assert!(
                h.app.world().get::<PoseRuntime>(target).unwrap().frame > start,
                "reaction must resume after the fade at {fps} FPS"
            );
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn production_blade_single_a_close_angles_hits_once() {
        for fps in [30, 60, 120] {
            for degrees in [-90.0_f32, -45.0, 0.0, 45.0, 90.0] {
                let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(8, true);
                h.app.insert_resource(NetState::default());
                h.app
                    .world_mut()
                    .entity_mut(target)
                    .insert(WeaponForm::default());
                for entity in [h.fighter, target] {
                    h.app
                        .world_mut()
                        .entity_mut(entity)
                        .insert(crate::components::fighter::VisualRootPin::default());
                    h.app.world_mut().get_mut::<Health>(entity).unwrap().current = 10000.0;
                }
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .facing = 0.0;
                h.app
                    .world_mut()
                    .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                    .yaw = 0.0;
                let angle = degrees.to_radians();
                h.app
                    .world_mut()
                    .get_mut::<Transform>(target)
                    .unwrap()
                    .translation = Vec3::new(angle.sin() * 2.0, 0.0, angle.cos() * 2.0);
                let schedule = super::super::fighter_frame::schedule(&h.app);
                h.app.add_systems(
                    Update,
                    tick_target_hit_clocks.in_set(crate::plugins::FighterFrameSet::PoseDecision),
                );
                h.app.add_systems(
                    schedule,
                    crate::plugins::character_plugin::drive_fighter_animation_for_test
                        .before(harness_retire_entry_blend)
                        .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
                );
                h.app.add_systems(
                    schedule,
                    attack_windows.in_set(crate::plugins::FighterFrameSet::HitJudge),
                );
                h.app.add_systems(
                    schedule,
                    hit_reaction_root_motion
                        .after(crate::plugins::movement_plugin::attack_root_motion)
                        .in_set(crate::plugins::FighterFrameSet::RootMotion),
                );
                h.press_mouse(MouseButton::Left);
                let mut previous_hp = 10000.0;
                let mut hits = Vec::new();
                for tick in 0..fps * 2 {
                    h.step_dt(1.0 / fps as f32);
                    if tick == 0 {
                        h.release_mouse(MouseButton::Left);
                    }
                    let hp = h.app.world().get::<Health>(target).unwrap().current;
                    if hp < previous_hp {
                        let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                        hits.push((tick, pose.pose_id, pose.frame, previous_hp - hp));
                        assert_eq!(pose.pose_id, 295);
                        assert!(
                            (5760.0..5764.0).contains(&pose.frame),
                            "hit outside authored window: fps={fps} angle={degrees} hits={hits:?}"
                        );
                    }
                    previous_hp = hp;
                }
                eprintln!("blade single A fps={fps} angle={degrees} hits={hits:?}");
                assert!(hits.len() <= 1, "fps={fps} angle={degrees} hits={hits:?}");
                if degrees == 0.0 {
                    assert_eq!(hits.len(), 1);
                }
            }
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    #[ignore = "Ninja 464 to 471 spacing diagnostic; writes source-resource simulation traces"]
    fn diagnose_ninja_464_471_spacing() {
        run_ninja_spacing(&[30, 60, 120], &[false, true], &[2.0, 3.0, 4.0, 5.0], false);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn production_ninja_close_slash_connects_launcher() {
        run_ninja_spacing(&[60, 120], &[true], &[2.0, 3.0, 4.0], true);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn run_ninja_spacing(
        frame_rates: &[u32],
        contacts: &[bool],
        distances: &[f32],
        require_chain: bool,
    ) {
        use crate::components::fighter::VisualRootPin;
        let mut rows = String::from("fps,contacts,yaw,distance,tick,source_pose,source_frame,source_x,source_y,source_z,source_yaw,target_pose,target_frame,target_x,target_y,target_z,target_yaw,hp,vx,vz\n");
        for &fps in frame_rates {
            for &contact in contacts {
                for (source_degrees, target_degrees) in [(0.0_f32, 180.0_f32)] {
                    for &distance in distances {
                        let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(12, true);

                        h.app
                            .world_mut()
                            .entity_mut(target)
                            .insert(WeaponForm::default());
                        h.app.insert_resource(NetState::default());
                        h.app
                            .world_mut()
                            .entity_mut(h.fighter)
                            .insert(HeadTrackState::default());
                        h.app.insert_resource(State::new(AppState::Playing));
                        configure_lock_updates(&mut h.app);
                        if contact {
                            h.app
                                .insert_resource(State::new(crate::app_state::AppState::Playing))
                                .add_plugins(
                                    crate::plugins::character_contact_plugin::CharacterContactPlugin,
                                );
                        }
                        for entity in [h.fighter, target] {
                            h.app
                                .world_mut()
                                .entity_mut(entity)
                                .insert(VisualRootPin::default());
                            h.app.world_mut().get_mut::<Health>(entity).unwrap().current = 10000.0;
                        }
                        h.app
                            .world_mut()
                            .get_mut::<Transform>(target)
                            .unwrap()
                            .translation = Vec3::new(0.0, 0.0, distance);
                        h.app
                            .world_mut()
                            .get_mut::<MotionState>(target)
                            .unwrap()
                            .facing = target_degrees.to_radians();
                        h.app
                            .world_mut()
                            .get_mut::<MotionState>(h.fighter)
                            .unwrap()
                            .facing = source_degrees.to_radians();
                        h.app
                            .world_mut()
                            .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                            .yaw = source_degrees.to_radians();
                        let actor_schedule = super::super::fighter_frame::schedule(&h.app);
                        h.app.add_systems(
                            actor_schedule,
                            crate::plugins::character_plugin::drive_fighter_animation_for_test
                                .before(harness_retire_entry_blend)
                                .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
                        );
                        h.app.add_systems(
                            actor_schedule,
                            (tick_target_hit_clocks, attack_windows)
                                .chain()
                                .in_set(crate::plugins::FighterFrameSet::HitJudge),
                        );
                        h.app.add_systems(
                            actor_schedule,
                            hit_reaction_root_motion
                                .after(crate::plugins::movement_plugin::attack_root_motion)
                                .in_set(crate::plugins::FighterFrameSet::RootMotion),
                        );
                        let mut phase = 0;
                        let mut previous_hp = 10000.0;
                        let mut hits = Vec::new();
                        let mut saw_launch = false;
                        let mut source_peak = 0.0_f32;
                        let mut ascent_speed = 0.0_f32;
                        for tick in 0..fps * 3 {
                            h.release_key(KeyCode::KeyW);
                            h.release_key(KeyCode::KeyS);
                            h.release_key(KeyCode::KeyQ);
                            h.release_mouse(MouseButton::Left);
                            let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                            match phase {
                                0 => {
                                    h.press_key(KeyCode::KeyS);
                                    h.press_mouse(MouseButton::Left);
                                    phase = 1;
                                }
                                1 if pose.pose_id == 464 && pose.frame >= 14876.0 => {
                                    h.press_key(KeyCode::KeyS);
                                    phase = 2;
                                }
                                2 => {
                                    h.press_key(KeyCode::KeyW);
                                    h.press_mouse(MouseButton::Left);
                                    phase = 3;
                                }
                                _ => {}
                            }
                            h.step_dt(1.0 / fps as f32);
                            let source_pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                            saw_launch |= source_pose.pose_id == 471;
                            let target_pose = h.app.world().get::<PoseRuntime>(target).unwrap();
                            let s = h
                                .app
                                .world()
                                .get::<Transform>(h.fighter)
                                .unwrap()
                                .translation;
                            let t = h.app.world().get::<Transform>(target).unwrap().translation;
                            let sm = h.app.world().get::<MotionState>(h.fighter).unwrap();
                            if source_pose.pose_id == 471 {
                                source_peak = source_peak.max(s.y);
                                if (15218.0..15221.0).contains(&source_pose.frame) {
                                    ascent_speed = ascent_speed.max(
                                        h.app
                                            .world()
                                            .get::<crate::plugins::movement_plugin::FighterAir>(
                                                h.fighter,
                                            )
                                            .unwrap()
                                            .air
                                            .vertical_speed,
                                    );
                                }
                            }
                            let tm = h.app.world().get::<MotionState>(target).unwrap();
                            let hp = h.app.world().get::<Health>(target).unwrap().current;
                            use std::fmt::Write;
                            writeln!(rows, "{fps},{contact},{source_degrees},{distance},{tick},{},{},{},{},{},{},{},{},{},{},{},{},{hp},{},{}", source_pose.pose_id, source_pose.frame, s.x,s.y,s.z,sm.facing,target_pose.pose_id,target_pose.frame,t.x,t.y,t.z,tm.facing,tm.velocity.x,tm.velocity.z).unwrap();
                            if hp != previous_hp {
                                hits.push((
                                    tick,
                                    source_pose.pose_id,
                                    target_pose.pose_id,
                                    s.distance(t),
                                ));
                                previous_hp = hp;
                            }
                        }
                        assert!(saw_launch, "471 command: fps={fps} contact={contact} distance={distance} phase={phase} hits={hits:?}");
                        if require_chain {
                            assert!((5.5..7.0).contains(&source_peak),
                                "stock launcher height: fps={fps} distance={distance} peak={source_peak}");
                            assert!(
                                (ascent_speed - 24.41806).abs() < 0.01,
                                "stock launcher vertical speed: fps={fps} speed={ascent_speed}"
                            );
                            assert!(hits.iter().any(|hit| hit.1 == 464) && hits.iter().any(|hit| hit.1 == 471),
                                "slash and launcher must both hit: fps={fps} distance={distance} hits={hits:?}");
                        }
                        eprintln!("fps={fps} contacts={contact} source={source_degrees} target={target_degrees} distance={distance} phase={phase} hits={hits:?}");
                    }
                }
            }
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(if require_chain {
            "../../../analysis/ninja-chain-20260919/ninja-spacing-regression.csv"
        } else {
            "../../../analysis/ninja-chain-20260919/ninja-spacing.csv"
        });
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, rows).unwrap();
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    #[ignore = "Spear spacing diagnostic; writes source-resource simulation traces"]
    fn diagnose_spear_spacing_with_character_contacts() {
        use crate::components::fighter::VisualRootPin;
        let mut rows = String::from("contacts,yaw,distance,tick,source_pose,source_frame,source_x,source_y,source_z,source_yaw,target_pose,target_frame,target_x,target_y,target_z,target_yaw,hp,vx,vz\n");
        for contact in [false, true] {
            for (source_degrees, target_degrees) in [
                (45.0_f32, 180.0_f32),
                (0.0, 135.0),
                (0.0, 180.0),
                (-45.0, 180.0),
            ] {
                for distance in [2.0, 3.0, 4.0, 5.0] {
                    let (mut h, target, _) = spawn_local_stock_hit_harness(7);
                    h.app.insert_resource(NetState::default());
                    h.app
                        .world_mut()
                        .entity_mut(h.fighter)
                        .insert(HeadTrackState::default());
                    h.app.insert_resource(State::new(AppState::Playing));
                    configure_lock_updates(&mut h.app);
                    if contact {
                        h.app
                            .insert_resource(State::new(crate::app_state::AppState::Playing))
                            .add_plugins(
                                crate::plugins::character_contact_plugin::CharacterContactPlugin,
                            );
                    }
                    for entity in [h.fighter, target] {
                        h.app
                            .world_mut()
                            .entity_mut(entity)
                            .insert(VisualRootPin::default());
                        h.app.world_mut().get_mut::<Health>(entity).unwrap().current = 10000.0;
                    }
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(target)
                        .unwrap()
                        .translation = Vec3::new(0.0, 0.0, distance);
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(target)
                        .unwrap()
                        .facing = target_degrees.to_radians();
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(h.fighter)
                        .unwrap()
                        .facing = source_degrees.to_radians();
                    h.app
                        .world_mut()
                        .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                        .yaw = source_degrees.to_radians();
                    h.app.add_systems(
                        Update,
                        crate::plugins::character_plugin::drive_fighter_animation_for_test
                            .after(harness_retire_entry_blend)
                            .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
                    );
                    h.app.add_systems(
                        Update,
                        (tick_target_hit_clocks, attack_windows)
                            .chain()
                            .in_set(crate::plugins::FighterFrameSet::HitJudge),
                    );
                    h.app.add_systems(
                        Update,
                        hit_reaction_root_motion
                            .after(crate::plugins::movement_plugin::attack_root_motion)
                            .in_set(crate::plugins::FighterFrameSet::RootMotion),
                    );
                    let mut phase = 0;
                    let mut previous_hp = 10000.0;
                    let mut hits = Vec::new();
                    for tick in 0..240 {
                        h.release_key(KeyCode::KeyW);
                        h.release_key(KeyCode::KeyS);
                        h.release_key(KeyCode::KeyQ);
                        h.release_mouse(MouseButton::Left);
                        let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                        match phase {
                            0 => {
                                h.press_key(KeyCode::KeyW);
                                h.press_mouse(MouseButton::Left);
                                phase = 1;
                            }
                            1 if pose.pose_id == 285 && pose.frame >= 2848.0 => {
                                h.press_key(KeyCode::KeyQ);
                                h.press_key(KeyCode::KeyS);
                                phase = 2;
                            }
                            2 => {
                                phase = 3;
                            }
                            3 => {
                                h.press_key(KeyCode::KeyS);
                                h.press_mouse(MouseButton::Left);
                                phase = 4;
                            }
                            4 if pose.pose_id == 287 && pose.frame >= 2978.0 => {
                                h.press_key(KeyCode::KeyW);
                                h.press_mouse(MouseButton::Left);
                                phase = 5;
                            }
                            _ => {}
                        }
                        h.step();
                        let source_pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                        let target_pose = h.app.world().get::<PoseRuntime>(target).unwrap();
                        let s = h
                            .app
                            .world()
                            .get::<Transform>(h.fighter)
                            .unwrap()
                            .translation;
                        let t = h.app.world().get::<Transform>(target).unwrap().translation;
                        let sm = h.app.world().get::<MotionState>(h.fighter).unwrap();
                        let tm = h.app.world().get::<MotionState>(target).unwrap();
                        let hp = h.app.world().get::<Health>(target).unwrap().current;
                        use std::fmt::Write;
                        writeln!(rows, "{contact},{source_degrees},{distance},{tick},{},{},{},{},{},{},{},{},{},{},{},{},{hp},{},{}", source_pose.pose_id, source_pose.frame, s.x,s.y,s.z,sm.facing,target_pose.pose_id,target_pose.frame,t.x,t.y,t.z,tm.facing,tm.velocity.x,tm.velocity.z).unwrap();
                        if hp != previous_hp {
                            hits.push((
                                tick,
                                source_pose.pose_id,
                                target_pose.pose_id,
                                s.distance(t),
                            ));
                            previous_hp = hp;
                        }
                    }
                    eprintln!("contacts={contact} source={source_degrees} target={target_degrees} distance={distance} phase={phase} hits={hits:?}");
                }
            }
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../analysis/rust-battle-offset-20260910/spear-spacing.csv");
        std::fs::write(path, rows).unwrap();
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    #[ignore = "Fist 417 hit-distance audit; writes original-resource simulation traces"]
    fn diagnose_fist_417_hit_distance() {
        diagnose_fist_hit_distance(417, 100, false, None);
    }

    #[test]
    fn same_pose_act_chain_rebases_root_without_rewind() {
        for fps in [30, 60, 120] {
            for carry in [Vec3::ZERO, Vec3::new(6.0, 0.0, -4.0)] {
                let mut h = spawn_gameplay_harness(7).expect("original P1 assets");
                let (pose, root, group) = {
                    let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
                    let common = &h.app.world().resource::<CommonAmb>().0;
                    assert!(assets.act.edges[&287].contains(&287));
                    let mut pose = PoseRuntime::idle();
                    assets.apply_pose(&mut pose, 287, true);
                    pose.frame = 2995.0;
                    (
                        pose,
                        assets.motion_root_at(common, 287, 2995.0),
                        assets.act.groups.get(&287).copied(),
                    )
                };
                let instance = pose.pose_instance_id;
                let mut sm = SmState::default();
                sm.current = FighterState::Attack1;
                h.app.world_mut().entity_mut(h.fighter).insert((
                    pose.clone(),
                    sm,
                    FighterAttackMotion {
                        active: true,
                        prev_pose_id: 287,
                        prev_pose_instance_id: instance,
                        prev_group: group,
                        prev_root_x: root[0],
                        prev_root_y: root[1],
                        ..default()
                    },
                ));
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .velocity = carry;

                h.press_key(KeyCode::KeyS);
                h.release_key(KeyCode::KeyS);
                h.press_key(KeyCode::KeyS);
                h.press_mouse(MouseButton::Left);
                h.step_dt(1.0 / fps as f32);
                let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap().clone();
                assert_eq!(pose.pose_id, 287);
                assert_ne!(pose.pose_instance_id, instance);
                assert_eq!(
                    h.app.world().get::<EntryBlend>(h.fighter).unwrap().kind,
                    EntryBlendKind::Chained
                );
                h.release_key(KeyCode::KeyS);
                h.release_mouse(MouseButton::Left);
                let expected = {
                    let am = h.app.world().get::<FighterAttackMotion>(h.fighter).unwrap();
                    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                    let dt = 1.0 / fps as f32;
                    Vec3::new(am.carry_x, 0.0, am.carry_z)
                        * (1.0 - super::super::movement_plugin::action_carry_decay(air) * dt)
                            .max(0.0)
                        * dt
                };
                let before = h.position();
                h.step_dt(1.0 / fps as f32);
                let delta = h.position() - before;
                let blend = h.app.world().get::<EntryBlend>(h.fighter).unwrap();
                eprintln!(
                    "SAME_POSE fps={fps} pose=287 source_frame=2995 target_frame={} fade={} delta={delta:?} speed={}",
                    pose.frame,
                    blend.duration,
                    delta.length() * fps as f32
                );
                assert!(
                    (delta - expected).length() < 1e-5,
                    "same-Pose ACT restart must keep only carry: {delta:?} != {expected:?}"
                );
            }
        }
    }

    #[test]
    fn fist_419_guard_reaction_obeys_shared_mover_speed_cap() {
        use bevy::ecs::system::RunSystemOnce;
        for fps in [30, 60, 120] {
            let mut h = spawn_gameplay_harness(6).expect("original P1 assets");
            let (pose, mut reaction, root) = {
                let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
                let common = &h.app.world().resource::<CommonAmb>().0;
                let available: Vec<_> = assets.poses.keys().copied().collect();
                assert_eq!(
                    meteor_combat::hit_reaction::guard_reaction_pose_id(0, 0, &available, 0),
                    Some(60)
                );
                let mut pose = PoseRuntime::idle();
                assets.apply_pose(&mut pose, 60, true);

                pose.frame = 8480.25;
                let mut reaction = getup_reaction_for_pose(60, pose.start, pose.end);
                reaction.kind = HitReactKind::Guard;
                reaction.motion_scale = assets.poses[&419].attacks[0].defense_move;
                (pose, reaction, assets.motion_root_at(common, 60, 8480.0))
            };
            reaction.minimum_remaining = 1.0;
            let mut runtime = FighterHitReaction::default();
            runtime.begin(reaction, false);
            runtime.root_started = true;
            runtime.root_prev = Some(root);
            runtime.root_pose_id = Some(60);
            h.app
                .world_mut()
                .entity_mut(h.fighter)
                .insert((pose, runtime));
            h.app
                .world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / fps as f32));
            let before = h.position();
            h.app
                .world_mut()
                .run_system_once(hit_reaction_root_motion)
                .unwrap();
            let delta = h.position() - before;
            let speed = delta.length() * fps as f32;
            eprintln!(
                "GUARD_CAP fps={fps} attack=419 reaction=60 defense_move=8 delta={delta:?} speed={speed} cap={CARRY_MAX_SPEED}"
            );
            assert!(
                (speed - CARRY_MAX_SPEED).abs() < 1e-3,
                "authored DefenseMove=8 must enter the shared 72 world/s mover: {speed}"
            );
        }
    }

    #[test]
    fn reaction_mover_caps_composed_xyz_and_preserves_direction() {
        use bevy::ecs::system::RunSystemOnce;
        for fps in [30, 60, 120] {
            for magnitude in [1.0, 8.0] {
                for gravity in [false, true] {
                    let dt = 1.0 / fps as f32;
                    let mut h = spawn_gameplay_harness(11).expect("original P1 assets");
                    let (pose, previous, root_velocity) = {
                        let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
                        let common = &h.app.world().resource::<CommonAmb>().0;
                        let mut pose = PoseRuntime::idle();
                        assets.apply_pose(&mut pose, if gravity { 103 } else { 100 }, true);
                        let root = assets.motion_root_at(common, pose.pose_id, pose.frame.floor());
                        let next =
                            assets.motion_root_at(common, pose.pose_id, pose.frame.floor() + 1.0);
                        let rate = 30.0 * meteor_core::MODEL_SCALE * magnitude;
                        let velocity =
                            Vec3::new(next[0] - root[0], next[2] - root[2], root[1] - next[1])
                                * rate;
                        (pose, root, velocity)
                    };

                    let pose_id = pose.pose_id;
                    let mut reaction = getup_reaction_for_pose(pose_id, pose.start, pose.end);
                    reaction.kind = HitReactKind::BeHit;
                    reaction.motion_scale = magnitude;
                    let mut runtime = FighterHitReaction::default();
                    runtime.begin(reaction, gravity);
                    runtime.root_started = true;
                    runtime.root_prev = Some(previous);
                    runtime.root_pose_id = Some(pose_id);
                    runtime.carry_x = 5.0;
                    runtime.carry_z = -3.0;
                    runtime.vertical_velocity = if gravity { -90.0 } else { 0.0 };
                    h.app
                        .world_mut()
                        .entity_mut(h.fighter)
                        .insert((pose, runtime));
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(h.fighter)
                        .unwrap()
                        .facing = 0.0;
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(h.fighter)
                        .unwrap()
                        .translation
                        .y = if gravity { 20.0 } else { 0.0 };
                    let decay = {
                        let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                        air.ground_external_velocity = Vec3::new(2.0, 0.0, 4.0);
                        super::super::movement_plugin::action_carry_decay(&air)
                    };
                    let mut velocity = root_velocity
                        + Vec3::new(5.0, 0.0, -3.0) * (1.0 - decay * dt).max(0.0)
                        + Vec3::new(2.0, 0.0, 4.0);
                    if gravity {
                        velocity.y = -90.0 + meteor_combat::hit_reaction::HIT_REACTION_GRAVITY * dt;
                    }
                    let expected = velocity * (CARRY_MAX_SPEED / velocity.length()).min(1.0) * dt;
                    h.app
                        .world_mut()
                        .resource_mut::<Time>()
                        .advance_by(std::time::Duration::from_secs_f32(dt));
                    let before = h.position();
                    h.app
                        .world_mut()
                        .run_system_once(hit_reaction_root_motion)
                        .unwrap();
                    let actual = h.position() - before;
                    assert!(
                        (actual - expected).length() < 1e-4,
                        "fps={fps} magnitude={magnitude} gravity={gravity}: {actual:?} != {expected:?}"
                    );
                    assert!(actual.length() <= CARRY_MAX_SPEED * dt + 1e-4);
                    let runtime = h.app.world().get::<FighterHitReaction>(h.fighter).unwrap();
                    assert!((runtime.vertical_velocity - expected.y / dt).abs() < 1e-3);
                }
            }
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    #[ignore = "Audit probe: records first reaction sampling across the production frame sets"]
    fn diagnose_chain_audit_hit_frame_sampling() {
        for fps in [30, 60, 120] {
            let dt = 1.0 / fps as f32;
            let (mut h, target, _) = spawn_local_stock_hit_harness(7);
            h.app.insert_resource(NetState::default());
            for entity in [h.fighter, target] {
                h.app
                    .world_mut()
                    .entity_mut(entity)
                    .insert(crate::components::fighter::VisualRootPin::default());
            }
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = 0.0;
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw = 0.0;
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = Vec3::new(0.0, 0.0, 4.0);
            h.app.add_systems(
                Update,
                crate::plugins::character_plugin::drive_fighter_animation_for_test
                    .after(harness_retire_entry_blend)
                    .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
            );
            h.app.add_systems(
                Update,
                (tick_target_hit_clocks, attack_windows)
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::HitJudge),
            );
            h.app.add_systems(
                Update,
                hit_reaction_root_motion
                    .after(crate::plugins::movement_plugin::attack_root_motion)
                    .in_set(crate::plugins::FighterFrameSet::RootMotion),
            );
            h.press_key(KeyCode::KeyW);
            h.press_mouse(MouseButton::Left);
            let mut accepted = false;
            for tick in 0..fps {
                h.step_dt(dt);
                h.release_key(KeyCode::KeyW);
                h.release_mouse(MouseButton::Left);
                if h.app
                    .world()
                    .get::<FighterHitReaction>(target)
                    .unwrap()
                    .current
                    .is_none()
                {
                    continue;
                }
                let pose = h.app.world().get::<PoseRuntime>(target).unwrap();
                assert_eq!(pose.pose_id, 100);
                let blend = h.app.world().get::<EntryBlend>(target).unwrap();
                assert!(blend.duration > 0.0);
                assert!(
                    blend.started_at_seconds.is_none(),
                    "new reaction entered after SkeletonSample"
                );
                h.step_dt(dt);
                let blend = h.app.world().get::<EntryBlend>(target).unwrap();
                assert!(blend.started_at_seconds.is_some());
                eprintln!("HIT_FRAME fps={fps} attack=285 reaction=100 hit_tick={tick} first_sample_tick={} gap_ms={}", tick+1, dt*1000.0);
                accepted = true;
                break;
            }
            assert!(accepted, "real spear front A must hit at {fps} FPS");
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    #[ignore = "Fist 419 spacing audit; writes original-resource simulation traces"]
    fn diagnose_fist_419_hit_distance() {
        diagnose_fist_hit_distance(419, 110, false, None);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    #[ignore = "Fist 419 command/chain spacing audit; writes simulation traces"]
    fn diagnose_fist_419_command_spacing() {
        diagnose_fist_hit_distance(419, 110, true, None);
        diagnose_fist_hit_distance(419, 110, true, Some(14113.0));
        diagnose_fist_hit_distance(419, 110, true, Some(14118.0));
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    #[ignore = "BV1BfBpBREJH reference: real 417/419 input, Q timing and contact traces; not an EXE parity assertion"]
    fn diagnose_fist_video_unlock_chain() {
        diagnose_fist_video_unlock_chain_impl(false);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    #[ignore = "Reference follow-up: repeat Q at each 419 entry with contacts; writes diagnostic traces"]
    fn diagnose_fist_video_repeated_unlock_chain() {
        diagnose_fist_video_unlock_chain_impl(true);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn diagnose_fist_video_unlock_chain_impl(repeat_unlock: bool) {
        use crate::plugins::FighterFrameSet;
        use std::fmt::Write;
        #[derive(Resource)]
        struct WindowTrace {
            source: Entity,
            target: Entity,
            tick: i32,
            prefix: String,
            rows: String,
        }
        fn record_window(
            mut trace: ResMut<WindowTrace>,
            geometry: HitGeometryQueries,
            assets: Res<CharacterAssetsSet>,
            fighters: Query<(
                &Fighter,
                &WeaponForm,
                &PoseRuntime,
                &EntryBlend,
                &TargetHitClock,
                &Transform,
            )>,
        ) {
            let (fighter, form, pose, blend, _, source_pos) = fighters.get(trace.source).unwrap();
            let (target, _, target_pose, _, clock, target_pos) =
                fighters.get(trace.target).unwrap();
            let character = &assets.0[&format!("p{}", fighter.character_id)];
            let Some((_, window)) = character
                .poses
                .get(&pose.pose_id)
                .and_then(|def| active_attack_window(pose.frame, &def.attacks))
            else {
                return;
            };
            if entry_blend_is_active(blend.duration, blend.pose_id, pose.pose_id) {
                return;
            }
            let attack = geometry.attack_obbs(
                trace.source,
                fighter,
                form,
                pose.pose_id,
                character,
                &window.bones,
            );
            let hurt = geometry.hurt_obbs(
                trace.target,
                &assets.0[&format!("p{}", target.character_id)],
            );
            let intersects = attack
                .iter()
                .any(|a| hurt.iter().any(|b| obbs_intersect(a, b)));
            let prefix = trace.prefix.clone();
            let tick = trace.tick;
            writeln!(
                trace.rows,
                "{prefix},{tick},{},{},{},{},{},{intersects},{},{},{},{},{},{},{},{}",
                pose.pose_id,
                pose.frame,
                target_pose.pose_id,
                attack.len(),
                hurt.len(),
                clock.elapsed,
                clock.spawn_elapsed,
                source_pos.translation.x,
                source_pos.translation.y,
                source_pos.translation.z,
                target_pos.translation.x,
                target_pos.translation.y,
                target_pos.translation.z
            )
            .unwrap();
        }
        let mut rows = String::from("contacts,fps,side,late_frames,q_delay,tick,command,pose,frame,blend,locked,x,y,z,yaw,carry_x,carry_z,target_pose,target_frame,target_x,target_y,target_z,hp\n");
        let mut windows = String::from("contacts,fps,side,late_frames,q_delay,tick,pose,frame,target_pose,attack_boxes,hurt_boxes,intersects,hit_clock,spawn_clock,x,y,z,target_x,target_y,target_z\n");
        for contact in [false, true] {
            for fps in [30, 60, 120] {
                for side in [-0.6_f32, 0.6] {
                    for late_frames in [0.0, 5.0] {
                        for q_delay in [-1.0_f32, 0.0, 0.08] {
                            if repeat_unlock && (!contact || q_delay != 0.0) {
                                continue;
                            }
                            let (mut h, target, _) = spawn_local_stock_hit_harness(11);

                            for entity in [h.fighter, target] {
                                h.app
                                    .world_mut()
                                    .entity_mut(entity)
                                    .insert(crate::components::fighter::VisualRootPin::default());
                            }
                            h.app.insert_resource(WindowTrace {
                                source: h.fighter,
                                target,
                                tick: 0,
                                prefix: format!("{contact},{fps},{side},{late_frames},{q_delay}"),
                                rows: String::new(),
                            });
                            h.app.add_systems(
                                Update,
                                record_window
                                    .after(tick_target_hit_clocks)
                                    .before(attack_windows)
                                    .in_set(FighterFrameSet::HitJudge),
                            );
                            h.app.insert_resource(NetState::default());
                            h.app.insert_resource(State::new(AppState::Playing));
                            if contact {
                                h.app.add_plugins(crate::plugins::character_contact_plugin::CharacterContactPlugin);
                            }
                            h.app
                                .world_mut()
                                .entity_mut(h.fighter)
                                .insert(HeadTrackState::default());
                            configure_lock_updates(&mut h.app);
                            crate::plugins::camera_plugin::install_follow_camera_for_test(
                                &mut h.app,
                            );
                            h.app.add_systems(
                                Update,
                                crate::plugins::character_plugin::drive_fighter_animation_for_test
                                    .before(harness_retire_entry_blend)
                                    .in_set(FighterFrameSet::SkeletonSample),
                            );
                            h.app.add_systems(
                                Update,
                                (tick_target_hit_clocks, attack_windows)
                                    .chain()
                                    .in_set(FighterFrameSet::HitJudge),
                            );
                            h.app.add_systems(
                                Update,
                                hit_reaction_root_motion
                                    .after(crate::plugins::movement_plugin::attack_root_motion)
                                    .in_set(FighterFrameSet::RootMotion),
                            );
                            h.app
                                .world_mut()
                                .get_mut::<Transform>(h.fighter)
                                .unwrap()
                                .translation
                                .x = side;
                            h.app
                                .world_mut()
                                .get_mut::<Transform>(target)
                                .unwrap()
                                .translation = Vec3::new(0.0, 0.0, 2.2);
                            h.app.world_mut().get_mut::<Health>(target).unwrap().current = 10000.0;

                            {
                                let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                                lock.system.lock_target = Some(target.to_bits());
                                lock.lock_entity = Some(target);
                            }
                            let mut pending_input: Option<(u16, i32)> = None;
                            let mut sent_for_pose = false;
                            let mut previous_pose = 0;
                            let mut first_419 = None;
                            let mut q_sent = false;
                            let mut previous_hp = 10000.0;
                            let mut hits = Vec::new();
                            let mut sequence = Vec::new();
                            let dt = 1.0 / fps as f32;
                            for tick in 0..fps * 4 {
                                let waiting_direction =
                                    pending_input.is_some_and(|(_, due)| tick < due);
                                if !waiting_direction {
                                    h.release_key(KeyCode::KeyS);
                                    h.release_key(KeyCode::KeyW);
                                }
                                h.release_key(KeyCode::KeyQ);
                                h.release_mouse(MouseButton::Left);
                                let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                                let pose_id = pose.pose_id;
                                let frame = pose.frame;
                                if pose_id != previous_pose {
                                    sequence.push((tick, pose_id));
                                    sent_for_pose = false;
                                    previous_pose = pose_id;
                                    if pose_id == 419 && (first_419.is_none() || repeat_unlock) {
                                        first_419 = Some(tick);
                                        q_sent = false;
                                    }
                                }
                                let mut command = "";
                                if pending_input.is_some_and(|(_, due)| tick >= due) {
                                    let (next, _) = pending_input.take().unwrap();
                                    h.press_key(if next == 417 {
                                        KeyCode::KeyW
                                    } else {
                                        KeyCode::KeyS
                                    });
                                    h.press_mouse(MouseButton::Left);
                                    command = if next == 417 { "W+A" } else { "S+A" };
                                } else if pending_input.is_none() {
                                    let window = h.app.world().resource::<CharacterAssetsSet>().0
                                        ["p1"]
                                        .poses
                                        .get(&pose_id)
                                        .and_then(|def| def.next_pose.as_ref())
                                        .and_then(|next| next.start)
                                        .map(|start| start as f32);
                                    if tick == 0
                                        || (!sent_for_pose
                                            && matches!(pose_id, 417 | 419)
                                            && window
                                                .is_some_and(|start| frame >= start + late_frames))
                                    {
                                        let next = if tick == 0 || pose_id == 419 {
                                            417
                                        } else {
                                            419
                                        };
                                        h.press_key(if next == 417 {
                                            KeyCode::KeyS
                                        } else {
                                            KeyCode::KeyW
                                        });
                                        command = if next == 417 { "S" } else { "W" };

                                        pending_input = Some((next, tick + fps / 30));
                                        sent_for_pose = true;
                                    }
                                }
                                if !q_sent
                                    && q_delay >= 0.0
                                    && first_419
                                        .is_some_and(|first| (tick - first) as f32 * dt >= q_delay)
                                {
                                    h.press_key(KeyCode::KeyQ);
                                    q_sent = true;
                                    command = if command.is_empty() {
                                        "Q"
                                    } else {
                                        "attack_input+Q"
                                    };
                                }
                                h.app.world_mut().resource_mut::<WindowTrace>().tick = tick;
                                h.step_dt(dt);
                                let world = h.app.world();
                                let p = world.get::<PoseRuntime>(h.fighter).unwrap();
                                let tp = world.get::<PoseRuntime>(target).unwrap();
                                let s = h.position();
                                let t = world.get::<Transform>(target).unwrap().translation;
                                let motion = world.get::<MotionState>(h.fighter).unwrap();
                                let carry = world.get::<FighterAttackMotion>(h.fighter).unwrap();
                                let blend = world.get::<EntryBlend>(h.fighter).unwrap().duration;
                                let locked =
                                    world.resource::<LockOnController>().lock_entity.is_some();
                                let hp = world.get::<Health>(target).unwrap().current;
                                if hp != previous_hp {
                                    hits.push((tick, p.pose_id, tp.pose_id));
                                    previous_hp = hp;
                                }
                                writeln!(rows,"{contact},{fps},{side},{late_frames},{q_delay},{tick},{command},{},{},{blend},{locked},{},{},{},{},{},{},{},{},{},{},{},{hp}",
                                p.pose_id,p.frame,s.x,s.y,s.z,motion.facing,carry.carry_x,carry.carry_z,tp.pose_id,tp.frame,t.x,t.y,t.z).unwrap();
                            }
                            eprintln!("contacts={contact} fps={fps} side={side} late={late_frames} q={q_delay} q_sent={q_sent} sequence={sequence:?} hits={hits:?}");
                            windows.push_str(&h.app.world().resource::<WindowTrace>().rows);
                            assert!(
                                sequence.iter().any(|(_, p)| *p == 417),
                                "real S,W+A must reach 417"
                            );
                        }
                    }
                }
            }
        }
        let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../analysis/fist-video-reference-20260915");
        std::fs::create_dir_all(&output).unwrap();
        let stem = if repeat_unlock {
            "repeated-unlock-chain"
        } else {
            "rust-unlock-chain"
        };
        let window_file = if repeat_unlock {
            "repeated-hit-windows.csv"
        } else {
            "hit-windows.csv"
        };
        std::fs::write(output.join(format!("{stem}.csv")), rows).unwrap();
        std::fs::write(output.join(window_file), windows).unwrap();
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn diagnose_fist_hit_distance(
        attack_pose: u16,
        reaction_pose: u16,
        command_input: bool,
        chain_frame: Option<f32>,
    ) {
        use crate::components::fighter::VisualRootPin;
        use crate::plugins::FighterFrameSet;
        use std::fmt::Write;

        let mut rows = String::from(
            "fps,contacts,distance,tick,source_pose,source_frame,source_x,source_y,source_z,target_pose,target_frame,target_x,target_y,target_z,hp,carry_x,carry_z,airborne\n",
        );
        let distances: &[f32] = if command_input {
            &[-3.0, -2.0, 2.0]
        } else if attack_pose == 419 {
            &[-4.0, -3.0, -2.0, -1.0, 1.0, 2.0, 3.0, 4.0]
        } else {
            &[2.0, 3.0, 4.0]
        };
        for fps in [30, 60, 120] {
            for contact in [false, true] {
                for &distance in distances {
                    let (mut h, target, _) = spawn_local_stock_hit_harness(11);
                    h.app.insert_resource(NetState::default());
                    h.app.insert_resource(State::new(AppState::Playing));
                    if command_input {
                        h.app
                            .world_mut()
                            .entity_mut(h.fighter)
                            .insert(HeadTrackState::default());
                        configure_lock_updates(&mut h.app);
                    }
                    if contact {
                        h.app.add_plugins(
                            crate::plugins::character_contact_plugin::CharacterContactPlugin,
                        );
                    }
                    for entity in [h.fighter, target] {
                        h.app
                            .world_mut()
                            .entity_mut(entity)
                            .insert(VisualRootPin::default());
                        h.app.world_mut().get_mut::<Health>(entity).unwrap().current = 10000.0;
                    }
                    let visible_roots: Vec<_> = {
                        use crate::plugins::character_plugin::{FighterDummy, FighterModel};
                        let world = h.app.world_mut();
                        let models: Vec<_> = world
                            .query::<(Entity, &FighterModel)>()
                            .iter(world)
                            .map(|(entity, model)| (entity, model.fighter))
                            .collect();
                        world
                            .query::<(Entity, &FighterDummy)>()
                            .iter(world)
                            .filter(|(_, dummy)| dummy.name == "d_base")
                            .map(|(entity, dummy)| {
                                let fighter = models
                                    .iter()
                                    .find(|(model, _)| *model == dummy.model)
                                    .unwrap()
                                    .1;
                                (fighter, entity)
                            })
                            .collect()
                    };
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(target)
                        .unwrap()
                        .translation = Vec3::new(0.0, 0.0, distance);
                    if !command_input {
                        let mut pose = PoseRuntime::idle();
                        h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(
                            &mut pose,
                            attack_pose,
                            true,
                        );
                        h.app.world_mut().entity_mut(h.fighter).insert((
                            pose,
                            SmState {
                                current: FighterState::Attack1,
                                attack_stage: 1,
                                locked: true,
                                ..default()
                            },
                        ));
                    }
                    h.app.add_systems(
                        Update,
                        crate::plugins::character_plugin::drive_fighter_animation_for_test
                            .before(harness_retire_entry_blend)
                            .in_set(FighterFrameSet::SkeletonSample),
                    );
                    h.app.add_systems(
                        Update,
                        (tick_target_hit_clocks, attack_windows)
                            .chain()
                            .in_set(FighterFrameSet::HitJudge),
                    );
                    h.app.add_systems(
                        Update,
                        hit_reaction_root_motion
                            .after(crate::plugins::movement_plugin::attack_root_motion)
                            .in_set(FighterFrameSet::RootMotion),
                    );
                    let mut previous_hp = 10000.0;
                    let mut hits = Vec::new();
                    let mut peak_y = 0.0_f32;
                    let mut hit_position = None;
                    let mut previous_target = Vec3::new(0.0, 0.0, distance);
                    let mut hit_planar_path = 0.0;
                    let mut source_min_z = 0.0_f32;
                    let mut source_max_z = 0.0_f32;
                    let mut command_phase = 0;
                    let mut pose_history = Vec::new();
                    for tick in 0..fps * 3 {
                        if command_input {
                            h.release_key(KeyCode::KeyW);
                            h.release_key(KeyCode::KeyS);
                            h.release_mouse(MouseButton::Left);
                            let source = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                            match command_phase {
                                0 => {
                                    h.press_key(KeyCode::KeyW);
                                    command_phase = 1;
                                }
                                1 => {
                                    h.press_key(KeyCode::KeyS);
                                    h.press_mouse(MouseButton::Left);
                                    command_phase = 2;
                                }
                                2 if chain_frame.is_some_and(|frame| {
                                    source.pose_id == 419 && source.frame >= frame
                                }) =>
                                {
                                    h.press_key(KeyCode::KeyS);
                                    command_phase = 3;
                                }
                                3 => {
                                    h.press_key(KeyCode::KeyW);
                                    h.press_mouse(MouseButton::Left);
                                    command_phase = 4;
                                }
                                _ => {}
                            }
                        }
                        h.step_dt(1.0 / fps as f32);
                        let world = h.app.world();
                        let source_pose = world.get::<PoseRuntime>(h.fighter).unwrap();
                        let target_pose = world.get::<PoseRuntime>(target).unwrap();
                        if pose_history.last().map(|(_, pose)| *pose) != Some(source_pose.pose_id) {
                            pose_history.push((tick, source_pose.pose_id));
                        }
                        let s = world.get::<Transform>(h.fighter).unwrap().translation;
                        let t = world.get::<Transform>(target).unwrap().translation;
                        let hp = world.get::<Health>(target).unwrap().current;
                        let reaction = world.get::<FighterHitReaction>(target).unwrap();
                        for &(fighter, dummy) in &visible_roots {
                            let pin = world.get::<VisualRootPin>(fighter).unwrap();
                            let visible_pose = world.get::<PoseRuntime>(fighter).unwrap();
                            if pin.active && pin.pose_id == visible_pose.pose_id {
                                let root = world.get::<Transform>(dummy).unwrap().translation;
                                assert!(
                                    (root - Vec3::from_array(pin.baseline)).length() < 1e-5,
                                    "visible d_base must not add authored movement a second time"
                                );
                            }
                        }
                        if attack_pose == 419
                            && !command_input
                            && !contact
                            && source_pose.pose_id == 419
                        {
                            let character = &world.resource::<CharacterAssetsSet>().0["p1"];
                            let common = &world.resource::<CommonAmb>().0;
                            let root = character.motion_root_at(common, 419, source_pose.frame);
                            let start =
                                character.motion_root_at(common, 419, source_pose.start as f32);
                            let (x, z) = meteor_anim::root_motion::root_delta_to_world_xz(
                                root[0] - start[0],
                                root[1] - start[1],
                                0.0,
                                1.0,
                            );
                            assert!(
                                (s - Vec3::new(x, 0.0, z)).length() < 1e-4,
                                "419 world position must follow the original curve at every sampled frame"
                            );
                        }
                        source_min_z = source_min_z.min(s.z);
                        source_max_z = source_max_z.max(s.z);
                        peak_y = peak_y.max(t.y);
                        if hp != previous_hp {
                            hits.push((tick, source_pose.pose_id, target_pose.pose_id));
                            hit_position.get_or_insert(previous_target);
                            assert_eq!(
                                target_pose.pose_id,
                                if source_pose.pose_id == 417 {
                                    100
                                } else {
                                    reaction_pose
                                }
                            );
                            assert_eq!(reaction.current.as_ref().unwrap().motion_scale, 1.0);
                            previous_hp = hp;
                        }
                        if hit_position.is_some() {
                            hit_planar_path +=
                                Vec2::new(t.x - previous_target.x, t.z - previous_target.z)
                                    .length();
                        }
                        previous_target = t;
                        writeln!(rows, "{fps},{contact},{distance},{tick},{},{},{},{},{},{},{},{},{},{},{hp},{},{},{}",
                            source_pose.pose_id, source_pose.frame, s.x, s.y, s.z,
                            target_pose.pose_id, target_pose.frame, t.x, t.y, t.z,
                            reaction.carry_x, reaction.carry_z, reaction.airborne).unwrap();
                    }
                    eprintln!(
                        "pose={attack_pose} command={command_input} chain={chain_frame:?} fps={fps} contacts={contact} distance={distance} poses={pose_history:?} hits={hits:?} hit_position={hit_position:?} final={previous_target:?} peak_y={peak_y} hit_planar_path={hit_planar_path} source_final={:?} source_z_range={source_min_z}..{source_max_z}",
                        h.position()
                    );
                    if command_input {
                        assert!(
                            pose_history.iter().any(|(_, pose)| *pose == 419),
                            "real forward-back-A must enter 419"
                        );
                        if chain_frame.is_some() {
                            assert!(
                                pose_history.iter().any(|(_, pose)| *pose == 417),
                                "real back-forward-A must chain to 417"
                            );
                        }
                    }
                    if attack_pose == 419 && !contact && chain_frame.is_none() {
                        assert_eq!(
                            previous_target,
                            Vec3::new(0.0, 0.0, distance),
                            "419 -> 110 must not inject horizontal or vertical knockback"
                        );
                        if !command_input {
                            assert!((h.position().z + 1.1832496).abs() < 1e-4);
                        }
                    }
                    if attack_pose == 419 && fps == 60 && distance == -2.0 {
                        assert!(hits
                            .iter()
                            .any(|(_, pose, reaction)| *pose == 419 && *reaction == 110));
                    }
                    if attack_pose == 417 {
                        assert_eq!(
                            hits.len(),
                            1,
                            "single {attack_pose} should land exactly once"
                        );
                    } else if chain_frame.is_none() {
                        assert!(hits.len() <= 1, "single 419 must not hit twice");
                    }
                }
            }
        }
        let suffix = if command_input {
            format!("-command-chain{}", chain_frame.unwrap_or(0.0))
        } else {
            String::new()
        };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../../analysis/rust-slide-audit-20260914/fist-hit{attack_pose}{suffix}-runtime.csv"
        ));
        std::fs::write(path, rows).unwrap();
    }

    #[test]
    fn original_spear_diagonal_hits_use_literal_exe_thresholds() {
        use meteor_combat::hit_reaction::{facing_bucket, pick_reaction_pose_id};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root),
            "p0",
        )
        .expect("original p0 assets");
        let poses: Vec<u16> = character.poses.keys().copied().collect();
        for (x, z, expected) in [
            (1.0, 1.0, [95, 99]),
            (-1.0, 1.0, [99, 95]),
            (1.0, -1.0, [98, 97]),
            (-1.0, -1.0, [98, 97]),
        ] {
            let bucket = facing_bucket(x, z, 0.0, 0.0, 0.0);
            for (attack, pose) in character.poses[&287].attacks.iter().zip(expected) {
                assert_eq!(pick_reaction_pose_id(attack, bucket, &poses), Some(pose));
            }
        }
    }

    #[test]
    fn production_spear_air_down_command_reaches_second_stage() {
        let mut h = spawn_gameplay_harness(7).expect("original resources");
        let keys = crate::app_state::Keymap::default();
        h.press_binding(keys.jump);
        h.step();
        h.release_binding(keys.jump);
        h.steps(4);
        h.press_binding(keys.move_down);
        h.press_binding(keys.attack);
        h.step();
        assert_eq!(h.pose_id(), 282);
        h.release_binding(keys.attack);
        h.release_binding(keys.move_down);
        h.steps(2);
        h.press_binding(keys.move_down);
        h.press_binding(keys.attack);
        h.step();
        h.release_binding(keys.attack);
        h.release_binding(keys.move_down);
        assert!(
            h.app
                .world()
                .get::<SmState>(h.fighter)
                .unwrap()
                .attack_queued
                .is_some(),
            "second air-down input must queue 283"
        );
        let mut saw_second = false;
        for _ in 0..150 {
            h.step();
            saw_second |= h.pose_id() == 283;
        }
        assert!(
            saw_second,
            "touchdown must preserve the queued second stage"
        );
        assert_eq!(h.state(), FighterState::Name);
        assert!(!h.airborne());
    }

    #[test]
    fn production_fake_held_guard_and_direction_recovery() {
        for direction in [false, true] {
            let mut h = spawn_gameplay_harness(7).expect("original resources");
            let keys = crate::app_state::Keymap::default();
            h.press_binding(keys.fake);
            h.step();
            h.release_binding(keys.fake);
            h.steps(50);
            assert_eq!(h.state(), FighterState::Fake);

            match keys.guard {
                crate::app_state::InputBinding::Key(code) => {
                    h.app
                        .world_mut()
                        .resource_mut::<ButtonInput<KeyCode>>()
                        .press(code);
                    h.app
                        .world_mut()
                        .resource_mut::<ButtonInput<KeyCode>>()
                        .clear();
                }
                crate::app_state::InputBinding::Mouse(button) => {
                    h.app
                        .world_mut()
                        .resource_mut::<ButtonInput<MouseButton>>()
                        .press(button);
                    h.app
                        .world_mut()
                        .resource_mut::<ButtonInput<MouseButton>>()
                        .clear();
                }
            }
            h.app.world_mut().resource_mut::<LocalGuardInput>().held = true;
            if direction {
                h.press_binding(keys.move_up);

                h.press_binding(keys.guard);
            }
            h.step();
            assert_eq!(h.pose_id(), if direction { 176 } else { 35 });
            h.release_binding(keys.move_up);
            h.steps(100);
            assert!(matches!(
                h.state(),
                FighterState::Name | FighterState::Guard
            ));
        }
    }

    #[test]
    fn production_dagger_air_down_hit_keeps_jump_locked_through_down_pose_minimum() {
        for fps in [30, 60, 120] {
            let mut h = spawn_gameplay_harness(4).expect("original resources");
            let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
            let attack = assets.poses[&252].attacks[0].clone();
            assert_eq!(attack.target_pose, Some(102));
            assert_eq!(attack.target_value, 0.4);
            let mut pose = PoseRuntime::idle();
            assets.apply_pose(&mut pose, 102, true);
            let mut reaction = getup_reaction_for_pose(102, pose.start, pose.end);
            reaction.kind = HitReactKind::BeHit;
            reaction.minimum_remaining = attack.target_value;
            reaction.authored_minimum = attack.target_value;
            reaction.motion_scale = attack.target_move;
            h.app.world_mut().entity_mut(h.fighter).insert(pose);
            h.app
                .world_mut()
                .get_mut::<SmState>(h.fighter)
                .unwrap()
                .current = FighterState::BeHit;
            h.app
                .world_mut()
                .get_mut::<CombatState>(h.fighter)
                .unwrap()
                .state = FighterState::BeHit;
            h.app
                .world_mut()
                .get_mut::<FighterHitReaction>(h.fighter)
                .unwrap()
                .begin(reaction, false);
            let dt = 1.0 / fps as f32;
            for _ in 0..fps {
                h.step_dt(dt);
                if h.pose_id() == 113 {
                    break;
                }
            }
            assert_eq!(h.pose_id(), 113);
            assert_eq!(
                h.state(),
                FighterState::BeHit,
                "{fps} fps: down pose entered too early"
            );
            let keys = crate::app_state::Keymap::default();
            for _ in 0..fps / 5 {
                h.press_binding(keys.jump);
                h.step_dt(dt);
                h.release_binding(keys.jump);
                assert_eq!(
                    h.state(),
                    FighterState::BeHit,
                    "{fps} fps: jump bypassed minimum"
                );
                assert!(
                    !h.app
                        .world()
                        .get::<FighterAir>(h.fighter)
                        .unwrap()
                        .air
                        .airborne
                );
            }
            for _ in 0..fps {
                h.step_dt(dt);
                if h.state() == FighterState::Down {
                    break;
                }
            }
            assert_eq!(h.state(), FighterState::Down);
            h.press_binding(keys.jump);
            h.step_dt(dt);
            h.release_binding(keys.jump);
            h.step_dt(dt);
            assert_eq!(
                h.state(),
                FighterState::Jump,
                "actual Down must still admit jump"
            );
        }
    }

    #[test]
    fn production_stinger_linked_reactions_reach_down_and_recover() {
        for reaction_pose in [114, 115, 95, 99, 103, 106] {
            let mut h = spawn_gameplay_harness(4).expect("original resources");
            let mut pose = PoseRuntime::idle();
            h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(
                &mut pose,
                reaction_pose,
                true,
            );
            let mut reaction = getup_reaction_for_pose(reaction_pose, pose.start, pose.end);
            reaction.kind = HitReactKind::BeHit;
            reaction.minimum_remaining = 0.2;
            reaction.authored_minimum = 0.2;
            h.app.world_mut().entity_mut(h.fighter).insert(pose);
            h.app
                .world_mut()
                .get_mut::<SmState>(h.fighter)
                .unwrap()
                .current = FighterState::BeHit;
            h.app
                .world_mut()
                .get_mut::<CombatState>(h.fighter)
                .unwrap()
                .state = FighterState::BeHit;
            h.app
                .world_mut()
                .get_mut::<FighterHitReaction>(h.fighter)
                .unwrap()
                .begin(reaction, false);
            for _ in 0..180 {
                h.step();
                if h.state() == FighterState::Down {
                    break;
                }
            }
            assert_eq!(
                h.state(),
                FighterState::Down,
                "reaction {reaction_pose} stuck in pose {}",
                h.pose_id()
            );
            let keys = crate::app_state::Keymap::default();
            h.press_binding(keys.guard);
            h.step();
            assert!(matches!(h.pose_id(), 119 | 120));
            h.steps(90);
            assert_eq!(h.state(), FighterState::Guard);
        }
    }

    #[test]
    fn production_all_authored_target_poses_eventually_release_control() {
        let seed = spawn_gameplay_harness(4).expect("original resources");
        let mut targets = std::collections::BTreeMap::<u16, f32>::new();
        for pose in seed.app.world().resource::<CharacterAssetsSet>().0["p1"]
            .poses
            .values()
        {
            for attack in &pose.attacks {
                for id in [
                    attack.target_pose,
                    attack.target_pose_front,
                    attack.target_pose_back,
                    attack.target_pose_left,
                    attack.target_pose_right,
                ]
                .into_iter()
                .flatten()
                {
                    targets
                        .entry(id)
                        .and_modify(|value| *value = value.max(attack.target_value))
                        .or_insert(attack.target_value);
                }
            }
        }

        targets.insert(111, 0.2);
        assert!(targets.contains_key(&114) && targets.contains_key(&115));
        for (id, minimum) in targets {
            let mut h = spawn_gameplay_harness(4).expect("original resources");
            let mut pose = PoseRuntime::idle();
            h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, id, true);
            assert_eq!(pose.pose_id, id);
            let mut reaction = getup_reaction_for_pose(id, pose.start, pose.end);
            reaction.kind = HitReactKind::BeHit;
            reaction.lock_remaining = reaction.lock_remaining.max(minimum);
            reaction.minimum_remaining = minimum;
            reaction.authored_minimum = minimum;
            h.app.world_mut().entity_mut(h.fighter).insert(pose);
            h.app
                .world_mut()
                .get_mut::<SmState>(h.fighter)
                .unwrap()
                .current = FighterState::BeHit;
            h.app
                .world_mut()
                .get_mut::<CombatState>(h.fighter)
                .unwrap()
                .state = FighterState::BeHit;
            h.app
                .world_mut()
                .get_mut::<FighterHitReaction>(h.fighter)
                .unwrap()
                .begin(reaction, false);
            for _ in 0..600 {
                h.step();
                if matches!(h.state(), FighterState::Name | FighterState::Down) {
                    break;
                }
            }
            assert!(
                matches!(h.state(), FighterState::Name | FighterState::Down),
                "target {id}, minimum {minimum}: stuck {:?} pose {}",
                h.state(),
                h.pose_id()
            );
            if h.state() == FighterState::Down {
                h.press_binding(crate::app_state::Keymap::default().guard);
                h.steps(90);
                assert_eq!(h.state(), FighterState::Guard, "target {id} getup");
            }
        }
    }

    #[test]
    fn every_weapon_maps_to_its_original_sustained_guard_pose() {
        let expected = [
            (1, 11),
            (2, 13),
            (3, 12),
            (4, 14),
            (5, 15),
            (6, 16),
            (7, 17),
            (8, 18),
            (9, 19),
            (10, 481),
            (11, 480),
            (12, 482),
        ];
        for (weapon_id, expected_pose) in expected {
            let strategy_id = weapon_to_strategy(weapon_id);
            assert_eq!(
                guard_pose_for_weapon(strategy_id),
                Some(expected_pose),
                "weapon {weapon_id} strategy {strategy_id}"
            );
        }
    }

    fn install_guard_test_animation(h: &mut GameplayHarness) {
        crate::plugins::character_plugin::spawn_fighter_model_for_test(
            h.app.world_mut(),
            h.fighter,
        );
        let schedule = super::super::fighter_frame::schedule(&h.app);
        h.app.add_systems(
            schedule,
            crate::plugins::character_plugin::drive_fighter_animation_for_test
                .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
        );
    }

    #[test]
    fn production_jump_held_guard_resumes_after_landing() {
        for serial in [false, true] {
            for fps in [30, 60, 120] {
                for release_before_landing in [false, true] {
                    let mut h = spawn_gameplay_harness_with_order(DAGGER, serial)
                        .expect("original character resources");
                    if serial {
                        install_guard_test_animation(&mut h);
                    }
                    let keys = crate::app_state::Keymap::default();
                    let dt = 1.0 / fps as f32;
                    h.press_binding(keys.move_up);
                    h.press_binding(keys.jump);
                    h.step_dt(dt);
                    h.release_binding(keys.jump);
                    h.step_dt(dt);
                    assert!(h.airborne());
                    h.press_binding(keys.guard);
                    h.step_dt(dt);
                    assert_eq!(h.state(), FighterState::Jump);
                    assert!(h.app.world().resource::<LocalGuardInput>().held);
                    if release_before_landing {
                        h.release_binding(keys.guard);
                    }
                    let mut landed = false;
                    for _ in 0..fps * 5 {
                        h.step_dt(dt);
                        if !h.airborne() {
                            landed = true;
                            break;
                        }
                        assert_ne!(h.state(), FighterState::Guard, "no airborne guard");
                    }
                    assert!(landed, "jump must land");

                    h.step_dt(dt);
                    assert_eq!(
                        h.state(),
                        if release_before_landing {
                            FighterState::Name
                        } else {
                            FighterState::Guard
                        },
                        "serial={serial}, fps={fps}, released={release_before_landing}"
                    );
                    if !release_before_landing {
                        assert_eq!(h.pose_id(), 15, "install dagger guard stance");
                        h.steps(3);
                        assert_eq!(h.state(), FighterState::Guard);

                        h.press_binding(keys.jump);
                        h.step_dt(dt);
                        h.release_binding(keys.jump);
                        h.step_dt(dt);
                        assert!(h.airborne(), "guard must still allow jumping");
                        for _ in 0..fps * 5 {
                            h.step_dt(dt);
                            if !h.airborne() {
                                break;
                            }
                            assert_ne!(h.state(), FighterState::Guard);
                        }
                        assert!(!h.airborne());
                        h.step_dt(dt);
                        assert_eq!(h.state(), FighterState::Guard);
                        h.release_binding(keys.guard);
                        h.step_dt(dt);
                        assert_eq!(h.state(), FighterState::Name);
                    }
                }
            }
        }
    }

    #[test]
    fn production_held_guard_waits_for_attack_and_dodge_recovery() {
        for dodge in [false, true] {
            let mut h = spawn_gameplay_harness_with_order(DAGGER, true)
                .expect("original character resources");
            install_guard_test_animation(&mut h);
            let keys = crate::app_state::Keymap::default();
            let action = if dodge { keys.dodge } else { keys.attack };
            h.press_binding(action);
            h.step();
            h.release_binding(action);
            let action_state = h.state();
            assert!(if dodge {
                action_state == FighterState::Dodge
            } else {
                is_attacking_state(action_state)
            });
            h.press_binding(keys.guard);
            h.step();
            assert_eq!(
                h.state(),
                action_state,
                "held guard must not interrupt action"
            );
            for _ in 0..180 {
                if h.state() == FighterState::Guard {
                    break;
                }
                h.step();
            }
            assert_eq!(h.state(), FighterState::Guard, "dodge={dodge}");
            assert_eq!(h.pose_id(), 15);
        }
    }

    #[test]
    fn production_guard_press_installs_weapon_stance_before_pose_advance() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let keymap = crate::app_state::Keymap::default();

        harness.press_binding(keymap.guard);
        harness.step();
        assert_eq!(harness.state(), FighterState::Guard);
        assert_eq!(
            harness.app.world().resource::<HarnessPoseBeforeAdvance>().0,
            Some(15),
            "dagger strategy 1 must sample sustained guard pose 15 on key-down"
        );
        assert_eq!(harness.pose_id(), 15);

        harness.release_binding(keymap.guard);
        harness.step();
        assert_eq!(harness.state(), FighterState::Name);
        assert_eq!(
            harness.app.world().resource::<HarnessPoseBeforeAdvance>().0,
            Some(L::IDLE),
            "guard release must restore Name pose before the same-frame sample"
        );
    }

    #[test]
    fn production_dodge_key_moves_the_fighter_on_the_ground() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let keymap = crate::app_state::Keymap::default();
        harness.trace("idle");
        let start = harness.position();
        assert_eq!(harness.state(), FighterState::Name, "harness starts idle");
        assert!(!harness.airborne(), "harness starts grounded");

        harness.press_binding(keymap.dodge);
        harness.step();
        harness.trace("dodge-frame-0");
        harness.release_binding(keymap.dodge);
        for i in 1..=6 {
            harness.step();
            harness.trace(&format!("dodge-frame-{i}"));
        }

        let moved = (harness.position() - start).length();
        assert!(
            moved > 0.05,
            "桌面闪避键必须让人物真正位移，实际只移动 {moved}"
        );
    }

    #[test]
    fn production_direction_double_tap_dodges_and_moves_the_fighter() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let start = harness.position();

        harness.press_key(KeyCode::KeyW);
        harness.step();
        harness.release_key(KeyCode::KeyW);
        harness.step();
        harness.press_key(KeyCode::KeyW);
        harness.step();
        harness.trace("double-tap");
        harness.release_key(KeyCode::KeyW);
        harness.steps(6);
        harness.trace("double-tap+6");

        assert!(
            matches!(harness.state(), FighterState::Dodge | FighterState::Name),
            "双击方向必须进过 Dodge"
        );
        let moved = (harness.position() - start).length();
        assert!(moved > 0.05, "瞬移必须位移真实人物，实际只移动 {moved}");
    }

    #[test]
    fn production_fake_key_enters_fake_and_direction_rolls_out_of_it() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let keymap = crate::app_state::Keymap::default();

        harness.press_binding(keymap.fake);
        harness.step();
        harness.trace("fake");
        harness.release_binding(keymap.fake);
        harness.step();
        assert_eq!(
            harness.state(),
            FighterState::Fake,
            "假死键必须进入 Fake（EXE 0x004ce100 只允许 Name 进入）"
        );

        let start = harness.position();
        harness.press_key(KeyCode::KeyW);
        for i in 0..20 {
            harness.step();
            harness.trace(&format!("fake-roll-{i}"));
        }
        let moved = (harness.position() - start).length();
        assert!(moved > 0.05, "躺地按方向必须翻滚位移，实际只移动 {moved}");
    }

    #[test]
    fn single_frame_loop_guard_poses_play_to_end_instead_of_pinning() {
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

        let mut guards: Vec<u16> = character
            .poses
            .iter()
            .filter(|(_, def)| {
                def.loops && def.loop_start == def.loop_end && def.end > def.loop_start
            })
            .map(|(id, _)| *id)
            .collect();
        guards.sort_unstable();
        eprintln!(
            "single-frame-guard poses (p1): {} 个 {guards:?}",
            guards.len()
        );

        assert!(
            guards.contains(&340),
            "匕首空中攻击收招 Pose 340 必须是单帧守卫形状（本缺陷的复现锚点）"
        );

        for id in guards {
            let mut pose = PoseRuntime::idle();
            character.apply_pose(&mut pose, id, true);
            assert!(pose.single_frame_loop_guard(), "pose {id} 应识别为单帧守卫");
            assert!(
                pose.plays_to_end(),
                "pose {id} 必须按 play-to-End clip 推进，否则游标永久钉死"
            );
            let def = character.poses.get(&id).expect("pose def");

            let mut guard_ticks = 0;
            while !pose.finished() {
                pose.frame = advance_pose_cursor(def, &pose, HARNESS_DT);
                guard_ticks += 1;
                assert!(
                    guard_ticks < 4096,
                    "pose {id} 游标未推进到 End（钉死在 {}）",
                    pose.frame
                );
            }
        }
    }

    #[test]
    fn production_fist_swap_chains_new_weapon_before_recovery_ends() {
        use meteor_input::command_parser::Dir;

        for (source, direction) in [(412, Dir::Left), (413, Dir::Right)] {
            for (weapon, command, target) in [
                (5, None, 248),
                (6, None, 262),
                (7, None, 279),
                (6, Some(Dir::Up), 266),
                (6, Some(Dir::Down), 267),
            ] {
                let mut h = spawn_gameplay_harness(11).expect("original fist ACT/POS resources");
                h.app
                    .world_mut()
                    .get_mut::<Fighter>(h.fighter)
                    .unwrap()
                    .weapon_slots = [11, weapon];
                let now = h.app.world().resource::<Time>().elapsed_secs_f64() * 1000.0;
                {
                    let mut input = h.app.world_mut().resource_mut::<LocalCommandInput>();
                    input.0.push_direction(direction, now);
                    input.0.release_direction(direction);
                    input.0.push_direction(direction, now);
                    input.0.release_direction(direction);
                }
                h.press_mouse(MouseButton::Left);
                h.step();
                h.release_mouse(MouseButton::Left);
                assert_eq!(h.pose_id(), source);
                assert_eq!(
                    h.app.world().get::<Fighter>(h.fighter).unwrap().weapon,
                    weapon
                );
                h.step();
                if let Some(direction) = command {
                    let now = h.app.world().resource::<Time>().elapsed_secs_f64() * 1000.0;
                    h.app
                        .world_mut()
                        .resource_mut::<LocalCommandInput>()
                        .0
                        .push_direction(direction, now);
                }
                h.press_mouse(MouseButton::Left);
                h.step();
                h.release_mouse(MouseButton::Left);
                assert_eq!(
                    h.pose_id(),
                    source,
                    "registration must retain the source Action"
                );

                for _ in 0..6 {
                    if h.pose_id() != source {
                        break;
                    }
                    h.step();
                }
                assert_eq!(h.pose_id(), target, "{source} must chain weapon {weapon}");
                assert_eq!(h.state(), FighterState::Attack2);
            }
        }
    }

    #[test]
    fn production_dagger_combo_chain_links_poses_and_keeps_advancing() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let start = harness.position();
        let mut poses: Vec<(u16, u32)> = Vec::new();
        let record = |h: &GameplayHarness, poses: &mut Vec<(u16, u32)>| {
            let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
            let entry = (pose.pose_id, pose.pose_instance_id);
            if poses.last() != Some(&entry) {
                poses.push(entry);
            }
        };

        for swing in 0..3 {
            harness.press_mouse(MouseButton::Left);
            harness.step();
            harness.release_mouse(MouseButton::Left);
            for _ in 0..12 {
                harness.step();
                record(&harness, &mut poses);
            }
            harness.trace(&format!("swing{swing}"));
        }
        for _ in 0..90 {
            harness.step();
            record(&harness, &mut poses);
            if harness.state() == FighterState::Name {
                break;
            }
        }
        harness.trace("combo-end");
        eprintln!("pose chain = {poses:?}");

        let distinct: std::collections::BTreeSet<u16> = poses.iter().map(|(id, _)| *id).collect();
        assert!(
            distinct.len() >= 3,
            "连点必须真的推进连招段（至少 3 个不同 Pose），实际链路 {poses:?}"
        );
        assert_eq!(
            harness.state(),
            FighterState::Name,
            "整套连招结束后必须交还控制权（实际 {:?} pose {}）",
            harness.state(),
            harness.pose_id()
        );
        let advanced = (harness.position() - start).length();
        eprintln!("combo advance = {advanced:.3}");
        assert!(
            advanced > 0.5,
            "连招整体应有前压位移（authored d_base + carry），实际只有 {advanced:.3}"
        );
    }

    #[test]
    fn production_dagger_second_light_does_not_jolt_the_follow_camera_anchor() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };

        harness.press_mouse(MouseButton::Left);
        harness.step();
        harness.release_mouse(MouseButton::Left);
        harness.steps(12);
        assert_eq!(harness.pose_id(), 248, "first A must still be Pose 248");

        let before = harness.position();
        harness.press_mouse(MouseButton::Left);
        harness.step();
        harness.release_mouse(MouseButton::Left);
        let after = harness.position();
        let step = (after - before).length();
        let blend = harness
            .app
            .world()
            .get::<EntryBlend>(harness.fighter)
            .unwrap();

        assert_eq!(harness.pose_id(), 249, "second A must commit Pose 249");
        let instance = harness
            .app
            .world()
            .get::<PoseRuntime>(harness.fighter)
            .unwrap()
            .pose_instance_id;
        let (queued_instance, queued_pose, queued_duration) = blend
            .pending_local_chain
            .expect("actual queued attack must publish its discrete network entry");
        assert_eq!((queued_instance, queued_pose), (instance, 249));
        assert!((queued_duration - 0.15).abs() < 1e-6);
        assert_eq!(blend.kind, EntryBlendKind::Chained);
        assert!((blend.duration - 0.15).abs() < 1e-6);
        assert!(
            step < 0.35,
            "second-A commit may keep carry but must not submit the cross-clip bank gap (step={step:.3})"
        );
    }

    #[test]
    fn production_side_jump_preserves_support_for_first_air_acceleration() {
        for left in [true, false] {
            let Some(mut h) = spawn_gameplay_harness(DAGGER) else {
                panic!("original assets required for side-jump regression");
            };
            let keymap = crate::app_state::Keymap::default();
            h.steps(2);
            h.press_binding(if left {
                keymap.move_left
            } else {
                keymap.move_right
            });
            h.steps(12);
            h.press_binding(keymap.jump);
            h.step();
            let before = h
                .app
                .world()
                .get::<MotionState>(h.fighter)
                .unwrap()
                .velocity;
            assert!(
                h.app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .unwrap()
                    .ground_contact
            );
            h.release_binding(keymap.jump);
            h.step();
            assert!(h.airborne());
            let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
            let speed = Vec2::new(air.air.planar_x, air.air.planar_z).length();

            assert!(speed > before.xz().length() + 2.5,
                "side jump lost supported takeoff acceleration: left={left}, before={before:?}, speed={speed}");
            h.steps(3);
            assert!(
                !h.app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .unwrap()
                    .ground_contact,
                "support must clear after leaving the floor"
            );
            h.steps(90);
            assert!(!h.airborne(), "side jump must land");
        }
    }

    #[test]
    fn production_plain_jump_is_ballistic_and_lands_forward() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let keymap = crate::app_state::Keymap::default();
        let start = harness.position();

        harness.press_binding(keymap.move_up);

        harness.steps(12);
        let takeoff_velocity = harness
            .app
            .world()
            .get::<MotionState>(harness.fighter)
            .unwrap()
            .velocity;
        let takeoff_position = harness.position();
        harness.press_binding(keymap.jump);
        harness.step();

        harness.release_binding(keymap.move_up);
        harness.release_binding(keymap.jump);
        harness.step();
        let first_air_position = harness.position();
        let first_air = harness
            .app
            .world()
            .get::<FighterAir>(harness.fighter)
            .unwrap();
        let first_air_speed = Vec2::new(first_air.air.planar_x, first_air.air.planar_z).length();

        assert!(
            takeoff_velocity.xz().length() > 17.0,
            "起跳前应处于原版前跑稳态（≈18.7），实际 velocity={takeoff_velocity:?}"
        );
        assert!(harness.airborne(), "松开跳跃键的同帧必须进入空中");
        assert!(
            first_air_speed > 16.0,
            "起跳必须把地面平面速度继承进 AirState，实际首帧 air speed={first_air_speed:.3}"
        );
        assert!(
            (first_air_position - takeoff_position).xz().length() > 0.35,
            "30 FPS 起跳首个空中 tick 必须提交继承速度产生的平移，实际 displacement={:?}",
            first_air_position - takeoff_position
        );

        let mut apex = 0.0_f32;
        let mut apex_frame = 0usize;
        let mut landed = None;
        let mut fall_ticks = 0usize;
        let mut previous = harness.position();
        let mut max_step = 0.0_f32;
        for i in 0..150 {
            harness.step();
            let position = harness.position();
            max_step = max_step.max((position - previous).length());
            previous = position;
            let y = position.y;
            if y > apex {
                apex = y;
                apex_frame = i;
            }
            if landed.is_none() && i > 3 && !harness.airborne() {
                landed = Some(i);
            }
            let pose = harness
                .app
                .world()
                .get::<PoseRuntime>(harness.fighter)
                .unwrap();
            if harness.airborne() && (pose.pose_id == 152 || pose.is_fall_hold()) {
                fall_ticks += 1;
            }
            if harness.airborne() && pose.is_fall_hold() {
                assert_eq!(
                    pose.frame, pose.start as f32,
                    "fall-hold tail must not play before floor contact"
                );
            }
        }
        let forward = harness.position().z - start.z;
        let inherited_air_forward = harness.position().z - first_air_position.z;
        eprintln!(
            "plain jump: apex={apex:.3} @frame {apex_frame}, landed@{landed:?}, \
             forward={forward:.3}, inherited_air_forward={inherited_air_forward:.3}"
        );

        assert!(apex > 1.0, "平跳必须真的升空，实际顶点只有 {apex:.3}");
        assert!(landed.is_some(), "平跳必须落地");

        assert!(fall_ticks > 1, "jump must enter the falling animation");
        assert!(
            max_step <= meteor_physics::carry_velocity::CARRY_MAX_SPEED * HARNESS_DT + 1e-3,
            "跳跃链任一 tick 都不得超过共享 mover 上限，实际最大位移 {max_step:.3}"
        );
        assert!(
            apex_frame > 0 && apex_frame < landed.unwrap(),
            "顶点必须在落地之前（弹道弧线），apex@{apex_frame} landed@{landed:?}"
        );
        assert!(
            forward > 2.0,
            "按住前进的平跳必须持续平移（原版跳跃保留平面速度），实际 {forward:.3}"
        );
        assert!(
            inherited_air_forward > 5.0,
            "松开前进后，继承的跑速仍须贯穿整个弹道阶段，实际只移动 {inherited_air_forward:.3}"
        );
        assert!(
            (harness.position().y).abs() < 0.05,
            "落地后必须贴地，实际 y={}",
            harness.position().y
        );
    }

    #[test]
    fn production_air_attack_uses_one_composed_mover_step() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let keymap = crate::app_state::Keymap::default();

        harness.press_binding(keymap.move_up);
        harness.steps(3);
        harness.press_binding(keymap.jump);
        harness.step();
        harness.release_binding(keymap.move_up);
        harness.release_binding(keymap.jump);
        harness.step();
        harness.steps(3);
        assert!(harness.airborne(), "测试入口必须已在弹道空中");
        let inherited = harness
            .app
            .world()
            .get::<FighterAir>(harness.fighter)
            .unwrap()
            .air
            .planar_z;
        assert!(
            inherited > 10.0,
            "空中 A 前必须仍有跑跳惯性，实际 {inherited}"
        );

        let before = harness.position();
        harness.press_mouse(MouseButton::Left);
        harness.step();
        harness.release_mouse(MouseButton::Left);
        let accepted = (harness.position() - before) / HARNESS_DT;
        let motion = harness
            .app
            .world()
            .get::<MotionState>(harness.fighter)
            .unwrap()
            .velocity;
        let attack = harness
            .app
            .world()
            .get::<FighterAttackMotion>(harness.fighter)
            .unwrap();

        assert_eq!(harness.state(), FighterState::Attack1);
        assert!(attack.active, "攻击根运动必须已接管 mode-3");
        assert!(
            attack.carry_z > 0.0 && attack.carry_z < inherited,
            "空中入攻应存档并按 9/s 衰减旧速度，carry={} inherited={inherited}",
            attack.carry_z,
        );
        assert!(
            (accepted.x - motion.x).abs() < 1e-4
                && (accepted.y - motion.y).abs() < 1e-4
                && (accepted.z - motion.z).abs() < 1e-4,
            "单次 mover 必须发布完整的已接受三维速度：accepted={accepted:?} motion={motion:?}",
        );
        assert!(
            accepted.z < inherited,
            "空中 A 入口不得把同一份跳跃速度同时从 carry 和 AirState 重复提交：{accepted:?}",
        );
    }

    #[test]
    fn production_forward_jump_air_attack_keeps_takeoff_direction() {
        for attack_delay in [0usize, 2] {
            let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
                eprintln!("origin_res missing, skipping");
                return;
            };
            let keymap = crate::app_state::Keymap::default();
            harness.press_binding(keymap.move_up);
            harness.steps(3);
            harness.press_binding(keymap.jump);
            harness.step();

            harness.release_binding(keymap.jump);
            if attack_delay == 0 {
                harness.press_mouse(MouseButton::Left);
            }
            harness.step();
            if attack_delay == 0 {
                harness.release_mouse(MouseButton::Left);
            } else {
                harness.steps(attack_delay);
                harness.press_mouse(MouseButton::Left);
                harness.step();
                harness.release_mouse(MouseButton::Left);
            }

            let attack_start = harness.position();
            let entry_air_z = harness
                .app
                .world()
                .get::<FighterAir>(harness.fighter)
                .unwrap()
                .air
                .planar_z;
            let entry_carry_z = harness
                .app
                .world()
                .get::<FighterAttackMotion>(harness.fighter)
                .unwrap()
                .carry_z;
            let mut samples = Vec::new();
            let mut expected_carry = entry_carry_z;
            let mut expected_travel = 0.0;
            for tick in 0..12 {
                let decay = if attack_delay == 0 && tick == 0 {
                    9.0
                } else {
                    0.2
                };
                expected_carry *= 1.0 - decay * HARNESS_DT;
                expected_travel += expected_carry * HARNESS_DT;
                harness.step();
                samples.push(harness.position().z);
            }
            let travelled = harness.position().z - attack_start.z;
            assert_eq!(harness.state(), FighterState::Attack1);
            assert!(
                entry_carry_z > 1.0 && entry_air_z > 1.0,
                "攻击入口必须继承前跳方向：air={entry_air_z} carry={entry_carry_z}",
            );
            assert!(
                (travelled - expected_travel).abs() < 1e-4,
                "漂移须遵守接触分类帧序：delay={attack_delay} travelled={travelled} expected={expected_travel}",
            );
            assert!(
                samples.windows(2).all(|pair| pair[1] + 1e-5 >= pair[0]),
                "空中 A 不得在中段反向：delay={attack_delay} samples={samples:?}",
            );
        }
    }

    #[test]
    fn production_air_attack_wall_glance_keeps_tangent_velocity() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let keymap = crate::app_state::Keymap::default();

        harness.press_binding(keymap.move_up);
        harness.press_binding(keymap.move_right);
        harness.steps(3);
        harness.press_binding(keymap.jump);
        harness.step();
        harness.release_binding(keymap.move_up);
        harness.release_binding(keymap.move_right);
        harness.release_binding(keymap.jump);
        harness.step();
        harness.steps(2);
        let air_before = harness
            .app
            .world()
            .get::<FighterAir>(harness.fighter)
            .unwrap()
            .air
            .clone();
        assert!(air_before.planar_x < -1.0 && air_before.planar_z > 10.0);

        let before = harness.position();
        harness.replace_stage_mesh(harness_ground_with_side_wall(before.x - 1.0));
        harness.press_mouse(MouseButton::Left);
        harness.step();
        harness.release_mouse(MouseButton::Left);

        let first_accepted = (harness.position() - before) / HARNESS_DT;
        let first_motion = harness
            .app
            .world()
            .get::<MotionState>(harness.fighter)
            .unwrap()
            .velocity;

        let second_before = harness.position();
        harness.step();
        let accepted = (harness.position() - second_before) / HARNESS_DT;
        let motion = harness
            .app
            .world()
            .get::<MotionState>(harness.fighter)
            .unwrap()
            .velocity;
        let air_after = harness
            .app
            .world()
            .get::<FighterAir>(harness.fighter)
            .unwrap();
        let attack = harness
            .app
            .world()
            .get::<FighterAttackMotion>(harness.fighter)
            .unwrap();

        assert!(
            first_accepted.x <= 0.0 && first_accepted.x > air_before.planar_x,
            "首接触帧法向不得反弹或超过接触前速度：{first_accepted:?}",
        );
        assert!(
            (first_accepted.x - first_motion.x).abs() < 1e-4
                && (first_accepted.y - first_motion.y).abs() < 1e-4
                && (first_accepted.z - first_motion.z).abs() < 1e-4,
            "首接触帧也必须发布完整已接受速度：accepted={first_accepted:?} motion={first_motion:?}",
        );
        assert!(
            accepted.x.abs() < 1e-5,
            "稳定接触帧法向必须被拒绝：{accepted:?}"
        );
        assert!(
            accepted.z > 5.0,
            "侧墙只能剔除法向，不得丢掉前向切向速度：{accepted:?}",
        );
        assert!(air_after.air.planar_x.abs() < 1e-5);
        assert!(air_after.air.planar_z > 5.0);
        assert!(attack.carry_x.abs() < 1e-5, "精确拒绝轴应清同轴 carry");
        assert!(attack.carry_z > 5.0, "切向 carry 必须保留");
        assert!(
            (accepted.x - motion.x).abs() < 1e-4
                && (accepted.y - motion.y).abs() < 1e-4
                && (accepted.z - motion.z).abs() < 1e-4,
            "碰墙后发布速度必须等于完整已接受位移：accepted={accepted:?} motion={motion:?}",
        );
    }

    #[test]
    fn production_forward_jump_enters_wall_cling_after_post_tick_probe() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        harness.replace_stage_mesh(harness_ground_with_front_wall(5.0));
        let keymap = crate::app_state::Keymap::default();

        harness.press_binding(keymap.move_up);
        harness.steps(3);
        harness.press_binding(keymap.jump);
        harness.step();
        harness.release_binding(keymap.jump);
        harness.step();

        let mut clung = None;
        for tick in 0..12 {
            harness.step();
            let wall = harness
                .app
                .world()
                .get::<FighterWall>(harness.fighter)
                .unwrap();
            if wall.cling_active {
                clung = Some((tick, wall.cling_pose, harness.position()));
                break;
            }
        }
        let (tick, pose, position) = clung.expect("持续按前的上升跳应在 16 源单位探针内抓住正面墙");
        assert_eq!(pose, L::WALL_CLING_F, "正面墙必须进入 Pose 161");
        assert!(tick >= 1, "抓墙必须等待原版 0.1s 武装门");
        assert!(position.y > 0.25, "抓墙前必须已真正离地：{position:?}");
        assert!(position.z < 5.0, "抓墙不得穿过墙面：{position:?}");
    }

    #[test]
    fn production_forward_jump_clings_to_a_near_front_wall_at_the_original_delay() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        harness.replace_stage_mesh(harness_ground_with_front_wall(2.0));
        let keymap = crate::app_state::Keymap::default();

        harness.press_binding(keymap.move_up);
        harness.press_binding(keymap.jump);
        harness.step();
        harness.release_binding(keymap.jump);
        harness.step();
        assert_eq!(
            harness.pose_id(),
            L::JUMP_F,
            "地面跳的同一个松键 tick 不得再被空中墙跳分支改成 Pose 157",
        );
        assert!(
            harness
                .app
                .world()
                .get::<FighterAir>(harness.fighter)
                .unwrap()
                .air
                .planar_z
                > 0.0,
            "近墙起跳首帧必须继续朝墙前进，不能被同帧墙跳反弹",
        );

        let mut trace = Vec::new();
        let mut clung_tick = None;
        for tick in 0..8 {
            harness.step();
            let air = harness
                .app
                .world()
                .get::<FighterAir>(harness.fighter)
                .unwrap();
            let wall = harness
                .app
                .world()
                .get::<FighterWall>(harness.fighter)
                .unwrap();
            let motion = harness
                .app
                .world()
                .get::<MotionState>(harness.fighter)
                .unwrap();
            trace.push((
                tick,
                harness.position(),
                air.air.air_time,
                air.air.planar_z,
                air.air.vertical_speed,
                air.air.cling_armed,
                wall.cling_active,
                motion.facing,
                motion.velocity.z,
            ));
            if wall.cling_active {
                clung_tick = Some(tick);
                break;
            }
        }
        assert!(
            clung_tick.is_some_and(|tick| tick <= 2),
            "近墙前跳应在 0.1s 门后的首个合格 tick 进入 161：{trace:?}",
        );
        assert_eq!(harness.pose_id(), L::WALL_CLING_F);
    }

    #[test]
    fn production_walk_contact_and_authored_attack_respect_walls() {
        use meteor_physics::character_contact::{CHARACTER_CONTACT_RADIUS, CHARACTER_PUSH_SPEED};
        use meteor_physics::collision::{CollisionSystem, Point3};
        for fps in [30, 60, 120] {
            for attack in [false, true] {
                let mut h = spawn_gameplay_harness(DAGGER).expect("original P1 assets");
                h.app
                    .insert_resource(State::new(AppState::Playing))
                    .add_plugins(crate::plugins::character_contact_plugin::CharacterContactPlugin);
                let mesh = harness_ground_with_front_wall(4.8);
                h.replace_stage_mesh(mesh.clone());
                let mut stage = CollisionSystem::new(0.55, 1.8);
                stage.set_mesh_collider(mesh);
                let target = h
                    .app
                    .world_mut()
                    .spawn((
                        Fighter {
                            character_id: 1,
                            player_slot: 1,
                            weapon: DAGGER,
                            weapon_slots: [DAGGER, 0],
                            weapon_slot: 0,
                            weapon_slot_variants: [0, 0],
                            weapon_variant: 0,
                        },
                        Transform::from_xyz(0.0, 0.0, 2.5),
                        PoseRuntime::idle(),
                        SmState::default(),
                        Health::default(),
                        MotionState {
                            movement_mode: meteor_core::MovementMode::Idle,
                            velocity: Vec3::ZERO,
                            facing: 0.0,
                        },
                    ))
                    .id();
                if attack {
                    h.app
                        .world_mut()
                        .resource_scope(|world, assets: Mut<CharacterAssetsSet>| {
                            assets.0["p1"].apply_pose(
                                &mut world.get_mut::<PoseRuntime>(h.fighter).unwrap(),
                                258,
                                true,
                            );
                        });
                    h.app
                        .world_mut()
                        .get_mut::<SmState>(h.fighter)
                        .unwrap()
                        .current = FighterState::Attack1;
                    h.app
                        .world_mut()
                        .get_mut::<CombatState>(h.fighter)
                        .unwrap()
                        .state = FighterState::Attack1;
                } else {
                    h.press_binding(crate::app_state::Keymap::default().move_up);
                }
                let dt = 1.0 / fps as f32;
                let mut previous_target = Vec3::new(0.0, 0.0, 2.5);
                let mut previous_mover = h.position();
                for _ in 0..fps * 2 {
                    h.step_dt(dt);
                    let mover = h.position();
                    let pushed = h.app.world().get::<Transform>(target).unwrap().translation;
                    if !attack {
                        assert!(
                            (pushed - mover).xz().length() >= 2.0 * CHARACTER_CONTACT_RADIUS - 1e-4,
                            "{fps} fps: controller penetration {mover:?} {pushed:?}"
                        );
                        assert!(
                            (pushed - previous_target).xz().length()
                                <= CHARACTER_PUSH_SPEED * dt + 1e-4,
                            "a single contact source must not amplify push speed"
                        );
                    }
                    assert!(stage.can_occupy(Point3::new(mover.x, mover.y, mover.z)));
                    assert!(stage.can_occupy(Point3::new(pushed.x, pushed.y, pushed.z)));
                    if !attack {
                        assert!(
                            mover.z >= previous_mover.z - 1e-4,
                            "walk contact must not pop backward"
                        );
                    }
                    previous_target = pushed;
                    previous_mover = mover;
                }
                if !attack {
                    assert!(
                        previous_target.z > 2.55,
                        "must actually exercise contact carry"
                    );
                }
            }
        }
    }

    #[test]
    fn locked_walk_uses_entry_facing_then_turns_towards_the_target() {
        for fps in [30, 60, 120] {
            let (mut h, target, _) = spawn_local_stock_hit_harness(11);
            h.app.insert_resource(NetState::default());
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = 0.0;
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = Vec3::new(6.0, 0.0, 6.0);
            {
                let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                lock.lock_entity = Some(target);
                lock.system.lock_target = Some(target.to_bits());
            }
            h.press_key(KeyCode::KeyW);
            let before = h.position();
            h.step_dt(1.0 / fps as f32);
            let accepted = h.position() - before;
            assert!(
                accepted.z > 0.0 && accepted.x.abs() < 1e-5,
                "first move uses the previous root basis: {accepted:?}"
            );
            let to_target =
                h.app.world().get::<Transform>(target).unwrap().translation - h.position();
            let facing = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
            assert!(
                (facing - to_target.x.atan2(to_target.z)).abs() < 1e-5,
                "ground Lock must still update after moving"
            );
        }
    }

    #[test]
    fn ground_lock_correction_returns_after_jump_and_dodge() {
        for fps in [30, 60, 120] {
            for dodge in [false, true] {
                let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(DAGGER, true);
                h.app.add_systems(
                    super::super::fighter_frame::schedule(&h.app),
                    crate::plugins::character_plugin::drive_fighter_animation_for_test
                        .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
                );
                let dt = 1.0 / fps as f32;
                let keys = crate::app_state::Keymap::default();
                h.app
                    .world_mut()
                    .get_mut::<Transform>(target)
                    .unwrap()
                    .translation = Vec3::Z * 10.0;
                {
                    let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                    lock.lock_entity = Some(target);
                    lock.system.lock_target = Some(target.to_bits());
                }
                let action = if dodge { keys.dodge } else { keys.jump };
                h.press_binding(action);
                h.step_dt(dt);
                h.release_binding(action);
                h.step_dt(dt);
                assert!(if dodge {
                    h.state() == FighterState::Dodge
                } else {
                    h.airborne()
                });
                let target_position = Vec3::new(8.0, 0.0, 8.0);
                h.app
                    .world_mut()
                    .get_mut::<Transform>(target)
                    .unwrap()
                    .translation = target_position;
                h.press_key(KeyCode::KeyW);
                let mut recovered = false;
                for _ in 0..fps * 3 {
                    h.step_dt(dt);
                    if !h.airborne() && h.state() == FighterState::Name {
                        h.step_dt(dt);
                        let delta = target_position - h.position();
                        let yaw = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                        let error = yaw - delta.x.atan2(delta.z);
                        assert!(
                            error.sin().atan2(error.cos()).abs() < 1e-4,
                            "ground tracking must resume: {fps} FPS dodge={dodge}"
                        );
                        recovered = true;
                        break;
                    }
                }
                assert!(
                    recovered,
                    "action must return to ground: {fps} FPS dodge={dodge}"
                );
            }
        }
    }

    #[test]
    fn recovered_ground_lock_resumes_forward_target_correction() {
        for fps in [30, 60, 120] {
            for serial in [false, true] {
                let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(11, serial);
                {
                    let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();

                    air.recovery_ground_mode = Some(true);
                    air.ground_contact = true;
                }
                {
                    let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                    lock.lock_entity = Some(target);
                    lock.system.lock_target = Some(target.to_bits());
                }
                h.press_key(KeyCode::KeyW);
                for tick in 0..fps / 2 {
                    let position = Vec3::new(8.0 + tick as f32 / fps as f32, 0.0, 8.0);
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(target)
                        .unwrap()
                        .translation = position;
                    h.step_dt(1.0 / fps as f32);
                    let delta = position - h.position();
                    let yaw = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                    let error = yaw - delta.x.atan2(delta.z);
                    assert!(
                        error.sin().atan2(error.cos()).abs() < 1e-4,
                        "recovered mode1 must face target: {fps} FPS serial={serial} tick={tick}"
                    );
                    assert_eq!(
                        h.app.world().resource::<LockOnController>().lock_entity,
                        Some(target)
                    );
                }
                assert!(
                    h.position().x > 0.5,
                    "W must recover movement toward the target"
                );
            }
        }
    }

    #[test]
    fn locked_forward_uses_entry_facing_then_tracks_a_moving_target() {
        for fps in [30, 60, 120] {
            let (mut h, target, _) = spawn_local_stock_hit_harness(11);
            let dt = 1.0 / fps as f32;
            {
                let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                lock.lock_entity = Some(target);
                lock.system.lock_target = Some(target.to_bits());
            }
            h.press_key(KeyCode::KeyW);
            for tick in 0..fps {
                let target_position = Vec3::new(6.0 + tick as f32 * dt, 0.0, 8.0);
                h.app
                    .world_mut()
                    .get_mut::<Transform>(target)
                    .unwrap()
                    .translation = target_position;
                let before = h.position();
                let entry_yaw = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                h.step_dt(dt);
                let displacement = h.position() - before;
                let right = Vec3::new(entry_yaw.cos(), 0.0, -entry_yaw.sin());
                if tick == 0 {
                    assert!(displacement.length() > 0.0);
                    assert!(
                        displacement.dot(right).abs() < 1e-4,
                        "W must use frame-entry facing before turning at {fps} FPS"
                    );
                }
                let delta = target_position - h.position();
                let yaw = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                let error = yaw - delta.x.atan2(delta.z);
                assert!(
                    error.sin().atan2(error.cos()).abs() < 1e-4,
                    "post-move facing must follow moving target at {fps} FPS, tick {tick}"
                );
            }
        }
    }

    #[test]
    fn ground_lock_facing_clears_out_of_range_targets_after_movement() {
        for target_position in [Vec3::X * 30.0, Vec3::new(4.0, 13.0, 0.0)] {
            let (mut h, target, _) = spawn_local_stock_hit_harness(11);
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = target_position;
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = 0.7;
            {
                let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                lock.lock_entity = Some(target);
                lock.system.lock_target = Some(target.to_bits());
            }

            h.step_dt(1.0 / 60.0);
            let lock = h.app.world().resource::<LockOnController>();
            assert!(lock.lock_entity.is_none());
            assert!(lock.system.lock_target.is_none());
            assert_eq!(
                h.app.world().get::<MotionState>(h.fighter).unwrap().facing,
                0.7
            );
            assert_eq!(
                h.app
                    .world()
                    .resource::<crate::plugins::camera_plugin::CameraBasis>()
                    .yaw,
                0.7
            );
        }
    }

    #[test]
    fn locked_ground_facing_tracks_target_and_forward_closes_distance() {
        for fps in [30, 60, 120] {
            for initial_target in [Vec3::X * 6.0, Vec3::NEG_X * 6.0, Vec3::NEG_Z * 6.0] {
                let (mut h, target, _) = spawn_local_stock_hit_harness(11);
                h.app.insert_resource(NetState::default());
                h.app.insert_resource(State::new(AppState::Playing));
                h.app
                    .world_mut()
                    .entity_mut(h.fighter)
                    .insert(HeadTrackState::default());
                configure_lock_updates(&mut h.app);
                h.app
                    .world_mut()
                    .get_mut::<Transform>(target)
                    .unwrap()
                    .translation = initial_target;
                {
                    let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                    lock.lock_entity = Some(target);
                    lock.system.lock_target = Some(target.to_bits());
                }
                let dt = 1.0 / fps as f32;
                for target_position in [initial_target, Vec3::new(4.0, 0.0, 4.0)] {
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(target)
                        .unwrap()
                        .translation = target_position;
                    h.step_dt(dt);
                    let delta = target_position - h.position();
                    let facing = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                    let error = facing - delta.x.atan2(delta.z);
                    assert!(
                        error.sin().atan2(error.cos()).abs() < 1e-4,
                        "locked idle must follow target at {fps} FPS: facing={facing}, delta={delta:?}"
                    );
                    assert_eq!(
                        h.app.world().resource::<LockOnController>().lock_entity,
                        Some(target)
                    );
                }
                let target_position = h.app.world().get::<Transform>(target).unwrap().translation;
                let before = h.position().distance(target_position);
                h.press_key(KeyCode::KeyW);
                for _ in 0..fps / 4 {
                    h.step_dt(dt);
                    let delta = target_position - h.position();
                    let facing = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                    let error = facing - delta.x.atan2(delta.z);
                    assert!(
                        error.sin().atan2(error.cos()).abs() < 1e-4,
                        "locked W must keep facing target at {fps} FPS"
                    );
                }
                assert!(
                    h.position().distance(target_position) < before - 0.5,
                    "locked W must approach target instead of following the stale facing"
                );
            }
        }
    }

    #[test]
    fn continuous_push_preserves_pressure_and_locked_target_spacing() {
        use crate::plugins::FighterFrameSet;
        for fps in [30, 60, 120] {
            for locked in [false, true] {
                for offset in [0.0, 0.1] {
                    let (mut h, target, _) = spawn_local_stock_hit_harness(11);
                    h.app.insert_resource(NetState::default());
                    h.app.insert_resource(State::new(AppState::Playing));
                    h.app.add_plugins(
                        crate::plugins::character_contact_plugin::CharacterContactPlugin,
                    );
                    h.app
                        .world_mut()
                        .entity_mut(h.fighter)
                        .insert(HeadTrackState::default());
                    configure_lock_updates(&mut h.app);
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(target)
                        .unwrap()
                        .translation = Vec3::new(offset, 0.0, 3.0);
                    let camera =
                        crate::plugins::camera_plugin::install_follow_camera_for_test(&mut h.app);
                    h.app.add_systems(
                        Update,
                        crate::plugins::camera_plugin::prepare_move_basis
                            .in_set(FighterFrameSet::Movement),
                    );
                    if locked {
                        let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                        lock.lock_entity = Some(target);
                        lock.system.lock_target = Some(target.to_bits());
                    }
                    h.press_key(KeyCode::KeyW);
                    for _ in 0..fps * 5 {
                        let previous_focus = h
                            .app
                            .world()
                            .get::<crate::plugins::camera_plugin::ThirdPersonState>(camera)
                            .unwrap()
                            .follow_focus_for_test();
                        h.step_dt(1.0 / fps as f32);
                        let camera_rotation =
                            h.app.world().get::<Transform>(camera).unwrap().rotation;
                        assert!(camera_rotation.is_finite());
                        if locked {
                            let world = h.app.world();
                            assert_eq!(
                                world.resource::<LockOnController>().lock_entity,
                                Some(target)
                            );
                            let delta =
                                world.get::<Transform>(target).unwrap().translation - h.position();
                            let desired_focus = h.position()
                                + delta * 0.25
                                + Vec3::Y * (24.0 * meteor_core::MODEL_SCALE);
                            let expected_focus =
                                previous_focus.lerp(desired_focus, 5.0 / fps as f32);
                            let actual_focus = world
                                .get::<crate::plugins::camera_plugin::ThirdPersonState>(camera)
                                .unwrap()
                                .follow_focus_for_test();
                            assert!(
                                actual_focus.distance(expected_focus) < 1e-5,
                                "camera must consume both positions after contact push"
                            );
                            let facing = world.get::<MotionState>(h.fighter).unwrap().facing;
                            let error = facing - delta.x.atan2(delta.z);
                            assert!(
                                error.sin().atan2(error.cos()).abs() < 1e-3,
                                "continuous locked push must face the target at {fps} FPS"
                            );
                        }
                    }
                    let target_pos = h.app.world().get::<Transform>(target).unwrap().translation;
                    eprintln!(
                        "fps={fps} locked={locked} offset={offset} player={:?} target={target_pos:?}",
                        h.position()
                    );
                    if locked || offset == 0.0 {
                        assert!(
                            target_pos.z > 14.0,
                            "continuous pressure must not lose every other push tick"
                        );
                    } else {
                        assert!(
                            target_pos.z > 3.2,
                            "edge contact must push before the mover slides past"
                        );
                    }
                    if locked {
                        assert!(
                            h.position().distance(target_pos) < 2.2,
                            "locked W must keep contact instead of running past the target"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn production_fall_pose_switch_preserves_source_for_entry_blend() {
        for (rise, fall, blocked_descent) in [(151, 152), (153, 154), (155, 156), (157, 158)]
            .into_iter()
            .flat_map(|(rise, fall)| [false, true].map(|blocked| (rise, fall, blocked)))
        {
            let mut h = spawn_gameplay_harness(DAGGER).expect("original P1 assets");
            if blocked_descent {
                let mut mesh = harness_ground();
                mesh.append_world_triangles(
                    &[
                        [-20.0, 0.0, 2.0],
                        [20.0, 0.0, 2.0],
                        [20.0, 30.0, 9.5],
                        [-20.0, 30.0, 9.5],
                    ],
                    &[0, 2, 1, 0, 3, 2],
                );
                h.replace_stage_mesh(mesh);
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation
                    .z = 6.335;
            }
            let assets = h.app.world().resource::<CharacterAssetsSet>();
            let mut pose = PoseRuntime::idle();
            assets.0["p1"].apply_pose(&mut pose, rise, true);
            h.app.world_mut().entity_mut(h.fighter).insert(pose);
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation
                .y = 20.0;
            h.app
                .world_mut()
                .get_mut::<SmState>(h.fighter)
                .unwrap()
                .current = FighterState::Jump;
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .movement_mode = meteor_core::MovementMode::Airborne;
            {
                let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                air.air.airborne = true;
                air.air.vertical_speed = -6.0;
                air.air.motion_mode = meteor_physics::jump::AirMotionMode::Ballistic;
                air.air.air_time = 0.2;
                air.late_visual_pose_snap = false;
            }
            h.step();
            assert_eq!(h.pose_id(), fall);
            let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
            assert!(
                !air.late_visual_pose_snap,
                "{rise}->{fall} must preserve the source skeleton for blending"
            );
            assert_eq!(air.motion_root_pin_pose_id, fall);
            let blend = h.app.world().get::<EntryBlend>(h.fighter).unwrap();
            assert!((blend.duration - 0.12).abs() < 1e-6);
            assert_eq!(blend.pose_id, fall);
            if blocked_descent {
                assert!(
                    air.air.vertical_speed.abs() < 1e-5,
                    "fixture must reject descent against the slanted body contact"
                );
                assert!(
                    air.air.airborne,
                    "steep contact must not become floor support"
                );
            }
        }
    }

    #[test]
    fn production_wall_fall_lands_on_high_platform_before_pose_180() {
        for fps in [30, 60, 120, 165] {
            for wall_pose in [162, 163] {
                let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
                let mut mesh = harness_ground();
                mesh.append_world_triangles(
                    &[
                        [-30.0, 17.68214, -30.0],
                        [-30.0, 17.68214, 30.0],
                        [30.0, 17.68214, 30.0],
                        [30.0, 17.68214, -30.0],
                    ],
                    &[0, 1, 2, 0, 2, 3],
                );
                h.replace_stage_mesh(mesh);
                let mut pose = PoseRuntime::idle();
                h.app.world().resource::<CharacterAssetsSet>().0["p1"]
                    .apply_pose(&mut pose, wall_pose, true);
                h.app.world_mut().entity_mut(h.fighter).insert(pose);
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation
                    .y = 19.888;
                h.app
                    .world_mut()
                    .get_mut::<SmState>(h.fighter)
                    .unwrap()
                    .current = FighterState::Jump;
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .movement_mode = meteor_core::MovementMode::Airborne;
                {
                    let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                    air.air.airborne = true;
                    air.air.vertical_speed = -4.0;
                    air.air.motion_mode = meteor_physics::jump::AirMotionMode::Ballistic;
                }
                let mut landed = false;
                for _ in 0..fps {
                    h.step_dt(1.0 / fps as f32);
                    assert_ne!(
                        h.pose_id(),
                        180,
                        "premature fall tail: fps={fps} wall={wall_pose}"
                    );
                    if !h.airborne() {
                        landed = true;
                        break;
                    }
                }
                assert!(landed);
                assert!((h.position().y - 17.68214).abs() < 0.01);
                h.step_dt(1.0 / fps as f32);
                assert_eq!(h.pose_id(), 0);
            }
        }
    }

    #[test]
    fn production_wall_release_defers_free_air_motion_to_next_tick() {
        let keymap = crate::app_state::Keymap::default();
        for fps in [30, 60, 120] {
            for release in ["key_up", "opposed_keys", "timeout"] {
                let mut h = spawn_gameplay_harness(DAGGER).expect("original P1 resources");
                h.replace_stage_mesh(harness_ground_with_front_wall(2.0));
                let dt = 1.0 / fps as f32;
                h.press_binding(keymap.move_up);
                h.press_binding(keymap.jump);
                h.step_dt(dt);
                h.release_binding(keymap.jump);
                for _ in 0..fps {
                    h.step_dt(dt);
                    if h.app
                        .world()
                        .get::<FighterWall>(h.fighter)
                        .unwrap()
                        .cling_active
                    {
                        break;
                    }
                }
                assert!(
                    h.app
                        .world()
                        .get::<FighterWall>(h.fighter)
                        .unwrap()
                        .cling_active
                );
                match release {
                    "key_up" => h.release_binding(keymap.move_up),
                    "opposed_keys" => h.press_binding(keymap.move_down),
                    _ => {
                        h.app
                            .world_mut()
                            .get_mut::<FighterWall>(h.fighter)
                            .unwrap()
                            .remaining = dt * 0.5
                    }
                }
                let before = h.position();
                let expected = h
                    .app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .unwrap()
                    .air
                    .clone();
                let expected_dy = 0.0;
                h.step_dt(dt);
                let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
                let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                assert!(
                    !wall.cling_active,
                    "{fps} fps {release} must release immediately"
                );
                assert!(
                    (h.position().y - before.y - expected_dy).abs() < 1e-4,
                    "{fps} fps {release}: extra wall movement: actual={} expected={expected_dy}",
                    h.position().y - before.y
                );
                assert!(
                    (air.air.vertical_speed - expected.vertical_speed).abs() < 1e-4,
                    "{fps} fps {release}: mode2 acceleration leaked into free fall"
                );
                assert_eq!(wall.external_accel, Vec3::ZERO);
                assert!(!air.air.cling_armed, "release must not rearm wall climbing");
            }
        }
    }

    #[test]
    fn production_opposed_direction_keys_do_not_arm_wall_entry() {
        let keymap = crate::app_state::Keymap::default();
        for fps in [30, 60, 120] {
            let mut h = spawn_gameplay_harness(DAGGER).expect("original P1 resources");
            h.replace_stage_mesh(harness_ground_with_front_wall(1.5));
            let dt = 1.0 / fps as f32;
            h.press_binding(keymap.move_up);
            h.press_binding(keymap.move_down);
            h.press_binding(keymap.jump);
            h.step_dt(dt);
            h.release_binding(keymap.jump);
            let mut eligible_rise = false;
            for _ in 0..fps / 3 {
                h.step_dt(dt);
                let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                eligible_rise |= air.air.air_time >= 0.1 && air.air.vertical_speed > 0.0;
                assert!(
                    !h.app
                        .world()
                        .get::<FighterWall>(h.fighter)
                        .unwrap()
                        .cling_active,
                    "{fps} fps: cancelled forward input must not grab the wall"
                );
            }
            assert!(
                eligible_rise,
                "must exercise the ascending wall-entry window"
            );
        }
    }

    #[test]
    fn production_wall_approach_angles_and_frame_rates() {
        let keymap = crate::app_state::Keymap::default();
        let mut failures = Vec::new();
        for fps in [30, 60, 120] {
            for slope in [0.0, 0.25] {
                for degrees in [0.0_f32, 30.0, 45.0] {
                    let mut h = spawn_gameplay_harness(DAGGER).unwrap();
                    let mut mesh = harness_ground();
                    mesh.append_world_triangles(
                        &[
                            [-20.0, 0.0, 2.0],
                            [20.0, 0.0, 2.0],
                            [20.0, 20.0, 2.0 + slope * 20.0],
                            [-20.0, 20.0, 2.0 + slope * 20.0],
                        ],
                        &[0, 2, 1, 0, 3, 2],
                    );
                    h.replace_stage_mesh(mesh);
                    let yaw = degrees.to_radians();
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(h.fighter)
                        .unwrap()
                        .facing = yaw;
                    h.app
                        .world_mut()
                        .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                        .yaw = yaw;
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(h.fighter)
                        .unwrap()
                        .translation
                        .z = -3.0;
                    h.press_binding(keymap.move_up);
                    for _ in 0..(fps / 5) {
                        h.step_dt(1.0 / fps as f32);
                    }
                    h.press_binding(keymap.jump);
                    h.step_dt(1.0 / fps as f32);
                    h.release_binding(keymap.jump);
                    let mut clung = false;
                    let mut trace = Vec::new();
                    for tick in 0..fps {
                        h.step_dt(1.0 / fps as f32);
                        let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                        let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
                        if wall.cling_active {
                            clung = true;
                            break;
                        }
                        if tick < 15 {
                            trace.push((
                                tick,
                                h.position(),
                                air.air.vertical_speed,
                                air.ground_y,
                                air.air.cling_armed,
                            ));
                        }
                    }
                    if !clung {
                        failures.push((fps, slope, degrees, trace));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "approach failures: {failures:?}");
    }

    #[test]
    fn production_sn13_mobile_root_probe_and_release_do_not_stick() {
        use crate::plugins::input_plugin::SyntheticAction;
        use meteor_physics::collision::MeshCollider;
        let mesh = sn13_test_mesh();
        let keymap = crate::app_state::Keymap::default();
        for fps in [30, 60, 120] {
            let dt = 1.0 / fps as f32;
            for degrees in [15.0_f32, 45.0, 75.0] {
                let yaw = degrees.to_radians();
                let mut h = spawn_gameplay_harness(DAGGER).unwrap();
                h.replace_stage_mesh(mesh.clone());
                let x = -123.83423 - yaw.sin() * 6.0;
                let z = -120.89842 - yaw.cos() * 6.0;
                let ground = mesh.sample_ground_hit(x, z, 0.0, 0.0).unwrap();
                let start = Vec3::new(x, ground.y, z);
                assert!(
                    !mesh.box_blocked(x, ground.y, z),
                    "start must be clear: {start:?}"
                );
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation = start;
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .facing = yaw;
                h.app
                    .world_mut()
                    .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                    .yaw = yaw;

                h.mobile_binding(keymap.move_up, true, None);
                for _ in 0..(fps / 5) {
                    h.step_dt(dt);
                }
                h.mobile_binding(keymap.jump, true, Some(SyntheticAction::Jump));
                h.step_dt(dt);
                h.mobile_binding(keymap.jump, false, Some(SyntheticAction::Jump));
                let mut clung = false;
                for _ in 0..fps {
                    h.step_dt(dt);
                    if h.app
                        .world()
                        .get::<FighterWall>(h.fighter)
                        .unwrap()
                        .cling_active
                    {
                        clung = true;
                        break;
                    }
                }

                if clung {
                    assert!(
                        [L::WALL_CLING_F, L::WALL_CLING_L, L::WALL_CLING_R].contains(&h.pose_id())
                    );
                }
                for _ in 0..(fps / 3) {
                    h.step_dt(dt);
                }
                let release = h.position();
                h.mobile_binding(keymap.move_up, false, None);
                for _ in 0..fps * 6 {
                    h.step_dt(dt);
                }
                let end = h.position();
                let support = mesh.sample_ground_hit(end.x, end.z, 0.0, end.y).unwrap();
                assert!((support.y-end.y).abs()<0.3 && support.ny>=0.42,
                    "mobile release must reach ground: fps={fps} degrees={degrees} release={release:?} end={end:?} support={support:?}");
                assert!(!h.airborne());
                assert!(
                    !h.app
                        .world()
                        .get::<FighterWall>(h.fighter)
                        .unwrap()
                        .cling_active
                );
            }
        }
    }

    #[test]
    fn production_held_jump_fires_at_threshold_and_release_does_not_wall_kick() {
        for fps in [30, 60, 120, 165] {
            for wall in [false, true] {
                let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
                if wall {
                    h.replace_stage_mesh(harness_ground_with_front_wall(2.0));
                }
                let dt = 1.0 / fps as f32;
                h.press_key(KeyCode::KeyW);
                h.press_key(KeyCode::Space);
                let mut first_air = None;
                for tick in 0..fps / 3 {
                    h.step_dt(dt);
                    if h.airborne() && first_air.is_none() {
                        first_air = Some(tick as f32 * dt);
                        assert!(
                            h.app
                                .world()
                                .get::<FighterAir>(h.fighter)
                                .unwrap()
                                .air
                                .vertical_speed
                                > 34.0
                        );
                    }
                }
                let at = first_air.expect("held Space must take off without key-up");
                assert!(
                    at >= 0.11 - dt && at <= 0.11 + dt * 2.0,
                    "fps={fps} wall={wall} at={at}"
                );
                assert!(
                    !h.app
                        .world()
                        .get::<FighterAir>(h.fighter)
                        .unwrap()
                        .jump_charge_active
                );
                h.release_key(KeyCode::Space);
                h.step_dt(dt);
                let pose = h.pose_id();
                assert!(
                    !matches!(pose, 157 | 159 | 160),
                    "spent key-up must not start a wall kick: {pose}"
                );
                if wall {
                    assert!(
                        h.app
                            .world()
                            .get::<FighterWall>(h.fighter)
                            .unwrap()
                            .cling_active
                    );
                }
            }
        }
    }

    #[test]
    fn production_dagger_high_drop_recorded_tail() {
        for fps in [30, 60, 120, 165] {
            for (height, speed) in [(24.3441, 3.1495), (29.9958, 1.3846), (37.0143, 0.456)] {
                let mut reference = Vec::new();
                for age in [0.0, 1.9, 4.38] {
                    let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
                    h.press_key(KeyCode::Space);
                    h.step();
                    h.release_key(KeyCode::Space);
                    h.steps(4);
                    h.press_key(KeyCode::KeyW);
                    h.step();
                    h.release_key(KeyCode::KeyW);
                    h.press_key(KeyCode::KeyS);
                    h.step();
                    h.release_key(KeyCode::KeyS);
                    h.press_mouse(MouseButton::Left);
                    h.step();
                    h.release_mouse(MouseButton::Left);
                    assert_eq!(h.pose_id(), 256);
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(h.fighter)
                        .unwrap()
                        .translation = Vec3::new(0.0, height, 0.0);
                    {
                        let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                        air.air.vertical_speed = speed;
                        air.air.air_time = age;
                    }
                    let mut landed = false;
                    for tick in 0..fps * 2 {
                        h.step_dt(1.0 / fps as f32);
                        let y = h.position().y;
                        if age == 0.0 {
                            reference.push(y);
                        } else {
                            assert!((y - reference[tick as usize]).abs() < 0.001,
                                "air age must not change the drop: fps={fps} age={age} tick={tick} y={y}");
                        }
                        if y > 0.01 {
                            assert!(h.airborne(), "false landing at height {y}, age={age}");
                            assert_eq!(h.pose_id(), 256, "fall pose inserted before contact");
                        } else {
                            landed = true;
                        }
                    }
                    assert!(landed);
                    assert_eq!(h.state(), FighterState::Name);
                }
            }
        }
    }

    #[test]
    fn production_sn01_recorded_motion_replay() {
        replay_sn01_motion(false);
    }

    #[test]
    #[ignore = "diagnostic replay of user wall-route recording"]
    fn diagnose_sn01_wall_route_landing() {
        replay_sn01_motion(true);
    }

    fn replay_sn01_motion(wall_route: bool) {
        use meteor_physics::collision::MeshCollider;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn01/sn01.des")).unwrap(),
        );
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn01/sn01.gmb")).unwrap())
                .unwrap();
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn01/sn01.cob")).unwrap())
            .unwrap();
        let mesh = StageMeshCollider::from_gmdl(
            &gmdl,
            &desc,
            Some(&cob.face_ids),
            meteor_scene::stage_loader::get_stage_center("sn01", None),
        );

        let mut mover = meteor_physics::collision::CollisionSystem::new(0.55, 1.8);
        mover.mesh = Some(mesh.clone());
        let at = meteor_physics::collision::Point3::new(
            (194.68843 + 11.0) * 0.12,
            134.02089 * 0.12,
            -(289.67325 - 16.0) * 0.12,
        );
        let dt = 0.016972895;
        assert!(mover.blocks_motion_sweep(
            at,
            meteor_physics::collision::Point3::new(
                -11.960583 * 0.12 * dt,
                -244.36856 * 0.12 * dt,
                358.75876 * 0.12 * dt
            )
        ));
        let repo = root.parent().unwrap();
        let csv = std::fs::read_to_string(repo.join(if wall_route {
            "analysis/wall-route-capture/motion-hook-20260921-000421.replay.csv"
        } else {
            "analysis/sn01-motion-capture/motion-hook-20260920-225006.replay.csv"
        }))
        .unwrap();
        let rows: Vec<Vec<f32>> = csv
            .lines()
            .skip(1)
            .map(|line| line.split(',').map(|x| x.parse().unwrap()).collect())
            .collect();
        let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
        h.replace_stage_mesh(mesh.clone());
        let r = &rows[0];
        let start = Vec3::new(r[4], r[5], r[6]);
        assert!(!mesh.box_blocked(start.x, start.y, start.z));
        h.app
            .world_mut()
            .get_mut::<Transform>(h.fighter)
            .unwrap()
            .translation = start;
        let mut output =
            String::from("t,x,y,z,pose,mode,stock_x,stock_y,stock_z,stock_pose,stock_mode\n");
        let mut old_mask = 0;
        let mut attack_landed = Vec::new();
        let mut was_air_attack = false;
        for row in &rows {
            let mask = row[3] as u32;
            for (i, key) in [
                KeyCode::KeyW,
                KeyCode::KeyS,
                KeyCode::KeyA,
                KeyCode::KeyD,
                KeyCode::Space,
            ]
            .into_iter()
            .enumerate()
            {
                if mask & (1 << i) != old_mask & (1 << i) {
                    if mask & (1 << i) != 0 {
                        h.press_key(key);
                    } else {
                        h.release_key(key);
                    }
                }
            }
            if mask & 32 != old_mask & 32 {
                if mask & 32 != 0 {
                    h.press_mouse(MouseButton::Left);
                } else {
                    h.release_mouse(MouseButton::Left);
                }
            }
            old_mask = mask;
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = row[2];
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw = row[2];
            h.step_dt(row[1]);
            if was_air_attack && !h.airborne() {
                attack_landed.push(row[0]);
            }
            was_air_attack = h.pose_id() == 256 && h.airborne();
            if !wall_route && ((17.35..17.4).contains(&row[0]) || (22.45..22.5).contains(&row[0])) {
                assert!(
                    (h.position().y - 17.68214).abs() < 0.01,
                    "recorded platform attempt must land on top: t={} pos={:?}",
                    row[0],
                    h.position()
                );
            }
            if !wall_route && ((5.61..5.63).contains(&row[0]) || (8.62..8.64).contains(&row[0])) {
                assert!(
                    h.airborne(),
                    "stock held jump must fire before key-up at {}",
                    row[0]
                );
            }
            let p = h.position();
            let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
            use std::fmt::Write;
            writeln!(
                output,
                "{},{},{},{},{},{:?},{},{},{},{},{}",
                row[0],
                p.x,
                p.y,
                p.z,
                h.pose_id(),
                air.air.motion_mode,
                row[4],
                row[5],
                row[6],
                row[7],
                row[8]
            )
            .unwrap();
        }
        if !wall_route {
            assert_eq!(
                attack_landed.len(),
                2,
                "both recorded ghost-vine attacks must land"
            );
            for (actual, expected) in attack_landed.iter().zip([7.722, 10.823]) {
                assert!(
                    (actual - expected).abs() < 0.05,
                    "stock ghost-vine landing: actual={actual} expected={expected}"
                );
            }
        }
        std::fs::write(
            repo.join(if wall_route {
                "analysis/wall-route-capture/rust-replay.csv"
            } else {
                "analysis/sn01-motion-capture/rust-replay.csv"
            }),
            output,
        )
        .unwrap();
    }

    #[test]
    fn production_repeated_wall_pose_handoffs_do_not_accumulate_visual_height() {
        let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
        h.app
            .world_mut()
            .entity_mut(h.fighter)
            .insert(crate::components::fighter::VisualRootPin::default());
        crate::plugins::character_plugin::spawn_fighter_model_for_test(
            h.app.world_mut(),
            h.fighter,
        );
        let schedule = crate::plugins::fighter_frame::schedule(&h.app);
        h.app.add_systems(
            schedule,
            crate::plugins::character_plugin::drive_fighter_animation_for_test
                .before(harness_retire_entry_blend)
                .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
        );

        for _ in 0..8 {
            for pose_id in [161, 157, 162, 159, 163, 160, 152, 256] {
                let (pose, raw_root) = {
                    let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
                    let common = &h.app.world().resource::<CommonAmb>().0;
                    let mut pose = PoseRuntime::idle();
                    assets.apply_pose(&mut pose, pose_id, true);
                    let raw = assets.motion_root_at(common, pose_id, pose.frame);
                    (pose, raw)
                };
                h.app.world_mut().entity_mut(h.fighter).insert(pose);
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation
                    .y = 30.0;
                h.app
                    .world_mut()
                    .get_mut::<SmState>(h.fighter)
                    .unwrap()
                    .current = if pose_id == 256 {
                    FighterState::Attack1
                } else {
                    FighterState::Jump
                };
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .movement_mode = meteor_core::MovementMode::Airborne;
                {
                    let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                    air.air.airborne = true;
                    air.air.motion_mode = meteor_physics::jump::AirMotionMode::Ballistic;
                    air.air.vertical_speed = -1.0;
                    air.motion_root_pin = Some(raw_root);
                    air.motion_root_pin_pose_id = pose_id;
                }
                h.step_dt(1.0 / 60.0);
                let pin = h
                    .app
                    .world()
                    .get::<crate::components::fighter::VisualRootPin>(h.fighter)
                    .unwrap();
                assert_eq!(
                    pin.baseline, [0.0; 3],
                    "pose {pose_id} inherited a visual offset"
                );
            }
        }
        let mut landed = false;
        for _ in 0..180 {
            h.step_dt(1.0 / 60.0);
            landed |= !h.airborne();
            let pin = h
                .app
                .world()
                .get::<crate::components::fighter::VisualRootPin>(h.fighter)
                .unwrap();
            if pin.active {
                assert_eq!(pin.baseline, [0.0; 3]);
            }
        }
        assert!(landed);
        assert_eq!(h.state(), FighterState::Name);
    }

    #[test]
    fn production_sn01_wall_release_ghost_vine_visual_root() {
        use meteor_physics::collision::MeshCollider;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn01/sn01.des")).unwrap(),
        );
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn01/sn01.gmb")).unwrap())
                .unwrap();
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn01/sn01.cob")).unwrap())
            .unwrap();
        let mesh = StageMeshCollider::from_gmdl(
            &gmdl,
            &desc,
            Some(&cob.face_ids),
            meteor_scene::stage_loader::get_stage_center("sn01", None),
        );

        for fps in [30, 60, 120, 125] {
            for route in [3, 4, 5, 6] {
                let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
                h.replace_stage_mesh(mesh.clone());
                h.app
                    .world_mut()
                    .entity_mut(h.fighter)
                    .insert(crate::components::fighter::VisualRootPin::default());
                crate::plugins::character_plugin::spawn_fighter_model_for_test(
                    h.app.world_mut(),
                    h.fighter,
                );
                let schedule = crate::plugins::fighter_frame::schedule(&h.app);
                h.app.add_systems(
                    schedule,
                    crate::plugins::character_plugin::drive_fighter_animation_for_test
                        .before(harness_retire_entry_blend)
                        .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
                );
                let start = Vec3::new(-31.3, 0.0, -26.5);
                assert!(!mesh.box_blocked(start.x, start.y, start.z));
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation = start;
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .facing = 1.62;
                h.app
                    .world_mut()
                    .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                    .yaw = 1.62;
                h.press_key(KeyCode::KeyW);
                let mut saw_attack = false;
                let mut saw_grounded_attack = false;
                for tick in 0..fps * 4 {
                    if route == 6 && tick == fps * 3 / 5 {
                        crate::plugins::camera_plugin::install_camera_input_for_test(&mut h.app);
                        crate::plugins::camera_plugin::install_follow_camera_for_test(&mut h.app);
                        h.app.insert_resource(State::new(AppState::Playing));
                        h.app.world_mut().spawn((
                            bevy::window::PrimaryWindow,
                            bevy::window::CursorOptions {
                                grab_mode: bevy::window::CursorGrabMode::Locked,
                                ..default()
                            },
                        ));
                    }

                    if tick == fps / 10 {
                        h.press_key(KeyCode::Space);
                    }
                    if tick == fps / 10 + 1 {
                        h.release_key(KeyCode::Space);
                    }
                    let attack_tick = if route == 3 { fps * 2 / 5 } else { fps * 4 / 5 };
                    if route >= 5 && tick == fps * 2 / 5 {
                        h.release_key(KeyCode::KeyW);
                    }
                    if route == 6 && tick == fps * 7 / 10 {
                        h.app
                            .world_mut()
                            .write_message(bevy::input::mouse::MouseMotion {
                                delta: Vec2::new(160.0, 0.0),
                            });
                    }
                    if tick == attack_tick {
                        if route == 6 {
                            let facing =
                                h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                            assert!(
                                (facing - 1.62).abs() > 0.1,
                                "mouse must turn the actor after release"
                            );
                        }
                        if route >= 5 {
                            assert!(h.airborne(), "delayed route must still be airborne");
                            assert!(
                                matches!(h.pose_id(), 152 | 180),
                                "delayed route pose {}",
                                h.pose_id()
                            );
                        }
                        h.release_key(KeyCode::KeyW);
                    }
                    if tick == attack_tick + 1 {
                        h.press_key(KeyCode::KeyW);
                    }
                    if tick == attack_tick + 2 {
                        h.release_key(KeyCode::KeyW);
                        h.press_key(KeyCode::KeyS);
                    }
                    if tick == attack_tick + 3 {
                        h.release_key(KeyCode::KeyS);
                        h.press_mouse(MouseButton::Left);
                    }
                    if tick == attack_tick + 4 {
                        h.release_mouse(MouseButton::Left);
                    }
                    h.step_dt(1.0 / fps as f32);
                    if h.pose_id() == 256 {
                        saw_attack = true;
                        saw_grounded_attack |= !h.airborne();
                        let pin = h
                            .app
                            .world()
                            .get::<crate::components::fighter::VisualRootPin>(h.fighter)
                            .unwrap();
                        let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
                        let common = &h.app.world().resource::<CommonAmb>().0;
                        let expected =
                            assets.motion_root_at(common, 0, assets.poses[&0].start as f32);
                        assert!((pin.baseline[2] - expected[2]).abs() < 0.01,
                            "wall release -> ghost vine must reset wall visual height: route={route} tick={tick} pin={:?} expected={expected:?}", pin.baseline);
                    }
                }
                assert!(
                    saw_attack && saw_grounded_attack,
                    "fps={fps} route={route}: exercise attack landing"
                );
            }
        }
    }

    #[test]
    fn production_sn01_reported_platform_jump() {
        use meteor_physics::collision::MeshCollider;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn01/sn01.des")).unwrap(),
        );
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn01/sn01.gmb")).unwrap())
                .unwrap();
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn01/sn01.cob")).unwrap())
            .unwrap();
        let centre = meteor_scene::stage_loader::get_stage_center("sn01", None);
        let mesh = StageMeshCollider::from_gmdl(&gmdl, &desc, Some(&cob.face_ids), centre);
        let keymap = crate::app_state::Keymap::default();
        for (fps, strong) in [30, 60, 120, 165]
            .into_iter()
            .flat_map(|fps| [(fps, false), (fps, true)])
        {
            let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
            h.replace_stage_mesh(mesh.clone());

            let start = Vec3::new(-12.42, 6.6, 16.85);
            assert!(!mesh.box_blocked(start.x, start.y, start.z));
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = start;
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = 2.24;
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw = 2.24;
            h.press_binding(keymap.move_up);
            let press_tick = fps / 10;
            let hold = if strong { fps / 4 } else { 1 };
            let mut reached = false;
            let mut peak = start.y;
            for tick in 0..fps * 3 {
                if tick == press_tick {
                    h.press_binding(keymap.jump);
                }
                if tick == press_tick + hold {
                    h.release_binding(keymap.jump);
                }
                h.step_dt(1.0 / fps as f32);
                let p = h.position();
                peak = peak.max(p.y);
                assert!(
                    !mesh.box_blocked(p.x, p.y, p.z),
                    "fps={fps} strong={strong} tick={tick} p={p:?}"
                );
                if !h.airborne() && (p.y - 17.682145).abs() < 0.01 {
                    reached = true;
                    break;
                }
            }
            eprintln!(
                "fps={fps} strong={strong} reached={reached} peak={peak} final={:?}",
                h.position()
            );
            if fps == 165 && !strong {
                assert!(!reached);
                assert!((peak - start.y).abs() < 0.01);
                continue;
            }
            assert!(
                reached,
                "sn01 platform: fps={fps} strong={strong} peak={peak} final={:?}",
                h.position()
            );
        }
    }

    #[test]
    fn production_sn01_jump_wall_paths_keep_the_body_outside_geometry() {
        use meteor_physics::collision::MeshCollider;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn01/sn01.des")).unwrap(),
        );
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn01/sn01.gmb")).unwrap())
                .unwrap();
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn01/sn01.cob")).unwrap())
            .unwrap();
        let centre = meteor_scene::stage_loader::get_stage_center("sn01", None);
        let mesh = StageMeshCollider::from_gmdl(&gmdl, &desc, Some(&cob.face_ids), centre);
        let keymap = crate::app_state::Keymap::default();

        let npc_start = Vec3::new(-117.67598, 19.5157, 12.211783);
        assert!(!mesh.box_blocked(npc_start.x, npc_start.y, npc_start.z));
        let unsafe_floor = mesh
            .sample_ground_hit(npc_start.x, npc_start.z, 0.0, npc_start.y)
            .unwrap();
        assert!((0.0..meteor_physics::jump::STEP_DOWN).contains(&(npc_start.y - unsafe_floor.y)));
        assert!(mesh.box_blocked(npc_start.x, unsafe_floor.y, npc_start.z));

        let mut mover = meteor_physics::collision::CollisionSystem::new(0.55, 1.8);
        mover.set_mesh_collider(mesh.clone());
        let mut transform = Transform::from_translation(npc_start);
        let mut motion = MotionState {
            velocity: Vec3::ZERO,
            facing: 0.0,
            movement_mode: meteor_core::MovementMode::Idle,
        };
        let mut air = FighterAir::default();
        crate::plugins::ai_plugin::advance_npc_motion(
            Entity::PLACEHOLDER,
            None,
            &mover,
            &mut transform,
            &mut motion,
            &mut air,
            Vec3::ZERO,
            HARNESS_DT,
            false,
        );
        assert!(!mesh.box_blocked(
            transform.translation.x,
            transform.translation.y,
            transform.translation.z
        ));

        let pad = desc
            .dummies
            .iter()
            .chain(&desc.scene_objects)
            .find(|o| o.name_attr.as_deref() == Some("user01"))
            .expect("sn01 user01");
        let x = (pad.position.x - centre[0]) * meteor_core::MODEL_SCALE;
        let z = -(pad.position.y - centre[1]) * meteor_core::MODEL_SCALE;
        let ground = mesh
            .sample_ground_hit(
                x,
                z,
                0.0,
                (pad.position.z - centre[2]) * meteor_core::MODEL_SCALE,
            )
            .expect("user01 ground");
        assert!(!mesh.box_blocked(x, ground.y, z));
        for degrees in [45.0_f32, 135.0] {
            let mut h = spawn_gameplay_harness(DAGGER).unwrap();
            h.replace_stage_mesh(mesh.clone());
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = Vec3::new(x, ground.y, z);
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = degrees.to_radians();
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw = degrees.to_radians();
            h.press_binding(keymap.move_up);
            for tick in 0..120 {
                if tick % 30 == 9 {
                    h.press_binding(keymap.jump);
                }
                if tick % 30 == 10 {
                    h.release_binding(keymap.jump);
                }
                let before = h.position();
                h.step_dt(1.0 / 30.0);
                let after = h.position();
                assert!(!mesh.box_blocked(after.x,after.y,after.z),"sn01 penetration pad={:?} degrees={degrees} tick={tick} before={before:?} after={after:?} pose={} air={}",pad.name_attr,h.pose_id(),h.airborne());
            }
        }
    }

    #[test]
    fn production_sn15_reported_wall_blocks_walking_and_spear292() {
        use meteor_physics::collision::{CollisionSystem, MeshCollider};
        let mesh = crate::plugins::stage_collider::sn15_test_mesh();
        let start = Vec3::new(17.9, 0.0, 54.7);
        let check = |p: Vec3| {
            assert!(
                !mesh.box_blocked(p.x, p.y, p.z),
                "sn15 wall overlap at {p:?}"
            );
            assert!(
                p.y >= 3.6 || p.z < 55.0,
                "sn15 crossed the reported wall below its top: {p:?}"
            );
        };
        for fps in [30, 60, 120] {
            let dt = 1.0 / fps as f32;
            let mut h = spawn_gameplay_harness(DAGGER).expect("original P1 assets");
            h.replace_stage_mesh(mesh.clone());
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = start;
            let keys = crate::app_state::Keymap::default();
            h.press_binding(keys.move_up);
            for _ in 0..fps * 2 {
                h.step_dt(dt);
                check(h.position());
            }
            assert!(h.position().z > start.z, "walking must reach the wall");

            let mut mover = CollisionSystem::new(0.55, 1.8);
            mover.set_mesh_collider(&mesh);
            let mut transform = Transform::from_translation(start);
            let mut motion = MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            };
            let mut air = FighterAir::default();
            for _ in 0..fps * 2 {
                crate::plugins::ai_plugin::advance_npc_motion(
                    Entity::PLACEHOLDER,
                    None,
                    &mover,
                    &mut transform,
                    &mut motion,
                    &mut air,
                    Vec3::new(0.0, 0.0, 12.0),
                    dt,
                    false,
                );
                check(transform.translation);
            }

            for npc in [false, true] {
                for reaction_pose in [103, 104, 105, 106] {
                    let start = Vec3::new(17.9, 0.0, 53.7);
                    let mut h = spawn_gameplay_harness(DAGGER).expect("original P1 assets");
                    h.replace_stage_mesh(mesh.clone());
                    h.app.add_systems(
                        Update,
                        hit_reaction_root_motion
                            .after(crate::plugins::movement_plugin::attack_root_motion)
                            .in_set(crate::plugins::FighterFrameSet::RootMotion),
                    );
                    if npc {
                        h.app
                            .world_mut()
                            .entity_mut(h.fighter)
                            .remove::<LocalFighter>()
                            .insert(NpcFighter);
                    }
                    let (pose, mut reaction, facing) = {
                        let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
                        let attack = &assets.poses[&292].attacks[0];
                        assert_eq!(attack.target_move, 3.0);
                        assert!([
                            attack.target_pose,
                            attack.target_pose_front,
                            attack.target_pose_back,
                            attack.target_pose_left,
                            attack.target_pose_right
                        ]
                        .contains(&Some(reaction_pose)));
                        let mut pose = PoseRuntime::idle();
                        assets.apply_pose(&mut pose, reaction_pose, true);
                        let mut reaction =
                            getup_reaction_for_pose(reaction_pose, pose.start, pose.end);
                        reaction.kind = HitReactKind::BeHit;
                        reaction.motion_scale = attack.target_move;
                        reaction.minimum_remaining = attack.target_value;
                        reaction.authored_minimum = attack.target_value;
                        let common = &h.app.world().resource::<CommonAmb>().0;
                        let first = assets.motion_root_at(common, reaction_pose, pose.start as f32);
                        let last = assets.motion_root_at(common, reaction_pose, pose.end as f32);

                        let (x, z) = meteor_anim::root_motion::root_delta_to_world_xz(
                            last[0] - first[0],
                            last[1] - first[1],
                            0.0,
                            1.0,
                        );
                        (pose, reaction, -x.atan2(z))
                    };
                    reaction.lock_remaining =
                        reaction.lock_remaining.max(reaction.minimum_remaining);
                    let mut runtime = FighterHitReaction::default();
                    runtime.begin(reaction, false);
                    h.app.world_mut().entity_mut(h.fighter).insert((
                        pose,
                        runtime,
                        Transform::from_translation(start),
                    ));
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(h.fighter)
                        .unwrap()
                        .facing = facing;
                    h.app
                        .world_mut()
                        .get_mut::<SmState>(h.fighter)
                        .unwrap()
                        .current = FighterState::BeHit;
                    h.app
                        .world_mut()
                        .get_mut::<CombatState>(h.fighter)
                        .unwrap()
                        .state = FighterState::BeHit;
                    for _ in 0..fps * 4 {
                        h.step_dt(dt);
                        check(h.position());
                    }
                    assert!(!h.airborne(), "sn15 spear292 npc={npc} reaction={reaction_pose} fps={fps} remains airborne at {:?}", h.position());
                    assert!(h.position().distance(start) > 0.02, "reaction must exercise actual root motion: npc={npc} pose={reaction_pose} fps={fps} at {:?}", h.position());
                }
            }
        }
    }

    #[test]
    fn production_sn15_combat_movers_commit_safe_xyz_for_player_and_npc() {
        use bevy::ecs::system::RunSystemOnce;
        use meteor_physics::collision::MeshCollider;
        let mesh = crate::plugins::stage_collider::sn15_test_mesh();
        for npc in [false, true] {
            for mode in ["attack", "reaction", "down"] {
                for fps in [30, 60, 120] {
                    let mut h = spawn_gameplay_harness(DAGGER).unwrap();
                    h.replace_stage_mesh(mesh.clone());
                    let p = Vec3::new(2.7278004, 8.0, 46.980072);
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(h.fighter)
                        .unwrap()
                        .translation = p;
                    if npc {
                        h.app
                            .world_mut()
                            .entity_mut(h.fighter)
                            .remove::<LocalFighter>()
                            .insert(NpcFighter);
                    }
                    let dt = 1.0 / fps as f32;
                    h.app
                        .world_mut()
                        .insert_resource(crate::plugins::FrameClock {
                            sim_dt: dt,
                            ..default()
                        });
                    let carry = 0.25 / (dt * (1.0 - 9.0 * dt));
                    let id = if mode == "attack" {
                        258
                    } else if mode == "down" {
                        112
                    } else {
                        92
                    };
                    let mut pose = PoseRuntime::idle();
                    h.app.world().resource::<CharacterAssetsSet>().0["p1"]
                        .apply_pose(&mut pose, id, true);
                    let mut reaction = getup_reaction_for_pose(id, pose.start, pose.end);
                    reaction.kind = if mode == "down" {
                        HitReactKind::Down
                    } else {
                        HitReactKind::BeHit
                    };
                    h.app.world_mut().entity_mut(h.fighter).insert(pose);
                    if mode == "attack" {
                        h.app
                            .world_mut()
                            .get_mut::<SmState>(h.fighter)
                            .unwrap()
                            .current = FighterState::Attack1;
                        h.app
                            .world_mut()
                            .get_mut::<MotionState>(h.fighter)
                            .unwrap()
                            .velocity
                            .z = carry;
                        h.app
                            .world_mut()
                            .run_system_once(crate::plugins::movement_plugin::attack_root_motion)
                            .unwrap();
                    } else {
                        let mut runtime = h
                            .app
                            .world_mut()
                            .get_mut::<FighterHitReaction>(h.fighter)
                            .unwrap();
                        runtime.current = Some(reaction);
                        runtime.carry_z = carry;
                        h.app
                            .world_mut()
                            .run_system_once(hit_reaction_root_motion)
                            .unwrap();
                    }
                    let after = h.position();
                    assert!(after.z > p.z + 0.2, "{mode} must exercise wall movement");
                    assert!(
                        !mesh.box_blocked(after.x, after.y, after.z),
                        "sn15 {mode} npc={npc} fps={fps} before={p:?} after={after:?}"
                    );
                    if !npc && mode != "down" {
                        for _ in 0..90 {
                            h.step_dt(dt);
                            let settled = h.position();
                            assert!(
                                !mesh.box_blocked(settled.x, settled.y, settled.z),
                                "sn15 {mode} settling at {settled:?}"
                            );
                        }
                        assert!(
                            !h.airborne(),
                            "sn15 {mode} must not stay in an air pose at {:?}",
                            h.position()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn production_sn15_spawn_wall_paths_keep_the_body_outside_geometry() {
        use meteor_physics::collision::MeshCollider;
        let mesh = crate::plugins::stage_collider::sn15_test_mesh();
        let keymap = crate::app_state::Keymap::default();
        for degrees in (0..360).step_by(45) {
            let mut h = spawn_gameplay_harness(DAGGER).unwrap();
            h.replace_stage_mesh(mesh.clone());
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = Vec3::new(26.2278, 3.6, 70.23007);
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = (degrees as f32).to_radians();
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw = (degrees as f32).to_radians();
            h.press_binding(keymap.move_up);
            for tick in 0..240 {
                if tick % 60 == 9 {
                    h.press_binding(keymap.jump);
                }
                if tick % 60 == 10 {
                    h.release_binding(keymap.jump);
                }
                if tick % 60 == 30 {
                    h.press_binding(keymap.attack);
                }
                if tick % 60 == 31 {
                    h.release_binding(keymap.attack);
                }
                let before = h.position();
                h.step_dt(1.0 / 30.0);
                let p = h.position();
                assert!(!mesh.box_blocked(p.x,p.y,p.z), "sn15 degrees={degrees} tick={tick} before={before:?} after={p:?} pose={} airborne={}", h.pose_id(),h.airborne());
            }
        }
    }

    fn sn14_test_mesh() -> StageMeshCollider {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn14/sn14.des")).unwrap(),
        );
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn14/sn14.gmb")).unwrap())
                .unwrap();
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn14/sn14.cob")).unwrap())
            .unwrap();
        let wp = std::fs::read_to_string(root.join("Level/sn14.wp")).unwrap();
        let center = meteor_scene::stage_loader::get_stage_center("sn14", Some(&wp));
        let mut mesh = StageMeshCollider::from_gmdl(&gmdl, &desc, Some(&cob.face_ids), center);
        crate::plugins::render_plugin::append_solid_props_for_test(&root, &desc, center, &mut mesh);
        mesh
    }

    #[test]
    fn production_sn14_spawn_box_rim_releases_npc_and_local_fighter() {
        use meteor_physics::collision::MeshCollider;
        let mesh = sn14_test_mesh();
        let mut mover = meteor_physics::collision::CollisionSystem::new(0.55, 1.8);
        mover.set_mesh_collider(&mesh);

        let approach = Vec3::new(2.1989815, 7.88952, 16.51384);
        let rim = Vec3::new(-0.93525547, 0.16641295, 20.405142);
        assert!(!mesh.box_blocked(rim.x, rim.y, rim.z));
        assert!(mesh.box_blocked(rim.x, rim.y - 0.12, rim.z));
        for fps in [30, 60, 120] {
            let dt = 1.0 / fps as f32;
            let mut transform = Transform::from_translation(approach);
            let mut motion = MotionState {
                velocity: Vec3::ZERO,
                facing: 0.0,
                movement_mode: meteor_core::MovementMode::Airborne,
            };
            let mut air = FighterAir::default();
            let angle = 7.0 * std::f32::consts::FRAC_PI_4;
            air.air.start_fall(12.0 * angle.sin(), 12.0 * angle.cos());
            for _ in 0..fps * 6 {
                crate::plugins::ai_plugin::advance_npc_motion(
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
            }
            assert!(
                !air.air.airborne,
                "NPC {fps} FPS end={:?}",
                transform.translation
            );
            assert!(!mesh.box_blocked(
                transform.translation.x,
                transform.translation.y,
                transform.translation.z
            ));

            for start in [approach, rim] {
                let mut h = spawn_gameplay_harness(DAGGER).unwrap();
                h.replace_stage_mesh(mesh.clone());
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation = start;
                h.app
                    .world_mut()
                    .get_mut::<FighterAir>(h.fighter)
                    .unwrap()
                    .air
                    .start_fall(
                        if start == approach {
                            12.0 * angle.sin()
                        } else {
                            0.0
                        },
                        if start == approach {
                            12.0 * angle.cos()
                        } else {
                            0.0
                        },
                    );
                for _ in 0..fps * 6 {
                    h.step_dt(dt);
                }
                assert!(
                    !h.airborne(),
                    "local {fps} FPS start={start:?} end={:?} pose={}",
                    h.position(),
                    h.pose_id()
                );
                assert!(!h
                    .app
                    .world()
                    .get::<PoseRuntime>(h.fighter)
                    .unwrap()
                    .is_fall_hold());
                assert!(!mesh.box_blocked(h.position().x, h.position().y, h.position().z));

                let before = h.position();
                h.app
                    .world_mut()
                    .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                    .yaw = 0.0;
                let keymap = crate::app_state::Keymap::default();
                h.press_binding(keymap.move_up);
                for _ in 0..fps {
                    h.step_dt(dt);
                }
                assert!(
                    h.position().distance(before) > 1.0,
                    "local {fps} FPS must be able to leave the box rim"
                );
            }
        }
    }

    fn sn06_test_mesh() -> StageMeshCollider {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn06/sn06.gmb")).unwrap())
                .unwrap();
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn06/sn06.des")).unwrap(),
        );
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn06/sn06.cob")).unwrap())
            .unwrap();
        let wp = std::fs::read_to_string(root.join("Level/sn06.wp")).unwrap();
        let center = meteor_scene::stage_loader::get_stage_center("sn06", Some(&wp));
        StageMeshCollider::from_gmdl(&gmdl, &desc, Some(&cob.face_ids), center)
    }

    #[test]
    fn production_sn06_missing_support_does_not_create_zero_height_floor() {
        let mesh = sn06_test_mesh();
        let start = Vec3::new(-33.3, 0.0, 66.6);
        let mut mover = meteor_physics::collision::CollisionSystem::new(0.55, 1.8);
        mover.set_mesh_collider(mesh.clone());
        assert!(mover.sample_ground_hit(start.x, start.z, start.y).is_none());
        for fps in [30, 60, 120] {
            let dt = 1.0 / fps as f32;
            for be_hit in [false, true] {
                let mut h = spawn_gameplay_harness(DAGGER).expect("original P1 assets");
                h.replace_stage_mesh(mesh.clone());
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation = start;
                if be_hit {
                    let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
                    let mut pose = PoseRuntime::idle();
                    assets.apply_pose(&mut pose, 92, true);
                    let mut reaction = getup_reaction_for_pose(92, pose.start, pose.end);
                    reaction.kind = HitReactKind::BeHit;
                    let mut runtime = FighterHitReaction::default();
                    runtime.begin(reaction, false);
                    h.app
                        .world_mut()
                        .entity_mut(h.fighter)
                        .insert((pose, runtime));
                    h.app
                        .world_mut()
                        .get_mut::<SmState>(h.fighter)
                        .unwrap()
                        .current = FighterState::BeHit;
                    h.app
                        .world_mut()
                        .get_mut::<CombatState>(h.fighter)
                        .unwrap()
                        .state = FighterState::BeHit;
                }
                for _ in 0..fps {
                    h.step_dt(dt);
                    assert!(h.position().is_finite());
                }
                assert!(
                    h.position().y < -1.0,
                    "fps={fps} be_hit={be_hit}: {:?}",
                    h.position()
                );
            }
            let mut transform = Transform::from_translation(start);
            let mut motion = MotionState {
                velocity: Vec3::ZERO,
                facing: 0.0,
                movement_mode: meteor_core::MovementMode::Idle,
            };
            let mut air = FighterAir::default();
            for _ in 0..fps {
                crate::plugins::ai_plugin::advance_npc_motion(
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
                assert!(transform.translation.is_finite());
            }
            assert!(
                transform.translation.y < -1.0,
                "npc fps={fps}: {:?}",
                transform.translation
            );
        }
    }

    #[test]
    fn production_sn19_wall_keeps_the_exe_eighteen_unit_clearance() {
        use meteor_physics::collision::MeshCollider;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn19/sn19.gmb")).unwrap())
                .unwrap();
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn19/sn19.des")).unwrap(),
        );
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn19/sn19.cob")).unwrap())
            .unwrap();
        let wp = std::fs::read_to_string(root.join("Level/sn19.wp")).unwrap();
        let centre = meteor_scene::stage_loader::get_stage_center("sn19", Some(&wp));
        let mesh = StageMeshCollider::from_gmdl(&gmdl, &desc, Some(&cob.face_ids), centre);
        for fps in [30, 60, 120] {
            let mut h = spawn_gameplay_harness(DAGGER).unwrap();
            h.replace_stage_mesh(mesh.clone());
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = Vec3::new(-31.1, 0.1, 87.5);
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = std::f32::consts::PI;
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw = std::f32::consts::PI;
            let keymap = crate::app_state::Keymap::default();
            h.press_binding(keymap.move_up);
            for _ in 0..fps {
                if h.position().z <= 84.1 {
                    break;
                }
                h.step_dt(1.0 / fps as f32);
            }
            h.press_binding(keymap.jump);
            h.step_dt(1.0 / fps as f32);
            h.release_binding(keymap.jump);
            let mut entered = false;
            for _ in 0..fps {
                h.step_dt(1.0 / fps as f32);
                let p = h.position();
                assert!(!mesh.box_blocked(p.x, p.y, p.z), "{fps}: {p:?}");
                if h.app
                    .world()
                    .get::<FighterWall>(h.fighter)
                    .unwrap()
                    .cling_active
                {
                    entered = true;
                    break;
                }
            }
            assert!(
                entered,
                "{fps}: failed to reach sn19 front wall: {:?}",
                h.position()
            );

            assert!(
                h.position().z < 84.68,
                "{fps}: entry moved to the wall offset early"
            );
            h.step_dt(1.0 / fps as f32);
            let p = h.position();
            assert!((p.z - (82.52011 + 2.16)).abs() < 0.002, "{fps}: {p:?}");
            assert!(!mesh.box_blocked(p.x, p.y, p.z));
        }
    }

    #[test]
    #[ignore = "diagnostic for sn19 rooftop locomotion; not an original-game parity assertion"]
    fn diagnostic_sn19_rooftop_locomotion() {
        use meteor_physics::collision::MeshCollider;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn19/sn19.gmb")).unwrap())
                .unwrap();
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn19/sn19.des")).unwrap(),
        );
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn19/sn19.cob")).unwrap())
            .unwrap();
        let wp = std::fs::read_to_string(root.join("Level/sn19.wp")).unwrap();
        let center = meteor_scene::stage_loader::get_stage_center("sn19", Some(&wp));
        let mesh = StageMeshCollider::from_gmdl(&gmdl, &desc, Some(&cob.face_ids), center);
        let keymap = crate::app_state::Keymap::default();
        let mut sampled = 0;
        for (tri, indices) in mesh.indices.chunks_exact(3).enumerate() {
            if std::env::var_os("METEOR_ROOF_RECOVERY").is_some() && tri != 142 {
                continue;
            }
            let a = Vec3::from_array(mesh.positions[indices[0] as usize]);
            let b = Vec3::from_array(mesh.positions[indices[1] as usize]);
            let c = Vec3::from_array(mesh.positions[indices[2] as usize]);
            let normal = (b - a).cross(c - a).normalize_or_zero();
            let start = (a + b + c) / 3.0;
            if start.y < 5.0 || normal.y.abs() < 0.42 || normal.y.abs() > 0.98 {
                continue;
            }
            let Some(hit) = mesh.sample_ground_hit(start.x, start.z, 0.0, start.y + 0.1) else {
                continue;
            };
            if (hit.y - start.y).abs() > 0.05 {
                continue;
            }
            let blocked = mesh.box_blocked(start.x, start.y, start.z);
            sampled += 1;
            for degrees in [0.0_f32, 90.0, 180.0, 270.0] {
                let angle = degrees.to_radians();
                let mut h = spawn_gameplay_harness(DAGGER).unwrap();
                h.replace_stage_mesh(mesh.clone());
                if std::env::var_os("METEOR_ROOF_RECOVERY").is_some() {
                    let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                    air.recovery_ground_mode = Some(true);
                    air.ground_contact = true;
                }
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation = start;
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .facing = angle;
                h.app
                    .world_mut()
                    .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                    .yaw = angle;
                h.press_binding(keymap.move_up);
                let mut air_ticks = 0;
                for _ in 0..60 {
                    h.step_dt(1.0 / 60.0);
                    air_ticks += usize::from(h.airborne());
                }
                eprintln!(
                    "ROOF tri={tri} start={start:?} ny={} blocked={blocked} angle={degrees} end={:?} distance={} air_ticks={air_ticks}",
                    normal.y,
                    h.position(),
                    (h.position() - start).with_y(0.0).length()
                );
            }
        }
        assert!(sampled > 0);
    }

    #[test]
    fn production_sn19_roof_recovery_preserves_running_speed() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn19/sn19.gmb")).unwrap())
                .unwrap();
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn19/sn19.des")).unwrap(),
        );
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn19/sn19.cob")).unwrap())
            .unwrap();
        let wp = std::fs::read_to_string(root.join("Level/sn19.wp")).unwrap();
        let center = meteor_scene::stage_loader::get_stage_center("sn19", Some(&wp));
        let mesh = StageMeshCollider::from_gmdl(&gmdl, &desc, Some(&cob.face_ids), center);
        let start = Vec3::new(-39.105392, 27.047083, 80.470406);
        let keymap = crate::app_state::Keymap::default();
        for fps in [30, 60, 125, 165] {
            for angle in [0.0, std::f32::consts::PI] {
                let mut distances = Vec::new();
                for recovery in [false, true] {
                    let mut h = spawn_gameplay_harness(DAGGER).unwrap();
                    h.replace_stage_mesh(mesh.clone());
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(h.fighter)
                        .unwrap()
                        .translation = start;
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(h.fighter)
                        .unwrap()
                        .facing = angle;
                    h.app
                        .world_mut()
                        .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                        .yaw = angle;
                    if recovery {
                        let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                        air.recovery_ground_mode = Some(true);
                        air.ground_contact = true;
                    }
                    h.press_binding(keymap.move_up);
                    for _ in 0..fps {
                        h.step_dt(1.0 / fps as f32);
                    }
                    let delta = h.position() - start;
                    assert!(
                        delta.x.abs() < 0.02,
                        "{fps} angle={angle} recovery={recovery}: sideways drift {delta:?}"
                    );
                    distances.push(delta.z.abs());
                }

                assert!(
                    (distances[0] - distances[1]).abs() < 0.4,
                    "{fps} angle={angle}: ordinary/recovered roof distances {distances:?}"
                );
                assert!(distances[1] > 16.0);
            }
        }
    }

    #[test]
    #[ignore = "diagnostic for the reported sn19 house contact"]
    fn diagnostic_sn19_reported_house_contact() {
        use meteor_physics::collision::{MeshCollider, Point3};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn19/sn19.gmb")).unwrap())
                .unwrap();
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn19/sn19.des")).unwrap(),
        );
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn19/sn19.cob")).unwrap())
            .unwrap();
        let wp = std::fs::read_to_string(root.join("Level/sn19.wp")).unwrap();
        let center = meteor_scene::stage_loader::get_stage_center("sn19", Some(&wp));
        let mesh = StageMeshCollider::from_gmdl(&gmdl, &desc, Some(&cob.face_ids), center);
        let p = Vec3::new(-31.1, 0.1, 83.5);
        eprintln!(
            "SN19 center={center:?} blocked={} ground={:?}",
            mesh.box_blocked(p.x, p.y, p.z),
            mesh.sample_ground_hit(p.x, p.z, 0.0, p.y)
        );
        for (tri, indices) in mesh.indices.chunks_exact(3).enumerate() {
            let vertices: Vec<_> = indices
                .iter()
                .map(|i| mesh.positions[*i as usize])
                .collect();
            if vertices.iter().map(|v| v[0]).fold(f32::INFINITY, f32::min) > p.x + 0.96
                || vertices
                    .iter()
                    .map(|v| v[0])
                    .fold(f32::NEG_INFINITY, f32::max)
                    < p.x - 0.96
                || vertices.iter().map(|v| v[2]).fold(f32::INFINITY, f32::min) > p.z + 0.96
                || vertices
                    .iter()
                    .map(|v| v[2])
                    .fold(f32::NEG_INFINITY, f32::max)
                    < p.z - 0.96
            {
                continue;
            }
            let mut one = StageMeshCollider::default();
            one.append_world_triangles(&vertices, &[0, 1, 2]);
            if one.box_blocked(p.x, p.y, p.z) {
                eprintln!("SN19 overlap tri={tri} vertices={vertices:?}");
            }
        }
        for obj in &gmdl.objects {
            let mut lo = Vec3::splat(f32::INFINITY);
            let mut hi = Vec3::splat(f32::NEG_INFINITY);
            for v in obj.positions.chunks_exact(3) {
                let w = Vec3::new(v[0] - center[0], v[2] - center[2], -(v[1] - center[1]))
                    * meteor_core::MODEL_SCALE;
                lo = lo.min(w);
                hi = hi.max(w);
            }
            if p.x >= lo.x - 3.0 && p.x <= hi.x + 3.0 && p.z >= lo.z - 3.0 && p.z <= hi.z + 3.0 {
                eprintln!("SN19 object={} lo={lo:?} hi={hi:?}", obj.name);
            }
        }
        let keymap = crate::app_state::Keymap::default();
        for degrees in (0..360).step_by(30) {
            let angle = (degrees as f32).to_radians();
            let dir = Point3::new(angle.sin(), 0.0, angle.cos());
            for height in [1.05, 2.52] {
                eprintln!(
                    "SN19 ray angle={degrees} h={height} hit={:?}",
                    mesh.raycast_hit(Point3::new(p.x, p.y + height, p.z), dir, 2.4)
                );
            }
            for (jumping, approach) in [(false, false), (true, false), (false, true), (true, true)]
            {
                let start = if approach {
                    p - Vec3::new(dir.x, 0.0, dir.z) * 4.0
                } else {
                    p
                };
                if approach && mesh.box_blocked(start.x, start.y, start.z) {
                    continue;
                }
                let mut h = spawn_gameplay_harness(DAGGER).unwrap();
                h.replace_stage_mesh(mesh.clone());
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation = start;
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .facing = angle;
                h.app
                    .world_mut()
                    .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                    .yaw = angle;
                h.press_binding(keymap.move_up);
                let mut clung = false;
                let mut penetration = None;
                let mut peak = p.y;
                let mut kicked = false;
                for tick in 0..180 {
                    if jumping && tick % 30 == 9 {
                        h.press_binding(keymap.jump);
                    }
                    if jumping && tick % 30 == 10 {
                        h.release_binding(keymap.jump);
                    }
                    let before = h.position();
                    h.step_dt(1.0 / 60.0);
                    let q = h.position();
                    peak = peak.max(q.y);
                    kicked |= h
                        .app
                        .world()
                        .get::<FighterWall>(h.fighter)
                        .unwrap()
                        .kick_pose
                        != 0;
                    if mesh.box_blocked(q.x, q.y, q.z) && penetration.is_none() {
                        penetration = Some((tick, before, q));
                    }
                    clung |= h
                        .app
                        .world()
                        .get::<FighterWall>(h.fighter)
                        .unwrap()
                        .cling_active;
                }
                eprintln!("SN19 path angle={degrees} jump={jumping} approach={approach} start={start:?} end={:?} pose={} air={} clung={clung} kicked={kicked} peak={peak} penetration={penetration:?}",h.position(),h.pose_id(),h.airborne());
                if approach {
                    assert!(
                        penetration.is_none(),
                        "clear-start sn19 trajectory entered geometry"
                    );
                }
            }
        }
    }

    fn sn13_test_mesh() -> StageMeshCollider {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn13/sn13.gmb")).unwrap())
                .unwrap();
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn13/sn13.des")).unwrap(),
        );
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn13/sn13.cob")).unwrap())
            .unwrap();
        let wp = std::fs::read_to_string(root.join("Level/sn13.wp")).unwrap();
        let center = meteor_scene::stage_loader::get_stage_center("sn13", Some(&wp));
        StageMeshCollider::from_gmdl(&gmdl, &desc, Some(&cob.face_ids), center)
    }

    fn assert_sn13_slide_finished(mesh: &StageMeshCollider, start: Vec3, end: Vec3) {
        let support = meteor_physics::collision::MeshCollider::sample_ground_hit(
            mesh, end.x, end.z, 0.0, end.y,
        );
        assert!(
            support.is_some_and(|hit| hit.ny >= 0.42 && (hit.y - end.y).abs() < 0.3),
            "must reach real walkable support: start={start:?} end={end:?} support={support:?}"
        );
        assert!(
            end.y < start.y - 2.0,
            "outer slope must release a fighter with no input: start={start:?} end={end:?}"
        );
    }

    #[test]
    fn production_sn13_slope_contact_releases_dagger_hold_before_landing() {
        let mesh = sn13_test_mesh();
        for fps in [30.0, 60.0, 120.0, 165.0] {
            let mut h =
                spawn_gameplay_harness_with_order(DAGGER, true).expect("original character assets");
            h.app.world_mut().entity_mut(h.fighter).insert((
                HeadTrackState::default(),
                crate::components::fighter::VisualRootPin::default(),
            ));
            crate::plugins::character_plugin::spawn_fighter_model_for_test(
                h.app.world_mut(),
                h.fighter,
            );
            let actor_schedule = crate::plugins::fighter_frame::schedule(&h.app);
            h.app.add_systems(
                actor_schedule,
                crate::plugins::character_plugin::drive_fighter_animation_for_test
                    .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
            );
            h.replace_stage_mesh(mesh.clone());
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = Vec3::new(-126.66256, 60.0, -123.60386);
            h.app
                .world_mut()
                .get_mut::<FighterAir>(h.fighter)
                .unwrap()
                .air
                .start_fall(0.0, 0.0);
            h.app
                .world_mut()
                .get_mut::<FighterAir>(h.fighter)
                .unwrap()
                .ground_contact = false;
            h.press_mouse(MouseButton::Left);
            h.step_dt(1.0 / fps);
            h.release_mouse(MouseButton::Left);
            assert_eq!(h.pose_id(), 245, "air attack entry at fps={fps}");
            for _ in 0..fps as usize {
                if h.pose_id() == 332 {
                    break;
                }
                h.step_dt(1.0 / fps);
            }
            assert_eq!(h.pose_id(), 332);
            assert!(h
                .app
                .world()
                .get::<PoseRuntime>(h.fighter)
                .unwrap()
                .is_fall_hold());

            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = Vec3::new(-126.66256, 11.306133, -123.60386);
            h.app
                .world_mut()
                .get_mut::<FighterAir>(h.fighter)
                .unwrap()
                .air
                .vertical_speed = -1.0;
            let mut released_in_air = false;
            let mut finished_in_air = false;
            for _ in 0..fps as usize {
                h.step_dt(1.0 / fps);
                let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                released_in_air |= h.airborne() && pose.pose_id == 332 && !pose.loops;
                finished_in_air |=
                    released_in_air && h.airborne() && !is_attacking_state(h.state());
            }
            assert!(
                released_in_air,
                "fps={fps}: steep contact must release 332 before landing"
            );
            assert!(
                finished_in_air,
                "fps={fps}: recovery must finish while sliding"
            );
        }
    }

    #[test]
    fn production_sn13_outer_slope_does_not_hold_a_stationary_fighter() {
        let mesh = sn13_test_mesh();

        for start in [
            Vec3::new(-126.66256, 11.306133, -123.60386),
            Vec3::new(-141.9704, 10.632593, -133.29366),
        ] {
            for (airborne, attack) in [(true, false), (false, false), (true, true)] {
                let mut harness =
                    spawn_gameplay_harness(DAGGER).expect("original character assets");
                harness.replace_stage_mesh(mesh.clone());
                harness
                    .app
                    .world_mut()
                    .get_mut::<Transform>(harness.fighter)
                    .unwrap()
                    .translation = start;
                if airborne {
                    harness
                        .app
                        .world_mut()
                        .get_mut::<FighterAir>(harness.fighter)
                        .unwrap()
                        .air
                        .start_fall(0.0, 0.0);
                }
                if attack {
                    harness.press_mouse(MouseButton::Left);
                    harness.step();
                    harness.release_mouse(MouseButton::Left);
                }
                harness.steps(180);
                assert_sn13_slide_finished(&mesh, start, harness.position());
            }
        }
    }

    #[test]
    fn production_sn13_outer_slope_releases_npc_without_steering() {
        let mesh = sn13_test_mesh();
        let mut mover = meteor_physics::collision::CollisionSystem::new(0.55, 1.8);
        mover.set_mesh_collider(mesh.clone());
        let start = Vec3::new(-126.66256, 11.306133, -123.60386);
        let mut transform = Transform::from_translation(start);
        let mut motion = MotionState {
            velocity: Vec3::ZERO,
            facing: 0.0,
            movement_mode: meteor_core::MovementMode::Airborne,
        };
        let mut air = FighterAir::default();
        air.air.start_fall(0.0, 0.0);
        for _ in 0..180 {
            crate::plugins::ai_plugin::advance_npc_motion(
                Entity::PLACEHOLDER,
                None,
                &mover,
                &mut transform,
                &mut motion,
                &mut air,
                Vec3::ZERO,
                HARNESS_DT,
                false,
            );
        }
        assert_sn13_slide_finished(&mesh, start, transform.translation);
    }

    #[test]
    fn production_wall_kick_separates_then_allows_a_new_ground_jump() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        harness.replace_stage_mesh(harness_ground_with_front_wall(2.0));
        let keymap = crate::app_state::Keymap::default();

        harness.press_binding(keymap.move_up);
        harness.press_binding(keymap.jump);
        harness.step();
        harness.release_binding(keymap.jump);
        harness.step();
        for _ in 0..8 {
            harness.step();
            if harness
                .app
                .world()
                .get::<FighterWall>(harness.fighter)
                .is_some_and(|wall| wall.cling_active)
            {
                break;
            }
        }
        let (cling_pose, wall_normal) = {
            let wall = harness
                .app
                .world()
                .get::<FighterWall>(harness.fighter)
                .unwrap();
            (wall.cling_pose, wall.wall_normal)
        };
        assert_eq!(cling_pose, L::WALL_CLING_F);
        assert!(wall_normal.z < -0.9);

        harness.press_binding(keymap.jump);
        harness.step();
        let before_kick = harness.position();
        harness.release_binding(keymap.jump);
        harness.step();

        let wall_after = harness
            .app
            .world()
            .get::<FighterWall>(harness.fighter)
            .unwrap();
        let air_after = harness
            .app
            .world()
            .get::<FighterAir>(harness.fighter)
            .unwrap();
        let motion_after = harness
            .app
            .world()
            .get::<MotionState>(harness.fighter)
            .unwrap();
        let displacement = harness.position() - before_kick;
        assert_eq!(harness.pose_id(), L::JUMP_B);
        assert_eq!(wall_after.kick_pose, L::JUMP_B);
        assert!(!wall_after.cling_active);
        assert!(
            air_after.air.planar_z < -1.0 && motion_after.velocity.z < -1.0,
            "正面贴墙二段必须沿保存的墙外法线离墙：air={} motion={} normal={:?}",
            air_after.air.planar_z,
            motion_after.velocity.z,
            wall_normal,
        );
        assert!(
            displacement.z < -0.01,
            "二段首帧胶囊必须实际离墙，不能被持续 W 翻回朝墙：{displacement:?}",
        );
        assert!(air_after.air.vertical_speed > 1.0);

        let first_kick_y = harness.position().y;
        let mut recling = Vec::new();
        let mut max_y = first_kick_y;
        let mut saw_clean_landing = false;
        let mut max_y_at_240 = first_kick_y;
        for tick in 0..480 {
            if tick == 240 {
                max_y_at_240 = max_y;
            }
            if tick % 2 == 0 {
                harness.press_binding(keymap.jump);
            } else {
                harness.release_binding(keymap.jump);
            }
            harness.step();
            let wall = harness
                .app
                .world()
                .get::<FighterWall>(harness.fighter)
                .unwrap();
            let air = harness
                .app
                .world()
                .get::<FighterAir>(harness.fighter)
                .unwrap();
            max_y = max_y.max(harness.position().y);
            if !air.air.airborne {
                let motion = harness
                    .app
                    .world()
                    .get::<MotionState>(harness.fighter)
                    .unwrap();

                assert_eq!(
                    motion.velocity.y, 0.0,
                    "落地后不得保留垂直速度：{:?}",
                    motion.velocity,
                );
                saw_clean_landing = true;
            }
            if wall.cling_active {
                assert!(
                    saw_clean_landing,
                    "same-wall reentry before landing: tick={tick}"
                );
                recling.push((
                    tick,
                    wall.cling_pose,
                    harness.position(),
                    air.air.airborne,
                    air.air.planar_z,
                    air.air.vertical_speed,
                ));
            }
        }
        assert!(saw_clean_landing, "夹具必须覆盖墙跳后的真实落地");
        assert!(
            !recling.is_empty() && saw_clean_landing,
            "落地后保持前进也必须能再次上墙：recling={recling:?}",
        );

        eprintln!(
            "wall spam: first_kick_y={first_kick_y:.3} max_y@240={max_y_at_240:.3} max_y@480={max_y:.3}"
        );
        assert!(
            (max_y - max_y_at_240).abs() < 1e-3,
            "连续点跳不得把同墙冲量堆成无限爬升：first_kick_y={first_kick_y} \
             max_y@240={max_y_at_240} max_y@480={max_y}",
        );
    }

    #[test]
    fn production_free_air_wall_kick_uses_the_visible_pose_for_each_wall_side() {
        let keymap = crate::app_state::Keymap::default();
        for (wall_x, expected_pose, away_sign) in [
            (-1.5, L::WALL_KICK_R, 1.0_f32),
            (1.5, L::WALL_KICK_L, -1.0_f32),
        ] {
            let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
                eprintln!("origin_res missing, skipping");
                return;
            };
            harness.replace_stage_mesh(harness_ground_with_side_wall(wall_x));

            harness.press_binding(keymap.jump);
            harness.step();
            harness.release_binding(keymap.jump);
            harness.step();
            harness.steps(2);
            assert!(harness.airborne());
            assert!(
                !harness
                    .app
                    .world()
                    .get::<FighterWall>(harness.fighter)
                    .unwrap()
                    .cling_active,
                "自由墙跳夹具不得先进入贴墙 mode2",
            );

            harness.press_binding(keymap.jump);
            harness.step();
            harness.release_binding(keymap.jump);
            harness.step();

            let preadvance_pose = harness.app.world().resource::<HarnessPoseBeforeAdvance>().0;
            let wall = harness
                .app
                .world()
                .get::<FighterWall>(harness.fighter)
                .unwrap();
            let air = harness
                .app
                .world()
                .get::<FighterAir>(harness.fighter)
                .unwrap();
            assert_eq!(
                preadvance_pose,
                Some(expected_pose),
                "wall_x={wall_x} 的墙跳首个可见帧不得闪反侧姿态",
            );
            assert_eq!(harness.pose_id(), expected_pose);
            assert_eq!(wall.kick_pose, expected_pose);
            assert!(
                air.air.planar_x * away_sign > 1.0,
                "墙跳速度必须离开墙面：wall_x={wall_x} velocity={}",
                air.air.planar_x,
            );
        }
    }

    #[test]
    fn production_wall_cling_publishes_current_velocity_for_wall_attack() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        harness.replace_stage_mesh(harness_ground_with_side_wall(-2.0));
        let keymap = crate::app_state::Keymap::default();

        harness.press_binding(keymap.move_up);
        harness.press_binding(keymap.move_right);
        harness.steps(2);
        harness.press_binding(keymap.jump);
        harness.step();
        harness.release_binding(keymap.jump);
        harness.step();
        for _ in 0..10 {
            harness.step();
            if harness
                .app
                .world()
                .get::<FighterWall>(harness.fighter)
                .is_some_and(|wall| wall.cling_active)
            {
                break;
            }
        }
        assert_eq!(
            harness
                .app
                .world()
                .get::<FighterWall>(harness.fighter)
                .unwrap()
                .cling_pose,
            L::WALL_CLING_R,
        );

        let entry_air = harness
            .app
            .world()
            .get::<FighterAir>(harness.fighter)
            .unwrap()
            .air
            .clone();
        let entry_motion = harness
            .app
            .world()
            .get::<MotionState>(harness.fighter)
            .unwrap()
            .velocity;
        assert!(entry_air.planar_x.abs() < 1e-5, "入墙必须剔除法向速度");
        assert!(entry_air.planar_z > 1.0, "入墙必须保留沿墙切向速度");
        assert!(
            (entry_motion.x - entry_air.planar_x).abs() < 1e-4
                && (entry_motion.y - entry_air.vertical_speed).abs() < 1e-4
                && (entry_motion.z - entry_air.planar_z).abs() < 1e-4,
            "入墙当帧必须发布投影后的 +0x390：air={entry_air:?} motion={entry_motion:?}",
        );

        harness.steps(2);
        let wall_air = harness
            .app
            .world()
            .get::<FighterAir>(harness.fighter)
            .unwrap()
            .air
            .clone();
        let wall_motion = harness
            .app
            .world()
            .get::<MotionState>(harness.fighter)
            .unwrap()
            .velocity;
        assert!(
            (wall_motion.x - wall_air.planar_x).abs() < 1e-4
                && (wall_motion.y - wall_air.vertical_speed).abs() < 1e-4
                && (wall_motion.z - wall_air.planar_z).abs() < 1e-4,
            "每个 mode2 tick 都必须发布碰撞投影后的当前速度：air={wall_air:?} motion={wall_motion:?}",
        );

        harness.release_binding(keymap.move_up);
        harness.release_binding(keymap.move_right);
        harness.press_mouse(MouseButton::Left);
        harness.step();
        harness.release_mouse(MouseButton::Left);
        let attack = harness
            .app
            .world()
            .get::<FighterAttackMotion>(harness.fighter)
            .unwrap();
        let wall = harness
            .app
            .world()
            .get::<FighterWall>(harness.fighter)
            .unwrap();
        assert_eq!(harness.state(), FighterState::Attack1);
        assert!(!wall.cling_active, "墙上出招必须切入 mode3");
        assert!(
            attack.carry_x.abs() < 1e-4,
            "不得捡回入墙前的法向速度：carry={} wall_air_x={} wall_motion_x={}",
            attack.carry_x,
            wall_air.planar_x,
            wall_motion.x,
        );
        assert!(
            attack.carry_z > 0.0 && attack.carry_z < wall_motion.z,
            "墙上出招必须继承并衰减当前沿墙速度：carry={} wall_motion={}",
            attack.carry_z,
            wall_motion.z,
        );
    }

    #[test]
    fn production_wall_kick_rearms_and_reclings_across_a_corridor() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        harness.replace_stage_mesh(harness_ground_with_side_corridor(2.0));
        let keymap = crate::app_state::Keymap::default();

        harness.press_binding(keymap.move_up);
        harness.press_binding(keymap.move_right);
        harness.steps(2);
        harness.press_binding(keymap.jump);
        harness.step();
        harness.release_binding(keymap.jump);
        harness.step();
        for _ in 0..10 {
            harness.step();
            if harness
                .app
                .world()
                .get::<FighterWall>(harness.fighter)
                .is_some_and(|wall| wall.cling_active)
            {
                break;
            }
        }
        let first_y = harness.position().y;
        assert_eq!(
            harness
                .app
                .world()
                .get::<FighterWall>(harness.fighter)
                .unwrap()
                .cling_pose,
            L::WALL_CLING_R,
            "W+D 必须抓住角色右侧墙 Pose 162",
        );

        harness.press_binding(keymap.jump);
        harness.step();
        harness.release_binding(keymap.jump);
        harness.step();
        assert_eq!(harness.pose_id(), L::WALL_KICK_R);
        assert!(
            harness
                .app
                .world()
                .get::<FighterAir>(harness.fighter)
                .unwrap()
                .air
                .cling_armed,
            "蹬墙跳成功必须重新武装 +0xe4",
        );
        harness.release_binding(keymap.move_right);
        harness.press_binding(keymap.move_left);
        let mut reclung = false;
        for _ in 0..10 {
            harness.step();
            let wall = harness
                .app
                .world()
                .get::<FighterWall>(harness.fighter)
                .unwrap();
            if wall.cling_active && wall.cling_pose == L::WALL_CLING_L {
                reclung = true;
                break;
            }
        }
        assert!(reclung, "162→159 蹬出后应在 0.1s 后抓住对面左墙 163");
        assert!(
            harness.position().y > first_y,
            "连续墙跳必须实际往上爬：first_y={first_y} now={}",
            harness.position().y,
        );
    }

    #[test]
    fn production_air_attack_vertical_follows_the_d_base_curve() {
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
        let common = std::fs::read(root.join("Common/character.amb"))
            .ok()
            .and_then(|bytes| meteor_asset::amb::parse_amb(&bytes).ok())
            .expect("common amb");

        let def = character.poses.get(&245).expect("pose 245");
        let z0 = character.motion_root_at(&common, 245, def.start as f32)[2];
        let z1 = character.motion_root_at(&common, 245, def.end as f32)[2];
        eprintln!("pose 245 d_base.z: {z0:.4} -> {z1:.4}");
        assert!(
            z1 - z0 > 1.0,
            "Pose 245 的 d_base.z 必须是抬升曲线（{z0:.4} -> {z1:.4}）；\
             若这里读到的是帧首 root_pivot 会得到反向的 19.5 -> 17.05"
        );
    }

    #[test]
    fn d_base_dummy_track_is_not_the_frame_root_pivot() {
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
        let common = std::fs::read(root.join("Common/character.amb"))
            .ok()
            .and_then(|bytes| meteor_asset::amb::parse_amb(&bytes).ok())
            .expect("common amb");

        let base_index = *character
            .dummy_order
            .get("d_base")
            .expect("d_base 必须在 BNC dummies 里");
        eprintln!("d_base dummy index = {base_index}");

        for pose_id in [253u16, 255, 245] {
            let def = character.poses.get(&pose_id).expect("pose");
            let bank = character.amb_for_pose(&common, pose_id);
            eprintln!("--- pose {pose_id} [{}..{}] ---", def.start, def.end);
            for f in def.start..=def.end.min(def.start + 14) {
                let s = bank.sample(f as usize);
                let d = s.dummy_pos.get(base_index).copied().unwrap_or_default();
                eprintln!(
                    "  f{f} root_pivot=({:8.3},{:8.3},{:8.3})  d_base=({:8.3},{:8.3},{:8.3})",
                    s.root_pivot[0], s.root_pivot[1], s.root_pivot[2], d[0], d[1], d[2]
                );
            }
        }
    }

    #[test]
    fn production_touch_commands_keep_real_combo_poses_across_frame_rates() {
        use bevy::input::touch::TouchPhase::{Moved, Started};
        for fps in [30, 60, 120] {
            for (deltas, expected_pose, expected_dodges) in [
                (vec![Vec2::new(0.35, -1.0)], 250, 0),
                (vec![Vec2::new(0.35, 1.0), Vec2::new(0.35, -1.0)], 257, 0),
                (
                    vec![Vec2::new(0.35, -1.0), Vec2::ZERO, Vec2::new(0.35, -1.0)],
                    253,
                    1,
                ),
            ] {
                for same_frame in [false, true] {
                    let mut h = spawn_gameplay_harness(DAGGER).expect("original P1 assets");
                    let (center, radius, attack) =
                        crate::plugins::mobile_controls_plugin::install_touch_input_for_test(
                            &mut h.app,
                        );
                    h.touch(1, Started, center);
                    for delta in &deltas {
                        h.touch(1, Moved, center + *delta * radius);
                        if !same_frame {
                            h.step_dt(1.0 / fps as f32);
                        }
                    }
                    h.touch(2, Started, attack);
                    h.step_dt(1.0 / fps as f32);
                    assert_eq!(
                        h.pose_id(),
                        expected_pose,
                        "fps={fps} same_frame={same_frame} deltas={deltas:?} edges={:?}",
                        h.edges()
                    );
                    if same_frame {
                        assert_eq!(
                            h.edges()
                                .iter()
                                .filter(|edge| matches!(edge, Edge::Dodge { .. }))
                                .count(),
                            expected_dodges
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn production_touch_diagonal_movement_cannot_pollute_held_command_or_dodge() {
        use bevy::input::touch::TouchPhase::{Canceled, Moved, Started};
        for fps in [30, 60, 120] {
            let mut h = spawn_gameplay_harness(DAGGER).expect("original P1 assets");
            let (center, radius, _) =
                crate::plugins::mobile_controls_plugin::install_touch_input_for_test(&mut h.app);
            h.touch(1, Started, center);

            for frame in 0..fps {
                let x = if frame % 2 == 0 { 0.27 } else { 0.35 };
                h.touch(1, Moved, center + Vec2::new(x, -1.0) * radius);
                h.step_dt(1.0 / fps as f32);
                assert_eq!(
                    h.app
                        .world()
                        .resource::<LocalCommandInput>()
                        .0
                        .command_string(),
                    "u"
                );
                assert!(!h
                    .edges()
                    .iter()
                    .any(|edge| matches!(edge, Edge::Dodge { .. })));
            }
            let keymap = h.app.world().resource::<crate::app_state::Keymap>();
            let keys = h.app.world().resource::<ButtonInput<KeyCode>>();
            let mouse = h.app.world().resource::<ButtonInput<MouseButton>>();
            assert!(keymap.move_up.pressed(keys, mouse));
            assert!(keymap.move_right.pressed(keys, mouse));
            assert!(h.position().x.abs() > 0.1 && h.position().z.abs() > 0.1);

            h.touch(1, Moved, center + Vec2::new(1.5, -1.0) * radius);
            h.step_dt(1.0 / fps as f32);
            assert_eq!(
                h.app
                    .world()
                    .resource::<LocalCommandInput>()
                    .0
                    .command_string(),
                "ur"
            );
            h.step_dt(0.31);
            assert_eq!(
                h.app
                    .world()
                    .resource::<LocalCommandInput>()
                    .0
                    .command_string(),
                "r"
            );
            h.touch(1, Canceled, center);
            h.step_dt(1.0 / fps as f32);
            h.step_dt(0.31);
            let command = &h.app.world().resource::<LocalCommandInput>().0;
            assert!(command.peek().is_empty());
            assert_eq!(command.held_direction(), None);
        }
    }

    #[test]
    fn production_touch_attack_consumes_only_the_directions_before_it() {
        use bevy::input::touch::TouchPhase::{Moved, Started};
        let mut h = spawn_gameplay_harness(DAGGER).expect("original P1 assets");
        let (center, radius, attack) =
            crate::plugins::mobile_controls_plugin::install_touch_input_for_test(&mut h.app);
        h.touch(1, Started, center);
        h.touch(1, Moved, center + Vec2::new(0.35, -1.0) * radius);
        h.touch(2, Started, attack);
        h.touch(1, Moved, center + Vec2::new(0.35, 1.0) * radius);
        h.step();
        assert_eq!(h.pose_id(), 250);
        assert_eq!(
            h.app
                .world()
                .resource::<LocalCommandInput>()
                .0
                .command_string(),
            "d"
        );
    }

    #[test]
    fn production_touch_center_jitter_and_debug_panel_do_not_leave_command_holds() {
        use bevy::input::touch::TouchPhase::{Moved, Started};
        let mut h = spawn_gameplay_harness(DAGGER).expect("original P1 assets");
        let (center, radius, attack) =
            crate::plugins::mobile_controls_plugin::install_touch_input_for_test(&mut h.app);
        h.touch(1, Started, center);
        for y in [0.3, 0.27, 0.29, 0.23, 0.3, 0.19] {
            h.touch(1, Moved, center - Vec2::Y * y * radius);
            h.step();
            assert_eq!(
                h.app
                    .world()
                    .resource::<LocalCommandInput>()
                    .0
                    .command_string(),
                "u"
            );
            assert!(!h
                .edges()
                .iter()
                .any(|edge| matches!(edge, Edge::Dodge { .. })));
        }
        h.app
            .insert_resource(crate::plugins::debug_plugin::DebugConsoleState {
                open: false,
                panel_open: true,
            });
        h.touch(1, Moved, center - Vec2::Y * radius);
        h.step();
        let command = &h.app.world().resource::<LocalCommandInput>().0;
        assert!(command.peek().is_empty());
        assert_eq!(command.held_direction(), None);
        h.app
            .world_mut()
            .resource_mut::<crate::plugins::debug_plugin::DebugConsoleState>()
            .panel_open = false;
        h.touch(2, Started, attack);
        h.step();
        assert_eq!(
            h.pose_id(),
            248,
            "closing the panel must not restore a stale Up command"
        );
    }

    #[test]
    fn production_dagger_displacing_attacks_actually_move_the_capsule() {
        let mut results: Vec<(&str, f32, f32, f32)> = Vec::new();
        for (name, dir_key, double_tap, min_distance) in [
            ("前前A", KeyCode::KeyW, true, 3.0_f32),
            ("右A", KeyCode::KeyD, false, 1.0_f32),
        ] {
            let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
                eprintln!("origin_res missing, skipping");
                return;
            };
            let start = harness.position();

            if double_tap {
                harness.press_key(dir_key);
                harness.step();
                harness.release_key(dir_key);
                harness.step();
                harness.press_key(dir_key);
                harness.step();
                harness.release_key(dir_key);
            } else {
                harness.press_key(dir_key);
                harness.step();
            }
            harness.press_mouse(MouseButton::Left);
            harness.step();
            harness.release_mouse(MouseButton::Left);
            eprintln!("--- {name} ---");
            harness.attack_trace("t0");

            let mut peak = 0.0_f32;
            for i in 0..60 {
                harness.step();
                peak = peak.max((harness.position() - start).length());
                if i < 24 {
                    harness.attack_trace(&format!("t{}", i + 1));
                }
                if harness.state() == FighterState::Name && i > 5 {
                    break;
                }
            }
            harness.release_key(dir_key);
            let net = (harness.position() - start).length();
            eprintln!("{name}: peak={peak:.3} net={net:.3} (要求 > {min_distance})");
            results.push((name, peak, net, min_distance));
        }
        for (name, peak, net, min_distance) in results {
            assert!(
                peak > min_distance,
                "{name} 是带位移招式，必须推进胶囊 > {min_distance}，实际峰值 {peak:.3}（净位移 {net:.3}）"
            );
        }
    }

    #[test]
    fn production_dagger_up_up_attack_launches_into_the_air() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        assert!(!harness.airborne(), "起手必须在地面");

        harness.press_key(KeyCode::KeyW);
        harness.step();
        harness.release_key(KeyCode::KeyW);
        harness.step();
        harness.press_key(KeyCode::KeyW);
        harness.step();
        harness.trace("up-up");
        harness.release_key(KeyCode::KeyW);

        harness.press_mouse(MouseButton::Left);
        harness.step();
        harness.release_mouse(MouseButton::Left);
        harness.trace("up-up-A");
        let launch_pose = harness.pose_id();

        let mut peak = harness.position().y;
        for i in 0..60 {
            harness.step();
            peak = peak.max(harness.position().y);
            if i % 10 == 0 {
                harness.trace(&format!("uua-{i}"));
            }
        }
        harness.trace("uua-settled");
        assert_ne!(
            launch_pose, 165,
            "攻击必须离开闪避 Pose 进入命令招式（实际仍是闪避 165）"
        );
        assert!(
            peak > 0.5,
            "匕首上上A 必须起飞（峰值高度只有 {peak}，起手 Pose {launch_pose}）"
        );
        assert_eq!(
            harness.state(),
            FighterState::Name,
            "起飞技结束后必须交还控制权（实际停在 {:?} pose {}）",
            harness.state(),
            harness.pose_id()
        );
    }

    #[test]
    fn production_ground_launch_decays_carry_before_lift_without_air_glide() {
        for fps in [30, 60, 120] {
            let dt = 1.0 / fps as f32;
            let mut trajectories = Vec::new();
            for initial_carry in [0.0, 12.0] {
                let mut h = spawn_gameplay_harness(DAGGER).expect("original P1 assets");
                h.press_key(KeyCode::KeyW);
                h.step_dt(dt);
                h.release_key(KeyCode::KeyW);
                h.step_dt(dt);
                h.press_key(KeyCode::KeyW);
                h.step_dt(dt);
                h.release_key(KeyCode::KeyW);
                h.press_mouse(MouseButton::Left);
                h.step_dt(dt);
                h.release_mouse(MouseButton::Left);
                assert_eq!(h.pose_id(), 253);
                h.app
                    .world_mut()
                    .get_mut::<FighterAttackMotion>(h.fighter)
                    .unwrap()
                    .carry_z = initial_carry;
                let start = h.position();
                let mut lifted = None;
                let mut cleared_support = false;
                for tick in 0..fps * 2 {
                    h.step_dt(dt);
                    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                    cleared_support |= air.carry_no_support && air.air.launch_pose_id == 253;
                    if h.position().y > 0.5 && lifted.is_none() {
                        let carry = h
                            .app
                            .world()
                            .get::<FighterAttackMotion>(h.fighter)
                            .unwrap()
                            .carry_z;
                        lifted = Some((tick, h.position().z, carry));
                    }
                }
                assert!(!h.airborne(), "self-launch must land");
                assert!(
                    cleared_support,
                    "a clear airborne solve must classify even during launch Pose 253"
                );
                let (tick, lift_z, carry) = lifted.expect("253 must lift");
                trajectories.push((h.position().z - start.z, h.position().z - lift_z));
                eprintln!("ground launch fps={fps} initial_carry={initial_carry} lift_tick={tick} lift_carry={carry} total={} after_lift={}", trajectories.last().unwrap().0, trajectories.last().unwrap().1);
            }
            assert!(
                (trajectories[1].0 - trajectories[0].0).abs() < 1.5,
                "12 world/s bank must decay during charge"
            );
            assert!(
                (trajectories[1].1 - trajectories[0].1).abs() < 0.15,
                "charge carry must not become sustained air glide"
            );
        }
    }

    #[test]
    #[ignore = "resource trajectory diagnostic for carry support boundaries"]
    fn diagnose_contact_air_support_trajectories() {
        for fps in [30, 60, 120] {
            for jumping in [false, true] {
                let mut h = spawn_gameplay_harness(DAGGER).expect("original P1 assets");
                let dt = 1.0 / fps as f32;
                h.press_key(KeyCode::KeyW);
                h.step_dt(dt);
                if jumping {
                    h.press_key(KeyCode::Space);
                    h.step_dt(dt);
                    h.release_key(KeyCode::Space);
                } else {
                    h.release_key(KeyCode::KeyW);
                    h.step_dt(dt);
                    h.press_key(KeyCode::KeyW);
                    h.step_dt(dt);
                    h.release_key(KeyCode::KeyW);
                }
                h.press_mouse(MouseButton::Left);
                h.step_dt(dt);
                h.release_mouse(MouseButton::Left);
                for tick in 0..fps * 2 {
                    let world = h.app.world();
                    let air = world.get::<FighterAir>(h.fighter).unwrap();
                    let attack = world.get::<FighterAttackMotion>(h.fighter).unwrap();
                    eprintln!("fps={fps} jump={jumping} tick={tick} pose={} pos={:?} launch={} support={} steep={} carry={} vy={}", h.pose_id(), h.position(), air.air.launch_pose_id, !air.carry_no_support, air.carry_decay_steep, attack.carry_z, air.air.vertical_speed);
                    h.step_dt(dt);
                }
            }
        }
    }

    #[test]
    fn production_dodge_crouch_roll_and_fake_all_survive_a_previous_attack() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let keymap = crate::app_state::Keymap::default();

        harness.press_key(KeyCode::Space);
        harness.step();
        harness.release_key(KeyCode::Space);
        harness.steps(4);
        harness.press_mouse(MouseButton::Left);
        harness.step();
        harness.release_mouse(MouseButton::Left);
        for _ in 0..90 {
            harness.step();
            if !harness.airborne() && harness.state() == FighterState::Name {
                break;
            }
        }
        harness.trace("after-air-attack");
        assert_eq!(
            harness.state(),
            FighterState::Name,
            "空中攻击必须交还控制权，否则后面所有输入都会被 can_interrupt 拒绝"
        );

        let before_dodge = harness.position();
        harness.press_binding(keymap.dodge);
        harness.step();
        harness.release_binding(keymap.dodge);
        harness.steps(8);
        harness.trace("after-dodge");
        let dodged = (harness.position() - before_dodge).length();
        assert!(dodged > 0.05, "攻击之后瞬移仍必须位移，实际只移动 {dodged}");

        for _ in 0..20 {
            harness.step();
            if harness.state() == FighterState::Name {
                break;
            }
        }
        harness.press_binding(keymap.crouch);
        harness.step();
        let before_roll = harness.position();
        harness.press_binding(keymap.dodge);
        harness.step();
        harness.release_binding(keymap.dodge);
        let roll_pose = harness.pose_id();
        harness.steps(10);
        harness.trace("after-crouch-roll");
        let rolled = (harness.position() - before_roll).length();
        harness.release_binding(keymap.crouch);
        assert!(
            (176..=179).contains(&roll_pose),
            "蹲伏中的闪避必须走通用翻滚 176..179，实际 Pose {roll_pose}"
        );
        assert!(rolled > 0.05, "下蹲滚动仍必须位移，实际只移动 {rolled}");

        for _ in 0..30 {
            harness.step();
            if harness.state() == FighterState::Name {
                break;
            }
        }
        harness.press_binding(keymap.fake);
        harness.step();
        harness.release_binding(keymap.fake);
        harness.trace("after-fake");
        assert_eq!(
            harness.state(),
            FighterState::Fake,
            "假死键在打过一套之后仍必须可用"
        );
    }

    #[test]
    fn production_dagger_air_attack_returns_to_the_ground() {
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };

        harness.press_key(KeyCode::KeyW);
        harness.steps(3);
        harness.press_key(KeyCode::Space);
        harness.step();
        harness.release_key(KeyCode::Space);
        harness.steps(4);
        harness.trace("jumped");
        assert!(harness.airborne(), "跳跃必须离地");

        harness.press_mouse(MouseButton::Left);
        harness.step();
        harness.release_mouse(MouseButton::Left);
        harness.trace("air-attack");

        let mut landed_at = None;
        for i in 0..150 {
            harness.step();
            if i % 10 == 0 {
                harness.trace(&format!("air-{i}"));
            }
            if landed_at.is_none() && !harness.airborne() {
                landed_at = Some(i);
                harness.trace(&format!("landed-at-{i}"));
            }
        }
        harness.trace("air-settled");
        assert!(
            landed_at.is_some(),
            "匕首空中攻击后必须落地，实际 5s 仍在空中 y={}",
            harness.position().y
        );
        assert_eq!(
            harness.state(),
            FighterState::Name,
            "落地后必须交还控制权（实际停在 {:?} pose {}）",
            harness.state(),
            harness.pose_id()
        );
    }

    #[test]
    fn production_dagger_air_up_down_attack_spawns_pose_effect() {
        use bevy::ecs::system::RunSystemOnce;
        let Some(mut harness) = spawn_gameplay_harness(DAGGER) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let app = &mut harness.app;
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<crate::plugins::effect_plugin::PonEfMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<crate::plugins::stage_material::GmdlMaterial>>()
            .init_resource::<Assets<AudioSource>>()
            .init_resource::<crate::plugins::effect_plugin::EfCache>()
            .init_resource::<crate::plugins::effect_plugin::EfTextureCache>()
            .init_resource::<crate::plugins::audio_plugin::SfxCache>();
        app.world_mut()
            .run_system_once(crate::plugins::effect_plugin::setup_effect_assets)
            .expect("effect assets setup");
        app.add_systems(
            Update,
            (
                crate::plugins::effect_plugin::tick_pon_ef_players,
                crate::plugins::effect_plugin::tick_pon_ef_particles,
            )
                .chain()
                .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
        );
        app.add_systems(
            Update,
            crate::plugins::effect_plugin::pose_effect_spawn
                .after(crate::plugins::FighterFrameSet::SkeletonSample),
        );
        app.world_mut().spawn((
            crate::plugins::character_plugin::FighterModel {
                fighter: harness.fighter,
                character: "p1".to_string(),
            },
            Transform::IDENTITY,
        ));

        harness.press_key(KeyCode::Space);
        harness.step();
        harness.release_key(KeyCode::Space);
        harness.steps(4);
        harness.trace("jumped");
        assert!(harness.airborne(), "跳跃必须离地");

        harness.press_key(KeyCode::KeyW);
        harness.step();
        harness.release_key(KeyCode::KeyW);
        harness.press_key(KeyCode::KeyS);
        harness.step();
        harness.release_key(KeyCode::KeyS);
        harness.press_mouse(MouseButton::Left);
        harness.step();
        harness.release_mouse(MouseButton::Left);
        harness.trace("air-up-down-A");
        assert_eq!(
            harness.pose_id(),
            256,
            "空中前后A 必须解析到 Pose 256（匕首空殺下踢）"
        );

        let mut spawned = false;
        for i in 0..45 {
            let count = harness
                .app
                .world_mut()
                .query::<&crate::plugins::effect_plugin::PonEfInstance>()
                .iter(harness.app.world())
                .count();
            if count > 0 {
                spawned = true;
                harness.trace(&format!("effect-spawned-at-{i}"));
                break;
            }
            harness.step();
        }
        assert!(
            spawned,
            "Pose 256（EffectID Pos256）播放期间必须生成 pos256.ef 特效播放器"
        );
    }
}
