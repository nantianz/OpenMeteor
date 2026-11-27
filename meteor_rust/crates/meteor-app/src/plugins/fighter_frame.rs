use bevy::ecs::schedule::{InternedScheduleLabel, ScheduleLabel, SingleThreadedExecutor};
use bevy::prelude::*;

use super::stage_plugin::LocalFighter;
use super::FighterFrameSet;
use crate::components::fighter::Fighter;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FighterUpdate;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FighterStepStart;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FighterStepEnd;

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

pub(crate) fn install(app: &mut App) {
    app.init_resource::<RegisteredFighters>()
        .init_resource::<ActiveFighter>()
        .init_schedule(FighterUpdate)
        .init_schedule(FighterStepStart)
        .init_schedule(FighterStepEnd)
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
                .run_if(super::ui_plugin::gameplay_hud_visibility::scene_session_active),
        );
    #[cfg(not(target_arch = "wasm32"))]
    super::combat_trace::install(app);
    #[cfg(not(target_arch = "wasm32"))]
    super::landing_trace::install(app);
    #[cfg(not(target_arch = "wasm32"))]
    super::jump_trace::install(app);
}

fn run_fighters(world: &mut World) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(15);
    let interval = world
        .get_resource::<super::FrameClock>()
        .filter(|_| world.contains_resource::<super::FrameClockInstalled>())
        .map(|clock| {
            (
                clock.sim_dt,
                clock.wall_dt,
                clock.wall_step,
                clock.measured_wall_step,
            )
        });
    let Some((total_dt, wall_dt, wall_step, measured_wall_step)) =
        interval.filter(|(dt, _, _, _)| *dt > 0.0 && dt.is_finite())
    else {
        run_controller_step(world);
        return;
    };
    let now = world.get_resource::<Time<bevy::time::Real>>().map_or_else(
        || world.resource::<Time>().elapsed_secs_f64(),
        |time| time.elapsed_secs_f64(),
    );

    let clock = world.resource::<super::FrameClock>();
    if let Some(steps) = clock.fixed_steps {
        let remainder = clock.pending_wall;
        let input_after_ticks = remainder > 1e-6 && has_fresh_local_input(world, now);
        for index in 0..steps.count {
            let remaining_steps = steps.count - index - 1;
            let remaining_wall = remainder + f64::from(remaining_steps) * steps.wall_dt;
            run_timed_controller_step(
                world,
                steps.sim_dt,
                steps.wall_dt as f32,
                super::ControllerSubstep {
                    now_seconds: now - remaining_wall,
                    first: index == 0,
                    last: remaining_steps == 0 && !input_after_ticks,
                    remaining_wall_seconds: remaining_wall,
                },
            );
        }
        if input_after_ticks {
            run_timed_controller_step(
                world,
                0.0,
                0.0,
                super::ControllerSubstep {
                    now_seconds: now,
                    first: false,
                    last: true,
                    remaining_wall_seconds: 0.0,
                },
            );
        }
    } else {
        run_variable_steps(world, now, total_dt, wall_step, measured_wall_step);
    }
    let mut clock = world.resource_mut::<super::FrameClock>();
    clock.sim_dt = total_dt;
    clock.wall_dt = wall_dt;
    clock.wall_step = wall_step;
    clock.substep = None;
}
fn has_fresh_local_input(world: &World, now: f64) -> bool {
    use super::input_plugin::LocalCombatInput;
    let Some(input) = world
        .get_resource::<LocalCombatInput>()
        .filter(|input| input.captured_real_at_seconds == Some(now))
    else {
        return false;
    };
    !input.edges.is_empty()
        || world
            .get_resource::<ButtonInput<KeyCode>>()
            .is_some_and(|keys| {
                keys.get_just_pressed().next().is_some()
                    || keys.get_just_released().next().is_some()
            })
        || world
            .get_resource::<ButtonInput<MouseButton>>()
            .is_some_and(|buttons| {
                buttons.get_just_pressed().next().is_some()
                    || buttons.get_just_released().next().is_some()
            })
}
fn run_variable_steps(
    world: &mut World,
    now: f64,
    total_dt: f32,
    wall_step: f32,
    measured_wall_step: f64,
) {
    let max_step = f64::from(1.0_f32 / 120.0);
    let total = f64::from(total_dt);
    let rounding_tolerance = total * f64::from(f32::EPSILON) * 2.0;

    let wall_span = if measured_wall_step > 0.0 {
        measured_wall_step
    } else {
        f64::from(wall_step).max(0.0)
    }
    .min(f64::from(meteor_core::MAX_FRAME_DT));
    let mut consumed = 0.0;
    let mut first = true;

    let mut replay_steps = world
        .get_resource::<super::FrameClock>()
        .map(|clock| clock.replay_steps.clone())
        .filter(|steps| !steps.is_empty())
        .map(|steps| steps.into_iter());
    while consumed < total {
        let remaining = total - consumed;

        let step = match replay_steps.as_mut().and_then(Iterator::next) {
            Some(step) => f64::from(step).min(remaining),
            None if remaining <= max_step + rounding_tolerance => remaining,
            None => max_step,
        };
        let step = if replay_steps.as_ref().is_some_and(|steps| steps.len() == 0) {
            remaining
        } else {
            step
        };
        consumed = (consumed + step).min(total);
        let step_wall = if total > 0.0 {
            wall_span * step / total
        } else {
            0.0
        };
        let step_now = if consumed == total {
            now
        } else {
            now - wall_span * (1.0 - consumed / total)
        };
        run_timed_controller_step(
            world,
            step as f32,
            step_wall as f32,
            super::ControllerSubstep {
                now_seconds: step_now,
                first,
                last: consumed == total,
                remaining_wall_seconds: (now - step_now).max(0.0),
            },
        );
        first = false;
    }
}
fn run_timed_controller_step(
    world: &mut World,
    dt: f32,
    wall_dt: f32,
    step: super::ControllerSubstep,
) {
    {
        let mut clock = world.resource_mut::<super::FrameClock>();
        clock.sim_dt = dt;
        clock.wall_dt = wall_dt;
        clock.wall_step = wall_dt;
        clock.substep = Some(step);
        if dt > 0.0 {
            clock.completed_controller_steps += 1;
        }
    }
    run_controller_step(world);
}

