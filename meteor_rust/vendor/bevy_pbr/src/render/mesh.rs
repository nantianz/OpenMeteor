use crate::contact_shadows::ContactShadows;
use crate::{
    material_bind_groups::MaterialBindGroupSlot, resources::write_atmosphere_buffer,
    skin::skin_uniforms_from_world,
};
use alloc::sync::Arc;
use bevy_asset::uuid::Uuid;
use bevy_asset::{embedded_asset, load_embedded_asset, AssetId, AssetIndex, AssetServer};
use bevy_camera::visibility::NoCpuCulling;
use bevy_camera::{
    primitives::Aabb,
    visibility::{NoFrustumCulling, RenderLayers, ViewVisibility, VisibilityRange},
    Camera, Projection,
};
use bevy_core_pipeline::{
    core_3d::{AlphaMask3d, Opaque3d, Transparent3d, CORE_3D_DEPTH_FORMAT},
    deferred::{AlphaMask3dDeferred, Opaque3dDeferred},
    oit::prepare_oit_buffers,
    prepass::MotionVectorPrepass,
};
use bevy_derive::{Deref, DerefMut};
use bevy_diagnostic::FrameCount;
use bevy_ecs::entity::EntityHash;
use bevy_ecs::{
    entity::EntityHashSet,
    prelude::*,
    query::{QueryData, ROQueryItem},
    relationship::RelationshipSourceCollection,
    system::{lifetimeless::*, SystemParamItem},
};
use bevy_image::{ImageSampler, TextureFormatPixelInfo};
use bevy_light::{
    EnvironmentMapLight, IrradianceVolume, NotShadowCaster, NotShadowReceiver,
    ShadowFilteringMethod, TransmittedShadowReceiver,
};
use bevy_math::{Affine3, Affine3Ext, Rect, UVec2, Vec3, Vec4};
use bevy_mesh::{
    skinning::SkinnedMesh, BaseMeshPipelineKey, Mesh, Mesh3d, MeshTag, MeshVertexBufferLayoutRef,
    VertexAttributeDescriptor,
};
use bevy_platform::collections::{hash_map::Entry, HashMap};
use bevy_render::batching::gpu_preprocessing::PreviousInstanceInputUniformBuffer;
use bevy_render::impl_atomic_pod;
use bevy_render::mesh::allocator::{MeshSlabId, MeshSlabs};
use bevy_render::mesh::morph::{
    MorphTargetImage, MorphTargetsResource, RenderMorphTargetAllocator,
};
use bevy_render::{
    batching::{
        gpu_preprocessing::{
            self, GpuPreprocessingSupport, IndirectBatchSet, IndirectParametersBuffers,
            IndirectParametersCpuMetadata, IndirectParametersIndexed, IndirectParametersNonIndexed,
            InstanceInputUniformBuffer, UntypedPhaseIndirectParametersBuffers,
        },
        no_gpu_preprocessing, GetBatchData, GetFullBatchData, NoAutomaticBatching,
    },
    mesh::{allocator::MeshAllocator, RenderMesh, RenderMeshBufferInfo},
    render_asset::RenderAssets,
    render_phase::{
        BinnedRenderPhasePlugin, InputUniformIndex, PhaseItem, PhaseItemExtraIndex, RenderCommand,
        RenderCommandResult, SortedRenderPhasePlugin, TrackedRenderPass,
    },
    render_resource::*,
    renderer::{RenderAdapter, RenderDevice, RenderQueue},
    sync_world::MainEntityHashSet,
    texture::{DefaultImageSampler, GpuImage},
    view::{self, NoIndirectDrawing, RenderVisibilityRanges, RetainedViewEntity},
    Extract,
};
use bevy_shader::{load_shader_library, Shader, ShaderDefVal, ShaderSettings};
use bevy_transform::components::GlobalTransform;
use bevy_utils::{default, Parallel, TypeIdMap};
use core::any::TypeId;
use core::iter;
use core::mem::{offset_of, size_of};
use core::sync::atomic::{AtomicU64, Ordering};
use indexmap::IndexSet;
use material_bind_groups::MaterialBindingId;
use static_assertions::const_assert_eq;
use std::sync::mpsc;
#[cfg(feature = "trace")]
use tracing::info_span;
use tracing::{error, warn};

use self::irradiance_volume::IRRADIANCE_VOLUMES_ARE_USABLE;
use crate::{
    render::{
        morph::{
            extract_morphs, no_automatic_morph_batching, write_morph_buffers, MorphIndices,
            MorphUniforms,
        },
        skin::no_automatic_skin_batching,
    },
    *,
};
use bevy_core_pipeline::oit::OrderIndependentTransparencySettings;
use bevy_core_pipeline::prepass::{DeferredPrepass, DepthPrepass, NormalPrepass};
use bevy_core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy_render::camera::{DirtySpecializations, ExtractedCamera, TemporalJitter};
use bevy_render::prelude::Msaa;
use bevy_render::sync_world::{MainEntity, MainEntityHashMap};
use bevy_render::view::{
    texture_format_from_code, texture_format_to_code, ExtractedView,
    RenderShadowMapVisibleEntities, RenderVisibleEntities,
};
use bevy_render::RenderSystems::PrepareAssets;
use bevy_tasks::ComputeTaskPool;

use bytemuck::{Pod, Zeroable};
use nonmax::{NonMaxU16, NonMaxU32};

pub struct MeshRenderPlugin {

    pub use_gpu_instance_buffer_builder: bool,

    pub debug_flags: RenderDebugFlags,
}

impl MeshRenderPlugin {

    pub fn new(debug_flags: RenderDebugFlags) -> MeshRenderPlugin {
        MeshRenderPlugin {
            use_gpu_instance_buffer_builder: false,
            debug_flags,
        }
    }
}

#[cfg(debug_assertions)]
pub const MESH_PIPELINE_VIEW_LAYOUT_SAFE_MAX_TEXTURES: usize = 10;

#[derive(Debug, Hash, PartialEq, Eq, Clone, SystemSet)]
pub struct MeshPipelineSystems;

impl Plugin for MeshRenderPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "forward_io.wgsl");
        load_shader_library!(app, "mesh_view_types.wgsl", |settings| *settings =
            ShaderSettings {
                shader_defs: vec![
                    ShaderDefVal::UInt(
                        "MAX_DIRECTIONAL_LIGHTS".into(),
                        MAX_DIRECTIONAL_LIGHTS as u32
                    ),
                    ShaderDefVal::UInt(
                        "MAX_CASCADES_PER_LIGHT".into(),
                        MAX_CASCADES_PER_LIGHT as u32,
                    ),
                    ShaderDefVal::UInt("MAX_RECT_LIGHTS".into(), MAX_RECT_LIGHTS as u32,),
                ]
            });
        load_shader_library!(app, "mesh_view_bindings.wgsl");
        load_shader_library!(app, "mesh_types.wgsl");
        load_shader_library!(app, "mesh_functions.wgsl");
        load_shader_library!(app, "skinning.wgsl");
        load_shader_library!(app, "morph.wgsl");
        load_shader_library!(app, "occlusion_culling.wgsl");

        embedded_asset!(app, "mesh.wgsl");

        if app.get_sub_app(RenderApp).is_none() {
            return;
        }

        app.add_systems(
            PostUpdate,
            (no_automatic_skin_batching, no_automatic_morph_batching),
        )
        .add_plugins((
            BinnedRenderPhasePlugin::<Opaque3d, MeshPipeline>::new(self.debug_flags),
            BinnedRenderPhasePlugin::<AlphaMask3d, MeshPipeline>::new(self.debug_flags),
            BinnedRenderPhasePlugin::<Shadow, MeshPipeline>::new(self.debug_flags),
            BinnedRenderPhasePlugin::<Opaque3dDeferred, MeshPipeline>::new(self.debug_flags),
            BinnedRenderPhasePlugin::<AlphaMask3dDeferred, MeshPipeline>::new(self.debug_flags),
            SortedRenderPhasePlugin::<Transmissive3d, MeshPipeline>::new(self.debug_flags),
            SortedRenderPhasePlugin::<Transparent3d, MeshPipeline>::new(self.debug_flags),
        ));

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_gpu_resource::<MeshCullingDataBuffer>()
                .init_resource::<RenderMaterialInstances>()
                .configure_sets(
                    ExtractSchedule,
                    MeshExtractionSystems.after(view::extract_visibility_ranges),
                )
                .add_systems(
                    ExtractSchedule,
                    (
                        extract_skins,
                        extract_morphs,
                        gpu_preprocessing::clear_batched_gpu_instance_buffers::<MeshPipeline>
                            .before(MeshExtractionSystems),
                    ),
                )
                .add_systems(
                    Render,
                    (
                        set_mesh_motion_vector_flags.in_set(RenderSystems::PrepareMeshes),
                        prepare_skins.in_set(RenderSystems::PrepareResources),
                        write_morph_buffers.in_set(RenderSystems::PrepareResourcesFlush),
                        prepare_mesh_bind_groups.in_set(RenderSystems::PrepareBindGroups),
                        prepare_mesh_view_bind_groups
                            .in_set(RenderSystems::PrepareBindGroups)
                            .after(prepare_oit_buffers)
                            .after(write_atmosphere_buffer),
                        no_gpu_preprocessing::clear_batched_cpu_instance_buffers::<MeshPipeline>
                            .in_set(RenderSystems::Cleanup)
                            .after(RenderSystems::Render),
                        prepare_morph_descriptors.in_set(RenderSystems::PrepareMeshes),
                    ),
                );
        }
    }

    fn finish(&self, app: &mut App) {
        let mut mesh_bindings_shader_defs = Vec::with_capacity(1);

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_gpu_resource::<MorphIndices>()
                .init_gpu_resource::<MorphUniforms>()
                .init_gpu_resource::<ViewKeyCache>()
                .init_resource::<GpuPreprocessingSupport>()
                .init_resource::<RenderGpuCulledEntities>()
                .add_systems(RenderStartup, skin_uniforms_from_world)
                .add_systems(
                    Render,
                    check_views_need_specialization.in_set(PrepareAssets),
                );

            let gpu_preprocessing_support =
                render_app.world().resource::<GpuPreprocessingSupport>();
            let use_gpu_instance_buffer_builder =
                self.use_gpu_instance_buffer_builder && gpu_preprocessing_support.is_available();

            let render_mesh_instances = RenderMeshInstances::new(use_gpu_instance_buffer_builder);
            render_app
                .allow_ambiguous_resource::<no_gpu_preprocessing::BatchedInstanceBuffer::<MeshUniform>>()
                .allow_ambiguous_resource::<gpu_preprocessing::BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>()
                .insert_resource(render_mesh_instances);

            if use_gpu_instance_buffer_builder {
                render_app
                    .init_gpu_resource::<gpu_preprocessing::BatchedInstanceBuffers<
                        MeshUniform,
                        MeshInputUniform
                    >>()
                    .init_gpu_resource::<RenderMeshInstanceGpuQueues>()
                    .init_resource::<MeshesToReextractNextFrame>()
                    .add_systems(
                        RenderStartup,
                        mark_all_meshes_for_reextraction,
                    )
                    .add_systems(
                        ExtractSchedule,
                            extract_meshes_for_gpu_building.in_set(MeshExtractionSystems),
                    )
                    .add_systems(
                        Render,
                        (
                            gpu_preprocessing::write_batched_instance_buffers::<MeshPipeline>
                                .in_set(RenderSystems::PrepareResourcesFlush)
                                .after(write_mesh_culling_data_buffer),
                            gpu_preprocessing::delete_old_work_item_buffers::<MeshPipeline>
                                .in_set(RenderSystems::PrepareResources),
                            collect_meshes_for_gpu_building
                                .in_set(RenderSystems::PrepareMeshes)

                                .before(set_mesh_motion_vector_flags)

                                .after(prepare_morph_descriptors),
                            collect_gpu_culled_meshes.in_set(RenderSystems::PrepareMeshes)
                                .after(collect_meshes_for_gpu_building)
                                .before(set_mesh_motion_vector_flags),
                        ),
                    );
            } else {
                render_app
                    .init_gpu_resource::<no_gpu_preprocessing::BatchedInstanceBuffer<MeshUniform>>()
                    .add_systems(
                        ExtractSchedule,
                        extract_meshes_for_cpu_building.in_set(MeshExtractionSystems),
                    )
                    .add_systems(
                        Render,
                        no_gpu_preprocessing::write_batched_instance_buffer::<MeshPipeline>
                            .in_set(RenderSystems::PrepareResourcesFlush),
                    );
            };

            let render_device = render_app.world().resource::<RenderDevice>();
            if let Some(per_object_buffer_batch_size) =
                GpuArrayBuffer::<MeshUniform>::batch_size(&render_device.limits())
            {
                mesh_bindings_shader_defs.push(ShaderDefVal::UInt(
                    "PER_OBJECT_BUFFER_BATCH_SIZE".into(),
                    per_object_buffer_batch_size,
                ));
            }

            render_app.add_systems(
                RenderStartup,
                (init_mesh_pipeline_view_layouts, init_mesh_pipeline)
                    .chain()
                    .in_set(MeshPipelineSystems),
            );
        }

        load_shader_library!(app, "mesh_bindings.wgsl", move |settings| *settings =
            ShaderSettings {
                shader_defs: mesh_bindings_shader_defs.clone(),
            });
    }
}

fn mark_all_meshes_for_reextraction(
    mut render_mesh_instances: ResMut<RenderMeshInstances>,
    mut meshes_to_reextract: ResMut<MeshesToReextractNextFrame>,
) {
    match *render_mesh_instances {
        RenderMeshInstances::CpuBuilding(ref mut cpu) => {
            meshes_to_reextract.extend(cpu.keys());
            cpu.clear();
        }
        RenderMeshInstances::GpuBuilding(ref mut gpu) => {
            meshes_to_reextract.extend(gpu.keys());
            gpu.clear();
        }
    }
}

#[derive(Resource, Deref, DerefMut, Default, Debug, Clone)]
pub struct ViewKeyCache(HashMap<RetainedViewEntity, MeshPipelineKey>);

pub fn check_views_need_specialization(
    mut view_key_cache: ResMut<ViewKeyCache>,
    mut dirty_specializations: ResMut<DirtySpecializations>,
    mut views: Query<(
        &ExtractedView,
        Option<&ExtractedCamera>,
        &Msaa,
        (Option<&Tonemapping>, Option<&DebandDither>),
        Option<&ShadowFilteringMethod>,
        Has<ScreenSpaceAmbientOcclusion>,
        (
            Has<NormalPrepass>,
            Has<DepthPrepass>,
            Has<MotionVectorPrepass>,
            Has<DeferredPrepass>,
        ),
        Option<&ScreenSpaceTransmission>,
        Has<TemporalJitter>,
        Option<&Projection>,
        Has<DistanceFog>,
        (
            Has<RenderViewLightProbes<EnvironmentMapLight>>,
            Has<RenderViewLightProbes<IrradianceVolume>>,
        ),
        (
            Has<OrderIndependentTransparencySettings>,
            Has<ExtractedAtmosphere>,
            Has<ScreenSpaceReflectionsUniform>,
            Has<ContactShadows>,
        ),
    )>,
) {
    for (
        view,
        camera,
        msaa,
        (tonemapping, dither),
        shadow_filter_method,
        ssao,
        (normal_prepass, depth_prepass, motion_vector_prepass, deferred_prepass),
        transmission,
        temporal_jitter,
        projection,
        distance_fog,
        (has_environment_maps, has_irradiance_volumes),
        (has_oit, has_atmosphere, has_ssr, has_contact_shadows),
    ) in views.iter_mut()
    {
        let mut view_key = MeshPipelineKey::from_msaa_samples(msaa.samples())
            | MeshPipelineKey::from_target_format(view.target_format);

        if normal_prepass {
            view_key |= MeshPipelineKey::NORMAL_PREPASS;
        }

        if depth_prepass {
            view_key |= MeshPipelineKey::DEPTH_PREPASS;
        }

        if motion_vector_prepass {
            view_key |= MeshPipelineKey::MOTION_VECTOR_PREPASS;
        }

        if deferred_prepass {
            view_key |= MeshPipelineKey::DEFERRED_PREPASS;
        }

        if temporal_jitter {
            view_key |= MeshPipelineKey::TEMPORAL_JITTER;
        }

        if has_environment_maps {
            view_key |= MeshPipelineKey::ENVIRONMENT_MAP;
        }

        if has_irradiance_volumes {
            view_key |= MeshPipelineKey::IRRADIANCE_VOLUME;
        }

        if has_ssr {
            view_key |= MeshPipelineKey::SCREEN_SPACE_REFLECTIONS;
        }

        if has_oit {
            view_key |= MeshPipelineKey::OIT_ENABLED;
        }

        if has_atmosphere {
            view_key |= MeshPipelineKey::ATMOSPHERE;
        }

        if has_contact_shadows {
            view_key |= MeshPipelineKey::CONTACT_SHADOWS;
        }

        if view.invert_culling {
            view_key |= MeshPipelineKey::INVERT_CULLING;
        }

        if let Some(projection) = projection {
            view_key |= match projection {
                Projection::Perspective(_) => MeshPipelineKey::VIEW_PROJECTION_PERSPECTIVE,
                Projection::Orthographic(_) => MeshPipelineKey::VIEW_PROJECTION_ORTHOGRAPHIC,
                Projection::Custom(_) => MeshPipelineKey::VIEW_PROJECTION_NONSTANDARD,
            };
        }

        match shadow_filter_method.unwrap_or(&ShadowFilteringMethod::default()) {
            ShadowFilteringMethod::Hardware2x2 => {
                view_key |= MeshPipelineKey::SHADOW_FILTER_METHOD_HARDWARE_2X2;
            }
            ShadowFilteringMethod::Gaussian => {
                view_key |= MeshPipelineKey::SHADOW_FILTER_METHOD_GAUSSIAN;
            }
            ShadowFilteringMethod::Temporal => {
                view_key |= MeshPipelineKey::SHADOW_FILTER_METHOD_TEMPORAL;
            }
        }

        if !camera.is_some_and(|camera| camera.hdr) {
            if let Some(tonemapping) = tonemapping {
                view_key |= MeshPipelineKey::TONEMAP_IN_SHADER;
                view_key |= tonemapping_pipeline_key(*tonemapping);
            }
            if let Some(DebandDither::Enabled) = dither {
                view_key |= MeshPipelineKey::DEBAND_DITHER;
            }
        }
        if ssao {
            view_key |= MeshPipelineKey::SCREEN_SPACE_AMBIENT_OCCLUSION;
        }
        if distance_fog {
            view_key |= MeshPipelineKey::DISTANCE_FOG;
        }
        if let Some(transmission) = transmission {
            view_key |= transmission.quality.pipeline_key();
        }
        if !view_key_cache
            .get_mut(&view.retained_view_entity)
            .is_some_and(|current_key| *current_key == view_key)
        {
            view_key_cache.insert(view.retained_view_entity, view_key);
            dirty_specializations
                .views
                .insert(view.retained_view_entity);
        }
    }
}

