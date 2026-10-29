

use core::num::{NonZero, NonZeroU64};

use bevy_app::{App, Plugin};
use bevy_asset::{embedded_asset, load_embedded_asset, Handle};
use bevy_core_pipeline::{
    deferred::node::late_deferred_prepass,
    mip_generation::experimental::depth::{early_downsample_depth, ViewDepthPyramid},
    prepass::{
        node::{early_prepass, late_prepass},
        DeferredPrepass, DepthPrepass, MotionVectorPrepass, NormalPrepass, PreviousViewData,
        PreviousViewUniformOffset, PreviousViewUniforms,
    },
    schedule::{Core3d, Core3dSystems},
};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    component::Component,
    entity::Entity,
    prelude::resource_exists,
    query::{Has, Or, With, Without},
    resource::Resource,
    schedule::{common_conditions::any_match_filter, IntoScheduleConfigs as _},
    system::{Commands, Query, Res, ResMut},
    world::{FromWorld, World},
};
use bevy_log::warn_once;
use bevy_math::Vec4;
use bevy_platform::collections::HashMap;
use bevy_render::{
    batching::gpu_preprocessing::{
        clear_bin_unpacking_buffers, BatchedInstanceBuffers, BinUnpackingBuffers,
        BinUnpackingBuffersKey, BinUnpackingJob, BinUnpackingMetadataIndex,
        GpuBinUnpackingMetadata, GpuOcclusionCullingWorkItemBuffers, GpuPreprocessingMode,
        GpuPreprocessingSupport, IndirectBatchSet, IndirectParametersBuffers,
        IndirectParametersCpuMetadata, IndirectParametersGpuMetadata, IndirectParametersIndexed,
        IndirectParametersNonIndexed, LatePreprocessWorkItemIndirectParameters, PreprocessWorkItem,
        PreprocessWorkItemBuffers, UntypedPhaseBatchedInstanceBuffers,
        UntypedPhaseIndirectParametersBuffers,
    },
    diagnostic::RecordDiagnostics as _,
    occlusion_culling::OcclusionCulling,
    render_phase::GpuRenderBinnedMeshInstance,
    render_resource::{
        binding_types::{storage_buffer, storage_buffer_read_only, texture_2d, uniform_buffer},
        BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
        BindingResource, Buffer, BufferBinding, BufferVec, CachedComputePipelineId,
        ComputePassDescriptor, ComputePipelineDescriptor, DynamicBindGroupLayoutEntries,
        PartialBufferVec, PipelineCache, RawBufferVec, ShaderStages, ShaderType,
        SparseBufferUpdateBindGroups, SparseBufferUpdateJobs, SparseBufferUpdatePipelines,
        SpecializedComputePipeline, SpecializedComputePipelines, TextureSampleType,
        UninitBufferVec,
    },
    renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
    settings::WgpuFeatures,
    view::{
        ExtractedView, NoIndirectDrawing, RenderVisibilityRanges, RetainedViewEntity, ViewUniform,
        ViewUniformOffset, ViewUniforms,
    },
    GpuResourceAppExt, Render, RenderApp, RenderSystems,
};
use bevy_shader::Shader;
use bevy_utils::{default, TypeIdMap};
use bitflags::bitflags;
use smallvec::{smallvec, SmallVec};
use tracing::warn;

use crate::{LightEntity, MeshCullingData, MeshCullingDataBuffer, MeshInputUniform, MeshUniform};

use super::{ShadowView, ViewLightEntities};

const WORKGROUP_SIZE: usize = 64;

pub struct GpuMeshPreprocessPlugin {

    pub use_gpu_instance_buffer_builder: bool,
}

#[derive(Resource)]
pub struct PreprocessPipelines {

    pub direct_preprocess: PreprocessPipeline,

    pub gpu_frustum_culling_preprocess: PreprocessPipeline,

    pub early_gpu_occlusion_culling_preprocess: PreprocessPipeline,

    pub late_gpu_occlusion_culling_preprocess: PreprocessPipeline,

    pub gpu_frustum_culling_build_indexed_indirect_params: BuildIndirectParametersPipeline,

    pub gpu_frustum_culling_build_non_indexed_indirect_params: BuildIndirectParametersPipeline,

    pub early_phase: PreprocessPhasePipelines,

    pub late_phase: PreprocessPhasePipelines,

    pub main_phase: PreprocessPhasePipelines,

    pub bin_unpacking: BinUnpackingPipeline,
}

#[derive(Clone)]
pub struct PreprocessPhasePipelines {

    pub reset_indirect_batch_sets: ResetIndirectBatchSetsPipeline,

    pub gpu_occlusion_culling_build_indexed_indirect_params: BuildIndirectParametersPipeline,

    pub gpu_occlusion_culling_build_non_indexed_indirect_params: BuildIndirectParametersPipeline,
}

pub struct PreprocessPipeline {

    pub bind_group_layout: BindGroupLayoutDescriptor,

    pub shader: Handle<Shader>,

    pub pipeline_id: Option<CachedComputePipelineId>,
}

#[derive(Clone)]
pub struct ResetIndirectBatchSetsPipeline {

    pub bind_group_layout: BindGroupLayoutDescriptor,

    pub shader: Handle<Shader>,

    pub pipeline_id: Option<CachedComputePipelineId>,
}

#[derive(Clone)]
pub struct BuildIndirectParametersPipeline {

    pub bind_group_layout: BindGroupLayoutDescriptor,

    pub shader: Handle<Shader>,

    pub pipeline_id: Option<CachedComputePipelineId>,
}

#[derive(Clone)]
pub struct BinUnpackingPipeline {

    pub bind_group_layout: BindGroupLayoutDescriptor,

    pub shader: Handle<Shader>,

    pub pipeline_id: Option<CachedComputePipelineId>,
}

bitflags! {

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    pub struct PreprocessPipelineKey: u8 {

        const FRUSTUM_CULLING = 1;

        const OCCLUSION_CULLING = 2;

        const EARLY_PHASE = 4;
    }

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    pub struct BuildIndirectParametersPipelineKey: u8 {

        const INDEXED = 1;

        const MULTI_DRAW_INDIRECT_COUNT_SUPPORTED = 2;

        const OCCLUSION_CULLING = 4;

        const EARLY_PHASE = 8;

        const LATE_PHASE = 16;

        const MAIN_PHASE = 32;
    }
}

#[derive(Component, Clone, Deref, DerefMut)]
pub struct PreprocessBindGroups(pub TypeIdMap<PhasePreprocessBindGroups>);

#[derive(Clone)]
pub enum PhasePreprocessBindGroups {

    Direct(BindGroup),

    IndirectFrustumCulling {

        indexed: Option<BindGroup>,

        non_indexed: Option<BindGroup>,
    },

    IndirectOcclusionCulling {

        early_indexed: Option<BindGroup>,

        early_non_indexed: Option<BindGroup>,

        late_indexed: Option<BindGroup>,

        late_non_indexed: Option<BindGroup>,
    },
}

#[derive(Resource, Default, Deref, DerefMut)]
pub struct BuildIndirectParametersBindGroups(pub TypeIdMap<PhaseBuildIndirectParametersBindGroups>);

impl BuildIndirectParametersBindGroups {

    pub fn new() -> BuildIndirectParametersBindGroups {
        Self::default()
    }
}

pub struct PhaseBuildIndirectParametersBindGroups {

    reset_indexed_indirect_batch_sets: Option<BindGroup>,

    reset_non_indexed_indirect_batch_sets: Option<BindGroup>,

    build_indexed_indirect: Option<BindGroup>,

    build_non_indexed_indirect: Option<BindGroup>,
}

#[derive(Clone, Resource, Default, Deref, DerefMut)]
pub struct BinUnpackingBindGroups(
    pub HashMap<BinUnpackingBuffersKey, ViewPhaseBinUnpackingBindGroups>,
);

#[derive(Clone)]
pub struct ViewPhaseBinUnpackingBindGroups {

    indexed: Vec<ViewPhaseBinUnpackingBindGroup>,

    non_indexed: Vec<ViewPhaseBinUnpackingBindGroup>,
}

#[derive(Clone)]
pub struct ViewPhaseBinUnpackingBindGroup {

    pub metadata_index: BinUnpackingMetadataIndex,

    pub bind_group: BindGroup,

    pub mesh_instance_count: u32,
}

#[derive(Component, Default)]
pub struct SkipGpuPreprocess;

type WithAnyPrepass = Or<(
    With<DepthPrepass>,
    With<NormalPrepass>,
    With<MotionVectorPrepass>,
    With<DeferredPrepass>,
)>;

impl Plugin for GpuMeshPreprocessPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "mesh_preprocess.wgsl");
        embedded_asset!(app, "reset_indirect_batch_sets.wgsl");
        embedded_asset!(app, "build_indirect_params.wgsl");
        embedded_asset!(app, "unpack_bins.wgsl");
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        let gpu_preprocessing_support = render_app.world().resource::<GpuPreprocessingSupport>();
        if !self.use_gpu_instance_buffer_builder || !gpu_preprocessing_support.is_available() {
            return;
        }

        render_app
            .init_gpu_resource::<BinUnpackingBindGroups>()
            .init_gpu_resource::<PreprocessPipelines>()
            .init_gpu_resource::<SpecializedComputePipelines<PreprocessPipeline>>()
            .init_gpu_resource::<SpecializedComputePipelines<ResetIndirectBatchSetsPipeline>>()
            .init_gpu_resource::<SpecializedComputePipelines<BuildIndirectParametersPipeline>>()
            .init_gpu_resource::<SpecializedComputePipelines<BinUnpackingPipeline>>()
            .add_systems(
                Render,
                (
                    clear_bin_unpacking_buffers.in_set(RenderSystems::PrepareResources),
                    prepare_preprocess_pipelines.in_set(RenderSystems::Prepare),
                    prepare_preprocess_bind_groups
                        .run_if(resource_exists::<BatchedInstanceBuffers<
                            MeshUniform,
                            MeshInputUniform
                        >>)
                        .in_set(RenderSystems::PrepareBindGroups)
                        .after(prepare_preprocess_pipelines),
                    write_mesh_culling_data_buffer.in_set(RenderSystems::PrepareResourcesFlush),
                ),
            )
            .add_systems(
                Core3d,
                (
                    (
                        clear_indirect_parameters_metadata,
                        unpack_bins,
                        early_gpu_preprocess,
                        early_prepass_build_indirect_parameters.run_if(any_match_filter::<(
                            With<PreprocessBindGroups>,
                            Without<SkipGpuPreprocess>,
                            Without<NoIndirectDrawing>,
                            Or<(WithAnyPrepass, With<ShadowView>)>,
                        )>),
                    )
                        .chain()
                        .before(early_prepass),
                    (
                        late_gpu_preprocess,
                        late_prepass_build_indirect_parameters.run_if(any_match_filter::<(
                            With<PreprocessBindGroups>,
                            Without<SkipGpuPreprocess>,
                            Without<NoIndirectDrawing>,
                            Or<(WithAnyPrepass, With<ShadowView>)>,
                            With<OcclusionCulling>,
                        )>),
                    )
                        .chain()
                        .after(early_downsample_depth)
                        .before(late_prepass),
                    main_build_indirect_parameters
                        .run_if(any_match_filter::<(
                            With<PreprocessBindGroups>,
                            Without<SkipGpuPreprocess>,
                            Without<NoIndirectDrawing>,
                        )>)
                        .after(late_prepass_build_indirect_parameters)
                        .after(late_deferred_prepass)
                        .before(Core3dSystems::MainPass),
                ),
            );
    }
}

