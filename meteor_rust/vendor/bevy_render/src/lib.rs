

#![expect(missing_docs, reason = "Not all docs are written yet, see #3492.")]
#![expect(unsafe_code, reason = "Unsafe code is used to improve performance.")]
#![cfg_attr(
    any(docsrs, docsrs_dep),
    expect(
        internal_features,
        reason = "rustdoc_internals is needed for fake_variadic"
    )
)]
#![cfg_attr(any(docsrs, docsrs_dep), feature(rustdoc_internals))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(
    html_logo_url = "https://bevy.org/assets/icon.png",
    html_favicon_url = "https://bevy.org/assets/icon.png"
)]

#[cfg(target_pointer_width = "16")]
compile_error!("bevy_render cannot compile for a 16-bit platform.");

extern crate alloc;
extern crate core;

extern crate self as bevy_render;

pub mod batching;
pub mod camera;
pub mod diagnostic;
pub mod erased_render_asset;
pub mod error_handler;
pub mod extract_component;
pub mod extract_instances;
mod extract_param;
pub mod extract_plugin;
pub mod extract_resource;
pub mod globals;
pub mod gpu_component_array_buffer;
pub mod gpu_readback;
pub mod mesh;
pub mod occlusion_culling;
#[cfg(not(target_arch = "wasm32"))]
pub mod pipelined_rendering;
pub mod render_asset;
pub mod render_phase;
pub mod render_resource;
pub mod renderer;
pub mod settings;
pub mod slab_allocator;
pub mod storage;
pub mod sync_component;
pub mod sync_world;
#[cfg(test)]
pub(crate) mod test_utils;
pub mod texture;
pub mod uniform;
pub mod view;

pub mod prelude {
    #[doc(hidden)]
    pub use crate::{
        camera::NormalizedRenderTargetExt as _, renderer::RenderGraph, texture::ManualTextureViews,
        view::Msaa, ExtractSchedule,
    };
}

pub use extract_param::Extract;
pub use extract_plugin::{ExtractSchedule, MainWorld};

use crate::{
    camera::CameraPlugin,
    error_handler::{RenderErrorHandler, RenderState},
    extract_plugin::ExtractPlugin,
    gpu_readback::GpuReadbackPlugin,
    mesh::{MeshRenderAssetPlugin, RenderMesh},
    render_asset::prepare_assets,
    render_resource::{PipelineCache, SparseBufferPlugin},
    renderer::{render_system, RenderAdapterInfo, RenderGraph},
    settings::{RenderCreation, WgpuLimits},
    storage::StoragePlugin,
    texture::TexturePlugin,
    view::{ViewPlugin, WindowRenderPlugin},
};
use alloc::sync::Arc;
use batching::gpu_preprocessing::BatchingPlugin;
use bevy_app::{App, AppLabel, First, Plugin, SubApp};
use bevy_asset::{AssetApp, AssetServer};
use bevy_derive::Deref;
use bevy_ecs::{
    prelude::*,
    schedule::{InternedScheduleLabel, ScheduleLabel},
};
use bevy_platform::time::Instant;
use bevy_shader::{load_shader_library, Shader, ShaderLoader};
use bevy_time::TimeSender;
use bevy_window::{PrimaryWindow, RawHandleWrapperHolder};
use bitflags::bitflags;
use globals::GlobalsPlugin;
use occlusion_culling::OcclusionCullingPlugin;
use render_asset::{
    extract_render_asset_bytes_per_frame, reset_render_asset_bytes_per_frame,
    RenderAssetBytesPerFrame, RenderAssetBytesPerFrameLimiter,
};
use settings::RenderResources;
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
pub struct RenderPlugin {
    pub render_creation: RenderCreation,

    pub synchronous_pipeline_compilation: bool,

    pub debug_flags: RenderDebugFlags,
}

bitflags! {

    #[derive(Clone, Copy, PartialEq, Default, Debug)]
    pub struct RenderDebugFlags: u8 {

        const ALLOW_COPIES_FROM_INDIRECT_PARAMETERS = 1;
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, SystemSet)]
pub enum RenderSystems {

    ExtractCommands,

    PrepareAssets,

    PrepareMeshes,

    CreateViews,

    Specialize,

    PrepareViews,

    Queue,

    QueueMeshes,

    QueueSweep,

    PhaseSort,

    Prepare,

    PrepareResources,

    PrepareResourcesBatchPhases,

    PrepareResourcesWritePhaseBuffers,

    PrepareResourcesCollectPhaseBuffers,

    PrepareResourcesFlush,

    PrepareBindGroups,

    Render,

    Cleanup,

    PostCleanup,
}

#[derive(ScheduleLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct RenderStartup;

pub fn init_gpu_resource<R: Resource + FromWorld>(world: &mut World) {
    let res = R::from_world(world);
    world.insert_resource(res);
}

pub trait GpuResourceAppExt {

    fn init_gpu_resource<R: Resource + FromWorld>(&mut self) -> &mut Self;
}

