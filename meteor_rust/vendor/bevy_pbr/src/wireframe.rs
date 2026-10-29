use crate::{
    collect_meshes_for_gpu_building,
    render::{PreprocessBindGroups, PreprocessPipelines},
    set_mesh_motion_vector_flags, DrawMesh, MeshPipeline, MeshPipelineKey, MeshPipelineSystems,
    RenderLightmaps, RenderMeshInstanceFlags, RenderMeshInstances, SetMeshBindGroup,
    SetMeshViewBindGroup, SetMeshViewBindingArrayBindGroup, ViewKeyCache,
};
use bevy_app::{App, Plugin, PostUpdate, Startup};
use bevy_asset::{
    embedded_asset, load_embedded_asset, prelude::AssetChanged, AsAssetId, Asset, AssetApp,
    AssetEventSystems, AssetId, AssetServer, Assets, Handle, UntypedAssetId,
};
use bevy_camera::{visibility::ViewVisibility, Camera, Camera3d};
use bevy_color::{Color, ColorToComponents};
use bevy_core_pipeline::schedule::{Core3d, Core3dSystems};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    prelude::*,
    query::ROQueryItem,
    system::{lifetimeless::SRes, SystemParamItem},
};
use bevy_mesh::{Mesh, Mesh3d, MeshVertexBufferLayoutRef};
use bevy_platform::{
    collections::{HashMap, HashSet},
    hash::FixedHasher,
};
use bevy_reflect::{std_traits::ReflectDefault, Reflect};
use bevy_render::{
    batching::gpu_preprocessing::{
        GpuPreprocessingMode, GpuPreprocessingSupport, IndirectBatchSet, IndirectParametersBuffers,
        IndirectParametersNonIndexed,
    },
    camera::{
        DirtySpecializationSystems, DirtyWireframeSpecializations, ExtractedCamera, PendingQueues,
    },
    extract_resource::ExtractResource,
    mesh::{
        allocator::{MeshAllocator, MeshAllocatorSettings, MeshSlabs},
        RenderMesh, RenderMeshBufferInfo,
    },
    prelude::*,
    render_asset::{
        prepare_assets, PrepareAssetError, RenderAsset, RenderAssetPlugin, RenderAssets,
    },
    render_phase::{
        AddRenderCommand, BinnedPhaseItem, BinnedRenderPhasePlugin, BinnedRenderPhaseType,
        CachedRenderPipelinePhaseItem, DrawFunctionId, DrawFunctions, PhaseItem,
        PhaseItemBatchSetKey, PhaseItemExtraIndex, RenderCommand, RenderCommandResult,
        SetItemPipeline, TrackedRenderPass, ViewBinnedRenderPhases,
    },
    render_resource::{binding_types::*, *},
    renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
    sync_world::{MainEntity, MainEntityHashMap},
    view::{
        ExtractedView, NoIndirectDrawing, RenderVisibilityRanges, RenderVisibleEntities,
        RetainedViewEntity, ViewDepthTexture, ViewTarget,
    },
    Extract, GpuResourceAppExt, Render, RenderApp, RenderDebugFlags, RenderStartup, RenderSystems,
};
use bevy_shader::Shader;
use bytemuck::{Pod, Zeroable};
use core::{any::TypeId, hash::Hash, mem::size_of, ops::Range};
use tracing::{error, warn};

#[derive(Debug, Default)]
pub struct WireframePlugin {

    pub debug_flags: RenderDebugFlags,
}

impl WireframePlugin {

    pub fn new(debug_flags: RenderDebugFlags) -> Self {
        Self { debug_flags }
    }
}

impl Plugin for WireframePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "render/wireframe.wgsl");

        app.add_plugins((
            BinnedRenderPhasePlugin::<Wireframe3d, MeshPipeline>::new(self.debug_flags),
            RenderAssetPlugin::<RenderWireframeMaterial>::default(),
        ))
        .init_asset::<WireframeMaterial>()
        .init_resource::<WireframeEntitiesNeedingSpecialization>()
        .init_resource::<SpecializedMeshPipelines<Wireframe3dPipeline>>()
        .init_resource::<WireframeConfig>()
        .init_resource::<WireframeEntitiesNeedingSpecialization>()
        .register_type::<WireframeLineWidth>()
        .register_type::<WireframeTopology>()
        .add_systems(Startup, setup_global_wireframe_material)
        .add_systems(
            PostUpdate,
            (
                wireframe_config_changed.run_if(resource_changed::<WireframeConfig>),
                wireframe_color_changed,
                wireframe_line_width_changed,
                wireframe_topology_changed,

                (apply_wireframe_material, apply_global_wireframe_material).chain(),
            ),
        )
        .add_systems(
            PostUpdate,
            check_wireframe_entities_needing_specialization
                .after(AssetEventSystems)
                .run_if(resource_exists::<WireframeConfig>),
        );
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        let required_features = WgpuFeatures::POLYGON_MODE_LINE | WgpuFeatures::IMMEDIATES;
        let render_device = render_app.world().resource::<RenderDevice>();
        if !render_device.features().contains(required_features) {
            warn!(
                "WireframePlugin not loaded. GPU lacks support for required features: {:?}.",
                required_features
            );
            return;
        }

        render_app
            .world_mut()
            .resource_mut::<MeshAllocatorSettings>()
            .extra_buffer_usages |= BufferUsages::STORAGE;

        render_app
            .init_gpu_resource::<SpecializedWireframePipelineCache>()
            .init_resource::<DrawFunctions<Wireframe3d>>()
            .add_render_command::<Wireframe3d, DrawWireframe3dThin>()
            .add_render_command::<Wireframe3d, DrawWireframe3dWide>()
            .init_gpu_resource::<RenderWireframeInstances>()
            .init_gpu_resource::<WireframeWideBindGroups>()
            .init_gpu_resource::<SpecializedMeshPipelines<Wireframe3dPipeline>>()
            .init_gpu_resource::<PendingWireframeQueues>()
            .add_systems(
                RenderStartup,
                init_wireframe_3d_pipeline.after(MeshPipelineSystems),
            )
            .add_systems(
                Core3d,
                wireframe_3d
                    .after(Core3dSystems::MainPass)
                    .before(Core3dSystems::EarlyPostProcess),
            )
            .add_systems(
                ExtractSchedule,
                (
                    extract_wireframe_3d_camera,
                    extract_wireframe_entities_needing_specialization
                        .in_set(DirtySpecializationSystems::CheckForChanges),
                    extract_wireframe_entities_that_need_specializations_removed
                        .in_set(DirtySpecializationSystems::CheckForRemovals),
                    extract_wireframe_materials,
                ),
            )
            .add_systems(
                Render,
                (
                    specialize_wireframes
                        .in_set(RenderSystems::Specialize)
                        .after(prepare_assets::<RenderWireframeMaterial>)
                        .after(prepare_assets::<RenderMesh>)
                        .after(collect_meshes_for_gpu_building)
                        .after(set_mesh_motion_vector_flags),
                    prepare_wireframe_wide_bind_groups
                        .in_set(RenderSystems::PrepareBindGroups)
                        .after(prepare_assets::<RenderWireframeMaterial>)
                        .after(prepare_assets::<RenderMesh>),
                    queue_wireframes
                        .in_set(RenderSystems::QueueMeshes)
                        .after(prepare_assets::<RenderWireframeMaterial>),
                ),
            );
    }
}