pub fn clear_indirect_parameters_metadata(
    indirect_parameters_buffers: Option<Res<IndirectParametersBuffers>>,
    mut ctx: RenderContext,
) {
    let Some(indirect_parameters_buffers) = indirect_parameters_buffers else {
        return;
    };

    for phase_indirect_parameters_buffers in indirect_parameters_buffers.values() {
        if let Some(indexed_gpu_metadata_buffer) = phase_indirect_parameters_buffers
            .indexed
            .gpu_metadata_buffer()
        {
            ctx.command_encoder().clear_buffer(
                indexed_gpu_metadata_buffer,
                0,
                Some(
                    phase_indirect_parameters_buffers.indexed.batch_count() as u64
                        * size_of::<IndirectParametersGpuMetadata>() as u64,
                ),
            );
        }

        if let Some(non_indexed_gpu_metadata_buffer) = phase_indirect_parameters_buffers
            .non_indexed
            .gpu_metadata_buffer()
        {
            ctx.command_encoder().clear_buffer(
                non_indexed_gpu_metadata_buffer,
                0,
                Some(
                    phase_indirect_parameters_buffers.non_indexed.batch_count() as u64
                        * size_of::<IndirectParametersGpuMetadata>() as u64,
                ),
            );
        }
    }
}

pub fn unpack_bins(
    current_view: ViewQuery<Option<&ViewLightEntities>, Without<SkipGpuPreprocess>>,
    view_query: Query<&ExtractedView, Without<SkipGpuPreprocess>>,
    light_query: Query<&LightEntity>,
    batched_instance_buffers: Res<BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>,
    pipeline_cache: Res<PipelineCache>,
    preprocess_pipelines: Res<PreprocessPipelines>,
    bin_unpacking_bind_groups: Res<BinUnpackingBindGroups>,
    mut render_context: RenderContext,
) {
    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();

    let command_encoder = render_context.command_encoder();
    let mut compute_pass = command_encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("bin unpacking"),
        timestamp_writes: None,
    });

    let pass_span = diagnostics.pass_span(&mut compute_pass, "bin_unpacking");

    let view_entity = current_view.entity();
    let shadow_cascade_views = current_view.into_inner();
    let all_views =
        gather_shadow_cascades_for_view(view_entity, shadow_cascade_views, &light_query);

    if let Some(bin_unpacking_pipeline_id) = preprocess_pipelines.bin_unpacking.pipeline_id
        && let Some(bin_unpacking_pipeline) =
            pipeline_cache.get_compute_pipeline(bin_unpacking_pipeline_id)
    {
        compute_pass.set_pipeline(bin_unpacking_pipeline);

        for view_entity in all_views {
            let Ok(view) = view_query.get(view_entity) else {
                continue;
            };

            for phase_type_id in batched_instance_buffers.phase_instance_buffers.keys() {
                let bin_unpacking_buffers_key = BinUnpackingBuffersKey {
                    phase: *phase_type_id,
                    view: view.retained_view_entity,
                };

                let Some(phase_bin_unpacking_bind_groups) =
                    bin_unpacking_bind_groups.get(&bin_unpacking_buffers_key)
                else {
                    continue;
                };

                for bin_unpacking_bind_group in phase_bin_unpacking_bind_groups
                    .indexed
                    .iter()
                    .chain(phase_bin_unpacking_bind_groups.non_indexed.iter())
                {
                    compute_pass.set_bind_group(0, &bin_unpacking_bind_group.bind_group, &[]);
                    let workgroup_count = (bin_unpacking_bind_group.mesh_instance_count as usize)
                        .div_ceil(WORKGROUP_SIZE);
                    if workgroup_count > 0 {
                        compute_pass.dispatch_workgroups(workgroup_count as u32, 1, 1);
                    }
                }
            }
        }
    }

    pass_span.end(&mut compute_pass);
}

pub fn early_gpu_preprocess(
    current_view: ViewQuery<Option<&ViewLightEntities>, Without<SkipGpuPreprocess>>,
    view_query: Query<
        (
            &ExtractedView,
            Option<&PreprocessBindGroups>,
            Option<&ViewUniformOffset>,
            Has<NoIndirectDrawing>,
            Has<OcclusionCulling>,
        ),
        Without<SkipGpuPreprocess>,
    >,
    light_query: Query<&LightEntity>,
    batched_instance_buffers: Res<BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>,
    pipeline_cache: Res<PipelineCache>,
    preprocess_pipelines: Res<PreprocessPipelines>,
    mut ctx: RenderContext,
) {
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();

    let command_encoder = ctx.command_encoder();

    let mut compute_pass = command_encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("early_mesh_preprocessing"),
        timestamp_writes: None,
    });

    let pass_span = diagnostics.pass_span(&mut compute_pass, "early_mesh_preprocessing");

    let view_entity = current_view.entity();
    let shadow_cascade_views = current_view.into_inner();
    let all_views =
        gather_shadow_cascades_for_view(view_entity, shadow_cascade_views, &light_query);

    for view_entity in all_views {
        let Ok((view, bind_groups, view_uniform_offset, no_indirect_drawing, occlusion_culling)) =
            view_query.get(view_entity)
        else {
            continue;
        };

        let Some(bind_groups) = bind_groups else {
            continue;
        };
        let Some(view_uniform_offset) = view_uniform_offset else {
            continue;
        };

        let maybe_pipeline_id = if no_indirect_drawing {
            preprocess_pipelines.direct_preprocess.pipeline_id
        } else if occlusion_culling {
            preprocess_pipelines
                .early_gpu_occlusion_culling_preprocess
                .pipeline_id
        } else {
            preprocess_pipelines
                .gpu_frustum_culling_preprocess
                .pipeline_id
        };

        let Some(preprocess_pipeline_id) = maybe_pipeline_id else {
            warn!("The build mesh uniforms pipeline wasn't ready");
            continue;
        };

        let Some(preprocess_pipeline) = pipeline_cache.get_compute_pipeline(preprocess_pipeline_id)
        else {

            continue;
        };

        compute_pass.set_pipeline(preprocess_pipeline);

        for (phase_type_id, batched_phase_instance_buffers) in
            &batched_instance_buffers.phase_instance_buffers
        {

            let Some(work_item_buffers) = batched_phase_instance_buffers
                .work_item_buffers
                .get(&view.retained_view_entity)
            else {
                continue;
            };

            let Some(phase_bind_groups) = bind_groups.get(phase_type_id) else {
                continue;
            };

            let dynamic_offsets = [view_uniform_offset.offset];

            match *phase_bind_groups {
                PhasePreprocessBindGroups::Direct(ref bind_group) => {

                    let PreprocessWorkItemBuffers::Direct(work_item_buffer) = work_item_buffers
                    else {
                        continue;
                    };
                    compute_pass.set_bind_group(0, bind_group, &dynamic_offsets);
                    let workgroup_count = work_item_buffer.len().div_ceil(WORKGROUP_SIZE);
                    if workgroup_count > 0 {
                        compute_pass.dispatch_workgroups(workgroup_count as u32, 1, 1);
                    }
                }

                PhasePreprocessBindGroups::IndirectFrustumCulling {
                    indexed: ref maybe_indexed_bind_group,
                    non_indexed: ref maybe_non_indexed_bind_group,
                }
                | PhasePreprocessBindGroups::IndirectOcclusionCulling {
                    early_indexed: ref maybe_indexed_bind_group,
                    early_non_indexed: ref maybe_non_indexed_bind_group,
                    ..
                } => {

                    let PreprocessWorkItemBuffers::Indirect {
                        indexed: indexed_buffer,
                        non_indexed: non_indexed_buffer,
                        ..
                    } = work_item_buffers
                    else {
                        continue;
                    };

                    if let Some(indexed_bind_group) = maybe_indexed_bind_group {
                        if let PreprocessWorkItemBuffers::Indirect {
                            gpu_occlusion_culling:
                                Some(GpuOcclusionCullingWorkItemBuffers {
                                    late_indirect_parameters_indexed_offset,
                                    ..
                                }),
                            ..
                        } = *work_item_buffers
                        {
                            compute_pass.set_immediates(
                                0,
                                bytemuck::bytes_of(&late_indirect_parameters_indexed_offset),
                            );
                        }

                        compute_pass.set_bind_group(0, indexed_bind_group, &dynamic_offsets);
                        let workgroup_count = indexed_buffer.len().div_ceil(WORKGROUP_SIZE);
                        if workgroup_count > 0 {
                            compute_pass.dispatch_workgroups(workgroup_count as u32, 1, 1);
                        }
                    }

                    if let Some(non_indexed_bind_group) = maybe_non_indexed_bind_group {
                        if let PreprocessWorkItemBuffers::Indirect {
                            gpu_occlusion_culling:
                                Some(GpuOcclusionCullingWorkItemBuffers {
                                    late_indirect_parameters_non_indexed_offset,
                                    ..
                                }),
                            ..
                        } = *work_item_buffers
                        {
                            compute_pass.set_immediates(
                                0,
                                bytemuck::bytes_of(&late_indirect_parameters_non_indexed_offset),
                            );
                        }

                        compute_pass.set_bind_group(0, non_indexed_bind_group, &dynamic_offsets);
                        let workgroup_count = non_indexed_buffer.len().div_ceil(WORKGROUP_SIZE);
                        if workgroup_count > 0 {
                            compute_pass.dispatch_workgroups(workgroup_count as u32, 1, 1);
                        }
                    }
                }
            }
        }
    }

    pass_span.end(&mut compute_pass);
}

fn gather_shadow_cascades_for_view(
    view_entity: Entity,
    shadow_cascade_views: Option<&ViewLightEntities>,
    light_query: &Query<&LightEntity>,
) -> SmallVec<[Entity; 8]> {
    let mut all_views: SmallVec<[_; 8]> = SmallVec::new();
    all_views.push(view_entity);
    if let Some(shadow_cascade_views) = shadow_cascade_views {
        all_views.extend(
            shadow_cascade_views
                .lights
                .iter()
                .filter(|light_entity| {
                    light_query.get(**light_entity).is_ok_and(|light_entity| {
                        matches!(*light_entity, LightEntity::Directional { .. })
                    })
                })
                .copied(),
        );
    }
    all_views
}

pub fn late_gpu_preprocess(
    current_view: ViewQuery<
        (&ExtractedView, &PreprocessBindGroups, &ViewUniformOffset),
        (
            Without<SkipGpuPreprocess>,
            Without<NoIndirectDrawing>,
            With<OcclusionCulling>,
            With<DepthPrepass>,
        ),
    >,
    batched_instance_buffers: Res<BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>,
    pipeline_cache: Res<PipelineCache>,
    preprocess_pipelines: Res<PreprocessPipelines>,
    mut ctx: RenderContext,
) {
    let (view, bind_groups, view_uniform_offset) = current_view.into_inner();

    let maybe_pipeline_id = preprocess_pipelines
        .late_gpu_occlusion_culling_preprocess
        .pipeline_id;

    let Some(preprocess_pipeline_id) = maybe_pipeline_id else {
        warn_once!("The build mesh uniforms pipeline wasn't ready");
        return;
    };

    let Some(preprocess_pipeline) = pipeline_cache.get_compute_pipeline(preprocess_pipeline_id)
    else {

        return;
    };

    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();

    let command_encoder = ctx.command_encoder();

    let mut compute_pass = command_encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("late_mesh_preprocessing"),
        timestamp_writes: None,
    });

    let pass_span = diagnostics.pass_span(&mut compute_pass, "late_mesh_preprocessing");

    compute_pass.set_pipeline(preprocess_pipeline);

    for (phase_type_id, batched_phase_instance_buffers) in
        &batched_instance_buffers.phase_instance_buffers
    {
        let UntypedPhaseBatchedInstanceBuffers {
            ref work_item_buffers,
            ref late_indexed_indirect_parameters_buffer,
            ref late_non_indexed_indirect_parameters_buffer,
            ..
        } = *batched_phase_instance_buffers;

        let Some(phase_work_item_buffers) = work_item_buffers.get(&view.retained_view_entity)
        else {
            continue;
        };

        let (
            PreprocessWorkItemBuffers::Indirect {
                gpu_occlusion_culling:
                    Some(GpuOcclusionCullingWorkItemBuffers {
                        late_indirect_parameters_indexed_offset,
                        late_indirect_parameters_non_indexed_offset,
                        ..
                    }),
                ..
            },
            Some(PhasePreprocessBindGroups::IndirectOcclusionCulling {
                late_indexed: maybe_late_indexed_bind_group,
                late_non_indexed: maybe_late_non_indexed_bind_group,
                ..
            }),
            Some(late_indexed_indirect_parameters_buffer),
            Some(late_non_indexed_indirect_parameters_buffer),
        ) = (
            phase_work_item_buffers,
            bind_groups.get(phase_type_id),
            late_indexed_indirect_parameters_buffer.buffer(),
            late_non_indexed_indirect_parameters_buffer.buffer(),
        )
        else {
            continue;
        };

        let mut dynamic_offsets: SmallVec<[u32; 1]> = smallvec![];
        dynamic_offsets.push(view_uniform_offset.offset);

        if let Some(late_indexed_bind_group) = maybe_late_indexed_bind_group {
            compute_pass.set_immediates(
                0,
                bytemuck::bytes_of(late_indirect_parameters_indexed_offset),
            );

            compute_pass.set_bind_group(0, late_indexed_bind_group, &dynamic_offsets);
            compute_pass.dispatch_workgroups_indirect(
                late_indexed_indirect_parameters_buffer,
                (*late_indirect_parameters_indexed_offset as u64)
                    * (size_of::<LatePreprocessWorkItemIndirectParameters>() as u64),
            );
        }

        if let Some(late_non_indexed_bind_group) = maybe_late_non_indexed_bind_group {
            compute_pass.set_immediates(
                0,
                bytemuck::bytes_of(late_indirect_parameters_non_indexed_offset),
            );

            compute_pass.set_bind_group(0, late_non_indexed_bind_group, &dynamic_offsets);
            compute_pass.dispatch_workgroups_indirect(
                late_non_indexed_indirect_parameters_buffer,
                (*late_indirect_parameters_non_indexed_offset as u64)
                    * (size_of::<LatePreprocessWorkItemIndirectParameters>() as u64),
            );
        }
    }

    pass_span.end(&mut compute_pass);
}

