use bevy_material::descriptor::{
    BindGroupLayoutDescriptor, CachedComputePipelineId, CachedRenderPipelineId,
    ComputePipelineDescriptor, PipelineDescriptor, RenderPipelineDescriptor,
};

use crate::{
    render_resource::*,
    renderer::{RenderAdapter, RenderDevice, WgpuWrapper},
    Extract,
};
use alloc::{borrow::Cow, sync::Arc};
use bevy_asset::{AssetEvent, AssetId, Assets, Handle};
use bevy_ecs::{
    message::MessageReader,
    resource::Resource,
    system::{Res, ResMut},
};
use bevy_log::error;
use bevy_platform::collections::{HashMap, HashSet};
use bevy_shader::{
    CachedPipelineId, Shader, ShaderCache, ShaderCacheError, ShaderCacheSource, ShaderDefVal,
    ValidateShader,
};
use bevy_tasks::Task;
use bevy_utils::default;
use core::{future::Future, mem};
use std::sync::{Mutex, PoisonError};
use wgpu::{PipelineCompilationOptions, VertexBufferLayout as RawVertexBufferLayout};

#[derive(Debug)]
pub enum Pipeline {
    RenderPipeline(RenderPipeline),
    ComputePipeline(ComputePipeline),
}

pub struct CachedPipeline {
    pub descriptor: PipelineDescriptor,
    pub state: CachedPipelineState,
}

#[derive(Debug)]
pub enum CachedPipelineState {

    Queued,

    Creating(Task<Result<Pipeline, ShaderCacheError>>),

    Ok(Pipeline),

    Err(ShaderCacheError),
}

impl CachedPipelineState {

    pub fn unwrap(&self) -> &Pipeline {
        match self {
            CachedPipelineState::Ok(pipeline) => pipeline,
            CachedPipelineState::Queued => {
                panic!("Pipeline has not been compiled yet. It is still in the 'Queued' state.")
            }
            CachedPipelineState::Creating(..) => {
                panic!("Pipeline has not been compiled yet. It is still in the 'Creating' state.")
            }
            CachedPipelineState::Err(err) => panic!("{}", err),
        }
    }
}

type ImmediateSize = u32;
type LayoutCacheKey = (Vec<BindGroupLayoutId>, ImmediateSize);
#[derive(Default)]
struct LayoutCache {
    layouts: HashMap<LayoutCacheKey, Arc<WgpuWrapper<PipelineLayout>>>,
}

impl LayoutCache {
    fn get(
        &mut self,
        render_device: &RenderDevice,
        bind_group_layouts: &[BindGroupLayout],
        immediate_size: u32,
    ) -> Arc<WgpuWrapper<PipelineLayout>> {
        let bind_group_ids = bind_group_layouts.iter().map(BindGroupLayout::id).collect();
        self.layouts
            .entry((bind_group_ids, immediate_size))
            .or_insert_with_key(|(_, immediate_size)| {
                let bind_group_layouts = bind_group_layouts
                    .iter()
                    .map(BindGroupLayout::value)
                    .map(Some)
                    .collect::<Vec<_>>();
                Arc::new(WgpuWrapper::new(render_device.create_pipeline_layout(
                    &PipelineLayoutDescriptor {
                        bind_group_layouts: &bind_group_layouts,
                        immediate_size: *immediate_size,
                        ..default()
                    },
                )))
            })
            .clone()
    }
}

