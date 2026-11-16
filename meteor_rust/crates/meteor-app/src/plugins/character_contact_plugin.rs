#[cfg(test)]
use crate::app_state::AppState;
use crate::components::fighter::{
    Fighter, FighterHitReaction, Health, MotionState, PoseRuntime, SmState, WeaponSwitchActive,
};
use crate::plugins::movement_plugin::{FighterAir, COLLIDER_HEIGHT, COLLIDER_RADIUS};
use crate::plugins::network_plugin::{NetworkFrameApply, RemoteContactSegment, RemoteFighter};
use crate::plugins::stage_collider::{StageCollider, StageMeshCollider};
use crate::plugins::stage_plugin::NpcPerformance;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;
use meteor_core::FighterState;
use meteor_physics::character_contact::{CharacterContactSystem, CharacterView};
use meteor_physics::collision::{CollisionSystem, MeshCollider, Point3};
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct ContactView {
    entity: Entity,
    position: (f32, f32, f32),
    pose_id: u16,
    guard: bool,
    airborne: bool,
    visible: bool,
    alive: bool,
    ground_y: f32,
    pushable: bool,
    explicit_ignore: bool,
    authored_motion: bool,
}

fn uses_authored_mover(state: FighterState) -> bool {
    matches!(
        state,
        FighterState::Attack1 | FighterState::Attack2 | FighterState::Attack3 | FighterState::BeHit
    )
}

impl CharacterView for ContactView {
    fn position(&self) -> (f32, f32, f32) {
        self.position
    }

    fn pose_id(&self) -> u16 {
        self.pose_id
    }

    fn in_guard(&self) -> bool {
        self.guard
    }

    fn airborne(&self) -> bool {
        self.airborne
    }

    fn visible(&self) -> bool {
        self.visible
    }

    fn alive(&self) -> bool {
        self.alive
    }

    fn explicit_ignore(&self) -> bool {
        self.explicit_ignore
    }

    fn ground_y(&self) -> f32 {
        self.ground_y
    }

    fn pushable(&self) -> bool {
        self.pushable
    }
}

#[derive(Resource, Default)]
pub(crate) struct CharacterContactFrame {
    #[cfg(test)]
    audit_bypass_actors: std::collections::HashSet<Entity>,
    #[cfg(test)]
    audit_moves: HashMap<Entity, Vec<serde_json::Value>>,
    system: CharacterContactSystem,
    views: Vec<ContactView>,
    indices: HashMap<Entity, usize>,

    last_mover_scene_contacts: HashMap<Entity, bool>,
    authored_repulsion: HashMap<Entity, Vec3>,

    candidate_distances: HashMap<Entity, Vec<(Entity, f32)>>,
    entry_states: HashMap<Entity, FighterState>,
    root_repulsion: HashMap<Entity, bool>,
}

impl CharacterContactFrame {
    pub(crate) fn last_mover_scene_contact(&self, entity: Entity) -> bool {
        self.last_mover_scene_contacts
            .get(&entity)
            .copied()
            .unwrap_or(false)
    }

    pub(super) fn cached_candidate_contains(&self, owner: Entity, target: Entity) -> Option<bool> {
        self.candidate_distances
            .get(&owner)
            .map(|candidates| candidates.iter().any(|(entity, _)| *entity == target))
    }

    #[cfg(test)]
    pub(crate) fn audit_record_moves(&mut self, entity: Entity) {
        self.audit_moves.insert(entity, Vec::new());
    }

    #[cfg(test)]
    pub(crate) fn audit_moves(&self, entity: Entity) -> &[serde_json::Value] {
        self.audit_moves.get(&entity).map_or(&[], Vec::as_slice)
    }

    #[cfg(test)]
    pub(crate) fn audit_bypass_primary_contact(&mut self, entity: Entity) {
        self.audit_bypass_actors.insert(entity);
    }

    pub(super) fn cache_candidates(
        &mut self,
        entity: Entity,
        origin: Vec3,
        positions: impl Iterator<Item = (Entity, Vec3)>,
    ) {
        self.candidate_distances.entry(entity).or_insert_with(|| {
            positions
                .filter_map(|(other, position)| {
                    let distance = origin.distance(position);

                    (other != entity && distance <= 100.0 * meteor_core::MODEL_SCALE)
                        .then_some((other, distance))
                })
                .collect()
        });
    }

    fn commit_position(&mut self, entity: Entity, position: Vec3) {
        if let Some(index) = self.indices.get(&entity).copied() {
            self.views[index].position = (position.x, position.y, position.z);
        }
    }
}

fn locally_pushable(
    remote: Option<&RemoteFighter>,
    _npc_performance: Option<&NpcPerformance>,
) -> bool {
    remote.is_none()
}

pub struct CharacterContactPlugin;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CharacterContactFinishSet;

impl Plugin for CharacterContactPlugin {
    fn build(&self, app: &mut App) {
        let actor_schedule = super::fighter_frame::schedule(app);
        let serial = super::fighter_frame::installed(app);
        let begin_schedule = if serial {
            super::fighter_frame::FighterStepStart.intern()
        } else {
            Update.intern()
        };
        app.add_systems(
            actor_schedule,
            remember_repulsion_entry
                .in_set(crate::plugins::FighterFrameSet::Movement)
                .before(crate::plugins::ai_plugin::drive_npc_ai)
                .run_if(crate::plugins::simulation_frame)
                .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
        );
        app.add_systems(
            actor_schedule,
            prepare_root_repulsion
                .after(crate::plugins::FighterFrameSet::HitJudge)
                .before(crate::plugins::FighterFrameSet::RootMotion)
                .run_if(crate::plugins::simulation_frame)
                .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
        );
        app.add_systems(
            actor_schedule,
            (apply_authored_repulsion
                .after(crate::plugins::FighterFrameSet::RootMotion)
                .before(finish_character_contact_frame)
                .before(crate::plugins::FighterFrameSet::AttackFacing),)
                .run_if(crate::plugins::simulation_frame)
                .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
        );
        app.add_systems(
            actor_schedule,
            apply_locomotion_repulsion
                .after(crate::plugins::movement_plugin::move_local_fighter)
                .after(crate::plugins::ai_plugin::move_npc_fighters)
                .in_set(crate::plugins::FighterFrameSet::Locomotion)
                .run_if(crate::plugins::simulation_frame)
                .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
        );
        let save_force = snapshot_authored_repulsion.after(finish_character_contact_frame);
        app.add_systems(
            actor_schedule,
            (if serial {
                save_force.before(crate::plugins::FighterFrameSet::AttackFacing)
            } else {
                save_force
            })
            .run_if(crate::plugins::simulation_frame)
            .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
        );
        if serial {
            app.add_systems(
                actor_schedule,
                sync_contact_views_at_actor_start
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .before(crate::plugins::ai_plugin::drive_npc_ai)
                    .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
            );
            app.add_systems(
                actor_schedule,
                finish_character_contact_frame
                    .after(crate::plugins::FighterFrameSet::RootMotion)
                    .before(crate::plugins::FighterFrameSet::AttackFacing)
                    .run_if(crate::plugins::simulation_frame)
                    .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
            );
            app.add_systems(
                super::fighter_frame::FighterStepEnd,
                end_serial_contact_frame
                    .in_set(CharacterContactFinishSet)
                    .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
            );
        }
        app.init_resource::<CharacterContactFrame>()
            .add_systems(
                begin_schedule,
                begin_character_contact_frame
                    .after(NetworkFrameApply)
                    .before(crate::plugins::FighterFrameSet::Movement)
                    .run_if(crate::plugins::simulation_frame)
                    .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
            )
            .add_systems(
                Update,
                finish_character_contact_frame
                    .run_if(move || !serial)
                    .in_set(crate::plugins::FighterFrameSet::PoseResolve)
                    .in_set(CharacterContactFinishSet)
                    .after(crate::plugins::combat_plugin::CombatPoseResolveSet)
                    .run_if(crate::plugins::simulation_frame)
                    .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
            )
            .add_systems(
                actor_schedule,
                sync_contact_views_before_npc
                    .after(crate::plugins::movement_plugin::move_local_fighter)
                    .before(crate::plugins::ai_plugin::move_npc_fighters)
                    .in_set(crate::plugins::FighterFrameSet::Locomotion)
                    .run_if(crate::plugins::simulation_frame)
                    .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
            )
            .add_systems(
                actor_schedule,
                sync_contact_views_before_root
                    .before(crate::plugins::movement_plugin::attack_root_motion)
                    .in_set(crate::plugins::FighterFrameSet::RootMotion)
                    .run_if(crate::plugins::simulation_frame)
                    .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
            );
    }
}

type ContactSnapshotQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static PoseRuntime,
        &'static SmState,
        &'static MotionState,
        &'static Health,
        Option<&'static FighterAir>,
        Option<&'static FighterHitReaction>,
        Option<&'static NpcPerformance>,
        Option<&'static RemoteFighter>,
        Option<&'static Visibility>,
    ),
    With<Fighter>,
>;

fn snapshot_authored_repulsion(
    active: Option<Res<super::fighter_frame::ActiveFighter>>,
    fighters: ContactSnapshotQuery<'_, '_>,
    mut frame: ResMut<CharacterContactFrame>,
) {
    for (entity, transform, _, _, _, _, _, _, performance, remote, _) in &fighters {
        if !super::fighter_frame::accepts(active.as_deref(), entity) {
            continue;
        }
        frame.authored_repulsion.remove(&entity);
        if !locally_pushable(remote, performance) {
            continue;
        }
        frame.cache_candidates(
            entity,
            transform.translation,
            fighters
                .iter()
                .map(|(other, position, ..)| (other, position.translation)),
        );
        let mut force = Vec3::ZERO;
        for &(other, candidate_distance) in &frame.candidate_distances[&entity] {
            let Ok((_, other_transform, ..)) = fighters.get(other) else {
                continue;
            };
            let delta = transform.translation - other_transform.translation;
            let (x, z) = meteor_physics::character_contact::authored_repulsion_force(
                (delta.x, delta.y, delta.z),
                candidate_distance,
            );
            force += Vec3::new(x, 0.0, z);
        }
        #[cfg(test)]
        if frame.audit_moves.contains_key(&entity) {
            let candidates: Vec<_> = frame.candidate_distances[&entity].iter()
                .filter_map(|(other, distance)| fighters.get(*other).ok().map(|(_, position, ..)|
                    serde_json::json!({"position":position.translation.to_array(),"distance":distance})))
                .collect();
            frame
                .audit_moves
                .get_mut(&entity)
                .unwrap()
                .push(serde_json::json!({
                "kind":"repulsion-save", "position":transform.translation.to_array(),
                "candidates":candidates,"force":force.to_array()}));
        }
        frame.authored_repulsion.insert(entity, force);
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct RepulsionParams<'w, 's> {
    active: Option<Res<'w, super::fighter_frame::ActiveFighter>>,
    time: Res<'w, Time>,
    clock: Option<Res<'w, crate::plugins::FrameClock>>,
    collider: Option<Res<'w, StageCollider>>,
    frame: ResMut<'w, CharacterContactFrame>,
    #[cfg(not(target_arch = "wasm32"))]
    trace: Option<ResMut<'w, super::combat_trace::CombatTrace>>,
    fighters: Query<
        'w,
        's,
        (
            Entity,
            &'static mut Transform,
            &'static MotionState,
            &'static SmState,
            Option<&'static FighterHitReaction>,
            &'static Health,
            Option<&'static PoseRuntime>,
            Option<&'static WeaponSwitchActive>,
            Option<&'static mut FighterAir>,
        ),
        (With<Fighter>, Without<RemoteFighter>),
    >,
}

fn uses_mode1_repulsion(state: FighterState, mode: meteor_core::MovementMode) -> bool {
    matches!(
        state,
        FighterState::Name
            | FighterState::Think
            | FighterState::View
            | FighterState::Look
            | FighterState::GetItem
            | FighterState::Spawn
    ) && matches!(
        mode,
        meteor_core::MovementMode::Idle
            | meteor_core::MovementMode::Walk
            | meteor_core::MovementMode::Run
            | meteor_core::MovementMode::Crouch
    )
}

fn remember_repulsion_entry(
    active: Option<Res<super::fighter_frame::ActiveFighter>>,
    actors: Query<(Entity, &SmState), (With<Fighter>, Without<RemoteFighter>)>,
    mut frame: ResMut<CharacterContactFrame>,
) {
    for (entity, state) in &actors {
        if super::fighter_frame::accepts(active.as_deref(), entity) {
            frame.entry_states.insert(entity, state.current);
        }
    }
}

fn root_repulsion_allowed(
    entry: Option<FighterState>,
    state: FighterState,
    mode: meteor_core::MovementMode,
    reaction: Option<meteor_combat::hit_reaction::HitReactKind>,
    pose: Option<&PoseRuntime>,
    switching: bool,
) -> bool {
    use meteor_combat::hit_reaction::HitReactKind;

    if matches!(entry, Some(FighterState::Down | FighterState::Fake))
        && (matches!(reaction, Some(HitReactKind::GetUp | HitReactKind::Roll))
            || matches!(state, FighterState::Dodge | FighterState::Burst))
    {
        return false;
    }
    if mode == meteor_core::MovementMode::WallRun || state == FighterState::Cure {
        return false;
    }
    if switching || state == FighterState::Dead {
        return pose.is_some_and(|pose| !pose.finished());
    }
    match state {
        FighterState::Dodge | FighterState::Burst => {
            pose.is_some_and(|pose| !crate::plugins::movement_plugin::dodge_pose_finished(pose))
        }
        FighterState::Attack1
        | FighterState::Attack2
        | FighterState::Attack3
        | FighterState::BeHit
        | FighterState::Guard
        | FighterState::Down
        | FighterState::Fake => true,
        FighterState::Jump => true,
        FighterState::Name
        | FighterState::Think
        | FighterState::View
        | FighterState::Look
        | FighterState::GetItem
        | FighterState::Spawn => mode == meteor_core::MovementMode::Airborne,
        _ => false,
    }
}

fn prepare_root_repulsion(
    active: Option<Res<super::fighter_frame::ActiveFighter>>,
    actors: Query<
        (
            Entity,
            &SmState,
            &MotionState,
            &PoseRuntime,
            Option<&FighterHitReaction>,
            Option<&WeaponSwitchActive>,
        ),
        (With<Fighter>, Without<RemoteFighter>),
    >,
    mut frame: ResMut<CharacterContactFrame>,
) {
    for (entity, state, motion, pose, reaction, switching) in &actors {
        if !super::fighter_frame::accepts(active.as_deref(), entity) {
            continue;
        }
        let allowed = root_repulsion_allowed(
            frame.entry_states.get(&entity).copied(),
            state.current,
            motion.movement_mode,
            reaction.and_then(|r| r.current.as_ref()).map(|r| r.kind),
            Some(pose),
            switching.is_some(),
        );
        frame.root_repulsion.insert(entity, allowed);
    }
}

fn apply_locomotion_repulsion(params: RepulsionParams) {
    apply_cached_repulsion(params, false);
}

pub(super) fn apply_authored_repulsion(params: RepulsionParams) {
    apply_cached_repulsion(params, true);
}

fn apply_cached_repulsion(params: RepulsionParams, authored_phase: bool) {
    let RepulsionParams {
        active,
        time,
        clock,
        collider,
        mut frame,
        #[cfg(not(target_arch = "wasm32"))]
        mut trace,
        mut fighters,
    } = params;
    let dt = crate::plugins::simulation_dt(clock.as_deref(), time.delta_secs());
    if !dt.is_finite() || dt <= 0.0 {
        return;
    }
    let mut stage = CollisionSystem::<&StageMeshCollider>::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    if let Some(mesh) = collider.as_deref().and_then(|c| c.mesh.as_ref()) {
        stage.set_mesh_collider(mesh);
    }
    for (entity, mut transform, motion, sm, reaction, health, pose, switching, air) in &mut fighters
    {
        if !super::fighter_frame::accepts(active.as_deref(), entity)
            || (health.current <= 0.0 && sm.current != FighterState::Dead)
        {
            continue;
        }
        if authored_phase {
            let allowed = frame
                .root_repulsion
                .get(&entity)
                .copied()
                .unwrap_or_else(|| {
                    root_repulsion_allowed(
                        frame.entry_states.get(&entity).copied(),
                        sm.current,
                        motion.movement_mode,
                        reaction.and_then(|r| r.current.as_ref()).map(|r| r.kind),
                        pose,
                        switching.is_some(),
                    )
                });
            if !allowed {
                continue;
            }
        } else if switching.is_some() || !uses_mode1_repulsion(sm.current, motion.movement_mode) {
            continue;
        }
        let Some(force) = frame.authored_repulsion.remove(&entity) else {
            continue;
        };

        if force.length() <= 5.0 * meteor_core::MODEL_SCALE {
            continue;
        }

        if let Some(mut air) = air {
            air.ground_external_velocity = Vec3::ZERO;
        }
        let delta = force * dt * dt;
        let (dx, _, dz, _) = meteor_physics::carry_velocity::clamp_composed_delta_3d(
            delta.x,
            0.0,
            delta.z,
            dt,
            meteor_physics::carry_velocity::CARRY_MAX_SPEED,
        );
        let before = transform.translation;
        let (x, z, y, _, blocked_axes) = stage.resolve_xz_with_contacts(
            Point3::new(before.x, before.y, before.z),
            dx,
            dz,
            false,
        );
        frame
            .last_mover_scene_contacts
            .insert(entity, blocked_axes.into_iter().any(|blocked| blocked));
        let accepted = Vec3::new(x, y, z);
        transform.translation += accepted;

        frame.commit_position(entity, transform.translation);
        #[cfg(test)]
        if let Some(moves) = frame.audit_moves.get_mut(&entity) {
            moves.push(serde_json::json!({
            "kind":"repulsion", "before":before.to_array(), "force":force.to_array(),
            "requested":[dx,0.0,dz], "accepted":accepted.to_array()}));
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(trace) = trace.as_deref_mut() {
            trace.event(serde_json::json!({"kind":"authored-repulsion", "entity":entity.to_bits(),
                "dt":dt, "force":force.to_array(), "requested": [dx,0.0,dz],
                "accepted":accepted.to_array(), "before":before.to_array(), "after":transform.translation.to_array()}));
        }
    }
}

pub(crate) fn begin_character_contact_frame(
    collider: Option<Res<StageCollider>>,
    fighters: ContactSnapshotQuery<'_, '_>,
    mut remote_segments: Query<(Entity, &mut RemoteContactSegment), With<RemoteFighter>>,
    mut frame: ResMut<CharacterContactFrame>,
) {
    let mut stage = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    if let Some(mesh) = collider
        .as_deref()
        .and_then(|collider| collider.mesh.as_ref())
    {
        stage.set_mesh_collider(mesh);
    }

    let previous_entities: Vec<_> = frame.views.iter().map(|view| view.entity).collect();
    #[cfg(test)]
    for moves in frame.audit_moves.values_mut() {
        moves.clear();
    }
    frame.candidate_distances.clear();
    frame.entry_states.clear();
    frame.root_repulsion.clear();
    frame.views.clear();
    frame.indices.clear();
    for (
        entity,
        transform,
        pose,
        sm,
        motion,
        health,
        air,
        hit_reaction,
        npc_performance,
        remote,
        visibility,
    ) in &fighters
    {
        let position = transform.translation;
        let ground_y = stage
            .sample_ground_hit(position.x, position.z, position.y)
            .map_or(f32::NEG_INFINITY, |hit| hit.y);
        let index = frame.views.len();
        frame.indices.insert(entity, index);
        frame.views.push(ContactView {
            entity,
            position: (position.x, position.y, position.z),
            pose_id: pose.pose_id,
            guard: sm.current == FighterState::Guard,
            airborne: air.is_some_and(|air| air.air.airborne)
                || hit_reaction.is_some_and(|reaction| reaction.airborne),
            visible: visibility.is_none_or(|visibility| *visibility != Visibility::Hidden),
            alive: health.current > 0.0,
            ground_y,
            pushable: locally_pushable(remote, npc_performance),
            explicit_ignore: motion.movement_mode == meteor_core::MovementMode::Dodge
                || matches!(
                    sm.current,
                    FighterState::Dodge | FighterState::Burst | FighterState::Down
                ),
            authored_motion: uses_authored_mover(sm.current),
        });
    }
    let live_entities: std::collections::HashSet<_> = frame.indices.keys().copied().collect();
    frame
        .authored_repulsion
        .retain(|entity, _| live_entities.contains(entity));
    frame
        .last_mover_scene_contacts
        .retain(|entity, _| live_entities.contains(entity));
    let active = frame
        .views
        .iter()
        .enumerate()
        .filter_map(|(index, view)| (view.visible && view.alive).then_some(index))
        .collect();
    let CharacterContactFrame {
        system, indices, ..
    } = &mut *frame;
    system.remap_deferred_targets(|old_index| {
        previous_entities
            .get(old_index)
            .and_then(|entity| indices.get(entity))
            .copied()
    });
    system.begin_frame(active);

    for (_, mut segment) in &mut remote_segments {
        segment.valid = false;
    }
}

pub(crate) fn resolve_fighter_xz<M: MeshCollider>(
    frame: Option<&mut CharacterContactFrame>,
    entity: Entity,
    stage: &CollisionSystem<M>,
    pos: Point3,
    dx: f32,
    dz: f32,
    no_probe: bool,
) -> (f32, f32, f32) {
    let (x, z, y, _) = resolve_fighter_xz_with_surface(frame, entity, stage, pos, dx, dz, no_probe);
    (x, z, y)
}

pub(crate) fn resolve_fighter_xz_with_surface<M: MeshCollider>(
    frame: Option<&mut CharacterContactFrame>,
    entity: Entity,
    stage: &CollisionSystem<M>,
    pos: Point3,
    dx: f32,
    dz: f32,
    no_probe: bool,
) -> (f32, f32, f32, Option<meteor_physics::collision::FootHit>) {
    let (x, z, y, surface, _, _) =
        resolve_fighter_xz_with_contacts(frame, entity, stage, pos, dx, dz, no_probe);
    (x, z, y, surface)
}

pub(crate) fn resolve_fighter_xz_with_contacts<M: MeshCollider>(
    frame: Option<&mut CharacterContactFrame>,
    entity: Entity,
    stage: &CollisionSystem<M>,
    pos: Point3,
    dx: f32,
    dz: f32,
    no_probe: bool,
) -> (
    f32,
    f32,
    f32,
    Option<meteor_physics::collision::FootHit>,
    [bool; 2],
    bool,
) {
    let stage_resolved = stage.resolve_xz_with_contacts(pos, dx, dz, no_probe);
    let Some(frame) = frame else {
        return (
            stage_resolved.0,
            stage_resolved.1,
            stage_resolved.2,
            stage_resolved.3,
            stage_resolved.4,
            false,
        );
    };
    frame
        .last_mover_scene_contacts
        .insert(entity, stage_resolved.4.into_iter().any(|blocked| blocked));
    let Some(index) = frame.indices.get(&entity).copied() else {
        return (
            stage_resolved.0,
            stage_resolved.1,
            stage_resolved.2,
            stage_resolved.3,
            stage_resolved.4,
            false,
        );
    };

    let resolved = stage_resolved;
    #[cfg(test)]
    let authored_motion = frame.views[index].authored_motion;
    #[cfg(test)]
    if let Some(moves) = frame.audit_moves.get_mut(&entity) {
        moves.push(serde_json::json!({
        "kind":"primary", "before":[pos.x,pos.y,pos.z], "requested":[dx,0.0,dz],
        "stage":[stage_resolved.0,stage_resolved.2,stage_resolved.1],
        "accepted":[resolved.0,resolved.2,resolved.1], "authored":authored_motion}));
    }
    let endpoint = Point3::new(pos.x + resolved.0, pos.y + resolved.2, pos.z + resolved.1);
    let view = &mut frame.views[index];
    view.position = (endpoint.x, endpoint.y, endpoint.z);
    view.ground_y = stage
        .sample_ground_hit(endpoint.x, endpoint.z, endpoint.y)
        .map_or(f32::NEG_INFINITY, |hit| hit.y);
    let blocked_axes = [
        stage_resolved.4[0] || resolved.4[0],
        stage_resolved.4[1] || resolved.4[1],
    ];
    frame
        .last_mover_scene_contacts
        .insert(entity, blocked_axes.into_iter().any(|blocked| blocked));
    (
        resolved.0,
        resolved.1,
        resolved.2,
        resolved.3,
        blocked_axes,
        false,
    )
}

pub(crate) fn resolve_fighter_ordinary_motion<M: MeshCollider>(
    frame: Option<&mut CharacterContactFrame>,
    entity: Entity,
    stage: &CollisionSystem<M>,
    pos: Point3,
    velocity: Point3,
    dt: f32,
) -> meteor_physics::collision::WallMove {
    let result = stage.resolve_ordinary_motion(pos, velocity, dt);
    if let Some(frame) = frame {
        frame
            .last_mover_scene_contacts
            .insert(entity, result.flags & 5 != 0);
        #[cfg(test)]
        if let Some(moves) = frame.audit_moves.get_mut(&entity) {
            moves.push(serde_json::json!({
            "kind":"primary", "before":[pos.x,pos.y,pos.z],
            "requested":[velocity.x*dt,velocity.y*dt,velocity.z*dt],
            "stage":[result.position.x-pos.x,result.position.y-pos.y,result.position.z-pos.z],
            "accepted":[result.position.x-pos.x,result.position.y-pos.y,result.position.z-pos.z],
            "authored":frame.indices.get(&entity).is_some_and(|&i| frame.views[i].authored_motion)}));
        }
        frame.commit_position(
            entity,
            Vec3::new(result.position.x, result.position.y, result.position.z),
        );
    }
    result
}

pub(crate) fn resolve_fighter_wall_motion<M: MeshCollider>(
    frame: Option<&mut CharacterContactFrame>,
    entity: Entity,
    stage: &CollisionSystem<M>,
    pos: Point3,
    velocity: Point3,
    probe: Point3,
    dt: f32,
) -> meteor_physics::collision::WallMove {
    let result = stage.resolve_wall_motion(pos, velocity, probe, dt);
    let Some(frame) = frame else {
        return result;
    };
    frame
        .last_mover_scene_contacts
        .insert(entity, result.flags & 5 != 0);
    frame.commit_position(
        entity,
        Vec3::new(result.position.x, result.position.y, result.position.z),
    );
    result
}

pub(crate) fn commit_fighter_position(
    frame: Option<&mut CharacterContactFrame>,
    entity: Entity,
    position: Vec3,
) {
    if let Some(frame) = frame {
        frame.commit_position(entity, position);
    }
}

pub(crate) fn sync_fighter_contact_state(
    frame: Option<&mut CharacterContactFrame>,
    entity: Entity,
    position: Vec3,
    pose_id: u16,
    state: FighterState,
    movement_mode: meteor_core::MovementMode,
    airborne: bool,
) {
    let Some(frame) = frame else {
        return;
    };
    let Some(index) = frame.indices.get(&entity).copied() else {
        return;
    };
    let view = &mut frame.views[index];
    view.position = (position.x, position.y, position.z);
    view.pose_id = pose_id;
    view.guard = state == FighterState::Guard;
    view.airborne = airborne;
    view.explicit_ignore = movement_mode == meteor_core::MovementMode::Dodge
        || matches!(
            state,
            FighterState::Dodge | FighterState::Burst | FighterState::Down
        );
    view.authored_motion = uses_authored_mover(state);
}

type ContactStateQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static PoseRuntime,
        &'static SmState,
        &'static MotionState,
        &'static Health,
        Option<&'static FighterAir>,
        Option<&'static FighterHitReaction>,
        Option<&'static Visibility>,
    ),
    With<Fighter>,
>;

fn sync_contact_views(query: &ContactStateQuery<'_, '_>, frame: &mut CharacterContactFrame) {
    for (entity, transform, pose, sm, motion, health, air, hit_reaction, visibility) in query {
        sync_fighter_contact_state(
            Some(frame),
            entity,
            transform.translation,
            pose.pose_id,
            sm.current,
            motion.movement_mode,
            air.is_some_and(|air| air.air.airborne)
                || hit_reaction.is_some_and(|reaction| reaction.airborne),
        );
        if let Some(index) = frame.indices.get(&entity).copied() {
            frame.views[index].alive = health.current > 0.0;
            frame.views[index].visible =
                visibility.is_none_or(|visibility| *visibility != Visibility::Hidden);
        }
    }
}

fn sync_contact_views_before_npc(
    query: ContactStateQuery<'_, '_>,
    mut frame: ResMut<CharacterContactFrame>,
) {
    sync_contact_views(&query, &mut frame);
}

fn sync_contact_views_at_actor_start(
    query: ContactStateQuery<'_, '_>,
    mut frame: ResMut<CharacterContactFrame>,
) {
    sync_contact_views(&query, &mut frame);
}

fn sync_contact_views_before_root(
    query: ContactStateQuery<'_, '_>,
    mut frame: ResMut<CharacterContactFrame>,
) {
    sync_contact_views(&query, &mut frame);
}

fn end_serial_contact_frame(mut frame: ResMut<CharacterContactFrame>) {
    frame.system.end_frame();
}

pub(super) fn finish_character_contact_frame(
    fighters: ContactStateQuery<'_, '_>,
    mut frame: ResMut<CharacterContactFrame>,
) {
    sync_contact_views(&fighters, &mut frame);
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("character_contact_plugin/repulsion_tests.rs");
    include!("character_contact_plugin/scene_contact_tests.rs");

    #[test]
    fn ordinary_and_crouched_actors_consume_previous_force_once() {
        for serial in [false, true] {
            for fps in [30.0, 60.0, 120.0] {
                for mode in [
                    meteor_core::MovementMode::Idle,
                    meteor_core::MovementMode::Crouch,
                ] {
                    let mut app = App::new();
                    crate::plugins::configure_fighter_frame_sets(&mut app);
                    if serial {
                        crate::plugins::fighter_frame::install(&mut app);
                    }
                    app.insert_resource(State::new(AppState::Playing))
                        .init_resource::<Time>()
                        .insert_resource(crate::plugins::FrameClock {
                            sim_dt: 1.0 / fps,
                            ..default()
                        })
                        .add_plugins(CharacterContactPlugin);
                    let mut spawn = |slot, x| {
                        app.world_mut()
                            .spawn((
                                fighter(slot),
                                Transform::from_xyz(x, 0.0, 0.0),
                                PoseRuntime {
                                    pose_id: if mode == meteor_core::MovementMode::Crouch {
                                        10
                                    } else {
                                        0
                                    },
                                    ..PoseRuntime::idle()
                                },
                                SmState::default(),
                                Health::default(),
                                MotionState {
                                    movement_mode: mode,
                                    velocity: Vec3::ZERO,
                                    facing: 0.0,
                                },
                            ))
                            .id()
                    };
                    let local = spawn(0, 0.0);
                    let remote = spawn(1, 1.44);
                    app.world_mut().entity_mut(remote).insert(RemoteFighter {
                        player_id: "ordinary-repulsion-target".into(),
                    });
                    app.world_mut().run_schedule(Update);
                    assert_eq!(
                        app.world().get::<Transform>(local).unwrap().translation,
                        Vec3::ZERO
                    );
                    app.world_mut().run_schedule(Update);

                    let expected = -108.0 / (fps * fps);
                    let position = app.world().get::<Transform>(local).unwrap().translation;
                    assert!(
                        (position.x - expected).abs() < 1e-5,
                        "serial={serial} fps={fps} mode={mode:?}: {position:?}"
                    );
                    assert!(
                        app.world()
                            .get::<MotionState>(local)
                            .unwrap()
                            .velocity
                            .x
                            .abs()
                            < 1e-4,
                        "the second mover must not pollute the drive velocity"
                    );
                    assert_eq!(
                        app.world().get::<Transform>(remote).unwrap().translation.x,
                        1.44
                    );
                    app.world_mut()
                        .get_mut::<Transform>(remote)
                        .unwrap()
                        .translation
                        .x = 20.0;
                    app.world_mut().run_schedule(Update);
                    let last = app.world().get::<Transform>(local).unwrap().translation;
                    assert!(last.x < position.x);
                    app.world_mut().run_schedule(Update);
                    assert_eq!(
                        app.world().get::<Transform>(local).unwrap().translation,
                        last
                    );
                }
            }
        }
    }

    #[test]
    fn authored_repulsion_moves_airborne_100_once_and_never_integrates_remote_owner() {
        use meteor_combat::hit_reaction::{FacingBucket, HitReact, HitReactKind};
        for serial in [false, true] {
            for fps in [30.0, 60.0, 120.0] {
                let mut app = App::new();
                crate::plugins::configure_fighter_frame_sets(&mut app);
                if serial {
                    crate::plugins::fighter_frame::install(&mut app);
                }
                app.insert_resource(State::new(AppState::Playing))
                    .init_resource::<Time>()
                    .insert_resource(crate::plugins::FrameClock {
                        sim_dt: 1.0 / fps,
                        ..default()
                    })
                    .add_plugins(CharacterContactPlugin);
                let mut reaction = FighterHitReaction::default();
                reaction.begin(
                    HitReact {
                        kind: HitReactKind::BeHit,
                        pose_id: Some(100),
                        head_pose_id: None,
                        lock_remaining: 0.1,
                        minimum_remaining: 0.1,
                        authored_minimum: 0.1,
                        motion_scale: 1.0,
                        down_remaining: 0.0,
                        bucket: FacingBucket::Front,
                        environmental: false,
                        selector: 0,
                        rewind_count: 0,
                        play_to_end_pose_id: None,
                    },
                    true,
                );
                let mut spawn = |slot, x| {
                    app.world_mut()
                        .spawn((
                            fighter(slot),
                            Transform::from_xyz(x, 1.5, 0.0),
                            PoseRuntime {
                                pose_id: 100,
                                ..PoseRuntime::idle()
                            },
                            SmState {
                                current: FighterState::BeHit,
                                ..default()
                            },
                            MotionState {
                                facing: 0.3,
                                velocity: Vec3::ZERO,
                                movement_mode: meteor_core::MovementMode::Idle,
                            },
                            reaction.clone(),
                            Health::default(),
                            FighterAir::default(),
                        ))
                        .id()
                };
                let local = spawn(0, 0.0);
                let remote = spawn(1, 1.44);
                app.world_mut().entity_mut(remote).insert(RemoteFighter {
                    player_id: "stock-1".into(),
                });
                let dt = 1.0 / fps;
                app.world_mut().run_schedule(Update);
                assert_eq!(
                    app.world().get::<Transform>(local).unwrap().translation.x,
                    0.0
                );
                app.world_mut().run_schedule(Update);
                let p = app.world().get::<Transform>(local).unwrap().translation;

                let expected = -108.0 * dt * dt;
                assert!(
                    (p.x - expected).abs() < 1e-5,
                    "serial={serial} fps={fps}: {p:?}"
                );
                let velocity = app.world().get::<MotionState>(local).unwrap().velocity;
                assert_eq!(
                    velocity,
                    Vec3::ZERO,
                    "second mover must preserve primary velocity"
                );
                assert_eq!(
                    app.world().get::<Transform>(remote).unwrap().translation.x,
                    1.44
                );
                assert_eq!(
                    app.world().get::<MotionState>(remote).unwrap().velocity,
                    Vec3::ZERO
                );

                app.world_mut()
                    .get_mut::<Transform>(remote)
                    .unwrap()
                    .translation
                    .x = 20.0;
                app.world_mut().run_schedule(Update);
                let final_position = app.world().get::<Transform>(local).unwrap().translation;
                assert!(final_position.x < p.x);
                app.world_mut().run_schedule(Update);
                assert_eq!(
                    app.world().get::<Transform>(local).unwrap().translation,
                    final_position
                );
            }
        }
    }

    #[test]
    fn authored_repulsion_uses_pre_root_direction_and_preserves_first_mover_velocity() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.init_resource::<CharacterContactFrame>()
            .init_resource::<Time>()
            .insert_resource(crate::plugins::FrameClock {
                sim_dt: 0.01,
                ..default()
            });
        let mut spawn = |x| {
            app.world_mut()
                .spawn((
                    fighter(0),
                    Transform::from_xyz(x, 0.0, 0.0),
                    PoseRuntime::idle(),
                    SmState {
                        current: FighterState::Attack1,
                        ..default()
                    },
                    MotionState {
                        facing: 0.0,
                        velocity: Vec3::X * 3.0,
                        movement_mode: meteor_core::MovementMode::Idle,
                    },
                    Health::default(),
                ))
                .id()
        };
        let mover = spawn(0.0);
        let _target = spawn(1.44);
        app.world_mut()
            .run_system_once(snapshot_authored_repulsion)
            .unwrap();

        app.world_mut()
            .get_mut::<Transform>(mover)
            .unwrap()
            .translation
            .x = 2.0;
        app.world_mut()
            .run_system_once(apply_authored_repulsion)
            .unwrap();
        assert!((app.world().get::<Transform>(mover).unwrap().translation.x - 1.9892).abs() < 1e-5);
        assert!((app.world().get::<MotionState>(mover).unwrap().velocity.x - 3.0).abs() < 1e-4);
    }

    #[test]
    fn serial_mode1_consumes_previous_force_before_later_actor_hit_judge() {
        use super::super::fighter_frame::{ActiveFighter, FighterUpdate};
        #[derive(Component)]
        struct ForwardStep;
        #[derive(Resource, Default)]
        struct Seen(Vec<(Entity, Vec<(Entity, f32)>)>);
        fn move_actor(
            active: Res<ActiveFighter>,
            mut contacts: ResMut<CharacterContactFrame>,
            mut actors: Query<&mut Transform, With<ForwardStep>>,
        ) {
            let entity = active.0.unwrap();
            let Ok(mut transform) = actors.get_mut(entity) else {
                return;
            };
            let stage =
                CollisionSystem::<&StageMeshCollider>::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
            let p = transform.translation;
            let (x, z, y) = resolve_fighter_xz(
                Some(&mut contacts),
                entity,
                &stage,
                Point3::new(p.x, p.y, p.z),
                0.1,
                0.0,
                false,
            );
            transform.translation += Vec3::new(x, y, z);
        }
        fn see(
            active: Res<ActiveFighter>,
            actors: Query<(Entity, &Transform), With<Fighter>>,
            mut seen: ResMut<Seen>,
        ) {
            seen.0.push((
                active.0.unwrap(),
                actors.iter().map(|(e, t)| (e, t.translation.x)).collect(),
            ));
        }
        for source_first in [true, false] {
            let mut app = App::new();
            crate::plugins::configure_fighter_frame_sets(&mut app);
            crate::plugins::fighter_frame::install(&mut app);
            app.insert_resource(State::new(AppState::Playing))
                .init_resource::<Time>()
                .init_resource::<Seen>()
                .add_plugins(CharacterContactPlugin);
            let mut spawn = |x| {
                app.world_mut()
                    .spawn((
                        fighter(0),
                        Transform::from_xyz(x, 0.0, 0.0),
                        PoseRuntime::idle(),
                        SmState::default(),
                        Health::default(),
                        MotionState {
                            facing: 0.0,
                            velocity: Vec3::ZERO,
                            movement_mode: meteor_core::MovementMode::Idle,
                        },
                    ))
                    .id()
            };
            let radius = meteor_physics::character_contact::CHARACTER_CONTACT_RADIUS * 2.0;
            let first = spawn(if source_first { 0.0 } else { radius });
            let second = spawn(if source_first { radius } else { 0.0 });
            let observer = spawn(10.0);
            let (source, target) = if source_first {
                (first, second)
            } else {
                (second, first)
            };
            app.world_mut().entity_mut(source).insert(ForwardStep);
            app.add_systems(
                FighterUpdate,
                move_actor.in_set(crate::plugins::FighterFrameSet::RootMotion),
            );
            app.add_systems(
                FighterUpdate,
                see.in_set(crate::plugins::FighterFrameSet::HitJudge),
            );
            for tick in 0..2 {
                app.world_mut().resource_mut::<Seen>().0.clear();
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
                app.world_mut().run_schedule(Update);

                let actual = app.world().get::<Transform>(target).unwrap().translation.x;
                assert!(
                    actual >= radius,
                    "push consumed out of order: {source_first}, {tick}, {actual}"
                );
                let seen = &app.world().resource::<Seen>().0;
                let last_view = &seen.iter().find(|(e, _)| *e == observer).unwrap().1;
                let seen_target = last_view.iter().find(|(e, _)| *e == target).unwrap().1;
                assert!(
                    (seen_target - actual).abs() < 1e-5,
                    "later hit judge saw stale contact position"
                );
            }
        }
    }

    fn fighter(slot: u8) -> Fighter {
        Fighter {
            character_id: 0,
            player_slot: slot,
            weapon: 6,
            weapon_slots: [6, 5],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        }
    }

    fn view(entity: Entity, x: f32) -> ContactView {
        ContactView {
            entity,
            position: (x, 0.0, 0.0),
            pose_id: 0,
            guard: false,
            airborne: false,
            visible: true,
            alive: true,
            ground_y: 0.0,
            pushable: true,
            explicit_ignore: false,
            authored_motion: false,
        }
    }

    #[test]
    fn legacy_diagnostic_push_tracks_entity_when_query_order_changes() {
        let mut app = App::new();
        app.init_resource::<CharacterContactFrame>()
            .add_systems(Update, begin_character_contact_frame);
        let actors: Vec<_> = [0.0, 4.0, 8.0]
            .into_iter()
            .enumerate()
            .map(|(slot, x)| {
                app.world_mut()
                    .spawn((
                        fighter(slot as u8),
                        Transform::from_xyz(x, 0.0, 0.0),
                        PoseRuntime::idle(),
                        SmState::default(),
                        MotionState {
                            movement_mode: meteor_core::MovementMode::Idle,
                            velocity: Vec3::ZERO,
                            facing: 0.0,
                        },
                        Health::default(),
                    ))
                    .id()
            })
            .collect();
        app.update();
        {
            let mut frame = app.world_mut().resource_mut::<CharacterContactFrame>();
            let a = frame.indices[&actors[0]];
            let b = frame.indices[&actors[1]];
            let CharacterContactFrame { system, views, .. } = &mut *frame;
            system.resolve_xz(a, (0.0, 0.0), (3.0, 0.0), views);
            system.finish_frame(1.0 / 30.0);
            system.resolve_xz(b, (4.0, 0.0), (3.0, 0.0), views);
            system.end_frame();
        }
        app.world_mut().despawn(actors[0]);
        app.update();
        let mut frame = app.world_mut().resource_mut::<CharacterContactFrame>();
        let target = frame.indices[&actors[2]];
        frame.system.finish_frame(1.0 / 30.0);
        assert_eq!(frame.system.contact_velocities.len(), 1);
        assert!(
            frame.system.contact_velocities.contains_key(&target),
            "deferred contact must follow its entity, not the previous query index"
        );
    }

    #[test]
    fn production_resolver_accepts_scene_motion_without_legacy_actor_carry() {
        let mut world = World::new();
        let mover = world.spawn_empty().id();
        let target = world.spawn_empty().id();
        let mut frame = CharacterContactFrame {
            views: vec![view(mover, 0.0), view(target, 4.0)],
            indices: HashMap::from([(mover, 0), (target, 1)]),
            ..default()
        };
        frame.system.begin_frame(vec![0, 1]);
        let stage: CollisionSystem<StageMeshCollider> =
            CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);

        let (dx, _, _) = resolve_fighter_xz(
            Some(&mut frame),
            mover,
            &stage,
            Point3::new(0.0, 0.0, 0.0),
            3.0,
            0.0,
            false,
        );
        assert_eq!(dx, 3.0);
        frame.commit_position(mover, Vec3::new(dx, 0.0, 0.0));
        frame.system.finish_frame(1.0 / 30.0);
        assert!(frame.system.contact_velocities.is_empty());
    }

    #[test]
    fn every_mover_keeps_scene_walls_without_actor_circle_veto() {
        let mut world = World::new();
        let mover = world.spawn_empty().id();
        let target = world.spawn_empty().id();
        for wall in [false, true] {
            let mut frame = CharacterContactFrame {
                views: vec![view(mover, 0.0), view(target, 2.0)],
                indices: HashMap::from([(mover, 0), (target, 1)]),
                ..default()
            };
            let mut stage: CollisionSystem<StageMeshCollider> =
                CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
            if wall {
                stage.add_box([1.2, -2.0, -10.0], [20.0, 10.0, 10.0]);
            }

            for (state, pose) in [
                (FighterState::Attack1, 464),
                (FighterState::BeHit, 103),
                (FighterState::Name, 0),
            ] {
                frame.system.begin_frame(vec![0, 1]);
                sync_fighter_contact_state(
                    Some(&mut frame),
                    mover,
                    Vec3::ZERO,
                    pose,
                    state,
                    meteor_core::MovementMode::Idle,
                    false,
                );
                let (dx, _, _) = resolve_fighter_xz(
                    Some(&mut frame),
                    mover,
                    &stage,
                    Point3::ZERO,
                    1.5,
                    0.0,
                    false,
                );
                if wall {
                    assert!(dx < 0.3, "wall={wall} state={state:?} dx={dx}");
                } else {
                    assert!((dx - 1.5).abs() < 1e-6);
                    frame.system.finish_frame(1.0 / 60.0);
                    assert!(
                        frame.system.contact_velocities.is_empty(),
                        "authored pass must not add the secondary fixed contact push"
                    );
                }
            }
        }
    }

    #[test]
    fn production_resolver_uses_world_xz_instead_of_xy() {
        let mut world = World::new();
        let mover = world.spawn_empty().id();
        let target = world.spawn_empty().id();
        let mut mover_view = view(mover, 0.0);
        mover_view.position = (0.0, 10.0, 0.0);
        mover_view.ground_y = 10.0;
        let mut target_view = view(target, 0.0);
        target_view.position = (0.0, 10.0, 4.0);
        target_view.ground_y = 10.0;
        let mut frame = CharacterContactFrame {
            views: vec![mover_view, target_view],
            indices: HashMap::from([(mover, 0), (target, 1)]),
            ..default()
        };
        frame.system.begin_frame(vec![0, 1]);
        let stage: CollisionSystem<StageMeshCollider> =
            CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);

        let (dx, dz, _) = resolve_fighter_xz(
            Some(&mut frame),
            mover,
            &stage,
            Point3::new(0.0, 10.0, 0.0),
            0.0,
            3.0,
            false,
        );
        assert!(dx.abs() < 1e-5);
        assert_eq!(
            dz, 3.0,
            "actors do not turn the XZ scene mover into a hard circle"
        );
        frame.system.finish_frame(1.0 / 30.0);
        assert!(frame.system.contact_velocities.is_empty());
    }

    #[test]
    fn same_frame_dodge_state_phases_before_pose_runtime_changes() {
        let mut world = World::new();
        let mover = world.spawn_empty().id();
        let target = world.spawn_empty().id();
        let mut frame = CharacterContactFrame {
            views: vec![view(mover, 0.0), view(target, 4.0)],
            indices: HashMap::from([(mover, 0), (target, 1)]),
            ..default()
        };
        frame.system.begin_frame(vec![0, 1]);
        sync_fighter_contact_state(
            Some(&mut frame),
            mover,
            Vec3::ZERO,
            0,
            FighterState::Dodge,
            meteor_core::MovementMode::Dodge,
            false,
        );
        let stage: CollisionSystem<StageMeshCollider> =
            CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        let (dx, _, _) = resolve_fighter_xz(
            Some(&mut frame),
            mover,
            &stage,
            Point3::new(0.0, 0.0, 0.0),
            3.0,
            0.0,
            false,
        );
        assert!((dx - 3.0).abs() < 1e-5);
    }

    #[test]
    fn hit_reaction_airborne_target_does_not_add_a_hard_circle_or_fixed_push() {
        let mut world = World::new();
        let mover = world.spawn_empty().id();
        let target = world.spawn_empty().id();
        let mut frame = CharacterContactFrame {
            views: vec![view(mover, 0.0), view(target, 4.0)],
            indices: HashMap::from([(mover, 0), (target, 1)]),
            ..default()
        };
        frame.system.begin_frame(vec![0, 1]);
        sync_fighter_contact_state(
            Some(&mut frame),
            target,
            Vec3::new(4.0, 0.0, 0.0),
            0,
            FighterState::BeHit,
            meteor_core::MovementMode::Idle,
            true,
        );
        let stage: CollisionSystem<StageMeshCollider> =
            CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        let (dx, _, _) = resolve_fighter_xz(
            Some(&mut frame),
            mover,
            &stage,
            Point3::ZERO,
            3.0,
            0.0,
            false,
        );
        assert_eq!(dx, 3.0, "受击目标不额外裁剪主场景位移");
        frame.system.finish_frame(1.0 / 30.0);
        assert!(!frame.system.contact_velocities.contains_key(&1));
    }

    #[test]
    fn stale_legacy_carry_never_changes_position_or_primary_velocity() {
        use bevy::ecs::system::RunSystemOnce;
        for state in [
            FighterState::Name,
            FighterState::Guard,
            FighterState::Dodge,
            FighterState::BeHit,
        ] {
            let mut app = App::new();
            app.init_resource::<CharacterContactFrame>()
                .init_resource::<Time>();
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / 30.0));
            let actors: Vec<_> = [0.0, 4.0]
                .into_iter()
                .enumerate()
                .map(|(i, x)| {
                    app.world_mut()
                        .spawn((
                            fighter(i as u8),
                            Transform::from_xyz(x, 0.0, 0.0),
                            PoseRuntime::idle(),
                            SmState::default(),
                            MotionState {
                                movement_mode: meteor_core::MovementMode::Idle,
                                velocity: Vec3::ZERO,
                                facing: 0.0,
                            },
                            Health::default(),
                            FighterAir::default(),
                        ))
                        .id()
                })
                .collect();
            app.world_mut()
                .run_system_once(begin_character_contact_frame)
                .unwrap();
            {
                let mut frame = app.world_mut().resource_mut::<CharacterContactFrame>();
                let mover = frame.indices[&actors[0]];
                let target = frame.indices[&actors[1]];
                let CharacterContactFrame { system, views, .. } = &mut *frame;
                system.resolve_xz(mover, (0.0, 0.0), (3.0, 0.0), views);
                assert_eq!(views[target].position.0, 4.0);
            }
            app.world_mut()
                .get_mut::<SmState>(actors[1])
                .unwrap()
                .current = state;
            if state == FighterState::BeHit {
                app.world_mut()
                    .get_mut::<FighterAir>(actors[1])
                    .unwrap()
                    .air
                    .airborne = true;
            }
            app.world_mut()
                .run_system_once(finish_character_contact_frame)
                .unwrap();
            let x = app
                .world()
                .get::<Transform>(actors[1])
                .unwrap()
                .translation
                .x;
            assert_eq!(
                x, 4.0,
                "{state:?}: legacy queued carry is not a stock mover"
            );
            assert_eq!(
                app.world().get::<MotionState>(actors[1]).unwrap().velocity,
                Vec3::ZERO,
                "{state:?}: legacy contact must not seed residual velocity"
            );
        }
    }

    #[test]
    fn passive_dummy_keeps_physics_ownership_but_remote_observer_does_not() {
        let remote = RemoteFighter {
            player_id: "peer".into(),
        };
        assert!(!locally_pushable(Some(&remote), None));
        assert!(locally_pushable(
            None,
            Some(&NpcPerformance {
                pause_remaining: f32::INFINITY,
                ..default()
            })
        ));
        assert!(locally_pushable(None, Some(&NpcPerformance::default())));
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn production_sn15_ground_repulsion_commits_safe_xyz_without_velocity() {
        let mesh = crate::plugins::stage_collider::sn15_test_mesh();
        for npc in [false, true] {
            let mut app = App::new();
            let dt = 1.0 / 30.0;
            app.init_resource::<Time>()
                .insert_resource(crate::plugins::FrameClock {
                    sim_dt: dt,
                    ..default()
                })
                .insert_resource(StageCollider {
                    mesh: Some(mesh.clone()),
                })
                .add_systems(Update, apply_locomotion_repulsion);
            let p = Vec3::new(2.7278004, 8.0, 46.980072);
            let actor = app
                .world_mut()
                .spawn((
                    fighter(0),
                    Transform::from_translation(p),
                    MotionState {
                        movement_mode: meteor_core::MovementMode::Idle,
                        velocity: Vec3::ZERO,
                        facing: 0.0,
                    },
                    Health::default(),
                    SmState::default(),
                    PoseRuntime::idle(),
                ))
                .id();
            if npc {
                app.world_mut()
                    .entity_mut(actor)
                    .insert(crate::plugins::combat_plugin::NpcFighter);
            }
            let mut snapshot = view(actor, p.x);
            snapshot.position = (p.x, p.y, p.z);
            snapshot.ground_y = p.y;
            let mut frame = CharacterContactFrame {
                views: vec![snapshot],
                indices: HashMap::from([(actor, 0)]),
                ..default()
            };
            frame.system.begin_frame(vec![0]);
            frame
                .authored_repulsion
                .insert(actor, Vec3::Z * (0.25 / (dt * dt)));
            app.insert_resource(frame);
            app.update();
            let after = app.world().get::<Transform>(actor).unwrap().translation;
            assert!(after.z > p.z + 0.2);
            assert!(
                !mesh.box_blocked(after.x, after.y, after.z),
                "sn15 push npc={npc} after={after:?}"
            );
            assert_eq!(
                app.world().get::<MotionState>(actor).unwrap().velocity,
                Vec3::ZERO
            );
            let frame = app.world().resource::<CharacterContactFrame>();
            assert_eq!(frame.views[0].position, (after.x, after.y, after.z));
        }
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn authored_repulsion_second_mover_commits_stage_collision_xyz() {
        use bevy::ecs::system::RunSystemOnce;
        let mesh = crate::plugins::stage_collider::sn15_test_mesh();
        let mut app = App::new();
        let dt = 1.0 / 30.0;
        app.init_resource::<Time>()
            .init_resource::<CharacterContactFrame>()
            .insert_resource(crate::plugins::FrameClock {
                sim_dt: dt,
                ..default()
            })
            .insert_resource(StageCollider {
                mesh: Some(mesh.clone()),
            });
        let p = Vec3::new(2.7278004, 8.0, 46.980072);
        let actor = app
            .world_mut()
            .spawn((
                fighter(0),
                Transform::from_translation(p),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                SmState {
                    current: FighterState::Attack1,
                    ..default()
                },
                Health::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<CharacterContactFrame>()
            .authored_repulsion
            .insert(actor, Vec3::Z * (0.25 / (dt * dt)));
        app.world_mut()
            .run_system_once(apply_authored_repulsion)
            .unwrap();
        let after = app.world().get::<Transform>(actor).unwrap().translation;
        assert!(after.z > p.z + 0.2);
        assert!(
            !mesh.box_blocked(after.x, after.y, after.z),
            "second mover entered stage: {after:?}"
        );
        let velocity = app.world().get::<MotionState>(actor).unwrap().velocity;
        assert_eq!(
            velocity,
            Vec3::ZERO,
            "stage-resolved repulsion is not primary velocity"
        );
    }

    #[test]
    fn production_finish_never_adds_contact_carry_or_residual_velocity() {
        let mut app = App::new();
        let mut time: Time = Time::default();
        let dt = 1.0 / 30.0;
        time.advance_by(std::time::Duration::from_secs_f32(dt));
        app.insert_resource(time)
            .add_systems(Update, finish_character_contact_frame);
        let mover = app
            .world_mut()
            .spawn((
                fighter(0),
                Transform::from_xyz(0.0, 0.0, 0.0),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Run,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                Health::default(),
            ))
            .id();
        let target = app
            .world_mut()
            .spawn((
                fighter(1),
                Transform::from_xyz(4.0, 0.0, 0.0),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                Health::default(),
            ))
            .id();
        let mut frame = CharacterContactFrame {
            views: vec![view(mover, 0.0), view(target, 4.0)],
            indices: HashMap::from([(mover, 0), (target, 1)]),
            ..default()
        };
        frame.system.begin_frame(vec![0, 1]);
        let stage: CollisionSystem<StageMeshCollider> =
            CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        let (dx, _, _) = resolve_fighter_xz(
            Some(&mut frame),
            mover,
            &stage,
            Point3::ZERO,
            3.0,
            0.0,
            false,
        );
        frame.commit_position(mover, Vec3::new(dx, 0.0, 0.0));
        app.insert_resource(frame);

        app.update();

        let transform = app.world().get::<Transform>(target).unwrap();
        let motion = app.world().get::<MotionState>(target).unwrap();
        assert_eq!(transform.translation.x, 4.0);
        assert_eq!(motion.velocity, Vec3::ZERO);
    }
}