#[derive(Component)]
pub struct MeshTransforms {
    pub world_from_local: Affine3,
    pub previous_world_from_local: Affine3,
    pub flags: u32,
}

#[derive(ShaderType, Clone)]
pub struct MeshUniform {

    pub world_from_local: [Vec4; 3],
    pub previous_world_from_local: [Vec4; 3],

    pub local_from_world_transpose_a: [Vec4; 2],
    pub local_from_world_transpose_b: f32,
    pub flags: u32,

    pub lightmap_uv_rect: UVec2,

    pub first_vertex_index: u32,

    pub current_skin_index: u32,

    pub material_and_lightmap_bind_group_slot: u32,

    pub tag: u32,

    pub morph_descriptor_index: u32,
}

#[derive(ShaderType, Pod, Zeroable, Clone, Copy, Default, Debug)]
#[repr(C)]
pub struct MeshInputUniform {

    pub world_from_local: [Vec4; 3],

    pub lightmap_uv_rect: UVec2,

    pub flags: u32,

    pub previous_input_index: u32,

    pub first_vertex_index: u32,

    pub first_index_index: u32,

    pub index_count: u32,

    pub current_skin_index: u32,

    pub material_and_lightmap_bind_group_slot: u32,

    pub timestamp: u32,

    pub tag: u32,

    pub morph_descriptor_index: u32,
}

impl_atomic_pod!(MeshInputUniform, MeshInputUniformBlob);

#[derive(ShaderType, Pod, Zeroable, Clone, Copy, Default)]
#[repr(C)]
pub struct MeshCullingData {

    pub aabb_center: Vec4,

    pub aabb_half_extents: Vec4,
}

#[derive(Resource, Deref, DerefMut)]
pub struct MeshCullingDataBuffer(AtomicSparseBufferVec<MeshCullingData>);

impl_atomic_pod!(MeshCullingData, MeshCullingDataBlob);

impl MeshUniform {
    pub fn new(
        mesh_transforms: &MeshTransforms,
        first_vertex_index: u32,
        material_bind_group_slot: MaterialBindGroupSlot,
        maybe_lightmap: Option<(LightmapSlotIndex, Rect)>,
        current_skin_index: Option<u32>,
        morph_descriptor_index: Option<MorphDescriptorIndex>,
        tag: Option<u32>,
    ) -> Self {
        let (local_from_world_transpose_a, local_from_world_transpose_b) =
            mesh_transforms.world_from_local.inverse_transpose_3x3();
        let lightmap_bind_group_slot = match maybe_lightmap {
            None => u16::MAX,
            Some((slot_index, _)) => slot_index.into(),
        };

        let material_slot = u32::from(material_bind_group_slot);
        debug_assert!(
            material_slot <= 0xFFFF,
            "Material bind group slot {material_slot} overflowed"
        );
        let material_and_lightmap_bind_group_slot =
            material_slot | ((lightmap_bind_group_slot as u32) << 16);

        Self {
            world_from_local: mesh_transforms.world_from_local.to_transpose(),
            previous_world_from_local: mesh_transforms.previous_world_from_local.to_transpose(),
            lightmap_uv_rect: pack_lightmap_uv_rect(maybe_lightmap.map(|(_, uv_rect)| uv_rect)),
            local_from_world_transpose_a,
            local_from_world_transpose_b,
            flags: mesh_transforms.flags,
            first_vertex_index,
            current_skin_index: current_skin_index.unwrap_or(u32::MAX),
            material_and_lightmap_bind_group_slot,
            tag: tag.unwrap_or(0),
            morph_descriptor_index: match morph_descriptor_index {
                Some(morph_descriptor_index) => morph_descriptor_index.0,
                None => u32::MAX,
            },
        }
    }
}

bitflags::bitflags! {

    #[repr(transparent)]
    pub struct MeshFlags: u32 {

        const LOD_INDEX_MASK              = (1 << 16) - 1;

        const AABB_BASED_VISIBILITY_RANGE = 1 << 27;

        const NO_FRUSTUM_CULLING          = 1 << 28;
        const SHADOW_RECEIVER             = 1 << 29;
        const TRANSMITTED_SHADOW_RECEIVER = 1 << 30;

        const SIGN_DETERMINANT_MODEL_3X3  = 1 << 31;
        const NONE                        = 0;
        const UNINITIALIZED               = 0xFFFFFFFF;
    }
}

impl MeshFlags {
    fn from_components(
        transform: &GlobalTransform,
        lod_index: Option<NonMaxU16>,
        visibility_range: Option<&VisibilityRange>,
        no_frustum_culling: bool,
        not_shadow_receiver: bool,
        transmitted_receiver: bool,
    ) -> MeshFlags {
        let mut mesh_flags = if not_shadow_receiver {
            MeshFlags::empty()
        } else {
            MeshFlags::SHADOW_RECEIVER
        };
        if visibility_range.is_some_and(|visibility_range| visibility_range.use_aabb) {
            mesh_flags |= MeshFlags::AABB_BASED_VISIBILITY_RANGE;
        }
        if no_frustum_culling {
            mesh_flags |= MeshFlags::NO_FRUSTUM_CULLING;
        }
        if transmitted_receiver {
            mesh_flags |= MeshFlags::TRANSMITTED_SHADOW_RECEIVER;
        }
        if transform.affine().matrix3.determinant().is_sign_positive() {
            mesh_flags |= MeshFlags::SIGN_DETERMINANT_MODEL_3X3;
        }

        let lod_index_bits = match lod_index {
            None => u16::MAX,
            Some(lod_index) => u16::from(lod_index),
        };
        mesh_flags |=
            MeshFlags::from_bits_retain((lod_index_bits as u32) << MeshFlags::LOD_INDEX_SHIFT);

        mesh_flags
    }

    pub const LOD_INDEX_SHIFT: u32 = 0;
}

bitflags::bitflags! {

    #[derive(Clone, Copy, Default, Pod, Zeroable)]
    #[repr(C)]
    pub struct RenderMeshInstanceFlags: u32 {

        const SHADOW_CASTER           = 1 << 0;

        const AUTOMATIC_BATCHING      = 1 << 1;

        const HAS_PREVIOUS_TRANSFORM  = 1 << 2;

        const HAS_PREVIOUS_SKIN       = 1 << 3;

        const HAS_PREVIOUS_MORPH      = 1 << 4;

        const NO_CPU_CULLING          = 1 << 5;
    }
}

#[derive(Deref, DerefMut)]
pub struct RenderMeshInstanceCpu {

    #[deref]
    pub shared: RenderMeshInstanceSharedFlatBlob,

    pub transforms: MeshTransforms,

    pub render_layers: Option<RenderLayers>,
}

#[derive(Deref)]
pub struct RenderMeshInstanceGpu {

    #[deref]
    pub shared: RenderMeshInstanceSharedFlatBlob,

    pub gpu_specific: RenderMeshInstanceGpuFlatBlob,

    pub render_layers: Option<RenderLayers>,
}

#[derive(Clone, Copy, Default, Pod, Zeroable)]
#[repr(C)]
pub struct RenderMeshInstanceSharedFlat {
    asset_id: MeshAssetIdFlat,
    material_bindings_index: MaterialBindingId,
    lightmap_slab_index: LightmapSlabIndexFlat,
    tag: u32,
    current_uniform_index: u32,
    flags: RenderMeshInstanceFlags,
}

impl_atomic_pod!(
    RenderMeshInstanceSharedFlat,
    RenderMeshInstanceSharedFlatBlob,
    field(asset_id: MeshAssetIdFlat, mesh_asset_id_flat, set_mesh_asset_id_flat),
    field(
        material_bindings_index: MaterialBindingId,
        material_bindings_index,
        set_material_bindings_index
    ),
    field(
        lightmap_slab_index: LightmapSlabIndexFlat,
        lightmap_slab_index_flat,
        set_lightmap_slab_index_flat
    ),
    field(tag: u32, tag, set_tag),
    field(flags: RenderMeshInstanceFlags, flags, set_flags),
);

impl RenderMeshInstanceSharedFlatBlob {
    pub fn mesh_asset_id(&self) -> AssetId<Mesh> {
        self.mesh_asset_id_flat().into()
    }

    pub fn set_mesh_asset_id(&self, asset_id: AssetId<Mesh>) {
        self.set_mesh_asset_id_flat(asset_id.into());
    }

    pub fn lightmap_slab_index(&self) -> Option<LightmapSlabIndex> {
        self.lightmap_slab_index_flat().into()
    }

    pub fn set_lightmap_slab_index(&self, lightmap_slab_index: Option<LightmapSlabIndex>) {
        self.set_lightmap_slab_index_flat(lightmap_slab_index.into());
    }

    #[inline]
    pub fn should_batch(&self) -> bool {
        self.flags()
            .contains(RenderMeshInstanceFlags::AUTOMATIC_BATCHING)
    }
}

#[derive(Clone, Copy, Default, Pod, Zeroable)]
#[repr(C)]
pub struct RenderMeshInstanceGpuFlat {
    current_uniform_index: u32,
}

impl_atomic_pod!(
    RenderMeshInstanceGpuFlat,
    RenderMeshInstanceGpuFlatBlob,
    field(current_uniform_index: u32, current_uniform_index, set_current_uniform_index),
);

#[derive(Clone, Copy, Default, Pod, Zeroable)]
#[repr(C)]
struct MeshAssetIdFlat {
    mode: u32,
    words: [u32; 4],
}

#[derive(Clone, Copy, Default, Pod, Zeroable)]
#[repr(C)]
struct LightmapSlabIndexFlat(u32);

const MESH_ASSET_ID_FLAT_MODE_INDEX: u32 = 0;
const MESH_ASSET_ID_FLAT_MODE_UUID: u32 = 1;

impl From<AssetId<Mesh>> for MeshAssetIdFlat {
    #[inline]
    fn from(value: AssetId<Mesh>) -> Self {
        match value {
            AssetId::Index { index, .. } => {
                let bits = index.to_bits();
                MeshAssetIdFlat {
                    mode: MESH_ASSET_ID_FLAT_MODE_INDEX,
                    words: [(bits & 0xffff_ffff) as u32, (bits >> 32) as u32, 0, 0],
                }
            }
            AssetId::Uuid { uuid } => {
                let (hi, lo) = uuid.as_u64_pair();
                MeshAssetIdFlat {
                    mode: MESH_ASSET_ID_FLAT_MODE_UUID,
                    words: [
                        (lo & 0xffff_ffff) as u32,
                        (lo >> 32) as u32,
                        (hi & 0xffff_ffff) as u32,
                        (hi >> 32) as u32,
                    ],
                }
            }
        }
    }
}

impl From<MeshAssetIdFlat> for AssetId<Mesh> {
    #[inline]
    fn from(value: MeshAssetIdFlat) -> AssetId<Mesh> {
        if value.mode == MESH_ASSET_ID_FLAT_MODE_INDEX {
            AssetId::from(AssetIndex::from_bits(
                (value.words[0] as u64) | ((value.words[1] as u64) << 32),
            ))
        } else {
            let lo = (value.words[0] as u64) | ((value.words[1] as u64) << 32);
            let hi = (value.words[2] as u64) | ((value.words[3] as u64) << 32);
            AssetId::Uuid {
                uuid: Uuid::from_u64_pair(hi, lo),
            }
        }
    }
}

impl From<Option<LightmapSlabIndex>> for LightmapSlabIndexFlat {
    #[inline]
    fn from(value: Option<LightmapSlabIndex>) -> Self {
        match value {
            Some(slab_index) => LightmapSlabIndexFlat((*slab_index).into()),
            None => LightmapSlabIndexFlat(!0),
        }
    }
}

impl From<LightmapSlabIndexFlat> for Option<LightmapSlabIndex> {
    #[inline]
    fn from(value: LightmapSlabIndexFlat) -> Self {
        NonMaxU32::new(value.0).map(LightmapSlabIndex)
    }
}

pub struct RenderMeshInstanceShared {

    pub mesh_asset_id: AssetId<Mesh>,

    pub material_bindings_index: MaterialBindingId,

    pub lightmap_slab_index: Option<LightmapSlabIndex>,

    pub center: Vec3,

    pub tag: u32,

    pub flags: RenderMeshInstanceFlags,
}

pub struct RenderMeshInstanceGpuBuilder {

    pub shared: RenderMeshInstanceSharedFlat,

    pub world_from_local: Affine3,

    pub lightmap_uv_rect: UVec2,

    pub previous_input_index: Option<NonMaxU32>,

    pub render_layers: Option<RenderLayers>,

    pub mesh_flags: MeshFlags,
}

#[derive(Default)]
pub enum RenderMeshInstanceGpuQueue {

    #[default]
    None,

    CpuCulling {

        changed: Vec<(MainEntity, RenderMeshInstanceGpuBuilder)>,

        removed: Vec<MainEntity>,
    },

    GpuCulling {

        changed_cpu_culling: Vec<(MainEntity, RenderMeshInstanceGpuBuilder, MeshCullingData)>,

        changed_gpu_culling: Vec<(MainEntity, RenderMeshInstanceGpuBuilder, MeshCullingData)>,

        removed: Vec<MainEntity>,
    },
}

#[derive(Resource, Default, Deref, DerefMut)]
pub struct RenderMeshInstanceGpuQueues(Parallel<RenderMeshInstanceGpuQueue>);

#[derive(Resource, Default, Deref, DerefMut)]
pub struct MeshesToReextractNextFrame(MainEntityHashSet);

impl RenderMeshInstanceSharedFlat {

    fn for_gpu_building(
        previous_transform: Option<&PreviousGlobalTransform>,
        mesh: &Mesh3d,
        tag: Option<&MeshTag>,
        not_shadow_caster: bool,
        no_automatic_batching: bool,
        no_cpu_culling: bool,
    ) -> Self {
        Self::new(
            previous_transform,
            mesh,
            tag,
            default(),
            not_shadow_caster,
            no_automatic_batching,
            no_cpu_culling,
        )
    }

    fn for_cpu_building(
        previous_transform: Option<&PreviousGlobalTransform>,
        mesh: &Mesh3d,
        tag: Option<&MeshTag>,
        material_bindings_index: MaterialBindingId,
        not_shadow_caster: bool,
        no_automatic_batching: bool,
    ) -> Self {
        Self::new(
            previous_transform,
            mesh,
            tag,
            material_bindings_index,
            not_shadow_caster,
            no_automatic_batching,
            false,
        )
    }

    fn new(
        previous_transform: Option<&PreviousGlobalTransform>,
        mesh: &Mesh3d,
        tag: Option<&MeshTag>,
        material_bindings_index: MaterialBindingId,
        not_shadow_caster: bool,
        no_automatic_batching: bool,
        no_cpu_culling: bool,
    ) -> Self {
        let mut mesh_instance_flags = RenderMeshInstanceFlags::empty();
        mesh_instance_flags.set(RenderMeshInstanceFlags::SHADOW_CASTER, !not_shadow_caster);
        mesh_instance_flags.set(
            RenderMeshInstanceFlags::AUTOMATIC_BATCHING,
            !no_automatic_batching,
        );
        mesh_instance_flags.set(
            RenderMeshInstanceFlags::HAS_PREVIOUS_TRANSFORM,
            previous_transform.is_some(),
        );
        mesh_instance_flags.set(RenderMeshInstanceFlags::NO_CPU_CULLING, no_cpu_culling);

        RenderMeshInstanceSharedFlat {
            asset_id: mesh.id().into(),
            material_bindings_index,
            tag: tag.map_or(0, |i| **i),
            flags: mesh_instance_flags,

            lightmap_slab_index: LightmapSlabIndexFlat::default(),

            current_uniform_index: 0,
        }
    }
}

#[derive(Resource)]
pub enum RenderMeshInstances {

    CpuBuilding(RenderMeshInstancesCpu),

    GpuBuilding(RenderMeshInstancesGpu),
}

#[derive(Default, Deref, DerefMut)]
pub struct RenderMeshInstancesCpu(MainEntityHashMap<RenderMeshInstanceCpu>);

#[derive(Default, Deref, DerefMut)]
pub struct RenderMeshInstancesGpu(MainEntityHashMap<RenderMeshInstanceGpu>);

impl RenderMeshInstances {

    fn new(use_gpu_instance_buffer_builder: bool) -> RenderMeshInstances {
        if use_gpu_instance_buffer_builder {
            RenderMeshInstances::GpuBuilding(RenderMeshInstancesGpu::default())
        } else {
            RenderMeshInstances::CpuBuilding(RenderMeshInstancesCpu::default())
        }
    }

    pub fn mesh_asset_id(&self, entity: MainEntity) -> Option<AssetId<Mesh>> {
        match *self {
            RenderMeshInstances::CpuBuilding(ref instances) => instances.mesh_asset_id(entity),
            RenderMeshInstances::GpuBuilding(ref instances) => instances.mesh_asset_id(entity),
        }
    }

    pub fn render_mesh_queue_data(&self, entity: MainEntity) -> Option<RenderMeshQueueData<'_>> {
        match *self {
            RenderMeshInstances::CpuBuilding(ref instances) => {
                instances.render_mesh_queue_data(entity)
            }
            RenderMeshInstances::GpuBuilding(ref instances) => {
                instances.render_mesh_queue_data(entity)
            }
        }
    }

    fn insert_mesh_instance_flags(&mut self, entity: MainEntity, flags: RenderMeshInstanceFlags) {
        match *self {
            RenderMeshInstances::CpuBuilding(ref mut instances) => {
                instances.insert_mesh_instance_flags(entity, flags);
            }
            RenderMeshInstances::GpuBuilding(ref mut instances) => {
                instances.insert_mesh_instance_flags(entity, flags);
            }
        }
    }
}

impl RenderMeshInstancesCpu {
    fn mesh_asset_id(&self, entity: MainEntity) -> Option<AssetId<Mesh>> {
        self.get(&entity)
            .map(|render_mesh_instance| render_mesh_instance.mesh_asset_id())
    }

    fn render_mesh_queue_data(&self, entity: MainEntity) -> Option<RenderMeshQueueData<'_>> {
        self.get(&entity)
            .map(|render_mesh_instance| RenderMeshQueueData {
                shared: &render_mesh_instance.shared,
                render_layers: render_mesh_instance.render_layers.clone(),
                current_uniform_index: InputUniformIndex::default(),
            })
    }

    fn insert_mesh_instance_flags(&mut self, entity: MainEntity, flags: RenderMeshInstanceFlags) {
        if let Some(instance) = self.get_mut(&entity) {
            instance.set_flags(instance.flags() | flags);
        }
    }
}