pub fn early_prepass_build_indirect_parameters(
    preprocess_pipelines: Res<PreprocessPipelines>,
    build_indirect_params_bind_groups: Option<Res<BuildIndirectParametersBindGroups>>,
    pipeline_cache: Res<PipelineCache>,
    indirect_parameters_buffers: Option<Res<IndirectParametersBuffers>>,
    mut ctx: RenderContext,
) {
    run_build_indirect_parameters(
        &mut ctx,
        build_indirect_params_bind_groups.as_deref(),
        &pipeline_cache,
        indirect_parameters_buffers.as_deref(),
        &preprocess_pipelines.early_phase,
        "early_prepass_indirect_parameters_building",
    );
}

pub fn late_prepass_build_indirect_parameters(
    preprocess_pipelines: Res<PreprocessPipelines>,
    build_indirect_params_bind_groups: Option<Res<BuildIndirectParametersBindGroups>>,
    pipeline_cache: Res<PipelineCache>,
    indirect_parameters_buffers: Option<Res<IndirectParametersBuffers>>,
    mut ctx: RenderContext,
) {
    run_build_indirect_parameters(
        &mut ctx,
        build_indirect_params_bind_groups.as_deref(),
        &pipeline_cache,
        indirect_parameters_buffers.as_deref(),
        &preprocess_pipelines.late_phase,
        "late_prepass_indirect_parameters_building",
    );
}

pub fn main_build_indirect_parameters(
    _current_view: ViewQuery<Entity, Without<ShadowView>>,
    preprocess_pipelines: Res<PreprocessPipelines>,
    build_indirect_params_bind_groups: Option<Res<BuildIndirectParametersBindGroups>>,
    pipeline_cache: Res<PipelineCache>,
    indirect_parameters_buffers: Option<Res<IndirectParametersBuffers>>,
    mut ctx: RenderContext,
) {
    run_build_indirect_parameters(
        &mut ctx,
        build_indirect_params_bind_groups.as_deref(),
        &pipeline_cache,
        indirect_parameters_buffers.as_deref(),
        &preprocess_pipelines.main_phase,
        "main_indirect_parameters_building",
    );
}

fn run_build_indirect_parameters(
    ctx: &mut RenderContext,
    build_indirect_params_bind_groups: Option<&BuildIndirectParametersBindGroups>,
    pipeline_cache: &PipelineCache,
    indirect_parameters_buffers: Option<&IndirectParametersBuffers>,
    preprocess_phase_pipelines: &PreprocessPhasePipelines,
    label: &'static str,
) {
    let Some(build_indirect_params_bind_groups) = build_indirect_params_bind_groups else {
        return;
    };

    let Some(indirect_parameters_buffers) = indirect_parameters_buffers else {
        return;
    };

    let command_encoder = ctx.command_encoder();

    let mut compute_pass = command_encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some(label),
        timestamp_writes: None,
    });

    let (
        Some(reset_indirect_batch_sets_pipeline_id),
        Some(build_indexed_indirect_params_pipeline_id),
        Some(build_non_indexed_indirect_params_pipeline_id),
    ) = (
        preprocess_phase_pipelines
            .reset_indirect_batch_sets
            .pipeline_id,
        preprocess_phase_pipelines
            .gpu_occlusion_culling_build_indexed_indirect_params
            .pipeline_id,
        preprocess_phase_pipelines
            .gpu_occlusion_culling_build_non_indexed_indirect_params
            .pipeline_id,
    )
    else {
        warn!("The build indirect parameters pipelines weren't ready");
        return;
    };

    let (
        Some(reset_indirect_batch_sets_pipeline),
        Some(build_indexed_indirect_params_pipeline),
        Some(build_non_indexed_indirect_params_pipeline),
    ) = (
        pipeline_cache.get_compute_pipeline(reset_indirect_batch_sets_pipeline_id),
        pipeline_cache.get_compute_pipeline(build_indexed_indirect_params_pipeline_id),
        pipeline_cache.get_compute_pipeline(build_non_indexed_indirect_params_pipeline_id),
    )
    else {

        return;
    };

    for (phase_type_id, phase_build_indirect_params_bind_groups) in
        build_indirect_params_bind_groups.iter()
    {
        let Some(phase_indirect_parameters_buffers) =
            indirect_parameters_buffers.get(phase_type_id)
        else {
            continue;
        };

        if let (
            Some(reset_indexed_indirect_batch_sets_bind_group),
            Some(build_indirect_indexed_params_bind_group),
        ) = (
            &phase_build_indirect_params_bind_groups.reset_indexed_indirect_batch_sets,
            &phase_build_indirect_params_bind_groups.build_indexed_indirect,
        ) {
            compute_pass.set_pipeline(reset_indirect_batch_sets_pipeline);
            compute_pass.set_bind_group(0, reset_indexed_indirect_batch_sets_bind_group, &[]);
            let workgroup_count = phase_indirect_parameters_buffers
                .batch_set_count(true)
                .div_ceil(WORKGROUP_SIZE);
            if workgroup_count > 0 {
                compute_pass.dispatch_workgroups(workgroup_count as u32, 1, 1);
            }

            compute_pass.set_pipeline(build_indexed_indirect_params_pipeline);
            compute_pass.set_bind_group(0, build_indirect_indexed_params_bind_group, &[]);
            let workgroup_count = phase_indirect_parameters_buffers
                .indexed
                .batch_count()
                .div_ceil(WORKGROUP_SIZE);
            if workgroup_count > 0 {
                compute_pass.dispatch_workgroups(workgroup_count as u32, 1, 1);
            }
        }

        if let (
            Some(reset_non_indexed_indirect_batch_sets_bind_group),
            Some(build_indirect_non_indexed_params_bind_group),
        ) = (
            &phase_build_indirect_params_bind_groups.reset_non_indexed_indirect_batch_sets,
            &phase_build_indirect_params_bind_groups.build_non_indexed_indirect,
        ) {
            compute_pass.set_pipeline(reset_indirect_batch_sets_pipeline);
            compute_pass.set_bind_group(0, reset_non_indexed_indirect_batch_sets_bind_group, &[]);
            let workgroup_count = phase_indirect_parameters_buffers
                .batch_set_count(false)
                .div_ceil(WORKGROUP_SIZE);
            if workgroup_count > 0 {
                compute_pass.dispatch_workgroups(workgroup_count as u32, 1, 1);
            }

            compute_pass.set_pipeline(build_non_indexed_indirect_params_pipeline);
            compute_pass.set_bind_group(0, build_indirect_non_indexed_params_bind_group, &[]);
            let workgroup_count = phase_indirect_parameters_buffers
                .non_indexed
                .batch_count()
                .div_ceil(WORKGROUP_SIZE);
            if workgroup_count > 0 {
                compute_pass.dispatch_workgroups(workgroup_count as u32, 1, 1);
            }
        }
    }
}

impl PreprocessPipelines {

    pub(crate) fn pipelines_are_loaded(
        &self,
        pipeline_cache: &PipelineCache,
        preprocessing_support: &GpuPreprocessingSupport,
    ) -> bool {
        match preprocessing_support.max_supported_mode {
            GpuPreprocessingMode::None => false,
            GpuPreprocessingMode::PreprocessingOnly => {
                self.direct_preprocess.is_loaded(pipeline_cache)
                    && self
                        .gpu_frustum_culling_preprocess
                        .is_loaded(pipeline_cache)
            }
            GpuPreprocessingMode::Culling => {
                self.direct_preprocess.is_loaded(pipeline_cache)
                    && self
                        .gpu_frustum_culling_preprocess
                        .is_loaded(pipeline_cache)
                    && self
                        .early_gpu_occlusion_culling_preprocess
                        .is_loaded(pipeline_cache)
                    && self
                        .late_gpu_occlusion_culling_preprocess
                        .is_loaded(pipeline_cache)
                    && self
                        .gpu_frustum_culling_build_indexed_indirect_params
                        .is_loaded(pipeline_cache)
                    && self
                        .gpu_frustum_culling_build_non_indexed_indirect_params
                        .is_loaded(pipeline_cache)
                    && self.early_phase.is_loaded(pipeline_cache)
                    && self.late_phase.is_loaded(pipeline_cache)
                    && self.main_phase.is_loaded(pipeline_cache)
            }
        }
    }
}

impl PreprocessPhasePipelines {
    fn is_loaded(&self, pipeline_cache: &PipelineCache) -> bool {
        self.reset_indirect_batch_sets.is_loaded(pipeline_cache)
            && self
                .gpu_occlusion_culling_build_indexed_indirect_params
                .is_loaded(pipeline_cache)
            && self
                .gpu_occlusion_culling_build_non_indexed_indirect_params
                .is_loaded(pipeline_cache)
    }
}

impl PreprocessPipeline {
    fn is_loaded(&self, pipeline_cache: &PipelineCache) -> bool {
        self.pipeline_id
            .is_some_and(|pipeline_id| pipeline_cache.get_compute_pipeline(pipeline_id).is_some())
    }
}

impl ResetIndirectBatchSetsPipeline {
    fn is_loaded(&self, pipeline_cache: &PipelineCache) -> bool {
        self.pipeline_id
            .is_some_and(|pipeline_id| pipeline_cache.get_compute_pipeline(pipeline_id).is_some())
    }
}

impl BuildIndirectParametersPipeline {

    fn is_loaded(&self, pipeline_cache: &PipelineCache) -> bool {
        self.pipeline_id
            .is_some_and(|pipeline_id| pipeline_cache.get_compute_pipeline(pipeline_id).is_some())
    }
}

impl SpecializedComputePipeline for PreprocessPipeline {
    type Key = PreprocessPipelineKey;

