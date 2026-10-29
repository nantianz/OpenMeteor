use crate::{
    error_handler::DeviceErrorHandler,
    render_resource::PipelineCache,
    renderer::{self, RenderAdapter, RenderAdapterInfo, RenderDevice, RenderInstance, RenderQueue},
    FutureRenderResources,
};
use alloc::borrow::Cow;
use bevy_ecs::world::World;
use bevy_image::{CompressedImageFormatSupport, CompressedImageFormats};
use bevy_window::RawHandleWrapperHolder;

use wgpu::MemoryBudgetThresholds;
pub use wgpu::{
    Backends, Dx12Compiler, Features as WgpuFeatures, Gles3MinorVersion, InstanceFlags,
    Limits as WgpuLimits, MemoryHints, PowerPreference,
};

#[derive(Clone)]
pub enum WgpuSettingsPriority {

    WebGPU,

    Functionality,

    WebGL2,
}

#[derive(Clone)]
pub struct WgpuSettings {
    pub device_label: Option<Cow<'static, str>>,
    pub backends: Option<Backends>,
    pub power_preference: PowerPreference,
    pub priority: WgpuSettingsPriority,

    pub features: WgpuFeatures,

    pub disabled_features: Option<WgpuFeatures>,

    pub limits: WgpuLimits,

    pub constrained_limits: Option<WgpuLimits>,

    pub dx12_shader_compiler: Dx12Compiler,

    pub gles3_minor_version: Gles3MinorVersion,

    pub instance_flags: InstanceFlags,

    pub memory_hints: MemoryHints,

    pub instance_memory_budget_thresholds: MemoryBudgetThresholds,

    pub force_fallback_adapter: bool,

    pub adapter_name: Option<String>,
}

impl Default for WgpuSettings {
    fn default() -> Self {
        let default_backends = if cfg!(all(
            feature = "webgl",
            target_arch = "wasm32",
            not(feature = "webgpu")
        )) {
            Backends::GL
        } else if cfg!(all(feature = "webgpu", target_arch = "wasm32")) {
            Backends::BROWSER_WEBGPU
        } else {
            Backends::all()
        };

        let backends = Some(Backends::from_env().unwrap_or(default_backends));

        let power_preference =
            PowerPreference::from_env().unwrap_or(PowerPreference::HighPerformance);

        let priority = settings_priority_from_env().unwrap_or(WgpuSettingsPriority::Functionality);

        let limits = if cfg!(all(
            feature = "webgl",
            target_arch = "wasm32",
            not(feature = "webgpu")
        )) || matches!(priority, WgpuSettingsPriority::WebGL2)
        {
            wgpu::Limits::downlevel_webgl2_defaults()
        } else {
            #[expect(clippy::allow_attributes, reason = "`unused_mut` is not always linted")]
            #[allow(
                unused_mut,
                reason = "This variable needs to be mutable if the `ci_limits` feature is enabled"
            )]
            let mut limits = wgpu::Limits::default();
            #[cfg(feature = "ci_limits")]
            {
                limits.max_storage_textures_per_shader_stage = 4;
                limits.max_texture_dimension_3d = 1024;
            }
            limits
        };

        let dx12_shader_compiler =
            Dx12Compiler::from_env().unwrap_or(if cfg!(feature = "statically-linked-dxc") {
                Dx12Compiler::StaticDxc
            } else {
                let dxc = "dxcompiler.dll";

                if cfg!(target_os = "windows") && std::fs::metadata(dxc).is_ok() {
                    Dx12Compiler::DynamicDxc {
                        dxc_path: String::from(dxc),
                    }
                } else {
                    Dx12Compiler::Fxc
                }
            });

        let gles3_minor_version = Gles3MinorVersion::from_env().unwrap_or_default();

        let mut instance_flags = InstanceFlags::default();
        #[cfg(not(debug_assertions))]
        {

            if !backends.is_some_and(|backends| backends.contains(Backends::DX12)) {

                instance_flags.remove(InstanceFlags::VALIDATION_INDIRECT_CALL);
            }
        }
        #[cfg(all(not(debug_assertions), feature = "raw_vulkan_init"))]

        instance_flags.remove(InstanceFlags::VALIDATION_INDIRECT_CALL);

        instance_flags = instance_flags.with_env();

        Self {
            device_label: Default::default(),
            backends,
            power_preference,
            priority,
            features: wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES,
            disabled_features: None,
            limits,
            constrained_limits: None,
            dx12_shader_compiler,
            gles3_minor_version,
            instance_flags,
            memory_hints: MemoryHints::default(),
            instance_memory_budget_thresholds: MemoryBudgetThresholds::default(),
            force_fallback_adapter: false,
            adapter_name: None,
        }
    }
}

