

use alloc::sync::Arc;
use std::sync::Mutex;

use bevy_app::{App, Plugin};
use bevy_asset::{embedded_asset, load_embedded_asset, AssetServer, Handle};
use bevy_camera::Camera;
use bevy_color::Color;
use bevy_core_pipeline::{prepass::node::early_prepass, Core3d, Core3dSystems};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    component::Component,
    entity::Entity,
    query::With,
    resource::Resource,
    schedule::IntoScheduleConfigs as _,
    system::{Commands, Query, Res, ResMut},
    world::{FromWorld, World},
};
use bevy_light::{
    cluster::{Clusters, GlobalClusterGpuSettings, GlobalClusterSettings},
    EnvironmentMapLight, IrradianceVolume,
};
use bevy_material::descriptor::{
    BindGroupLayoutDescriptor, CachedComputePipelineId, CachedRenderPipelineId,
    ComputePipelineDescriptor, FragmentState, RenderPipelineDescriptor, VertexState,
};
use bevy_math::{vec2, Vec2};
use bevy_mesh::{VertexBufferLayout, VertexFormat};
use bevy_render::{
    diagnostic::RecordDiagnostics as _,
    extract_resource::{ExtractResource, ExtractResourcePlugin},
    render_resource::{
        binding_types,
        encase::internal::{CreateFrom as _, Reader},
        BindGroup, BindGroupEntry, BindGroupLayoutEntries, Buffer, BufferBindingType,
        BufferDescriptor, BufferInitDescriptor, BufferUsages, ColorTargetState, ColorWrites,
        CommandEncoder, ComputePassDescriptor, ComputePipeline, Extent3d, IndexFormat, LoadOp,
        MapMode, Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor,
        RenderPipeline, ShaderStages, ShaderType, SpecializedComputePipeline,
        SpecializedComputePipelines, SpecializedRenderPipeline, SpecializedRenderPipelines,
        StorageBuffer, StoreOp, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
        UninitBufferVec, VertexAttribute, VertexStepMode,
    },
    renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
    sync_world::{MainEntity, MainEntityHashMap, MainEntityHashSet, RenderEntity},
    texture::{CachedTexture, TextureCache},
    view::{ExtractedView, ViewUniform, ViewUniformOffset, ViewUniforms},
    GpuResourceAppExt, MainWorld, Render, RenderApp, RenderSystems,
};
use bevy_shader::{load_shader_library, Shader, ShaderDefVal};
use bevy_utils::default;
use bytemuck::{Pod, Zeroable};
use tracing::{error, trace, warn};

use crate::{
    cluster::{
        GpuClusterOffsetAndCounts, GpuClusterOffsetsAndCountsStorage,
        GpuClusterableObjectIndexListsStorage, ViewClusterBuffers,
    },
    decal::clustered::{DecalsBuffer, RenderClusteredDecal, RenderClusteredDecals},
    gpu_clustering_is_enabled, ExtractedClusterConfig, GlobalClusterableObjectMeta,
    GpuClusteredLight, GpuLights, LightMeta, LightProbesBuffer, LightProbesUniform,
    RenderViewLightProbes, ViewClusterBindings, ViewLightProbesUniformOffset,
    ViewLightsUniformOffset,
};

const ALLOCATION_WORKGROUP_SIZE: u32 = 256;

const Z_SLICING_WORKGROUP_SIZE: u32 = 64;

pub struct GpuClusteringPlugin;

impl Plugin for GpuClusteringPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "cluster.wgsl");
        embedded_asset!(app, "cluster_z_slice.wgsl");
        embedded_asset!(app, "cluster_raster.wgsl");
        embedded_asset!(app, "cluster_allocate.wgsl");

        app.add_plugins(ExtractResourcePlugin::<
            GlobalClusterSettings,
            GpuClusteringPlugin,
        >::default());
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        let render_device = render_app.world().resource::<RenderDevice>();
        if render_device.limits().max_storage_buffers_per_shader_stage == 0 {
            return;
        }

        render_app
            .init_gpu_resource::<SpecializedRenderPipelines<ClusteringRasterPipeline>>()
            .init_gpu_resource::<SpecializedComputePipelines<ClusteringZSlicingPipeline>>()
            .init_gpu_resource::<SpecializedComputePipelines<ClusteringAllocationPipeline>>()
            .init_gpu_resource::<RenderViewClusteringReadbackData>()
            .init_gpu_resource::<GpuClusteringMeshBuffers>()
            .init_gpu_resource::<ClusteringRasterPipeline>()
            .init_gpu_resource::<ClusteringZSlicingPipeline>()
            .init_gpu_resource::<ClusteringAllocationPipeline>()
            .add_systems(
                Render,
                (prepare_clustering_pipelines, prepare_cluster_dummy_textures)
                    .in_set(RenderSystems::Prepare)
                    .run_if(gpu_clustering_is_enabled),
            )
            .add_systems(
                Render,
                (
                    prepare_clusters_for_gpu_clustering,
                    upload_view_gpu_clustering_buffers,
                )
                    .chain()
                    .in_set(RenderSystems::PrepareResources)
                    .run_if(gpu_clustering_is_enabled),
            )
            .add_systems(
                Render,
                prepare_clustering_bind_groups
                    .in_set(RenderSystems::PrepareBindGroups)
                    .run_if(gpu_clustering_is_enabled),
            )
            .add_systems(
                Core3d,
                cluster_on_gpu
                    .before(early_prepass)
                    .in_set(Core3dSystems::Prepass)
                    .run_if(gpu_clustering_is_enabled),
            );
    }
}

#[derive(Component, Deref, DerefMut)]
pub struct ViewClusteringDummyTexture(CachedTexture);

#[derive(Component)]
pub struct ViewClusteringBindGroups {

    clustering_bind_group_z_slicing_pass: BindGroup,

    clustering_bind_group_count_pass: BindGroup,

    clustering_bind_group_allocate_pass: BindGroup,

    clustering_bind_group_populate_pass: BindGroup,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, ShaderType, Pod, Zeroable)]
#[repr(C)]
pub struct ClusterableObjectZSlice {

    pub object_index: u32,

    pub object_type: u32,

