use super::*;
use crate::plugins::network_plugin::{NetState, NetStatus, NetTx, RemoteFighter};
use meteor_network::protocol::Message;
use meteor_network::runtime_sync::{ProjectileState, ProjectileTracker};
use std::collections::HashMap;

type Scope = (String, Option<u32>, Option<u32>, Option<u32>);
fn scope(net: &NetState) -> Scope {
    (
        net.room_id.clone(),
        net.match_epoch,
        net.round_epoch,
        net.stock_runtime_generation,
    )
}
#[derive(Resource, Default)]
struct SourceFlights {
    scope: Scope,
    next_id: u32,
    next_sample: f64,
    sent: HashMap<Entity, ProjectileState>,
}
#[derive(Resource, Default)]
struct ObservedFlights {
    scope: Scope,
    tracker: ProjectileTracker,
    entities: HashMap<(String, u32), Entity>,
}
#[derive(Component)]
pub(super) struct ObserverProjectile {
    pub owner: Entity,
    pub kind: ProjectileKind,
    pub weapon: u8,
    pub variant: u8,
    sequence: u32,
    target: Vec3,
    rotation: Quat,
    velocity: Vec3,
    received_at: f64,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<SourceFlights>()
        .init_resource::<ObservedFlights>()
        .add_systems(
            Update,
            (
                publish_flights,
                receive_flights.before(super::attach_projectile_visuals),
            )
                .chain()
                .after(super::update_projectiles)
                .after(crate::plugins::fighter_frame::SerialFighterSet)
                .after(crate::plugins::network_plugin::NetworkFrameApply)
                .run_if(crate::plugins::ui_plugin::gameplay_hud_visibility::scene_session_active),
        )
        .add_systems(
            Update,
            move_observers
                .after(receive_flights)
                .run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            OnExit(AppState::Playing),
            clear_observers.run_if(crate::plugins::ends_playing_session),
        );
}
fn publish_flights(
    time: Res<Time>,
    net: Res<NetState>,
    tx: Option<Res<NetTx>>,
    next_state: Option<Res<NextState<AppState>>>,
    mut source: ResMut<SourceFlights>,
    projectiles: Query<(Entity, &Projectile, &Transform)>,
    fighters: Query<&Fighter>,
) {
    if next_state.is_some_and(|next| matches!(&*next, NextState::Pending(state) if !matches!(state, AppState::Playing | AppState::Paused))) { return; }
    if source.scope != scope(&net) {
        *source = SourceFlights {
            scope: scope(&net),
            ..default()
        };
    }
    if net.status != NetStatus::InRoom || !net.round_active || !net.projectile_sync_enabled {
        return;
    }
    let Some(tx) = tx else { return };
    let now = time.elapsed_secs_f64();
    let due = now >= source.next_sample;
    if due {
        source.next_sample = now + 0.1;
    }
    let mut present = std::collections::HashSet::new();
    for (entity, projectile, transform) in &projectiles {
        let kind = match projectile.kind {
            ProjectileKind::Dart => 1,
            ProjectileKind::FlyingWheel => 2,
            _ => continue,
        };
        let owner = if let Some(actor) = net.stock_npc_actors.get(&projectile.source.owner) {
            format!("stock-{actor}")
        } else if net.is_stock_host() && !fighters.get(projectile.source.owner).is_ok() {
            continue;
        } else {
            net.player_id.clone()
        };
        present.insert(entity);
        let initial = !source.sent.contains_key(&entity);
        if !initial && !due {
            continue;
        }
        let id = if let Some(previous) = source.sent.get(&entity) {
            previous.id
        } else {
            source.next_id = source.next_id.wrapping_add(1).max(1);
            source.next_id
        };
        let sequence = source
            .sent
            .get(&entity)
            .map_or(1, |s| s.sequence.wrapping_add(1));
        let state = ProjectileState {
            player_id: owner,
            id,
            sequence,
            kind,
            weapon: projectile.source.weapon,
            variant: source.sent.get(&entity).map_or_else(
                || {
                    fighters
                        .get(projectile.source.owner)
                        .map_or(0, |f| f.weapon_variant)
                },
                |previous| previous.variant,
            ),
            pose_id: projectile.source.pose_id,
            pose_frame: projectile.source.pose_frame,
            position: transform.translation.to_array(),
            rotation: transform.rotation.to_array(),
            velocity: if source
                .sent
                .get(&entity)
                .is_some_and(|s| s.age == projectile.age)
            {
                [0.0; 3]
            } else {
                projectile.velocity.to_array()
            },
            age: projectile.age,
            active: true,
        };
        if !state.valid() {
            continue;
        }
        tx.send_control(message(&net, state.clone()));
        source.sent.insert(entity, state);
    }
    let removed = source
        .sent
        .keys()
        .filter(|e| !present.contains(e))
        .copied()
        .collect::<Vec<_>>();
    for entity in removed {
        if let Some(mut state) = source.sent.remove(&entity) {
            state.sequence = state.sequence.wrapping_add(1);
            state.active = false;
            tx.send_control(message(&net, state));
        }
    }
}
fn message(net: &NetState, state: ProjectileState) -> Message {
    Message::ProjectileState {
        room_id: net.room_id.clone(),
        match_epoch: net.match_epoch.unwrap_or(0),
        round_epoch: net.round_epoch.unwrap_or(0),
        state,
    }
}
fn receive_flights(
    mut commands: Commands,
    time: Res<Time>,
    mut net: ResMut<NetState>,
    mut flights: ResMut<ObservedFlights>,
    fighters: Query<(Entity, &RemoteFighter)>,
    mut observers: Query<&mut ObserverProjectile>,
) {
    if flights.scope != scope(&net) {
        for (_, entity) in flights.entities.drain() {
            commands.entity(entity).despawn();
        }
        flights.tracker.clear();
        flights.scope = scope(&net);
    }
    if net.status != NetStatus::InRoom || !net.round_active {
        for (_, entity) in flights.entities.drain() {
            commands.entity(entity).despawn();
        }
        flights.tracker.clear();
        net.pending_projectiles.clear();
        return;
    }
    let now = time.elapsed_secs_f64();
    for owner in std::mem::take(&mut net.pending_projectile_owner_clears) {
        flights.tracker.forget_owner(&owner);
        let keys = flights
            .entities
            .keys()
            .filter(|(player, _)| player == &owner)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(entity) = flights.entities.remove(&key) {
                commands.entity(entity).despawn();
            }
        }
    }
    for state in std::mem::take(&mut net.pending_projectiles) {
        flights.tracker.accept(state, (now * 1000.0) as u64);
    }
    let _ = flights.tracker.expire((now * 1000.0) as u64);
    let live = flights.tracker.active().cloned().collect::<Vec<_>>();
    let present = live
        .iter()
        .map(|s| (s.player_id.clone(), s.id))
        .collect::<std::collections::HashSet<_>>();
    let removed = flights
        .entities
        .keys()
        .filter(|key| !present.contains(*key))
        .cloned()
        .collect::<Vec<_>>();
    for key in removed {
        if let Some(entity) = flights.entities.remove(&key) {
            commands.entity(entity).despawn();
        }
    }
    for state in live {
        let key = (state.player_id.clone(), state.id);
        let Some((owner, _)) = fighters
            .iter()
            .find(|(_, remote)| remote.player_id == state.player_id)
        else {
            if let Some(entity) = flights.entities.remove(&key) {
                commands.entity(entity).despawn();
            }
            continue;
        };
        let target = Vec3::from_array(state.position);
        let rotation = Quat::from_array(state.rotation).normalize();
        let component = ObserverProjectile {
            owner,
            kind: if state.kind == 1 {
                ProjectileKind::Dart
            } else {
                ProjectileKind::FlyingWheel
            },
            weapon: state.weapon,
            variant: state.variant,
            sequence: state.sequence,
            target,
            rotation,
            velocity: Vec3::from_array(state.velocity),
            received_at: now,
        };
        if let Some(&entity) = flights.entities.get(&key) {
            if let Ok(mut observer) = observers.get_mut(entity) {
                if observer.sequence != state.sequence {
                    *observer = component;
                }
            }
        } else {
            let entity = commands
                .spawn((
                    component,
                    Transform::from_translation(target).with_rotation(rotation),
                    Visibility::Visible,
                ))
                .id();
            flights.entities.insert(key, entity);
        }
    }
}
fn move_observers(
    time: Res<Time>,
    fighters: Query<(), With<Fighter>>,
    mut observers: Query<(&ObserverProjectile, &mut Transform)>,
) {
    let dt = time.delta_secs();
    let amount = 1.0 - (-25.0 * dt).exp();
    for (observer, mut transform) in &mut observers {
        if fighters.get(observer.owner).is_err() {
            continue;
        }
        let prediction = (time.elapsed_secs_f64() - observer.received_at).clamp(0.0, 0.1) as f32;
        transform.translation = transform
            .translation
            .lerp(observer.target + observer.velocity * prediction, amount);
        transform.rotation = transform.rotation.slerp(observer.rotation, amount);
    }
}
fn clear_observers(
    mut commands: Commands,
    query: Query<Entity, With<ObserverProjectile>>,
    mut flights: ResMut<ObservedFlights>,
) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
    *flights = default();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(kind: u8) -> ProjectileState {
        ProjectileState {
            player_id: "p2".into(),
            id: 1,
            sequence: 1,
            kind,
            weapon: if kind == 1 { 1 } else { 2 },
            variant: 0,
            pose_id: 200,
            pose_frame: 3519.0,
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            velocity: [3.0, 4.0, 5.0],
            age: 0.1,
            active: true,
        }
    }
    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<ObservedFlights>()
            .insert_resource(NetState {
                status: NetStatus::InRoom,
                room_id: "room".into(),
                player_id: "p1".into(),
                round_active: true,
                ..default()
            })
            .init_resource::<PendingRangedHits>()
            .init_resource::<StageCollider>()
            .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>()
            .add_systems(
                Update,
                (
                    receive_flights,
                    move_observers,
                    super::super::update_projectiles,
                )
                    .chain(),
            );
        app
    }
    fn owner(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "p2".into(),
                },
                Fighter {
                    character_id: 0,
                    player_slot: 1,
                    weapon: 1,
                    weapon_slots: [1, 2],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health::default(),
                Transform::default(),
            ))
            .id()
    }
    #[test]
    fn observer_flights_wait_for_actor_and_move_without_damage_or_projectile_judges() {
        for kind in [1, 2] {
            let mut app = app();
            app.world_mut()
                .resource_mut::<NetState>()
                .pending_projectiles
                .push(packet(kind));
            app.update();
            assert_eq!(
                app.world_mut()
                    .query::<&ObserverProjectile>()
                    .iter(app.world())
                    .count(),
                0
            );
            let source = owner(&mut app);
            app.update();
            let entity = *app
                .world()
                .resource::<ObservedFlights>()
                .entities
                .values()
                .next()
                .unwrap();
            let before = app.world().get::<Transform>(entity).unwrap().translation;
            let mut state = packet(kind);
            state.sequence = 2;
            state.position = [2.0, 4.0, 6.0];
            app.world_mut()
                .resource_mut::<NetState>()
                .pending_projectiles
                .push(state);
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(0.05));
            app.update();
            assert!(
                app.world()
                    .get::<Transform>(entity)
                    .unwrap()
                    .translation
                    .distance(before)
                    > 0.01
            );
            assert_eq!(
                app.world_mut()
                    .query::<&Projectile>()
                    .iter(app.world())
                    .count(),
                0
            );
            assert!(app.world().resource::<PendingRangedHits>().0.is_empty());
            assert_eq!(
                app.world().get::<Health>(source).unwrap().current,
                Health::default().current
            );
            let mut stop = packet(kind);
            stop.sequence = 3;
            stop.active = false;
            app.world_mut()
                .resource_mut::<NetState>()
                .pending_projectiles
                .push(stop);
            app.update();
            assert!(app.world().get_entity(entity).is_err());
            let mut resurrect = packet(kind);
            resurrect.sequence = 4;
            app.world_mut()
                .resource_mut::<NetState>()
                .pending_projectiles
                .push(resurrect);
            app.update();
            assert!(app
                .world()
                .resource::<ObservedFlights>()
                .entities
                .is_empty());
        }
    }
    #[test]
    fn observer_flights_clear_on_departure_and_round_change_without_stale_resurrection() {
        let mut app = app();
        owner(&mut app);
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_projectiles
            .push(packet(1));
        app.update();
        let old = *app
            .world()
            .resource::<ObservedFlights>()
            .entities
            .values()
            .next()
            .unwrap();
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_projectile_owner_clears
            .push("p2".into());

        app.world_mut()
            .resource_mut::<NetState>()
            .pending_projectiles
            .push(packet(1));
        app.update();
        assert!(app.world().get_entity(old).is_err());
        assert_eq!(app.world().resource::<ObservedFlights>().entities.len(), 1);
        app.world_mut().resource_mut::<NetState>().round_epoch = Some(2);
        app.update();
        assert!(app
            .world()
            .resource::<ObservedFlights>()
            .entities
            .is_empty());
        assert!(app
            .world_mut()
            .query::<&ObserverProjectile>()
            .iter(app.world())
            .next()
            .is_none());
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use crate::components::projectile::RangedHitSource;
    #[test]
    fn source_flights_publish_real_samples_freeze_variant_and_emit_one_terminal_record() {
        let (tx, output) = crate::plugins::network_plugin::control_test_channel();
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<SourceFlights>()
            .insert_resource(tx)
            .insert_resource(NetState {
                status: NetStatus::InRoom,
                room_id: "room".into(),
                player_id: "p1".into(),
                round_active: true,
                projectile_sync_enabled: true,
                match_epoch: Some(1),
                round_epoch: Some(1),
                ..default()
            })
            .add_systems(Update, publish_flights);
        let owner = app
            .world_mut()
            .spawn(Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 1,
                weapon_slots: [1, 2],
                weapon_slot: 0,
                weapon_slot_variants: [2, 0],
                weapon_variant: 2,
            })
            .id();
        let attack = meteor_asset::pos::AttackBlock {
            start: 3517,
            end: 3519,
            attack_type: 0,
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
            bones: Vec::new(),
        };
        let projectile = app
            .world_mut()
            .spawn((
                Projectile {
                    kind: ProjectileKind::Dart,
                    source: RangedHitSource {
                        owner,
                        weapon: 1,
                        weapon_stats: None,
                        pose_id: 200,
                        pose_frame: 3519.0,
                        pose_start: 3511,
                        pose_instance_id: 1,
                        attack_index: 0,
                        attack,
                        player_slot: 0,
                        combat_team: 1,
                        script_character_id: 0,
                        script_team: Some(1),
                        attack_origin: Vec3::ZERO,
                        attack_forward: Vec3::Z,
                    },
                    velocity: Vec3::new(1.0, 2.0, 3.0),
                    age: 0.05,
                    hit_scale: 1.0,
                    speed: 42.0,
                    launch_point: Vec3::ZERO,
                    guide_target: None,
                    guide_initialized: true,
                    route_to_target: false,
                    collision_return: false,
                },
                Transform::from_translation(Vec3::new(2.0, 4.0, 6.0)),
            ))
            .id();
        app.update();
        let Message::ProjectileState { state: first, .. } = output().pop().unwrap() else {
            panic!("flight expected")
        };
        assert_eq!(first.position, [2.0, 4.0, 6.0]);
        assert_eq!(first.velocity, [1.0, 2.0, 3.0]);
        assert_eq!(first.variant, 2);
        app.world_mut()
            .get_mut::<Fighter>(owner)
            .unwrap()
            .weapon_variant = 3;
        app.world_mut()
            .get_mut::<Projectile>(projectile)
            .unwrap()
            .age = 0.2;
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.11));
        app.update();
        let Message::ProjectileState { state: second, .. } = output().pop().unwrap() else {
            panic!("flight expected")
        };
        assert_eq!(second.id, first.id);
        assert_eq!(second.sequence, first.sequence + 1);
        assert_eq!(
            second.variant, 2,
            "a wielded variant change does not replace an existing dart"
        );
        app.world_mut().despawn(projectile);
        app.update();
        let messages = output();
        assert_eq!(messages.len(), 1);
        assert!(
            matches!(&messages[0], Message::ProjectileState { state, .. } if !state.active && state.id == first.id)
        );
        app.update();
        assert!(output().is_empty());
    }
}