#[derive(Component, Debug, Clone, Default, Reflect, Eq, PartialEq)]
#[reflect(Component, Default, Debug, PartialEq)]
pub struct Wireframe;

pub struct Wireframe3d {

    pub batch_set_key: Wireframe3dBatchSetKey,

    pub bin_key: Wireframe3dBinKey,

    pub representative_entity: (Entity, MainEntity),

    pub batch_range: Range<u32>,

    pub extra_index: PhaseItemExtraIndex,
}

impl PhaseItem for Wireframe3d {
    fn entity(&self) -> Entity {
        self.representative_entity.0
    }

    fn main_entity(&self) -> MainEntity {
        self.representative_entity.1
    }

    fn draw_function(&self) -> DrawFunctionId {
        self.batch_set_key.draw_function
    }

    fn batch_range(&self) -> &Range<u32> {
        &self.batch_range
    }

    fn batch_range_mut(&mut self) -> &mut Range<u32> {
        &mut self.batch_range
    }

    fn extra_index(&self) -> PhaseItemExtraIndex {
        self.extra_index.clone()
    }

    fn batch_range_and_extra_index_mut(&mut self) -> (&mut Range<u32>, &mut PhaseItemExtraIndex) {
        (&mut self.batch_range, &mut self.extra_index)
    }
}

impl CachedRenderPipelinePhaseItem for Wireframe3d {
    fn cached_pipeline(&self) -> CachedRenderPipelineId {
        self.batch_set_key.pipeline
    }
}

impl BinnedPhaseItem for Wireframe3d {
    type BinKey = Wireframe3dBinKey;
    type BatchSetKey = Wireframe3dBatchSetKey;

