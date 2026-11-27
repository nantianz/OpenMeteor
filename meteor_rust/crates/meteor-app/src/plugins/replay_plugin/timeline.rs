use super::{MrcPlayback, ReplaySeekRequest, MRC_MAX_FRAMES_PER_UPDATE};
use crate::app_state::AppState;
use crate::plugins::{debug_plugin::DebugSimControl, loading_ui::LoadingPresentation, FrameClock};
use bevy::app::MainScheduleOrder;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::time::Real;
use std::time::Duration;

const SEEK_FRAME_LIMIT: usize = 24;
const SEEK_BUDGET: Duration = Duration::from_millis(8);

const FALLBACK_HZ: f32 = 120.0;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct ReplayUpdate;

#[derive(Resource, Default)]
pub(crate) struct MrcTimeline {
    pub(crate) target: Option<f32>,
    pub(crate) restarting: bool,
    origin: Option<(Time, Time<Real>)>,
}

pub(super) fn install(app: &mut App) {
    let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
    let update = order
        .labels
        .iter_mut()
        .find(|label| **label == Update.intern())
        .expect("the replay driver requires the main Update schedule");
    *update = ReplayUpdate.intern();
    app.init_resource::<MrcTimeline>()
        .add_systems(ReplayUpdate, run_update)
        .add_systems(OnEnter(AppState::Error), cancel_restart)
        .add_systems(OnEnter(AppState::Menu), cancel_restart);
}

fn cancel_restart(mut timeline: ResMut<MrcTimeline>, mut playback: ResMut<MrcPlayback>) {
    if timeline.restarting {
        *timeline = MrcTimeline::default();
        playback.active = false;
        playback.paused = false;
    }
}

fn run_update(world: &mut World) {
    if *world.resource::<State<AppState>>().get() != AppState::Playing
        || !world.resource::<MrcPlayback>().active
        || world
            .get_resource::<LoadingPresentation>()
            .is_some_and(|loading| loading.active)
    {
        world.run_schedule(Update);
        return;
    }

    let host_time = *world.resource::<Time>();
    let host_real = *world.resource::<Time<Real>>();
    let elapsed = world.resource::<MrcPlayback>().elapsed;
    let duration = world.resource::<MrcPlayback>().duration();
    if let Some(target) = world
        .resource_mut::<ReplaySeekRequest>()
        .0
        .take()
        .filter(|t| t.is_finite())
    {
        let target = target.clamp(0.0, duration);
        world.resource_mut::<MrcTimeline>().target = Some(target);
        if target < elapsed {
            let mut timeline = world.resource_mut::<MrcTimeline>();
            timeline.restarting = true;
            timeline.origin = None;
            world
                .resource_mut::<NextState<AppState>>()
                .set(AppState::Loading);
            return;
        }
    }
    let (origin, target) = {
        let mut timeline = world.resource_mut::<MrcTimeline>();
        timeline.restarting = false;
        let origin = *timeline.origin.get_or_insert((host_time, host_real));
        (origin, timeline.target)
    };
    let seeking = target.is_some();
    let pending_time = if let Some(target) = target {
        f64::from(target)
    } else {
        let speed = world
            .get_resource::<DebugSimControl>()
            .map_or(1.0, |debug| {
                if debug.paused {
                    0.0
                } else {
                    f64::from(debug.time_scale.max(0.0))
                }
            });
        let mut playback = world.resource_mut::<MrcPlayback>();
        if !playback.paused {
            playback.frame_wall += host_real.delta_secs_f64() * speed;
        }
        playback.frame_wall
    };

    let started = Instant::now();
    let limit = if seeking {
        SEEK_FRAME_LIMIT
    } else {
        MRC_MAX_FRAMES_PER_UPDATE
    };
    let mut steps = 0;
    while steps < limit {
        let playback = world.resource::<MrcPlayback>();
        let elapsed = playback.elapsed;
        let next = next_frame(playback);
        if (!seeking && playback.paused) || f64::from(next) > pending_time {
            if seeking {
                world.resource_mut::<MrcTimeline>().target = None;
                world.resource_mut::<MrcPlayback>().frame_wall = f64::from(elapsed);
            }
            break;
        }
        let mut playback = world.resource_mut::<MrcPlayback>();
        playback.frame_cursor += 1;
        playback.frame_target = Some(next);
        run_frame(world, origin, elapsed, next);
        steps += 1;
        if seeking && started.elapsed() >= SEEK_BUDGET {
            break;
        }
    }
    if steps == 0 {
        run_frame(world, origin, elapsed, elapsed);
    }
    if !seeking {
        let mut playback = world.resource_mut::<MrcPlayback>();
        if f64::from(playback.elapsed) + 0.25 < playback.frame_wall {
            playback.frame_wall = f64::from(playback.elapsed);
        }
    }
    world.insert_resource(host_time);
    world.insert_resource(host_real);
}