    fn specialize(&self, key: Self::Key) -> ComputePipelineDescriptor {
        let mut shader_defs = vec!["WRITE_INDIRECT_PARAMETERS_METADATA".into()];
        if key.contains(PreprocessPipelineKey::FRUSTUM_CULLING) {
            shader_defs.push("INDIRECT".into());
            shader_defs.push("FRUSTUM_CULLING".into());
        }
        if key.contains(PreprocessPipelineKey::OCCLUSION_CULLING) {
            shader_defs.push("OCCLUSION_CULLING".into());
            if key.contains(PreprocessPipelineKey::EARLY_PHASE) {
                shader_defs.push("EARLY_PHASE".into());
            } else {
                shader_defs.push("LATE_PHASE".into());
            }
        }

        ComputePipelineDescriptor {
            label: Some(
                format!(
                    "mesh preprocessing ({})",
                    if key.contains(
                        PreprocessPipelineKey::OCCLUSION_CULLING
                            | PreprocessPipelineKey::EARLY_PHASE
                    ) {
                        "early GPU occlusion culling"
                    } else if key.contains(PreprocessPipelineKey::OCCLUSION_CULLING) {
                        "late GPU occlusion culling"
                    } else if key.contains(PreprocessPipelineKey::FRUSTUM_CULLING) {
                        "GPU frustum culling"
                    } else {
                        "direct"
                    }
                )
                .into(),
            ),
            layout: vec![self.bind_group_layout.clone()],
            immediate_size: if key.contains(PreprocessPipelineKey::OCCLUSION_CULLING) {
                4
            } else {
                0
            },
            shader: self.shader.clone(),
            shader_defs,
            ..default()
        }
    }
}

impl FromWorld for PreprocessPipelines {
    fn from_world(world: &mut World) -> Self {

        let direct_bind_group_layout_entries = preprocess_direct_bind_group_layout_entries();
        let gpu_frustum_culling_bind_group_layout_entries = gpu_culling_bind_group_layout_entries();
        let gpu_early_occlusion_culling_bind_group_layout_entries =
            gpu_occlusion_culling_bind_group_layout_entries().extend_with_indices((
                (
                    12,
                    storage_buffer::<PreprocessWorkItem>(                        false),
                ),
                (
                    13,
                    storage_buffer::<LatePreprocessWorkItemIndirectParameters>(
                                                false,
                    ),
                ),
            ));
        let gpu_late_occlusion_culling_bind_group_layout_entries =
            gpu_occlusion_culling_bind_group_layout_entries().extend_with_indices(((
                13,
                storage_buffer_read_only::<LatePreprocessWorkItemIndirectParameters>(
                                            false,
                ),
            ),));

        let reset_indirect_batch_sets_bind_group_layout_entries =
            DynamicBindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (storage_buffer::<IndirectBatchSet>(false),),
            );

        let build_indexed_indirect_params_bind_group_layout_entries =
            build_indirect_params_bind_group_layout_entries()
                .extend_sequential((storage_buffer::<IndirectParametersIndexed>(false),));
        let build_non_indexed_indirect_params_bind_group_layout_entries =
            build_indirect_params_bind_group_layout_entries()
                .extend_sequential((storage_buffer::<IndirectParametersNonIndexed>(false),));

        let bin_unpacking_bind_group_layout_entries = bin_unpacking_bind_group_layout_entries();

        let direct_bind_group_layout = BindGroupLayoutDescriptor::new(
            "build mesh uniforms direct bind group layout",
            &direct_bind_group_layout_entries,
        );
        let gpu_frustum_culling_bind_group_layout = BindGroupLayoutDescriptor::new(
            "build mesh uniforms GPU frustum culling bind group layout",
            &gpu_frustum_culling_bind_group_layout_entries,
        );
        let gpu_early_occlusion_culling_bind_group_layout = BindGroupLayoutDescriptor::new(
            "build mesh uniforms GPU early occlusion culling bind group layout",
            &gpu_early_occlusion_culling_bind_group_layout_entries,
        );
        let gpu_late_occlusion_culling_bind_group_layout = BindGroupLayoutDescriptor::new(
            "build mesh uniforms GPU late occlusion culling bind group layout",
            &gpu_late_occlusion_culling_bind_group_layout_entries,
        );
        let reset_indirect_batch_sets_bind_group_layout = BindGroupLayoutDescriptor::new(
            "reset indirect batch sets bind group layout",
            &reset_indirect_batch_sets_bind_group_layout_entries,
        );
        let build_indexed_indirect_params_bind_group_layout = BindGroupLayoutDescriptor::new(
            "build indexed indirect parameters bind group layout",
            &build_indexed_indirect_params_bind_group_layout_entries,
        );
        let build_non_indexed_indirect_params_bind_group_layout = BindGroupLayoutDescriptor::new(
            "build non-indexed indirect parameters bind group layout",
            &build_non_indexed_indirect_params_bind_group_layout_entries,
        );
        let bin_unpacking_bind_group_layout = BindGroupLayoutDescriptor::new(
            "bin unpacking bind group layout",
            &bin_unpacking_bind_group_layout_entries,
        );

        let preprocess_shader = load_embedded_asset!(world, "mesh_preprocess.wgsl");
        let reset_indirect_batch_sets_shader =
            load_embedded_asset!(world, "reset_indirect_batch_sets.wgsl");
        let build_indirect_params_shader =
            load_embedded_asset!(world, "build_indirect_params.wgsl");
        let bin_unpacking_shader = load_embedded_asset!(world, "unpack_bins.wgsl");

        let preprocess_phase_pipelines = PreprocessPhasePipelines {
            reset_indirect_batch_sets: ResetIndirectBatchSetsPipeline {
                bind_group_layout: reset_indirect_batch_sets_bind_group_layout.clone(),
                shader: reset_indirect_batch_sets_shader,
                pipeline_id: None,
            },
            gpu_occlusion_culling_build_indexed_indirect_params: BuildIndirectParametersPipeline {
                bind_group_layout: build_indexed_indirect_params_bind_group_layout.clone(),
                shader: build_indirect_params_shader.clone(),
                pipeline_id: None,
            },
            gpu_occlusion_culling_build_non_indexed_indirect_params:
                BuildIndirectParametersPipeline {
                    bind_group_layout: build_non_indexed_indirect_params_bind_group_layout.clone(),
                    shader: build_indirect_params_shader.clone(),
                    pipeline_id: None,
                },
        };

        PreprocessPipelines {
            direct_preprocess: PreprocessPipeline {
                bind_group_layout: direct_bind_group_layout,
                shader: preprocess_shader.clone(),
                pipeline_id: None,
            },
            gpu_frustum_culling_preprocess: PreprocessPipeline {
                bind_group_layout: gpu_frustum_culling_bind_group_layout,
                shader: preprocess_shader.clone(),
                pipeline_id: None,
            },
            early_gpu_occlusion_culling_preprocess: PreprocessPipeline {
                bind_group_layout: gpu_early_occlusion_culling_bind_group_layout,
                shader: preprocess_shader.clone(),
                pipeline_id: None,
            },
            late_gpu_occlusion_culling_preprocess: PreprocessPipeline {
                bind_group_layout: gpu_late_occlusion_culling_bind_group_layout,
                shader: preprocess_shader,
                pipeline_id: None,
            },
            gpu_frustum_culling_build_indexed_indirect_params: BuildIndirectParametersPipeline {
                bind_group_layout: build_indexed_indirect_params_bind_group_layout.clone(),
                shader: build_indirect_params_shader.clone(),
                pipeline_id: None,
            },
            gpu_frustum_culling_build_non_indexed_indirect_params:
                BuildIndirectParametersPipeline {
                    bind_group_layout: build_non_indexed_indirect_params_bind_group_layout.clone(),
                    shader: build_indirect_params_shader,
                    pipeline_id: None,
                },
            early_phase: preprocess_phase_pipelines.clone(),
            late_phase: preprocess_phase_pipelines.clone(),
            main_phase: preprocess_phase_pipelines.clone(),
            bin_unpacking: BinUnpackingPipeline {
                bind_group_layout: bin_unpacking_bind_group_layout,
                shader: bin_unpacking_shader,
                pipeline_id: None,
            },
        }
    }
}

fn preprocess_direct_bind_group_layout_entries() -> DynamicBindGroupLayoutEntries {
    DynamicBindGroupLayoutEntries::new_with_indices(
        ShaderStages::COMPUTE,
        (

            (
                0,
                uniform_buffer::<ViewUniform>(                          true),
            ),

            (3, storage_buffer_read_only::<MeshInputUniform>(false)),

            (4, storage_buffer_read_only::<MeshInputUniform>(false)),

            (5, storage_buffer_read_only::<PreprocessWorkItem>(false)),

            (6, storage_buffer::<MeshUniform>(false)),
        ),
    )
}

fn build_indirect_params_bind_group_layout_entries() -> DynamicBindGroupLayoutEntries {
    DynamicBindGroupLayoutEntries::new_with_indices(
        ShaderStages::COMPUTE,
        (
            (0, storage_buffer_read_only::<MeshInputUniform>(false)),
            (
                1,
                storage_buffer_read_only::<IndirectParametersCpuMetadata>(false),
            ),
            (
                2,
                storage_buffer_read_only::<IndirectParametersGpuMetadata>(false),
            ),
            (3, storage_buffer::<IndirectBatchSet>(false)),
        ),
    )
}

fn gpu_culling_bind_group_layout_entries() -> DynamicBindGroupLayoutEntries {

    preprocess_direct_bind_group_layout_entries().extend_with_indices((

        (
            7,
            storage_buffer_read_only::<IndirectParametersCpuMetadata>(
                                          false,
            ),
        ),

        (
            8,
            storage_buffer::<IndirectParametersGpuMetadata>(                          false),
        ),

        (
            9,
            storage_buffer_read_only::<MeshCullingData>(                          false),
        ),

        (
            10,
            storage_buffer_read_only::<Vec4>(                          false),
        ),
    ))
}

fn gpu_occlusion_culling_bind_group_layout_entries() -> DynamicBindGroupLayoutEntries {
    gpu_culling_bind_group_layout_entries().extend_with_indices((
        (
            2,
            uniform_buffer::<PreviousViewData>(                        false),
        ),
        (
            11,
            texture_2d(TextureSampleType::Float { filterable: true }),
        ),
    ))
}

fn bin_unpacking_bind_group_layout_entries() -> BindGroupLayoutEntries<4> {
    BindGroupLayoutEntries::sequential(
        ShaderStages::COMPUTE,
        (

            uniform_buffer::<GpuBinUnpackingMetadata>(false),

            storage_buffer_read_only::<GpuRenderBinnedMeshInstance>(false),

            storage_buffer::<PreprocessWorkItem>(false),

            storage_buffer_read_only::<u32>(false),
        ),
    )
}

