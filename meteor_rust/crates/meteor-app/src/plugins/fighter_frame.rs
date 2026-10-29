use bevy::ecs::schedule::{InternedScheduleLabel, ScheduleLabel, SingleThreadedExecutor};
use bevy::prelude::*;

use super::stage_plugin::LocalFighter;
use super::FighterFrameSet;
use crate::components::fighter::Fighter;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FighterUpdate;

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SerialFighterSet;

#[derive(Resource, Default)]
struct RegisteredFighters {
    entries: Vec<(Entity, u64)>,
    next_registration: u64,
}

#[derive(Resource, Default)]
pub(crate) struct ActiveFighter(pub Option<Entity>);

pub(crate) fn accepts(active: Option<&ActiveFighter>, entity: Entity) -> bool {
    active
        .and_then(|active| active.0)
        .is_none_or(|current| current == entity)
}

pub(crate) fn current_is_local(
    active: Option<Res<ActiveFighter>>,
    local: Query<(), With<LocalFighter>>,
) -> bool {
    active
        .and_then(|active| active.0)
        .is_none_or(|entity| local.contains(entity))
}

pub(crate) fn schedule(app: &App) -> InternedScheduleLabel {
    if installed(app) {
        FighterUpdate.intern()
    } else {
        Update.intern()
    }
}

pub(crate) fn installed(app: &App) -> bool {
    app.world().contains_resource::<RegisteredFighters>()
}

fn playing(state: Option<Res<State<crate::app_state::AppState>>>) -> bool {
    state.is_none_or(|state| *state.get() == crate::app_state::AppState::Playing)
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<RegisteredFighters>()
        .init_resource::<ActiveFighter>()
        .init_schedule(FighterUpdate)
        .edit_schedule(FighterUpdate, |schedule| {
            schedule.set_executor(SingleThreadedExecutor::new());
        })
        .add_observer(
            |event: On<Add, Fighter>, mut actors: ResMut<RegisteredFighters>| {
                let registration = actors.next_registration;
                actors.next_registration += 1;
                actors.entries.push((event.entity, registration));
            },
        )
        .add_observer(
            |event: On<Remove, Fighter>, mut actors: ResMut<RegisteredFighters>| {
                actors.entries.retain(|(entity, _)| *entity != event.entity);
            },
        )
        .configure_sets(
            FighterUpdate,
            (
                FighterFrameSet::Movement,
                FighterFrameSet::PoseDecision,
                FighterFrameSet::Locomotion,
                FighterFrameSet::HitJudge,
            )
                .run_if(super::battle_result::gameplay_active),
        )
        .configure_sets(
            FighterUpdate,
            (
                FighterFrameSet::Movement,
                FighterFrameSet::PoseDecision,
                FighterFrameSet::Locomotion,
                (
                    FighterFrameSet::PoseAdvance,
                    FighterFrameSet::SkeletonSample,
                    FighterFrameSet::HitJudge,
                    FighterFrameSet::RootMotion,
                    FighterFrameSet::AttackFacing,
                    FighterFrameSet::PoseResolve,
                )
                    .chain()
                    .run_if(super::simulation_frame),
            )
                .chain(),
        )
        .add_systems(
            Update,
            run_fighters
                .in_set(SerialFighterSet)
                .after(FighterFrameSet::PoseDecision)
                .after(super::network_plugin::NetworkFrameApply)
                .before(FighterFrameSet::Locomotion)
                .run_if(playing),
        );
    #[cfg(not(target_arch = "wasm32"))]
    super::combat_trace::install(app);
}