fn next_frame(playback: &MrcPlayback) -> f32 {
    let candidate = playback
        .frame_times
        .get(playback.frame_cursor)
        .copied()
        .unwrap_or_else(|| {
            if playback.frame_times.is_empty() {
                (playback.frame_cursor + 1) as f32 / FALLBACK_HZ
            } else {
                playback.elapsed + 1.0 / FALLBACK_HZ
            }
        });

    let duration = playback.duration();
    if playback.elapsed < duration {
        candidate.min(duration)
    } else {
        candidate
    }
}

fn run_frame(world: &mut World, (mut time, mut real): (Time, Time<Real>), from: f32, to: f32) {
    let elapsed = Duration::from_secs_f32(from);
    let step = Duration::from_secs_f32(to - from);
    time.advance_by(elapsed);
    time.advance_by(step);
    real.advance_by(elapsed);
    real.advance_by(step);
    world.insert_resource(time);
    world.insert_resource(real);
    world
        .resource_mut::<FrameClock>()
        .set_mrc_step(to, to - from);
    world.run_schedule(Update);
}

#[cfg(test)]
mod tests {
    use super::super::MrcRemoteFighter;
    use super::*;
    use crate::app_state::{Keymap, LaunchRequest, Settings};
    use crate::components::fighter::{Fighter, MotionState};
    use crate::plugins::camera_plugin::CameraBasis;
    use crate::plugins::network_plugin::{NetState, RemoteVitalsStatus};
    use crate::plugins::stage_plugin::LocalFighter;
    use meteor_asset::mrc::{MrcQueueAEntry, MrcQueueBEntry};
    use meteor_network::role_state::{
        encode_event2_state, forward_from_yaw, role_state_from_runtime, RoleStateRuntime, RoleVec3,
    };
    use meteor_network::stock_protocol::StockSceneTakenEvent;

    #[derive(Resource, Default, Debug, PartialEq)]
    struct Samples(Vec<(f32, f32, [u8; 2], f32)>);

    fn spawn_actor(mut commands: Commands) {
        commands.spawn((
            LocalFighter,
            Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 6,
                weapon_slots: [6, 0],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
            Transform::default(),
        ));
    }