impl RenderMeshInstancesGpu {
    fn mesh_asset_id(&self, entity: MainEntity) -> Option<AssetId<Mesh>> {
        self.get(&entity)
            .map(|render_mesh_instance| render_mesh_instance.mesh_asset_id())
    }

    fn render_mesh_queue_data(&self, entity: MainEntity) -> Option<RenderMeshQueueData<'_>> {
        self.get(&entity)
            .map(|render_mesh_instance| RenderMeshQueueData {
                shared: &render_mesh_instance.shared,
                render_layers: render_mesh_instance.render_layers.clone(),
                current_uniform_index: InputUniformIndex(
                    render_mesh_instance.gpu_specific.current_uniform_index(),
                ),
            })
    }

    fn insert_mesh_instance_flags(&mut self, entity: MainEntity, flags: RenderMeshInstanceFlags) {
        if let Some(instance) = self.get_mut(&entity) {
            instance.set_flags(instance.flags() | flags);
        }
    }
}

impl RenderMeshInstanceGpuQueue {

    fn init(&mut self, any_gpu_culling: bool) {
        match (any_gpu_culling, &mut *self) {
            (
                true,
                RenderMeshInstanceGpuQueue::GpuCulling {
                    changed_cpu_culling,
                    changed_gpu_culling,
                    removed,
                },
            ) => {
                changed_cpu_culling.clear();
                changed_gpu_culling.clear();
                removed.clear();
            }
            (true, _) => {
                *self = RenderMeshInstanceGpuQueue::GpuCulling {
                    changed_cpu_culling: vec![],
                    changed_gpu_culling: vec![],
                    removed: vec![],
                }
            }
            (false, RenderMeshInstanceGpuQueue::CpuCulling { changed, removed }) => {
                changed.clear();
                removed.clear();
            }
            (false, _) => {
                *self = RenderMeshInstanceGpuQueue::CpuCulling {
                    changed: vec![],
                    removed: vec![],
                }
            }
        }
    }

    fn push(
        &mut self,
        entity: MainEntity,
        instance_builder: RenderMeshInstanceGpuBuilder,
        culling_data_builder: Option<MeshCullingData>,
        no_cpu_culling: bool,
    ) {
        match (&mut *self, culling_data_builder, no_cpu_culling) {
            (
                &mut RenderMeshInstanceGpuQueue::CpuCulling {
                    changed: ref mut queue,
                    ..
                },
                None,
                _,
            ) => {
                queue.push((entity, instance_builder));
            }

            (
                &mut RenderMeshInstanceGpuQueue::GpuCulling {
                    changed_cpu_culling: ref mut queue,
                    ..
                },
                Some(culling_data_builder),
                false,
            )
            | (
                &mut RenderMeshInstanceGpuQueue::GpuCulling {
                    changed_gpu_culling: ref mut queue,
                    ..
                },
                Some(culling_data_builder),
                true,
            ) => {
                queue.push((entity, instance_builder, culling_data_builder));
            }

            (_, None, _) => {
                *self = RenderMeshInstanceGpuQueue::CpuCulling {
                    changed: vec![(entity, instance_builder)],
                    removed: vec![],
                };
            }

            (_, Some(culling_data_builder), false) => {
                *self = RenderMeshInstanceGpuQueue::GpuCulling {
                    changed_cpu_culling: vec![(entity, instance_builder, culling_data_builder)],
                    changed_gpu_culling: vec![],
                    removed: vec![],
                };
            }
            (_, Some(culling_data_builder), true) => {
                *self = RenderMeshInstanceGpuQueue::GpuCulling {
                    changed_cpu_culling: vec![],
                    changed_gpu_culling: vec![(entity, instance_builder, culling_data_builder)],
                    removed: vec![],
                };
            }
        }
    }

    fn remove(&mut self, entity: MainEntity, gpu_culling: bool) {
        match (&mut *self, gpu_culling) {
            (RenderMeshInstanceGpuQueue::None, false) => {
                *self = RenderMeshInstanceGpuQueue::CpuCulling {
                    changed: vec![],
                    removed: vec![entity],
                }
            }
            (RenderMeshInstanceGpuQueue::None, true) => {
                *self = RenderMeshInstanceGpuQueue::GpuCulling {
                    changed_cpu_culling: vec![],
                    changed_gpu_culling: vec![],
                    removed: vec![entity],
                }
            }
            (RenderMeshInstanceGpuQueue::CpuCulling { removed, .. }, _)
            | (RenderMeshInstanceGpuQueue::GpuCulling { removed, .. }, _) => {
                removed.push(entity);
            }
        }
    }
}

impl RenderMeshInstanceGpuBuilder {

    fn prepare(
        mut self,
        entity: MainEntity,
        mesh_allocator: &MeshAllocator,
        mesh_material_ids: &RenderMaterialInstances,
        render_material_bindings: &RenderMaterialBindings,
        render_lightmaps: &RenderLightmaps,
        skin_uniforms: &SkinUniforms,
        morph_indices: &MorphIndices,
        timestamp: FrameCount,
    ) -> Option<RenderMeshInstanceGpuPrepared> {

        let mesh_material = mesh_material_ids.mesh_material(entity);
        let mesh_material_binding_id = if mesh_material != DUMMY_MESH_MATERIAL.untyped() {
            render_material_bindings.get(&mesh_material).copied()?
        } else {

            MaterialBindingId::default()
        };
        self.shared.material_bindings_index = mesh_material_binding_id;

        let (first_vertex_index, vertex_count) =
            match mesh_allocator.mesh_vertex_slice(&self.shared.asset_id.into()) {
                Some(mesh_vertex_slice) => (
                    mesh_vertex_slice.range.start,
                    mesh_vertex_slice.range.end - mesh_vertex_slice.range.start,
                ),
                None => (0, 0),
            };
        let (mesh_is_indexed, first_index_index, index_count) =
            match mesh_allocator.mesh_index_slice(&self.shared.asset_id.into()) {
                Some(mesh_index_slice) => (
                    true,
                    mesh_index_slice.range.start,
                    mesh_index_slice.range.end - mesh_index_slice.range.start,
                ),
                None => (false, 0, 0),
            };
        let current_skin_index = match skin_uniforms.skin_byte_offset(entity) {
            Some(skin_index) => skin_index.index(),
            None => u32::MAX,
        };

        let lightmap_slot = match render_lightmaps.render_lightmaps.get(&entity) {
            Some(render_lightmap) => u16::from(*render_lightmap.slot_index),
            None => u16::MAX,
        };
        let lightmap_slab_index = render_lightmaps
            .render_lightmaps
            .get(&entity)
            .map(|lightmap| lightmap.slab_index);
        self.shared.lightmap_slab_index = lightmap_slab_index.into();

        let morph_descriptor_index = match morph_indices.morph_descriptor_index(entity) {
            Some(morph_descriptor_index) => *morph_descriptor_index,
            None => u32::MAX,
        };

        let material_slot = u32::from(self.shared.material_bindings_index.slot);
        debug_assert!(
            material_slot <= 0xFFFF,
            "Material bind group slot {material_slot} overflowed"
        );
        let material_and_lightmap_bind_group_slot = material_slot | ((lightmap_slot as u32) << 16);

        let mesh_input_uniform = MeshInputUniform {
            world_from_local: self.world_from_local.to_transpose(),
            lightmap_uv_rect: self.lightmap_uv_rect,
            flags: self.mesh_flags.bits(),
            previous_input_index: u32::MAX,
            timestamp: timestamp.0,
            first_vertex_index,
            first_index_index,
            index_count: if mesh_is_indexed {
                index_count
            } else {
                vertex_count
            },
            current_skin_index,
            material_and_lightmap_bind_group_slot,
            tag: self.shared.tag,
            morph_descriptor_index,
        };

        Some(RenderMeshInstanceGpuPrepared {
            shared: self.shared,
            mesh_input_uniform,
            render_layers: self.render_layers,
        })
    }
}

pub struct RenderMeshInstanceGpuPrepared {

    shared: RenderMeshInstanceSharedFlat,

    mesh_input_uniform: MeshInputUniform,

    render_layers: Option<RenderLayers>,
}

impl RenderMeshInstanceGpuPrepared {

    fn update(
        mut self,
        entity: MainEntity,
        render_mesh_instances: &mut MainEntityHashMap<RenderMeshInstanceGpu>,
        current_input_buffer: &mut InstanceInputUniformBuffer<MeshInputUniform>,
        previous_input_buffer: &PreviousInstanceInputUniformBuffer<MeshInputUniform>,
    ) -> u32 {

        let current_uniform_index;
        match render_mesh_instances.entry(entity) {
            Entry::Occupied(mut occupied_entry) => {

                current_uniform_index = occupied_entry
                    .get_mut()
                    .gpu_specific
                    .current_uniform_index();

                let previous_mesh_input_uniform =
                    current_input_buffer.get_unchecked(current_uniform_index);
                let previous_input_index = previous_input_buffer.push(previous_mesh_input_uniform);
                self.mesh_input_uniform.previous_input_index = previous_input_index;

                current_input_buffer.set(current_uniform_index, self.mesh_input_uniform);

                let existing_instance = occupied_entry.get_mut();
                self.shared.write_to_blob(&existing_instance.shared);
                existing_instance
                    .gpu_specific
                    .set_current_uniform_index(current_uniform_index);
                existing_instance.render_layers = self.render_layers.clone();
            }

            Entry::Vacant(vacant_entry) => {

                current_uniform_index = current_input_buffer.add(self.mesh_input_uniform);

                let new_instance = vacant_entry.insert(RenderMeshInstanceGpu {
                    shared: RenderMeshInstanceSharedFlatBlob::default(),
                    gpu_specific: RenderMeshInstanceGpuFlatBlob::default(),
                    render_layers: self.render_layers.clone(),
                });
                self.shared.write_to_blob(&new_instance.shared);
                RenderMeshInstanceGpuFlat {
                    current_uniform_index,
                }
                .write_to_blob(&new_instance.gpu_specific);
            }
        }

        current_uniform_index
    }
}

fn remove_mesh_input_uniform(
    entity: MainEntity,
    render_mesh_instances: &mut MainEntityHashMap<RenderMeshInstanceGpu>,
    current_input_buffer: &mut InstanceInputUniformBuffer<MeshInputUniform>,
) -> Option<u32> {

    let removed_render_mesh_instance = render_mesh_instances.remove(&entity)?;

    let removed_uniform_index = removed_render_mesh_instance
        .gpu_specific
        .current_uniform_index();
    current_input_buffer.remove(removed_uniform_index);
    Some(removed_uniform_index)
}

impl MeshCullingData {

    fn new(aabb: Option<&Aabb>) -> Self {
        match aabb {
            Some(aabb) => MeshCullingData {
                aabb_center: aabb.center.extend(0.0),
                aabb_half_extents: aabb.half_extents.extend(0.0),
            },
            None => MeshCullingData {
                aabb_center: Vec3::ZERO.extend(0.0),
                aabb_half_extents: Vec3::INFINITY.extend(0.0),
            },
        }
    }

    fn update(
        &self,
        mesh_culling_data_buffer: &mut MeshCullingDataBuffer,
        instance_data_index: u32,
    ) {
        while mesh_culling_data_buffer.len() < instance_data_index + 1 {
            mesh_culling_data_buffer.push(MeshCullingData::default());
        }
        mesh_culling_data_buffer.set(instance_data_index, *self);
    }
}

impl Default for MeshCullingDataBuffer {
    #[inline]
    fn default() -> Self {
        Self(AtomicSparseBufferVec::new(
            BufferUsages::STORAGE,
            8,
            Arc::from("mesh culling data buffer"),
        ))
    }
}

#[derive(Deref)]
pub struct RenderMeshQueueData<'a> {

    #[deref]
    pub shared: &'a RenderMeshInstanceSharedFlatBlob,

    pub render_layers: Option<RenderLayers>,

    pub current_uniform_index: InputUniformIndex,
}

#[derive(SystemSet, Clone, PartialEq, Eq, Debug, Hash)]
pub struct MeshExtractionSystems;

#[derive(Resource, Default)]
pub struct RenderGpuCulledEntities {

    pub entities: MainEntityHashMap<RenderLayers>,

    pub added: Vec<MainEntity>,

    pub removed: Vec<MainEntity>,

    pub changed_layers: Vec<MainEntity>,
}

impl RenderGpuCulledEntities {

    pub fn prepare_for_new_frame(&mut self) {
        self.added.clear();
        self.removed.clear();
        self.changed_layers.clear();
    }

    pub fn update(
        &mut self,
        new_entity: MainEntity,
        render_layers: RenderLayers,
        no_cpu_culling: bool,
    ) {
        match self.entities.entry(new_entity) {
            Entry::Occupied(mut occupied_entry) => {
                if no_cpu_culling {
                    occupied_entry.insert(render_layers);
                } else {
                    occupied_entry.remove();
                    self.removed.push(new_entity);
                }
            }
            Entry::Vacant(vacant_entry) => {
                if no_cpu_culling {
                    vacant_entry.insert(render_layers);
                    self.added.push(new_entity);
                }
            }
        }
    }

    pub fn remove(&mut self, old_entity: MainEntity) {
        self.removed.push(old_entity);
        self.entities.remove(&old_entity);
    }
}

pub fn extract_meshes_for_cpu_building(
    mut render_mesh_instances: ResMut<RenderMeshInstances>,
    mesh_material_ids: Res<RenderMaterialInstances>,
    render_material_bindings: Res<RenderMaterialBindings>,
    render_visibility_ranges: Res<RenderVisibilityRanges>,
    mut render_mesh_instance_queues: Local<Parallel<Vec<(Entity, RenderMeshInstanceCpu)>>>,
    meshes_query: Extract<
        Query<(
            Entity,
            &ViewVisibility,
            &GlobalTransform,
            Option<&PreviousGlobalTransform>,
            &Mesh3d,
            Option<&MeshTag>,
            Has<NoFrustumCulling>,
            Has<NotShadowReceiver>,
            Has<TransmittedShadowReceiver>,
            Has<NotShadowCaster>,
            Has<NoAutomaticBatching>,
            Option<&VisibilityRange>,
            Option<&RenderLayers>,
        )>,
    >,
) {
    meshes_query.par_iter().for_each_init(
        || render_mesh_instance_queues.borrow_local_mut(),
        |queue,
         (
            entity,
            view_visibility,
            transform,
            previous_transform,
            mesh,
            tag,
            no_frustum_culling,
            not_shadow_receiver,
            transmitted_receiver,
            not_shadow_caster,
            no_automatic_batching,
            visibility_range,
            render_layers,
        )| {
            if !view_visibility.get() {
                return;
            }

            let mut lod_index = None;
            if visibility_range.is_some() {
                lod_index = render_visibility_ranges.lod_index_for_entity(entity.into());
            }

            let mesh_flags = MeshFlags::from_components(
                transform,
                lod_index,
                visibility_range,
                no_frustum_culling,
                not_shadow_receiver,
                transmitted_receiver,
            );

            let mesh_material = mesh_material_ids.mesh_material(MainEntity::from(entity));

            let material_bindings_index = render_material_bindings
                .get(&mesh_material)
                .copied()
                .unwrap_or_default();

            let shared = RenderMeshInstanceSharedFlat::for_cpu_building(
                previous_transform,
                mesh,
                tag,
                material_bindings_index,
                not_shadow_caster,
                no_automatic_batching,
            );

            let world_from_local = transform.affine();
            let previous_world_from_local = previous_transform
                .map(|previous_transform| previous_transform.0)
                .unwrap_or(world_from_local);

            queue.push((
                entity,
                RenderMeshInstanceCpu {
                    transforms: MeshTransforms {
                        world_from_local: world_from_local.into(),
                        previous_world_from_local: previous_world_from_local.into(),
                        flags: mesh_flags.bits(),
                    },
                    shared: (&shared).into(),
                    render_layers: render_layers.cloned(),
                },
            ));
        },
    );

    let RenderMeshInstances::CpuBuilding(ref mut render_mesh_instances) = *render_mesh_instances
    else {
        panic!(
            "`extract_meshes_for_cpu_building` should only be called if we're using CPU \
            `MeshUniform` building"
        );
    };

    render_mesh_instances.clear();
    for queue in render_mesh_instance_queues.iter_mut() {
        for (entity, render_mesh_instance) in queue.drain(..) {
            render_mesh_instances.insert(entity.into(), render_mesh_instance);
        }
    }
}

type GpuMeshExtractionQuery = (
    Entity,
    Read<ViewVisibility>,
    Read<GlobalTransform>,
    Option<Read<PreviousGlobalTransform>>,
    Option<Read<Lightmap>>,
    Option<Read<Aabb>>,
    Read<Mesh3d>,
    Option<Read<MeshTag>>,
    (
        Has<NoFrustumCulling>,
        Has<NotShadowReceiver>,
        Has<TransmittedShadowReceiver>,
        Has<NotShadowCaster>,
        Has<NoAutomaticBatching>,
        Has<NoCpuCulling>,
    ),
    Option<Read<VisibilityRange>>,
    Option<Read<RenderLayers>>,
);