    fn new(
        batch_set_key: Self::BatchSetKey,
        bin_key: Self::BinKey,
        representative_entity: (Entity, MainEntity),
        batch_range: Range<u32>,
        extra_index: PhaseItemExtraIndex,
    ) -> Self {
        Self {
            batch_set_key,
            bin_key,
            representative_entity,
            batch_range,
            extra_index,
        }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Wireframe3dBatchSetKey {

    pub pipeline: CachedRenderPipelineId,

    pub asset_id: UntypedAssetId,

    pub draw_function: DrawFunctionId,

    pub slabs: MeshSlabs,

    pub mesh_asset_id: Option<UntypedAssetId>,
}

impl PhaseItemBatchSetKey for Wireframe3dBatchSetKey {
    fn indexed(&self) -> bool {
        self.slabs.index_slab_id.is_some()
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Wireframe3dBinKey {

    pub asset_id: UntypedAssetId,
}

pub struct SetWireframe3dThinImmediates;

impl<P: PhaseItem> RenderCommand<P> for SetWireframe3dThinImmediates {
    type Param = (
        SRes<RenderWireframeInstances>,
        SRes<RenderAssets<RenderWireframeMaterial>>,
    );
    type ViewQuery = ();
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        item: &P,
        _view: (),
        _item_query: Option<()>,
        (wireframe_instances, wireframe_assets): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(wireframe_material) = wireframe_instances.get(&item.main_entity()) else {
            return RenderCommandResult::Failure("No wireframe material found for entity");
        };
        let Some(wireframe_material) = wireframe_assets.get(*wireframe_material) else {
            return RenderCommandResult::Failure("No wireframe material found for entity");
        };

        pass.set_immediates(0, bytemuck::bytes_of(&wireframe_material.color));
        RenderCommandResult::Success
    }
}

pub struct SetWireframe3dWideImmediates;

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct WireframeWideImmediates {
    color: [f32; 4],
    line_width: f32,
    smoothing: f32,
    #[cfg(all(feature = "webgl", target_arch = "wasm32", not(feature = "webgpu")))]
    _padding: [f32; 2],
}

impl<P: PhaseItem> RenderCommand<P> for SetWireframe3dWideImmediates {
    type Param = (
        SRes<RenderWireframeInstances>,
        SRes<RenderAssets<RenderWireframeMaterial>>,
    );
    type ViewQuery = ();
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        item: &P,
        _view: (),
        _item_query: Option<()>,
        (wireframe_instances, wireframe_assets): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(wireframe_material) = wireframe_instances.get(&item.main_entity()) else {
            return RenderCommandResult::Failure("No wireframe material found for entity");
        };
        let Some(wireframe_material) = wireframe_assets.get(*wireframe_material) else {
            return RenderCommandResult::Failure("No wireframe material found for entity");
        };

        let push = WireframeWideImmediates {
            color: wireframe_material.color,
            line_width: wireframe_material.line_width,
            smoothing: if wireframe_material.line_width <= 1.0 {
                0.5
            } else {
                1.0
            },
            #[cfg(all(feature = "webgl", target_arch = "wasm32", not(feature = "webgpu")))]
            _padding: [0.0; 2],
        };
        pass.set_immediates(0, bytemuck::bytes_of(&push));
        RenderCommandResult::Success
    }
}

#[derive(Clone, Copy, ShaderType, Pod, Zeroable)]
#[repr(C)]
pub struct WireframeVertexPullParams {
    pub index_offset: u32,
    pub vertex_stride_u32s: u32,
    pub position_offset_u32s: u32,
}

#[derive(Resource, Default)]
pub struct WireframeWideBindGroups {
    pub params: DynamicUniformBuffer<WireframeVertexPullParams>,
    pub bind_groups: HashMap<AssetId<Mesh>, (BindGroup, u32)>,
}

pub fn prepare_wireframe_wide_bind_groups(
    render_mesh_instances: Res<RenderMeshInstances>,
    render_meshes: Res<RenderAssets<RenderMesh>>,
    render_wireframe_instances: Res<RenderWireframeInstances>,
    render_wireframe_assets: Res<RenderAssets<RenderWireframeMaterial>>,
    mesh_allocator: Res<MeshAllocator>,
    pipeline: Res<Wireframe3dPipeline>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut wide_bind_groups: ResMut<WireframeWideBindGroups>,
) {
    wide_bind_groups.bind_groups.clear();

    struct MeshInfo {
        mesh_id: AssetId<Mesh>,
        params: WireframeVertexPullParams,
        vertex_buffer: Buffer,
        index_buffer: Buffer,
    }

    let mut infos: Vec<MeshInfo> = Vec::new();
    let mut seen: HashSet<AssetId<Mesh>, FixedHasher> = HashSet::default();

    for (entity, wireframe_asset_id) in render_wireframe_instances.iter() {
        let Some(material) = render_wireframe_assets.get(*wireframe_asset_id) else {
            continue;
        };
        if material.line_width <= 1.0 && material.topology != WireframeTopology::Quads {
            continue;
        }

        let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*entity) else {
            continue;
        };
        let mesh_id = mesh_instance.mesh_asset_id();
        if !seen.insert(mesh_id) {
            continue;
        }

        let Some(mesh) = render_meshes.get(mesh_id) else {
            continue;
        };
        let Some(vertex_slice) = mesh_allocator.mesh_vertex_slice(&mesh_id) else {
            continue;
        };
        let Some(index_slice) = mesh_allocator.mesh_index_slice(&mesh_id) else {
            continue;
        };

        let vertex_stride_bytes = mesh.layout.0.layout().array_stride as u32;
        let position_offset_bytes = mesh
            .layout
            .0
            .layout()
            .attributes
            .first()
            .map(|a| a.offset as u32)
            .unwrap_or(0);

        infos.push(MeshInfo {
            mesh_id,
            params: WireframeVertexPullParams {
                index_offset: index_slice.range.start,
                vertex_stride_u32s: vertex_stride_bytes / 4,
                position_offset_u32s: position_offset_bytes / 4,
            },
            vertex_buffer: vertex_slice.buffer.clone(),
            index_buffer: index_slice.buffer.clone(),
        });
    }

    if infos.is_empty() {
        return;
    }

    let Some(mut writer) =
        wide_bind_groups
            .params
            .get_writer(infos.len(), &render_device, &render_queue)
    else {
        return;
    };

    let offsets: Vec<u32> = infos
        .iter()
        .map(|info| writer.write(&info.params))
        .collect();
    drop(writer);

    let WireframeWideBindGroups {
        ref params,
        ref mut bind_groups,
    } = *wide_bind_groups;
    let Some(params_binding) = params.binding() else {
        return;
    };

    for (i, info) in infos.iter().enumerate() {
        let bind_group = render_device.create_bind_group(
            "wireframe_wide_bind_group",
            &pipeline.wide_bind_group_layout,
            &BindGroupEntries::sequential((
                info.vertex_buffer.as_entire_buffer_binding(),
                info.index_buffer.as_entire_buffer_binding(),
                params_binding.clone(),
            )),
        );
        bind_groups.insert(info.mesh_id, (bind_group, offsets[i]));
    }
}

pub struct SetWireframe3dWideBindGroup;

impl<P: PhaseItem> RenderCommand<P> for SetWireframe3dWideBindGroup {
    type Param = (SRes<RenderMeshInstances>, SRes<WireframeWideBindGroups>);
    type ViewQuery = ();
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        item: &P,
        _view: (),
        _item_query: Option<()>,
        (render_mesh_instances, wide_bind_groups): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(item.main_entity())
        else {
            return RenderCommandResult::Skip;
        };
        let Some((bind_group, dynamic_offset)) = wide_bind_groups
            .into_inner()
            .bind_groups
            .get(&mesh_instance.mesh_asset_id())
        else {
            return RenderCommandResult::Skip;
        };

        pass.set_bind_group(3, bind_group, &[*dynamic_offset]);
        RenderCommandResult::Success
    }
}

pub struct DrawWireframeMeshPulled;

impl<P: PhaseItem> RenderCommand<P> for DrawWireframeMeshPulled {
    type Param = (
        SRes<RenderMeshInstances>,
        SRes<RenderAssets<RenderMesh>>,
        SRes<MeshAllocator>,
        SRes<IndirectParametersBuffers>,
        SRes<PipelineCache>,
        Option<SRes<PreprocessPipelines>>,
        SRes<GpuPreprocessingSupport>,
    );
    type ViewQuery = Has<PreprocessBindGroups>;
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        item: &P,
        has_preprocess_bind_group: ROQueryItem<Self::ViewQuery>,
        _item_query: Option<()>,
        (
            render_mesh_instances,
            render_meshes,
            mesh_allocator,
            indirect_parameters_buffers,
            pipeline_cache,
            preprocess_pipelines,
            preprocessing_support,
        ): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        if let Some(preprocess_pipelines) = preprocess_pipelines
            && (!has_preprocess_bind_group
                || !preprocess_pipelines
                    .pipelines_are_loaded(&pipeline_cache, &preprocessing_support))
        {
            return RenderCommandResult::Skip;
        }

        let render_mesh_instances = render_mesh_instances.into_inner();
        let render_meshes = render_meshes.into_inner();
        let mesh_allocator = mesh_allocator.into_inner();
        let indirect_parameters_buffers = indirect_parameters_buffers.into_inner();

        let Some(mesh_asset_id) = render_mesh_instances.mesh_asset_id(item.main_entity()) else {
            return RenderCommandResult::Skip;
        };
        let Some(gpu_mesh) = render_meshes.get(mesh_asset_id) else {
            return RenderCommandResult::Skip;
        };

        let index_count = match &gpu_mesh.buffer_info {
            RenderMeshBufferInfo::Indexed { count, .. } => *count,
            RenderMeshBufferInfo::NonIndexed => gpu_mesh.vertex_count,
        };

