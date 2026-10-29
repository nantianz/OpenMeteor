use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use super::stage_collider::StageCollider;
use crate::app_state::{AppState, LoadedStageInfo, Settings};
use bevy::prelude::*;

#[cfg(feature = "perf-diagnostics")]
mod render_timing;

const NAMES: [&str; 25] = [
    "ground",
    "foot",
    "body",
    "ray",
    "camera",
    "materials",
    "radar",
    "animation",
    "fmc",
    "deform",
    "scripts",
    "ai",
    "ui_layout_wall",
    "npc_movement",
    "hit_judge",
    "fighter_schedule_wall",
    "pon_players",
    "pon_particles",
    "actor_wall",
    "ai_snapshot",
    "mission_script",
    "nav_step",
    "nav_plan",
    "nav_walkable",
    "nav_start_scan",
];
static ACTIVE: AtomicBool = AtomicBool::new(false);
static NANOS: [AtomicU64; NAMES.len()] = [const { AtomicU64::new(0) }; NAMES.len()];
static CALLS: [AtomicU64; NAMES.len()] = [const { AtomicU64::new(0) }; NAMES.len()];
static MAX_NANOS: [AtomicU64; NAMES.len()] = [const { AtomicU64::new(0) }; NAMES.len()];

const EVENT_NAMES: [&str; 8] = [
    "gmdl_prepare",
    "shared_buffer_new",
    "params_buffer_new",
    "uv_reconfigure",
    "ifl_change",
    "pon_prepare",
    "pon_buffer_new",
    "pon_uniform_write",
];
static EVENTS: [AtomicU64; EVENT_NAMES.len()] = [const { AtomicU64::new(0) }; EVENT_NAMES.len()];
pub(crate) fn count_event(index: usize) {
    EVENTS[index].fetch_add(1, Relaxed);
}

pub(crate) struct Scope(usize, Option<Instant>);
impl Scope {
    pub(crate) fn new(index: usize) -> Self {
        Self(index, ACTIVE.load(Relaxed).then(Instant::now))
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(start) = self.1 {
            let nanos = start.elapsed().as_nanos() as u64;
            NANOS[self.0].fetch_add(nanos, Relaxed);
            MAX_NANOS[self.0].fetch_max(nanos, Relaxed);
            CALLS[self.0].fetch_add(1, Relaxed);
        }
    }
}

#[derive(Resource, Default)]
struct Samples {
    previous: Option<Instant>,
    start: Option<Instant>,
    frames: Vec<f64>,
    main_ms: f64,
    main_max_ms: f64,
    measured_frames: u64,
}

pub(crate) struct PerfPlugin;
impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        if !cfg!(feature = "perf-diagnostics")
            && std::env::var("METEOR_PERF_LOG").as_deref() != Ok("1")
        {
            return;
        }
        info!(target: "meteor::perf", "perf_capture enabled=true gpu_instrumented={} interval_s=5", cfg!(feature = "perf-diagnostics"));
        #[cfg(feature = "perf-diagnostics")]
        render_timing::install(app);
        app.init_resource::<Samples>()
            .add_systems(First, begin)
            .add_systems(Last, finish)
            .init_resource::<UiTiming>()
            .add_systems(PostUpdate, begin_ui.before(bevy::ui::UiSystems::Prepare))
            .add_systems(
                PostUpdate,
                end_ui
                    .after(begin_ui)
                    .after(bevy::ui::UiSystems::PostLayout),
            );
    }
}

#[derive(Resource, Default)]
struct UiTiming(Option<Scope>);
fn begin_ui(mut timing: ResMut<UiTiming>) {
    timing.0 = Some(Scope::new(12));
}
fn end_ui(mut timing: ResMut<UiTiming>) {
    drop(timing.0.take());
}

fn begin(state: Res<State<AppState>>, mut samples: ResMut<Samples>) {
    let active = *state.get() == AppState::Playing;
    ACTIVE.store(active, Relaxed);
    if !active {
        *samples = Samples::default();
        for event in &EVENTS {
            event.store(0, Relaxed);
        }
        for i in 0..NAMES.len() {
            NANOS[i].store(0, Relaxed);
            CALLS[i].store(0, Relaxed);
            MAX_NANOS[i].store(0, Relaxed);
        }
        return;
    }
    let now = Instant::now();
    if let Some(previous) = samples.previous {
        samples
            .frames
            .push(now.duration_since(previous).as_secs_f64() * 1000.0);
    }
    samples.previous = Some(now);
    samples.start = Some(now);
}