    pub z_slice: u32,
}

#[derive(Clone, Copy, Default, ShaderType, Pod, Zeroable)]
#[repr(C)]
pub struct ClusterMetadata {

    indirect_draw_params: ClusterRasterIndirectDrawParams,

    clustered_light_count: u32,

    reflection_probe_count: u32,

    irradiance_volume_count: u32,

    decal_count: u32,

    z_slice_list_capacity: u32,

    index_list_capacity: u32,

    farthest_z: u32,
}

#[derive(Clone, Copy, Default, ShaderType, Pod, Zeroable)]
#[repr(C)]
pub struct ClusterRasterIndirectDrawParams {
    index_count: u32,

    instance_count: u32,

    first_index: u32,
    base_vertex: u32,
    first_instance: u32,
}

#[derive(Component)]
pub struct ViewGpuClusteringBuffers {

    pub z_slices_buffer: UninitBufferVec<ClusterableObjectZSlice>,

    scratchpad_offsets_and_counts_buffer: UninitBufferVec<GpuClusterOffsetAndCounts>,

    cluster_metadata_buffer: StorageBuffer<ClusterMetadata>,
}

impl ViewGpuClusteringBuffers {

    pub(crate) fn new() -> ViewGpuClusteringBuffers {
        let mut cluster_metadata_buffer = StorageBuffer::from(ClusterMetadata::default());
        cluster_metadata_buffer.add_usages(BufferUsages::COPY_SRC | BufferUsages::INDIRECT);
        cluster_metadata_buffer.set_label(Some("clustering Z slicing metadata buffer"));

        ViewGpuClusteringBuffers {
            cluster_metadata_buffer,
            z_slices_buffer: UninitBufferVec::new(BufferUsages::STORAGE | BufferUsages::COPY_DST),
            scratchpad_offsets_and_counts_buffer: UninitBufferVec::new(
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            ),
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct RenderViewClusteringReadbackData {

    views: MainEntityHashMap<Arc<Mutex<ViewClusteringReadbackData>>>,
}

struct ViewClusteringReadbackData {

    z_slice_list_capacity: usize,

    max_index_list_capacity: usize,

    metadata_staging_pending_buffers: Vec<Buffer>,

    metadata_staging_free_buffers: Vec<Buffer>,

    last_frame_statistics: Option<ViewClusteringLastFrameStatistics>,
}

struct ViewClusteringLastFrameStatistics {

    index_list_size: u32,

    farthest_z: f32,
}

impl ViewClusteringReadbackData {

    fn new(settings: &GlobalClusterGpuSettings) -> ViewClusteringReadbackData {
        ViewClusteringReadbackData {
            z_slice_list_capacity: settings.initial_z_slice_list_capacity,
            max_index_list_capacity: settings.initial_index_list_capacity,
            metadata_staging_pending_buffers: vec![],
            metadata_staging_free_buffers: vec![],
            last_frame_statistics: None,
        }
    }

    fn get_or_create_staging_buffer(&mut self, render_device: &RenderDevice) -> Buffer {
        let staging_buffer = self.metadata_staging_free_buffers.pop().unwrap_or_else(|| {
            render_device.create_buffer(&BufferDescriptor {
                label: Some("clustering metadata staging buffer"),
                size: ClusterMetadata::min_size().into(),
                usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        self.metadata_staging_pending_buffers
            .push(staging_buffer.clone());
        staging_buffer
    }

    fn update_from_metadata(&mut self, gpu_clustering_metadata: &ClusterMetadata) {

        if self.z_slice_list_capacity
            < gpu_clustering_metadata.indirect_draw_params.instance_count as usize
        {
            let new_capacity = gpu_clustering_metadata
                .indirect_draw_params
                .instance_count
                .next_power_of_two();
            warn!(
                "Resizing the view clustering Z slice list from a capacity of {0} elements to \
                a capacity of {1} elements. The scene lighting may have been corrupted for a \
                few frames. To avoid this, set the `gpu_clustering.z_slice_list_capacity` field \
                on the `GlobalClusterSettings` resource to at least {1}.",
                self.z_slice_list_capacity, new_capacity
            );
            self.z_slice_list_capacity = new_capacity as usize;
        }

        if self.max_index_list_capacity < gpu_clustering_metadata.index_list_capacity as usize {
            let new_capacity = gpu_clustering_metadata
                .index_list_capacity
                .next_power_of_two();
            warn!(
                "Resizing the view clustering index list from a capacity of {0} elements to a \
                capacity of {1} elements. The scene lighting may have been corrupted for a \
                few frames. To avoid this, set the `gpu_clustering.index_list_capacity` field on \
                the `GlobalClusterSettings` resource to at least {1}.",
                self.max_index_list_capacity, new_capacity
            );
            self.max_index_list_capacity = new_capacity as usize;
        }

        self.last_frame_statistics = Some(ViewClusteringLastFrameStatistics {
            index_list_size: gpu_clustering_metadata.index_list_capacity,
            farthest_z: f32::from_bits(sortable_u32_to_f32_bits(
                gpu_clustering_metadata.farthest_z,
            )),
        });
    }
}

fn sortable_u32_to_f32_bits(bits: u32) -> u32 {
    let mask = (!((bits as i32) >> 31)) as u32 | 0x80000000;
    bits ^ mask
}

#[derive(Resource)]
pub struct ClusteringRasterPipeline {

    pub bind_group_layout_count_pass: BindGroupLayoutDescriptor,

    pub bind_group_layout_populate_pass: BindGroupLayoutDescriptor,

    pub shader: Handle<Shader>,
}

#[derive(Resource)]
pub struct ClusteringZSlicingPipeline {

    pub bind_group_layout: BindGroupLayoutDescriptor,

    pub shader: Handle<Shader>,
}

#[derive(Resource)]
pub struct ClusteringAllocationPipeline {

    pub bind_group_layout: BindGroupLayoutDescriptor,

    pub shader: Handle<Shader>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClusteringRasterPipelineKey {

    populate_pass: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClusteringAllocationPipelineKey {

    global_pass: bool,
}

impl FromWorld for ClusteringRasterPipeline {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();

        let mut bind_group_layout_entries_count_pass = vec![

            binding_types::storage_buffer_read_only::<ClusterableObjectZSlice>(false)
                .build(0, ShaderStages::VERTEX_FRAGMENT),

            binding_types::storage_buffer::<GpuClusterableObjectIndexListsStorage>(false)
                .build(1, ShaderStages::FRAGMENT),

            binding_types::storage_buffer_read_only::<GpuClusteredLight>(false)
                .build(2, ShaderStages::VERTEX_FRAGMENT),

            binding_types::uniform_buffer::<LightProbesUniform>(true)
                .build(3, ShaderStages::VERTEX_FRAGMENT),

            binding_types::storage_buffer_read_only::<RenderClusteredDecal>(false)
                .build(4, ShaderStages::VERTEX_FRAGMENT),

            binding_types::uniform_buffer::<GpuLights>(true)
                .build(5, ShaderStages::VERTEX_FRAGMENT),

            binding_types::uniform_buffer::<ViewUniform>(true)
                .build(6, ShaderStages::VERTEX_FRAGMENT),
        ];

        let mut bind_group_layout_entries_populate_pass =
            bind_group_layout_entries_count_pass.clone();

        bind_group_layout_entries_count_pass.push(
            binding_types::storage_buffer::<GpuClusterOffsetsAndCountsStorage>(false)
                .build(7, ShaderStages::FRAGMENT),
        );

        bind_group_layout_entries_populate_pass.push(
            binding_types::storage_buffer_read_only::<GpuClusterOffsetsAndCountsStorage>(false)
                .build(7, ShaderStages::FRAGMENT),
        );

        bind_group_layout_entries_populate_pass.push(
            binding_types::storage_buffer::<GpuClusterOffsetsAndCountsStorage>(false)
                .build(8, ShaderStages::FRAGMENT),
        );

        let bind_group_layout_count_pass = BindGroupLayoutDescriptor::new(
            "clustering count pass bind group layout",
            &bind_group_layout_entries_count_pass,
        );
        let bind_group_layout_populate_pass = BindGroupLayoutDescriptor::new(
            "clustering populate pass bind group layout",
            &bind_group_layout_entries_populate_pass,
        );

        let shader = load_embedded_asset!(asset_server, "cluster_raster.wgsl");

        ClusteringRasterPipeline {
            bind_group_layout_count_pass,
            bind_group_layout_populate_pass,
            shader,
        }
    }
}

impl SpecializedRenderPipeline for ClusteringRasterPipeline {
    type Key = ClusteringRasterPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        let mut fragment_shader_defs = vec![];
        if key.populate_pass {
            fragment_shader_defs.push(ShaderDefVal::from("POPULATE_PASS"));
        } else {
            fragment_shader_defs.push(ShaderDefVal::from("COUNT_PASS"));
        }

        let mut vertex_shader_defs = fragment_shader_defs.clone();
        vertex_shader_defs.push(ShaderDefVal::from("VERTEX_SHADER"));

        RenderPipelineDescriptor {
            label: if key.populate_pass {
                Some("clustering populate pipeline".into())
            } else {
                Some("clustering count pipeline".into())
            },
            layout: vec![if key.populate_pass {
                self.bind_group_layout_populate_pass.clone()
            } else {
                self.bind_group_layout_count_pass.clone()
            }],
            immediate_size: 0,
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: vertex_shader_defs,
                entry_point: Some("vertex_main".into()),
                buffers: vec![VertexBufferLayout {
                    array_stride: size_of::<Vec2>() as u64,
                    step_mode: VertexStepMode::Vertex,
                    attributes: vec![VertexAttribute {
                        format: VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: fragment_shader_defs,
                entry_point: Some("fragment_main".into()),
                targets: vec![Some(ColorTargetState {
                    format: TextureFormat::R8Unorm,
                    blend: None,

                    write_mask: ColorWrites::empty(),
                })],
            }),
            ..default()
        }
    }
}

impl FromWorld for ClusteringZSlicingPipeline {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();

        let bind_group_layout = BindGroupLayoutDescriptor::new(
            "clustering Z slicing pass bind group layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (

                    binding_types::storage_buffer::<ClusterMetadata>(false),

                    binding_types::storage_buffer::<ClusterableObjectZSlice>(false),

                    binding_types::storage_buffer_read_only::<GpuClusteredLight>(false),

                    binding_types::uniform_buffer::<LightProbesUniform>(true),

                    binding_types::storage_buffer_read_only::<RenderClusteredDecal>(false),

                    binding_types::uniform_buffer::<GpuLights>(true),

                    binding_types::uniform_buffer::<ViewUniform>(true),
                ),
            ),
        );

        let shader = load_embedded_asset!(asset_server, "cluster_z_slice.wgsl");

        ClusteringZSlicingPipeline {
            bind_group_layout,
            shader,
        }
    }
}

impl SpecializedComputePipeline for ClusteringZSlicingPipeline {
    type Key = ();

    fn specialize(&self, _: Self::Key) -> ComputePipelineDescriptor {
        ComputePipelineDescriptor {
            label: Some("clustering Z slicing pipeline".into()),
            layout: vec![self.bind_group_layout.clone()],
            shader: self.shader.clone(),
            shader_defs: vec![],
            entry_point: Some("z_slice_main".into()),
            zero_initialize_workgroup_memory: true,
            ..default()
        }
    }
}

impl FromWorld for ClusteringAllocationPipeline {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();

        let bind_group_layout = BindGroupLayoutDescriptor::new(
            "clustering allocation pass bind group layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (

                    binding_types::storage_buffer::<GpuClusterOffsetsAndCountsStorage>(false),

                    binding_types::uniform_buffer::<GpuLights>(true),

                    binding_types::storage_buffer::<ClusterMetadata>(false),

                    binding_types::storage_buffer::<GpuClusterOffsetsAndCountsStorage>(false),
                ),
            ),
        );

        let shader = load_embedded_asset!(asset_server, "cluster_allocate.wgsl");

        ClusteringAllocationPipeline {
            bind_group_layout,
            shader,
        }
    }
}

impl SpecializedComputePipeline for ClusteringAllocationPipeline {
    type Key = ClusteringAllocationPipelineKey;

    fn specialize(&self, key: Self::Key) -> ComputePipelineDescriptor {
        ComputePipelineDescriptor {
            label: if key.global_pass {
                Some("clustering allocation global pass pipeline".into())
            } else {
                Some("clustering allocation local pass pipeline".into())
            },
            layout: vec![self.bind_group_layout.clone()],
            shader: self.shader.clone(),
            shader_defs: vec![],
            entry_point: if key.global_pass {
                Some("allocate_global_main".into())
            } else {
                Some("allocate_local_main".into())
            },
            zero_initialize_workgroup_memory: true,
            ..default()
        }
    }
}

static GPU_CLUSTERING_VERTICES: [Vec2; 4] = [
    vec2(0.0, 0.0),
    vec2(1.0, 0.0),
    vec2(0.0, 1.0),
    vec2(1.0, 1.0),
];

static GPU_CLUSTERING_INDICES: [u32; 6] = [0, 1, 2, 1, 3, 2];

#[derive(Resource)]
struct GpuClusteringMeshBuffers {

    vertex_buffer: Buffer,

    index_buffer: Buffer,
}

impl FromWorld for GpuClusteringMeshBuffers {
    fn from_world(world: &mut World) -> Self {
        let render_device = world.resource::<RenderDevice>();
        GpuClusteringMeshBuffers {
            vertex_buffer: render_device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("GPU clustering vertex buffer"),
                contents: bytemuck::bytes_of(&GPU_CLUSTERING_VERTICES),
                usage: BufferUsages::COPY_DST | BufferUsages::VERTEX,
            }),
            index_buffer: render_device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("GPU clustering index buffer"),
                contents: bytemuck::bytes_of(&GPU_CLUSTERING_INDICES),
                usage: BufferUsages::COPY_DST | BufferUsages::INDEX,
            }),
        }
    }
}

