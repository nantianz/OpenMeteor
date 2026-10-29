

mod erased_render_asset_diagnostic_plugin;
pub(crate) mod internal;
mod mesh_allocator_diagnostic_plugin;
mod render_asset_diagnostic_plugin;
#[cfg(feature = "tracing-tracy")]
mod tracy_gpu;

use alloc::{borrow::Cow, sync::Arc};
use bevy_ecs::{
    schedule::IntoScheduleConfigs,
    system::{Res, ResMut},
    world::{FromWorld, World},
};
use core::marker::PhantomData;
use wgpu::{BufferSlice, CommandEncoder};

use bevy_app::{App, Plugin, PreUpdate};

use crate::{
    renderer::{PendingCommandBuffers, RenderGraph, RenderGraphSystems},
    GpuResourceAppExt, RenderApp,
};

use self::internal::{sync_diagnostics, Pass, RenderDiagnosticsMutex, WriteTimestamp};
pub use self::{
    erased_render_asset_diagnostic_plugin::ErasedRenderAssetDiagnosticPlugin,
    internal::DiagnosticsRecorder, mesh_allocator_diagnostic_plugin::MeshAllocatorDiagnosticPlugin,
    render_asset_diagnostic_plugin::RenderAssetDiagnosticPlugin,
};

use crate::renderer::RenderDevice;

#[derive(Default)]
pub struct RenderDiagnosticsPlugin;

impl Plugin for RenderDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        let render_diagnostics_mutex = RenderDiagnosticsMutex::default();
        app.insert_resource(render_diagnostics_mutex.clone())
            .add_systems(PreUpdate, sync_diagnostics);

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.insert_resource(render_diagnostics_mutex);
        }
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app.init_gpu_resource::<DiagnosticsRecorder>();

        render_app.add_systems(
            RenderGraph,
            (
                begin_diagnostics_frame.in_set(RenderGraphSystems::Begin),
                resolve_encoder
                    .after(RenderGraphSystems::Render)
                    .before(RenderGraphSystems::Submit),
                finish_diagnostics_frame.in_set(RenderGraphSystems::Finish),
            ),
        );
    }
}

impl FromWorld for DiagnosticsRecorder {
    fn from_world(world: &mut World) -> Self {
        DiagnosticsRecorder::new(world.resource(), world.resource(), world.resource())
    }
}

pub fn begin_diagnostics_frame(mut recorder: ResMut<DiagnosticsRecorder>) {
    recorder.begin_frame();
}

pub fn resolve_encoder(
    mut recorder: ResMut<DiagnosticsRecorder>,
    render_device: Res<RenderDevice>,
    mut pending_buffers: ResMut<PendingCommandBuffers>,
) {
    let mut encoder =
        render_device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    recorder.resolve(&mut encoder);
    pending_buffers.push_encoder(encoder);
}

fn finish_diagnostics_frame(
    mut recorder: ResMut<DiagnosticsRecorder>,
    render_device: Res<RenderDevice>,
    mutex: Res<RenderDiagnosticsMutex>,
) {
    let mutex = mutex.0.clone();
    recorder.finish_frame(&render_device, move |diagnostics| {
        *mutex.lock().unwrap() = Some(diagnostics);
    });
}

pub trait RecordDiagnostics: Send + Sync {

    fn time_span<E, N>(&self, encoder: &mut E, name: N) -> TimeSpanGuard<'_, Self, E>
    where
        E: WriteTimestamp,
        N: Into<Cow<'static, str>>,
    {
        self.begin_time_span(encoder, name.into());
        TimeSpanGuard {
            recorder: self,
            marker: PhantomData,
        }
    }

    fn pass_span<P, N>(&self, pass: &mut P, name: N) -> PassSpanGuard<'_, Self, P>
    where
        P: Pass,
        N: Into<Cow<'static, str>>,
    {
        let name = name.into();
        self.begin_pass_span(pass, name.clone());
        PassSpanGuard {
            recorder: self,
            name,
            marker: PhantomData,
        }
    }

    fn record_f32<N>(&self, command_encoder: &mut CommandEncoder, buffer: &BufferSlice, name: N)
    where
        N: Into<Cow<'static, str>>;

    fn record_u32<N>(&self, command_encoder: &mut CommandEncoder, buffer: &BufferSlice, name: N)
    where
        N: Into<Cow<'static, str>>;