fn run_fighters(world: &mut World) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(15);

    let mut previous = None;
    loop {
        let next = world
            .resource::<RegisteredFighters>()
            .entries
            .iter()
            .copied()
            .find(|(_, registration)| previous.is_none_or(|old| *registration > old));
        let Some((entity, registration)) = next else {
            break;
        };
        previous = Some(registration);
        world.resource_mut::<ActiveFighter>().0 = Some(entity);

        #[cfg(not(target_arch = "wasm32"))]
        let _actor_perf = super::perf_plugin::Scope::new(18);
        world.run_schedule(FighterUpdate);
    }
    world.resource_mut::<ActiveFighter>().0 = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_preserves_deferred_hits_and_mid_frame_roster_changes() {
        #[derive(Component)]
        struct Interrupted;
        #[derive(Resource)]
        struct Probe {
            source: Entity,
            target: Entity,
            removed: Entity,
            spawned: Option<Entity>,
            seen: Vec<(Entity, bool)>,
        }
        fn observe(
            active: Res<ActiveFighter>,
            mut probe: ResMut<Probe>,
            interrupted: Query<(), With<Interrupted>>,
        ) {
            let entity = active.0.unwrap();
            probe.seen.push((entity, interrupted.contains(entity)));
        }
        fn hit(
            active: Res<ActiveFighter>,
            mut probe: ResMut<Probe>,
            fighters: Query<&Fighter>,
            mut commands: Commands,
        ) {
            if active.0 == Some(probe.source) {
                commands.entity(probe.target).insert(Interrupted);
                commands.entity(probe.removed).remove::<Fighter>();
                probe.spawned = Some(
                    commands
                        .spawn(fighters.get(probe.source).unwrap().clone())
                        .id(),
                );
            }
        }
        for single in [false, true] {
            let mut app = App::new();
            super::super::configure_fighter_frame_sets(&mut app);
            install(&mut app);
            app.edit_schedule(FighterUpdate, |schedule| {
                if !single {
                    schedule.set_executor(bevy::ecs::schedule::MultiThreadedExecutor::new());
                }
            });
            let actors: Vec<_> = (0..3)
                .map(|slot| {
                    app.world_mut()
                        .spawn(Fighter {
                            character_id: 1,
                            player_slot: slot,
                            weapon: 6,
                            weapon_slots: [6, 0],
                            weapon_slot: 0,
                            weapon_slot_variants: [0, 0],
                            weapon_variant: 0,
                        })
                        .id()
                })
                .collect();
            app.insert_resource(Probe {
                source: actors[0],
                target: actors[1],
                removed: actors[2],
                spawned: None,
                seen: Vec::new(),
            })
            .add_systems(
                FighterUpdate,
                (
                    observe.in_set(FighterFrameSet::Movement),
                    hit.in_set(FighterFrameSet::HitJudge),
                ),
            );
            app.world_mut().run_schedule(Update);
            let probe = app.world().resource::<Probe>();
            assert_eq!(
                probe.seen,
                vec![
                    (actors[0], false),
                    (actors[1], true),
                    (probe.spawned.unwrap(), false),
                ],
                "single={single}: commands must flush before the next registered actor"
            );
            assert!(app.world().resource::<ActiveFighter>().0.is_none());
        }
    }

    #[derive(Resource, Default)]
    struct Updates {
        seen: Vec<Entity>,
        reregister: Option<Entity>,
    }

    fn tick(world: &mut World) {
        let entity = world.resource::<ActiveFighter>().0.unwrap();
        world.resource_mut::<Updates>().seen.push(entity);
        if let Some(target) = world.resource_mut::<Updates>().reregister.take() {
            let fighter = world.get::<Fighter>(target).unwrap().clone();
            world.entity_mut(target).remove::<Fighter>().insert(fighter);
        }
    }

    #[test]
    fn registration_survives_archetype_moves_and_follows_mid_tick_tail_insertion() {
        let mut app = App::new();
        super::super::configure_fighter_frame_sets(&mut app);
        install(&mut app);
        app.init_resource::<Updates>()
            .add_systems(FighterUpdate, tick);
        let actors: Vec<_> = (0..3)
            .map(|slot| {
                app.world_mut()
                    .spawn(Fighter {
                        character_id: 1,
                        player_slot: slot,
                        weapon: 6,
                        weapon_slots: [6, 0],
                        weapon_slot: 0,
                        weapon_slot_variants: [0, 0],
                        weapon_variant: 0,
                    })
                    .id()
            })
            .collect();
        app.world_mut()
            .entity_mut(actors[0])
            .insert(Name::new("different archetype"));
        app.world_mut().run_schedule(Update);
        assert_eq!(app.world().resource::<Updates>().seen, actors);
        app.world_mut().resource_mut::<Updates>().seen.clear();
        app.world_mut().resource_mut::<Updates>().reregister = Some(actors[1]);
        app.world_mut().run_schedule(Update);
        assert_eq!(
            app.world().resource::<Updates>().seen,
            vec![actors[0], actors[2], actors[1]]
        );
        app.world_mut().resource_mut::<Updates>().seen.clear();
        app.world_mut().run_schedule(Update);
        assert_eq!(
            app.world().resource::<Updates>().seen,
            vec![actors[0], actors[2], actors[1]]
        );
        assert!(app.world().resource::<ActiveFighter>().0.is_none());
    }
}