        match item.extra_index() {
            PhaseItemExtraIndex::None | PhaseItemExtraIndex::DynamicOffset(_) => {

                let Some(vertex_slice) = mesh_allocator.mesh_vertex_slice(&mesh_asset_id) else {
                    return RenderCommandResult::Skip;
                };
                let first_vertex = vertex_slice.range.start;
                pass.draw(
                    first_vertex..(first_vertex + index_count),
                    item.batch_range().clone(),
                );
            }
            PhaseItemExtraIndex::IndirectParametersIndex {
                range: indirect_parameters_range,
                batch_set_index,
            } => {

                let Some(phase_indirect) = indirect_parameters_buffers.get(&TypeId::of::<P>())
                else {
                    warn!("Wireframe wide: indirect parameters buffer missing for phase");
                    return RenderCommandResult::Skip;
                };
                let (Some(indirect_buffer), Some(batch_sets_buffer)) = (
                    phase_indirect.non_indexed.data_buffer(),
                    phase_indirect.non_indexed.batch_sets_buffer(),
                ) else {
                    warn!("Wireframe wide: non-indexed indirect parameters buffer not ready");
                    return RenderCommandResult::Skip;
                };

                let indirect_parameters_offset = indirect_parameters_range.start as u64
                    * size_of::<IndirectParametersNonIndexed>() as u64;
                let indirect_parameters_count =
                    indirect_parameters_range.end - indirect_parameters_range.start;

                match batch_set_index {
                    Some(batch_set_index) => {
                        let count_offset =
                            u32::from(batch_set_index) * (size_of::<IndirectBatchSet>() as u32);
                        pass.multi_draw_indirect_count(
                            indirect_buffer,
                            indirect_parameters_offset,
                            batch_sets_buffer,
                            count_offset as u64,
                            indirect_parameters_count,
                        );
                    }
                    None => {
                        pass.multi_draw_indirect(
                            indirect_buffer,
                            indirect_parameters_offset,
                            indirect_parameters_count,
                        );
                    }
                }
            }
        }
        RenderCommandResult::Success
    }
}

pub type DrawWireframe3dThin = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    SetMeshBindGroup<2>,
    SetWireframe3dThinImmediates,
    DrawMesh,
);

pub type DrawWireframe3dWide = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    SetMeshBindGroup<2>,
    SetWireframe3dWideBindGroup,
    SetWireframe3dWideImmediates,
    DrawWireframeMeshPulled,
);

#[derive(Resource, Clone)]
pub struct Wireframe3dPipeline {
    mesh_pipeline: MeshPipeline,
    shader: Handle<Shader>,
    pub wide_bind_group_layout: BindGroupLayout,
    pub wide_bind_group_layout_descriptor: BindGroupLayoutDescriptor,
}

pub fn init_wireframe_3d_pipeline(
    mut commands: Commands,
    mesh_pipeline: Res<MeshPipeline>,
    asset_server: Res<AssetServer>,
    render_device: Res<RenderDevice>,
) {
    let wide_bgl_entries = BindGroupLayoutEntries::sequential(
        ShaderStages::VERTEX,
        (
            storage_buffer_read_only::<u32>(false),
            storage_buffer_read_only::<u32>(false),
            uniform_buffer::<WireframeVertexPullParams>(true),
        ),
    );

    let wide_bind_group_layout = render_device
        .create_bind_group_layout("wireframe_wide_bind_group_layout", &wide_bgl_entries);

    let wide_bind_group_layout_descriptor =
        BindGroupLayoutDescriptor::new("wireframe_wide_bind_group_layout", &wide_bgl_entries);

    commands.insert_resource(Wireframe3dPipeline {
        mesh_pipeline: mesh_pipeline.clone(),
        shader: load_embedded_asset!(asset_server.as_ref(), "render/wireframe.wgsl"),
        wide_bind_group_layout,
        wide_bind_group_layout_descriptor,
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WireframePipelineKey {
    pub mesh_key: MeshPipelineKey,
    pub wide: bool,
    pub quads: bool,
    pub line_mode: bool,
}

impl SpecializedMeshPipeline for Wireframe3dPipeline {
    type Key = WireframePipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut descriptor = self.mesh_pipeline.specialize(key.mesh_key, layout)?;

        if descriptor.primitive.topology.is_triangles() {
            descriptor.depth_stencil.as_mut().unwrap().bias.slope_scale = 1.0;
        }

        if key.wide {
            descriptor.label = Some("wireframe_3d_wide_pipeline".into());

            descriptor.vertex.shader = self.shader.clone();
            descriptor.vertex.shader_defs.push("WIREFRAME_WIDE".into());
            if key.quads {
                descriptor.vertex.shader_defs.push("WIREFRAME_QUADS".into());
            }
            descriptor.vertex.entry_point = Some("vertex".into());
            descriptor.vertex.buffers = vec![];

            let fragment = descriptor.fragment.as_mut().unwrap();
            fragment.shader = self.shader.clone();
            fragment.shader_defs.push("WIREFRAME_WIDE".into());
            fragment.entry_point = Some("fragment".into());

            for state in fragment.targets.iter_mut().flatten() {
                state.blend = Some(BlendState::ALPHA_BLENDING);
            }

            descriptor.primitive.polygon_mode = if key.line_mode {
                PolygonMode::Line
            } else {
                PolygonMode::Fill
            };
            descriptor.immediate_size = 32;

            descriptor
                .layout
                .push(self.wide_bind_group_layout_descriptor.clone());
        } else {
            descriptor.label = Some("wireframe_3d_pipeline".into());
            descriptor.immediate_size = 16;
            let fragment = descriptor.fragment.as_mut().unwrap();
            fragment.shader = self.shader.clone();
            descriptor.primitive.polygon_mode = PolygonMode::Line;
        }

        Ok(descriptor)
    }
}

pub fn wireframe_3d(
    world: &World,
    view: ViewQuery<(
        &ExtractedCamera,
        &ExtractedView,
        &ViewTarget,
        &ViewDepthTexture,
    )>,
    wireframe_phases: Res<ViewBinnedRenderPhases<Wireframe3d>>,
    mut ctx: RenderContext,
) {
    let view_entity = view.entity();

    let (camera, extracted_view, target, depth) = view.into_inner();

    let Some(wireframe_phase) = wireframe_phases.get(&extracted_view.retained_view_entity) else {
        return;
    };

    if wireframe_phase.is_empty() {
        return;
    }

    let mut render_pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("wireframe_3d"),
        color_attachments: &[Some(target.get_color_attachment())],
        depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });

    if let Some(viewport) = camera.viewport.as_ref() {
        render_pass.set_camera_viewport(viewport);
    }

    if let Err(err) = wireframe_phase.render(&mut render_pass, world, view_entity) {
        error!("Error encountered while rendering the wireframe phase {err:?}");
    }
}