fn local_attack_step_predates_input(world: &mut World, entity: Entity) -> bool {
    use crate::components::fighter::{EntryBlend, PoseRuntime, SmState};
    if world.get::<LocalFighter>(entity).is_none() {
        return false;
    }
    let Some(step) = world
        .get_resource::<super::FrameClock>()
        .filter(|clock| clock.sim_dt > 0.0)
        .and_then(|clock| clock.substep)
    else {
        return false;
    };
    let Some((instance, entered_at)) = world
        .get::<EntryBlend>(entity)
        .and_then(|blend| blend.local_input_start)
    else {
        return false;
    };
    let same_attack = world
        .get::<PoseRuntime>(entity)
        .is_some_and(|pose| pose.pose_instance_id == instance)
        && world.get::<SmState>(entity).is_some_and(|state| {
            matches!(
                state.current,
                meteor_core::FighterState::Attack1
                    | meteor_core::FighterState::Attack2
                    | meteor_core::FighterState::Attack3
            )
        });

    if same_attack && step.now_seconds <= entered_at + 1e-8 {
        return true;
    }
    world
        .get_mut::<EntryBlend>(entity)
        .unwrap()
        .local_input_start = None;
    false
}

fn run_controller_step(world: &mut World) {
    world.run_schedule(FighterStepStart);

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
        if local_attack_step_predates_input(world, entity) {
            continue;
        }
        world.resource_mut::<ActiveFighter>().0 = Some(entity);

        #[cfg(not(target_arch = "wasm32"))]
        let _actor_perf = super::perf_plugin::Scope::new(18);
        world.run_schedule(FighterUpdate);
    }
    world.resource_mut::<ActiveFighter>().0 = None;
    world.run_schedule(FighterStepEnd);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_clock_substeps_keep_actor_order_elapsed_time_and_one_input_edge() {
        #[derive(Resource, Default)]
        struct StepSamples {
            actors: Vec<(Entity, f32, f64, bool, bool)>,
            starts: usize,
            ends: usize,
        }
        for (fps, expected) in [(30, 4), (60, 2), (120, 1), (240, 1)] {
            let mut app = App::new();
            super::super::configure_fighter_frame_sets(&mut app);
            install(&mut app);
            app.init_resource::<Time>()
                .init_resource::<Time<bevy::time::Real>>()
                .init_resource::<crate::app_state::Settings>()
                .init_resource::<StepSamples>();
            app.world_mut()
                .resource_mut::<crate::app_state::Settings>()
                .variable_physics = true;
            super::super::install_frame_clock(&mut app);
            app.add_systems(FighterStepStart, |mut samples: ResMut<StepSamples>| {
                samples.starts += 1
            })
            .add_systems(FighterStepEnd, |mut samples: ResMut<StepSamples>| {
                samples.ends += 1
            })
            .add_systems(
                FighterUpdate,
                |active: Res<ActiveFighter>,
                 clock: Res<super::super::FrameClock>,
                 mut samples: ResMut<StepSamples>| {
                    samples.actors.push((
                        active.0.unwrap(),
                        clock.sim_dt,
                        super::super::simulation_now(Some(&clock), -1.0),
                        super::super::first_simulation_substep(Some(&clock)),
                        super::super::input_simulation_substep(Some(&clock)),
                    ));
                },
            );
            let actors: Vec<_> = (0..2)
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
            let duration = std::time::Duration::from_secs_f32(1.0 / fps as f32);
            app.world_mut().resource_mut::<Time>().advance_by(duration);
            app.world_mut()
                .resource_mut::<Time<bevy::time::Real>>()
                .advance_by(duration);
            app.update();
            let samples = app.world().resource::<StepSamples>();
            assert_eq!(
                (samples.starts, samples.ends),
                (expected, expected),
                "fps={fps}"
            );
            assert_eq!(samples.actors.len(), expected * 2);
            for (index, pair) in samples.actors.chunks_exact(2).enumerate() {
                assert_eq!([pair[0].0, pair[1].0], [actors[0], actors[1]]);
                assert_eq!(
                    pair[0].2, pair[1].2,
                    "actors share one substep wall instant"
                );
                assert_eq!(pair[0].3, index == 0);
                assert_eq!(pair[1].3, index == 0);
                assert_eq!(pair[0].4, index == expected - 1);
                assert_eq!(pair[1].4, index == expected - 1);
            }
            let elapsed: f64 = samples
                .actors
                .iter()
                .step_by(2)
                .map(|sample| f64::from(sample.1))
                .sum();
            assert!(
                (elapsed - duration.as_secs_f64()).abs() < 2e-9,
                "no elapsed time lost: fps={fps}"
            );
            assert_eq!(samples.actors.last().unwrap().2, duration.as_secs_f64());
        }
    }

    #[test]
    fn production_clock_substeps_discard_clamped_history_without_rewinding_real_age() {
        #[derive(Resource, Default)]
        struct StepSamples(Vec<(f32, f64, f64, bool)>);
        let mut app = App::new();
        super::super::configure_fighter_frame_sets(&mut app);
        install(&mut app);
        app.init_resource::<Time>()
            .init_resource::<Time<bevy::time::Real>>()
            .init_resource::<crate::app_state::Settings>()
            .init_resource::<StepSamples>();
        app.world_mut()
            .resource_mut::<crate::app_state::Settings>()
            .variable_physics = true;
        super::super::install_frame_clock(&mut app);
        app.add_systems(
            FighterUpdate,
            |clock: Res<super::super::FrameClock>, mut samples: ResMut<StepSamples>| {
                samples.0.push((
                    clock.sim_dt,
                    super::super::simulation_now(Some(&clock), -1.0),
                    super::super::simulation_remaining_wall(Some(&clock)),
                    super::super::input_simulation_substep(Some(&clock)),
                ));
            },
        );
        app.world_mut().spawn(Fighter {
            character_id: 1,
            player_slot: 0,
            weapon: 6,
            weapon_slots: [6, 0],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        });
        let duration = std::time::Duration::from_secs(1);
        app.world_mut().resource_mut::<Time>().advance_by(duration);
        app.world_mut()
            .resource_mut::<Time<bevy::time::Real>>()
            .advance_by(duration);
        app.update();
        let samples = &app.world().resource::<StepSamples>().0;
        assert_eq!(samples.len(), 24);
        let retained = f64::from(meteor_core::MAX_FRAME_DT);
        let mut elapsed = 0.0;
        for (index, &(dt, now, remaining, input)) in samples.iter().enumerate() {
            elapsed += f64::from(dt);
            assert!((now - (1.0 - retained + elapsed)).abs() < 2e-9);
            assert!((remaining - (1.0 - now)).abs() < 2e-9);
            assert_eq!(input, index == 23);
        }
        assert!((elapsed - retained).abs() < 2e-9);
        assert_eq!(samples.last().unwrap().1, 1.0);
        let clock = app.world().resource::<super::super::FrameClock>();
        assert_eq!(
            clock.wall_step, 1.0,
            "global spawn age retains the entire stall"
        );
        assert_eq!(clock.measured_wall_step, 1.0);
        assert_eq!(
            app.world()
                .resource::<Time<bevy::time::Real>>()
                .elapsed_secs_f64(),
            1.0
        );
        assert!(clock.substep.is_none());
    }

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