pub fn prepare_preprocess_pipelines(
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    mut specialized_preprocess_pipelines: ResMut<SpecializedComputePipelines<PreprocessPipeline>>,
    mut specialized_reset_indirect_batch_sets_pipelines: ResMut<
        SpecializedComputePipelines<ResetIndirectBatchSetsPipeline>,
    >,
    mut specialized_build_indirect_parameters_pipelines: ResMut<
        SpecializedComputePipelines<BuildIndirectParametersPipeline>,
    >,
    mut specialized_bin_unpacking_pipelines: ResMut<
        SpecializedComputePipelines<BinUnpackingPipeline>,
    >,
    preprocess_pipelines: ResMut<PreprocessPipelines>,
    gpu_preprocessing_support: Res<GpuPreprocessingSupport>,
) {
    let preprocess_pipelines = preprocess_pipelines.into_inner();

    preprocess_pipelines.direct_preprocess.prepare(
        &pipeline_cache,
        &mut specialized_preprocess_pipelines,
        PreprocessPipelineKey::empty(),
    );
    preprocess_pipelines.gpu_frustum_culling_preprocess.prepare(
        &pipeline_cache,
        &mut specialized_preprocess_pipelines,
        PreprocessPipelineKey::FRUSTUM_CULLING,
    );

    if gpu_preprocessing_support.is_culling_supported() {
        preprocess_pipelines
            .early_gpu_occlusion_culling_preprocess
            .prepare(
                &pipeline_cache,
                &mut specialized_preprocess_pipelines,
                PreprocessPipelineKey::FRUSTUM_CULLING
                    | PreprocessPipelineKey::OCCLUSION_CULLING
                    | PreprocessPipelineKey::EARLY_PHASE,
            );
        preprocess_pipelines
            .late_gpu_occlusion_culling_preprocess
            .prepare(
                &pipeline_cache,
                &mut specialized_preprocess_pipelines,
                PreprocessPipelineKey::FRUSTUM_CULLING | PreprocessPipelineKey::OCCLUSION_CULLING,
            );
    }

    let mut build_indirect_parameters_pipeline_key = BuildIndirectParametersPipelineKey::empty();

    if render_device
        .wgpu_device()
        .features()
        .contains(WgpuFeatures::MULTI_DRAW_INDIRECT_COUNT)
    {
        build_indirect_parameters_pipeline_key
            .insert(BuildIndirectParametersPipelineKey::MULTI_DRAW_INDIRECT_COUNT_SUPPORTED);
    }

    preprocess_pipelines
        .gpu_frustum_culling_build_indexed_indirect_params
        .prepare(
            &pipeline_cache,
            &mut specialized_build_indirect_parameters_pipelines,
            build_indirect_parameters_pipeline_key | BuildIndirectParametersPipelineKey::INDEXED,
        );
    preprocess_pipelines
        .gpu_frustum_culling_build_non_indexed_indirect_params
        .prepare(
            &pipeline_cache,
            &mut specialized_build_indirect_parameters_pipelines,
            build_indirect_parameters_pipeline_key,
        );

    if !gpu_preprocessing_support.is_culling_supported() {
        return;
    }

    for (preprocess_phase_pipelines, build_indirect_parameters_phase_pipeline_key) in [
        (
            &mut preprocess_pipelines.early_phase,
            BuildIndirectParametersPipelineKey::EARLY_PHASE,
        ),
        (
            &mut preprocess_pipelines.late_phase,
            BuildIndirectParametersPipelineKey::LATE_PHASE,
        ),
        (
            &mut preprocess_pipelines.main_phase,
            BuildIndirectParametersPipelineKey::MAIN_PHASE,
        ),
    ] {
        preprocess_phase_pipelines
            .reset_indirect_batch_sets
            .prepare(
                &pipeline_cache,
                &mut specialized_reset_indirect_batch_sets_pipelines,
            );
        preprocess_phase_pipelines
            .gpu_occlusion_culling_build_indexed_indirect_params
            .prepare(
                &pipeline_cache,
                &mut specialized_build_indirect_parameters_pipelines,
                build_indirect_parameters_pipeline_key
                    | build_indirect_parameters_phase_pipeline_key
                    | BuildIndirectParametersPipelineKey::INDEXED
                    | BuildIndirectParametersPipelineKey::OCCLUSION_CULLING,
            );
        preprocess_phase_pipelines
            .gpu_occlusion_culling_build_non_indexed_indirect_params
            .prepare(
                &pipeline_cache,
                &mut specialized_build_indirect_parameters_pipelines,
                build_indirect_parameters_pipeline_key
                    | build_indirect_parameters_phase_pipeline_key
                    | BuildIndirectParametersPipelineKey::OCCLUSION_CULLING,
            );
    }

    preprocess_pipelines
        .bin_unpacking
        .prepare(&pipeline_cache, &mut specialized_bin_unpacking_pipelines);
}

impl PreprocessPipeline {
    fn prepare(
        &mut self,
        pipeline_cache: &PipelineCache,
        pipelines: &mut SpecializedComputePipelines<PreprocessPipeline>,
        key: PreprocessPipelineKey,
    ) {
        if self.pipeline_id.is_some() {
            return;
        }

        let preprocess_pipeline_id = pipelines.specialize(pipeline_cache, self, key);
        self.pipeline_id = Some(preprocess_pipeline_id);
    }
}

impl SpecializedComputePipeline for ResetIndirectBatchSetsPipeline {
    type Key = ();

    fn specialize(&self, _: Self::Key) -> ComputePipelineDescriptor {
        ComputePipelineDescriptor {
            label: Some("reset indirect batch sets".into()),
            layout: vec![self.bind_group_layout.clone()],
            shader: self.shader.clone(),
            ..default()
        }
    }
}

impl SpecializedComputePipeline for BuildIndirectParametersPipeline {
    type Key = BuildIndirectParametersPipelineKey;

    fn specialize(&self, key: Self::Key) -> ComputePipelineDescriptor {
        let mut shader_defs = vec![];
        if key.contains(BuildIndirectParametersPipelineKey::INDEXED) {
            shader_defs.push("INDEXED".into());
        }
        if key.contains(BuildIndirectParametersPipelineKey::MULTI_DRAW_INDIRECT_COUNT_SUPPORTED) {
            shader_defs.push("MULTI_DRAW_INDIRECT_COUNT_SUPPORTED".into());
        }
        if key.contains(BuildIndirectParametersPipelineKey::OCCLUSION_CULLING) {
            shader_defs.push("OCCLUSION_CULLING".into());
        }
        if key.contains(BuildIndirectParametersPipelineKey::EARLY_PHASE) {
            shader_defs.push("EARLY_PHASE".into());
        }
        if key.contains(BuildIndirectParametersPipelineKey::LATE_PHASE) {
            shader_defs.push("LATE_PHASE".into());
        }
        if key.contains(BuildIndirectParametersPipelineKey::MAIN_PHASE) {
            shader_defs.push("MAIN_PHASE".into());
        }

        let label = format!(
            "{} build {}indexed indirect parameters",
            if !key.contains(BuildIndirectParametersPipelineKey::OCCLUSION_CULLING) {
                "frustum culling"
            } else if key.contains(BuildIndirectParametersPipelineKey::EARLY_PHASE) {
                "early occlusion culling"
            } else if key.contains(BuildIndirectParametersPipelineKey::LATE_PHASE) {
                "late occlusion culling"
            } else {
                "main occlusion culling"
            },
            if key.contains(BuildIndirectParametersPipelineKey::INDEXED) {
                ""
            } else {
                "non-"
            }
        );

        ComputePipelineDescriptor {
            label: Some(label.into()),
            layout: vec![self.bind_group_layout.clone()],
            shader: self.shader.clone(),
            shader_defs,
            ..default()
        }
    }
}

impl SpecializedComputePipeline for BinUnpackingPipeline {
    type Key = ();

    fn specialize(&self, _: Self::Key) -> ComputePipelineDescriptor {
        ComputePipelineDescriptor {
            label: Some("bin unpacking".into()),
            layout: vec![self.bind_group_layout.clone()],
            shader: self.shader.clone(),
            shader_defs: vec![],
            ..default()
        }
    }
}

impl ResetIndirectBatchSetsPipeline {
    fn prepare(
        &mut self,
        pipeline_cache: &PipelineCache,
        pipelines: &mut SpecializedComputePipelines<ResetIndirectBatchSetsPipeline>,
    ) {
        if self.pipeline_id.is_some() {
            return;
        }

        let reset_indirect_batch_sets_pipeline_id = pipelines.specialize(pipeline_cache, self, ());
        self.pipeline_id = Some(reset_indirect_batch_sets_pipeline_id);
    }
}

impl BuildIndirectParametersPipeline {
    fn prepare(
        &mut self,
        pipeline_cache: &PipelineCache,
        pipelines: &mut SpecializedComputePipelines<BuildIndirectParametersPipeline>,
        key: BuildIndirectParametersPipelineKey,
    ) {
        if self.pipeline_id.is_some() {
            return;
        }

        let build_indirect_parameters_pipeline_id = pipelines.specialize(pipeline_cache, self, key);
        self.pipeline_id = Some(build_indirect_parameters_pipeline_id);
    }
}

impl BinUnpackingPipeline {

    fn prepare(
        &mut self,
        pipeline_cache: &PipelineCache,
        pipelines: &mut SpecializedComputePipelines<BinUnpackingPipeline>,
    ) {
        if self.pipeline_id.is_some() {
            return;
        }

        let bin_unpacking_pipeline_id = pipelines.specialize(pipeline_cache, self, ());
        self.pipeline_id = Some(bin_unpacking_pipeline_id);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "it's a system that needs a lot of arguments"
)]
pub fn prepare_preprocess_bind_groups(
    mut commands: Commands,
    views: Query<(Entity, &ExtractedView)>,
    view_depth_pyramids: Query<(&ViewDepthPyramid, &PreviousViewUniformOffset)>,
    render_device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    batched_instance_buffers: Res<BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>,
    indirect_parameters_buffers: Res<IndirectParametersBuffers>,
    bin_unpacking_buffers: Res<BinUnpackingBuffers>,
    mesh_culling_data_buffer: Res<MeshCullingDataBuffer>,
    visibility_ranges: Res<RenderVisibilityRanges>,
    view_uniforms: Res<ViewUniforms>,
    previous_view_uniforms: Res<PreviousViewUniforms>,
    pipelines: Res<PreprocessPipelines>,
    mut bin_unpacking_bind_groups: ResMut<BinUnpackingBindGroups>,
) {

    let BatchedInstanceBuffers {
        current_input_buffer: current_input_buffer_vec,
        previous_input_buffer: previous_input_buffer_vec,
        phase_instance_buffers,
    } = batched_instance_buffers.into_inner();

    let (Some(current_input_buffer), Some(previous_input_buffer)) = (
        current_input_buffer_vec.buffer().buffer(),
        previous_input_buffer_vec.buffer(),
    ) else {
        return;
    };

    let mut any_indirect = false;

    for (view_entity, view) in &views {
        let mut bind_groups = TypeIdMap::default();

        for (phase_type_id, phase_instance_buffers) in phase_instance_buffers {
            let UntypedPhaseBatchedInstanceBuffers {
                data_buffer: ref data_buffer_vec,
                ref work_item_buffers,
                ref late_indexed_indirect_parameters_buffer,
                ref late_non_indexed_indirect_parameters_buffer,
            } = *phase_instance_buffers;

            let Some(data_buffer) = data_buffer_vec.buffer() else {
                continue;
            };

            let Some(phase_indirect_parameters_buffers) =
                indirect_parameters_buffers.get(phase_type_id)
            else {
                continue;
            };

            let Some(work_item_buffers) = work_item_buffers.get(&view.retained_view_entity) else {
                continue;
            };

            let preprocess_bind_group_builder = PreprocessBindGroupBuilder {
                view: view_entity,
                late_indexed_indirect_parameters_buffer,
                late_non_indexed_indirect_parameters_buffer,
                render_device: &render_device,
                pipeline_cache: &pipeline_cache,
                phase_indirect_parameters_buffers,
                mesh_culling_data_buffer: &mesh_culling_data_buffer,
                visibility_range_data_buffer: visibility_ranges.buffer(),
                view_uniforms: &view_uniforms,
                previous_view_uniforms: &previous_view_uniforms,
                pipelines: &pipelines,
                current_input_buffer,
                previous_input_buffer,
                data_buffer,
            };

            let (was_indirect, bind_group) = match *work_item_buffers {
                PreprocessWorkItemBuffers::Direct(ref work_item_buffer) => (
                    false,
                    preprocess_bind_group_builder
                        .create_direct_preprocess_bind_groups(work_item_buffer),
                ),

                PreprocessWorkItemBuffers::Indirect {
                    indexed: ref indexed_work_item_buffer,
                    non_indexed: ref non_indexed_work_item_buffer,
                    gpu_occlusion_culling: Some(ref gpu_occlusion_culling_work_item_buffers),
                } => (
                    true,
                    preprocess_bind_group_builder
                        .create_indirect_occlusion_culling_preprocess_bind_groups(
                            &view_depth_pyramids,
                            indexed_work_item_buffer,
                            non_indexed_work_item_buffer,
                            gpu_occlusion_culling_work_item_buffers,
                        ),
                ),

                PreprocessWorkItemBuffers::Indirect {
                    indexed: ref indexed_work_item_buffer,
                    non_indexed: ref non_indexed_work_item_buffer,
                    gpu_occlusion_culling: None,
                } => (
                    true,
                    preprocess_bind_group_builder
                        .create_indirect_frustum_culling_preprocess_bind_groups(
                            indexed_work_item_buffer,
                            non_indexed_work_item_buffer,
                        ),
                ),
            };

            if let Some(bind_group) = bind_group {
                any_indirect = any_indirect || was_indirect;
                bind_groups.insert(*phase_type_id, bind_group);
            }
        }

        commands
            .entity(view_entity)
            .insert(PreprocessBindGroups(bind_groups));
    }

    if any_indirect {
        create_build_indirect_parameters_bind_groups(
            &mut commands,
            &render_device,
            &pipeline_cache,
            &pipelines,
            current_input_buffer,
            &indirect_parameters_buffers,
        );
    }

    for (_, view) in &views {
        create_bin_unpacking_bind_groups(
            &mut bin_unpacking_bind_groups,
            &render_device,
            &pipeline_cache,
            &pipelines,
            &indirect_parameters_buffers,
            phase_instance_buffers,
            &bin_unpacking_buffers,
            &view.retained_view_entity,
        );
    }
}

