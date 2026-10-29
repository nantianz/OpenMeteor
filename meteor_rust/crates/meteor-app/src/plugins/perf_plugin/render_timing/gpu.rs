use super::*;
use bevy::core_pipeline::{
    core_3d::{main_opaque_pass_3d, main_transparent_pass_3d},
    schedule::{Core2d, Core3d},
    upscaling::upscaling,
    Core2dSystems, Core3dSystems,
};
use bevy::render::{
    render_resource::MapMode,
    renderer::{CurrentView, RenderContext},
};
use std::sync::atomic::{AtomicBool, Ordering};
use wgpu::{QuerySet, QuerySetDescriptor, QueryType};

const QUERY_COUNT: u32 = 128;
const SLOT_COUNT: usize = 4;

#[derive(Clone)]
struct Span {
    name: String,
    start: u32,
    end: u32,
    closed: bool,
}

struct Slot {
    queries: QuerySet,
    resolve: Buffer,
    readback: Buffer,
    busy: Arc<AtomicBool>,
}

struct Frame {
    slot: usize,
    epoch: u32,
    spans: Vec<Span>,
}

#[derive(Resource, Default)]
struct GpuTimer {
    slots: Vec<Slot>,
    frame: Option<Frame>,
}

#[derive(Default)]
struct Results {
    epoch: u32,
    active: bool,
    samples: BTreeMap<String, Distribution>,
    discarded_frames: u64,
    skipped_frames: u64,
    failed_frames: u64,
}

impl Results {
    fn accept(&mut self, epoch: u32, spans: Vec<Span>, ticks: &[u64], period: f32) {
        if !self.active || self.epoch != epoch {
            self.discarded_frames += 1;
            return;
        }
        let values: Option<Vec<_>> = spans
            .into_iter()
            .map(|span| {
                Some((
                    span.name,
                    elapsed_ms(
                        *ticks.get(span.start as usize)?,
                        *ticks.get(span.end as usize)?,
                        period,
                    )?,
                ))
            })
            .collect();
        if let Some(values) = values {
            for (name, ms) in values {
                self.samples.entry(name).or_default().add(ms);
            }
        } else {
            self.failed_frames += 1;
        }
    }
}

#[derive(Resource, Clone, Default)]
pub(super) struct GpuResults(Arc<Mutex<Results>>);

impl GpuResults {
    pub(super) fn enter(&self, epoch: u32, active: bool) {
        let mut results = self.0.lock().unwrap();
        if results.epoch != epoch || results.active != active {
            *results = Results {
                epoch,
                active,
                ..default()
            };
        }
    }

    pub(super) fn take(&self) -> serde_json::Value {
        let mut results = self.0.lock().unwrap();
        serde_json::json!({
            "spans": std::mem::take(&mut results.samples),
            "discarded_frames": std::mem::take(&mut results.discarded_frames),
            "skipped_frames": std::mem::take(&mut results.skipped_frames),
            "failed_frames": std::mem::take(&mut results.failed_frames),
        })
    }
}

pub(super) fn install(app: &mut App) {
    let results = GpuResults::default();
    app.insert_resource(results.clone());
    let render = app.sub_app_mut(RenderApp);
    render
        .insert_resource(results)
        .init_resource::<GpuTimer>()
        .add_systems(RenderStartup, setup.after(super::setup_support))
        .add_systems(
            RenderGraph,
            (
                begin_frame
                    .after(bevy::render::diagnostic::begin_diagnostics_frame)
                    .before(RenderGraphSystems::Render),
                end_frame
                    .after(RenderGraphSystems::Render)
                    .before(bevy::render::diagnostic::resolve_encoder)
                    .before(RenderGraphSystems::Submit),
            ),
        )
        .add_systems(
            Core3d,
            (
                begin_scene
                    .in_set(Core3dSystems::MainPass)
                    .before(main_opaque_pass_3d),
                end_scene
                    .in_set(Core3dSystems::MainPass)
                    .after(main_transparent_pass_3d),
                begin_ui
                    .after(Core3dSystems::PostProcess)
                    .before(bevy::ui_render::ui_pass),
                end_ui.after(bevy::ui_render::ui_pass).before(upscaling),
            ),
        )
        .add_systems(
            Core2d,
            (
                begin_ui
                    .after(Core2dSystems::PostProcess)
                    .before(bevy::ui_render::ui_pass),
                end_ui.after(bevy::ui_render::ui_pass).before(upscaling),
            ),
        );
}