fn load_module(
    render_device: &RenderDevice,
    shader_source: ShaderCacheSource,
    validate_shader: &ValidateShader,
) -> Result<WgpuWrapper<ShaderModule>, ShaderCacheError> {
    let shader_source = match shader_source {
        #[cfg(feature = "shader_format_spirv")]
        ShaderCacheSource::SpirV(data) => wgpu::util::make_spirv(data),
        #[cfg(not(feature = "shader_format_spirv"))]
        ShaderCacheSource::SpirV(_) => {
            unimplemented!("Enable feature \"shader_format_spirv\" to use SPIR-V shaders")
        }
        ShaderCacheSource::Wgsl(src) => ShaderSource::Wgsl(Cow::Owned(src)),
        #[cfg(not(feature = "decoupled_naga"))]
        ShaderCacheSource::Naga(src) => ShaderSource::Naga(Cow::Owned(src)),
    };
    let module_descriptor = ShaderModuleDescriptor {
        label: None,
        source: shader_source,
    };

    let scope = render_device
        .wgpu_device()
        .push_error_scope(wgpu::ErrorFilter::Validation);

    let shader_module = WgpuWrapper::new(match validate_shader {
        ValidateShader::Enabled => {
            render_device.create_and_validate_shader_module(module_descriptor)
        }

        ValidateShader::Disabled => unsafe {
            render_device.create_shader_module(module_descriptor)
        },
    });

    let error = scope.pop();

    if let Some(Some(wgpu::Error::Validation { description, .. })) =
        bevy_tasks::futures::now_or_never(error)
    {
        return Err(ShaderCacheError::CreateShaderModule(description));
    }

    Ok(shader_module)
}

#[derive(Default)]
struct BindGroupLayoutCache {
    bgls: HashMap<BindGroupLayoutDescriptor, BindGroupLayout>,
}

impl BindGroupLayoutCache {
    fn get(
        &mut self,
        render_device: &RenderDevice,
        descriptor: BindGroupLayoutDescriptor,
    ) -> BindGroupLayout {
        self.bgls
            .entry(descriptor)
            .or_insert_with_key(|descriptor| {
                render_device
                    .create_bind_group_layout(descriptor.label.as_ref(), &descriptor.entries)
            })
            .clone()
    }
}

#[derive(Resource)]
pub struct PipelineCache {
    layout_cache: Arc<Mutex<LayoutCache>>,
    bindgroup_layout_cache: Arc<Mutex<BindGroupLayoutCache>>,
    shader_cache: Arc<Mutex<ShaderCache<WgpuWrapper<ShaderModule>, RenderDevice>>>,
    device: RenderDevice,
    pipelines: Vec<CachedPipeline>,
    waiting_pipelines: HashSet<CachedPipelineId>,
    new_pipelines: Mutex<Vec<CachedPipeline>>,
    global_shader_defs: Vec<ShaderDefVal>,

    pub(crate) synchronous_pipeline_compilation: bool,

    needs_shader_reload: bool,
}

impl PipelineCache {

    pub fn pipelines(&self) -> impl Iterator<Item = &CachedPipeline> {
        self.pipelines.iter()
    }