struct PreprocessBindGroupBuilder<'a> {

    view: Entity,

    late_indexed_indirect_parameters_buffer:
        &'a RawBufferVec<LatePreprocessWorkItemIndirectParameters>,

    late_non_indexed_indirect_parameters_buffer:
        &'a RawBufferVec<LatePreprocessWorkItemIndirectParameters>,

    render_device: &'a RenderDevice,

    pipeline_cache: &'a PipelineCache,

    phase_indirect_parameters_buffers: &'a UntypedPhaseIndirectParametersBuffers,

    mesh_culling_data_buffer: &'a MeshCullingDataBuffer,

    visibility_range_data_buffer: &'a BufferVec<Vec4>,

    view_uniforms: &'a ViewUniforms,

    previous_view_uniforms: &'a PreviousViewUniforms,

    pipelines: &'a PreprocessPipelines,

    current_input_buffer: &'a Buffer,

    previous_input_buffer: &'a Buffer,

    data_buffer: &'a Buffer,
}

impl<'a> PreprocessBindGroupBuilder<'a> {

    fn create_direct_preprocess_bind_groups(
        &self,
        work_item_buffer: &RawBufferVec<PreprocessWorkItem>,
    ) -> Option<PhasePreprocessBindGroups> {

        let work_item_buffer_size = NonZero::<u64>::try_from(
            work_item_buffer.len() as u64 * u64::from(PreprocessWorkItem::min_size()),
        )
        .ok();

        Some(PhasePreprocessBindGroups::Direct(
            self.render_device.create_bind_group(
                "preprocess_direct_bind_group",
                &self
                    .pipeline_cache
                    .get_bind_group_layout(&self.pipelines.direct_preprocess.bind_group_layout),
                &BindGroupEntries::with_indices((
                    (0, self.view_uniforms.uniforms.binding()?),
                    (3, self.current_input_buffer.as_entire_binding()),
                    (4, self.previous_input_buffer.as_entire_binding()),
                    (
                        5,
                        BindingResource::Buffer(BufferBinding {
                            buffer: work_item_buffer.buffer()?,
                            offset: 0,
                            size: work_item_buffer_size,
                        }),
                    ),
                    (6, self.data_buffer.as_entire_binding()),
                )),
            ),
        ))
    }

    fn create_indirect_occlusion_culling_preprocess_bind_groups(
        &self,
        view_depth_pyramids: &Query<(&ViewDepthPyramid, &PreviousViewUniformOffset)>,
        indexed_work_item_buffer: &PartialBufferVec<PreprocessWorkItem>,
        non_indexed_work_item_buffer: &PartialBufferVec<PreprocessWorkItem>,
        gpu_occlusion_culling_work_item_buffers: &GpuOcclusionCullingWorkItemBuffers,
    ) -> Option<PhasePreprocessBindGroups> {
        let GpuOcclusionCullingWorkItemBuffers {
            late_indexed: ref late_indexed_work_item_buffer,
            late_non_indexed: ref late_non_indexed_work_item_buffer,
            ..
        } = *gpu_occlusion_culling_work_item_buffers;

        let (view_depth_pyramid, previous_view_uniform_offset) =
            view_depth_pyramids.get(self.view).ok()?;

        Some(PhasePreprocessBindGroups::IndirectOcclusionCulling {
            early_indexed: self.create_indirect_occlusion_culling_early_indexed_bind_group(
                view_depth_pyramid,
                previous_view_uniform_offset,
                indexed_work_item_buffer,
                late_indexed_work_item_buffer,
            ),

            early_non_indexed: self.create_indirect_occlusion_culling_early_non_indexed_bind_group(
                view_depth_pyramid,
                previous_view_uniform_offset,
                non_indexed_work_item_buffer,
                late_non_indexed_work_item_buffer,
            ),

            late_indexed: self.create_indirect_occlusion_culling_late_indexed_bind_group(
                view_depth_pyramid,
                previous_view_uniform_offset,
                late_indexed_work_item_buffer,
            ),

            late_non_indexed: self.create_indirect_occlusion_culling_late_non_indexed_bind_group(
                view_depth_pyramid,
                previous_view_uniform_offset,
                late_non_indexed_work_item_buffer,
            ),
        })
    }

    fn create_indirect_occlusion_culling_early_indexed_bind_group(
        &self,
        view_depth_pyramid: &ViewDepthPyramid,
        previous_view_uniform_offset: &PreviousViewUniformOffset,
        indexed_work_item_buffer: &PartialBufferVec<PreprocessWorkItem>,
        late_indexed_work_item_buffer: &UninitBufferVec<PreprocessWorkItem>,
    ) -> Option<BindGroup> {
        let mesh_culling_data_buffer = self.mesh_culling_data_buffer.buffer()?;
        let visibility_range_binding = self.visibility_range_data_buffer.binding()?;
        let view_uniforms_binding = self.view_uniforms.uniforms.binding()?;
        let previous_view_buffer = self.previous_view_uniforms.uniforms.buffer()?;

        match (
            self.phase_indirect_parameters_buffers
                .indexed
                .cpu_metadata_buffer(),
            self.phase_indirect_parameters_buffers
                .indexed
                .gpu_metadata_buffer(),
            indexed_work_item_buffer.buffer(),
            late_indexed_work_item_buffer.buffer(),
            self.late_indexed_indirect_parameters_buffer.buffer(),
        ) {
            (
                Some(indexed_cpu_metadata_buffer),
                Some(indexed_gpu_metadata_buffer),
                Some(indexed_work_item_gpu_buffer),
                Some(late_indexed_work_item_gpu_buffer),
                Some(late_indexed_indirect_parameters_buffer),
            ) => {

                let indexed_work_item_buffer_size = NonZero::<u64>::try_from(
                    indexed_work_item_buffer.len() as u64
                        * u64::from(PreprocessWorkItem::min_size()),
                )
                .ok();

                Some(
                    self.render_device.create_bind_group(
                        "preprocess_early_indexed_gpu_occlusion_culling_bind_group",
                        &self.pipeline_cache.get_bind_group_layout(
                            &self
                                .pipelines
                                .early_gpu_occlusion_culling_preprocess
                                .bind_group_layout,
                        ),
                        &BindGroupEntries::with_indices((
                            (3, self.current_input_buffer.as_entire_binding()),
                            (4, self.previous_input_buffer.as_entire_binding()),
                            (
                                5,
                                BindingResource::Buffer(BufferBinding {
                                    buffer: indexed_work_item_gpu_buffer,
                                    offset: 0,
                                    size: indexed_work_item_buffer_size,
                                }),
                            ),
                            (6, self.data_buffer.as_entire_binding()),
                            (7, indexed_cpu_metadata_buffer.as_entire_binding()),
                            (8, indexed_gpu_metadata_buffer.as_entire_binding()),
                            (9, mesh_culling_data_buffer.as_entire_binding()),
                            (10, visibility_range_binding.clone()),
                            (0, view_uniforms_binding.clone()),
                            (11, &view_depth_pyramid.all_mips),
                            (
                                2,
                                BufferBinding {
                                    buffer: previous_view_buffer,
                                    offset: previous_view_uniform_offset.offset as u64,
                                    size: NonZeroU64::new(size_of::<PreviousViewData>() as u64),
                                },
                            ),
                            (
                                12,
                                BufferBinding {
                                    buffer: late_indexed_work_item_gpu_buffer,
                                    offset: 0,
                                    size: indexed_work_item_buffer_size,
                                },
                            ),
                            (
                                13,
                                BufferBinding {
                                    buffer: late_indexed_indirect_parameters_buffer,
                                    offset: 0,
                                    size: NonZeroU64::new(
                                        late_indexed_indirect_parameters_buffer.size(),
                                    ),
                                },
                            ),
                        )),
                    ),
                )
            }
            _ => None,
        }
    }

    fn create_indirect_occlusion_culling_early_non_indexed_bind_group(
        &self,
        view_depth_pyramid: &ViewDepthPyramid,
        previous_view_uniform_offset: &PreviousViewUniformOffset,
        non_indexed_work_item_buffer: &PartialBufferVec<PreprocessWorkItem>,
        late_non_indexed_work_item_buffer: &UninitBufferVec<PreprocessWorkItem>,
    ) -> Option<BindGroup> {
        let mesh_culling_data_buffer = self.mesh_culling_data_buffer.buffer()?;
        let visibility_range_binding = self.visibility_range_data_buffer.binding()?;
        let view_uniforms_binding = self.view_uniforms.uniforms.binding()?;
        let previous_view_buffer = self.previous_view_uniforms.uniforms.buffer()?;

        match (
            self.phase_indirect_parameters_buffers
                .non_indexed
                .cpu_metadata_buffer(),
            self.phase_indirect_parameters_buffers
                .non_indexed
                .gpu_metadata_buffer(),
            non_indexed_work_item_buffer.buffer(),
            late_non_indexed_work_item_buffer.buffer(),
            self.late_non_indexed_indirect_parameters_buffer.buffer(),
        ) {
            (
                Some(non_indexed_cpu_metadata_buffer),
                Some(non_indexed_gpu_metadata_buffer),
                Some(non_indexed_work_item_gpu_buffer),
                Some(late_non_indexed_work_item_buffer),
                Some(late_non_indexed_indirect_parameters_buffer),
            ) => {

                let non_indexed_work_item_buffer_size = NonZero::<u64>::try_from(
                    non_indexed_work_item_buffer.len() as u64
                        * u64::from(PreprocessWorkItem::min_size()),
                )
                .ok();

                Some(
                    self.render_device.create_bind_group(
                        "preprocess_early_non_indexed_gpu_occlusion_culling_bind_group",
                        &self.pipeline_cache.get_bind_group_layout(
                            &self
                                .pipelines
                                .early_gpu_occlusion_culling_preprocess
                                .bind_group_layout,
                        ),
                        &BindGroupEntries::with_indices((
                            (3, self.current_input_buffer.as_entire_binding()),
                            (4, self.previous_input_buffer.as_entire_binding()),
                            (
                                5,
                                BindingResource::Buffer(BufferBinding {
                                    buffer: non_indexed_work_item_gpu_buffer,
                                    offset: 0,
                                    size: non_indexed_work_item_buffer_size,
                                }),
                            ),
                            (6, self.data_buffer.as_entire_binding()),
                            (7, non_indexed_cpu_metadata_buffer.as_entire_binding()),
                            (8, non_indexed_gpu_metadata_buffer.as_entire_binding()),
                            (9, mesh_culling_data_buffer.as_entire_binding()),
                            (10, visibility_range_binding.clone()),
                            (0, view_uniforms_binding.clone()),
                            (11, &view_depth_pyramid.all_mips),
                            (
                                2,
                                BufferBinding {
                                    buffer: previous_view_buffer,
                                    offset: previous_view_uniform_offset.offset as u64,
                                    size: NonZeroU64::new(size_of::<PreviousViewData>() as u64),
                                },
                            ),
                            (
                                12,
                                BufferBinding {
                                    buffer: late_non_indexed_work_item_buffer,
                                    offset: 0,
                                    size: non_indexed_work_item_buffer_size,
                                },
                            ),
                            (
                                13,
                                BufferBinding {
                                    buffer: late_non_indexed_indirect_parameters_buffer,
                                    offset: 0,
                                    size: NonZeroU64::new(
                                        late_non_indexed_indirect_parameters_buffer.size(),
                                    ),
                                },
                            ),
                        )),
                    ),
                )
            }
            _ => None,
        }
    }