impl GpuResourceAppExt for SubApp {
    fn init_gpu_resource<R: Resource + FromWorld>(&mut self) -> &mut Self {
        self.add_systems(RenderStartup, init_gpu_resource::<R>.ambiguous_with_all())
    }
}

#[derive(ScheduleLabel, Debug, Hash, PartialEq, Eq, Clone)]
struct RenderRecovery;

#[derive(Resource, Debug)]
pub struct RenderScheduleOrder {

    pub labels: Vec<InternedScheduleLabel>,
}

impl Default for RenderScheduleOrder {
    fn default() -> Self {
        Self {
            labels: vec![First.intern(), Render.intern()],
        }
    }
}

impl RenderScheduleOrder {

    pub fn insert_after(&mut self, after: impl ScheduleLabel, schedule: impl ScheduleLabel) {
        let index = self
            .labels
            .iter()
            .position(|current| (**current).eq(&after))
            .unwrap_or_else(|| panic!("Expected {after:?} to exist"));
        self.labels.insert(index + 1, schedule.intern());
    }

    pub fn insert_before(&mut self, before: impl ScheduleLabel, schedule: impl ScheduleLabel) {
        let index = self
            .labels
            .iter()
            .position(|current| (**current).eq(&before))
            .unwrap_or_else(|| panic!("Expected {before:?} to exist"));
        self.labels.insert(index, schedule.intern());
    }
}

#[derive(ScheduleLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct Render;

impl Render {

    pub fn base_schedule() -> Schedule {
        use RenderSystems::*;

        let mut schedule = Schedule::new(Self);

        schedule.configure_sets(
            (
                ExtractCommands,
                PrepareMeshes,
                CreateViews,
                Specialize,
                PrepareViews,
                Queue,
                PhaseSort,
                Prepare,
                Render,
                Cleanup,
                PostCleanup,
            )
                .chain(),
        );
        schedule.ignore_ambiguity(Specialize, Specialize);

        schedule.configure_sets((ExtractCommands, PrepareAssets, PrepareMeshes, Prepare).chain());
        schedule.configure_sets(
            (QueueMeshes, QueueSweep)
                .chain()
                .in_set(Queue)
                .after(prepare_assets::<RenderMesh>),
        );
        schedule.configure_sets(
            (
                PrepareResources,
                PrepareResourcesBatchPhases,
                PrepareResourcesWritePhaseBuffers,
                PrepareResourcesCollectPhaseBuffers,
                PrepareResourcesFlush,
                PrepareBindGroups,
            )
                .chain()
                .in_set(Prepare),
        );

        schedule
    }
}

#[derive(Resource, Default, Clone, Deref)]
pub(crate) struct FutureRenderResources(Arc<Mutex<Option<RenderResources>>>);

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, AppLabel)]
pub struct RenderApp;

impl Plugin for RenderPlugin {

    fn build(&self, app: &mut App) {
        app.init_asset::<Shader>()
            .init_asset_loader::<ShaderLoader>();
        load_shader_library!(app, "maths.wgsl");
        load_shader_library!(app, "color_operations.wgsl");
        load_shader_library!(app, "bindless.wgsl");

        if insert_future_resources(&self.render_creation, app.world_mut()) {

            app.add_plugins(ExtractPlugin {
                pre_extract: error_handler::update_state,
            });
        };

        app.add_plugins((
            WindowRenderPlugin,
            CameraPlugin,
            ViewPlugin,
            MeshRenderAssetPlugin,
            GlobalsPlugin,
            TexturePlugin,
            BatchingPlugin {
                debug_flags: self.debug_flags,
            },
            StoragePlugin,
            GpuReadbackPlugin::default(),
            OcclusionCullingPlugin,
            SparseBufferPlugin,
            #[cfg(feature = "tracing-tracy")]
            diagnostic::RenderDiagnosticsPlugin,
        ));

        let (sender, receiver) = bevy_time::create_time_channels();
        app.insert_resource(receiver);

        let asset_server = app.world().resource::<AssetServer>().clone();
        app.init_resource::<RenderAssetBytesPerFrame>()
            .init_resource::<RenderErrorHandler>();
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.init_resource::<RenderScheduleOrder>();
            render_app.init_resource::<RenderAssetBytesPerFrameLimiter>();
            render_app.init_gpu_resource::<renderer::PendingCommandBuffers>();
            render_app.insert_resource(sender);
            render_app.insert_resource(asset_server);
            render_app.insert_resource(RenderState::Initializing);
            render_app.add_systems(
                ExtractSchedule,
                (
                    extract_render_asset_bytes_per_frame,
                    PipelineCache::extract_shaders,
                ),
            );

            #[cfg(not(feature = "reflect_auto_register"))]
            render_app.init_resource::<AppTypeRegistry>();

            #[cfg(feature = "reflect_auto_register")]
            render_app.insert_resource(AppTypeRegistry::new_with_derived_types());

            #[cfg(feature = "reflect_functions")]
            render_app.init_resource::<AppFunctionRegistry>();

            render_app.add_schedule(RenderGraph::base_schedule());

            render_app.init_schedule(RenderStartup);
            render_app
                .get_schedule_mut(RenderStartup)
                .unwrap()
                .set_executor(bevy_ecs::schedule::SingleThreadedExecutor::new());
            render_app.update_schedule = Some(RenderRecovery.intern());
            render_app.add_systems(
                RenderRecovery,
                (run_render_schedule.run_if(renderer_is_ready), send_time).chain(),
            );
            render_app.add_systems(
                Render,
                (
                    (PipelineCache::process_pipeline_queue_system, render_system)
                        .chain()
                        .in_set(RenderSystems::Render),
                    reset_render_asset_bytes_per_frame.in_set(RenderSystems::Cleanup),
                ),
            );
        }
    }

    fn ready(&self, app: &App) -> bool {

        app.world()
            .get_resource::<FutureRenderResources>()
            .and_then(|frr| frr.try_lock().map(|locked| locked.is_some()).ok())
            .unwrap_or(true)
    }

    fn finish(&self, app: &mut App) {
        if let Some(future_render_resources) =
            app.world_mut().remove_resource::<FutureRenderResources>()
        {
            let bevy_app::SubApps { main, sub_apps } = app.sub_apps_mut();
            let render = sub_apps.get_mut(&RenderApp.intern()).unwrap();
            let render_resources = future_render_resources.0.lock().unwrap().take().unwrap();

            render_resources.unpack_into(
                main.world_mut(),
                render.world_mut(),
                self.synchronous_pipeline_compilation,
            );
        }
    }
}