#[derive(Component, Debug, Clone, Default, Reflect)]
#[reflect(Component, Default, Debug)]
pub struct WireframeColor {
    pub color: Color,
}

#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default, Debug)]
pub struct WireframeLineWidth {
    pub width: f32,
}

impl Default for WireframeLineWidth {
    fn default() -> Self {
        Self { width: 1.0 }
    }
}

#[derive(Component, Debug, Clone, Default, Reflect, Eq, PartialEq)]
#[reflect(Component, Default, Debug, PartialEq)]
pub struct NoWireframe;

#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Reflect)]
#[reflect(Component, Default, Debug)]
pub enum WireframeTopology {
    #[default]
    Triangles,

    Quads,
}

#[derive(Resource, Debug, Clone, ExtractResource, Reflect)]
#[reflect(Resource, Debug, Default)]
pub struct WireframeConfig {

    pub global: bool,

    pub default_color: Color,

    pub default_line_width: f32,

    pub default_topology: WireframeTopology,
}

impl Default for WireframeConfig {
    fn default() -> Self {
        Self {
            global: false,
            default_color: Color::default(),
            default_line_width: 1.0,
            default_topology: WireframeTopology::default(),
        }
    }
}

#[derive(Asset, Reflect, Clone, Debug)]
#[reflect(Clone, Default)]
pub struct WireframeMaterial {
    pub color: Color,
    pub line_width: f32,
    pub topology: WireframeTopology,
}

impl Default for WireframeMaterial {
    fn default() -> Self {
        Self {
            color: Color::default(),
            line_width: 1.0,
            topology: WireframeTopology::default(),
        }
    }
}

pub struct RenderWireframeMaterial {
    pub color: [f32; 4],
    pub line_width: f32,
    pub topology: WireframeTopology,
}

#[derive(
    Component, FromTemplate, Clone, Debug, Default, Deref, DerefMut, Reflect, PartialEq, Eq,
)]
#[reflect(Component, Default, Clone, PartialEq)]
pub struct Mesh3dWireframe(pub Handle<WireframeMaterial>);

impl AsAssetId for Mesh3dWireframe {
    type Asset = WireframeMaterial;

    fn as_asset_id(&self) -> AssetId<Self::Asset> {
        self.0.id()
    }
}

impl RenderAsset for RenderWireframeMaterial {
    type SourceAsset = WireframeMaterial;
    type Param = ();

    fn prepare_asset(
        source_asset: Self::SourceAsset,
        _asset_id: AssetId<Self::SourceAsset>,
        _param: &mut SystemParamItem<Self::Param>,
        _previous_asset: Option<&Self>,
    ) -> Result<Self, PrepareAssetError<Self::SourceAsset>> {
        Ok(RenderWireframeMaterial {
            color: source_asset.color.to_linear().to_f32_array(),
            line_width: source_asset.line_width,
            topology: source_asset.topology,
        })
    }
}

#[derive(Resource, Deref, DerefMut, Default)]
pub struct RenderWireframeInstances(MainEntityHashMap<AssetId<WireframeMaterial>>);

#[derive(Clone, Resource, Debug, Default)]
pub struct WireframeEntitiesNeedingSpecialization {

    pub changed: Vec<Entity>,

    pub removed: Vec<Entity>,
}

#[derive(Resource, Default)]
pub struct SpecializedWireframePipelineCache {
    views: HashMap<RetainedViewEntity, SpecializedWireframeViewPipelineCache>,
    wide: HashMap<(MeshPipelineKey, MeshVertexBufferLayoutRef, bool, bool), CachedRenderPipelineId>,
}

#[derive(Deref, DerefMut, Default)]
pub struct SpecializedWireframeViewPipelineCache {

    #[deref]
    map: MainEntityHashMap<CachedRenderPipelineId>,
}

#[derive(Resource)]
struct GlobalWireframeMaterial {

    handle: Handle<WireframeMaterial>,
}

pub fn extract_wireframe_materials(
    mut material_instances: ResMut<RenderWireframeInstances>,
    changed_meshes_query: Extract<
        Query<
            (Entity, &ViewVisibility, &Mesh3dWireframe),
            Or<(Changed<ViewVisibility>, Changed<Mesh3dWireframe>)>,
        >,
    >,
    mut removed_visibilities_query: Extract<RemovedComponents<ViewVisibility>>,
    mut removed_materials_query: Extract<RemovedComponents<Mesh3dWireframe>>,
) {
    for (entity, view_visibility, material) in &changed_meshes_query {
        if view_visibility.get() {
            material_instances.insert(entity.into(), material.id());
        } else {
            material_instances.remove(&MainEntity::from(entity));
        }
    }

    for entity in removed_visibilities_query
        .read()
        .chain(removed_materials_query.read())
    {

        if !changed_meshes_query.contains(entity) {
            material_instances.remove(&MainEntity::from(entity));
        }
    }
}

fn setup_global_wireframe_material(
    mut commands: Commands,
    mut materials: ResMut<Assets<WireframeMaterial>>,
    config: Res<WireframeConfig>,
) {
    commands.insert_resource(GlobalWireframeMaterial {
        handle: materials.add(WireframeMaterial {
            color: config.default_color,
            line_width: config.default_line_width,
            topology: config.default_topology,
        }),
    });
}

fn wireframe_config_changed(
    config: Res<WireframeConfig>,
    mut materials: ResMut<Assets<WireframeMaterial>>,
    global_material: Res<GlobalWireframeMaterial>,
    mut per_entity_wireframes: Query<
        (
            &mut Mesh3dWireframe,
            Option<&WireframeColor>,
            Option<&WireframeLineWidth>,
            Option<&WireframeTopology>,
        ),
        With<Wireframe>,
    >,
) {
    if let Some(mut mat) = materials.get_mut(&global_material.handle) {
        mat.color = config.default_color;
        mat.line_width = config.default_line_width;
        mat.topology = config.default_topology;
    }

    for (mut handle, maybe_color, maybe_width, maybe_topology) in &mut per_entity_wireframes {
        if handle.0 == global_material.handle {
            continue;
        }
        handle.0 = materials.add(WireframeMaterial {
            color: maybe_color.map(|c| c.color).unwrap_or(config.default_color),
            line_width: maybe_width
                .map(|w| w.width)
                .unwrap_or(config.default_line_width),
            topology: maybe_topology.copied().unwrap_or(config.default_topology),
        });
    }
}