pub fn extract_meshes_for_gpu_building(
    mut render_mesh_instances: ResMut<RenderMeshInstances>,
    render_visibility_ranges: Res<RenderVisibilityRanges>,
    mut render_mesh_instance_queues: ResMut<RenderMeshInstanceGpuQueues>,
    changed_meshes_query: Extract<
        Query<
            GpuMeshExtractionQuery,
            Or<(
                Changed<ViewVisibility>,
                Changed<GlobalTransform>,
                Changed<PreviousGlobalTransform>,
                Changed<Lightmap>,
                Changed<Aabb>,
                Changed<Mesh3d>,
                Changed<MeshTag>,
                Or<(
                    Changed<NoFrustumCulling>,
                    Changed<NotShadowReceiver>,
                    Changed<TransmittedShadowReceiver>,
                    Changed<NotShadowCaster>,
                    Changed<NoAutomaticBatching>,
                    Changed<NoCpuCulling>,
                )>,
                Changed<VisibilityRange>,
                Changed<SkinnedMesh>,
            )>,
        >,
    >,
    (
        mut removed_previous_global_transform_query,
        mut removed_lightmap_query,
        mut removed_aabb_query,
        mut removed_mesh_tag_query,
        mut removed_no_frustum_culling_query,
        mut removed_not_shadow_receiver_query,
        mut removed_transmitted_receiver_query,
        mut removed_not_shadow_caster_query,
        mut removed_no_automatic_batching_query,
        mut removed_no_cpu_culling_query,
        mut removed_visibility_range_query,
        mut removed_skinned_mesh_query,
    ): (
        Extract<RemovedComponents<PreviousGlobalTransform>>,
        Extract<RemovedComponents<Lightmap>>,
        Extract<RemovedComponents<Aabb>>,
        Extract<RemovedComponents<MeshTag>>,
        Extract<RemovedComponents<NoFrustumCulling>>,
        Extract<RemovedComponents<NotShadowReceiver>>,
        Extract<RemovedComponents<TransmittedShadowReceiver>>,
        Extract<RemovedComponents<NotShadowCaster>>,
        Extract<RemovedComponents<NoAutomaticBatching>>,
        Extract<RemovedComponents<NoCpuCulling>>,
        Extract<RemovedComponents<VisibilityRange>>,
        Extract<RemovedComponents<SkinnedMesh>>,
    ),
    all_meshes_query: Extract<Query<GpuMeshExtractionQuery>>,
    mut removed_meshes_query: Extract<RemovedComponents<Mesh3d>>,
    gpu_culling_query: Extract<Query<(), (With<Camera>, Without<NoIndirectDrawing>)>>,
    meshes_to_reextract_next_frame: ResMut<MeshesToReextractNextFrame>,
    mut reextract_entities: Local<EntityHashSet>,
    mut potential_reextraction_set: Local<IndexSet<Entity, EntityHash>>,
    mut potential_reextraction_bitfield: Local<Vec<AtomicU64>>,
) {
    reextract_entities.clear();

    let any_gpu_culling = !gpu_culling_query.is_empty();
    for render_mesh_instance_queue in render_mesh_instance_queues.iter_mut() {
        render_mesh_instance_queue.init(any_gpu_culling);
    }

    potential_reextraction_set.clear();
    potential_reextraction_set.extend(
        meshes_to_reextract_next_frame
            .iter()
            .map(|&e| *e)
            .chain(removed_previous_global_transform_query.read())
            .chain(removed_lightmap_query.read())
            .chain(removed_aabb_query.read())
            .chain(removed_mesh_tag_query.read())
            .chain(removed_no_frustum_culling_query.read())
            .chain(removed_not_shadow_receiver_query.read())
            .chain(removed_transmitted_receiver_query.read())
            .chain(removed_not_shadow_caster_query.read())
            .chain(removed_no_automatic_batching_query.read())
            .chain(removed_no_cpu_culling_query.read())
            .chain(removed_visibility_range_query.read())
            .chain(removed_skinned_mesh_query.read()),
    );

    potential_reextraction_bitfield.clear();
    potential_reextraction_bitfield.extend(
        iter::repeat_with(|| AtomicU64::new(0)).take(potential_reextraction_set.len().div_ceil(64)),
    );

    let RenderMeshInstances::GpuBuilding(ref mut render_mesh_instances) = *render_mesh_instances
    else {
        panic!(
            "`extract_meshes_for_gpu_building` should only be called if we're \
            using GPU `MeshUniform` building"
        );
    };

    changed_meshes_query.par_iter().for_each_init(
        || render_mesh_instance_queues.borrow_local_mut(),
        |queue, query_row| {
            extract_mesh_for_gpu_building(
                query_row,
                &render_visibility_ranges,
                render_mesh_instances,
                queue,
                any_gpu_culling,
            );

            if let Some(bit_index) = potential_reextraction_set.get_index_of(&query_row.0) {
                potential_reextraction_bitfield[bit_index / 64]
                    .fetch_or(1 << (bit_index % 64), Ordering::Relaxed);
            }
        },
    );

    for bit in AtomicU64ZeroBitIter::new(&potential_reextraction_bitfield) {
        if let Some(entity) = potential_reextraction_set.get_index(bit as usize) {
            reextract_entities.insert(*entity);
        }
    }

    let mut queue = render_mesh_instance_queues.borrow_local_mut();
    for entity in &reextract_entities {
        if let Ok(query_row) = all_meshes_query.get(*entity) {
            extract_mesh_for_gpu_building(
                query_row,
                &render_visibility_ranges,
                render_mesh_instances,
                &mut queue,
                any_gpu_culling,
            );
        }
    }

    for entity in removed_meshes_query.read() {

        let entity = MainEntity::from(entity);
        if !changed_meshes_query.contains(*entity)
            && !meshes_to_reextract_next_frame.contains(&entity)
        {
            queue.remove(entity, any_gpu_culling);
        }
    }
}

fn extract_mesh_for_gpu_building(
    (
        entity,
        view_visibility,
        transform,
        previous_transform,
        lightmap,
        aabb,
        mesh,
        tag,
        (
            no_frustum_culling,
            not_shadow_receiver,
            transmitted_receiver,
            not_shadow_caster,
            no_automatic_batching,
            no_cpu_culling,
        ),
        visibility_range,
        render_layers,
    ): <GpuMeshExtractionQuery as QueryData>::Item<'_, '_>,
    render_visibility_ranges: &RenderVisibilityRanges,
    render_mesh_instances: &RenderMeshInstancesGpu,
    queue: &mut RenderMeshInstanceGpuQueue,
    any_gpu_culling: bool,
) {

    if !view_visibility.get() {
        queue.remove(entity.into(), any_gpu_culling);
        return;
    }

    let mut lod_index = None;
    if visibility_range.is_some() {
        lod_index = render_visibility_ranges.lod_index_for_entity(entity.into());
    }

    let mesh_flags = MeshFlags::from_components(
        transform,
        lod_index,
        visibility_range,
        no_frustum_culling,
        not_shadow_receiver,
        transmitted_receiver,
    );

    let shared = RenderMeshInstanceSharedFlat::for_gpu_building(
        previous_transform,
        mesh,
        tag,
        not_shadow_caster,
        no_automatic_batching,
        no_cpu_culling,
    );

    let lightmap_uv_rect = pack_lightmap_uv_rect(lightmap.map(|lightmap| lightmap.uv_rect));

    let gpu_mesh_culling_data = any_gpu_culling.then(|| MeshCullingData::new(aabb));

    let previous_input_index = if shared
        .flags
        .contains(RenderMeshInstanceFlags::HAS_PREVIOUS_TRANSFORM)
    {
        render_mesh_instances
            .get(&MainEntity::from(entity))
            .and_then(|render_mesh_instance| {
                NonMaxU32::new(render_mesh_instance.gpu_specific.current_uniform_index())
            })
    } else {
        None
    };

    let gpu_mesh_instance_builder = RenderMeshInstanceGpuBuilder {
        shared,
        world_from_local: (transform.affine()).into(),
        lightmap_uv_rect,
        mesh_flags,
        previous_input_index,
        render_layers: render_layers.cloned(),
    };

    queue.push(
        entity.into(),
        gpu_mesh_instance_builder,
        gpu_mesh_culling_data,
        no_cpu_culling,
    );
}

struct AtomicU64ZeroBitIter<'a> {

    bits: &'a [AtomicU64],

    current_word: u64,

    next_index: usize,
}

impl<'a> AtomicU64ZeroBitIter<'a> {

    fn new(bits: &'a [AtomicU64]) -> AtomicU64ZeroBitIter<'a> {
        AtomicU64ZeroBitIter {
            bits,
            current_word: !0,
            next_index: 0,
        }
    }
}

impl<'a> Iterator for AtomicU64ZeroBitIter<'a> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {

        while self.current_word == !0 {
            self.current_word = self.bits.get(self.next_index)?.load(Ordering::Relaxed);
            self.next_index += 1;
        }

        let bit_index = self.current_word.trailing_ones();
        self.current_word |= 1 << bit_index;
        let word_index = ((self.next_index - 1) * 64) as u32;
        Some(word_index + bit_index)
    }
}

pub fn collect_gpu_culled_meshes(
    mut cameras: Query<(Option<&RenderLayers>, &mut RenderVisibleEntities), With<ExtractedView>>,
    mut lights: Query<(Option<&RenderLayers>, &mut RenderShadowMapVisibleEntities)>,
    mut render_gpu_culled_entities: ResMut<RenderGpuCulledEntities>,
) {

    for (maybe_render_layers, mut render_visible_entities) in &mut cameras {
        collect_gpu_culled_meshes_for_subview(
            maybe_render_layers,
            &mut render_visible_entities,
            &mut render_gpu_culled_entities,
        );
    }

    for (maybe_render_layers, mut render_shadow_map_visible_entities) in &mut lights {
        for render_visible_entities in render_shadow_map_visible_entities.subviews.values_mut() {
            collect_gpu_culled_meshes_for_subview(
                maybe_render_layers,
                render_visible_entities,
                &mut render_gpu_culled_entities,
            );
        }
    }
}

fn collect_gpu_culled_meshes_for_subview(
    maybe_view_render_layers: Option<&RenderLayers>,
    render_visible_entities: &mut RenderVisibleEntities,
    render_mesh_instance_gpu_queues: &mut RenderGpuCulledEntities,
) {

    let render_view_visible_mesh_entities = render_visible_entities
        .classes
        .entry(TypeId::of::<Mesh3d>())
        .or_default();

    for main_entity in &render_mesh_instance_gpu_queues.removed {
        if render_view_visible_mesh_entities
            .entities_gpu_culling
            .remove(main_entity)
            .is_some()
        {
            render_view_visible_mesh_entities
                .removed_entities
                .push((Entity::PLACEHOLDER, *main_entity));
        }
    }

    let mut any_added = false;
    for main_entity in &render_mesh_instance_gpu_queues.added {

        let maybe_entity_render_layers = render_mesh_instance_gpu_queues.entities.get(main_entity);
        if let (Some(view_render_layers), Some(entity_render_layers)) =
            (maybe_view_render_layers, maybe_entity_render_layers)
            && !view_render_layers.intersects(entity_render_layers)
        {
            continue;
        }

        render_view_visible_mesh_entities
            .entities_gpu_culling
            .insert(*main_entity, Entity::PLACEHOLDER);
        render_view_visible_mesh_entities.add_entity((Entity::PLACEHOLDER, *main_entity));
        any_added = true;
    }

    for main_entity in &render_mesh_instance_gpu_queues.changed_layers {
        let Some(new_render_layers) = render_mesh_instance_gpu_queues.entities.get(main_entity)
        else {
            continue;
        };

        let entity_was_visible = render_view_visible_mesh_entities
            .entities_gpu_culling
            .contains_key(main_entity);
        let entity_is_visible = maybe_view_render_layers
            .is_none_or(|render_layers| render_layers.intersects(new_render_layers));
        match (entity_was_visible, entity_is_visible) {
            (false, false) | (true, true) => {

            }
            (false, true) => {

                render_view_visible_mesh_entities
                    .entities_gpu_culling
                    .insert(*main_entity, Entity::PLACEHOLDER);
                render_view_visible_mesh_entities.add_entity((Entity::PLACEHOLDER, *main_entity));
                any_added = true;
            }
            (true, false) => {

                render_view_visible_mesh_entities
                    .entities_gpu_culling
                    .remove(main_entity);
                render_view_visible_mesh_entities
                    .removed_entities
                    .push((Entity::PLACEHOLDER, *main_entity));
            }
        }
    }

    if any_added {
        render_view_visible_mesh_entities.sort_added_entities();
    }
}

pub fn set_mesh_motion_vector_flags(
    mut render_mesh_instances: ResMut<RenderMeshInstances>,
    skin_uniforms: Res<SkinUniforms>,
    morph_indices: Res<MorphIndices>,
) {

    for &entity in skin_uniforms.all_skins() {
        render_mesh_instances
            .insert_mesh_instance_flags(entity, RenderMeshInstanceFlags::HAS_PREVIOUS_SKIN);
    }

    match *morph_indices {
        MorphIndices::Uniform { ref prev, .. } => {
            for &entity in prev.keys() {
                render_mesh_instances.insert_mesh_instance_flags(
                    entity,
                    RenderMeshInstanceFlags::HAS_PREVIOUS_MORPH,
                );
            }
        }
        MorphIndices::Storage {
            morph_weights_info: ref morph_target_info,
            ..
        } => {
            for (entity, morph_target_info) in morph_target_info {
                if morph_target_info.prev_weight_offset.is_some() {
                    render_mesh_instances.insert_mesh_instance_flags(
                        *entity,
                        RenderMeshInstanceFlags::HAS_PREVIOUS_MORPH,
                    );
                }
            }
        }
    }
}

pub fn collect_meshes_for_gpu_building(
    render_mesh_instances: ResMut<RenderMeshInstances>,
    batched_instance_buffers: ResMut<
        gpu_preprocessing::BatchedInstanceBuffers<MeshUniform, MeshInputUniform>,
    >,
    mut mesh_culling_data_buffer: ResMut<MeshCullingDataBuffer>,
    mut render_mesh_instance_queues: ResMut<RenderMeshInstanceGpuQueues>,
    mut render_gpu_culled_entities: ResMut<RenderGpuCulledEntities>,
    mesh_allocator: Res<MeshAllocator>,
    mesh_material_ids: Res<RenderMaterialInstances>,
    render_material_bindings: Res<RenderMaterialBindings>,
    render_lightmaps: Res<RenderLightmaps>,
    skin_uniforms: Res<SkinUniforms>,
    morph_indices: Res<MorphIndices>,
    frame_count: Res<FrameCount>,
    mut meshes_to_reextract_next_frame: ResMut<MeshesToReextractNextFrame>,
) {
    let RenderMeshInstances::GpuBuilding(render_mesh_instances) =
        render_mesh_instances.into_inner()
    else {
        return;
    };

    meshes_to_reextract_next_frame.clear();

    let gpu_preprocessing::BatchedInstanceBuffers {
        current_input_buffer,
        previous_input_buffer,
        ..
    } = batched_instance_buffers.into_inner();

    if !current_input_buffer.is_empty() {
        mesh_culling_data_buffer.grow(current_input_buffer.len() as u32);
    }

    previous_input_buffer.reserve(current_input_buffer.len() as u32);

    render_gpu_culled_entities.prepare_for_new_frame();

    let (prepared_tx, prepared_rx) = mpsc::channel();
    let (reextract_tx, reextract_rx) = mpsc::channel();
    let (removed_tx, removed_rx) = mpsc::channel();

    {

        let mesh_allocator = &mesh_allocator;
        let mesh_material_ids = &mesh_material_ids;
        let render_material_bindings = &render_material_bindings;
        let render_lightmaps = &render_lightmaps;
        let skin_uniforms = &skin_uniforms;
        let frame_count = *frame_count;
        let render_mesh_instances = &*render_mesh_instances;
        let current_input_buffer = &*current_input_buffer;
        let previous_input_buffer = &*previous_input_buffer;
        let mesh_culling_data_buffer = &*mesh_culling_data_buffer;
        let morph_indices = &*morph_indices;

        ComputeTaskPool::get().scope(|scope| {

            for queue in render_mesh_instance_queues.iter_mut() {
                match *queue {
                    RenderMeshInstanceGpuQueue::None => {

                    }

                    RenderMeshInstanceGpuQueue::CpuCulling {
                        ref mut changed,
                        ref mut removed,
                    } => {
                        let prepared_tx = prepared_tx.clone();
                        let reextract_tx = reextract_tx.clone();
                        let removed_tx = removed_tx.clone();
                        scope.spawn(async move {
                            #[cfg(feature = "trace")]
                            let _span = info_span!("prepared_mesh_producer").entered();
                            changed
                                .drain(..)
                                .for_each(|(entity, mesh_instance_builder)| {
                                    match mesh_instance_builder.prepare(
                                        entity,
                                        mesh_allocator,
                                        mesh_material_ids,
                                        render_material_bindings,
                                        render_lightmaps,
                                        skin_uniforms,
                                        morph_indices,
                                        frame_count,
                                    ) {
                                        Some(prepared) => {
                                            prepared_tx.send((entity, prepared, None)).ok();
                                        }
                                        None => {
                                            reextract_tx.send(entity).ok();
                                        }
                                    }
                                });

                            for entity in removed.drain(..) {
                                removed_tx.send(entity).unwrap();
                            }
                        });
                    }

                    RenderMeshInstanceGpuQueue::GpuCulling {
                        ref mut changed_cpu_culling,
                        ref mut changed_gpu_culling,
                        ref mut removed,
                    } => {
                        let prepared_tx = prepared_tx.clone();
                        let reextract_tx = reextract_tx.clone();
                        let removed_tx = removed_tx.clone();
                        scope.spawn(async move {
                            #[cfg(feature = "trace")]
                            let _span = info_span!("prepared_mesh_producer").entered();
                            for (entity, mesh_instance_builder, mesh_culling_builder) in
                                changed_cpu_culling
                                    .drain(..)
                                    .chain(changed_gpu_culling.drain(..))
                            {
                                let Some(mut prepared) = mesh_instance_builder.prepare(
                                    entity,
                                    mesh_allocator,
                                    mesh_material_ids,
                                    render_material_bindings,
                                    render_lightmaps,
                                    skin_uniforms,
                                    morph_indices,
                                    frame_count,
                                ) else {
                                    reextract_tx.send(entity).ok();
                                    continue;
                                };

                                let Some(render_mesh_instance) = render_mesh_instances.get(&entity)
                                else {

                                    let _ = prepared_tx.send((
                                        entity,
                                        prepared,
                                        Some(mesh_culling_builder),
                                    ));
                                    continue;
                                };

                                if prepared.render_layers != render_mesh_instance.render_layers
                                    || prepared
                                        .shared
                                        .flags
                                        .contains(RenderMeshInstanceFlags::NO_CPU_CULLING)
                                        != render_mesh_instance
                                            .shared
                                            .flags()
                                            .contains(RenderMeshInstanceFlags::NO_CPU_CULLING)
                                {

                                    let _ = prepared_tx.send((
                                        entity,
                                        prepared,
                                        Some(mesh_culling_builder),
                                    ));
                                    continue;
                                }

                                prepared.shared.write_to_blob(&render_mesh_instance.shared);

                                let current_uniform_index =
                                    render_mesh_instance.gpu_specific.current_uniform_index();

                                let previous_mesh_input_uniform =
                                    current_input_buffer.get_unchecked(current_uniform_index);
                                let previous_input_index =
                                    previous_input_buffer.push(previous_mesh_input_uniform);
                                prepared.mesh_input_uniform.previous_input_index =
                                    previous_input_index;

                                current_input_buffer
                                    .set(current_uniform_index, prepared.mesh_input_uniform);
                                mesh_culling_data_buffer
                                    .set(current_uniform_index, mesh_culling_builder);
                            }

                            for entity in removed.drain(..) {
                                removed_tx.send(entity).unwrap();
                            }
                        });
                    }
                }
            }

            drop(prepared_tx);
            drop(reextract_tx);
            drop(removed_tx);
        });
    }

    while let Ok(batch) = prepared_rx.recv() {
        let (entity, prepared, mesh_culling_builder) = batch;
        let instance_data_index = prepared.update(
            entity,
            &mut *render_mesh_instances,
            current_input_buffer,
            previous_input_buffer,
        );
        if let Some(mesh_culling_data) = mesh_culling_builder {
            mesh_culling_data.update(&mut mesh_culling_data_buffer, instance_data_index);
        }

        let (render_layers, no_cpu_culling) = match render_mesh_instances.get(&entity) {
            None => (RenderLayers::default(), false),
            Some(render_mesh_instance) => (
                render_mesh_instance
                    .render_layers
                    .clone()
                    .unwrap_or_default(),
                render_mesh_instance
                    .shared
                    .flags()
                    .contains(RenderMeshInstanceFlags::NO_CPU_CULLING),
            ),
        };
        render_gpu_culled_entities.update(entity, render_layers, no_cpu_culling);
    }
    while let Ok(batch) = removed_rx.recv() {
        let entity = batch;
        remove_mesh_input_uniform(entity, &mut *render_mesh_instances, current_input_buffer);
        render_gpu_culled_entities.remove(entity);
    }
    while let Ok(batch) = reextract_rx.recv() {
        let entity = batch;
        meshes_to_reextract_next_frame.insert(entity);
    }

    previous_input_buffer.ensure_nonempty();
}