fn setup(mut timer: ResMut<GpuTimer>, support: Res<Support>, device: Res<RenderDevice>) {
    if !support.encoder_timestamps {
        return;
    }
    timer.slots = (0..SLOT_COUNT)
        .map(|_| Slot {
            queries: device.wgpu_device().create_query_set(&QuerySetDescriptor {
                label: Some("meteor complete-pass timestamps"),
                ty: QueryType::Timestamp,
                count: QUERY_COUNT,
            }),
            resolve: device.create_buffer(&BufferDescriptor {
                label: Some("meteor timestamp resolve"),
                size: QUERY_COUNT as u64 * 8,
                usage: BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&BufferDescriptor {
                label: Some("meteor timestamp readback"),
                size: QUERY_COUNT as u64 * 8,
                usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            busy: Arc::new(AtomicBool::new(false)),
        })
        .collect();
}

impl GpuTimer {
    fn begin(&mut self, name: String, ctx: &mut RenderContext) {
        let Some(frame) = &mut self.frame else {
            return;
        };
        let start = frame.spans.len() as u32 * 2;
        if start + 1 >= QUERY_COUNT {
            return;
        }
        ctx.command_encoder()
            .write_timestamp(&self.slots[frame.slot].queries, start);
        frame.spans.push(Span {
            name,
            start,
            end: start + 1,
            closed: false,
        });
    }

    fn end(&mut self, name: &str, ctx: &mut RenderContext) {
        let Some(frame) = &mut self.frame else {
            return;
        };
        let Some(span) = frame
            .spans
            .iter_mut()
            .rev()
            .find(|s| s.name == name && !s.closed)
        else {
            return;
        };
        ctx.command_encoder()
            .write_timestamp(&self.slots[frame.slot].queries, span.end);
        span.closed = true;
    }
}

fn begin_frame(
    stamp: Res<ExtractedCapture>,
    mut timer: ResMut<GpuTimer>,
    results: Res<GpuResults>,
    mut ctx: RenderContext,
) {
    debug_assert!(timer.frame.is_none());
    if !stamp.active || timer.slots.is_empty() {
        return;
    }
    let Some(slot) = timer
        .slots
        .iter()
        .position(|s| !s.busy.load(Ordering::Acquire))
    else {
        let mut results = results.0.lock().unwrap();
        if results.epoch == stamp.epoch {
            results.skipped_frames += 1;
        }
        return;
    };
    timer.slots[slot].busy.store(true, Ordering::Release);
    timer.frame = Some(Frame {
        slot,
        epoch: stamp.epoch,
        spans: Vec::with_capacity(8),
    });
    timer.begin("frame_render".into(), &mut ctx);
}

fn begin_scene(view: Res<CurrentView>, mut timer: ResMut<GpuTimer>, mut ctx: RenderContext) {
    timer.begin(format!("scene_main/view_{}", view.0.to_bits()), &mut ctx);
}
fn end_scene(view: Res<CurrentView>, mut timer: ResMut<GpuTimer>, mut ctx: RenderContext) {
    timer.end(&format!("scene_main/view_{}", view.0.to_bits()), &mut ctx);
}
fn begin_ui(view: Res<CurrentView>, mut timer: ResMut<GpuTimer>, mut ctx: RenderContext) {
    timer.begin(format!("ui/view_{}", view.0.to_bits()), &mut ctx);
}
fn end_ui(view: Res<CurrentView>, mut timer: ResMut<GpuTimer>, mut ctx: RenderContext) {
    timer.end(&format!("ui/view_{}", view.0.to_bits()), &mut ctx);
}

fn elapsed_ms(start: u64, end: u64, period: f32) -> Option<f64> {
    let ticks = end.checked_sub(start)?;
    let ms = ticks as f64 * period as f64 / 1e6;
    (ms.is_finite() && period > 0.0).then_some(ms)
}

fn end_frame(
    mut timer: ResMut<GpuTimer>,
    results: Res<GpuResults>,
    support: Res<Support>,
    mut ctx: RenderContext,
) {
    timer.end("frame_render", &mut ctx);
    let Some(frame) = timer.frame.take() else {
        return;
    };
    let slot = &timer.slots[frame.slot];

    assert!(
        frame.spans.iter().all(|s| s.closed),
        "unclosed GPU pass boundary"
    );
    let count = frame.spans.len() as u32 * 2;
    let encoder = ctx.command_encoder();
    encoder.resolve_query_set(&slot.queries, 0..count, &slot.resolve, 0);
    encoder.copy_buffer_to_buffer(&slot.resolve, 0, &slot.readback, 0, count as u64 * 8);
    let readback = slot.readback.clone();
    let busy = slot.busy.clone();
    let results = results.clone();
    let period = support.timestamp_period_ns;
    encoder.map_buffer_on_submit(
        &slot.readback,
        MapMode::Read,
        0..count as u64 * 8,
        move |result| {
            let mut results = results.0.lock().unwrap();
            if result.is_err() {
                if results.epoch == frame.epoch {
                    results.failed_frames += 1;
                }
            } else {
                {
                    let data = readback.slice(0..count as u64 * 8).get_mapped_range();
                    if results.active && results.epoch == frame.epoch {
                        let ticks: Vec<u64> = data
                            .chunks_exact(8)
                            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                            .collect();
                        results.accept(frame.epoch, frame.spans, &ticks, period);
                    } else {
                        results.discarded_frames += 1;
                    }
                }
                readback.unmap();
            }
            busy.store(false, Ordering::Release);
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delayed_gpu_results_never_leak_across_pause_or_stage_change() {
        let results = GpuResults::default();
        let spans = || {
            vec![Span {
                name: "frame_render".into(),
                start: 0,
                end: 1,
                closed: true,
            }]
        };
        results.enter(1, true);
        results
            .0
            .lock()
            .unwrap()
            .accept(1, spans(), &[100, 200], 50.0);
        assert_eq!(results.take()["spans"]["frame_render"]["samples"], 1);
        assert!(results.take()["spans"].as_object().unwrap().is_empty());
        results.enter(2, false);
        results
            .0
            .lock()
            .unwrap()
            .accept(1, spans(), &[100, 200], 50.0);
        assert!(results.take()["spans"].as_object().unwrap().is_empty());
        results.enter(3, true);
        results
            .0
            .lock()
            .unwrap()
            .accept(1, spans(), &[100, 200], 50.0);
        results
            .0
            .lock()
            .unwrap()
            .accept(3, spans(), &[200, 100], 50.0);
        let window = results.take();
        assert_eq!(window["discarded_frames"], 1);
        assert_eq!(window["failed_frames"], 1);
        assert!(window["spans"].as_object().unwrap().is_empty());
    }

    #[test]
    fn timestamp_subtraction_keeps_low_bits_and_rejects_invalid_intervals() {
        let base = (1u64 << 60) + 17;
        assert_eq!(elapsed_ms(base, base + 70, 50.0), Some(0.0035));
        assert_eq!(elapsed_ms(10, 9, 50.0), None);
        assert_eq!(elapsed_ms(1, 2, f32::NAN), None);
    }
}
