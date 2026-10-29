pub(crate) mod server_route;
pub(crate) mod studio_splash;

#[cfg(not(target_arch = "wasm32"))]
mod combat_trace;
pub(crate) mod fighter_frame;
pub(crate) mod frame_work;
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
    accumulator: f32,
    pending_wall: f32,
    target_fps: u32,
}

impl Default for FrameClock {
    fn default() -> Self {
        Self {
            sim_dt: 0.0,
            wall_dt: 0.0,
            wall_step: 0.0,
            accumulator: 0.0,
            pending_wall: 0.0,
            target_fps: 0,
        }
    }
}

fn advance_frame_clock(clock: &mut FrameClock, raw_wall_dt: f32, target_fps: u32) {
    let raw = raw_wall_dt.max(0.0);
    clock.wall_dt = raw;
    clock.pending_wall += raw;
    if clock.target_fps != target_fps {
        clock.target_fps = target_fps;
        clock.accumulator = 0.0;
    }
    let clamped = raw.min(meteor_core::MAX_FRAME_DT);
    clock.sim_dt = if target_fps == 0 {
        clamped
    } else {
        let step = 1.0 / target_fps.clamp(1, 240) as f32;
        clock.accumulator += clamped;
        let mut dt = 0.0;
        let mut steps = 0;
        while clock.accumulator >= step && steps < 4 {
            clock.accumulator -= step;
            dt += step;
            steps += 1;
        }
        dt
    };
    if clock.sim_dt > 0.0 {
        clock.wall_step = std::mem::take(&mut clock.pending_wall);
    } else {
        clock.wall_step = 0.0;
    }
}

fn begin_frame_clock(
    time: Res<bevy::time::Time<bevy::time::Real>>,
    settings: Res<crate::app_state::Settings>,
    impact: Option<Res<crate::plugins::impact_fx_plugin::ImpactSimulationScale>>,
    debug_sim: Option<Res<crate::plugins::debug_plugin::DebugSimControl>>,
    mut clock: ResMut<FrameClock>,
    loading: Option<Res<loading_ui::LoadingPresentation>>,
) {
    advance_frame_clock(&mut clock, time.delta_secs(), settings.target_fps);
    if loading.is_some_and(|loading| loading.active) {
        clock.sim_dt = 0.0;
    }
    clock.sim_dt *= impact.as_deref().map_or(1.0, |impact| impact.scale);

    if let Some(debug_sim) = debug_sim.as_deref() {
        if debug_sim.paused {
            clock.sim_dt = 0.0;
        } else {
            clock.sim_dt = (clock.sim_dt * debug_sim.time_scale).max(0.0);
        }
    }
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

pub(crate) fn install_frame_clock(app: &mut App) {
    app.init_resource::<FrameClock>()
        .add_systems(First, begin_frame_clock.after(bevy::time::TimeSystems));
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
    fn target_fps_matches_web_accumulator_and_wall_step() {
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
pub mod debug_plugin;
pub(crate) mod dialogue_portrait;
pub mod display_settings_plugin;
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
mod radar_map;
pub mod ranged_plugin;
pub mod render_plugin;
pub mod replay_plugin;
pub mod settings_ui;
pub mod stage_collider;
pub mod stage_material;
pub mod stage_plugin;
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