    pub fn waiting_pipelines(&self) -> impl Iterator<Item = CachedPipelineId> + '_ {
        self.waiting_pipelines.iter().copied()
    }

    pub fn new(
        device: RenderDevice,
        render_adapter: RenderAdapter,
        synchronous_pipeline_compilation: bool,
    ) -> Self {
        let mut global_shader_defs = Vec::new();

        if cfg!(all(target_os = "windows", feature = "native-gles")) {
            global_shader_defs.push("NATIVE_GLES_GLOBALS_ALIGNMENT".into());
        }
        #[cfg(all(feature = "webgl", target_arch = "wasm32", not(feature = "webgpu")))]
        {
            global_shader_defs.push("NO_ARRAY_TEXTURES_SUPPORT".into());
            global_shader_defs.push("NO_CUBE_ARRAY_TEXTURES_SUPPORT".into());
            global_shader_defs.push("SIXTEEN_BYTE_ALIGNMENT".into());
        }

        if cfg!(all(target_os = "windows", feature = "native-gles"))
            && !render_adapter.get_downlevel_capabilities().flags.contains(DownlevelFlags::CUBE_ARRAY_TEXTURES)
        {
            global_shader_defs.push("NO_CUBE_ARRAY_TEXTURES_SUPPORT".into());
        }

        if cfg!(target_abi = "sim") {
            global_shader_defs.push("NO_CUBE_ARRAY_TEXTURES_SUPPORT".into());
        }

        global_shader_defs.push(ShaderDefVal::UInt(
            String::from("AVAILABLE_STORAGE_BUFFER_BINDINGS"),
            device.limits().max_storage_buffers_per_shader_stage,
        ));

        Self {
            shader_cache: Arc::new(Mutex::new(ShaderCache::new(
                device.clone(),
                device.features(),
                render_adapter.get_downlevel_capabilities().flags,
                load_module,
            ))),
            device,
            layout_cache: default(),
            bindgroup_layout_cache: default(),
            waiting_pipelines: default(),
            new_pipelines: default(),
            pipelines: default(),
            global_shader_defs,
            synchronous_pipeline_compilation,
            needs_shader_reload: true,
        }
    }

    #[inline]
    pub fn get_render_pipeline_state(&self, id: CachedRenderPipelineId) -> &CachedPipelineState {

        self.pipelines
            .get(id.id())
            .map_or(&CachedPipelineState::Queued, |pipeline| &pipeline.state)
    }

    #[inline]
    pub fn get_compute_pipeline_state(&self, id: CachedComputePipelineId) -> &CachedPipelineState {

        self.pipelines
            .get(id.id())
            .map_or(&CachedPipelineState::Queued, |pipeline| &pipeline.state)
    }

    #[inline]
    pub fn get_render_pipeline_descriptor(
        &self,
        id: CachedRenderPipelineId,
    ) -> &RenderPipelineDescriptor {
        match &self.pipelines[id.id()].descriptor {
            PipelineDescriptor::RenderPipelineDescriptor(descriptor) => descriptor,
            PipelineDescriptor::ComputePipelineDescriptor(_) => unreachable!(),
        }
    }

    #[inline]
    pub fn get_compute_pipeline_descriptor(
        &self,
        id: CachedComputePipelineId,
    ) -> &ComputePipelineDescriptor {
        match &self.pipelines[id.id()].descriptor {
            PipelineDescriptor::RenderPipelineDescriptor(_) => unreachable!(),
            PipelineDescriptor::ComputePipelineDescriptor(descriptor) => descriptor,
        }
    }

    #[inline]
    pub fn get_render_pipeline(&self, id: CachedRenderPipelineId) -> Option<&RenderPipeline> {
        if let CachedPipelineState::Ok(Pipeline::RenderPipeline(pipeline)) =
            &self.pipelines.get(id.id())?.state
        {
            Some(pipeline)
        } else {
            None
        }
    }

    #[inline]
    pub fn block_on_render_pipeline(&mut self, id: CachedRenderPipelineId) {
        if self.pipelines.len() <= id.id() {
            self.process_queue();
        }

        let state = &mut self.pipelines[id.id()].state;
        if let CachedPipelineState::Creating(task) = state {
            *state = match bevy_tasks::block_on(task) {
                Ok(p) => CachedPipelineState::Ok(p),
                Err(e) => CachedPipelineState::Err(e),
            };
        }
    }

    #[inline]
    pub fn get_compute_pipeline(&self, id: CachedComputePipelineId) -> Option<&ComputePipeline> {
        if let CachedPipelineState::Ok(Pipeline::ComputePipeline(pipeline)) =
            &self.pipelines.get(id.id())?.state
        {
            Some(pipeline)
        } else {
            None
        }
    }

    pub fn queue_render_pipeline(
        &self,
        descriptor: RenderPipelineDescriptor,
    ) -> CachedRenderPipelineId {
        let mut new_pipelines = self
            .new_pipelines
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let id = CachedRenderPipelineId::new(self.pipelines.len() + new_pipelines.len());
        new_pipelines.push(CachedPipeline {
            descriptor: PipelineDescriptor::RenderPipelineDescriptor(Box::new(descriptor)),
            state: CachedPipelineState::Queued,
        });
        id
    }

    pub fn queue_compute_pipeline(
        &self,
        descriptor: ComputePipelineDescriptor,
    ) -> CachedComputePipelineId {
        let mut new_pipelines = self
            .new_pipelines
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let id = CachedComputePipelineId::new(self.pipelines.len() + new_pipelines.len());
        new_pipelines.push(CachedPipeline {
            descriptor: PipelineDescriptor::ComputePipelineDescriptor(Box::new(descriptor)),
            state: CachedPipelineState::Queued,
        });
        id
    }

    pub fn get_bind_group_layout(
        &self,
        bind_group_layout_descriptor: &BindGroupLayoutDescriptor,
    ) -> BindGroupLayout {
        self.bindgroup_layout_cache
            .lock()
            .unwrap()
            .get(&self.device, bind_group_layout_descriptor.clone())
    }

    pub fn set_shader(&mut self, id: AssetId<Shader>, shader: Shader) {
        let mut shader_cache = self.shader_cache.lock().unwrap();
        let pipelines_to_queue = shader_cache.set_shader(id, shader);
        for cached_pipeline in pipelines_to_queue {
            self.pipelines[cached_pipeline].state = CachedPipelineState::Queued;
            self.waiting_pipelines.insert(cached_pipeline);
        }
    }

    pub fn remove_shader(&mut self, shader: AssetId<Shader>) {
        let mut shader_cache = self.shader_cache.lock().unwrap();
        let pipelines_to_queue = shader_cache.remove(shader);
        for cached_pipeline in pipelines_to_queue {
            self.pipelines[cached_pipeline].state = CachedPipelineState::Queued;
            self.waiting_pipelines.insert(cached_pipeline);
        }
    }

    fn start_create_render_pipeline(
        &mut self,
        id: CachedPipelineId,
        descriptor: RenderPipelineDescriptor,
    ) -> CachedPipelineState {
        let device = self.device.clone();
        let shader_cache = self.shader_cache.clone();
        let layout_cache = self.layout_cache.clone();
        let mut bindgroup_layout_cache = self.bindgroup_layout_cache.lock().unwrap();
        let bind_group_layout = descriptor
            .layout
            .iter()
            .map(|bind_group_layout_descriptor| {
                bindgroup_layout_cache.get(&self.device, bind_group_layout_descriptor.clone())
            })
            .collect::<Vec<_>>();

        create_pipeline_task(
            async move {
                let mut shader_cache = shader_cache.lock().unwrap();
                let mut layout_cache = layout_cache.lock().unwrap();

                let vertex_module = match shader_cache.get(
                    id,
                    descriptor.vertex.shader.id(),
                    &descriptor.vertex.shader_defs,
                ) {
                    Ok(module) => module,
                    Err(err) => return Err(err),
                };

                let fragment_module = match &descriptor.fragment {
                    Some(fragment) => {
                        match shader_cache.get(id, fragment.shader.id(), &fragment.shader_defs) {
                            Ok(module) => Some(module),
                            Err(err) => return Err(err),
                        }
                    }
                    None => None,
                };

                let layout = if descriptor.layout.is_empty() && descriptor.immediate_size == 0 {
                    None
                } else {
                    Some(layout_cache.get(&device, &bind_group_layout, descriptor.immediate_size))
                };

                drop((shader_cache, layout_cache));

                let vertex_buffer_layouts = descriptor
                    .vertex
                    .buffers
                    .iter()
                    .map(|layout| RawVertexBufferLayout {
                        array_stride: layout.array_stride,
                        attributes: &layout.attributes,
                        step_mode: layout.step_mode,
                    })
                    .collect::<Vec<_>>();

                let fragment_data = descriptor.fragment.as_ref().map(|fragment| {
                    (
                        fragment_module.unwrap(),
                        fragment.entry_point.as_deref(),
                        fragment.targets.as_slice(),
                    )
                });

                let compilation_options = PipelineCompilationOptions {
                    constants: &[],
                    zero_initialize_workgroup_memory: descriptor.zero_initialize_workgroup_memory,
                };

                let descriptor = RawRenderPipelineDescriptor {
                    multiview_mask: None,
                    depth_stencil: descriptor.depth_stencil.clone(),
                    label: descriptor.label.as_deref(),
                    layout: layout.as_ref().map(|layout| -> &PipelineLayout { layout }),
                    multisample: descriptor.multisample,
                    primitive: descriptor.primitive,
                    vertex: RawVertexState {
                        buffers: &vertex_buffer_layouts,
                        entry_point: descriptor.vertex.entry_point.as_deref(),
                        module: &vertex_module,

                        compilation_options: compilation_options.clone(),
                    },
                    fragment: fragment_data
                        .as_ref()
                        .map(|(module, entry_point, targets)| RawFragmentState {
                            entry_point: entry_point.as_deref(),
                            module,
                            targets,

                            compilation_options,
                        }),
                    cache: None,
                };

                Ok(Pipeline::RenderPipeline(
                    device.create_render_pipeline(&descriptor),
                ))
            },
            self.synchronous_pipeline_compilation,
        )
    }

    fn start_create_compute_pipeline(
        &mut self,
        id: CachedPipelineId,
        descriptor: ComputePipelineDescriptor,
    ) -> CachedPipelineState {
        let device = self.device.clone();
        let shader_cache = self.shader_cache.clone();
        let layout_cache = self.layout_cache.clone();
        let mut bindgroup_layout_cache = self.bindgroup_layout_cache.lock().unwrap();
        let bind_group_layout = descriptor
            .layout
            .iter()
            .map(|bind_group_layout_descriptor| {
                bindgroup_layout_cache.get(&self.device, bind_group_layout_descriptor.clone())
            })
            .collect::<Vec<_>>();

        create_pipeline_task(
            async move {
                let mut shader_cache = shader_cache.lock().unwrap();
                let mut layout_cache = layout_cache.lock().unwrap();

                let compute_module =
                    match shader_cache.get(id, descriptor.shader.id(), &descriptor.shader_defs) {
                        Ok(module) => module,
                        Err(err) => return Err(err),
                    };

                let layout = if descriptor.layout.is_empty() && descriptor.immediate_size == 0 {
                    None
                } else {
                    Some(layout_cache.get(&device, &bind_group_layout, descriptor.immediate_size))
                };

                drop((shader_cache, layout_cache));

                let descriptor = RawComputePipelineDescriptor {
                    label: descriptor.label.as_deref(),
                    layout: layout.as_ref().map(|layout| -> &PipelineLayout { layout }),
                    module: &compute_module,
                    entry_point: descriptor.entry_point.as_deref(),

                    compilation_options: PipelineCompilationOptions {
                        constants: &[],
                        zero_initialize_workgroup_memory: descriptor
                            .zero_initialize_workgroup_memory,
                    },
                    cache: None,
                };

                Ok(Pipeline::ComputePipeline(
                    device.create_compute_pipeline(&descriptor),
                ))
            },
            self.synchronous_pipeline_compilation,
        )
    }

    pub fn process_queue(&mut self) {
        let mut waiting_pipelines = mem::take(&mut self.waiting_pipelines);
        let mut pipelines = mem::take(&mut self.pipelines);

        {
            let mut new_pipelines = self
                .new_pipelines
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for new_pipeline in new_pipelines.drain(..) {
                let id = pipelines.len();
                pipelines.push(new_pipeline);
                waiting_pipelines.insert(id);
            }
        }

        for id in waiting_pipelines {
            self.process_pipeline(&mut pipelines[id], id);
        }

        self.pipelines = pipelines;
    }

    fn process_pipeline(&mut self, cached_pipeline: &mut CachedPipeline, id: usize) {
        match &mut cached_pipeline.state {
            CachedPipelineState::Queued => {
                cached_pipeline.state = match &cached_pipeline.descriptor {
                    PipelineDescriptor::RenderPipelineDescriptor(descriptor) => {
                        self.start_create_render_pipeline(id, *descriptor.clone())
                    }
                    PipelineDescriptor::ComputePipelineDescriptor(descriptor) => {
                        self.start_create_compute_pipeline(id, *descriptor.clone())
                    }
                };
            }

            CachedPipelineState::Creating(task) => match bevy_tasks::futures::check_ready(task) {
                Some(Ok(pipeline)) => {
                    cached_pipeline.state = CachedPipelineState::Ok(pipeline);
                    return;
                }
                Some(Err(err)) => cached_pipeline.state = CachedPipelineState::Err(err),
                _ => (),
            },

            CachedPipelineState::Err(err) => match err {

                ShaderCacheError::ShaderNotLoaded(_)
                | ShaderCacheError::ShaderImportNotYetAvailable => {
                    cached_pipeline.state = CachedPipelineState::Queued;
                }

                ShaderCacheError::ProcessShaderError(err) => {
                    let error_detail =
                        err.emit_to_string(&self.shader_cache.lock().unwrap().composer);
                    if std::env::var("VERBOSE_SHADER_ERROR")
                        .is_ok_and(|v| !(v.is_empty() || v == "0" || v == "false"))
                    {
                        error!("{}", pipeline_error_context(cached_pipeline));
                    }
                    error!("failed to process shader error:\n{}", error_detail);
                    return;
                }
                ShaderCacheError::CreateShaderModule(description) => {
                    error!("failed to create shader module: {}", description);
                    return;
                }
            },

            CachedPipelineState::Ok(_) => return,
        }

        self.waiting_pipelines.insert(id);
    }

    pub(crate) fn process_pipeline_queue_system(mut cache: ResMut<Self>) {
        cache.process_queue();
    }

    pub(crate) fn extract_shaders(
        mut cache: ResMut<Self>,
        shaders: Extract<Res<Assets<Shader>>>,
        mut events: Extract<MessageReader<AssetEvent<Shader>>>,
    ) {
        if cache.needs_shader_reload {
            cache.needs_shader_reload = false;
            for (id, shader) in shaders.iter() {
                let mut shader = shader.clone();
                shader.shader_defs.extend(cache.global_shader_defs.clone());
                cache.set_shader(id, shader);
            }

            for _ in events.read() {}
            return;
        }

        for event in events.read() {
            #[expect(
                clippy::match_same_arms,
                reason = "LoadedWithDependencies is marked as a TODO, so it's likely this will no longer lint soon."
            )]
            match event {

                AssetEvent::Added { id } | AssetEvent::Modified { id } => {
                    if let Some(shader) = shaders.get(*id) {
                        let mut shader = shader.clone();
                        shader.shader_defs.extend(cache.global_shader_defs.clone());

                        cache.set_shader(*id, shader);
                    }
                }
                AssetEvent::Removed { id } => cache.remove_shader(*id),
                AssetEvent::Unused { .. } => {}
                AssetEvent::LoadedWithDependencies { .. } => {

                }
            }
        }
    }
}