    fn create_indirect_occlusion_culling_late_indexed_bind_group(
        &self,
        view_depth_pyramid: &ViewDepthPyramid,
        previous_view_uniform_offset: &PreviousViewUniformOffset,
        late_indexed_work_item_buffer: &UninitBufferVec<PreprocessWorkItem>,
    ) -> Option<BindGroup> {
        let mesh_culling_data_buffer = self.mesh_culling_data_buffer.buffer()?;
        let visibility_range_binding = self.visibility_range_data_buffer.binding()?;
        let view_uniforms_binding = self.view_uniforms.uniforms.binding()?;
        let previous_view_buffer = self.previous_view_uniforms.uniforms.buffer()?;

        match (
            self.phase_indirect_parameters_buffers
                .indexed
                .cpu_metadata_buffer(),
            self.phase_indirect_parameters_buffers
                .indexed
                .gpu_metadata_buffer(),
            late_indexed_work_item_buffer.buffer(),
            self.late_indexed_indirect_parameters_buffer.buffer(),
        ) {
            (
                Some(indexed_cpu_metadata_buffer),
                Some(indexed_gpu_metadata_buffer),
                Some(late_indexed_work_item_gpu_buffer),
                Some(late_indexed_indirect_parameters_buffer),
            ) => {

                let late_indexed_work_item_buffer_size = NonZero::<u64>::try_from(
                    late_indexed_work_item_buffer.len() as u64
                        * u64::from(PreprocessWorkItem::min_size()),
                )
                .ok();

                Some(
                    self.render_device.create_bind_group(
                        "preprocess_late_indexed_gpu_occlusion_culling_bind_group",
                        &self.pipeline_cache.get_bind_group_layout(
                            &self
                                .pipelines
                                .late_gpu_occlusion_culling_preprocess
                                .bind_group_layout,
                        ),
                        &BindGroupEntries::with_indices((
                            (3, self.current_input_buffer.as_entire_binding()),
                            (4, self.previous_input_buffer.as_entire_binding()),
                            (
                                5,
                                BindingResource::Buffer(BufferBinding {
                                    buffer: late_indexed_work_item_gpu_buffer,
                                    offset: 0,
                                    size: late_indexed_work_item_buffer_size,
                                }),
                            ),
                            (6, self.data_buffer.as_entire_binding()),
                            (7, indexed_cpu_metadata_buffer.as_entire_binding()),
                            (8, indexed_gpu_metadata_buffer.as_entire_binding()),
                            (9, mesh_culling_data_buffer.as_entire_binding()),
                            (10, visibility_range_binding.clone()),
                            (0, view_uniforms_binding.clone()),
                            (11, &view_depth_pyramid.all_mips),
                            (
                                2,
                                BufferBinding {
                                    buffer: previous_view_buffer,
                                    offset: previous_view_uniform_offset.offset as u64,
                                    size: NonZeroU64::new(size_of::<PreviousViewData>() as u64),
                                },
                            ),
                            (
                                13,
                                BufferBinding {
                                    buffer: late_indexed_indirect_parameters_buffer,
                                    offset: 0,
                                    size: NonZeroU64::new(
                                        late_indexed_indirect_parameters_buffer.size(),
                                    ),
                                },
                            ),
                        )),
                    ),
                )
            }
            _ => None,
        }
    }

    fn create_indirect_occlusion_culling_late_non_indexed_bind_group(
        &self,
        view_depth_pyramid: &ViewDepthPyramid,
        previous_view_uniform_offset: &PreviousViewUniformOffset,
        late_non_indexed_work_item_buffer: &UninitBufferVec<PreprocessWorkItem>,
    ) -> Option<BindGroup> {
        let mesh_culling_data_buffer = self.mesh_culling_data_buffer.buffer()?;
        let visibility_range_binding = self.visibility_range_data_buffer.binding()?;
        let view_uniforms_binding = self.view_uniforms.uniforms.binding()?;
        let previous_view_buffer = self.previous_view_uniforms.uniforms.buffer()?;

        match (
            self.phase_indirect_parameters_buffers
                .non_indexed
                .cpu_metadata_buffer(),
            self.phase_indirect_parameters_buffers
                .non_indexed
                .gpu_metadata_buffer(),
            late_non_indexed_work_item_buffer.buffer(),
            self.late_non_indexed_indirect_parameters_buffer.buffer(),
        ) {
            (
                Some(non_indexed_cpu_metadata_buffer),
                Some(non_indexed_gpu_metadata_buffer),
                Some(non_indexed_work_item_gpu_buffer),
                Some(late_non_indexed_indirect_parameters_buffer),
            ) => {

                let non_indexed_work_item_buffer_size = NonZero::<u64>::try_from(
                    late_non_indexed_work_item_buffer.len() as u64
                        * u64::from(PreprocessWorkItem::min_size()),
                )
                .ok();

                Some(
                    self.render_device.create_bind_group(
                        "preprocess_late_non_indexed_gpu_occlusion_culling_bind_group",
                        &self.pipeline_cache.get_bind_group_layout(
                            &self
                                .pipelines
                                .late_gpu_occlusion_culling_preprocess
                                .bind_group_layout,
                        ),
                        &BindGroupEntries::with_indices((
                            (3, self.current_input_buffer.as_entire_binding()),
                            (4, self.previous_input_buffer.as_entire_binding()),
                            (
                                5,
                                BindingResource::Buffer(BufferBinding {
                                    buffer: non_indexed_work_item_gpu_buffer,
                                    offset: 0,
                                    size: non_indexed_work_item_buffer_size,
                                }),
                            ),
                            (6, self.data_buffer.as_entire_binding()),
                            (7, non_indexed_cpu_metadata_buffer.as_entire_binding()),
                            (8, non_indexed_gpu_metadata_buffer.as_entire_binding()),
                            (9, mesh_culling_data_buffer.as_entire_binding()),
                            (10, visibility_range_binding.clone()),
                            (0, view_uniforms_binding.clone()),
                            (11, &view_depth_pyramid.all_mips),
                            (
                                2,
                                BufferBinding {
                                    buffer: previous_view_buffer,
                                    offset: previous_view_uniform_offset.offset as u64,
                                    size: NonZeroU64::new(size_of::<PreviousViewData>() as u64),
                                },
                            ),
                            (
                                13,
                                BufferBinding {
                                    buffer: late_non_indexed_indirect_parameters_buffer,
                                    offset: 0,
                                    size: NonZeroU64::new(
                                        late_non_indexed_indirect_parameters_buffer.size(),
                                    ),
                                },
                            ),
                        )),
                    ),
                )
            }
            _ => None,
        }
    }

    fn create_indirect_frustum_culling_preprocess_bind_groups(
        &self,
        indexed_work_item_buffer: &PartialBufferVec<PreprocessWorkItem>,
        non_indexed_work_item_buffer: &PartialBufferVec<PreprocessWorkItem>,
    ) -> Option<PhasePreprocessBindGroups> {
        Some(PhasePreprocessBindGroups::IndirectFrustumCulling {
            indexed: self
                .create_indirect_frustum_culling_indexed_bind_group(indexed_work_item_buffer),
            non_indexed: self.create_indirect_frustum_culling_non_indexed_bind_group(
                non_indexed_work_item_buffer,
            ),
        })
    }

    fn create_indirect_frustum_culling_indexed_bind_group(
        &self,
        indexed_work_item_buffer: &PartialBufferVec<PreprocessWorkItem>,
    ) -> Option<BindGroup> {
        let mesh_culling_data_buffer = self.mesh_culling_data_buffer.buffer()?;
        let visibility_range_binding = self.visibility_range_data_buffer.binding()?;
        let view_uniforms_binding = self.view_uniforms.uniforms.binding()?;

        match (
            self.phase_indirect_parameters_buffers
                .indexed
                .cpu_metadata_buffer(),
            self.phase_indirect_parameters_buffers
                .indexed
                .gpu_metadata_buffer(),
            indexed_work_item_buffer.buffer(),
        ) {
            (
                Some(indexed_cpu_metadata_buffer),
                Some(indexed_gpu_metadata_buffer),
                Some(indexed_work_item_gpu_buffer),
            ) => {

                let indexed_work_item_buffer_size = NonZero::<u64>::try_from(
                    indexed_work_item_buffer.len() as u64
                        * u64::from(PreprocessWorkItem::min_size()),
                )
                .ok();

                Some(
                    self.render_device.create_bind_group(
                        "preprocess_gpu_indexed_frustum_culling_bind_group",
                        &self.pipeline_cache.get_bind_group_layout(
                            &self
                                .pipelines
                                .gpu_frustum_culling_preprocess
                                .bind_group_layout,
                        ),
                        &BindGroupEntries::with_indices((
                            (3, self.current_input_buffer.as_entire_binding()),
                            (4, self.previous_input_buffer.as_entire_binding()),
                            (
                                5,
                                BindingResource::Buffer(BufferBinding {
                                    buffer: indexed_work_item_gpu_buffer,
                                    offset: 0,
                                    size: indexed_work_item_buffer_size,
                                }),
                            ),
                            (6, self.data_buffer.as_entire_binding()),
                            (7, indexed_cpu_metadata_buffer.as_entire_binding()),
                            (8, indexed_gpu_metadata_buffer.as_entire_binding()),
                            (9, mesh_culling_data_buffer.as_entire_binding()),
                            (10, visibility_range_binding.clone()),
                            (0, view_uniforms_binding.clone()),
                        )),
                    ),
                )
            }
            _ => None,
        }
    }

    fn create_indirect_frustum_culling_non_indexed_bind_group(
        &self,
        non_indexed_work_item_buffer: &PartialBufferVec<PreprocessWorkItem>,
    ) -> Option<BindGroup> {
        let mesh_culling_data_buffer = self.mesh_culling_data_buffer.buffer()?;
        let visibility_range_binding = self.visibility_range_data_buffer.binding()?;
        let view_uniforms_binding = self.view_uniforms.uniforms.binding()?;

        match (
            self.phase_indirect_parameters_buffers
                .non_indexed
                .cpu_metadata_buffer(),
            self.phase_indirect_parameters_buffers
                .non_indexed
                .gpu_metadata_buffer(),
            non_indexed_work_item_buffer.buffer(),
        ) {
            (
                Some(non_indexed_cpu_metadata_buffer),
                Some(non_indexed_gpu_metadata_buffer),
                Some(non_indexed_work_item_gpu_buffer),
            ) => {

                let non_indexed_work_item_buffer_size = NonZero::<u64>::try_from(
                    non_indexed_work_item_buffer.len() as u64
                        * u64::from(PreprocessWorkItem::min_size()),
                )
                .ok();

                Some(
                    self.render_device.create_bind_group(
                        "preprocess_gpu_non_indexed_frustum_culling_bind_group",
                        &self.pipeline_cache.get_bind_group_layout(
                            &self
                                .pipelines
                                .gpu_frustum_culling_preprocess
                                .bind_group_layout,
                        ),
                        &BindGroupEntries::with_indices((
                            (3, self.current_input_buffer.as_entire_binding()),
                            (4, self.previous_input_buffer.as_entire_binding()),
                            (
                                5,
                                BindingResource::Buffer(BufferBinding {
                                    buffer: non_indexed_work_item_gpu_buffer,
                                    offset: 0,
                                    size: non_indexed_work_item_buffer_size,
                                }),
                            ),
                            (6, self.data_buffer.as_entire_binding()),
                            (7, non_indexed_cpu_metadata_buffer.as_entire_binding()),
                            (8, non_indexed_gpu_metadata_buffer.as_entire_binding()),
                            (9, mesh_culling_data_buffer.as_entire_binding()),
                            (10, visibility_range_binding.clone()),
                            (0, view_uniforms_binding.clone()),
                        )),
                    ),
                )
            }
            _ => None,
        }
    }
}