fn wireframe_color_changed(
    mut materials: ResMut<Assets<WireframeMaterial>>,
    mut colors_changed: Query<
        (
            &mut Mesh3dWireframe,
            &WireframeColor,
            Option<&WireframeLineWidth>,
            Option<&WireframeTopology>,
        ),
        (With<Wireframe>, Changed<WireframeColor>),
    >,
    config: Res<WireframeConfig>,
) {
    for (mut handle, wireframe_color, maybe_width, maybe_topology) in &mut colors_changed {
        handle.0 = materials.add(WireframeMaterial {
            color: wireframe_color.color,
            line_width: maybe_width
                .map(|w| w.width)
                .unwrap_or(config.default_line_width),
            topology: maybe_topology.copied().unwrap_or(config.default_topology),
        });
    }
}

fn wireframe_line_width_changed(
    mut materials: ResMut<Assets<WireframeMaterial>>,
    mut widths_changed: Query<
        (
            &mut Mesh3dWireframe,
            &WireframeLineWidth,
            Option<&WireframeColor>,
            Option<&WireframeTopology>,
        ),
        (With<Wireframe>, Changed<WireframeLineWidth>),
    >,
    config: Res<WireframeConfig>,
) {
    for (mut handle, wireframe_width, maybe_color, maybe_topology) in &mut widths_changed {
        handle.0 = materials.add(WireframeMaterial {
            color: maybe_color.map(|c| c.color).unwrap_or(config.default_color),
            line_width: wireframe_width.width,
            topology: maybe_topology.copied().unwrap_or(config.default_topology),
        });
    }
}

fn wireframe_topology_changed(
    mut materials: ResMut<Assets<WireframeMaterial>>,
    mut topology_changed: Query<
        (
            &mut Mesh3dWireframe,
            &WireframeTopology,
            Option<&WireframeColor>,
            Option<&WireframeLineWidth>,
        ),
        (With<Wireframe>, Changed<WireframeTopology>),
    >,
    config: Res<WireframeConfig>,
) {
    for (mut handle, topology, maybe_color, maybe_width) in &mut topology_changed {
        handle.0 = materials.add(WireframeMaterial {
            color: maybe_color.map(|c| c.color).unwrap_or(config.default_color),
            line_width: maybe_width
                .map(|w| w.width)
                .unwrap_or(config.default_line_width),
            topology: *topology,
        });
    }
}

fn apply_wireframe_material(
    mut commands: Commands,
    mut materials: ResMut<Assets<WireframeMaterial>>,
    wireframes: Query<
        (
            Entity,
            Option<&WireframeColor>,
            Option<&WireframeLineWidth>,
            Option<&WireframeTopology>,
        ),
        (With<Wireframe>, Without<Mesh3dWireframe>),
    >,
    no_wireframes: Query<Entity, (With<NoWireframe>, With<Mesh3dWireframe>)>,
    mut removed_wireframes: RemovedComponents<Wireframe>,
    global_material: Res<GlobalWireframeMaterial>,
    config: Res<WireframeConfig>,
) {
    for e in removed_wireframes.read().chain(no_wireframes.iter()) {
        if let Ok(mut commands) = commands.get_entity(e) {
            commands.remove::<Mesh3dWireframe>();
        }
    }

    let mut material_to_spawn = vec![];
    for (e, maybe_color, maybe_width, maybe_topology) in &wireframes {
        let material = get_wireframe_material(
            maybe_color,
            maybe_width,
            maybe_topology,
            &mut materials,
            &global_material,
            &config,
        );
        material_to_spawn.push((e, Mesh3dWireframe(material)));
    }
    commands.try_insert_batch(material_to_spawn);
}

type WireframeFilter = (With<Mesh3d>, Without<Wireframe>, Without<NoWireframe>);

fn apply_global_wireframe_material(
    mut commands: Commands,
    config: Res<WireframeConfig>,
    meshes_without_material: Query<
        (
            Entity,
            Option<&WireframeColor>,
            Option<&WireframeLineWidth>,
            Option<&WireframeTopology>,
        ),
        (WireframeFilter, Without<Mesh3dWireframe>),
    >,
    meshes_with_global_material: Query<Entity, (WireframeFilter, With<Mesh3dWireframe>)>,
    global_material: Res<GlobalWireframeMaterial>,
    mut materials: ResMut<Assets<WireframeMaterial>>,
) {
    if config.global {
        let mut material_to_spawn = vec![];
        for (e, maybe_color, maybe_width, maybe_topology) in &meshes_without_material {
            let material = get_wireframe_material(
                maybe_color,
                maybe_width,
                maybe_topology,
                &mut materials,
                &global_material,
                &config,
            );

            material_to_spawn.push((e, Mesh3dWireframe(material)));
        }
        commands.try_insert_batch(material_to_spawn);
    } else {
        for e in &meshes_with_global_material {
            commands.entity(e).remove::<Mesh3dWireframe>();
        }
    }
}

fn get_wireframe_material(
    maybe_color: Option<&WireframeColor>,
    maybe_width: Option<&WireframeLineWidth>,
    maybe_topology: Option<&WireframeTopology>,
    wireframe_materials: &mut Assets<WireframeMaterial>,
    global_material: &GlobalWireframeMaterial,
    config: &WireframeConfig,
) -> Handle<WireframeMaterial> {
    if maybe_color.is_some() || maybe_width.is_some() || maybe_topology.is_some() {
        wireframe_materials.add(WireframeMaterial {
            color: maybe_color.map(|c| c.color).unwrap_or(config.default_color),
            line_width: maybe_width
                .map(|w| w.width)
                .unwrap_or(config.default_line_width),
            topology: maybe_topology.copied().unwrap_or(config.default_topology),
        })
    } else {

        global_material.handle.clone()
    }
}