#[derive(Component)]
pub struct ViewGpuClusteringPipelineIds {

    clustering_z_slicing_pipeline_id: CachedComputePipelineId,

    clustering_count_pipeline_id: CachedRenderPipelineId,

    clustering_allocation_local_pipeline_id: CachedComputePipelineId,

    clustering_allocation_global_pipeline_id: CachedComputePipelineId,

    clustering_populate_pipeline_id: CachedRenderPipelineId,
}

fn cluster_on_gpu(
    view_query: ViewQuery<(
        &MainEntity,
        Option<&ViewGpuClusteringBuffers>,
        Option<&ViewGpuClusteringPipelineIds>,
        Option<&ViewClusteringDummyTexture>,
        Option<&ViewClusteringBindGroups>,
        Option<&ViewLightProbesUniformOffset>,
        Option<&ViewLightsUniformOffset>,
        Option<&ViewUniformOffset>,
        Option<&ExtractedClusterConfig>,
    )>,
    pipeline_cache: Res<PipelineCache>,
    clustering_mesh_buffers: Res<GpuClusteringMeshBuffers>,
    render_view_clustering_readback_data: Res<RenderViewClusteringReadbackData>,
    mut render_context: RenderContext,
) {
    let (
        view_main_entity,
        Some(view_gpu_clustering_buffers),
        Some(view_gpu_clustering_pipeline_ids),
        Some(view_clustering_dummy_texture),
        Some(view_clustering_bind_groups),
        Some(view_light_probes_uniform_offset),
        Some(view_lights_uniform_offset),
        Some(view_uniform_offset),
        Some(extracted_cluster_config),
    ) = view_query.into_inner()
    else {
        trace!("Failed to match view query; not clustering");
        return;
    };

    let Some(view_clustering_readback_data) = render_view_clustering_readback_data
        .views
        .get(view_main_entity)
    else {
        return;
    };

    let (
        Some(clustering_z_slicing_compute_pipeline),
        Some(clustering_count_render_pipeline),
        Some(clustering_allocate_local_compute_pipeline),
        Some(clustering_allocate_global_compute_pipeline),
        Some(clustering_populate_render_pipeline),
    ) = (
        pipeline_cache.get_compute_pipeline(
            view_gpu_clustering_pipeline_ids.clustering_z_slicing_pipeline_id,
        ),
        pipeline_cache
            .get_render_pipeline(view_gpu_clustering_pipeline_ids.clustering_count_pipeline_id),
        pipeline_cache.get_compute_pipeline(
            view_gpu_clustering_pipeline_ids.clustering_allocation_local_pipeline_id,
        ),
        pipeline_cache.get_compute_pipeline(
            view_gpu_clustering_pipeline_ids.clustering_allocation_global_pipeline_id,
        ),
        pipeline_cache
            .get_render_pipeline(view_gpu_clustering_pipeline_ids.clustering_populate_pipeline_id),
    )
    else {
        trace!("One or more clustering pipelines not found; not clustering");
        return;
    };

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let time_span = diagnostics.time_span(render_context.command_encoder(), "clustering");

    let Ok(staging_buffer) = view_clustering_readback_data
        .lock()
        .map(|mut data| data.get_or_create_staging_buffer(render_context.render_device()))
    else {
        error!("Failed to fetch staging buffer; not clustering.");
        return;
    };

    let command_encoder = render_context.command_encoder();
    command_encoder.push_debug_group("clustering");

    run_clustering_z_slicing_pass(
        command_encoder,
        clustering_z_slicing_compute_pipeline,
        &view_clustering_bind_groups.clustering_bind_group_z_slicing_pass,
        &view_gpu_clustering_buffers.cluster_metadata_buffer,
        view_light_probes_uniform_offset,
        view_lights_uniform_offset,
        view_uniform_offset,
    );

    run_clustering_rasterization_pass(
        command_encoder,
        clustering_count_render_pipeline,
        &view_clustering_bind_groups.clustering_bind_group_count_pass,
        view_gpu_clustering_buffers,
        view_light_probes_uniform_offset,
        view_lights_uniform_offset,
        view_uniform_offset,
        view_clustering_dummy_texture,
        extracted_cluster_config,
        &clustering_mesh_buffers,
        false,
    );

    run_clustering_allocation_pass(
        command_encoder,
        clustering_allocate_local_compute_pipeline,
        view_clustering_bind_groups,
        view_lights_uniform_offset,
        extracted_cluster_config,
        false,
    );

    run_clustering_allocation_pass(
        command_encoder,
        clustering_allocate_global_compute_pipeline,
        view_clustering_bind_groups,
        view_lights_uniform_offset,
        extracted_cluster_config,
        true,
    );

    run_clustering_rasterization_pass(
        command_encoder,
        clustering_populate_render_pipeline,
        &view_clustering_bind_groups.clustering_bind_group_populate_pass,
        view_gpu_clustering_buffers,
        view_light_probes_uniform_offset,
        view_lights_uniform_offset,
        view_uniform_offset,
        view_clustering_dummy_texture,
        extracted_cluster_config,
        &clustering_mesh_buffers,
        true,
    );

    schedule_readback_staging(
        command_encoder,
        view_gpu_clustering_buffers,
        &staging_buffer,
    );
    schedule_readback_buffer_map(
        command_encoder,
        view_clustering_readback_data.clone(),
        &staging_buffer,
    );

    command_encoder.pop_debug_group();
    time_span.end(render_context.command_encoder());

    fn run_clustering_z_slicing_pass(
        command_encoder: &mut CommandEncoder,
        clustering_z_slicing_pipeline: &ComputePipeline,
        clustering_z_slicing_bind_group: &BindGroup,
        clustering_cluster_metadata_buffer: &StorageBuffer<ClusterMetadata>,
        view_light_probes_uniform_offset: &ViewLightProbesUniformOffset,
        view_lights_uniform_offset: &ViewLightsUniformOffset,
        view_uniform_offset: &ViewUniformOffset,
    ) {
        let mut compute_pass = command_encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("clustering Z slicing pass"),
            ..default()
        });
        compute_pass.set_pipeline(clustering_z_slicing_pipeline);
        compute_pass.set_bind_group(
            0,
            Some(&**clustering_z_slicing_bind_group),
            &[
                **view_light_probes_uniform_offset,
                view_lights_uniform_offset.offset,
                view_uniform_offset.offset,
            ],
        );

        let clustering_cluster_metadata = clustering_cluster_metadata_buffer.get();
        let clusterable_object_count = clustering_cluster_metadata.clustered_light_count
            + clustering_cluster_metadata.reflection_probe_count
            + clustering_cluster_metadata.irradiance_volume_count
            + clustering_cluster_metadata.decal_count;

        let workgroup_count = clusterable_object_count.div_ceil(Z_SLICING_WORKGROUP_SIZE);
        compute_pass.dispatch_workgroups(workgroup_count, 1, 1);
    }

    fn run_clustering_rasterization_pass(
        command_encoder: &mut CommandEncoder,
        clustering_render_pipeline: &RenderPipeline,
        clustering_bind_group: &BindGroup,
        view_gpu_clustering_buffers: &ViewGpuClusteringBuffers,
        view_light_probes_uniform_offset: &ViewLightProbesUniformOffset,
        view_lights_uniform_offset: &ViewLightsUniformOffset,
        view_uniform_offset: &ViewUniformOffset,
        view_clustering_dummy_texture: &ViewClusteringDummyTexture,
        extracted_cluster_config: &ExtractedClusterConfig,
        clustering_mesh_buffers: &GpuClusteringMeshBuffers,
        populate_pass: bool,
    ) {
        let Some(cluster_metadata_buffer) =
            view_gpu_clustering_buffers.cluster_metadata_buffer.buffer()
        else {
            error!("Z slicing metadata buffer was never uploaded");
            return;
        };

        let mut render_pass = command_encoder.begin_render_pass(&RenderPassDescriptor {
            label: if populate_pass {
                Some("clustering populate pass")
            } else {
                Some("clustering count pass")
            },
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &view_clustering_dummy_texture.default_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {

                    load: LoadOp::Clear(Color::BLACK.to_linear().into()),
                    store: StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: None,
            ..default()
        });
        render_pass.set_pipeline(clustering_render_pipeline);
        render_pass.set_bind_group(
            0,
            Some(&**clustering_bind_group),
            &[
                **view_light_probes_uniform_offset,
                view_lights_uniform_offset.offset,
                view_uniform_offset.offset,
            ],
        );

        render_pass.set_viewport(
            0.0,
            0.0,
            extracted_cluster_config.dimensions.x as f32,
            extracted_cluster_config.dimensions.y as f32,
            0.0,
            1.0,
        );

        render_pass.set_vertex_buffer(0, *clustering_mesh_buffers.vertex_buffer.slice(..));
        render_pass.set_index_buffer(
            *clustering_mesh_buffers.index_buffer.slice(..),
            IndexFormat::Uint32,
        );
        render_pass.draw_indexed_indirect(cluster_metadata_buffer, 0);
    }

    fn run_clustering_allocation_pass(
        command_encoder: &mut CommandEncoder,
        clustering_allocation_pipeline: &ComputePipeline,
        view_clustering_bind_groups: &ViewClusteringBindGroups,
        view_lights_uniform_offset: &ViewLightsUniformOffset,
        extracted_cluster_config: &ExtractedClusterConfig,
        global_pass: bool,
    ) {
        let mut compute_pass = command_encoder.begin_compute_pass(&ComputePassDescriptor {
            label: if global_pass {
                Some("clustering allocation global pass")
            } else {
                Some("clustering allocation local pass")
            },
            ..default()
        });
        compute_pass.set_pipeline(clustering_allocation_pipeline);
        compute_pass.set_bind_group(
            0,
            Some(&*view_clustering_bind_groups.clustering_bind_group_allocate_pass),
            &[view_lights_uniform_offset.offset],
        );

        let workgroup_count = if global_pass {
            1
        } else {
            extracted_cluster_config
                .dimensions
                .element_product()
                .div_ceil(ALLOCATION_WORKGROUP_SIZE)
        };
        compute_pass.dispatch_workgroups(workgroup_count, 1, 1);
    }

    fn schedule_readback_staging(
        command_encoder: &mut CommandEncoder,
        view_gpu_clustering_buffers: &ViewGpuClusteringBuffers,
        staging_buffer: &Buffer,
    ) {
        match view_gpu_clustering_buffers.cluster_metadata_buffer.buffer() {
            None => {

                error!("No clustering Z slicing metadata buffer found");
            }
            Some(metadata_buffer) => {

                command_encoder.copy_buffer_to_buffer(
                    metadata_buffer,
                    0,
                    staging_buffer,
                    0,
                    Some(u64::from(ClusterMetadata::min_size())),
                );
            }
        }
    }

    fn schedule_readback_buffer_map(
        command_encoder: &mut CommandEncoder,
        view_clustering_readback_data: Arc<Mutex<ViewClusteringReadbackData>>,
        staging_buffer: &Buffer,
    ) {
        let captured_staging_buffer = staging_buffer.clone();
        command_encoder.map_buffer_on_submit(staging_buffer, MapMode::Read, .., move |result| {
            if result.is_err() {
                return;
            };

            let mut view_clustering_readback_data = view_clustering_readback_data.lock().unwrap();

            {

                let buffer_view = captured_staging_buffer.slice(..).get_mapped_range();
                let Ok(mut buffer_reader) =
                    Reader::new::<ClusterMetadata>(buffer_view[..].to_vec(), 0)
                else {
                    return;
                };
                let gpu_clustering_metadata = ClusterMetadata::create_from(&mut buffer_reader);

                view_clustering_readback_data.update_from_metadata(&gpu_clustering_metadata);
            }

            captured_staging_buffer.unmap();

            view_clustering_readback_data
                .metadata_staging_free_buffers
                .push(captured_staging_buffer);
        });
    }
}