    fn fixture(paused: bool) -> App {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .insert_state(AppState::Playing)
            .init_resource::<Time>()
            .init_resource::<Time<Real>>()
            .init_resource::<Keymap>()
            .init_resource::<Settings>()
            .init_resource::<LaunchRequest>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<CameraBasis>()
            .init_resource::<NetState>()
            .init_resource::<Samples>();
        crate::plugins::install_frame_clock(&mut app);
        crate::plugins::configure_fighter_frame_sets(&mut app);
        app.add_plugins(super::super::ReplayPlugin)
            .add_systems(OnEnter(AppState::Playing), spawn_actor)
            .add_systems(
                OnExit(AppState::Playing),
                |mut commands: Commands,
                 actors: Query<Entity, Or<(With<LocalFighter>, With<MrcRemoteFighter>)>>| {
                    for actor in &actors {
                        commands.entity(actor).despawn();
                    }
                },
            )
            .add_systems(
                OnEnter(AppState::Loading),
                |mut next: ResMut<NextState<AppState>>| {
                    next.set(AppState::Playing);
                },
            )
            .add_systems(
                Update,
                (|playback: Res<MrcPlayback>,
                  clock: Res<FrameClock>,
                  keys: Res<ButtonInput<KeyCode>>,
                  mut actor: Single<(&mut Transform, &Fighter), With<LocalFighter>>,
                  remotes: Query<
                    (&Transform, &RemoteVitalsStatus),
                    (With<MrcRemoteFighter>, Without<LocalFighter>),
                  >,
                  mut samples: ResMut<Samples>| {
                    let (position, fighter) = &mut *actor;
                    if keys.pressed(KeyCode::KeyW) {
                        position.translation.x += clock.sim_dt;
                    }
                    let (remote, vitals) = remotes.single().expect("recorded opponent");
                    assert!(!vitals.health_known && !vitals.anger_known);
                    samples.0.push((
                        playback.elapsed,
                        position.translation.x,
                        fighter.weapon_slots,
                        remote.translation.x,
                    ));
                })
                .after(crate::plugins::FighterFrameSet::Movement)
                .run_if(in_state(AppState::Playing))
                .run_if(crate::plugins::simulation_frame),
            );
        let mut doc = super::super::mrc_tests::empty_mrc_document();
        doc.state[4..8].copy_from_slice(&1_u32.to_le_bytes());
        let remote_sample = |time, x| {
            let state = role_state_from_runtime(RoleStateRuntime {
                position: RoleVec3 { x, y: 0.0, z: 0.0 },
                forward: forward_from_yaw(0.0),
                velocity: RoleVec3::ZERO,
                pose_id: 0,
                grounded: true,
                guard_route: false,
                playback_arg: 0,
                pose_sequence: 1,
            });
            let mut blob = 2_u16.to_le_bytes().to_vec();
            blob.extend_from_slice(&encode_event2_state(state));
            MrcQueueAEntry { id: 2, time, blob }
        };
        doc.queue_a.push(remote_sample(0.01, 1.0));
        doc.queue_a.push(MrcQueueAEntry {
            id: 32,
            time: 0.025,
            blob: StockSceneTakenEvent {
                actor_id: 1,
                record_id: 5,
                category: 1,
                item_id: 7,
            }
            .encode()
            .to_vec(),
        });
        doc.queue_a.push(remote_sample(0.03, 9.0));
        doc.queue_b = vec![
            MrcQueueBEntry {
                code: 0x57,
                value: 1,
                time: 0.01,
            },
            MrcQueueBEntry {
                code: 0x57,
                value: 0,
                time: 0.04,
            },
            MrcQueueBEntry {
                code: 0x57,
                value: 0,
                time: 1.0,
            },
        ];
        app.insert_resource(MrcPlayback {
            active: true,
            paused,
            doc: Some(doc),
            frame_times: vec![0.01, 0.02, 0.03, 0.04, 0.05],
            ..default()
        });
        app
    }

    fn seek_to(app: &mut App, target: f32) {
        app.world_mut().resource_mut::<ReplaySeekRequest>().0 = Some(target);
        for _ in 0..16 {
            app.update();
            let timeline = app.world().resource::<MrcTimeline>();
            if timeline.target.is_none() && !timeline.restarting {
                return;
            }
        }
        panic!("seek never completed");
    }

    #[test]
    fn timeline_seek_replays_each_frame_and_reloads_before_rewinding() {
        let mut linear = fixture(false);
        for _ in 0..4 {
            linear
                .world_mut()
                .resource_mut::<Time<Real>>()
                .advance_by(Duration::from_millis(10));
            linear.update();
        }
        let linear_samples = &linear.world().resource::<Samples>().0;
        assert_eq!(linear_samples.len(), 4);
        assert_eq!(linear_samples[0].2, [6, 0]);
        assert_eq!(linear_samples[2].2, [6, 7]);
        assert!(linear_samples[2].1 > 0.02);
        assert!(linear_samples[3].3 > linear_samples[0].3);
        assert!(linear_samples[3].3 < 9.0);

        let mut replay = fixture(true);
        seek_to(&mut replay, 0.04);
        assert_eq!(&replay.world().resource::<Samples>().0, linear_samples);
        assert!(replay.world().resource::<MrcPlayback>().paused);

        replay.world_mut().resource_mut::<Samples>().0.clear();
        seek_to(&mut replay, 0.02);
        assert_eq!(
            &replay.world().resource::<Samples>().0,
            &linear_samples[..2]
        );
        assert!(replay.world().resource::<MrcPlayback>().paused);
    }
}
