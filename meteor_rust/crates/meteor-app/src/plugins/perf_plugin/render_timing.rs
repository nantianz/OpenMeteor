use super::*;
use bevy::diagnostic::{DiagnosticPath, DiagnosticsStore};
use bevy::platform::time::Instant as DiagnosticInstant;
use bevy::render::{
    diagnostic::{RecordDiagnostics, RenderDiagnosticsPlugin},
    pipelined_rendering::RenderExtractApp,
    render_resource::{Buffer, BufferDescriptor, BufferUsages, WgpuFeatures},
    renderer::{
        PendingCommandBuffers, RenderAdapterInfo, RenderDevice, RenderGraph, RenderGraphSystems,
        RenderQueue,
    },
    Render, RenderApp, RenderStartup, RenderSystems,
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

const EPOCH_PATH: &str = "render/meteor_capture_epoch";
mod gpu;

fn complete_pass_timestamp(path: &str) -> bool {
    matches!(
        path,
        "render/upscaling/elapsed_gpu"
            | "render/msaa_writeback/elapsed_gpu"
            | "render/meteor_display_post_process/elapsed_gpu"
    )
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct Distribution {
    samples: u64,
    #[serde(skip)]
    sum_ms: f64,
    mean_ms: f64,
    max_ms: f64,
}

impl Distribution {
    fn add(&mut self, ms: f64) {
        if !ms.is_finite() || ms < 0.0 {
            return;
        }
        self.samples += 1;
        self.sum_ms += ms;
        self.mean_ms = self.sum_ms / self.samples as f64;
        self.max_ms = self.max_ms.max(ms);
    }
}

#[derive(Default, Serialize)]
struct PreparedDrawCounts {
    frames: u64,
    transparent_instances: u64,
    transparent_batches: u64,
    largest_batch: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct TargetDimensions {
    view: u64,
    viewport: [u32; 4],
    color: [u32; 2],
    msaa_color: Option<[u32; 2]>,
    depth: Option<[u32; 2]>,
    samples: u32,
}

#[derive(Default)]
struct Capture {
    epoch: u32,
    stage: String,
    active: bool,
    window_start: Option<Instant>,
    handoff: Option<(u32, Instant)>,
    main_start: Option<(u32, Instant)>,
    cpu: BTreeMap<&'static str, Distribution>,
    draws: PreparedDrawCounts,
    targets: Vec<TargetDimensions>,
}

impl Capture {
    fn enter(&mut self, active: bool, stage: &str) {
        if self.active != active || self.stage != stage {
            self.epoch = self.epoch.checked_add(1).expect("capture epoch overflow");
            self.stage = stage.into();
            self.active = active;
            self.cpu.clear();
            self.draws = default();
            self.targets.clear();
            self.handoff = None;
            self.main_start = None;
            self.window_start = active.then(Instant::now);
        }
    }

    fn record(&mut self, epoch: u32, name: &'static str, elapsed: Duration) {
        if self.active && self.epoch == epoch {
            self.cpu
                .entry(name)
                .or_default()
                .add(elapsed.as_secs_f64() * 1000.0);
        }
    }
}

#[derive(Resource, Clone, Default)]
struct SharedCapture(Arc<Mutex<Capture>>);

#[derive(Resource, Clone, Copy, Default)]
struct ExtractedCapture {
    epoch: u32,
    active: bool,
}

#[derive(Resource, Default)]
struct RenderClock {
    start: Option<(u32, Instant)>,
    submit: Option<Instant>,
}

#[derive(Resource)]
struct EpochBuffer(Buffer);

#[derive(Resource, Clone, Default, Serialize)]
struct Support {
    backend: String,
    timestamp_query: bool,
    encoder_timestamps: bool,
    pass_timestamps: bool,
    pipeline_statistics: bool,
    timestamp_period_ns: f32,
}

impl Support {
    fn new(device: &RenderDevice, queue: &RenderQueue, adapter: &RenderAdapterInfo) -> Self {
        let features = device.features();
        Self {
            backend: format!("{:?}", adapter.backend),
            timestamp_query: features.contains(WgpuFeatures::TIMESTAMP_QUERY),
            encoder_timestamps: features.contains(WgpuFeatures::TIMESTAMP_QUERY_INSIDE_ENCODERS)
                && !cfg!(target_os = "macos"),
            pass_timestamps: features.contains(WgpuFeatures::TIMESTAMP_QUERY_INSIDE_PASSES)
                && !cfg!(target_os = "macos"),
            pipeline_statistics: features.contains(WgpuFeatures::PIPELINE_STATISTICS_QUERY),
            timestamp_period_ns: queue.get_timestamp_period(),
        }
    }

    fn can_tag(&self) -> bool {
        self.timestamp_query || self.pipeline_statistics
    }
}

#[derive(Default)]
struct PassWindow {
    epoch: u32,
    last_seen: BTreeMap<String, DiagnosticInstant>,
    samples: BTreeMap<String, Distribution>,
    discarded: u64,
}

impl PassWindow {
    fn collect(&mut self, store: &DiagnosticsStore, epoch: u32, active: bool, support: &Support) {
        if self.epoch != epoch {
            self.epoch = epoch;
            self.samples.clear();
            self.discarded = 0;
        }
        let epoch_path = DiagnosticPath::const_new(EPOCH_PATH);
        let tags: BTreeMap<_, _> = store
            .get(&epoch_path)
            .into_iter()
            .flat_map(|d| d.measurements())
            .map(|m| (m.time, m.value))
            .collect();
        for diagnostic in store.iter() {
            let path = diagnostic.path().as_str();
            let gpu = path.ends_with("/elapsed_gpu");
            if !path.starts_with("render/") || !(gpu || path.ends_with("/elapsed_cpu")) {
                continue;
            }
            let previous = self.last_seen.get(path).copied();
            for measurement in diagnostic
                .measurements()
                .filter(|m| previous.is_none_or(|t| m.time > t))
            {
                self.last_seen.insert(path.into(), measurement.time);

                if !active || tags.get(&measurement.time) != Some(&(epoch as f64)) {
                    self.discarded += 1;
                    continue;
                }

                if gpu
                    && (!support.encoder_timestamps
                        || support.backend == "Metal"
                        || !complete_pass_timestamp(path))
                {
                    continue;
                }
                self.samples
                    .entry(path.into())
                    .or_default()
                    .add(measurement.value);
            }
        }
    }
}

pub(super) fn install(app: &mut App) {
    if app.get_sub_app(RenderApp).is_none() {
        return;
    }
    let shared = SharedCapture::default();
    app.insert_resource(shared.clone())
        .init_resource::<DiagnosticsStore>()
        .add_plugins(RenderDiagnosticsPlugin)
        .add_systems(First, begin_main.before(super::begin))
        .add_systems(Last, report.after(super::finish));
    let render = app.sub_app_mut(RenderApp);
    render
        .insert_resource(shared)
        .init_resource::<ExtractedCapture>()
        .init_resource::<RenderClock>()
        .add_systems(RenderStartup, setup_support)
        .add_systems(
            Render,
            (
                begin_render.before(RenderSystems::ExtractCommands),
                count_prepared_draws
                    .after(RenderSystems::Prepare)
                    .before(RenderSystems::Render),
                begin_submit
                    .after(RenderSystems::Prepare)
                    .before(RenderSystems::Render),
                end_submit
                    .after(RenderSystems::Render)
                    .before(RenderSystems::Cleanup),
                end_render.after(RenderSystems::PostCleanup),
            ),
        )
        .add_systems(
            RenderGraph,
            record_epoch
                .after(bevy::render::diagnostic::begin_diagnostics_frame)
                .before(RenderGraphSystems::Render),
        );
    gpu::install(app);
    wrap_extraction(app);
}

fn wrap_extraction(app: &mut App) {
    let shared = app.world().resource::<SharedCapture>().clone();
    let render = app.sub_app_mut(RenderApp);
    let mut original = render.take_extract();
    let inner_shared = shared.clone();
    render.set_extract(move |main, render| {
        let start = Instant::now();
        let stamp = {
            let mut capture = inner_shared.0.lock().unwrap();
            if let Some((epoch, handoff)) = capture.handoff {
                capture.record(
                    epoch,
                    "main_wait_render_wall",
                    start.duration_since(handoff),
                );
            }
            ExtractedCapture {
                epoch: capture.epoch,
                active: capture.active,
            }
        };
        if let Some(original) = original.as_mut() {
            original(main, render);
        }
        inner_shared
            .0
            .lock()
            .unwrap()
            .record(stamp.epoch, "extract_sync_wall", start.elapsed());
        render.insert_resource(stamp);
    });
    if let Some(extract) = app.get_sub_app_mut(RenderExtractApp) {
        let mut original = extract.take_extract();
        extract.set_extract(move |main, extract| {
            let start = Instant::now();
            let epoch = {
                let mut capture = shared.0.lock().unwrap();
                let epoch = capture.epoch;
                capture.handoff = Some((epoch, start));
                epoch
            };
            if let Some(original) = original.as_mut() {
                original(main, extract);
            }
            let mut capture = shared.0.lock().unwrap();
            capture.record(epoch, "handoff_total_wall", start.elapsed());
            capture.handoff = None;
        });
    }
}

fn begin_main(
    state: Res<State<AppState>>,
    stage: Res<LoadedStageInfo>,
    shared: Res<SharedCapture>,
    gpu: Res<gpu::GpuResults>,
) {
    let mut capture = shared.0.lock().unwrap();
    capture.enter(*state.get() == AppState::Playing, &stage.stage_id);
    gpu.enter(capture.epoch, capture.active);
    capture.main_start = capture.active.then(|| (capture.epoch, Instant::now()));
}

fn setup_support(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    adapter: Res<RenderAdapterInfo>,
) {
    let support = Support::new(&device, &queue, &adapter);
    info!(target: "meteor::perf", "render_caps_v1 {}", serde_json::to_string(&support).unwrap());
    commands.insert_resource(support);
    commands.insert_resource(EpochBuffer(device.create_buffer(&BufferDescriptor {
        label: Some("meteor capture epoch"),
        size: 4,
        usage: BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })));
}

fn record_epoch(
    stamp: Res<ExtractedCapture>,
    support: Res<Support>,
    buffer: Res<EpochBuffer>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    recorder: Res<bevy::render::diagnostic::DiagnosticsRecorder>,
    mut pending: ResMut<PendingCommandBuffers>,
) {
    if !support.can_tag() {
        return;
    }
    queue.write_buffer(&buffer.0, 0, &stamp.epoch.to_le_bytes());
    let mut encoder = device.create_command_encoder(&default());
    recorder.record_u32(&mut encoder, &buffer.0.slice(..), "meteor_capture_epoch");
    pending.push_encoder(encoder);
}

fn count_prepared_draws(
    phases: Res<
        bevy::render::render_phase::ViewSortedRenderPhases<
            bevy::core_pipeline::core_3d::Transparent3d,
        >,
    >,
    stamp: Res<ExtractedCapture>,
    shared: Res<SharedCapture>,
    views: Query<(
        Entity,
        &bevy::render::view::ExtractedView,
        &bevy::render::view::ViewTarget,
        Option<&bevy::render::view::ViewDepthTexture>,
    )>,
) {
    let mut capture = shared.0.lock().unwrap();
    if !stamp.active || !capture.active || stamp.epoch != capture.epoch {
        return;
    }
    capture.draws.frames += 1;
    capture.targets.clear();
    for (entity, view, target, depth) in &views {
        let color = target.main_texture();
        capture.targets.push(TargetDimensions {
            view: entity.to_bits(),
            viewport: view.viewport.to_array(),
            color: [color.width(), color.height()],
            msaa_color: target
                .sampled_main_texture()
                .map(|t| [t.width(), t.height()]),
            depth: depth.map(|d| [d.texture.width(), d.texture.height()]),
            samples: target
                .sampled_main_texture()
                .map_or(1, |t| t.sample_count()),
        });
    }

    for phase in phases.values() {
        let mut index = 0;
        while let Some((_, item)) = phase.items.get_index(index) {
            let n = item.batch_range.len();
            if n == 0 {
                index += 1;
                continue;
            }
            capture.draws.transparent_instances += n as u64;
            capture.draws.transparent_batches += 1;
            capture.draws.largest_batch = capture.draws.largest_batch.max(n);
            index += n;
        }
    }
}

fn begin_render(stamp: Res<ExtractedCapture>, mut clock: ResMut<RenderClock>) {
    clock.start = stamp.active.then(|| (stamp.epoch, Instant::now()));
    clock.submit = None;
}

fn begin_submit(mut clock: ResMut<RenderClock>, shared: Res<SharedCapture>) {
    if let Some((epoch, start)) = clock.start {
        let now = Instant::now();
        shared
            .0
            .lock()
            .unwrap()
            .record(epoch, "render_prepare_wall", now.duration_since(start));
        clock.submit = Some(now);
    }
}

fn end_submit(clock: Res<RenderClock>, shared: Res<SharedCapture>) {
    if let (Some((epoch, _)), Some(start)) = (clock.start, clock.submit) {
        shared
            .0
            .lock()
            .unwrap()
            .record(epoch, "render_submit_present_wall", start.elapsed());
    }
}

fn end_render(mut clock: ResMut<RenderClock>, shared: Res<SharedCapture>) {
    if let Some((epoch, start)) = clock.start.take() {
        shared
            .0
            .lock()
            .unwrap()
            .record(epoch, "render_schedule_wall", start.elapsed());
    }
}

fn report(
    shared: Res<SharedCapture>,
    gpu: Res<gpu::GpuResults>,
    store: Res<DiagnosticsStore>,
    device: Option<Res<RenderDevice>>,
    adapter: Option<Res<RenderAdapterInfo>>,
    queue: Option<Res<RenderQueue>>,
    mut passes: Local<PassWindow>,
    mut support: Local<Support>,
) {
    if let (Some(device), Some(adapter), Some(queue)) = (device, adapter, queue) {
        if support.backend.is_empty() || device.is_changed() {
            *support = Support::new(&device, &queue, &adapter);
        }
    }
    let (epoch, active) = {
        let mut capture = shared.0.lock().unwrap();
        if let Some((epoch, start)) = capture.main_start.take() {
            capture.record(epoch, "main_first_last_wall", start.elapsed());
        }
        (capture.epoch, capture.active)
    };
    passes.collect(&store, epoch, active, &support);
    let (stage, window_ms, cpu, draws, targets) = {
        let mut capture = shared.0.lock().unwrap();
        let Some(start) = capture.window_start else {
            return;
        };
        if start.elapsed() < Duration::from_secs(5) {
            return;
        }
        let window_ms = start.elapsed().as_secs_f64() * 1000.0;
        capture.window_start = Some(Instant::now());
        (
            capture.stage.clone(),
            window_ms,
            std::mem::take(&mut capture.cpu),
            std::mem::take(&mut capture.draws),
            capture.targets.clone(),
        )
    };
    let gpu = gpu.take();
    let gpu_status = if !support.encoder_timestamps {
        "unsupported"
    } else if gpu["spans"]
        .as_object()
        .is_none_or(|spans| spans.is_empty())
    {
        "no_samples"
    } else {
        "available"
    };
    info!(target: "meteor::perf", "render_perf_v2 {}", serde_json::json!({
        "stage": stage, "epoch": epoch, "window_ms": window_ms,
        "gpu_status": gpu_status, "gpu_scope": "encoder_boundaries_inclusive_elapsed", "pass_sample_unit": "invocation", "support": *support,
        "cpu_wall": cpu, "passes": passes.samples, "prepared_draws": draws,
        "target_dimensions_last_frame": targets,
        "gpu": gpu, "pass_gpu_filter": "complete_pass_encoder_only",
        "discarded_readbacks": passes.discarded, "gpu_window": "readback_arrival", "inclusive_scopes": true,
    }));
    passes.samples.clear();
    passes.discarded = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::diagnostic::{Diagnostic, DiagnosticMeasurement};

    fn add(store: &mut DiagnosticsStore, path: &str, time: DiagnosticInstant, value: f64) {
        let path = DiagnosticPath::new(path.to_owned());
        if store.get(&path).is_none() {
            store.add(Diagnostic::new(path.clone()));
        }
        store
            .get_mut(&path)
            .unwrap()
            .add_measurement(DiagnosticMeasurement { time, value });
    }

    #[test]
    fn gpu_windows_keep_multiple_views_without_recounting_or_old_session_readbacks() {
        let support = Support {
            backend: "Vulkan".into(),
            encoder_timestamps: true,
            pass_timestamps: true,
            ..default()
        };
        let path = "render/upscaling/elapsed_gpu";
        let mut store = DiagnosticsStore::default();
        let mut window = PassWindow::default();
        let t = DiagnosticInstant::now();
        add(&mut store, EPOCH_PATH, t, 1.0);

        add(&mut store, path, t, 0.2);
        add(&mut store, path, t, 0.4);
        window.collect(&store, 1, true, &support);
        assert_eq!(window.samples[path].samples, 2);
        assert!((window.samples[path].sum_ms - 0.6).abs() < 1e-9);
        window.collect(&store, 1, true, &support);
        assert_eq!(window.samples[path].samples, 2);
        window.samples.clear();
        window.collect(&store, 1, true, &support);
        assert!(
            window.samples.is_empty(),
            "a fresh window must not repeat old GPU values"
        );
        add(&mut store, EPOCH_PATH, t + Duration::from_millis(1), 1.0);
        add(&mut store, path, t + Duration::from_millis(1), 99.0);
        window.collect(&store, 2, true, &support);
        assert!(
            window.samples.is_empty(),
            "late old-map/pause readbacks must be rejected"
        );
        add(&mut store, EPOCH_PATH, t + Duration::from_millis(2), 2.0);
        add(&mut store, path, t + Duration::from_millis(2), 0.0);
        window.collect(&store, 2, true, &support);
        assert_eq!(
            window.samples[path].samples, 1,
            "a measured zero differs from no sample"
        );
        assert_eq!(window.samples[path].mean_ms, 0.0);
        assert_eq!(window.discarded, 1);
    }

    #[test]
    fn pass_internal_gpu_values_are_rejected_even_when_device_supports_them() {
        let support = Support {
            backend: "Vulkan".into(),
            encoder_timestamps: true,
            pass_timestamps: true,
            ..default()
        };
        let mut store = DiagnosticsStore::default();
        let mut window = PassWindow::default();
        let t = DiagnosticInstant::now();
        add(&mut store, EPOCH_PATH, t, 1.0);
        add(
            &mut store,
            "render/main_opaque_pass_3d/elapsed_gpu",
            t,
            0.003,
        );
        add(&mut store, "render/ui/elapsed_gpu", t, 0.5);
        add(&mut store, "render/ui/elapsed_cpu", t, 0.1);
        add(
            &mut store,
            "render/meteor_display_post_process/elapsed_gpu",
            t,
            1.2,
        );
        window.collect(&store, 1, true, &support);
        assert!(!window
            .samples
            .contains_key("render/main_opaque_pass_3d/elapsed_gpu"));
        assert!(!window.samples.contains_key("render/ui/elapsed_gpu"));
        assert_eq!(window.samples["render/ui/elapsed_cpu"].samples, 1);
        assert_eq!(
            window.samples["render/meteor_display_post_process/elapsed_gpu"].samples,
            1
        );
    }

    #[test]
    fn unsupported_gpu_and_inactive_capture_never_report_fake_zero_timings() {
        let mut store = DiagnosticsStore::default();
        let mut window = PassWindow::default();
        let t = DiagnosticInstant::now();
        add(&mut store, EPOCH_PATH, t, 1.0);
        add(&mut store, "render/ui/elapsed_gpu", t, 0.0);
        add(&mut store, "render/ui/elapsed_cpu", t, 0.3);
        window.collect(&store, 1, true, &Support::default());
        assert!(!window.samples.contains_key("render/ui/elapsed_gpu"));
        assert_eq!(window.samples["render/ui/elapsed_cpu"].samples, 1);
        window.collect(&store, 2, false, &Support::default());
        assert!(window.samples.is_empty());
        let mut capture = Capture::default();
        capture.enter(true, "sn06");
        let old = capture.epoch;
        capture.enter(false, "sn06");
        capture.enter(true, "sn03");
        capture.record(old, "render_schedule_wall", Duration::from_secs(1));
        assert!(
            capture.cpu.is_empty(),
            "an old render frame cannot leak into the next stage"
        );
    }

    #[test]
    fn handoff_probe_delegates_to_the_real_pipelined_renderer_without_changing_order() {
        use bevy::app::SubApp;
        use bevy::ecs::schedule::ScheduleLabel;
        use bevy::render::pipelined_rendering::{PipelinedRenderingPlugin, RenderAppChannels};
        #[derive(ScheduleLabel, Debug, Clone, Hash, Eq, PartialEq)]
        struct FixtureRender;
        #[derive(Resource, Clone, Copy)]
        struct Value(u32);
        #[derive(Resource, Clone, Default)]
        struct Seen(Arc<Mutex<Vec<(u32, std::thread::ThreadId)>>>);
        let mut app = App::new();
        app.add_plugins(bevy::app::TaskPoolPlugin::default());
        let shared = SharedCapture::default();
        shared.0.lock().unwrap().enter(true, "sn06");
        app.insert_resource(shared.clone())
            .insert_resource(Value(0));
        let seen = Seen::default();
        let mut render = SubApp::new();
        render.update_schedule = Some(FixtureRender.intern());
        render
            .insert_resource(seen.clone())
            .set_extract(|main, render| {
                render.insert_resource(*main.resource::<Value>());
            });

        render.add_systems(FixtureRender, |world: &mut World| {
            std::thread::sleep(Duration::from_millis(2));
            world
                .resource::<Seen>()
                .0
                .lock()
                .unwrap()
                .push((world.resource::<Value>().0, std::thread::current().id()));
        });
        app.insert_sub_app(RenderApp, render);
        app.add_plugins(PipelinedRenderingPlugin);
        wrap_extraction(&mut app);
        app.finish();
        app.cleanup();
        for frame in 1..=8 {
            app.world_mut().resource_mut::<Value>().0 = frame;
            app.update();
        }

        app.world_mut().remove_resource::<RenderAppChannels>();
        let seen = seen.0.lock().unwrap();
        assert_eq!(
            seen.iter().map(|v| v.0).collect::<Vec<_>>(),
            (1..=8).collect::<Vec<_>>()
        );
        assert!(seen.iter().all(|v| v.1 != std::thread::current().id()));
        let capture = shared.0.lock().unwrap();
        for key in [
            "handoff_total_wall",
            "main_wait_render_wall",
            "extract_sync_wall",
        ] {
            assert_eq!(capture.cpu[key].samples, 8);
        }
        assert!(
            capture.cpu["handoff_total_wall"].sum_ms >= capture.cpu["extract_sync_wall"].sum_ms
        );
        assert!(capture.handoff.is_none());
    }
}