fn prepare_clustering_bind_groups(
    mut commands: Commands,
    views_query: Query<
        (Entity, &ViewGpuClusteringBuffers, &ViewClusterBindings),
        With<ExtractedView>,
    >,
    render_device: Res<RenderDevice>,
    clustering_z_slicing_pipeline: Res<ClusteringZSlicingPipeline>,
    clustering_raster_pipeline: Res<ClusteringRasterPipeline>,
    clustering_allocation_pipeline: Res<ClusteringAllocationPipeline>,
    global_clusterable_object_meta: Res<GlobalClusterableObjectMeta>,
    pipeline_cache: Res<PipelineCache>,
    light_probes_buffer: Res<LightProbesBuffer>,
    decals_buffer: Res<DecalsBuffer>,
    light_meta: Res<LightMeta>,
    view_uniforms: Res<ViewUniforms>,
) {
    let (
        Some(gpu_clustered_lights_binding),
        Some(light_probes_binding),
        Some(decals_buffer),
        Some(lights_binding),
        Some(view_binding),
    ) = (
        global_clusterable_object_meta
            .gpu_clustered_lights
            .binding(),
        light_probes_buffer.binding(),
        decals_buffer.buffer(),
        light_meta.view_gpu_lights.binding(),
        view_uniforms.uniforms.binding(),
    )
    else {
        return;
    };

    for (view_entity, view_gpu_clustering_buffers, view_cluster_bindings) in &views_query {
        let ViewClusterBuffers::Storage {
            clusterable_object_index_lists: ref maybe_clusterable_object_index_lists,
            cluster_offsets_and_counts: ref maybe_cluster_offsets_and_counts,
        } = view_cluster_bindings.buffers
        else {
            continue;
        };

        let (
            Some(z_slices_buffer),
            Some(cluster_metadata_buffer),
            Some(scratchpad_offsets_and_counts_buffer),
            Some(clusterable_object_index_lists),
            Some(cluster_offsets_and_counts),
        ) = (
            view_gpu_clustering_buffers.z_slices_buffer.buffer(),
            view_gpu_clustering_buffers.cluster_metadata_buffer.buffer(),
            view_gpu_clustering_buffers
                .scratchpad_offsets_and_counts_buffer
                .buffer(),
            maybe_clusterable_object_index_lists.buffer(),
            maybe_cluster_offsets_and_counts.buffer(),
        )
        else {
            continue;
        };

        let clustering_bind_group_entries_z_slicing_pass = [

            BindGroupEntry {
                binding: 0,
                resource: cluster_metadata_buffer.as_entire_binding(),
            },

            BindGroupEntry {
                binding: 1,
                resource: z_slices_buffer.as_entire_binding(),
            },

            BindGroupEntry {
                binding: 2,
                resource: gpu_clustered_lights_binding.clone(),
            },

            BindGroupEntry {
                binding: 3,
                resource: light_probes_binding.clone(),
            },

            BindGroupEntry {
                binding: 4,
                resource: decals_buffer.as_entire_binding(),
            },

            BindGroupEntry {
                binding: 5,
                resource: lights_binding.clone(),
            },

            BindGroupEntry {
                binding: 6,
                resource: view_binding.clone(),
            },
        ];

        let mut clustering_bind_group_entries_count_pass: Vec<BindGroupEntry> = vec![

            BindGroupEntry {
                binding: 0,
                resource: z_slices_buffer.as_entire_binding(),
            },

            BindGroupEntry {
                binding: 1,
                resource: clusterable_object_index_lists.as_entire_binding(),
            },

            BindGroupEntry {
                binding: 2,
                resource: gpu_clustered_lights_binding.clone(),
            },

            BindGroupEntry {
                binding: 3,
                resource: light_probes_binding.clone(),
            },

            BindGroupEntry {
                binding: 4,
                resource: decals_buffer.as_entire_binding(),
            },

            BindGroupEntry {
                binding: 5,
                resource: lights_binding.clone(),
            },

            BindGroupEntry {
                binding: 6,
                resource: view_binding.clone(),
            },
        ];

        let mut clustering_bind_group_entries_populate_pass =
            clustering_bind_group_entries_count_pass.clone();

        clustering_bind_group_entries_count_pass.push(

            BindGroupEntry {
                binding: 7,
                resource: cluster_offsets_and_counts.as_entire_binding(),
            },
        );

        clustering_bind_group_entries_populate_pass.push(

            BindGroupEntry {
                binding: 7,
                resource: cluster_offsets_and_counts.as_entire_binding(),
            },
        );
        clustering_bind_group_entries_populate_pass.push(

            BindGroupEntry {
                binding: 8,
                resource: scratchpad_offsets_and_counts_buffer.as_entire_binding(),
            },
        );

        let clustering_bind_group_entries_allocation_pass: [BindGroupEntry; _] = [

            BindGroupEntry {
                binding: 0,
                resource: cluster_offsets_and_counts.as_entire_binding(),
            },

            BindGroupEntry {
                binding: 1,
                resource: lights_binding.clone(),
            },

            BindGroupEntry {
                binding: 2,
                resource: cluster_metadata_buffer.as_entire_binding(),
            },

            BindGroupEntry {
                binding: 3,
                resource: scratchpad_offsets_and_counts_buffer.as_entire_binding(),
            },
        ];

        let clustering_bind_group_z_slicing_pass = render_device.create_bind_group(
            "clustering Z slicing pass bind group",
            &pipeline_cache.get_bind_group_layout(&clustering_z_slicing_pipeline.bind_group_layout),
            &clustering_bind_group_entries_z_slicing_pass,
        );
        let clustering_bind_group_count_pass = render_device.create_bind_group(
            "clustering count pass bind group",
            &pipeline_cache
                .get_bind_group_layout(&clustering_raster_pipeline.bind_group_layout_count_pass),
            &clustering_bind_group_entries_count_pass,
        );
        let clustering_bind_group_allocate_pass = render_device.create_bind_group(
            "clustering allocate pass bind group",
            &pipeline_cache
                .get_bind_group_layout(&clustering_allocation_pipeline.bind_group_layout),
            &clustering_bind_group_entries_allocation_pass,
        );
        let clustering_bind_group_populate_pass = render_device.create_bind_group(
            "clustering populate pass bind group",
            &pipeline_cache
                .get_bind_group_layout(&clustering_raster_pipeline.bind_group_layout_populate_pass),
            &clustering_bind_group_entries_populate_pass,
        );

        commands
            .entity(view_entity)
            .insert(ViewClusteringBindGroups {
                clustering_bind_group_z_slicing_pass,
                clustering_bind_group_count_pass,
                clustering_bind_group_allocate_pass,
                clustering_bind_group_populate_pass,
            });
    }
}