    #[doc(hidden)]
    fn begin_time_span<E: WriteTimestamp>(&self, encoder: &mut E, name: Cow<'static, str>);

    #[doc(hidden)]
    fn end_time_span<E: WriteTimestamp>(&self, encoder: &mut E);

    #[doc(hidden)]
    fn begin_pass_span<P: Pass>(&self, pass: &mut P, name: Cow<'static, str>);

    #[doc(hidden)]
    fn end_pass_span<P: Pass>(&self, pass: &mut P);
}

pub struct TimeSpanGuard<'a, R: ?Sized, E> {
    recorder: &'a R,
    marker: PhantomData<E>,
}

impl<R: RecordDiagnostics + ?Sized, E: WriteTimestamp> TimeSpanGuard<'_, R, E> {

    pub fn end(self, encoder: &mut E) {
        self.recorder.end_time_span(encoder);
        core::mem::forget(self);
    }
}

impl<R: ?Sized, E> Drop for TimeSpanGuard<'_, R, E> {
    fn drop(&mut self) {
        bevy_log::error!("TimeSpanScope::end was never called");
    }
}

pub struct PassSpanGuard<'a, R: ?Sized, P> {
    recorder: &'a R,
    name: Cow<'static, str>,
    marker: PhantomData<P>,
}

impl<R: RecordDiagnostics + ?Sized, P: Pass> PassSpanGuard<'_, R, P> {

    pub fn end(self, pass: &mut P) {
        self.recorder.end_pass_span(pass);
        core::mem::forget(self);
    }
}

impl<R: ?Sized, P> Drop for PassSpanGuard<'_, R, P> {
    fn drop(&mut self) {
        panic!("PassSpanGuard::end was never called for {}", self.name)
    }
}

impl<T: RecordDiagnostics> RecordDiagnostics for Option<Arc<T>> {
    fn record_f32<N>(&self, command_encoder: &mut CommandEncoder, buffer: &BufferSlice, name: N)
    where
        N: Into<Cow<'static, str>>,
    {
        if let Some(recorder) = &self {
            recorder.record_f32(command_encoder, buffer, name);
        }
    }

    fn record_u32<N>(&self, command_encoder: &mut CommandEncoder, buffer: &BufferSlice, name: N)
    where
        N: Into<Cow<'static, str>>,
    {
        if let Some(recorder) = &self {
            recorder.record_u32(command_encoder, buffer, name);
        }
    }

    fn begin_time_span<E: WriteTimestamp>(&self, encoder: &mut E, name: Cow<'static, str>) {
        if let Some(recorder) = &self {
            recorder.begin_time_span(encoder, name);
        }
    }

    fn end_time_span<E: WriteTimestamp>(&self, encoder: &mut E) {
        if let Some(recorder) = &self {
            recorder.end_time_span(encoder);
        }
    }

    fn begin_pass_span<P: Pass>(&self, pass: &mut P, name: Cow<'static, str>) {
        if let Some(recorder) = &self {
            recorder.begin_pass_span(pass, name);
        }
    }

    fn end_pass_span<P: Pass>(&self, pass: &mut P) {
        if let Some(recorder) = &self {
            recorder.end_pass_span(pass);
        }
    }
}

impl<'a, T: RecordDiagnostics> RecordDiagnostics for Option<&'a T> {
    fn record_f32<N>(&self, command_encoder: &mut CommandEncoder, buffer: &BufferSlice, name: N)
    where
        N: Into<Cow<'static, str>>,
    {
        if let Some(recorder) = self {
            recorder.record_f32(command_encoder, buffer, name);
        }
    }

    fn record_u32<N>(&self, command_encoder: &mut CommandEncoder, buffer: &BufferSlice, name: N)
    where
        N: Into<Cow<'static, str>>,
    {
        if let Some(recorder) = self {
            recorder.record_u32(command_encoder, buffer, name);
        }
    }

    fn begin_time_span<E: WriteTimestamp>(&self, encoder: &mut E, name: Cow<'static, str>) {
        if let Some(recorder) = self {
            recorder.begin_time_span(encoder, name);
        }
    }

    fn end_time_span<E: WriteTimestamp>(&self, encoder: &mut E) {
        if let Some(recorder) = self {
            recorder.end_time_span(encoder);
        }
    }

    fn begin_pass_span<P: Pass>(&self, pass: &mut P, name: Cow<'static, str>) {
        if let Some(recorder) = self {
            recorder.begin_pass_span(pass, name);
        }
    }

    fn end_pass_span<P: Pass>(&self, pass: &mut P) {
        if let Some(recorder) = self {
            recorder.end_pass_span(pass);
        }
    }
}