fn pipeline_error_context(cached_pipeline: &CachedPipeline) -> String {
    fn format(
        shader: &Handle<Shader>,
        entry: &Option<Cow<'static, str>>,
        shader_defs: &[ShaderDefVal],
    ) -> String {
        let source = match shader.path() {
            Some(path) => path.path().to_string_lossy().to_string(),
            None => String::new(),
        };
        let entry = match entry {
            Some(entry) => entry.to_string(),
            None => String::new(),
        };
        let shader_defs = shader_defs
            .iter()
            .flat_map(|def| match def {
                ShaderDefVal::Bool(k, v) if *v => Some(k.to_string()),
                ShaderDefVal::Int(k, v) => Some(format!("{k} = {v}")),
                ShaderDefVal::UInt(k, v) => Some(format!("{k} = {v}")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("{source}:{entry}\nshader defs: {shader_defs}")
    }
    match &cached_pipeline.descriptor {
        PipelineDescriptor::RenderPipelineDescriptor(desc) => {
            let vert = &desc.vertex;
            let vert_str = format(&vert.shader, &vert.entry_point, &vert.shader_defs);
            let Some(frag) = desc.fragment.as_ref() else {
                return vert_str;
            };
            let frag_str = format(&frag.shader, &frag.entry_point, &frag.shader_defs);
            format!("vertex {vert_str}\nfragment {frag_str}")
        }
        PipelineDescriptor::ComputePipelineDescriptor(desc) => {
            format(&desc.shader, &desc.entry_point, &desc.shader_defs)
        }
    }
}

#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "macos"),
    feature = "multi_threaded"
))]
fn create_pipeline_task(
    task: impl Future<Output = Result<Pipeline, ShaderCacheError>> + Send + 'static,
    sync: bool,
) -> CachedPipelineState {
    if !sync {
        return CachedPipelineState::Creating(bevy_tasks::AsyncComputeTaskPool::get().spawn(task));
    }

    match bevy_tasks::block_on(task) {
        Ok(pipeline) => CachedPipelineState::Ok(pipeline),
        Err(err) => CachedPipelineState::Err(err),
    }
}

#[cfg(any(
    target_arch = "wasm32",
    target_os = "macos",
    not(feature = "multi_threaded")
))]
fn create_pipeline_task(
    task: impl Future<Output = Result<Pipeline, ShaderCacheError>> + Send + 'static,
    _sync: bool,
) -> CachedPipelineState {
    match bevy_tasks::block_on(task) {
        Ok(pipeline) => CachedPipelineState::Ok(pipeline),
        Err(err) => CachedPipelineState::Err(err),
    }
}