fn finish(
    mut samples: ResMut<Samples>,
    stage: Res<LoadedStageInfo>,
    collider: Res<StageCollider>,
    settings: Res<Settings>,
    request: Option<Res<crate::app_state::LaunchRequest>>,
    fighters: Query<(), With<crate::components::fighter::Fighter>>,
    npcs: Query<(), With<super::stage_plugin::MissionNpc>>,
    effects: Query<(), With<super::effect_plugin::PonEfInstance>>,
    particles: Query<(), With<super::effect_plugin::PonEfParticle>>,
    windows: Query<&Window>,
    cameras: Query<
        (
            &Camera,
            Option<&super::display_settings_plugin::RenderScaleResolution>,
        ),
        With<super::camera_plugin::GameplayCamera>,
    >,
) {
    let Some(start) = samples.start.take() else {
        return;
    };

    samples.measured_frames += 1;
    let main_ms = start.elapsed().as_secs_f64() * 1000.0;
    samples.main_ms += main_ms;
    samples.main_max_ms = samples.main_max_ms.max(main_ms);
    let total: f64 = samples.frames.iter().sum();
    if total < 5000.0 {
        return;
    }
    let count = samples.frames.len() as f64;
    samples.frames.sort_by(f64::total_cmp);
    let p95 = samples.frames[((count * 0.95).ceil() as usize).saturating_sub(1)];
    let p50 = samples.frames[((count * 0.5).ceil() as usize).saturating_sub(1)];
    let p99 = samples.frames[((count * 0.99).ceil() as usize).saturating_sub(1)];
    let over33 = samples
        .frames
        .iter()
        .filter(|&&ms| ms > 1000.0 / 30.0)
        .count();
    let over50 = samples.frames.iter().filter(|&&ms| ms > 50.0).count();
    let events = EVENT_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| format!("{name}={}", EVENTS[i].swap(0, Relaxed)))
        .collect::<Vec<_>>()
        .join(" ");
    let max = samples.frames.last().copied().unwrap_or_default();
    let measured = samples.measured_frames as f64;
    let timing = NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let ms = NANOS[i].swap(0, Relaxed) as f64 / 1e6 / measured;
            let calls = CALLS[i].swap(0, Relaxed) as f64 / measured;
            let max_ms = MAX_NANOS[i].swap(0, Relaxed) as f64 / 1e6;
            format!("{name}={ms:.2}ms/{calls:.1}calls {name}_max_ms={max_ms:.2}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let resolution = windows
        .iter()
        .next()
        .map(|w| (w.physical_width(), w.physical_height()));
    let window = windows.iter().next();
    let dpi = window.map(|w| w.scale_factor());
    let render_size = cameras.iter().next().and_then(|(camera, override_size)| {
        override_size
            .map(|size| size.0)
            .or_else(|| camera.physical_viewport_size())
    });
    info!(target: "meteor::perf", "perf_context stage={} mode={} mission_npcs={} effects={} particles={} main_max_ms={:.2} gpu_instrumented={}",
        stage.stage_id, request.as_deref().map_or("unknown", |r| r.mode.as_str()),
        npcs.iter().count(), effects.iter().count(), particles.iter().count(), samples.main_max_ms, cfg!(feature = "perf-diagnostics"));
    info!(target: "meteor::perf", "perf_v3 stage={} debug={} fps={:.1} frame_ms={:.2} p95_ms={:.2} max_ms={:.2} main_wall_ms={:.2} tris={} fighters={} pixels={:?} scale={} cap={} render_pixels={:?} dpi={:?} {} inclusive_scopes=true gpu_ms=see_render_perf_v2 post_update_executor={} frames={} p50_ms={:.2} p99_ms={:.2} over33={} over50={} {}",
        stage.stage_id, cfg!(debug_assertions), count * 1000.0 / total, total / count,
        p95, max, samples.main_ms / measured,
        collider.mesh.as_ref().map_or(0, |m| m.triangle_count()), fighters.iter().count(),
        resolution, settings.render_scale, settings.target_fps, render_size, dpi, timing, super::frame_work::post_update_executor(), count, p50, p99, over33, over50, events);
    samples.frames.clear();
    samples.main_ms = 0.0;
    samples.main_max_ms = 0.0;
    samples.measured_frames = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_capture_flushes_and_resets_when_leaving_playing() {
        let mut app = App::new();
        app.insert_resource(State::new(AppState::Playing))
            .init_resource::<LoadedStageInfo>()
            .init_resource::<StageCollider>()
            .init_resource::<Settings>()
            .init_resource::<Samples>()
            .add_systems(First, begin)
            .add_systems(Last, finish);
        app.update();
        app.world_mut().resource_mut::<Samples>().previous =
            Some(Instant::now() - std::time::Duration::from_secs(6));
        app.update();
        let samples = app.world().resource::<Samples>();
        assert!(samples.frames.is_empty());
        assert_eq!(samples.measured_frames, 0);
        assert_eq!(samples.main_max_ms, 0.0);
        app.insert_resource(State::new(AppState::Menu));
        app.update();
        assert!(app.world().resource::<Samples>().previous.is_none());
    }
}
