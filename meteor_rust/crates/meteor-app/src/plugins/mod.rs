pub(crate) mod graphics_pack;
pub mod mrc_file_picker;
pub(crate) mod server_route;
pub(crate) mod studio_splash;

#[cfg(not(target_arch = "wasm32"))]
mod combat_trace;
pub(crate) mod fighter_frame;
pub(crate) mod frame_work;
#[cfg(not(target_arch = "wasm32"))]
mod jump_trace;
#[cfg(not(target_arch = "wasm32"))]
mod landing_trace;
pub(crate) mod rebalance_plugin;
mod render_pacing;
pub(crate) mod status_plugin;

use bevy::prelude::{
    App, First, IntoScheduleConfigs, PreviousState, Res, ResMut, Resource, State, SystemSet,
    Update, World,
};

use crate::app_state::AppState;

pub(crate) fn starts_fresh_playing_session(previous: Option<Res<PreviousState<AppState>>>) -> bool {
    previous.is_none_or(|previous| *previous.get() != AppState::Paused)
}

pub(crate) fn ends_playing_session(current: Res<State<AppState>>) -> bool {
    *current.get() != AppState::Paused
}

pub(crate) fn cleanup_suspended_playing_session(world: &mut World) {
    if let Err(error) = world.try_run_schedule(bevy::state::prelude::OnExit(AppState::Playing)) {
        bevy::log::error!("failed to clean suspended Playing session: {error}");
    }
}

#[derive(Resource, Debug)]
pub(crate) struct FrameClock {
    pub sim_dt: f32,
    pub wall_dt: f32,
    pub wall_step: f32,
    pending_wall: f64,
    fixed_steps: Option<FixedControllerSteps>,
    #[cfg(test)]
    accumulator: f32,
    #[cfg(test)]
    target_fps: u32,
    pub(crate) measured_wall_step: f64,

    pub(crate) completed_controller_steps: u32,
    play_elapsed_seconds: Option<f64>,
    play_frame_seconds: f64,

    pub(crate) substep: Option<ControllerSubstep>,