fn create_build_indirect_parameters_bind_groups(
    commands: &mut Commands,
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    pipelines: &PreprocessPipelines,
    current_input_buffer: &Buffer,
    indirect_parameters_buffers: &IndirectParametersBuffers,
) {
    let mut build_indirect_parameters_bind_groups = BuildIndirectParametersBindGroups::new();

    for (phase_type_id, phase_indirect_parameters_buffer) in indirect_parameters_buffers.iter() {
        build_indirect_parameters_bind_groups.insert(
            *phase_type_id,
            PhaseBuildIndirectParametersBindGroups {
                reset_indexed_indirect_batch_sets: phase_indirect_parameters_buffer
                    .indexed
                    .batch_sets_buffer()
                    .map(|indexed_batch_sets_buffer| {
                        render_device.create_bind_group(
                            "reset_indexed_indirect_batch_sets_bind_group",

                            &pipeline_cache.get_bind_group_layout(
                                &pipelines
                                    .early_phase
                                    .reset_indirect_batch_sets
                                    .bind_group_layout,
                            ),
                            &BindGroupEntries::sequential((
                                indexed_batch_sets_buffer.as_entire_binding(),
                            )),
                        )
                    }),

                reset_non_indexed_indirect_batch_sets: phase_indirect_parameters_buffer
                    .non_indexed
                    .batch_sets_buffer()
                    .map(|non_indexed_batch_sets_buffer| {
                        render_device.create_bind_group(
                            "reset_non_indexed_indirect_batch_sets_bind_group",

                            &pipeline_cache.get_bind_group_layout(
                                &pipelines
                                    .early_phase
                                    .reset_indirect_batch_sets
                                    .bind_group_layout,
                            ),
                            &BindGroupEntries::sequential((
                                non_indexed_batch_sets_buffer.as_entire_binding(),
                            )),
                        )
                    }),

                build_indexed_indirect: match (
                    phase_indirect_parameters_buffer
                        .indexed
                        .cpu_metadata_buffer(),
                    phase_indirect_parameters_buffer
                        .indexed
                        .gpu_metadata_buffer(),
                    phase_indirect_parameters_buffer.indexed.data_buffer(),
                    phase_indirect_parameters_buffer.indexed.batch_sets_buffer(),
                ) {
                    (
                        Some(indexed_indirect_parameters_cpu_metadata_buffer),
                        Some(indexed_indirect_parameters_gpu_metadata_buffer),
                        Some(indexed_indirect_parameters_data_buffer),
                        Some(indexed_batch_sets_buffer),
                    ) => Some(
                        render_device.create_bind_group(
                            "build_indexed_indirect_parameters_bind_group",

                            &pipeline_cache.get_bind_group_layout(
                                &pipelines
                                    .gpu_frustum_culling_build_indexed_indirect_params
                                    .bind_group_layout,
                            ),
                            &BindGroupEntries::sequential((
                                current_input_buffer.as_entire_binding(),

                                BufferBinding {
                                    buffer: indexed_indirect_parameters_cpu_metadata_buffer,
                                    offset: 0,
                                    size: NonZeroU64::new(
                                        phase_indirect_parameters_buffer.indexed.batch_count()
                                            as u64
                                            * size_of::<IndirectParametersCpuMetadata>() as u64,
                                    ),
                                },
                                BufferBinding {
                                    buffer: indexed_indirect_parameters_gpu_metadata_buffer,
                                    offset: 0,
                                    size: NonZeroU64::new(
                                        phase_indirect_parameters_buffer.indexed.batch_count()
                                            as u64
                                            * size_of::<IndirectParametersGpuMetadata>() as u64,
                                    ),
                                },
                                indexed_batch_sets_buffer.as_entire_binding(),
                                indexed_indirect_parameters_data_buffer.as_entire_binding(),
                            )),
                        ),
                    ),
                    _ => None,
                },

                build_non_indexed_indirect: match (
                    phase_indirect_parameters_buffer
                        .non_indexed
                        .cpu_metadata_buffer(),
                    phase_indirect_parameters_buffer
                        .non_indexed
                        .gpu_metadata_buffer(),
                    phase_indirect_parameters_buffer.non_indexed.data_buffer(),
                    phase_indirect_parameters_buffer
                        .non_indexed
                        .batch_sets_buffer(),
                ) {
                    (
                        Some(non_indexed_indirect_parameters_cpu_metadata_buffer),
                        Some(non_indexed_indirect_parameters_gpu_metadata_buffer),
                        Some(non_indexed_indirect_parameters_data_buffer),
                        Some(non_indexed_batch_sets_buffer),
                    ) => Some(
                        render_device.create_bind_group(
                            "build_non_indexed_indirect_parameters_bind_group",

                            &pipeline_cache.get_bind_group_layout(
                                &pipelines
                                    .gpu_frustum_culling_build_non_indexed_indirect_params
                                    .bind_group_layout,
                            ),
                            &BindGroupEntries::sequential((
                                current_input_buffer.as_entire_binding(),

                                BufferBinding {
                                    buffer: non_indexed_indirect_parameters_cpu_metadata_buffer,
                                    offset: 0,
                                    size: NonZeroU64::new(
                                        phase_indirect_parameters_buffer.non_indexed.batch_count()
                                            as u64
                                            * size_of::<IndirectParametersCpuMetadata>() as u64,
                                    ),
                                },
                                BufferBinding {
                                    buffer: non_indexed_indirect_parameters_gpu_metadata_buffer,
                                    offset: 0,
                                    size: NonZeroU64::new(
                                        phase_indirect_parameters_buffer.non_indexed.batch_count()
                                            as u64
                                            * size_of::<IndirectParametersGpuMetadata>() as u64,
                                    ),
                                },
                                non_indexed_batch_sets_buffer.as_entire_binding(),
                                non_indexed_indirect_parameters_data_buffer.as_entire_binding(),
                            )),
                        ),
                    ),
                    _ => None,
                },
            },
        );
    }

    commands.insert_resource(build_indirect_parameters_bind_groups);
}

fn create_bin_unpacking_bind_groups(
    bin_unpacking_bind_groups: &mut BinUnpackingBindGroups,
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    preprocess_pipelines: &PreprocessPipelines,
    indirect_parameters_buffers: &IndirectParametersBuffers,
    phase_instance_buffers: &TypeIdMap<UntypedPhaseBatchedInstanceBuffers<MeshUniform>>,
    bin_unpacking_buffers: &BinUnpackingBuffers,
    view_entity: &RetainedViewEntity,
) {
    let Some(bin_unpacking_metadata_buffer) = bin_unpacking_buffers.bin_unpacking_metadata.buffer()
    else {
        return;
    };

    for phase_type_id in indirect_parameters_buffers.keys() {
        let bin_unpacking_buffers_key = BinUnpackingBuffersKey {
            phase: *phase_type_id,
            view: *view_entity,
        };

        let Some(phase_batched_instance_buffers) = phase_instance_buffers.get(phase_type_id) else {
            continue;
        };
        let Some(work_item_buffers) = phase_batched_instance_buffers
            .work_item_buffers
            .get(view_entity)
        else {
            continue;
        };
        let Some(view_phase_bin_unpacking_buffers) = bin_unpacking_buffers
            .view_phase_buffers
            .get(&bin_unpacking_buffers_key)
        else {
            continue;
        };

        let maybe_indexed_work_item_buffer = match *work_item_buffers {
            PreprocessWorkItemBuffers::Direct(ref raw_buffer_vec) => raw_buffer_vec.buffer(),
            PreprocessWorkItemBuffers::Indirect { ref indexed, .. } => indexed.buffer(),
        };
        let maybe_non_indexed_work_item_buffer = match *work_item_buffers {
            PreprocessWorkItemBuffers::Direct(ref raw_buffer_vec) => raw_buffer_vec.buffer(),
            PreprocessWorkItemBuffers::Indirect {
                ref non_indexed, ..
            } => non_indexed.buffer(),
        };

        bin_unpacking_bind_groups.insert(
            bin_unpacking_buffers_key,
            ViewPhaseBinUnpackingBindGroups {
                indexed: match maybe_indexed_work_item_buffer {
                    Some(indexed_work_item_buffer) => view_phase_bin_unpacking_buffers
                        .indexed_unpacking_jobs
                        .iter()
                        .map(|job| {
                            create_bin_unpacking_bind_group(
                                render_device,
                                preprocess_pipelines,
                                pipeline_cache,
                                job,
                                bin_unpacking_metadata_buffer,
                                indexed_work_item_buffer,
                                true,
                            )
                        })
                        .collect(),
                    None => vec![],
                },
                non_indexed: match maybe_non_indexed_work_item_buffer {
                    Some(non_indexed_work_item_buffer) => view_phase_bin_unpacking_buffers
                        .non_indexed_unpacking_jobs
                        .iter()
                        .map(|job| {
                            create_bin_unpacking_bind_group(
                                render_device,
                                preprocess_pipelines,
                                pipeline_cache,
                                job,
                                bin_unpacking_metadata_buffer,
                                non_indexed_work_item_buffer,
                                false,
                            )
                        })
                        .collect(),
                    None => vec![],
                },
            },
        );
    }
}

fn create_bin_unpacking_bind_group(
    render_device: &RenderDevice,
    preprocess_pipelines: &PreprocessPipelines,
    pipeline_cache: &PipelineCache,
    job: &BinUnpackingJob,
    bin_unpacking_metadata_buffer: &Buffer,
    work_item_buffer: &Buffer,
    indexed: bool,
) -> ViewPhaseBinUnpackingBindGroup {
    let bind_group = render_device.create_bind_group(
        if indexed {
            "bin unpacking indexed bind group"
        } else {
            "bin unpacking non-indexed bind group"
        },
        &pipeline_cache
            .get_bind_group_layout(&preprocess_pipelines.bin_unpacking.bind_group_layout),
        &BindGroupEntries::sequential((

            BindingResource::Buffer(BufferBinding {
                buffer: bin_unpacking_metadata_buffer,
                offset: job.bin_unpacking_metadata_index.uniform_offset() as u64,
                size: NonZeroU64::new(size_of::<GpuBinUnpackingMetadata>() as u64),
            }),

            job.render_binned_mesh_instance_buffer.as_entire_binding(),

            work_item_buffer.as_entire_binding(),

            job.bin_index_to_indirect_parameters_offset_buffer
                .as_entire_binding(),
        )),
    );
    ViewPhaseBinUnpackingBindGroup {
        metadata_index: job.bin_unpacking_metadata_index,
        bind_group,
        mesh_instance_count: job.mesh_instance_count,
    }
}

pub fn write_mesh_culling_data_buffer(
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut mesh_culling_data_buffer: ResMut<MeshCullingDataBuffer>,
    pipeline_cache: Res<PipelineCache>,
    mut sparse_buffer_update_jobs: ResMut<SparseBufferUpdateJobs>,
    mut sparse_buffer_update_bind_groups: ResMut<SparseBufferUpdateBindGroups>,
    sparse_buffer_update_pipelines: Res<SparseBufferUpdatePipelines>,
) {
    mesh_culling_data_buffer.write_buffers(&render_device, &render_queue);
    mesh_culling_data_buffer.prepare_to_populate_buffers(
        &render_device,
        &pipeline_cache,
        &mut sparse_buffer_update_jobs,
        &mut sparse_buffer_update_bind_groups,
        &sparse_buffer_update_pipelines,
    );
}