fn prepare_cluster_dummy_textures(
    mut commands: Commands,
    views_query: Query<(Entity, &ExtractedClusterConfig), With<ExtractedView>>,
    render_device: Res<RenderDevice>,
    mut texture_cache: ResMut<TextureCache>,
) {
    for (view_entity, view_cluster_config) in &views_query {
        let dummy_texture = texture_cache.get(
            &render_device,
            TextureDescriptor {
                label: Some("clustering dummy texture"),

                size: Extent3d {
                    width: view_cluster_config.dimensions.x.next_multiple_of(32),
                    height: view_cluster_config.dimensions.y.next_multiple_of(32),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::R8Unorm,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_DST,
                view_formats: &[],
            },
        );
        commands
            .entity(view_entity)
            .insert(ViewClusteringDummyTexture(dummy_texture));
    }
}

fn prepare_clustering_pipelines(
    mut commands: Commands,
    views_query: Query<Entity, With<ExtractedView>>,
    pipeline_cache: Res<PipelineCache>,
    mut clustering_z_slicing_pipelines: ResMut<
        SpecializedComputePipelines<ClusteringZSlicingPipeline>,
    >,
    mut clustering_raster_pipelines: ResMut<SpecializedRenderPipelines<ClusteringRasterPipeline>>,
    mut clustering_allocation_pipelines: ResMut<
        SpecializedComputePipelines<ClusteringAllocationPipeline>,
    >,
    clustering_z_slicing_pipeline: Res<ClusteringZSlicingPipeline>,
    clustering_raster_pipeline: Res<ClusteringRasterPipeline>,
    clustering_allocation_pipeline: Res<ClusteringAllocationPipeline>,
) {
    for view_entity in &views_query {
        let clustering_z_slicing_pipeline_id = clustering_z_slicing_pipelines.specialize(
            &pipeline_cache,
            &clustering_z_slicing_pipeline,
            (),
        );
        let clustering_count_pipeline_id = clustering_raster_pipelines.specialize(
            &pipeline_cache,
            &clustering_raster_pipeline,
            ClusteringRasterPipelineKey {
                populate_pass: false,
            },
        );
        let clustering_local_allocation_pipeline_id = clustering_allocation_pipelines.specialize(
            &pipeline_cache,
            &clustering_allocation_pipeline,
            ClusteringAllocationPipelineKey { global_pass: false },
        );
        let clustering_global_allocation_pipeline_id = clustering_allocation_pipelines.specialize(
            &pipeline_cache,
            &clustering_allocation_pipeline,
            ClusteringAllocationPipelineKey { global_pass: true },
        );
        let clustering_populate_pipeline_id = clustering_raster_pipelines.specialize(
            &pipeline_cache,
            &clustering_raster_pipeline,
            ClusteringRasterPipelineKey {
                populate_pass: true,
            },
        );

        commands
            .entity(view_entity)
            .insert(ViewGpuClusteringPipelineIds {
                clustering_z_slicing_pipeline_id,
                clustering_count_pipeline_id,
                clustering_allocation_local_pipeline_id: clustering_local_allocation_pipeline_id,
                clustering_allocation_global_pipeline_id: clustering_global_allocation_pipeline_id,
                clustering_populate_pipeline_id,
            });
    }
}

fn upload_view_gpu_clustering_buffers(
    mut views_query: Query<&mut ViewGpuClusteringBuffers>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    for mut view_gpu_clustering_buffers in &mut views_query {
        view_gpu_clustering_buffers
            .z_slices_buffer
            .write_buffer(&render_device);

        view_gpu_clustering_buffers
            .cluster_metadata_buffer
            .write_buffer(&render_device, &render_queue);

        if view_gpu_clustering_buffers
            .scratchpad_offsets_and_counts_buffer
            .is_empty()
        {
            view_gpu_clustering_buffers
                .scratchpad_offsets_and_counts_buffer
                .add();
        }
        view_gpu_clustering_buffers
            .scratchpad_offsets_and_counts_buffer
            .write_buffer(&render_device);
    }
}

pub fn extract_clusters_for_gpu_clustering(
    mut commands: Commands,
    mut main_world: ResMut<MainWorld>,
    render_view_clustering_index_list_sizes: Res<RenderViewClusteringReadbackData>,
) {
    let mut views = main_world.query::<(Entity, RenderEntity, &mut Clusters, &Camera)>();

    for (main_view_entity, render_view_entity, mut clusters, camera) in
        views.iter_mut(&mut main_world)
    {
        let mut entity_commands = commands
            .get_entity(render_view_entity)
            .expect("Clusters entity wasn't synced.");
        if !camera.is_active {
            entity_commands.remove::<ExtractedClusterConfig>();
            continue;
        }

        entity_commands.insert(ExtractedClusterConfig::from(&*clusters));

        if let Some(view_clustering_buffer_size_data) = render_view_clustering_index_list_sizes
            .views
            .get(&MainEntity::from(main_view_entity))
        {
            let view_clustering_buffer_size_data = view_clustering_buffer_size_data.lock().unwrap();
            if let Some(last_frame_statistics) =
                &view_clustering_buffer_size_data.last_frame_statistics
            {
                clusters.last_frame_farthest_z = Some(last_frame_statistics.farthest_z);
                clusters.last_frame_total_cluster_index_count =
                    Some(last_frame_statistics.index_list_size as usize);
            }
        }
    }

    let global_cluster_settings = main_world.resource::<GlobalClusterSettings>();
    commands.insert_resource(global_cluster_settings.clone());
}

pub(crate) fn prepare_clusters_for_gpu_clustering(
    mut commands: Commands,
    views_query: Query<(
        Entity,
        &MainEntity,
        &ExtractedClusterConfig,
        Option<&RenderViewLightProbes<EnvironmentMapLight>>,
        Option<&RenderViewLightProbes<IrradianceVolume>>,
    )>,
    render_clustered_decals: Res<RenderClusteredDecals>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    global_clusterable_object_meta: Res<GlobalClusterableObjectMeta>,
    global_cluster_settings: Res<GlobalClusterSettings>,
    mut render_view_clustering_index_list_sizes: ResMut<RenderViewClusteringReadbackData>,
) {
    let render_device = render_device.into_inner();

    let Some(ref global_cluster_settings_gpu) = global_cluster_settings.gpu_clustering else {
        error!("`prepare_clusters_for_gpu_clustering() called when not GPU clustering");
        return;
    };

    let gpu_clustered_lights_storage = &global_clusterable_object_meta.gpu_clustered_lights;

    let mut all_view_main_entities = MainEntityHashSet::default();

    for (
        view_entity,
        view_main_entity,
        extracted_cluster_config,
        maybe_environment_maps,
        maybe_irradiance_volumes,
    ) in &views_query
    {

        let mut view_clusters_bindings =
            ViewClusterBindings::new(BufferBindingType::Storage { read_only: false });
        view_clusters_bindings.clear();
        let cluster_count = extracted_cluster_config.dimensions.x as usize
            * extracted_cluster_config.dimensions.y as usize
            * extracted_cluster_config.dimensions.z as usize;
        view_clusters_bindings.reserve_clusters(cluster_count);

        all_view_main_entities.insert(*view_main_entity);

        let Ok(view_clustering_buffer_size_data) = render_view_clustering_index_list_sizes
            .views
            .entry(*view_main_entity)
            .or_insert_with(|| {
                Arc::new(Mutex::new(ViewClusteringReadbackData::new(
                    global_cluster_settings_gpu,
                )))
            })
            .lock()
        else {
            warn!("Failed to acquire lock for view clustering buffer size data; skipping buffer creation for view: {}", view_entity.to_bits());
            continue;
        };

        let mut view_gpu_clustering_buffers = ViewGpuClusteringBuffers::new();

        let clustered_light_count = gpu_clustered_lights_storage.data.len() as u32;
        let reflection_probe_count = match maybe_environment_maps {
            Some(view_reflection_probes) => view_reflection_probes.len() as u32,
            None => 0,
        };
        let irradiance_volume_count = match maybe_irradiance_volumes {
            Some(view_irradiance_volumes) => view_irradiance_volumes.len() as u32,
            None => 0,
        };
        let decal_count = render_clustered_decals.len() as u32;

        *view_gpu_clustering_buffers
            .cluster_metadata_buffer
            .get_mut() = ClusterMetadata {
            indirect_draw_params: ClusterRasterIndirectDrawParams {
                index_count: 6,

                instance_count: 0,
                first_index: 0,
                base_vertex: 0,
                first_instance: 0,
            },
            clustered_light_count,
            reflection_probe_count,
            irradiance_volume_count,
            decal_count,
            index_list_capacity: view_clustering_buffer_size_data.max_index_list_capacity as u32,
            z_slice_list_capacity: view_clustering_buffer_size_data.z_slice_list_capacity as u32,
            farthest_z: 0,
        };

        if view_gpu_clustering_buffers.z_slices_buffer.len()
            < view_clustering_buffer_size_data.z_slice_list_capacity
        {
            view_gpu_clustering_buffers.z_slices_buffer.add_multiple(
                view_clustering_buffer_size_data.z_slice_list_capacity
                    - view_gpu_clustering_buffers.z_slices_buffer.len(),
            );
        }

        view_clusters_bindings
            .reserve_indices(view_clustering_buffer_size_data.max_index_list_capacity);
        view_clusters_bindings.write_buffers(render_device, &render_queue);

        view_gpu_clustering_buffers
            .scratchpad_offsets_and_counts_buffer
            .add_multiple(cluster_count);

        commands
            .entity(view_entity)
            .insert((view_clusters_bindings, view_gpu_clustering_buffers));
    }

    render_view_clustering_index_list_sizes
        .views
        .retain(|view_main_entity, _| all_view_main_entities.contains(view_main_entity));
}

impl ExtractResource<GpuClusteringPlugin> for GlobalClusterSettings {
    type Source = GlobalClusterSettings;

    fn extract_resource(source: &Self::Source) -> Self {
        source.clone()
    }
}