    pub(crate) replay_steps: Vec<f32>,
}
#[derive(Debug, Clone, Copy)]
struct FixedControllerSteps {
    count: u32,
    wall_dt: f64,
    sim_dt: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ControllerSubstep {
    pub now_seconds: f64,
    pub first: bool,
    pub last: bool,
    pub remaining_wall_seconds: f64,
}

#[derive(Resource)]
pub(crate) struct FrameClockInstalled;

impl Default for FrameClock {
    fn default() -> Self {
        Self {
            sim_dt: 0.0,
            wall_dt: 0.0,
            wall_step: 0.0,
            pending_wall: 0.0,
            fixed_steps: None,
            #[cfg(test)]
            accumulator: 0.0,
            #[cfg(test)]
            target_fps: 0,
            measured_wall_step: 0.0,
            completed_controller_steps: 0,
            play_elapsed_seconds: None,
            play_frame_seconds: 0.0,
            substep: None,
            replay_steps: Vec::new(),
        }
    }
}

#[cfg(test)]
fn advance_frame_clock(clock: &mut FrameClock, raw_wall_dt: f32, target_fps: u32) {
    advance_frame_clock_seconds(clock, f64::from(raw_wall_dt), target_fps);
}

#[cfg(test)]
fn advance_frame_clock_seconds(clock: &mut FrameClock, raw_wall_dt: f64, target_fps: u32) {
    let raw = start_clock_frame(clock, raw_wall_dt);
    if clock.target_fps != target_fps {
        clock.target_fps = target_fps;
        clock.accumulator = 0.0;
    }
    clock.accumulator += raw as f32;
    let interval = 1.0 / target_fps.clamp(1, 240) as f32;
    clock.sim_dt = if target_fps == 0 || clock.accumulator >= interval {
        clock.accumulator = if target_fps == 0 {
            0.0
        } else {
            clock.accumulator % interval
        };
        clock.pending_wall.min(f64::from(meteor_core::MAX_FRAME_DT)) as f32
    } else {
        0.0
    };
    if clock.sim_dt > 0.0 {
        clock.measured_wall_step = std::mem::take(&mut clock.pending_wall);
        clock.wall_step = clock.measured_wall_step as f32;

        clock.sim_dt = clock.wall_step.min(meteor_core::MAX_FRAME_DT);
    } else {
        clock.wall_step = 0.0;
        clock.measured_wall_step = 0.0;
    }
}

fn start_clock_frame(clock: &mut FrameClock, raw_wall_dt: f64) -> f64 {
    let raw = raw_wall_dt.max(0.0);
    clock.play_frame_seconds = raw;
    if let Some(elapsed) = &mut clock.play_elapsed_seconds {
        *elapsed += raw;
    }
    clock.substep = None;
    clock.completed_controller_steps = 0;
    clock.replay_steps.clear();
    clock.fixed_steps = None;
    clock.wall_dt = raw as f32;
    clock.pending_wall += raw;
    raw
}
fn advance_gameplay_clock(clock: &mut FrameClock, raw_wall_dt: f64, physics_hz: u32) {
    start_clock_frame(clock, raw_wall_dt);
    let elapsed = clock.pending_wall;
    let available = elapsed.min(f64::from(meteor_core::MAX_FRAME_DT));
    clock.sim_dt = available as f32;
    clock.pending_wall = 0.0;
    if physics_hz != 0 {
        let wall_dt = 1.0 / f64::from(physics_hz);
        let count = ((available + 1e-9) / wall_dt).floor() as u32;
        clock.pending_wall = (available - f64::from(count) * wall_dt).max(0.0);
        let sim_dt = wall_dt as f32;
        clock.sim_dt = count as f32 * sim_dt;
        clock.fixed_steps = Some(FixedControllerSteps {
            count,
            wall_dt,
            sim_dt,
        });
    }
    clock.measured_wall_step = (elapsed - clock.pending_wall).max(0.0);
    clock.wall_step = clock.measured_wall_step as f32;
}
fn effective_physics_hz(
    settings: &crate::app_state::Settings,
    net: Option<&network_plugin::NetState>,
) -> u32 {
    if let Some(net) = net.filter(|net| net.status == network_plugin::NetStatus::InRoom) {
        return if net.is_stock_udp() {
            net.stock_physics_hz
        } else {
            meteor_core::PHYSICS_HZ_DEFAULT
        };
    }
    if settings.variable_physics {
        0
    } else {
        settings.physics_hz
    }
}
fn begin_frame_clock(
    time: Res<bevy::time::Time<bevy::time::Real>>,
    settings: Res<crate::app_state::Settings>,
    net: Option<Res<network_plugin::NetState>>,
    impact: Option<Res<crate::plugins::impact_fx_plugin::ImpactSimulationScale>>,
    debug_sim: Option<Res<crate::plugins::debug_plugin::DebugSimControl>>,
    mut clock: ResMut<FrameClock>,
    loading: Option<Res<loading_ui::LoadingPresentation>>,
    mut mrc: Option<ResMut<replay_plugin::MrcPlayback>>,
    seek: Option<Res<replay_plugin::ReplaySeekRequest>>,
    player: Option<Res<replay_plugin::ReplayPlayer>>,
    mrc_timeline: Option<Res<replay_plugin::MrcTimeline>>,
) {
    let physics_hz = effective_physics_hz(&settings, net.as_deref());
    advance_gameplay_clock(&mut clock, time.delta_secs_f64(), physics_hz);
    if loading.is_some_and(|loading| loading.active) {
        clock.sim_dt = 0.0;
        clock.pending_wall = 0.0;
    } else if mrc_timeline.is_some() && mrc.as_deref().is_some_and(|mrc| mrc.active) {
        suspend_clock_frame(&mut clock);
        return;
    } else if let Some(mrc) = mrc.as_deref_mut().filter(|mrc| {
        mrc.active
            && !mrc.paused
            && !mrc.frame_times.is_empty()
            && seek.as_deref().is_none_or(|seek| seek.0.is_none())
    }) {
        let speed = debug_sim.as_deref().map_or(1.0, |debug| {
            if debug.paused {
                0.0
            } else {
                f64::from(debug.time_scale.max(0.0))
            }
        });
        pace_recorded_frames(&mut clock, mrc, time.delta_secs_f64() * speed);
        return;
    }
    let impact_scale = impact.as_deref().map_or(1.0, |impact| impact.scale);
    clock.sim_dt *= impact_scale;
    if let Some(steps) = &mut clock.fixed_steps {
        steps.sim_dt *= impact_scale;
    }

    if let Some(debug_sim) = debug_sim.as_deref() {
        if debug_sim.paused {
            suspend_clock_frame(&mut clock);
        } else {
            clock.sim_dt = (clock.sim_dt * debug_sim.time_scale).max(0.0);
            if let Some(steps) = &mut clock.fixed_steps {
                steps.sim_dt = (steps.sim_dt * debug_sim.time_scale).max(0.0);
            }
        }
    }
    if mrc.as_deref().is_some_and(|mrc| mrc.active && mrc.paused)
        || player
            .as_deref()
            .is_some_and(|player| player.active && player.paused)
    {
        suspend_clock_frame(&mut clock);
    }
}
impl FrameClock {
    pub(crate) fn set_mrc_step(&mut self, elapsed: f32, dt: f32) {
        self.sim_dt = dt;
        self.wall_dt = dt;
        self.wall_step = dt;
        self.measured_wall_step = f64::from(dt);
        self.play_elapsed_seconds = Some(f64::from(elapsed));
        self.pending_wall = 0.0;
        self.substep = None;
        self.completed_controller_steps = 0;
        self.fixed_steps = None;
        self.replay_steps.clear();
        if dt > 0.0 {
            self.replay_steps.push(dt);
        }
    }
}

fn pace_recorded_frames(
    clock: &mut FrameClock,
    mrc: &mut replay_plugin::MrcPlayback,
    raw_wall_dt: f64,
) {
    let raw = raw_wall_dt.max(0.0);
    mrc.frame_wall += raw;
    let start = mrc.frame_cursor;
    let mut end = start;
    while end < mrc.frame_times.len()
        && f64::from(mrc.frame_times[end]) <= mrc.frame_wall
        && end - start < replay_plugin::MRC_MAX_FRAMES_PER_UPDATE
    {
        end += 1;
    }
    if end == mrc.frame_times.len() {
        mrc.frame_times.clear();
        return;
    }
    if end == start {
        clock.sim_dt = 0.0;
        clock.wall_step = 0.0;
        clock.measured_wall_step = 0.0;
        clock.pending_wall = 0.0;
        clock.replay_steps.clear();
        clock.fixed_steps = None;
        return;
    }
    let mut previous = mrc.elapsed.min(mrc.frame_times[start]);
    let steps: Vec<f32> = mrc.frame_times[start..end]
        .iter()
        .map(|&frame| {
            let step = (frame - previous).max(0.0);
            previous = frame;
            step
        })
        .collect();
    let total: f32 = steps.iter().sum();
    if f64::from(mrc.frame_times[end - 1]) + 0.25 < mrc.frame_wall {
        mrc.frame_wall = f64::from(mrc.frame_times[end - 1]);
    }
    mrc.frame_cursor = end;
    mrc.frame_target = Some(mrc.frame_times[end - 1]);
    clock.sim_dt = total;
    clock.wall_step = total;
    clock.measured_wall_step = f64::from(total);
    clock.pending_wall = 0.0;
    clock.fixed_steps = None;
    clock.replay_steps = steps;
}

pub(crate) fn simulation_frame(clock: Option<Res<FrameClock>>) -> bool {
    clock.is_none_or(|clock| clock.sim_dt > 0.0)
}

pub(crate) fn simulation_dt(clock: Option<&FrameClock>, fallback: f32) -> f32 {
    clock
        .map(|clock| clock.sim_dt)
        .unwrap_or_else(|| fallback.clamp(0.0, meteor_core::MAX_FRAME_DT))
}

pub(crate) fn simulation_wall_step(clock: Option<&FrameClock>, fallback: f32) -> f32 {
    clock
        .map(|clock| clock.wall_step)
        .unwrap_or_else(|| fallback.max(0.0))
}

pub(crate) fn simulation_now(clock: Option<&FrameClock>, fallback: f64) -> f64 {
    clock
        .and_then(|clock| clock.substep)
        .map_or(fallback, |step| step.now_seconds)
}

pub(crate) fn simulation_play_now(clock: Option<&FrameClock>, fallback: f64) -> f64 {
    clock
        .and_then(|clock| {
            clock
                .play_elapsed_seconds
                .map(|elapsed| elapsed - simulation_remaining_wall(Some(clock)))
        })
        .unwrap_or_else(|| simulation_now(clock, fallback))
}

fn suspend_clock_frame(clock: &mut FrameClock) {
    let play_elapsed_seconds = clock
        .play_elapsed_seconds
        .map(|elapsed| (elapsed - clock.play_frame_seconds).max(0.0));
    *clock = FrameClock {
        play_elapsed_seconds,
        ..Default::default()
    };
}

pub(crate) fn first_simulation_substep(clock: Option<&FrameClock>) -> bool {
    clock
        .and_then(|clock| clock.substep)
        .is_none_or(|step| step.first)
}

pub(crate) fn input_simulation_substep(clock: Option<&FrameClock>) -> bool {
    clock.is_none_or(|clock| {
        !clock.replay_steps.is_empty() || clock.substep.is_none_or(|step| step.last)
    })
}

pub(crate) fn simulation_remaining_wall(clock: Option<&FrameClock>) -> f64 {
    clock
        .and_then(|clock| clock.substep)
        .map_or(0.0, |step| step.remaining_wall_seconds)
}

fn suspend_offline_frame_clock(
    state: Option<Res<State<AppState>>>,
    net: Option<Res<network_plugin::NetState>>,
    mut clock: ResMut<FrameClock>,
) {
    let network_room = net.is_some_and(|net| net.status == network_plugin::NetStatus::InRoom);
    let suspended =
        state.is_some_and(|state| matches!(state.get(), AppState::Paused | AppState::Settings));
    if suspended && !network_room {
        suspend_clock_frame(&mut clock);
    }
}

pub(crate) fn install_frame_clock(app: &mut App) {
    app.init_resource::<FrameClock>();
    app.world_mut()
        .resource_mut::<FrameClock>()
        .play_elapsed_seconds
        .get_or_insert(0.0);
    app.insert_resource(FrameClockInstalled)
        .add_systems(First, begin_frame_clock.after(bevy::time::TimeSystems))
        .add_systems(
            bevy::state::state::StateTransition,
            suspend_offline_frame_clock
                .after(bevy::state::state::StateTransitionSystems::EnterSchedules),
        );
}
pub(crate) fn install_render_pacing(app: &mut App) {
    render_pacing::install(app);
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FighterFrameSet {
    Movement,
    PoseDecision,
    Locomotion,
    PoseAdvance,
    SkeletonSample,
    HitJudge,
    RootMotion,
    AttackFacing,
    PoseResolve,
    CameraFinalize,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiTouchGestureSet;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiTouchActionSet;

pub(crate) fn configure_fighter_frame_sets(app: &mut App) {
    app.configure_sets(
        Update,
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
                .run_if(simulation_frame),
            FighterFrameSet::CameraFinalize,
        )
            .chain(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct PlayingLifecycle {
        starts: u32,
        cleanups: u32,
    }

    fn record_playing_start(mut lifecycle: ResMut<PlayingLifecycle>) {
        lifecycle.starts += 1;
    }

    fn record_playing_cleanup(mut lifecycle: ResMut<PlayingLifecycle>) {
        lifecycle.cleanups += 1;
    }

    fn transition_to(app: &mut App, state: AppState) {
        app.world_mut()
            .resource_mut::<bevy::state::state::NextState<AppState>>()
            .set(state);
        app.update();
    }

    #[derive(Resource, Default)]
    struct FrameOrder(Vec<&'static str>);

    fn record_movement(mut order: ResMut<FrameOrder>) {
        order.0.push("movement");
    }

    fn record_pose_decision(mut order: ResMut<FrameOrder>) {
        order.0.push("pose-decision");
    }

    fn record_locomotion(mut order: ResMut<FrameOrder>) {
        order.0.push("locomotion");
    }

    fn record_pose_advance(mut order: ResMut<FrameOrder>) {
        order.0.push("pose-advance");
    }

    fn record_root_motion(mut order: ResMut<FrameOrder>) {
        order.0.push("root-motion");
    }

    fn record_pose_resolve(mut order: ResMut<FrameOrder>) {
        order.0.push("pose-resolve");
    }

    fn record_skeleton_sample(mut order: ResMut<FrameOrder>) {
        order.0.push("skeleton-sample");
    }

    fn record_hit_judge(mut order: ResMut<FrameOrder>) {
        order.0.push("hit-judge");
    }

    fn record_camera_finalize(mut order: ResMut<FrameOrder>) {
        order.0.push("camera-finalize");
    }

    fn record_attack_facing(mut order: ResMut<FrameOrder>) {
        order.0.push("attack-facing");
    }

    #[test]
    fn fighter_frame_sets_run_in_declared_order() {
        let mut app = App::new();
        app.init_resource::<FrameOrder>();
        configure_fighter_frame_sets(&mut app);
        app.add_systems(
            Update,
            (
                record_camera_finalize.in_set(FighterFrameSet::CameraFinalize),
                record_hit_judge.in_set(FighterFrameSet::HitJudge),
                record_skeleton_sample.in_set(FighterFrameSet::SkeletonSample),
                record_pose_resolve.in_set(FighterFrameSet::PoseResolve),
                record_root_motion.in_set(FighterFrameSet::RootMotion),
                record_attack_facing.in_set(FighterFrameSet::AttackFacing),
                record_pose_advance.in_set(FighterFrameSet::PoseAdvance),
                record_locomotion.in_set(FighterFrameSet::Locomotion),
                record_pose_decision.in_set(FighterFrameSet::PoseDecision),
                record_movement.in_set(FighterFrameSet::Movement),
            ),
        );

        app.update();

        assert_eq!(
            app.world().resource::<FrameOrder>().0,
            [
                "movement",
                "pose-decision",
                "locomotion",
                "pose-advance",
                "skeleton-sample",
                "hit-judge",
                "root-motion",
                "attack-facing",
                "pose-resolve",
                "camera-finalize",
            ]
        );
    }

    #[test]
    fn pause_resume_preserves_playing_session_and_menu_exit_cleans_it_once() {
        use bevy::state::app::AppExtStates;

        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .init_state::<AppState>()
            .init_resource::<PlayingLifecycle>()
            .add_systems(
                bevy::state::prelude::OnEnter(AppState::Playing),
                record_playing_start.run_if(starts_fresh_playing_session),
            )
            .add_systems(
                bevy::state::prelude::OnExit(AppState::Playing),
                record_playing_cleanup.run_if(ends_playing_session),
            )
            .add_systems(
                bevy::state::prelude::OnTransition {
                    exited: AppState::Paused,
                    entered: AppState::Menu,
                },
                cleanup_suspended_playing_session,
            );

        app.update();
        transition_to(&mut app, AppState::Playing);
        assert_eq!(app.world().resource::<PlayingLifecycle>().starts, 1);
        assert_eq!(app.world().resource::<PlayingLifecycle>().cleanups, 0);

        transition_to(&mut app, AppState::Paused);
        transition_to(&mut app, AppState::Playing);
        let lifecycle = app.world().resource::<PlayingLifecycle>();
        assert_eq!((lifecycle.starts, lifecycle.cleanups), (1, 0));

        transition_to(&mut app, AppState::Paused);
        transition_to(&mut app, AppState::Menu);
        let lifecycle = app.world().resource::<PlayingLifecycle>();
        assert_eq!((lifecycle.starts, lifecycle.cleanups), (1, 1));
    }

    #[test]
    fn target_fps_gates_measured_intervals_and_preserves_wall_step() {
        let mut clock = FrameClock::default();
        advance_frame_clock(&mut clock, 1.0 / 120.0, 30);
        assert_eq!(clock.sim_dt, 0.0);
        for _ in 0..3 {
            advance_frame_clock(&mut clock, 1.0 / 120.0, 30);
        }
        assert!((clock.sim_dt - 1.0 / 30.0).abs() < 1e-6);
        assert!((clock.wall_step - 4.0 / 120.0).abs() < 1e-6);

        advance_frame_clock(&mut clock, 0.2, 0);
        assert_eq!(clock.sim_dt, meteor_core::MAX_FRAME_DT);
        assert!((clock.wall_step - 0.2).abs() < 1e-6);
    }
    #[test]
    fn network_room_uses_host_frequency_without_overwriting_solo_preference() {
        let settings = crate::app_state::Settings {
            physics_hz: 150,
            ..Default::default()
        };
        let mut net = network_plugin::NetState::default();
        assert_eq!(effective_physics_hz(&settings, Some(&net)), 150);
        net.status = network_plugin::NetStatus::InRoom;
        assert_eq!(effective_physics_hz(&settings, Some(&net)), 120);
        assert_eq!(effective_physics_hz(&settings, None), 150);
        let settings = crate::app_state::Settings {
            variable_physics: true,
            ..settings
        };
        assert_eq!(effective_physics_hz(&settings, Some(&net)), 120);
        #[cfg(not(target_arch = "wasm32"))]
        {
            net.address = "udp://127.0.0.1:5230".into();
            net.stock_physics_hz = 173;
            assert_eq!(effective_physics_hz(&settings, Some(&net)), 173);
            let mut clock = FrameClock::default();
            advance_gameplay_clock(
                &mut clock,
                1.0 / 173.0,
                effective_physics_hz(&settings, Some(&net)),
            );
            assert_eq!(clock.fixed_steps.unwrap().count, 1);
            net.address = "ws://127.0.0.1:9000".into();
            assert_eq!(effective_physics_hz(&settings, Some(&net)), 120);
        }
        assert_eq!(effective_physics_hz(&settings, None), 0);
    }

    #[test]
    fn irregular_render_frames_are_not_quantized_or_replayed_after_stalls() {
        let mut clock = FrameClock::default();
        advance_frame_clock(&mut clock, 0.02, 30);
        assert_eq!(clock.sim_dt, 0.0);
        advance_frame_clock(&mut clock, 0.02, 30);
        assert!((clock.sim_dt - 0.04).abs() < 1e-6);
        assert!((clock.wall_step - 0.04).abs() < 1e-6);

        advance_frame_clock(&mut clock, 1.0, 30);
        assert_eq!(clock.sim_dt, 0.2);
        assert_eq!(clock.wall_step, 1.0);
        advance_frame_clock(&mut clock, 0.01, 30);
        assert_eq!(clock.sim_dt, 0.0, "a stall must not leave catch-up steps");
        advance_frame_clock(&mut clock, 0.03, 30);
        assert!((clock.sim_dt - 0.04).abs() < 1e-6);
    }

    #[test]
    fn low_frame_rates_keep_elapsed_time_up_to_the_original_point_two_cap() {
        for fps in [0, 10, 30, 60, 120] {
            let mut clock = FrameClock::default();
            advance_frame_clock(&mut clock, 0.1, fps);
            assert!((clock.sim_dt - 0.1).abs() < 1e-6, "fps={fps}");
            advance_frame_clock(&mut clock, 0.3, fps);
            assert_eq!(clock.sim_dt, 0.2, "fps={fps}");
        }
    }

    #[test]
    fn capped_clock_jitter_preserves_elapsed_time_and_stall_cap() {
        for fps in [30, 60, 120, 144, 240] {
            let mut clock = FrameClock::default();
            let mut wall = 0.0_f64;
            let mut simulated = 0.0_f64;
            for index in 0..1000 {
                let dt = [0.0037, 0.0041, 0.0063, 0.0039][index % 4];
                wall += f64::from(dt);
                advance_frame_clock(&mut clock, dt, fps);
                if clock.sim_dt > 0.0 {
                    assert_eq!(clock.sim_dt, clock.wall_step);
                    simulated += f64::from(clock.sim_dt);
                    assert!((simulated - wall).abs() < 0.00001, "fps={fps}");
                } else {
                    assert_eq!(clock.wall_step, 0.0);
                }
            }
            advance_frame_clock(&mut clock, 0.2, fps);
            assert_eq!(clock.sim_dt, meteor_core::MAX_FRAME_DT);
            assert!(clock.wall_step >= 0.2);
        }
    }

    #[test]
    fn changing_fps_does_not_drop_time_since_the_last_simulation_update() {
        let mut clock = FrameClock::default();
        advance_frame_clock(&mut clock, 0.02, 30);
        assert_eq!(clock.sim_dt, 0.0);
        advance_frame_clock(&mut clock, 0.01, 0);
        assert!((clock.sim_dt - 0.03).abs() < 1e-6);
        assert!((clock.wall_step - 0.03).abs() < 1e-6);
    }

    #[test]
    fn installed_gameplay_wall_clock_freezes_offline_pause_but_not_lan_menu() {
        use bevy::prelude::Time;
        use bevy::state::app::AppExtStates;
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .init_state::<AppState>()
            .init_resource::<Time<bevy::time::Real>>()
            .init_resource::<crate::app_state::Settings>()
            .init_resource::<network_plugin::NetState>();
        install_frame_clock(&mut app);
        fn step(app: &mut App, seconds: f64, state: Option<AppState>) -> f64 {
            if let Some(state) = state {
                app.world_mut()
                    .resource_mut::<bevy::state::state::NextState<AppState>>()
                    .set(state);
            }
            app.world_mut()
                .resource_mut::<Time<bevy::time::Real>>()
                .advance_by(std::time::Duration::from_secs_f64(seconds));
            app.update();
            simulation_play_now(Some(app.world().resource::<FrameClock>()), -1.0)
        }
        assert!((step(&mut app, 0.5, Some(AppState::Playing)) - 0.5).abs() < 1e-9);
        assert!((step(&mut app, 0.25, Some(AppState::Paused)) - 0.5).abs() < 1e-9);
        assert!((step(&mut app, 2.0, None) - 0.5).abs() < 1e-9);
        assert!((step(&mut app, 0.01, Some(AppState::Playing)) - 0.51).abs() < 1e-9);
        app.world_mut()
            .resource_mut::<network_plugin::NetState>()
            .status = network_plugin::NetStatus::InRoom;
        assert!((step(&mut app, 1.0, Some(AppState::Paused)) - 1.51).abs() < 1e-9);
    }

    #[test]
    fn gameplay_wall_clock_includes_stalls_and_uses_substep_time_without_double_spending() {
        let mut clock = FrameClock {
            play_elapsed_seconds: Some(0.0),
            ..Default::default()
        };
        for frame in 1..=4 {
            advance_frame_clock_seconds(&mut clock, 1.0 / 120.0, 30);
            assert!(
                (simulation_play_now(Some(&clock), -1.0) - f64::from(frame) / 120.0).abs() < 1e-9
            );
        }
        clock.substep = Some(ControllerSubstep {
            now_seconds: 100.0,
            first: true,
            last: false,
            remaining_wall_seconds: 3.0 / 120.0,
        });
        assert!((simulation_play_now(Some(&clock), -1.0) - 1.0 / 120.0).abs() < 1e-9);
        advance_frame_clock_seconds(&mut clock, 0.5, 30);
        assert_eq!(clock.sim_dt, meteor_core::MAX_FRAME_DT);
        assert!((simulation_play_now(Some(&clock), -1.0) - (0.5 + 4.0 / 120.0)).abs() < 1e-9);
        suspend_clock_frame(&mut clock);
        suspend_clock_frame(&mut clock);
        assert!(
            (simulation_play_now(Some(&clock), -1.0) - 4.0 / 120.0).abs() < 1e-9,
            "two suspension gates may undo this display interval only once"
        );
        assert_eq!(
            simulation_play_now(Some(&FrameClock::default()), 7.0),
            7.0,
            "manual fixtures keep their caller's time domain"
        );
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn stock_mrc_frame_timeline_recovers_the_recorder_rate() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../mrcs/tests");
        for (file, hz) in [
            ("剑小跳.mrc", 101.0),
            ("忍烧.mrc", 125.0),
            ("指虎大绝.mrc", 131.0),
        ] {
            let doc =
                meteor_asset::mrc::decode_mrc(&std::fs::read(root.join(file)).unwrap()).unwrap();
            let frames = replay_plugin::mrc_recorded_frame_times(&doc);
            let mut gaps: Vec<f32> = frames.windows(2).map(|w| w[1] - w[0]).collect();
            gaps.sort_by(f32::total_cmp);
            let median = gaps[gaps.len() / 2];
            assert!(
                (1.0 / median - hz).abs() < 1.0,
                "{file}: {} Hz",
                1.0 / median
            );
            assert!(
                frames.windows(2).all(|w| w[1] > w[0]),
                "{file}: timeline must be increasing"
            );

            for entry in doc.queue_d.iter().take(200) {
                let nearest = frames
                    .iter()
                    .map(|t| (t - entry.time).abs())
                    .fold(f32::MAX, f32::min);
                assert!(
                    nearest < 1e-4,
                    "{file}: D at {} is not a frame end",
                    entry.time
                );
            }
        }
    }

    #[test]
    fn stock_mrc_pacing_idles_merges_and_steps_recorded_intervals() {
        let mut clock = FrameClock::default();
        let mut mrc = replay_plugin::MrcPlayback {
            active: true,
            frame_times: (1..=200).map(|k| k as f32 * 0.008).collect(),
            ..Default::default()
        };

        let mut runs = Vec::new();
        for _ in 0..120 {
            pace_recorded_frames(&mut clock, &mut mrc, 1.0 / 120.0);
            runs.push(clock.replay_steps.len());
            if let Some(target) = mrc.frame_target.take() {
                mrc.elapsed = target;
            }
            assert!((clock.sim_dt - clock.replay_steps.iter().sum::<f32>()).abs() < 1e-6);
            assert!(clock
                .replay_steps
                .iter()
                .all(|step| (step - 0.008).abs() < 1e-5));
        }
        assert!(runs.contains(&2) && !runs.contains(&0) && !runs.iter().any(|n| *n > 2));
        assert_eq!(mrc.frame_cursor, runs.iter().sum::<usize>());

        let mut slow = replay_plugin::MrcPlayback {
            active: true,
            frame_times: (1..=200).map(|k| k as f32 * 0.0099).collect(),
            ..Default::default()
        };
        let idle = (0..120)
            .filter(|_| {
                pace_recorded_frames(&mut clock, &mut slow, 1.0 / 120.0);
                if let Some(target) = slow.frame_target.take() {
                    slow.elapsed = target;
                }
                clock.sim_dt == 0.0
            })
            .count();
        assert!(
            idle >= 15,
            "only {idle} idle Updates for a 101 Hz recording"
        );
    }

    #[test]
    fn paused_mrc_freezes_simulation_without_accumulating_catch_up_time() {
        let mut app = App::new();
        app.init_resource::<bevy::time::Time<bevy::time::Real>>()
            .init_resource::<crate::app_state::Settings>()
            .init_resource::<FrameClock>()
            .insert_resource(replay_plugin::MrcPlayback {
                active: true,
                paused: true,
                ..Default::default()
            })
            .add_systems(Update, begin_frame_clock);
        app.world_mut()
            .resource_mut::<bevy::time::Time<bevy::time::Real>>()
            .advance_by(std::time::Duration::from_millis(16));
        app.update();
        let clock = app.world().resource::<FrameClock>();
        assert_eq!(
            (
                clock.sim_dt,
                clock.wall_dt,
                clock.wall_step,
                clock.pending_wall
            ),
            (0.0, 0.0, 0.0, 0.0)
        );
        app.world_mut()
            .resource_mut::<replay_plugin::MrcPlayback>()
            .paused = false;
        app.world_mut()
            .resource_mut::<bevy::time::Time<bevy::time::Real>>()
            .advance_by(std::time::Duration::from_millis(16));
        app.update();
        let clock = app.world().resource::<FrameClock>();
        assert!(clock.sim_dt > 0.0);
        assert!(clock.wall_step <= 0.0161);
    }

    #[test]
    fn capped_display_frame_keeps_input_decision_and_camera_but_skips_time_advance() {
        let mut app = App::new();
        app.init_resource::<FrameOrder>();
        app.insert_resource(FrameClock::default());
        configure_fighter_frame_sets(&mut app);
        app.add_systems(
            Update,
            (
                record_camera_finalize.in_set(FighterFrameSet::CameraFinalize),
                record_skeleton_sample.in_set(FighterFrameSet::SkeletonSample),
                record_attack_facing.in_set(FighterFrameSet::AttackFacing),
                record_pose_advance.in_set(FighterFrameSet::PoseAdvance),
                record_locomotion.in_set(FighterFrameSet::Locomotion),
                record_pose_decision.in_set(FighterFrameSet::PoseDecision),
                record_movement.in_set(FighterFrameSet::Movement),
            ),
        );

        app.update();

        assert_eq!(
            app.world().resource::<FrameOrder>().0,
            ["movement", "pose-decision", "locomotion", "camera-finalize"]
        );
    }
}

pub mod ai_plugin;
pub mod anim_plugin;
pub mod asset_plugin;
mod audio_dsp;
mod audio_output;
pub mod audio_plugin;
pub mod battle_result;
pub mod camera_plugin;
pub(crate) mod campaign_ui;
pub mod character_contact_plugin;
pub mod character_plugin;
mod combat_hud_layout;
pub mod combat_plugin;
pub(crate) mod countdown_hud;
pub mod debug_plugin;
pub(crate) mod dialogue_portrait;
pub mod display_settings_plugin;
pub mod dynamic_shadows_plugin;
pub mod editor_plugin;
pub mod effect_plugin;
pub(crate) mod escort_assets;
pub mod ifl_plugin;
pub mod impact_fx_plugin;
pub mod input_plugin;
pub mod item_plugin;
pub mod launcher_ui;
pub(crate) mod loading_ui;
pub mod mobile_controls_plugin;
pub mod movement_plugin;
pub mod native_text_input;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod native_web_p2p;
pub mod network_plugin;
mod npc_navigation;
pub mod pak_textures;
pub mod physics_plugin;
mod pst_ui;
mod radar_map;
pub mod ranged_plugin;
pub mod render_plugin;
pub mod replay_plugin;
pub(crate) mod room_map_preview;
mod scene_render_optimizations;
pub mod settings_ui;
pub mod stage_collider;
pub mod stage_material;
pub mod stage_plugin;
mod stage_presentation;
#[cfg(not(target_arch = "wasm32"))]
pub mod stock_bot;
pub(crate) mod system_notice;
pub mod ui_components;
pub mod ui_plugin;
pub mod ui_scale_plugin;
pub(crate) mod ui_theme;
pub mod weapon_trail_plugin;

pub use ai_plugin::AiPlugin;
pub use anim_plugin::AnimPlugin;
pub use asset_plugin::AssetPlugin;
pub use audio_plugin::AudioPlugin;
pub use camera_plugin::CameraPlugin;
pub use character_contact_plugin::CharacterContactPlugin;
pub use character_plugin::CharacterPlugin;
pub use combat_plugin::CombatPlugin;
pub use debug_plugin::DebugPlugin;
pub use display_settings_plugin::DisplaySettingsPlugin;
pub use dynamic_shadows_plugin::DynamicShadowsPlugin;
pub use effect_plugin::EffectPlugin;
pub use ifl_plugin::IflPlugin;
pub use impact_fx_plugin::ImpactFxPlugin;
pub use input_plugin::InputPlugin;
pub use item_plugin::ItemPlugin;
pub use mobile_controls_plugin::MobileControlsPlugin;
pub use movement_plugin::MovementPlugin;
pub use network_plugin::NetworkPlugin;
pub use physics_plugin::PhysicsPlugin;
pub use ranged_plugin::RangedPlugin;
pub use render_plugin::RenderPlugin;
pub use replay_plugin::ReplayPlugin;
pub use stage_plugin::StagePlugin;
pub use ui_components::UiComponentsPlugin;
pub use ui_plugin::UiPlugin;
pub use ui_scale_plugin::UiScalePlugin;
pub use weapon_trail_plugin::WeaponTrailPlugin;

pub(crate) mod hazard_plugin;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod perf_plugin;

pub(crate) mod scoreboard;