fn renderer_is_ready(state: Res<RenderState>) -> bool {
    matches!(*state, RenderState::Ready)
}

fn run_render_schedule(world: &mut World) {
    world.resource_scope(|world, order: Mut<RenderScheduleOrder>| {
        for &label in &order.labels {
            let _ = world.try_run_schedule(label);
        }
    });
}

fn send_time(time_sender: Res<TimeSender>) {

    if let Err(error) = time_sender.0.try_send(Instant::now()) {
        match error {
            bevy_time::TrySendError::Full(_) => {
                panic!(
                    "The TimeSender channel should always be empty during render. \
                            You might need to add the bevy::core::time_system to your app."
                );
            }
            bevy_time::TrySendError::Disconnected(_) => {

            }
        }
    }
}

fn insert_future_resources(render_creation: &RenderCreation, main_world: &mut World) -> bool {
    let primary_window = main_world
        .query_filtered::<&RawHandleWrapperHolder, With<PrimaryWindow>>()
        .single(main_world)
        .ok()
        .cloned();

    #[cfg(feature = "raw_vulkan_init")]
    let raw_vulkan_init_settings = main_world
        .get_resource::<renderer::raw_vulkan_init::RawVulkanInitSettings>()
        .cloned()
        .unwrap_or_default();

    let future_resources = FutureRenderResources::default();
    let success = render_creation.create_render(
        future_resources.clone(),
        primary_window,
        #[cfg(feature = "raw_vulkan_init")]
        raw_vulkan_init_settings,
    );
    if success {

        main_world.insert_resource(future_resources);
    }
    success
}

pub fn get_adreno_model(adapter_info: &RenderAdapterInfo) -> Option<u32> {
    if !cfg!(target_os = "android") {
        return None;
    }

    let adreno_model = adapter_info.name.strip_prefix("Adreno (TM) ")?;

    Some(
        adreno_model
            .chars()
            .map_while(|c| c.to_digit(10))
            .fold(0, |acc, digit| acc * 10 + digit),
    )
}

pub fn get_mali_driver_version(adapter_info: &RenderAdapterInfo) -> Option<u32> {
    if !cfg!(target_os = "android") {
        return None;
    }

    if !adapter_info.name.contains("Mali") {
        return None;
    }
    let driver_info = &adapter_info.driver_info;
    if let Some(start_pos) = driver_info.find("v1.r")
        && let Some(end_pos) = driver_info[start_pos..].find('p')
    {
        let start_idx = start_pos + 4;
        let end_idx = start_pos + end_pos;

        return driver_info[start_idx..end_idx].parse::<u32>().ok();
    }

    None
}

pub fn get_pixel10_driver_version(adapter_info: &RenderAdapterInfo) -> Option<u32> {
    if !cfg!(target_os = "android") {
        return None;
    }

    if adapter_info.name != "PowerVR D-Series DXT-48-1536 MC1" {
        return None;
    }

    let (_, driver_version) = adapter_info.driver_info.split_once('@')?;
    driver_version.parse::<u32>().ok()
}

pub fn storage_buffers_are_unsupported(limits: &WgpuLimits) -> bool {
    static STORAGE_BUFFERS_UNSUPPORTED: OnceLock<bool> = OnceLock::new();
    *STORAGE_BUFFERS_UNSUPPORTED.get_or_init(|| limits.max_storage_buffers_per_shader_stage == 0)
}