fn extract_wireframe_3d_camera(
    mut wireframe_3d_phases: ResMut<ViewBinnedRenderPhases<Wireframe3d>>,
    cameras: Extract<Query<(Entity, &Camera, Has<NoIndirectDrawing>), With<Camera3d>>>,
    mut live_entities: Local<HashSet<RetainedViewEntity>>,
    gpu_preprocessing_support: Res<GpuPreprocessingSupport>,
) {
    live_entities.clear();
    for (main_entity, camera, no_indirect_drawing) in &cameras {
        if !camera.is_active {
            continue;
        }
        let gpu_preprocessing_mode = gpu_preprocessing_support.min(if !no_indirect_drawing {
            GpuPreprocessingMode::Culling
        } else {
            GpuPreprocessingMode::PreprocessingOnly
        });

        let retained_view_entity = RetainedViewEntity::new(main_entity.into(), None, 0);
        wireframe_3d_phases.prepare_for_new_frame(retained_view_entity, gpu_preprocessing_mode);
        live_entities.insert(retained_view_entity);
    }

    wireframe_3d_phases.retain(|camera_entity, _| live_entities.contains(camera_entity));
}

pub fn extract_wireframe_entities_needing_specialization(
    entities_needing_specialization: Extract<Res<WireframeEntitiesNeedingSpecialization>>,
    mut dirty_wireframe_specializations: ResMut<DirtyWireframeSpecializations>,
) {

    for entity in entities_needing_specialization.changed.iter() {
        dirty_wireframe_specializations
            .changed_renderables
            .insert(MainEntity::from(*entity));
    }
}

pub fn extract_wireframe_entities_that_need_specializations_removed(
    entities_needing_specialization: Extract<Res<WireframeEntitiesNeedingSpecialization>>,
    mut dirty_wireframe_specializations: ResMut<DirtyWireframeSpecializations>,
) {
    for entity in entities_needing_specialization.removed.iter() {
        dirty_wireframe_specializations
            .removed_renderables
            .insert(MainEntity::from(*entity));
    }
}

pub fn check_wireframe_entities_needing_specialization(
    needs_specialization: Query<
        Entity,
        Or<(
            Changed<Mesh3d>,
            AssetChanged<Mesh3d>,
            Changed<Mesh3dWireframe>,
            AssetChanged<Mesh3dWireframe>,
            Changed<WireframeLineWidth>,
            Changed<WireframeTopology>,
        )>,
    >,
    mut entities_needing_specialization: ResMut<WireframeEntitiesNeedingSpecialization>,
    mut removed_mesh_3d_components: RemovedComponents<Mesh3d>,
    mut removed_mesh_3d_wireframe_components: RemovedComponents<Mesh3dWireframe>,
) {
    entities_needing_specialization.changed.clear();
    entities_needing_specialization.removed.clear();

    for entity in &needs_specialization {
        entities_needing_specialization.changed.push(entity);
    }

    for entity in removed_mesh_3d_components
        .read()
        .chain(removed_mesh_3d_wireframe_components.read())
    {
        entities_needing_specialization.removed.push(entity);
    }
}

#[derive(Default, Deref, DerefMut, Resource)]
pub struct PendingWireframeQueues(pub PendingQueues);

pub fn specialize_wireframes(
    render_meshes: Res<RenderAssets<RenderMesh>>,
    render_mesh_instances: Res<RenderMeshInstances>,
    render_wireframe_instances: Res<RenderWireframeInstances>,
    render_wireframe_assets: Res<RenderAssets<RenderWireframeMaterial>>,
    render_visibility_ranges: Res<RenderVisibilityRanges>,
    wireframe_phases: Res<ViewBinnedRenderPhases<Wireframe3d>>,
    views: Query<(&ExtractedView, &RenderVisibleEntities)>,
    view_key_cache: Res<ViewKeyCache>,
    dirty_wireframe_specializations: Res<DirtyWireframeSpecializations>,
    mut specialized_material_pipeline_cache: ResMut<SpecializedWireframePipelineCache>,
    mut pipelines: ResMut<SpecializedMeshPipelines<Wireframe3dPipeline>>,
    mut pending_wireframe_queues: ResMut<PendingWireframeQueues>,
    pipeline: Res<Wireframe3dPipeline>,
    pipeline_cache: Res<PipelineCache>,
    render_lightmaps: Res<RenderLightmaps>,
) {
    let mut all_views: HashSet<RetainedViewEntity, FixedHasher> = HashSet::default();

    let SpecializedWireframePipelineCache {
        views: ref mut views_pipeline_cache,
        wide: ref mut wide_pipeline_cache,
    } = *specialized_material_pipeline_cache;

    for (view, visible_entities) in &views {
        all_views.insert(view.retained_view_entity);

        if !wireframe_phases.contains_key(&view.retained_view_entity) {
            continue;
        }

        let Some(view_key) = view_key_cache.get(&view.retained_view_entity) else {
            continue;
        };

        let view_specialized_material_pipeline_cache = views_pipeline_cache
            .entry(view.retained_view_entity)
            .or_default();

        let Some(render_visible_mesh_entities) = visible_entities.get::<Mesh3d>() else {
            continue;
        };

        let view_pending_wireframe_queues =
            pending_wireframe_queues.prepare_for_new_frame(view.retained_view_entity);

        if dirty_wireframe_specializations
            .must_wipe_specializations_for_view(view.retained_view_entity)
        {
            view_specialized_material_pipeline_cache.clear();
        } else {
            for &renderable_entity in dirty_wireframe_specializations.iter_to_despecialize() {
                view_specialized_material_pipeline_cache.remove(&renderable_entity);
            }
        }

        for (render_entity, visible_entity) in dirty_wireframe_specializations.iter_to_specialize(
            view.retained_view_entity,
            render_visible_mesh_entities,
            &view_pending_wireframe_queues.prev_frame,
        ) {
            if view_specialized_material_pipeline_cache.contains_key(visible_entity) {
                continue;
            }

            if !render_wireframe_instances.contains_key(visible_entity) {
                continue;
            };
            let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*visible_entity)
            else {

                view_pending_wireframe_queues
                    .current_frame
                    .insert((*render_entity, *visible_entity));
                continue;
            };
            let Some(mesh) = render_meshes.get(mesh_instance.mesh_asset_id()) else {
                continue;
            };

            let mut mesh_key = *view_key;
            mesh_key |= MeshPipelineKey::from_primitive_topology_and_strip_index(
                mesh.primitive_topology(),
                mesh.index_format(),
            );

            if render_visibility_ranges.entity_has_crossfading_visibility_ranges(*visible_entity) {
                mesh_key |= MeshPipelineKey::VISIBILITY_RANGE_DITHER;
            }

            if view_key.contains(MeshPipelineKey::MOTION_VECTOR_PREPASS) {

                if mesh_instance
                    .flags()
                    .contains(RenderMeshInstanceFlags::HAS_PREVIOUS_SKIN)
                {
                    mesh_key |= MeshPipelineKey::HAS_PREVIOUS_SKIN;
                }
                if mesh_instance
                    .flags()
                    .contains(RenderMeshInstanceFlags::HAS_PREVIOUS_MORPH)
                {
                    mesh_key |= MeshPipelineKey::HAS_PREVIOUS_MORPH;
                }
            }

            if render_lightmaps
                .render_lightmaps
                .contains_key(visible_entity)
            {
                mesh_key |= MeshPipelineKey::LIGHTMAPPED;
            }

            let mat = render_wireframe_instances
                .get(visible_entity)
                .and_then(|asset_id| render_wireframe_assets.get(*asset_id));
            let quads = mat
                .map(|m| m.topology == WireframeTopology::Quads)
                .unwrap_or(false);
            let thick = mat.map(|m| m.line_width > 1.0).unwrap_or(false);
            let wide = thick || quads;
            let line_mode = wide && !thick;

            let pipeline_id = if wide {
                let cache_key = (mesh_key, mesh.layout.clone(), quads, line_mode);
                *wide_pipeline_cache.entry(cache_key).or_insert_with(|| {
                    let wireframe_key = WireframePipelineKey {
                        mesh_key,
                        wide: true,
                        quads,
                        line_mode,
                    };
                    match pipeline.specialize(wireframe_key, &mesh.layout) {
                        Ok(descriptor) => pipeline_cache.queue_render_pipeline(descriptor),
                        Err(err) => {
                            error!("{}", err);
                            CachedRenderPipelineId::INVALID
                        }
                    }
                })
            } else {
                let wireframe_key = WireframePipelineKey {
                    mesh_key,
                    wide: false,
                    quads: false,
                    line_mode: false,
                };
                match pipelines.specialize(&pipeline_cache, &pipeline, wireframe_key, &mesh.layout)
                {
                    Ok(id) => id,
                    Err(err) => {
                        error!("{}", err);
                        continue;
                    }
                }
            };

            view_specialized_material_pipeline_cache.insert(*visible_entity, pipeline_id);
        }
    }

    pending_wireframe_queues.expire_stale_views(&all_views);

    views_pipeline_cache.retain(|retained_view_entity, _| all_views.contains(retained_view_entity));
}

