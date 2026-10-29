#[cfg(feature = "raw_vulkan_init")]
pub mod raw_vulkan_init;
mod render_context;
mod render_device;
mod wgpu_wrapper;

pub use render_context::{
    CurrentView, FlushCommands, PendingCommandBuffers, RenderContext, RenderContextState, ViewQuery,
};
pub use render_device::*;
pub use wgpu_wrapper::WgpuWrapper;

use crate::{
    settings::{RenderResources, WgpuSettings, WgpuSettingsPriority},
    view::{ExtractedWindows, ViewTarget},
};
use alloc::sync::Arc;
use bevy_camera::NormalizedRenderTarget;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::schedule::ScheduleLabel;
use bevy_ecs::{prelude::*, system::SystemState};
#[cfg(feature = "trace")]
use bevy_log::info_span;
use bevy_log::{debug, info, warn};
use bevy_render::camera::ExtractedCamera;
use bevy_window::RawHandleWrapperHolder;
use wgpu::{
    Adapter, AdapterInfo, Backends, DeviceType, ForceShaderModelToken, Instance, Queue,
    RequestAdapterOptions, Trace,
};

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct RenderGraph;

impl RenderGraph {
    pub fn base_schedule() -> Schedule {
        let mut schedule = Schedule::new(Self);
        schedule.configure_sets(
            (
                RenderGraphSystems::Begin,
                RenderGraphSystems::Render,
                RenderGraphSystems::Submit,
                RenderGraphSystems::Finish,
            )
                .chain(),
        );
        schedule
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum RenderGraphSystems {

    Begin,

    Render,

    Submit,

    Finish,
}

pub fn render_system(
    world: &mut World,
    state: &mut SystemState<Query<(&ViewTarget, &ExtractedCamera)>>,
) {
    #[cfg(feature = "trace")]
    let _span = info_span!("main_render_schedule").entered();

    world.run_schedule(RenderGraph);

    {
        let render_device = world.resource::<RenderDevice>();
        let render_queue = world.resource::<RenderQueue>();

        let mut encoder =
            render_device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

        crate::view::screenshot::submit_screenshot_commands(world, &mut encoder);
        crate::gpu_readback::submit_readback_commands(world, &mut encoder);

        render_queue.submit([encoder.finish()]);
    }

    {
        #[cfg(feature = "trace")]
        let _span = info_span!("present_frames").entered();

        world.resource_scope(|world, mut windows: Mut<ExtractedWindows>| {
            let views = state.get(world).unwrap();
            for window in windows.values_mut() {
                let view_needs_present = views.iter().any(|(view_target, camera)| {
                    matches!(
                        camera.target,
                        Some(NormalizedRenderTarget::Window(w)) if w.entity() == window.entity
                    ) && view_target.needs_present()
                });

                if view_needs_present || window.needs_initial_present {
                    window.present();
                    window.needs_initial_present = false;
                }
            }
        });

        #[cfg(feature = "tracing-tracy")]
        bevy_log::event!(
            bevy_log::Level::INFO,
            message = "finished frame",
            tracy.frame_mark = true
        );
    }

    crate::view::screenshot::collect_screenshots(world);
}

#[derive(Resource, Clone, Deref, DerefMut)]
pub struct RenderQueue(pub Arc<WgpuWrapper<Queue>>);

#[derive(Resource, Clone, Debug, Deref, DerefMut)]
pub struct RenderAdapter(pub Arc<WgpuWrapper<Adapter>>);

#[derive(Resource, Clone, Deref, DerefMut)]
pub struct RenderInstance(pub Arc<WgpuWrapper<Instance>>);

#[derive(Resource, Clone, Deref, DerefMut)]
pub struct RenderAdapterInfo(pub WgpuWrapper<AdapterInfo>);

const GPU_NOT_FOUND_ERROR_MESSAGE: &str = if cfg!(target_os = "linux") {
    "Unable to find a GPU! Make sure you have installed required drivers! For extra information, see: https://github.com/bevyengine/bevy/blob/latest/docs/linux_dependencies.md"
} else {
    "Unable to find a GPU! Make sure you have installed required drivers!"
};

#[cfg(not(target_family = "wasm"))]
async fn find_adapter_by_name(
    instance: &Instance,
    options: &WgpuSettings,
    compatible_surface: Option<&wgpu::Surface<'_>>,
    adapter_name: &str,
) -> Option<Adapter> {
    for adapter in instance
        .enumerate_adapters(options.backends.expect(
            "The `backends` field of `WgpuSettings` must be set to use a specific adapter.",
        ))
        .await
    {
        bevy_log::trace!("Checking adapter: {:?}", adapter.get_info());
        let info = adapter.get_info();
        if let Some(surface) = compatible_surface
            && !adapter.is_surface_supported(surface)
        {
            continue;
        }

        if info
            .name
            .to_lowercase()
            .contains(&adapter_name.to_lowercase())
        {
            return Some(adapter);
        }
    }
    None
}

pub async fn initialize_renderer(
    backends: Backends,
    primary_window: Option<RawHandleWrapperHolder>,
    options: &WgpuSettings,
    #[cfg(feature = "raw_vulkan_init")]
    raw_vulkan_init_settings: raw_vulkan_init::RawVulkanInitSettings,
) -> RenderResources {
    let instance_descriptor = wgpu::InstanceDescriptor {
        backends,
        flags: options.instance_flags,
        memory_budget_thresholds: options.instance_memory_budget_thresholds,
        display: None,
        backend_options: wgpu::BackendOptions {
            gl: wgpu::GlBackendOptions {
                gles_minor_version: options.gles3_minor_version,
                fence_behavior: wgpu::GlFenceBehavior::Normal,
                debug_fns: wgpu::GlDebugFns::Auto,
            },
            dx12: wgpu::Dx12BackendOptions {
                shader_compiler: options.dx12_shader_compiler.clone(),
                presentation_system: wgpu::wgt::Dx12SwapchainKind::from_env().unwrap_or_default(),
                latency_waitable_object: wgpu::wgt::Dx12UseFrameLatencyWaitableObject::from_env()
                    .unwrap_or_default(),
                force_shader_model: ForceShaderModelToken::default(),
                agility_sdk: None,
            },
            noop: wgpu::NoopBackendOptions { enable: false },
        },
    };

    #[cfg(not(feature = "raw_vulkan_init"))]
    let instance = Instance::new(instance_descriptor);
    #[cfg(feature = "raw_vulkan_init")]
    let mut additional_vulkan_features = raw_vulkan_init::AdditionalVulkanFeatures::default();
    #[cfg(feature = "raw_vulkan_init")]
    let instance = raw_vulkan_init::create_raw_vulkan_instance(
        instance_descriptor,
        &raw_vulkan_init_settings,
        &mut additional_vulkan_features,
    );

    let surface = primary_window.and_then(|wrapper| {
        let maybe_handle = wrapper
            .0
            .lock()
            .expect("Couldn't get the window handle in time for renderer initialization");
        if let Some(wrapper) = maybe_handle.as_ref() {

            let handle = unsafe { wrapper.get_handle() };
            Some(
                instance
                    .create_surface(handle)
                    .expect("Failed to create wgpu surface"),
            )
        } else {
            None
        }
    });

    let force_fallback_adapter = std::env::var("WGPU_FORCE_FALLBACK_ADAPTER")
        .map_or(options.force_fallback_adapter, |v| {
            !(v.is_empty() || v == "0" || v == "false")
        });

    let desired_adapter_name = std::env::var("WGPU_ADAPTER_NAME")
        .as_deref()
        .map_or(options.adapter_name.clone(), |x| Some(x.to_lowercase()));

    let request_adapter_options = RequestAdapterOptions {
        power_preference: options.power_preference,
        compatible_surface: surface.as_ref(),
        force_fallback_adapter,
    };

    #[cfg(not(target_family = "wasm"))]
    let mut selected_adapter = if let Some(adapter_name) = desired_adapter_name {
        find_adapter_by_name(
            &instance,
            options,
            request_adapter_options.compatible_surface,
            &adapter_name,
        )
        .await
    } else {
        None
    };
    #[cfg(target_family = "wasm")]
    let mut selected_adapter = None;

    #[cfg(target_family = "wasm")]
    if desired_adapter_name.is_some() {
        warn!("Choosing an adapter is not supported on wasm.");
    }

    if selected_adapter.is_none() {
        debug!(
            "Searching for adapter with options: {:?}",
            request_adapter_options
        );
        selected_adapter = instance
            .request_adapter(&request_adapter_options)
            .await
            .ok();
    }

    let adapter = selected_adapter.expect(GPU_NOT_FOUND_ERROR_MESSAGE);
    let adapter_info = adapter.get_info();
    info!("{:?}", adapter_info);

    if adapter_info.device_type == DeviceType::Cpu {
        warn!(
            "The selected adapter is using a driver that only supports software rendering. \
             This is likely to be very slow. See https://bevy.org/learn/errors/b0006/"
        );
    }

    let mut features = wgpu::Features::empty();
    let mut limits = options.limits.clone();
    if matches!(options.priority, WgpuSettingsPriority::Functionality) {
        features = adapter.features();
        if adapter_info.device_type == DeviceType::DiscreteGpu {

            features.remove(wgpu::Features::MAPPABLE_PRIMARY_BUFFERS);
        }

        limits = adapter.limits();
    }

    if let Some(disabled_features) = options.disabled_features {
        features.remove(disabled_features);
    }

    features |= options.features;

    if let Some(constrained_limits) = options.constrained_limits.as_ref() {

        limits = limits.or_worse_values_from(constrained_limits);
    }

    let device_descriptor = wgpu::DeviceDescriptor {
        label: options.device_label.as_ref().map(AsRef::as_ref),
        required_features: features,
        required_limits: limits,

        experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
        memory_hints: options.memory_hints.clone(),

        trace: Trace::Off,
    };

    #[cfg(not(feature = "raw_vulkan_init"))]
    let (device, queue) = adapter.request_device(&device_descriptor).await.unwrap();

    #[cfg(feature = "raw_vulkan_init")]
    let (device, queue) = raw_vulkan_init::create_raw_device(
        &adapter,
        &device_descriptor,
        &raw_vulkan_init_settings,
        &mut additional_vulkan_features,
    )
    .await
    .unwrap();

    debug!("Configured wgpu adapter Limits: {:#?}", device.limits());
    debug!("Configured wgpu adapter Features: {:#?}", device.features());

    RenderResources(
        RenderDevice::from(device),
        RenderQueue(Arc::new(WgpuWrapper::new(queue))),
        RenderAdapterInfo(WgpuWrapper::new(adapter_info)),
        RenderAdapter(Arc::new(WgpuWrapper::new(adapter))),
        RenderInstance(Arc::new(WgpuWrapper::new(instance))),
        #[cfg(feature = "raw_vulkan_init")]
        additional_vulkan_features,
    )
}