#[derive(Resource, Clone)]
pub struct MeshPipeline {

    pub view_layouts: MeshPipelineViewLayouts,
    pub clustered_forward_buffer_binding_type: BufferBindingType,
    pub mesh_layouts: MeshLayouts,

    pub shader: Handle<Shader>,

    pub per_object_buffer_batch_size: Option<u32>,

    pub binding_arrays_are_usable: bool,

    pub clustered_decals_are_usable: bool,

    pub skins_use_uniform_buffers: bool,
}

fn init_mesh_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    render_adapter: Res<RenderAdapter>,
    view_layouts: Res<MeshPipelineViewLayouts>,
    asset_server: Res<AssetServer>,
) {
    let shader = load_embedded_asset!(asset_server.as_ref(), "mesh.wgsl");

    let clustered_forward_buffer_binding_type =
        render_device.get_supported_read_only_binding_type(CLUSTERED_FORWARD_STORAGE_BUFFER_COUNT);

    let res = MeshPipeline {
        view_layouts: view_layouts.clone(),
        clustered_forward_buffer_binding_type,
        mesh_layouts: MeshLayouts::new(&render_device, &render_adapter),
        shader,
        per_object_buffer_batch_size: GpuArrayBuffer::<MeshUniform>::batch_size(
            &render_device.limits(),
        ),
        binding_arrays_are_usable: binding_arrays_are_usable(&render_device, &render_adapter),
        clustered_decals_are_usable: decal::clustered::clustered_decals_are_usable(
            &render_device,
            &render_adapter,
        ),
        skins_use_uniform_buffers: skins_use_uniform_buffers(&render_device.limits()),
    };

    commands.insert_resource(res);
}

impl MeshPipeline {
    pub fn get_view_layout(&self, layout_key: MeshPipelineViewLayoutKey) -> MeshPipelineViewLayout {
        self.view_layouts.get_view_layout(layout_key)
    }
}

pub fn build_dummy_white_gpu_image(
    render_device: Res<RenderDevice>,
    default_sampler: Res<DefaultImageSampler>,
    render_queue: Res<RenderQueue>,
) -> GpuImage {
    let image = Image::default();
    let texture = render_device.create_texture(&image.texture_descriptor);
    let sampler = match image.sampler {
        ImageSampler::Default => (**default_sampler).clone(),
        ImageSampler::Descriptor(ref descriptor) => {
            render_device.create_sampler(&descriptor.as_wgpu())
        }
    };

    if let Ok(format_size) = image.texture_descriptor.format.pixel_size() {
        render_queue.write_texture(
            texture.as_image_copy(),
            image.data.as_ref().expect("Image was created without data"),
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width() * format_size as u32),
                rows_per_image: None,
            },
            image.texture_descriptor.size,
        );
    }

    let texture_view = texture.create_view(&TextureViewDescriptor::default());
    GpuImage {
        texture,
        texture_view,
        sampler,
        texture_descriptor: image.texture_descriptor,
        texture_view_descriptor: image.texture_view_descriptor,
        had_data: true,
    }
}