fn queue_wireframes(
    custom_draw_functions: Res<DrawFunctions<Wireframe3d>>,
    render_mesh_instances: Res<RenderMeshInstances>,
    gpu_preprocessing_support: Res<GpuPreprocessingSupport>,
    mesh_allocator: Res<MeshAllocator>,
    specialized_wireframe_pipeline_cache: Res<SpecializedWireframePipelineCache>,
    render_wireframe_instances: Res<RenderWireframeInstances>,
    dirty_wireframe_specializations: Res<DirtyWireframeSpecializations>,
    render_wireframe_assets: Res<RenderAssets<RenderWireframeMaterial>>,
    mut wireframe_3d_phases: ResMut<ViewBinnedRenderPhases<Wireframe3d>>,
    mut pending_wireframe_queues: ResMut<PendingWireframeQueues>,
    mut views: Query<(&ExtractedView, &RenderVisibleEntities)>,
) {
    for (view, visible_entities) in &mut views {
        let Some(wireframe_phase) = wireframe_3d_phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        let draw_functions = custom_draw_functions.read();
        let draw_thin = draw_functions.id::<DrawWireframe3dThin>();
        let draw_wide = draw_functions.id::<DrawWireframe3dWide>();

        let Some(view_specialized_material_pipeline_cache) = specialized_wireframe_pipeline_cache
            .views
            .get(&view.retained_view_entity)
        else {
            continue;
        };

        let Some(render_mesh_visible_entities) = visible_entities.get::<Mesh3d>() else {
            continue;
        };

        let view_pending_wireframe_queues = pending_wireframe_queues
            .get_mut(&view.retained_view_entity)
            .expect(
                "View pending wireframe queues should have been created in `specialize_wireframes`",
            );

        for &main_entity in dirty_wireframe_specializations
            .iter_to_dequeue(view.retained_view_entity, render_mesh_visible_entities)
        {
            wireframe_phase.remove(main_entity);
        }

        for (render_entity, visible_entity) in dirty_wireframe_specializations.iter_to_queue(
            view.retained_view_entity,
            render_mesh_visible_entities,
            &view_pending_wireframe_queues.prev_frame,
        ) {
            let Some(wireframe_instance) = render_wireframe_instances.get(visible_entity) else {
                continue;
            };
            let Some(pipeline_id) = view_specialized_material_pipeline_cache
                .get(visible_entity)
                .copied()
            else {
                continue;
            };

            let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*visible_entity)
            else {

                view_pending_wireframe_queues
                    .current_frame
                    .insert((*render_entity, *visible_entity));
                continue;
            };

            let is_wide = render_wireframe_assets
                .get(*wireframe_instance)
                .map(|mat| mat.line_width > 1.0 || mat.topology == WireframeTopology::Quads)
                .unwrap_or(false);
            let draw_function = if is_wide { draw_wide } else { draw_thin };

            let Some(MeshSlabs {
                vertex_slab_id: vertex_slab,
                index_slab_id: index_slab,
                morph_target_slab_id: morph_target_slab,
            }) = mesh_allocator.mesh_slabs(&mesh_instance.mesh_asset_id())
            else {
                continue;
            };
            let bin_key = Wireframe3dBinKey {
                asset_id: mesh_instance.mesh_asset_id().untyped(),
            };
            let batch_set_key = Wireframe3dBatchSetKey {
                pipeline: pipeline_id,
                asset_id: wireframe_instance.untyped(),
                draw_function,
                slabs: MeshSlabs {
                    vertex_slab_id: vertex_slab,
                    morph_target_slab_id: morph_target_slab,

                    index_slab_id: if is_wide { None } else { index_slab },
                },
                mesh_asset_id: if is_wide {
                    Some(mesh_instance.mesh_asset_id().untyped())
                } else {
                    None
                },
            };
            wireframe_phase.add(
                batch_set_key,
                bin_key,
                (*render_entity, *visible_entity),
                mesh_instance.current_uniform_index,
                BinnedRenderPhaseType::mesh(
                    mesh_instance.should_batch(),
                    &gpu_preprocessing_support,
                ),
            );
        }
    }
}