#[derive(Clone)]
pub struct RenderResources(
    pub RenderDevice,
    pub RenderQueue,
    pub RenderAdapterInfo,
    pub RenderAdapter,
    pub RenderInstance,
    #[cfg(feature = "raw_vulkan_init")] pub renderer::raw_vulkan_init::AdditionalVulkanFeatures,
);

impl RenderResources {

    pub(crate) fn unpack_into(
        self,
        main_world: &mut World,
        render_world: &mut World,
        synchronous_pipeline_compilation: bool,
    ) {
        let RenderResources(device, queue, adapter_info, render_adapter, instance, ..) = self;

        let compressed_image_format_support =
            CompressedImageFormatSupport(CompressedImageFormats::from_features(device.features()));

        main_world.insert_resource(device.clone());
        main_world.insert_resource(queue.clone());
        main_world.insert_resource(adapter_info.clone());
        main_world.insert_resource(render_adapter.clone());
        main_world.insert_resource(compressed_image_format_support);

        #[cfg(feature = "raw_vulkan_init")]
        {
            let additional_vulkan_features: renderer::raw_vulkan_init::AdditionalVulkanFeatures =
                self.5;
            render_world.insert_resource(additional_vulkan_features);
        }

        render_world.insert_resource(instance);
        render_world.insert_resource(PipelineCache::new(
            device.clone(),
            render_adapter.clone(),
            synchronous_pipeline_compilation,
        ));
        render_world.insert_resource(DeviceErrorHandler::new(&device));
        render_world.insert_resource(device);
        render_world.insert_resource(queue);
        render_world.insert_resource(render_adapter);
        render_world.insert_resource(adapter_info);
    }
}

pub enum RenderCreation {

    Manual(RenderResources),

    Automatic(Box<WgpuSettings>),
}

impl RenderCreation {

    pub fn manual(
        device: RenderDevice,
        queue: RenderQueue,
        adapter_info: RenderAdapterInfo,
        adapter: RenderAdapter,
        instance: RenderInstance,
        #[cfg(feature = "raw_vulkan_init")]
        additional_vulkan_features: renderer::raw_vulkan_init::AdditionalVulkanFeatures,
    ) -> Self {
        RenderResources(
            device,
            queue,
            adapter_info,
            adapter,
            instance,
            #[cfg(feature = "raw_vulkan_init")]
            additional_vulkan_features,
        )
        .into()
    }

    pub(crate) fn create_render(
        &self,
        future_resources: FutureRenderResources,
        primary_window: Option<RawHandleWrapperHolder>,
        #[cfg(feature = "raw_vulkan_init")]
        raw_vulkan_init_settings: renderer::raw_vulkan_init::RawVulkanInitSettings,
    ) -> bool {
        match self {
            RenderCreation::Manual(resources) => {
                *future_resources.lock().unwrap() = Some(resources.clone());
            }
            RenderCreation::Automatic(render_creation) => {
                let Some(backends) = render_creation.backends else {
                    return false;
                };
                let settings = render_creation.clone();

                let async_renderer = async move {
                    let render_resources = renderer::initialize_renderer(
                        backends,
                        primary_window,
                        &settings,
                        #[cfg(feature = "raw_vulkan_init")]
                        raw_vulkan_init_settings,
                    )
                    .await;

                    *future_resources.lock().unwrap() = Some(render_resources);
                };

                #[cfg(target_arch = "wasm32")]
                bevy_tasks::IoTaskPool::get()
                    .spawn_local(async_renderer)
                    .detach();

                #[cfg(not(target_arch = "wasm32"))]
                bevy_tasks::block_on(async_renderer);
            }
        }
        true
    }
}

impl From<RenderResources> for RenderCreation {
    fn from(value: RenderResources) -> Self {
        Self::Manual(value)
    }
}

impl Default for RenderCreation {
    fn default() -> Self {
        Self::Automatic(Default::default())
    }
}

impl From<WgpuSettings> for RenderCreation {
    fn from(value: WgpuSettings) -> Self {
        Self::Automatic(Box::new(value))
    }
}

pub fn settings_priority_from_env() -> Option<WgpuSettingsPriority> {
    Some(
        match std::env::var("WGPU_SETTINGS_PRIO")
            .as_deref()
            .map(str::to_lowercase)
            .as_deref()
        {
            Ok("webgpu") => WgpuSettingsPriority::WebGPU,
            Ok("functionality") => WgpuSettingsPriority::Functionality,
            Ok("webgl2") => WgpuSettingsPriority::WebGL2,
            _ => return None,
        },
    )
}