pub fn get_image_texture<'a>(
    dummy_white_gpu_image: &'a GpuImage,
    gpu_images: &'a RenderAssets<GpuImage>,
    handle_option: &Option<Handle<Image>>,
) -> Option<(&'a TextureView, &'a Sampler)> {
    if let Some(handle) = handle_option {
        let gpu_image = gpu_images.get(handle)?;
        Some((&gpu_image.texture_view, &gpu_image.sampler))
    } else {
        Some((
            &dummy_white_gpu_image.texture_view,
            &dummy_white_gpu_image.sampler,
        ))
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct MeshBatchSetCompareData {

    material_bind_group_index: MaterialBindGroupIndex,

    mesh_slabs: MeshSlabs,

    lightmap_slab: Option<NonMaxU32>,
}

impl GetBatchData for MeshPipeline {
    type Param = (
        SRes<RenderMeshInstances>,
        SRes<RenderLightmaps>,
        SRes<RenderAssets<RenderMesh>>,
        SRes<MeshAllocator>,
        SRes<SkinUniforms>,
        SRes<MorphIndices>,
    );
    type BatchSetCompareData = MeshBatchSetCompareData;
    type BatchCompareData = AssetId<Mesh>;

    type BufferData = MeshUniform;

    fn get_batch_data(
        (mesh_instances, lightmaps, _, mesh_allocator, skin_uniforms, morph_indices): &SystemParamItem<
            Self::Param,
        >,
        (_entity, main_entity): (Entity, MainEntity),
    ) -> Option<(
        Self::BufferData,
        Option<(Self::BatchSetCompareData, Self::BatchCompareData)>,
    )> {
        let RenderMeshInstances::CpuBuilding(ref mesh_instances) = **mesh_instances else {
            error!(
                "`get_batch_data` should never be called in GPU mesh uniform \
                building mode"
            );
            return None;
        };
        let mesh_instance = mesh_instances.get(&main_entity)?;
        let first_vertex_index =
            match mesh_allocator.mesh_vertex_slice(&mesh_instance.mesh_asset_id()) {
                Some(mesh_vertex_slice) => mesh_vertex_slice.range.start,
                None => 0,
            };
        let mesh_slabs = mesh_allocator.mesh_slabs(&mesh_instance.mesh_asset_id())?;
        let maybe_lightmap = lightmaps.render_lightmaps.get(&main_entity);

        let current_skin_index = skin_uniforms.skin_index(main_entity);
        let morph_descriptor_index = morph_indices.morph_descriptor_index(main_entity);
        let material_bind_group_index = mesh_instance.material_bindings_index();

        Some((
            MeshUniform::new(
                &mesh_instance.transforms,
                first_vertex_index,
                material_bind_group_index.slot,
                maybe_lightmap.map(|lightmap| (lightmap.slot_index, lightmap.uv_rect)),
                current_skin_index,
                morph_descriptor_index,
                Some(mesh_instance.tag()),
            ),
            mesh_instance.should_batch().then_some((
                MeshBatchSetCompareData {
                    material_bind_group_index: material_bind_group_index.group,
                    mesh_slabs,
                    lightmap_slab: maybe_lightmap.map(|lightmap| lightmap.slab_index.0),
                },
                mesh_instance.mesh_asset_id(),
            )),
        ))
    }
}

impl GetFullBatchData for MeshPipeline {
    type BufferInputData = MeshInputUniform;

    fn get_index_and_compare_data(
        (mesh_instances, lightmaps, _, mesh_allocator, _, _): &SystemParamItem<Self::Param>,
        main_entity: MainEntity,
    ) -> Option<(
        NonMaxU32,
        Option<(Self::BatchSetCompareData, Self::BatchCompareData)>,
    )> {

        let RenderMeshInstances::GpuBuilding(ref mesh_instances) = **mesh_instances else {
            error!(
                "`get_index_and_compare_data` should never be called in CPU mesh uniform building \
                mode"
            );
            return None;
        };

        let mesh_instance = mesh_instances.get(&main_entity)?;
        let mesh_slabs = mesh_allocator.mesh_slabs(&mesh_instance.mesh_asset_id())?;
        let maybe_lightmap = lightmaps.render_lightmaps.get(&main_entity);
        let material_bind_group_index = mesh_instance.material_bindings_index();

        Some((
            NonMaxU32::new(mesh_instance.gpu_specific.current_uniform_index())?,
            mesh_instance.should_batch().then_some((
                MeshBatchSetCompareData {
                    material_bind_group_index: material_bind_group_index.group,
                    mesh_slabs,
                    lightmap_slab: maybe_lightmap.map(|lightmap| lightmap.slab_index.0),
                },
                mesh_instance.mesh_asset_id(),
            )),
        ))
    }

    fn get_binned_batch_data(
        (mesh_instances, lightmaps, _, mesh_allocator, skin_uniforms, morph_indices): &SystemParamItem<
            Self::Param,
        >,
        main_entity: MainEntity,
    ) -> Option<Self::BufferData> {
        let RenderMeshInstances::CpuBuilding(ref mesh_instances) = **mesh_instances else {
            error!(
                "`get_binned_batch_data` should never be called in GPU mesh uniform building mode"
            );
            return None;
        };
        let mesh_instance = mesh_instances.get(&main_entity)?;
        let first_vertex_index =
            match mesh_allocator.mesh_vertex_slice(&mesh_instance.mesh_asset_id()) {
                Some(mesh_vertex_slice) => mesh_vertex_slice.range.start,
                None => 0,
            };
        let maybe_lightmap = lightmaps.render_lightmaps.get(&main_entity);

        let current_skin_index = skin_uniforms.skin_index(main_entity);
        let morph_descriptor_index = morph_indices.morph_descriptor_index(main_entity);

        Some(MeshUniform::new(
            &mesh_instance.transforms,
            first_vertex_index,
            mesh_instance.material_bindings_index().slot,
            maybe_lightmap.map(|lightmap| (lightmap.slot_index, lightmap.uv_rect)),
            current_skin_index,
            morph_descriptor_index,
            Some(mesh_instance.tag()),
        ))
    }

    fn get_binned_index(
        (mesh_instances, _, _, _, _, _): &SystemParamItem<Self::Param>,
        main_entity: MainEntity,
    ) -> Option<NonMaxU32> {

        let RenderMeshInstances::GpuBuilding(ref mesh_instances) = **mesh_instances else {
            error!(
                "`get_binned_index` should never be called in CPU mesh uniform \
                building mode"
            );
            return None;
        };

        mesh_instances
            .get(&main_entity)
            .and_then(|entity| NonMaxU32::new(entity.gpu_specific.current_uniform_index()))
    }

    fn write_batch_indirect_parameters_metadata(
        indexed: bool,
        base_output_index: u32,
        batch_set_index: Option<NonMaxU32>,
        phase_indirect_parameters_buffers: &mut UntypedPhaseIndirectParametersBuffers,
        indirect_parameters_offset: u32,
    ) {
        let indirect_parameters = IndirectParametersCpuMetadata {
            base_output_index,
            batch_set_index: match batch_set_index {
                Some(batch_set_index) => u32::from(batch_set_index),
                None => !0,
            },
        };

        if indexed {
            phase_indirect_parameters_buffers
                .indexed
                .set(indirect_parameters_offset, indirect_parameters);
        } else {
            phase_indirect_parameters_buffers
                .non_indexed
                .set(indirect_parameters_offset, indirect_parameters);
        }
    }
}

bitflags::bitflags! {
    #[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]

    pub struct MeshPipelineKey: u64 {

        const NONE                              = 0;

        const MORPH_TARGETS                     = BaseMeshPipelineKey::MORPH_TARGETS.bits();

        const TONEMAP_IN_SHADER                 = 1 << 0;
        const DEBAND_DITHER                     = 1 << 1;
        const DEPTH_PREPASS                     = 1 << 2;
        const NORMAL_PREPASS                    = 1 << 3;
        const DEFERRED_PREPASS                  = 1 << 4;
        const MOTION_VECTOR_PREPASS             = 1 << 5;
        const MAY_DISCARD                       = 1 << 6;

        const ENVIRONMENT_MAP                   = 1 << 7;
        const SCREEN_SPACE_AMBIENT_OCCLUSION    = 1 << 8;
        const UNCLIPPED_DEPTH_ORTHO             = 1 << 9;

        const TEMPORAL_JITTER                   = 1 << 10;
        const READS_VIEW_TRANSMISSION_TEXTURE   = 1 << 11;
        const LIGHTMAPPED                       = 1 << 12;
        const LIGHTMAP_BICUBIC_SAMPLING         = 1 << 13;
        const IRRADIANCE_VOLUME                 = 1 << 14;
        const VISIBILITY_RANGE_DITHER           = 1 << 15;
        const SCREEN_SPACE_REFLECTIONS          = 1 << 16;
        const HAS_PREVIOUS_SKIN                 = 1 << 17;
        const HAS_PREVIOUS_MORPH                = 1 << 18;
        const OIT_ENABLED                       = 1 << 19;
        const DISTANCE_FOG                      = 1 << 20;
        const ATMOSPHERE                        = 1 << 21;
        const INVERT_CULLING                    = 1 << 22;
        const PREPASS_READS_MATERIAL            = 1 << 23;
        const CONTACT_SHADOWS                   = 1 << 24;
        const LAST_FLAG                         = Self::CONTACT_SHADOWS.bits();

        const ALL_PREPASS_BITS                  = Self::DEPTH_PREPASS.bits()
                                                | Self::NORMAL_PREPASS.bits()
                                                | Self::DEFERRED_PREPASS.bits()
                                                | Self::MOTION_VECTOR_PREPASS.bits()
                                                | Self::MAY_DISCARD.bits()
                                                | Self::PREPASS_READS_MATERIAL.bits();

        const MSAA_RESERVED_BITS                = Self::MSAA_MASK_BITS << Self::MSAA_SHIFT_BITS;
        const BLEND_RESERVED_BITS               = Self::BLEND_MASK_BITS << Self::BLEND_SHIFT_BITS;
        const BLEND_OPAQUE                      = 0 << Self::BLEND_SHIFT_BITS;
        const BLEND_PREMULTIPLIED_ALPHA         = 1 << Self::BLEND_SHIFT_BITS;
        const BLEND_MULTIPLY                    = 2 << Self::BLEND_SHIFT_BITS;
        const BLEND_ALPHA                       = 3 << Self::BLEND_SHIFT_BITS;
        const BLEND_ALPHA_TO_COVERAGE           = 4 << Self::BLEND_SHIFT_BITS;
        const TONEMAP_METHOD_RESERVED_BITS      = Self::TONEMAP_METHOD_MASK_BITS << Self::TONEMAP_METHOD_SHIFT_BITS;
        const TONEMAP_METHOD_NONE               = 0 << Self::TONEMAP_METHOD_SHIFT_BITS;
        const TONEMAP_METHOD_REINHARD           = 1 << Self::TONEMAP_METHOD_SHIFT_BITS;
        const TONEMAP_METHOD_REINHARD_LUMINANCE = 2 << Self::TONEMAP_METHOD_SHIFT_BITS;
        const TONEMAP_METHOD_ACES_FITTED        = 3 << Self::TONEMAP_METHOD_SHIFT_BITS;
        const TONEMAP_METHOD_AGX                = 4 << Self::TONEMAP_METHOD_SHIFT_BITS;
        const TONEMAP_METHOD_SOMEWHAT_BORING_DISPLAY_TRANSFORM = 5 << Self::TONEMAP_METHOD_SHIFT_BITS;
        const TONEMAP_METHOD_TONY_MC_MAPFACE    = 6 << Self::TONEMAP_METHOD_SHIFT_BITS;
        const TONEMAP_METHOD_BLENDER_FILMIC     = 7 << Self::TONEMAP_METHOD_SHIFT_BITS;
        const TONEMAP_METHOD_PBR_NEUTRAL        = 8 << Self::TONEMAP_METHOD_SHIFT_BITS;
        const SHADOW_FILTER_METHOD_RESERVED_BITS = Self::SHADOW_FILTER_METHOD_MASK_BITS << Self::SHADOW_FILTER_METHOD_SHIFT_BITS;
        const SHADOW_FILTER_METHOD_HARDWARE_2X2  = 0 << Self::SHADOW_FILTER_METHOD_SHIFT_BITS;
        const SHADOW_FILTER_METHOD_GAUSSIAN      = 1 << Self::SHADOW_FILTER_METHOD_SHIFT_BITS;
        const SHADOW_FILTER_METHOD_TEMPORAL      = 2 << Self::SHADOW_FILTER_METHOD_SHIFT_BITS;
        const VIEW_PROJECTION_RESERVED_BITS     = Self::VIEW_PROJECTION_MASK_BITS << Self::VIEW_PROJECTION_SHIFT_BITS;
        const VIEW_PROJECTION_NONSTANDARD       = 0 << Self::VIEW_PROJECTION_SHIFT_BITS;
        const VIEW_PROJECTION_PERSPECTIVE       = 1 << Self::VIEW_PROJECTION_SHIFT_BITS;
        const VIEW_PROJECTION_ORTHOGRAPHIC      = 2 << Self::VIEW_PROJECTION_SHIFT_BITS;
        const VIEW_PROJECTION_RESERVED          = 3 << Self::VIEW_PROJECTION_SHIFT_BITS;
        const SCREEN_SPACE_SPECULAR_TRANSMISSION_RESERVED_BITS = Self::SCREEN_SPACE_SPECULAR_TRANSMISSION_MASK_BITS << Self::SCREEN_SPACE_SPECULAR_TRANSMISSION_SHIFT_BITS;
        const SCREEN_SPACE_SPECULAR_TRANSMISSION_LOW    = 0 << Self::SCREEN_SPACE_SPECULAR_TRANSMISSION_SHIFT_BITS;
        const SCREEN_SPACE_SPECULAR_TRANSMISSION_MEDIUM = 1 << Self::SCREEN_SPACE_SPECULAR_TRANSMISSION_SHIFT_BITS;
        const SCREEN_SPACE_SPECULAR_TRANSMISSION_HIGH   = 2 << Self::SCREEN_SPACE_SPECULAR_TRANSMISSION_SHIFT_BITS;
        const SCREEN_SPACE_SPECULAR_TRANSMISSION_ULTRA  = 3 << Self::SCREEN_SPACE_SPECULAR_TRANSMISSION_SHIFT_BITS;
        const COLOR_TARGET_FORMAT_RESERVED_BITS = Self::COLOR_TARGET_FORMAT_MASK_BITS
            << Self::COLOR_TARGET_FORMAT_SHIFT_BITS;
        const ALL_RESERVED_BITS =
            Self::BLEND_RESERVED_BITS.bits() |
            Self::MSAA_RESERVED_BITS.bits() |
            Self::TONEMAP_METHOD_RESERVED_BITS.bits() |
            Self::SHADOW_FILTER_METHOD_RESERVED_BITS.bits() |
            Self::VIEW_PROJECTION_RESERVED_BITS.bits() |
            Self::SCREEN_SPACE_SPECULAR_TRANSMISSION_RESERVED_BITS.bits() |
            Self::COLOR_TARGET_FORMAT_RESERVED_BITS.bits();
    }
}

impl MeshPipelineKey {
    const MSAA_MASK_BITS: u64 = 0b111;
    const MSAA_SHIFT_BITS: u64 = Self::LAST_FLAG.bits().trailing_zeros() as u64 + 1;

    const BLEND_MASK_BITS: u64 = 0b111;
    const BLEND_SHIFT_BITS: u64 = Self::MSAA_MASK_BITS.count_ones() as u64 + Self::MSAA_SHIFT_BITS;

    const TONEMAP_METHOD_MASK_BITS: u64 = 0b1111;
    const TONEMAP_METHOD_SHIFT_BITS: u64 =
        Self::BLEND_MASK_BITS.count_ones() as u64 + Self::BLEND_SHIFT_BITS;

    const SHADOW_FILTER_METHOD_MASK_BITS: u64 = 0b11;
    const SHADOW_FILTER_METHOD_SHIFT_BITS: u64 =
        Self::TONEMAP_METHOD_MASK_BITS.count_ones() as u64 + Self::TONEMAP_METHOD_SHIFT_BITS;

    const VIEW_PROJECTION_MASK_BITS: u64 = 0b11;
    const VIEW_PROJECTION_SHIFT_BITS: u64 = Self::SHADOW_FILTER_METHOD_MASK_BITS.count_ones()
        as u64
        + Self::SHADOW_FILTER_METHOD_SHIFT_BITS;

    const SCREEN_SPACE_SPECULAR_TRANSMISSION_MASK_BITS: u64 = 0b11;
    const SCREEN_SPACE_SPECULAR_TRANSMISSION_SHIFT_BITS: u64 =
        Self::VIEW_PROJECTION_MASK_BITS.count_ones() as u64 + Self::VIEW_PROJECTION_SHIFT_BITS;

    const COLOR_TARGET_FORMAT_MASK_BITS: u64 = view::COLOR_TARGET_FORMAT_MASK_BITS as u64;
    const COLOR_TARGET_FORMAT_SHIFT_BITS: u64 = Self::SCREEN_SPACE_SPECULAR_TRANSMISSION_MASK_BITS
        .count_ones() as u64
        + Self::SCREEN_SPACE_SPECULAR_TRANSMISSION_SHIFT_BITS;

    pub fn from_msaa_samples(msaa_samples: u32) -> Self {
        let msaa_bits =
            (msaa_samples.trailing_zeros() as u64 & Self::MSAA_MASK_BITS) << Self::MSAA_SHIFT_BITS;
        Self::from_bits_retain(msaa_bits)
    }

    #[inline]
    pub fn from_target_format(format: TextureFormat) -> Self {
        let code = texture_format_to_code(format)
            .expect("Texture format is not supported by the pipeline") as u64;
        Self::from_bits_retain(
            (code & Self::COLOR_TARGET_FORMAT_MASK_BITS) << Self::COLOR_TARGET_FORMAT_SHIFT_BITS,
        )
    }

    #[inline]
    pub fn target_format(&self) -> TextureFormat {
        let code = ((self.bits() >> Self::COLOR_TARGET_FORMAT_SHIFT_BITS)
            & Self::COLOR_TARGET_FORMAT_MASK_BITS) as u8;
        texture_format_from_code(code)
            .expect("Unknown bits in `COLOR_TARGET_FORMAT_MASK_BITS` of the pipeline key")
    }

    pub fn msaa_samples(&self) -> u32 {
        1 << ((self.bits() >> Self::MSAA_SHIFT_BITS) & Self::MSAA_MASK_BITS)
    }

    pub fn from_primitive_topology_and_strip_index(
        primitive_topology: PrimitiveTopology,
        strip_index_format: Option<IndexFormat>,
    ) -> Self {
        let index_bits = if primitive_topology.is_strip() {
            match strip_index_format {
                None => BaseMeshPipelineKey::STRIP_INDEX_FORMAT_NONE,
                Some(indices) => match indices {
                    IndexFormat::Uint16 => BaseMeshPipelineKey::STRIP_INDEX_FORMAT_U16,
                    IndexFormat::Uint32 => BaseMeshPipelineKey::STRIP_INDEX_FORMAT_U32,
                },
            }
        } else {
            BaseMeshPipelineKey::STRIP_INDEX_FORMAT_NONE
        }
        .bits();
        let primitive_topology_bits = ((primitive_topology as u64)
            & BaseMeshPipelineKey::PRIMITIVE_TOPOLOGY_MASK_BITS)
            << BaseMeshPipelineKey::PRIMITIVE_TOPOLOGY_SHIFT_BITS;
        Self::from_bits_retain(primitive_topology_bits | index_bits)
    }

    pub fn primitive_topology(&self) -> PrimitiveTopology {
        let primitive_topology_bits = (self.bits()
            >> BaseMeshPipelineKey::PRIMITIVE_TOPOLOGY_SHIFT_BITS)
            & BaseMeshPipelineKey::PRIMITIVE_TOPOLOGY_MASK_BITS;
        match primitive_topology_bits {
            x if x == PrimitiveTopology::PointList as u64 => PrimitiveTopology::PointList,
            x if x == PrimitiveTopology::LineList as u64 => PrimitiveTopology::LineList,
            x if x == PrimitiveTopology::LineStrip as u64 => PrimitiveTopology::LineStrip,
            x if x == PrimitiveTopology::TriangleList as u64 => PrimitiveTopology::TriangleList,
            x if x == PrimitiveTopology::TriangleStrip as u64 => PrimitiveTopology::TriangleStrip,
            _ => PrimitiveTopology::default(),
        }
    }

    pub fn strip_index_format(&self) -> Option<IndexFormat> {
        let index_bits = self.bits() & BaseMeshPipelineKey::STRIP_INDEX_FORMAT_RESERVED_BITS.bits();
        match index_bits {
            x if x == BaseMeshPipelineKey::STRIP_INDEX_FORMAT_U16.bits() => {
                Some(IndexFormat::Uint16)
            }
            x if x == BaseMeshPipelineKey::STRIP_INDEX_FORMAT_U32.bits() => {
                Some(IndexFormat::Uint32)
            }
            x if x == BaseMeshPipelineKey::STRIP_INDEX_FORMAT_NONE.bits() => None,
            _ => unreachable!(),
        }
    }
}

impl From<u64> for MeshPipelineKey {
    fn from(value: u64) -> Self {
        MeshPipelineKey::from_bits_retain(value)
    }
}

impl From<MeshPipelineKey> for u64 {
    fn from(value: MeshPipelineKey) -> Self {
        value.bits()
    }
}

const_assert_eq!(
    (((MeshPipelineKey::LAST_FLAG.bits() << 1) - 1) | MeshPipelineKey::ALL_RESERVED_BITS.bits())
        & BaseMeshPipelineKey::all().bits(),
    0
);

const_assert_eq!(
    BaseMeshPipelineKey::all().bits() & MeshPipelineKey::all().bits(),
    MeshPipelineKey::MORPH_TARGETS.bits()
);

fn is_skinned(layout: &MeshVertexBufferLayoutRef) -> bool {
    layout.0.contains(Mesh::ATTRIBUTE_JOINT_INDEX)
        && layout.0.contains(Mesh::ATTRIBUTE_JOINT_WEIGHT)
}
pub fn setup_morph_and_skinning_defs(
    mesh_layouts: &MeshLayouts,
    layout: &MeshVertexBufferLayoutRef,
    offset: u32,
    key: &MeshPipelineKey,
    shader_defs: &mut Vec<ShaderDefVal>,
    vertex_attributes: &mut Vec<VertexAttributeDescriptor>,
    skins_use_uniform_buffers: bool,
) -> BindGroupLayoutDescriptor {
    let is_morphed = key.intersects(MeshPipelineKey::MORPH_TARGETS);
    let is_lightmapped = key.intersects(MeshPipelineKey::LIGHTMAPPED);
    let motion_vector_prepass = key.intersects(MeshPipelineKey::MOTION_VECTOR_PREPASS);

    if skins_use_uniform_buffers {
        shader_defs.push("SKINS_USE_UNIFORM_BUFFERS".into());
    }

    let mut add_skin_data = || {
        shader_defs.push("SKINNED".into());
        vertex_attributes.push(Mesh::ATTRIBUTE_JOINT_INDEX.at_shader_location(offset));
        vertex_attributes.push(Mesh::ATTRIBUTE_JOINT_WEIGHT.at_shader_location(offset + 1));
    };

    match (
        is_skinned(layout),
        is_morphed,
        is_lightmapped,
        motion_vector_prepass,
    ) {
        (true, false, _, true) => {
            add_skin_data();
            mesh_layouts.skinned_motion.clone()
        }
        (true, false, _, false) => {
            add_skin_data();
            mesh_layouts.skinned.clone()
        }
        (true, true, _, true) => {
            add_skin_data();
            shader_defs.push("MORPH_TARGETS".into());
            mesh_layouts.morphed_skinned_motion.clone()
        }
        (true, true, _, false) => {
            add_skin_data();
            shader_defs.push("MORPH_TARGETS".into());
            mesh_layouts.morphed_skinned.clone()
        }
        (false, true, _, true) => {
            shader_defs.push("MORPH_TARGETS".into());
            mesh_layouts.morphed_motion.clone()
        }
        (false, true, _, false) => {
            shader_defs.push("MORPH_TARGETS".into());
            mesh_layouts.morphed.clone()
        }
        (false, false, true, _) => mesh_layouts.lightmapped.clone(),
        (false, false, false, _) => mesh_layouts.model_only.clone(),
    }
}

impl SpecializedMeshPipeline for MeshPipeline {
    type Key = MeshPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut shader_defs = Vec::new();
        let mut vertex_attributes = Vec::new();

        shader_defs.push("MESH_PIPELINE".into());

        shader_defs.push("VERTEX_OUTPUT_INSTANCE_INDEX".into());

        if layout.0.contains(Mesh::ATTRIBUTE_POSITION) {
            shader_defs.push("VERTEX_POSITIONS".into());
            vertex_attributes.push(Mesh::ATTRIBUTE_POSITION.at_shader_location(0));
        }

        if layout.0.contains(Mesh::ATTRIBUTE_NORMAL) {
            shader_defs.push("VERTEX_NORMALS".into());
            vertex_attributes.push(Mesh::ATTRIBUTE_NORMAL.at_shader_location(1));
        }

        if layout.0.contains(Mesh::ATTRIBUTE_UV_0) {
            shader_defs.push("VERTEX_UVS".into());
            shader_defs.push("VERTEX_UVS_A".into());
            vertex_attributes.push(Mesh::ATTRIBUTE_UV_0.at_shader_location(2));
        }

        if layout.0.contains(Mesh::ATTRIBUTE_UV_1) {
            shader_defs.push("VERTEX_UVS".into());
            shader_defs.push("VERTEX_UVS_B".into());
            vertex_attributes.push(Mesh::ATTRIBUTE_UV_1.at_shader_location(3));
        }

        if layout.0.contains(Mesh::ATTRIBUTE_TANGENT) {
            shader_defs.push("VERTEX_TANGENTS".into());
            vertex_attributes.push(Mesh::ATTRIBUTE_TANGENT.at_shader_location(4));
        }

        if layout.0.contains(Mesh::ATTRIBUTE_COLOR) {
            shader_defs.push("VERTEX_COLORS".into());
            vertex_attributes.push(Mesh::ATTRIBUTE_COLOR.at_shader_location(5));
        }

        if cfg!(feature = "pbr_transmission_textures") {
            shader_defs.push("PBR_TRANSMISSION_TEXTURES_SUPPORTED".into());
        }
        if cfg!(feature = "pbr_multi_layer_material_textures") {
            shader_defs.push("PBR_MULTI_LAYER_MATERIAL_TEXTURES_SUPPORTED".into());
        }
        if cfg!(feature = "pbr_anisotropy_texture") {
            shader_defs.push("PBR_ANISOTROPY_TEXTURE_SUPPORTED".into());
        }
        if cfg!(feature = "pbr_specular_textures") {
            shader_defs.push("PBR_SPECULAR_TEXTURES_SUPPORTED".into());
        }
        if cfg!(feature = "bluenoise_texture") {
            shader_defs.push("BLUE_NOISE_TEXTURE".into());
        }
        if cfg!(feature = "dfg_lut") {
            shader_defs.push("DFG_LUT".into());
        }
        if cfg!(feature = "area_light_luts") {
            shader_defs.push("AREA_LIGHT_LUTS".into());
        }

        let bind_group_layout = self.get_view_layout(key.into());
        let mut bind_group_layout = vec![
            bind_group_layout.main_layout.clone(),
            bind_group_layout.binding_array_layout.clone(),
        ];

        if key.msaa_samples() > 1 {
            shader_defs.push("MULTISAMPLED".into());
        };

        bind_group_layout.push(setup_morph_and_skinning_defs(
            &self.mesh_layouts,
            layout,
            6,
            &key,
            &mut shader_defs,
            &mut vertex_attributes,
            self.skins_use_uniform_buffers,
        ));

        if key.contains(MeshPipelineKey::SCREEN_SPACE_AMBIENT_OCCLUSION) {
            shader_defs.push("SCREEN_SPACE_AMBIENT_OCCLUSION".into());
        }

        if key.contains(MeshPipelineKey::CONTACT_SHADOWS) {
            shader_defs.push("CONTACT_SHADOWS".into());
        }

        let vertex_buffer_layout = layout.0.get_layout(&vertex_attributes)?;

        let (label, blend, depth_write_enabled);
        let pass = key.intersection(MeshPipelineKey::BLEND_RESERVED_BITS);
        let (mut is_opaque, mut alpha_to_coverage_enabled) = (false, false);
        if key.contains(MeshPipelineKey::OIT_ENABLED) && pass == MeshPipelineKey::BLEND_ALPHA {
            label = "oit_mesh_pipeline".into();

            blend = None;
            shader_defs.push("OIT_ENABLED".into());

            depth_write_enabled = false;
        } else if pass == MeshPipelineKey::BLEND_ALPHA {
            label = "alpha_blend_mesh_pipeline".into();
            blend = Some(BlendState::ALPHA_BLENDING);

            depth_write_enabled = false;
        } else if pass == MeshPipelineKey::BLEND_PREMULTIPLIED_ALPHA {
            label = "premultiplied_alpha_mesh_pipeline".into();
            blend = Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING);
            shader_defs.push("PREMULTIPLY_ALPHA".into());
            shader_defs.push("BLEND_PREMULTIPLIED_ALPHA".into());

            depth_write_enabled = false;
        } else if pass == MeshPipelineKey::BLEND_MULTIPLY {
            label = "multiply_mesh_pipeline".into();
            blend = Some(BlendState {
                color: BlendComponent {
                    src_factor: BlendFactor::Dst,
                    dst_factor: BlendFactor::OneMinusSrcAlpha,
                    operation: BlendOperation::Add,
                },
                alpha: BlendComponent::OVER,
            });
            shader_defs.push("PREMULTIPLY_ALPHA".into());
            shader_defs.push("BLEND_MULTIPLY".into());

            depth_write_enabled = false;
        } else if pass == MeshPipelineKey::BLEND_ALPHA_TO_COVERAGE {
            label = "alpha_to_coverage_mesh_pipeline".into();

            blend = None;

            depth_write_enabled = true;
            is_opaque = !key.contains(MeshPipelineKey::READS_VIEW_TRANSMISSION_TEXTURE);
            alpha_to_coverage_enabled = true;
            shader_defs.push("ALPHA_TO_COVERAGE".into());
        } else {
            label = "opaque_mesh_pipeline".into();

            blend = None;

            depth_write_enabled = true;
            is_opaque = !key.contains(MeshPipelineKey::READS_VIEW_TRANSMISSION_TEXTURE);
        }

        if key.contains(MeshPipelineKey::NORMAL_PREPASS) {
            shader_defs.push("NORMAL_PREPASS".into());
        }

        if key.contains(MeshPipelineKey::DEPTH_PREPASS) {
            shader_defs.push("DEPTH_PREPASS".into());
        }

        if key.contains(MeshPipelineKey::MOTION_VECTOR_PREPASS) {
            shader_defs.push("MOTION_VECTOR_PREPASS".into());
        }

        if key.contains(MeshPipelineKey::HAS_PREVIOUS_SKIN) {
            shader_defs.push("HAS_PREVIOUS_SKIN".into());
        }

        if key.contains(MeshPipelineKey::HAS_PREVIOUS_MORPH) {
            shader_defs.push("HAS_PREVIOUS_MORPH".into());
        }

        if key.contains(MeshPipelineKey::DEFERRED_PREPASS) {
            shader_defs.push("DEFERRED_PREPASS".into());
        }

        if key.contains(MeshPipelineKey::NORMAL_PREPASS) && key.msaa_samples() == 1 && is_opaque {
            shader_defs.push("LOAD_PREPASS_NORMALS".into());
        }

        let view_projection = key.intersection(MeshPipelineKey::VIEW_PROJECTION_RESERVED_BITS);
        if view_projection == MeshPipelineKey::VIEW_PROJECTION_NONSTANDARD {
            shader_defs.push("VIEW_PROJECTION_NONSTANDARD".into());
        } else if view_projection == MeshPipelineKey::VIEW_PROJECTION_PERSPECTIVE {
            shader_defs.push("VIEW_PROJECTION_PERSPECTIVE".into());
        } else if view_projection == MeshPipelineKey::VIEW_PROJECTION_ORTHOGRAPHIC {
            shader_defs.push("VIEW_PROJECTION_ORTHOGRAPHIC".into());
        }

        #[cfg(all(feature = "webgl", target_arch = "wasm32", not(feature = "webgpu")))]
        shader_defs.push("WEBGL2".into());

        #[cfg(feature = "experimental_pbr_pcss")]
        shader_defs.push("PCSS_SAMPLERS_AVAILABLE".into());

        if key.contains(MeshPipelineKey::TONEMAP_IN_SHADER) {
            shader_defs.push("TONEMAP_IN_SHADER".into());
            shader_defs.push(ShaderDefVal::UInt(
                "TONEMAPPING_LUT_TEXTURE_BINDING_INDEX".into(),
                TONEMAPPING_LUT_TEXTURE_BINDING_INDEX,
            ));
            shader_defs.push(ShaderDefVal::UInt(
                "TONEMAPPING_LUT_SAMPLER_BINDING_INDEX".into(),
                TONEMAPPING_LUT_SAMPLER_BINDING_INDEX,
            ));

            let method = key.intersection(MeshPipelineKey::TONEMAP_METHOD_RESERVED_BITS);

            if method == MeshPipelineKey::TONEMAP_METHOD_NONE {
                shader_defs.push("TONEMAP_METHOD_NONE".into());
            } else if method == MeshPipelineKey::TONEMAP_METHOD_REINHARD {
                shader_defs.push("TONEMAP_METHOD_REINHARD".into());
            } else if method == MeshPipelineKey::TONEMAP_METHOD_REINHARD_LUMINANCE {
                shader_defs.push("TONEMAP_METHOD_REINHARD_LUMINANCE".into());
            } else if method == MeshPipelineKey::TONEMAP_METHOD_ACES_FITTED {
                shader_defs.push("TONEMAP_METHOD_ACES_FITTED".into());
            } else if method == MeshPipelineKey::TONEMAP_METHOD_AGX {
                shader_defs.push("TONEMAP_METHOD_AGX".into());
            } else if method == MeshPipelineKey::TONEMAP_METHOD_SOMEWHAT_BORING_DISPLAY_TRANSFORM {
                shader_defs.push("TONEMAP_METHOD_SOMEWHAT_BORING_DISPLAY_TRANSFORM".into());
            } else if method == MeshPipelineKey::TONEMAP_METHOD_BLENDER_FILMIC {
                shader_defs.push("TONEMAP_METHOD_BLENDER_FILMIC".into());
            } else if method == MeshPipelineKey::TONEMAP_METHOD_TONY_MC_MAPFACE {
                shader_defs.push("TONEMAP_METHOD_TONY_MC_MAPFACE".into());
            } else if method == MeshPipelineKey::TONEMAP_METHOD_PBR_NEUTRAL {
                shader_defs.push("TONEMAP_METHOD_PBR_NEUTRAL".into());
            }

            if key.contains(MeshPipelineKey::DEBAND_DITHER) {
                shader_defs.push("DEBAND_DITHER".into());
            }
        }

        if key.contains(MeshPipelineKey::MAY_DISCARD) {
            shader_defs.push("MAY_DISCARD".into());
        }

        if key.contains(MeshPipelineKey::ENVIRONMENT_MAP) {
            shader_defs.push("ENVIRONMENT_MAP".into());
        }

        if key.contains(MeshPipelineKey::IRRADIANCE_VOLUME) && IRRADIANCE_VOLUMES_ARE_USABLE {
            shader_defs.push("IRRADIANCE_VOLUME".into());
        }

        if key.contains(MeshPipelineKey::LIGHTMAPPED) {
            shader_defs.push("LIGHTMAP".into());
        }
        if key.contains(MeshPipelineKey::LIGHTMAP_BICUBIC_SAMPLING) {
            shader_defs.push("LIGHTMAP_BICUBIC_SAMPLING".into());
        }

        if key.contains(MeshPipelineKey::TEMPORAL_JITTER) {
            shader_defs.push("TEMPORAL_JITTER".into());
        }

        let shadow_filter_method =
            key.intersection(MeshPipelineKey::SHADOW_FILTER_METHOD_RESERVED_BITS);
        if shadow_filter_method == MeshPipelineKey::SHADOW_FILTER_METHOD_HARDWARE_2X2 {
            shader_defs.push("SHADOW_FILTER_METHOD_HARDWARE_2X2".into());
        } else if shadow_filter_method == MeshPipelineKey::SHADOW_FILTER_METHOD_GAUSSIAN {
            shader_defs.push("SHADOW_FILTER_METHOD_GAUSSIAN".into());
        } else if shadow_filter_method == MeshPipelineKey::SHADOW_FILTER_METHOD_TEMPORAL {
            shader_defs.push("SHADOW_FILTER_METHOD_TEMPORAL".into());
        }

        let blur_quality =
            key.intersection(MeshPipelineKey::SCREEN_SPACE_SPECULAR_TRANSMISSION_RESERVED_BITS);

        shader_defs.push(ShaderDefVal::Int(
            "SCREEN_SPACE_SPECULAR_TRANSMISSION_BLUR_TAPS".into(),
            match blur_quality {
                MeshPipelineKey::SCREEN_SPACE_SPECULAR_TRANSMISSION_LOW => 4,
                MeshPipelineKey::SCREEN_SPACE_SPECULAR_TRANSMISSION_MEDIUM => 8,
                MeshPipelineKey::SCREEN_SPACE_SPECULAR_TRANSMISSION_HIGH => 16,
                MeshPipelineKey::SCREEN_SPACE_SPECULAR_TRANSMISSION_ULTRA => 32,
                _ => unreachable!(),
            },
        ));

        if key.contains(MeshPipelineKey::VISIBILITY_RANGE_DITHER) {
            shader_defs.push("VISIBILITY_RANGE_DITHER".into());
        }

        if key.contains(MeshPipelineKey::DISTANCE_FOG) {
            shader_defs.push("DISTANCE_FOG".into());
        }

        if key.contains(MeshPipelineKey::ATMOSPHERE) {
            shader_defs.push("ATMOSPHERE".into());
        }

        if self.binding_arrays_are_usable {
            shader_defs.push("MULTIPLE_LIGHT_PROBES_IN_ARRAY".into());
            shader_defs.push("MULTIPLE_LIGHTMAPS_IN_ARRAY".into());
        }

        if IRRADIANCE_VOLUMES_ARE_USABLE {
            shader_defs.push("IRRADIANCE_VOLUMES_ARE_USABLE".into());
        }

        if self.clustered_decals_are_usable {
            shader_defs.push("CLUSTERED_DECALS_ARE_USABLE".into());
            if cfg!(feature = "pbr_light_textures") {
                shader_defs.push("LIGHT_TEXTURES".into());
            }
        }

        let format = key.target_format();

        if let Some(per_object_buffer_batch_size) = self.per_object_buffer_batch_size {
            shader_defs.push(ShaderDefVal::UInt(
                "PER_OBJECT_BUFFER_BATCH_SIZE".into(),
                per_object_buffer_batch_size,
            ));
        }

        Ok(RenderPipelineDescriptor {
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: shader_defs.clone(),
                buffers: vec![vertex_buffer_layout],
                ..default()
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs,
                targets: vec![Some(ColorTargetState {
                    format,
                    blend,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            layout: bind_group_layout,
            primitive: PrimitiveState {
                cull_mode: Some(Face::Back),
                unclipped_depth: false,
                topology: key.primitive_topology(),
                strip_index_format: key.strip_index_format(),
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                depth_write_enabled: Some(depth_write_enabled),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: StencilState {
                    front: StencilFaceState::IGNORE,
                    back: StencilFaceState::IGNORE,
                    read_mask: 0,
                    write_mask: 0,
                },
                bias: DepthBiasState {
                    constant: 0,
                    slope_scale: 0.0,
                    clamp: 0.0,
                },
            }),
            multisample: MultisampleState {
                count: key.msaa_samples(),
                mask: !0,
                alpha_to_coverage_enabled,
            },
            label: Some(label),
            ..default()
        })
    }
}

pub struct MeshPhaseBindGroups {
    model_only: Option<BindGroup>,
    skinned: Option<MeshBindGroupPair>,

    morph_targets: MeshMorphTargetBindGroups,
    lightmaps: HashMap<LightmapSlabIndex, BindGroup>,
}

pub enum MeshMorphTargetBindGroups {

    Uniform(HashMap<AssetId<Mesh>, MeshBindGroupPair>),

    Storage(HashMap<MeshSlabId, MeshMorphTargetStorageBindGroups>),
}

pub struct MeshMorphTargetStorageBindGroups {

    skinned: Option<MeshBindGroupPair>,

    unskinned: Option<MeshBindGroupPair>,
}

impl MeshMorphTargetBindGroups {

    fn clear(&mut self) {
        match *self {
            MeshMorphTargetBindGroups::Uniform(ref mut mesh_to_bind_group_pair) => {
                mesh_to_bind_group_pair.clear();
            }
            MeshMorphTargetBindGroups::Storage(ref mut slab_id_to_bind_groups) => {
                slab_id_to_bind_groups.clear();
            }
        }
    }
}

pub struct MeshBindGroupPair {
    motion_vectors: BindGroup,
    no_motion_vectors: BindGroup,
}

#[derive(Resource)]
pub enum MeshBindGroups {

    CpuPreprocessing(MeshPhaseBindGroups),

    GpuPreprocessing(TypeIdMap<MeshPhaseBindGroups>),
}

impl MeshPhaseBindGroups {

    pub fn new(render_device: &RenderDevice) -> MeshPhaseBindGroups {
        MeshPhaseBindGroups {
            model_only: None,
            skinned: None,
            morph_targets: if skins_use_uniform_buffers(&render_device.limits()) {
                MeshMorphTargetBindGroups::Uniform(HashMap::default())
            } else {
                MeshMorphTargetBindGroups::Storage(HashMap::default())
            },
            lightmaps: HashMap::default(),
        }
    }

    pub fn reset(&mut self) {
        self.model_only = None;
        self.skinned = None;
        self.morph_targets.clear();
        self.lightmaps.clear();
    }

    pub fn get(
        &self,
        lightmap: Option<LightmapSlabIndex>,
        is_skinned: bool,
        morph: MeshMorphBindGroupKey,
        motion_vectors: bool,
    ) -> Option<&BindGroup> {
        match (is_skinned, morph, lightmap) {
            (_, MeshMorphBindGroupKey::Uniform(asset_id), _) => match self.morph_targets {
                MeshMorphTargetBindGroups::Uniform(ref morph_targets) => morph_targets
                    .get(&asset_id)
                    .map(|bind_group_pair| bind_group_pair.get(motion_vectors)),
                MeshMorphTargetBindGroups::Storage(..) => {
                    error!(
                        "Tried to look up a mesh morph target bind group using a mesh ID, but \
                        we're using storage buffers. Look it up using the slab ID instead."
                    );
                    None
                }
            },
            (_, MeshMorphBindGroupKey::Storage(slab_id), _) => match &self.morph_targets {
                MeshMorphTargetBindGroups::Uniform(..) => {
                    error!(
                        "Tried to look up a mesh morph target bind group using a slab ID, but \
                        we're using uniform buffers. Look it up using the mesh ID instead."
                    );
                    None
                }
                MeshMorphTargetBindGroups::Storage(slab_to_bind_group) => {
                    let slab_bind_group = slab_to_bind_group.get(&slab_id)?;
                    if is_skinned {
                        slab_bind_group
                            .skinned
                            .as_ref()
                            .map(|bind_group_pair| bind_group_pair.get(motion_vectors))
                    } else {
                        slab_bind_group
                            .unskinned
                            .as_ref()
                            .map(|bind_group_pair| bind_group_pair.get(motion_vectors))
                    }
                }
            },
            (true, MeshMorphBindGroupKey::NoMorphTargets, _) => self
                .skinned
                .as_ref()
                .map(|bind_group_pair| bind_group_pair.get(motion_vectors)),
            (false, MeshMorphBindGroupKey::NoMorphTargets, Some(lightmap_slab)) => {
                self.lightmaps.get(&lightmap_slab)
            }
            (false, MeshMorphBindGroupKey::NoMorphTargets, None) => self.model_only.as_ref(),
        }
    }
}

impl MeshBindGroupPair {
    fn get(&self, motion_vectors: bool) -> &BindGroup {
        if motion_vectors {
            &self.motion_vectors
        } else {
            &self.no_motion_vectors
        }
    }
}

#[derive(Clone, Copy)]
pub enum MeshMorphBindGroupKey {

    NoMorphTargets,

    Uniform(AssetId<Mesh>),

    Storage(MeshSlabId),
}

pub fn prepare_mesh_bind_groups(
    mut commands: Commands,
    meshes: Res<RenderAssets<RenderMesh>>,
    mesh_pipeline: Res<MeshPipeline>,
    render_device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    cpu_batched_instance_buffer: Option<
        Res<no_gpu_preprocessing::BatchedInstanceBuffer<MeshUniform>>,
    >,
    gpu_batched_instance_buffers: Option<
        Res<gpu_preprocessing::BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>,
    >,
    skins_uniform: Res<SkinUniforms>,
    weights_uniform: Res<MorphUniforms>,
    mesh_allocator: Res<MeshAllocator>,
    render_morph_target_allocator: Res<RenderMorphTargetAllocator>,
    mut render_lightmaps: ResMut<RenderLightmaps>,
) {

    if let Some(cpu_batched_instance_buffer) = cpu_batched_instance_buffer
        && let Some(instance_data_binding) = cpu_batched_instance_buffer
            .into_inner()
            .instance_data_binding()
    {

        let cpu_preprocessing_mesh_bind_groups = prepare_mesh_bind_groups_for_phase(
            instance_data_binding,
            &meshes,
            &mesh_pipeline,
            &render_device,
            &pipeline_cache,
            &skins_uniform,
            &weights_uniform,
            &mesh_allocator,
            &render_morph_target_allocator,
            &mut render_lightmaps,
        );

        commands.insert_resource(MeshBindGroups::CpuPreprocessing(
            cpu_preprocessing_mesh_bind_groups,
        ));
        return;
    }

    if let Some(gpu_batched_instance_buffers) = gpu_batched_instance_buffers {
        let mut gpu_preprocessing_mesh_bind_groups = TypeIdMap::default();

        for (phase_type_id, batched_phase_instance_buffers) in
            &gpu_batched_instance_buffers.phase_instance_buffers
        {
            let Some(instance_data_binding) =
                batched_phase_instance_buffers.instance_data_binding()
            else {
                continue;
            };

            let mesh_phase_bind_groups = prepare_mesh_bind_groups_for_phase(
                instance_data_binding,
                &meshes,
                &mesh_pipeline,
                &render_device,
                &pipeline_cache,
                &skins_uniform,
                &weights_uniform,
                &mesh_allocator,
                &render_morph_target_allocator,
                &mut render_lightmaps,
            );

            gpu_preprocessing_mesh_bind_groups.insert(*phase_type_id, mesh_phase_bind_groups);
        }

        commands.insert_resource(MeshBindGroups::GpuPreprocessing(
            gpu_preprocessing_mesh_bind_groups,
        ));
    }
}

fn prepare_mesh_bind_groups_for_phase(
    model: BindingResource,
    meshes: &RenderAssets<RenderMesh>,
    mesh_pipeline: &MeshPipeline,
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    skins_uniform: &SkinUniforms,
    weights_uniform: &MorphUniforms,
    mesh_allocator: &MeshAllocator,
    render_morph_target_allocator: &RenderMorphTargetAllocator,
    render_lightmaps: &mut RenderLightmaps,
) -> MeshPhaseBindGroups {
    let layouts = &mesh_pipeline.mesh_layouts;

    let mut groups = MeshPhaseBindGroups {
        model_only: Some(layouts.model_only(render_device, pipeline_cache, &model)),
        ..MeshPhaseBindGroups::new(render_device)
    };

    let (skin, prev_skin) = (&skins_uniform.current_buffer, &skins_uniform.prev_buffer);
    groups.skinned = Some(MeshBindGroupPair {
        motion_vectors: layouts.skinned_motion(
            render_device,
            pipeline_cache,
            &model,
            skin,
            prev_skin,
        ),
        no_motion_vectors: layouts.skinned(render_device, pipeline_cache, &model, skin),
    });

    if weights_uniform.current_buffer.buffer().is_some() {
        match (render_morph_target_allocator, &mut groups.morph_targets) {
            (
                RenderMorphTargetAllocator::Image { mesh_id_to_image },
                &mut MeshMorphTargetBindGroups::Uniform(ref mut morph_targets),
            ) => {
                prepare_mesh_morph_target_bind_groups_for_phase_using_uniforms(
                    &model,
                    meshes,
                    layouts,
                    render_device,
                    pipeline_cache,
                    skins_uniform,
                    weights_uniform,
                    mesh_id_to_image,
                    morph_targets,
                );
            }

            (
                &RenderMorphTargetAllocator::Storage,
                &mut MeshMorphTargetBindGroups::Storage(ref mut morph_target_storage_bind_groups),
            ) => {
                prepare_mesh_morph_target_bind_groups_for_phase_using_storage(
                    &model,
                    layouts,
                    render_device,
                    pipeline_cache,
                    skins_uniform,
                    weights_uniform,
                    mesh_allocator,
                    morph_target_storage_bind_groups,
                );
            }

            _ => {
                error!(
                    "Mismatched render morph target allocator and mesh morph target bind groups"
                );
            }
        }
    }

    let bindless_supported = render_lightmaps.bindless_supported;
    for (lightmap_slab_id, lightmap_slab) in render_lightmaps.slabs.iter_mut().enumerate() {
        groups.lightmaps.insert(
            LightmapSlabIndex(NonMaxU32::new(lightmap_slab_id as u32).unwrap()),
            layouts.lightmapped(
                render_device,
                pipeline_cache,
                &model,
                lightmap_slab,
                bindless_supported,
            ),
        );
    }

    groups
}

fn prepare_mesh_morph_target_bind_groups_for_phase_using_uniforms(
    model: &BindingResource,
    meshes: &RenderAssets<RenderMesh>,
    layouts: &MeshLayouts,
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    skins_uniform: &SkinUniforms,
    weights_uniform: &MorphUniforms,
    mesh_id_to_image: &HashMap<AssetId<Mesh>, MorphTargetImage>,
    morph_targets: &mut HashMap<AssetId<Mesh>, MeshBindGroupPair>,
) {
    let (skin, prev_skin) = (&skins_uniform.current_buffer, &skins_uniform.prev_buffer);
    let weights = weights_uniform
        .current_buffer
        .buffer()
        .expect("Should have a weights buffer by now");
    let prev_weights = weights_uniform.prev_buffer.buffer().unwrap_or(weights);
    let maybe_morph_descriptors = weights_uniform
        .descriptors_buffer
        .as_ref()
        .and_then(|descriptors_buffer| descriptors_buffer.buffer());

    for (id, gpu_mesh) in meshes.iter() {
        if !gpu_mesh.has_morph_targets() {
            continue;
        }
        let Some(morph_targets_image) = mesh_id_to_image.get(&id) else {
            continue;
        };
        let targets = MorphTargetsResource::Texture(&morph_targets_image.texture_view);
        let bind_group_pair = if is_skinned(&gpu_mesh.layout) {
            MeshBindGroupPair {
                motion_vectors: layouts.morphed_skinned_motion(
                    render_device,
                    pipeline_cache,
                    model,
                    skin,
                    weights,
                    targets,
                    prev_skin,
                    prev_weights,
                    maybe_morph_descriptors,
                ),
                no_motion_vectors: layouts.morphed_skinned(
                    render_device,
                    pipeline_cache,
                    model,
                    skin,
                    weights,
                    targets,
                    maybe_morph_descriptors,
                ),
            }
        } else {
            MeshBindGroupPair {
                motion_vectors: layouts.morphed_motion(
                    render_device,
                    pipeline_cache,
                    model,
                    weights,
                    prev_weights,
                    targets,
                    maybe_morph_descriptors,
                ),
                no_motion_vectors: layouts.morphed(
                    render_device,
                    pipeline_cache,
                    model,
                    weights,
                    targets,
                    maybe_morph_descriptors,
                ),
            }
        };
        morph_targets.insert(id, bind_group_pair);
    }
}

fn prepare_mesh_morph_target_bind_groups_for_phase_using_storage(
    model: &BindingResource,
    layouts: &MeshLayouts,
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    skins_uniform: &SkinUniforms,
    weights_uniform: &MorphUniforms,
    mesh_allocator: &MeshAllocator,
    morph_target_storage_bind_groups: &mut HashMap<MeshSlabId, MeshMorphTargetStorageBindGroups>,
) {
    let (skin, prev_skin) = (&skins_uniform.current_buffer, &skins_uniform.prev_buffer);
    let weights = weights_uniform
        .current_buffer
        .buffer()
        .expect("Should have a weights buffer by now");
    let prev_weights = weights_uniform.prev_buffer.buffer().unwrap_or(weights);
    let maybe_morph_descriptors = weights_uniform
        .descriptors_buffer
        .as_ref()
        .and_then(|descriptors_buffer| descriptors_buffer.buffer());

    for morph_target_slab_id in mesh_allocator.morph_target_slabs() {
        let Some(buffer) = mesh_allocator.buffer_for_slab(morph_target_slab_id) else {
            continue;
        };
        let targets = MorphTargetsResource::Storage(buffer);
        morph_target_storage_bind_groups.insert(
            morph_target_slab_id,
            MeshMorphTargetStorageBindGroups {
                skinned: Some(MeshBindGroupPair {
                    motion_vectors: layouts.morphed_skinned_motion(
                        render_device,
                        pipeline_cache,
                        model,
                        skin,
                        weights,
                        targets,
                        prev_skin,
                        prev_weights,
                        maybe_morph_descriptors,
                    ),
                    no_motion_vectors: layouts.morphed_skinned(
                        render_device,
                        pipeline_cache,
                        model,
                        skin,
                        weights,
                        targets,
                        maybe_morph_descriptors,
                    ),
                }),
                unskinned: Some(MeshBindGroupPair {
                    motion_vectors: layouts.morphed_motion(
                        render_device,
                        pipeline_cache,
                        model,
                        weights,
                        prev_weights,
                        targets,
                        maybe_morph_descriptors,
                    ),
                    no_motion_vectors: layouts.morphed(
                        render_device,
                        pipeline_cache,
                        model,
                        weights,
                        targets,
                        maybe_morph_descriptors,
                    ),
                }),
            },
        );
    }
}

pub struct SetMeshViewBindGroup<const I: usize>;
impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetMeshViewBindGroup<I> {
    type Param = ();
    type ViewQuery = (Read<MeshViewBindGroup>,);
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        _item: &P,
        (mesh_view_bind_group,): ROQueryItem<'w, '_, Self::ViewQuery>,
        _entity: Option<()>,
        _: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        pass.set_bind_group(
            I,
            &mesh_view_bind_group.main,
            &mesh_view_bind_group.main_offsets,
        );

        RenderCommandResult::Success
    }
}

pub struct SetMeshViewBindingArrayBindGroup<const I: usize>;
impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetMeshViewBindingArrayBindGroup<I> {
    type Param = ();
    type ViewQuery = (Read<MeshViewBindGroup>,);
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        _item: &P,
        (mesh_view_bind_group,): ROQueryItem<'w, '_, Self::ViewQuery>,
        _entity: Option<()>,
        _: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        pass.set_bind_group(I, &mesh_view_bind_group.binding_array, &[]);

        RenderCommandResult::Success
    }
}

pub struct SetMeshViewEmptyBindGroup<const I: usize>;
impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetMeshViewEmptyBindGroup<I> {
    type Param = ();
    type ViewQuery = (Read<MeshViewBindGroup>,);
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        _item: &P,
        (mesh_view_bind_group,): ROQueryItem<'w, '_, Self::ViewQuery>,
        _entity: Option<()>,
        _: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        pass.set_bind_group(I, &mesh_view_bind_group.empty, &[]);

        RenderCommandResult::Success
    }
}

pub struct SetMeshBindGroup<const I: usize>;
impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetMeshBindGroup<I> {
    type Param = (
        SRes<RenderDevice>,
        SRes<MeshBindGroups>,
        SRes<RenderMeshInstances>,
        SRes<SkinUniforms>,
        SRes<MorphIndices>,
        SRes<MeshAllocator>,
        SRes<RenderLightmaps>,
    );
    type ViewQuery = Has<MotionVectorPrepass>;
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        item: &P,
        has_motion_vector_prepass: bool,
        _item_query: Option<()>,
        (
            render_device,
            bind_groups,
            mesh_instances,
            skin_uniforms,
            morph_indices,
            mesh_allocator,
            lightmaps,
        ): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let bind_groups = bind_groups.into_inner();
        let mesh_instances = mesh_instances.into_inner();
        let skin_uniforms = skin_uniforms.into_inner();
        let morph_indices = morph_indices.into_inner();

        let entity = &item.main_entity();

        let Some(mesh_asset_id) = mesh_instances.mesh_asset_id(*entity) else {
            return RenderCommandResult::Success;
        };

        let skins_use_uniform_buffers = skins_use_uniform_buffers(&render_device.limits());

        let current_skin_byte_offset = skin_uniforms.skin_byte_offset(*entity);

        let (current_morph_index, prev_morph_index, morph_bind_group_key);
        match *morph_indices {
            MorphIndices::Uniform {
                ref current,
                ref prev,
            } => {
                current_morph_index = current.get(entity);
                prev_morph_index = prev.get(entity);
                morph_bind_group_key = if current_morph_index.is_some() {
                    MeshMorphBindGroupKey::Uniform(mesh_asset_id)
                } else {
                    MeshMorphBindGroupKey::NoMorphTargets
                };
            }
            MorphIndices::Storage { .. } => {
                current_morph_index = None;
                prev_morph_index = None;
                morph_bind_group_key = match mesh_allocator
                    .mesh_slabs(&mesh_asset_id)
                    .and_then(|mesh_slabs| mesh_slabs.morph_target_slab_id)
                {
                    Some(morph_target_slab_id) => {
                        MeshMorphBindGroupKey::Storage(morph_target_slab_id)
                    }
                    None => MeshMorphBindGroupKey::NoMorphTargets,
                };
            }
        };

        let is_skinned = current_skin_byte_offset.is_some();

        let lightmap_slab_index = lightmaps
            .render_lightmaps
            .get(entity)
            .map(|render_lightmap| render_lightmap.slab_index);

        let Some(mesh_phase_bind_groups) = (match *bind_groups {
            MeshBindGroups::CpuPreprocessing(ref mesh_phase_bind_groups) => {
                Some(mesh_phase_bind_groups)
            }
            MeshBindGroups::GpuPreprocessing(ref mesh_phase_bind_groups) => {
                mesh_phase_bind_groups.get(&TypeId::of::<P>())
            }
        }) else {

            return RenderCommandResult::Success;
        };

        let Some(bind_group) = mesh_phase_bind_groups.get(
            lightmap_slab_index,
            is_skinned,
            morph_bind_group_key,
            has_motion_vector_prepass,
        ) else {
            return RenderCommandResult::Failure(
                "The MeshBindGroups resource wasn't set in the render phase. \
                It should be set by the prepare_mesh_bind_group system.\n\
                This is a bevy bug! Please open an issue.",
            );
        };

        let mut dynamic_offsets: [u32; 5] = Default::default();
        let mut offset_count = 0;
        if let PhaseItemExtraIndex::DynamicOffset(dynamic_offset) = item.extra_index() {
            dynamic_offsets[offset_count] = dynamic_offset;
            offset_count += 1;
        }
        if skins_use_uniform_buffers {
            if let Some(current_skin_index) = current_skin_byte_offset {
                dynamic_offsets[offset_count] = current_skin_index.byte_offset;
                offset_count += 1;
            }
            if let Some(current_morph_index) = current_morph_index {
                dynamic_offsets[offset_count] = current_morph_index.index;
                offset_count += 1;
            }
        }

        if skins_use_uniform_buffers && has_motion_vector_prepass {

            if let Some(current_skin_byte_offset) = current_skin_byte_offset {
                dynamic_offsets[offset_count] = current_skin_byte_offset.byte_offset;
                offset_count += 1;
            }

            if current_morph_index.is_some() {
                match prev_morph_index {
                    Some(prev_morph_index) => {
                        dynamic_offsets[offset_count] = prev_morph_index.index;
                    }
                    None => dynamic_offsets[offset_count] = 0,
                }
                offset_count += 1;
            }
        }

        pass.set_bind_group(I, bind_group, &dynamic_offsets[0..offset_count]);

        RenderCommandResult::Success
    }
}

pub struct DrawMesh;
impl<P: PhaseItem> RenderCommand<P> for DrawMesh {
    type Param = (
        SRes<RenderAssets<RenderMesh>>,
        SRes<RenderMeshInstances>,
        SRes<IndirectParametersBuffers>,
        SRes<PipelineCache>,
        SRes<MeshAllocator>,
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
            meshes,
            mesh_instances,
            indirect_parameters_buffer,
            pipeline_cache,
            mesh_allocator,
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

        let meshes = meshes.into_inner();
        let mesh_instances = mesh_instances.into_inner();
        let indirect_parameters_buffer = indirect_parameters_buffer.into_inner();
        let mesh_allocator = mesh_allocator.into_inner();

        let Some(mesh_asset_id) = mesh_instances.mesh_asset_id(item.main_entity()) else {
            return RenderCommandResult::Skip;
        };
        let Some(gpu_mesh) = meshes.get(mesh_asset_id) else {
            return RenderCommandResult::Skip;
        };
        let Some(vertex_buffer_slice) = mesh_allocator.mesh_vertex_slice(&mesh_asset_id) else {
            return RenderCommandResult::Skip;
        };

        pass.set_vertex_buffer(0, vertex_buffer_slice.buffer.slice(..));

        let batch_range = item.batch_range();

        match &gpu_mesh.buffer_info {
            RenderMeshBufferInfo::Indexed {
                index_format,
                count,
            } => {
                let Some(index_buffer_slice) = mesh_allocator.mesh_index_slice(&mesh_asset_id)
                else {
                    return RenderCommandResult::Skip;
                };

                pass.set_index_buffer(index_buffer_slice.buffer.slice(..), *index_format);

                match item.extra_index() {
                    PhaseItemExtraIndex::None | PhaseItemExtraIndex::DynamicOffset(_) => {
                        pass.draw_indexed(
                            index_buffer_slice.range.start
                                ..(index_buffer_slice.range.start + *count),
                            vertex_buffer_slice.range.start as i32,
                            batch_range.clone(),
                        );
                    }
                    PhaseItemExtraIndex::IndirectParametersIndex {
                        range: indirect_parameters_range,
                        batch_set_index,
                    } => {

                        let Some(phase_indirect_parameters_buffers) =
                            indirect_parameters_buffer.get(&TypeId::of::<P>())
                        else {
                            warn!(
                                "Not rendering mesh because indexed indirect parameters buffer \
                                 wasn't present for this phase",
                            );
                            return RenderCommandResult::Skip;
                        };
                        let (Some(indirect_parameters_buffer), Some(batch_sets_buffer)) = (
                            phase_indirect_parameters_buffers.indexed.data_buffer(),
                            phase_indirect_parameters_buffers
                                .indexed
                                .batch_sets_buffer(),
                        ) else {
                            warn!(
                                "Not rendering mesh because indexed indirect parameters buffer \
                                 wasn't present",
                            );
                            return RenderCommandResult::Skip;
                        };

                        let indirect_parameters_offset = indirect_parameters_range.start as u64
                            * size_of::<IndirectParametersIndexed>() as u64;
                        let indirect_parameters_count =
                            indirect_parameters_range.end - indirect_parameters_range.start;

                        match batch_set_index {
                            Some(batch_set_index) => {
                                let count_offset = u32::from(batch_set_index)
                                    * (size_of::<IndirectBatchSet>() as u32);
                                pass.multi_draw_indexed_indirect_count(
                                    indirect_parameters_buffer,
                                    indirect_parameters_offset,
                                    batch_sets_buffer,
                                    count_offset as u64,
                                    indirect_parameters_count,
                                );
                            }
                            None => {
                                pass.multi_draw_indexed_indirect(
                                    indirect_parameters_buffer,
                                    indirect_parameters_offset,
                                    indirect_parameters_count,
                                );
                            }
                        }
                    }
                }
            }

            RenderMeshBufferInfo::NonIndexed => match item.extra_index() {
                PhaseItemExtraIndex::None | PhaseItemExtraIndex::DynamicOffset(_) => {
                    pass.draw(vertex_buffer_slice.range, batch_range.clone());
                }
                PhaseItemExtraIndex::IndirectParametersIndex {
                    range: indirect_parameters_range,
                    batch_set_index,
                } => {

                    let Some(phase_indirect_parameters_buffers) =
                        indirect_parameters_buffer.get(&TypeId::of::<P>())
                    else {
                        warn!(
                            "Not rendering mesh because non-indexed indirect parameters buffer \
                                 wasn't present for this phase",
                        );
                        return RenderCommandResult::Skip;
                    };
                    let (Some(indirect_parameters_buffer), Some(batch_sets_buffer)) = (
                        phase_indirect_parameters_buffers.non_indexed.data_buffer(),
                        phase_indirect_parameters_buffers
                            .non_indexed
                            .batch_sets_buffer(),
                    ) else {
                        warn!(
                            "Not rendering mesh because non-indexed indirect parameters buffer \
                             wasn't present"
                        );
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
                                indirect_parameters_buffer,
                                indirect_parameters_offset,
                                batch_sets_buffer,
                                count_offset as u64,
                                indirect_parameters_count,
                            );
                        }
                        None => {
                            pass.multi_draw_indirect(
                                indirect_parameters_buffer,
                                indirect_parameters_offset,
                                indirect_parameters_count,
                            );
                        }
                    }
                }
            },
        }
        RenderCommandResult::Success
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU64, Ordering};

    use super::{AtomicU64ZeroBitIter, MeshPipelineKey};

    #[test]
    fn mesh_key_msaa_samples() {
        for i in [1, 2, 4, 8, 16, 32, 64, 128] {
            assert_eq!(MeshPipelineKey::from_msaa_samples(i).msaa_samples(), i);
        }
    }

    #[test]
    fn atomic_u64_zero_bit_iter() {

        static TEST_DATA_0: [AtomicU64; 1] = [AtomicU64::new(0x7b52f5ca63498b6a)];
        static TEST_DATA_1: [AtomicU64; 2] = [
            AtomicU64::new(0x5705b33451b95827),
            AtomicU64::new(0x1939ee614074abad),
        ];
        static TEST_DATA_2: [AtomicU64; 3] = [
            AtomicU64::new(0xf33c508d14d145c),
            AtomicU64::new(0x2a4749823594ea),
            AtomicU64::new(0x5e68df04196a3818),
        ];
        static TEST_DATA_3: [AtomicU64; 6] = [
            AtomicU64::new(0x2336dbda2bd74d09),
            AtomicU64::new(0x10b7da1dacf22d33),
            AtomicU64::new(0x6eaaf908d957923a),
            AtomicU64::new(0x7ec7ffb64cb9c4a6),
            AtomicU64::new(0x6dd027da8ad22fa0),
            AtomicU64::new(0x13278c5caa74f73d),
        ];
        static TEST_DATA_4: [AtomicU64; 4] = [
            AtomicU64::new(0x32c66f0334bb09e9),
            AtomicU64::new(0x60815770d307bdcd),
            AtomicU64::new(0x6270a1e972fb8469),
            AtomicU64::new(0x610e995c042d6df4),
        ];
        static TEST_DATA_5: [AtomicU64; 3] = [
            AtomicU64::new(0x7fe756fc690097eb),
            AtomicU64::new(0x15d87ce6679b1bd8),
            AtomicU64::new(0x1985ea515135b255),
        ];
        static TEST_DATA_6: [AtomicU64; 4] = [
            AtomicU64::new(0x1afb9d361c135827),
            AtomicU64::new(0x4a79ad582628a854),
            AtomicU64::new(0x57a802160315c974),
            AtomicU64::new(0x1c0aef068db1f6fb),
        ];
        static TEST_DATA_7: [AtomicU64; 3] = [
            AtomicU64::new(0x53d10eb77230c696),
            AtomicU64::new(0x2ca2d709994855fb),
            AtomicU64::new(0x26536a13f647f2f7),
        ];
        static TEST_DATA_8: [AtomicU64; 2] = [
            AtomicU64::new(0x6ed95eb903155e00),
            AtomicU64::new(0x5e5d58eec92cba0),
        ];
        static TEST_DATA_9: [AtomicU64; 1] = [AtomicU64::new(0x2caa4b77512b1664)];

        let test_data = [
            &TEST_DATA_0[..],
            &TEST_DATA_1[..],
            &TEST_DATA_2[..],
            &TEST_DATA_3[..],
            &TEST_DATA_4[..],
            &TEST_DATA_5[..],
            &TEST_DATA_6[..],
            &TEST_DATA_7[..],
            &TEST_DATA_8[..],
            &TEST_DATA_9[..],
        ];

        for bits in &test_data {

            let mut reference_zero_positions = vec![];
            for (word_index, word) in (*bits).iter().enumerate() {
                let word = word.load(Ordering::Relaxed);
                for bit_index in 0..64 {
                    if ((word >> bit_index) & 1) == 0 {
                        reference_zero_positions.push((word_index * 64 + bit_index) as u32);
                    }
                }
            }

            let test_zero_positions: Vec<_> = AtomicU64ZeroBitIter::new(bits).collect();

            assert_eq!(test_zero_positions, reference_zero_positions);
        }
    }
}
