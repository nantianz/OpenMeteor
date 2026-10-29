use crate::material_bind_groups::{
    FallbackBindlessResources, MaterialBindGroupAllocator, MaterialBindingId,
};
use crate::*;
use alloc::sync::Arc;
use bevy_asset::prelude::AssetChanged;
use bevy_asset::{Asset, AssetEventSystems, AssetId, AssetServer, UntypedAssetId};
use bevy_camera::visibility::ViewVisibility;
use bevy_core_pipeline::core_3d::TransparentSortingInfo3d;
use bevy_core_pipeline::deferred::{AlphaMask3dDeferred, Opaque3dDeferred};
use bevy_core_pipeline::prepass::{AlphaMask3dPrepass, Opaque3dPrepass};
use bevy_core_pipeline::{
    core_3d::{AlphaMask3d, Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey, Transparent3d},
    prepass::{OpaqueNoLightmap3dBatchSetKey, OpaqueNoLightmap3dBinKey},
    tonemapping::Tonemapping,
};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::change_detection::Tick;
use bevy_ecs::system::SystemParam;
use bevy_ecs::{
    prelude::*,
    system::{
        lifetimeless::{SRes, SResMut},
        SystemParamItem, SystemState,
    },
};
use bevy_material::{
    key::{ErasedMaterialKey, ErasedMaterialPipelineKey, ErasedMeshPipelineKey},
    labels::{DrawFunctionLabel, InternedShaderLabel, ShaderLabel},
    MaterialProperties, OpaqueRendererMethod, RenderPhaseType,
};
use bevy_math::{Affine3, Affine3Ext as _};
use bevy_mesh::{
    mark_3d_meshes_as_changed_if_their_assets_changed, Mesh3d, MeshVertexBufferLayoutRef,
};
use bevy_platform::collections::hash_map::Entry;
use bevy_platform::collections::{HashMap, HashSet};
use bevy_platform::hash::FixedHasher;
use bevy_reflect::std_traits::ReflectDefault;
use bevy_reflect::Reflect;
use bevy_render::batching::gpu_preprocessing::BatchedInstanceBuffers;
use bevy_render::camera::{DirtySpecializationSystems, DirtySpecializations, PendingQueues};
use bevy_render::erased_render_asset::{
    ErasedRenderAsset, ErasedRenderAssetPlugin, ErasedRenderAssets, PrepareAssetError,
};
use bevy_render::render_asset::{prepare_assets, RenderAssets};
use bevy_render::renderer::RenderQueue;
use bevy_render::GpuResourceAppExt;
use bevy_render::RenderStartup;
use bevy_render::{
    batching::gpu_preprocessing::GpuPreprocessingSupport,
    extract_resource::ExtractResource,
    mesh::RenderMesh,
    prelude::*,
    render_phase::*,
    render_resource::*,
    renderer::RenderDevice,
    sync_world::MainEntity,
    view::{ExtractedView, Msaa, RenderVisibilityRanges, RetainedViewEntity},
    Extract,
};
use bevy_render::{mesh::allocator::MeshAllocator, sync_world::MainEntityHashMap};
use bevy_render::{texture::FallbackImage, view::RenderVisibleEntities};
use bevy_shader::ShaderDefVal;
use bevy_utils::Parallel;
use core::{
    any::{Any, TypeId},
    hash::Hash,
    marker::PhantomData,
};
use smallvec::SmallVec;
use tracing::error;

pub const MATERIAL_BIND_GROUP_INDEX: usize = 3;

pub trait Material: Asset + AsBindGroup + Clone + Sized {

    fn vertex_shader() -> ShaderRef {
        ShaderRef::Default
    }

    fn fragment_shader() -> ShaderRef {
        ShaderRef::Default
    }

    #[inline]
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Opaque
    }

    #[inline]
    fn opaque_render_method(&self) -> OpaqueRendererMethod {
        OpaqueRendererMethod::Forward
    }

    #[inline]

    fn depth_bias(&self) -> f32 {
        0.0
    }

    #[inline]

    fn reads_view_transmission_texture(&self) -> bool {
        false
    }

    #[inline]
    fn enable_prepass() -> bool {
        true
    }

    #[inline]
    fn enable_shadows() -> bool {
        true
    }

    fn prepass_vertex_shader() -> ShaderRef {
        ShaderRef::Default
    }

    fn prepass_fragment_shader() -> ShaderRef {
        ShaderRef::Default
    }

    fn deferred_vertex_shader() -> ShaderRef {
        ShaderRef::Default
    }

    fn deferred_fragment_shader() -> ShaderRef {
        ShaderRef::Default
    }

    #[cfg(feature = "meshlet")]
    fn meshlet_mesh_fragment_shader() -> ShaderRef {
        ShaderRef::Default
    }

    #[cfg(feature = "meshlet")]
    fn meshlet_mesh_prepass_fragment_shader() -> ShaderRef {
        ShaderRef::Default
    }

    #[cfg(feature = "meshlet")]
    fn meshlet_mesh_deferred_fragment_shader() -> ShaderRef {
        ShaderRef::Default
    }

    #[expect(
        unused_variables,
        reason = "The parameters here are intentionally unused by the default implementation; however, putting underscores here will result in the underscores being copied by rust-analyzer's tab completion."
    )]
    #[inline]
    fn specialize(
        pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        Ok(())
    }
}

#[derive(Default)]
pub struct MaterialsPlugin {

    pub debug_flags: RenderDebugFlags,
}

impl Plugin for MaterialsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((PrepassPipelinePlugin, PrepassPlugin::new(self.debug_flags)));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_gpu_resource::<SpecializedMaterialPipelineCache>()
                .init_gpu_resource::<SpecializedMeshPipelines<MaterialPipelineSpecializer>>()
                .init_gpu_resource::<LightKeyCache>()
                .init_gpu_resource::<SpecializedShadowMaterialPipelineCache>()
                .init_resource::<DrawFunctions<Shadow>>()
                .init_resource::<RenderMaterialInstances>()
                .allow_ambiguous_resource::<RenderMaterialInstances>()
                .init_resource::<MaterialBindGroupAllocators>()
                .allow_ambiguous_resource::<MaterialBindGroupAllocators>()
                .init_gpu_resource::<PendingMeshMaterialQueues>()
                .allow_ambiguous_resource::<PendingMeshMaterialQueues>()
                .init_gpu_resource::<PendingShadowQueues>()
                .allow_ambiguous_resource::<PendingShadowQueues>()
                .add_render_command::<Shadow, DrawPrepass>()
                .add_render_command::<Shadow, DrawDepthOnlyPrepass>()
                .add_render_command::<Transparent3d, DrawMaterial>()
                .add_render_command::<Opaque3d, DrawMaterial>()
                .add_render_command::<AlphaMask3d, DrawMaterial>()
                .add_systems(
                    RenderStartup,
                    init_material_pipeline.after(MeshPipelineSystems),
                )
                .add_systems(
                    Render,
                    (
                        specialize_material_meshes
                            .in_set(RenderSystems::Specialize)
                            .after(prepare_assets::<RenderMesh>)
                            .after(collect_meshes_for_gpu_building)
                            .after(set_mesh_motion_vector_flags),
                        queue_material_meshes.in_set(RenderSystems::QueueMeshes),
                    ),
                )
                .add_systems(
                    Render,
                    (
                        prepare_material_bind_groups,
                        write_material_bind_group_buffers,
                    )
                        .chain()
                        .in_set(RenderSystems::PrepareBindGroups),
                )
                .add_systems(
                    Render,
                    (
                        check_views_lights_need_specialization
                            .in_set(RenderSystems::Specialize)
                            .before(specialize_shadows),

                        specialize_shadows
                            .in_set(RenderSystems::Specialize)
                            .after(prepare_lights),
                        queue_shadows.in_set(RenderSystems::QueueMeshes),
                    ),
                );
        }
    }
}

pub struct MaterialPlugin<M: Material> {

    pub debug_flags: RenderDebugFlags,
    pub _marker: PhantomData<M>,
}

impl<M: Material> Default for MaterialPlugin<M> {
    fn default() -> Self {
        Self {
            debug_flags: RenderDebugFlags::default(),
            _marker: Default::default(),
        }
    }
}

impl<M: Material> Plugin for MaterialPlugin<M>
where
    M::Data: PartialEq + Eq + Hash + Clone,
{
    fn build(&self, app: &mut App) {
        app.init_asset::<M>()
            .register_type::<MeshMaterial3d<M>>()
            .init_resource::<EntitiesNeedingSpecialization<M>>()
            .add_plugins((ErasedRenderAssetPlugin::<MeshMaterial3d<M>>::default(),))
            .add_systems(
                PostUpdate,
                (
                    mark_meshes_as_changed_if_their_materials_changed::<M>.ambiguous_with_all(),
                    check_entities_needing_specialization::<M>.after(AssetEventSystems),
                )
                    .after(mark_3d_meshes_as_changed_if_their_assets_changed),
            );

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .add_systems(RenderStartup, add_material_bind_group_allocator::<M>)
                .add_systems(
                    ExtractSchedule,
                    (
                        extract_mesh_materials::<M>.in_set(MaterialExtractionSystems),
                        early_sweep_material_instances::<M>
                            .after(MaterialExtractionSystems)
                            .before(late_sweep_material_instances),
                        extract_entities_needs_specialization::<M>
                            .in_set(DirtySpecializationSystems::CheckForChanges),
                        extract_entities_that_need_specializations_removed::<M>
                            .in_set(DirtySpecializationSystems::CheckForRemovals),
                    ),
                );
        }
    }
}

fn add_material_bind_group_allocator<M: Material>(
    render_device: Res<RenderDevice>,
    mut bind_group_allocators: ResMut<MaterialBindGroupAllocators>,
) {
    bind_group_allocators.insert(
        TypeId::of::<M>(),
        MaterialBindGroupAllocator::new(
            &render_device,
            M::label(),
            material_uses_bindless_resources::<M>(&render_device)
                .then(|| M::bindless_descriptor())
                .flatten(),
            M::bind_group_layout_descriptor(&render_device),
            M::bindless_slot_count(),
        ),
    );
}

pub(crate) static DUMMY_MESH_MATERIAL: AssetId<StandardMaterial> =
    AssetId::<StandardMaterial>::invalid();

pub struct MaterialPipelineKey<M: Material> {
    pub mesh_key: MeshPipelineKey,
    pub bind_group_data: M::Data,
}

#[derive(Resource, Clone)]
pub struct MaterialPipeline {
    pub mesh_pipeline: MeshPipeline,
}

pub struct MaterialPipelineSpecializer {
    pub(crate) pipeline: MaterialPipeline,
    pub(crate) properties: Arc<MaterialProperties>,
}

impl SpecializedMeshPipeline for MaterialPipelineSpecializer {
    type Key = ErasedMaterialPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let concrete_mesh_key: MeshPipelineKey = key.mesh_key.downcast();
        let mut descriptor = self
            .pipeline
            .mesh_pipeline
            .specialize(concrete_mesh_key, layout)?;

        descriptor.vertex.shader_defs.push(ShaderDefVal::UInt(
            "MATERIAL_BIND_GROUP".into(),
            MATERIAL_BIND_GROUP_INDEX as u32,
        ));
        if let Some(ref mut fragment) = descriptor.fragment {
            fragment.shader_defs.push(ShaderDefVal::UInt(
                "MATERIAL_BIND_GROUP".into(),
                MATERIAL_BIND_GROUP_INDEX as u32,
            ));
        };
        if let Some(vertex_shader) = self.properties.get_shader(MaterialVertexShader) {
            descriptor.vertex.shader = vertex_shader.clone();
        }

        if let Some(fragment_shader) = self.properties.get_shader(MaterialFragmentShader) {
            descriptor.fragment.as_mut().unwrap().shader = fragment_shader.clone();
        }

        descriptor
            .layout
            .insert(3, self.properties.material_layout.as_ref().unwrap().clone());

        if let Some(specialize) = self.properties.user_specialize {
            specialize(&self.pipeline as &dyn Any, &mut descriptor, layout, key)?;
        }

        if self.properties.bindless {
            descriptor.vertex.shader_defs.push("BINDLESS".into());
            if let Some(ref mut fragment) = descriptor.fragment {
                fragment.shader_defs.push("BINDLESS".into());
            }
        }

        Ok(descriptor)
    }
}

pub fn init_material_pipeline(mut commands: Commands, mesh_pipeline: Res<MeshPipeline>) {
    commands.insert_resource(MaterialPipeline {
        mesh_pipeline: mesh_pipeline.clone(),
    });
}

pub type DrawMaterial = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    SetMeshBindGroup<2>,
    SetMaterialBindGroup<MATERIAL_BIND_GROUP_INDEX>,
    DrawMesh,
);

pub struct SetMaterialBindGroup<const I: usize>;
impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetMaterialBindGroup<I> {
    type Param = (
        SRes<ErasedRenderAssets<PreparedMaterial>>,
        SRes<RenderMaterialInstances>,
        SRes<MaterialBindGroupAllocators>,
    );
    type ViewQuery = ();
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        item: &P,
        _view: (),
        _item_query: Option<()>,
        (materials, material_instances, material_bind_group_allocator): SystemParamItem<
            'w,
            '_,
            Self::Param,
        >,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let materials = materials.into_inner();
        let material_instances = material_instances.into_inner();
        let material_bind_group_allocators = material_bind_group_allocator.into_inner();

        let Some(material_instance) = material_instances.instances.get(&item.main_entity()) else {
            return RenderCommandResult::Skip;
        };
        let Some(material_bind_group_allocator) =
            material_bind_group_allocators.get(&material_instance.asset_id.type_id())
        else {
            return RenderCommandResult::Skip;
        };
        let Some(material) = materials.get(material_instance.asset_id) else {
            return RenderCommandResult::Skip;
        };
        let Some(material_bind_group) = material_bind_group_allocator.get(material.binding.group)
        else {
            return RenderCommandResult::Skip;
        };
        let Some(bind_group) = material_bind_group.bind_group() else {
            return RenderCommandResult::Skip;
        };
        pass.set_bind_group(I, bind_group, &[]);
        RenderCommandResult::Success
    }
}

#[derive(Resource, Default)]
pub struct RenderMaterialInstances {

    pub instances: MainEntityHashMap<RenderMaterialInstance>,

    pub current_change_tick: Tick,
}

impl RenderMaterialInstances {

    pub(crate) fn mesh_material(&self, entity: MainEntity) -> UntypedAssetId {
        match self.instances.get(&entity) {
            Some(render_instance) => render_instance.asset_id,
            None => DUMMY_MESH_MATERIAL.into(),
        }
    }
}

pub struct RenderMaterialInstance {

    pub asset_id: UntypedAssetId,

    pub last_change_tick: Tick,
}

#[derive(SystemSet, Clone, PartialEq, Eq, Debug, Hash)]
pub struct MaterialExtractionSystems;

pub const fn alpha_mode_pipeline_key(alpha_mode: AlphaMode, msaa: &Msaa) -> MeshPipelineKey {
    match alpha_mode {

        AlphaMode::Premultiplied | AlphaMode::Add => MeshPipelineKey::BLEND_PREMULTIPLIED_ALPHA,
        AlphaMode::Blend => MeshPipelineKey::BLEND_ALPHA,
        AlphaMode::Multiply => MeshPipelineKey::BLEND_MULTIPLY,
        AlphaMode::Mask(_) => MeshPipelineKey::MAY_DISCARD,
        AlphaMode::AlphaToCoverage => match *msaa {
            Msaa::Off => MeshPipelineKey::MAY_DISCARD,
            _ => MeshPipelineKey::BLEND_ALPHA_TO_COVERAGE,
        },
        _ => MeshPipelineKey::NONE,
    }
}

pub const fn tonemapping_pipeline_key(tonemapping: Tonemapping) -> MeshPipelineKey {
    match tonemapping {
        Tonemapping::None => MeshPipelineKey::TONEMAP_METHOD_NONE,
        Tonemapping::Reinhard => MeshPipelineKey::TONEMAP_METHOD_REINHARD,
        Tonemapping::ReinhardLuminance => MeshPipelineKey::TONEMAP_METHOD_REINHARD_LUMINANCE,
        Tonemapping::AcesFitted => MeshPipelineKey::TONEMAP_METHOD_ACES_FITTED,
        Tonemapping::AgX => MeshPipelineKey::TONEMAP_METHOD_AGX,
        Tonemapping::SomewhatBoringDisplayTransform => {
            MeshPipelineKey::TONEMAP_METHOD_SOMEWHAT_BORING_DISPLAY_TRANSFORM
        }
        Tonemapping::TonyMcMapface => MeshPipelineKey::TONEMAP_METHOD_TONY_MC_MAPFACE,
        Tonemapping::BlenderFilmic => MeshPipelineKey::TONEMAP_METHOD_BLENDER_FILMIC,
        Tonemapping::KhronosPbrNeutral => MeshPipelineKey::TONEMAP_METHOD_PBR_NEUTRAL,
    }
}

fn mark_meshes_as_changed_if_their_materials_changed<M>(
    mut changed_meshes_query: Query<
        &mut Mesh3d,
        Or<(Changed<MeshMaterial3d<M>>, AssetChanged<MeshMaterial3d<M>>)>,
    >,
) where
    M: Material,
{
    changed_meshes_query.par_iter_mut().for_each(|mut mesh| {
        mesh.set_changed();
    });
}

fn extract_mesh_materials<M: Material>(
    mut material_instances: ResMut<RenderMaterialInstances>,
    changed_meshes_query: Extract<
        Query<
            (Entity, &ViewVisibility, &MeshMaterial3d<M>),
            Or<(Changed<ViewVisibility>, Changed<MeshMaterial3d<M>>)>,
        >,
    >,
) {
    let last_change_tick = material_instances.current_change_tick;

    for (entity, view_visibility, material) in &changed_meshes_query {
        if view_visibility.get() {
            material_instances.instances.insert(
                entity.into(),
                RenderMaterialInstance {
                    asset_id: material.id().untyped(),
                    last_change_tick,
                },
            );
        } else {
            material_instances
                .instances
                .remove(&MainEntity::from(entity));
        }
    }
}

fn early_sweep_material_instances<M>(
    mut material_instances: ResMut<RenderMaterialInstances>,
    mut removed_materials_query: Extract<RemovedComponents<MeshMaterial3d<M>>>,
) where
    M: Material,
{
    let last_change_tick = material_instances.current_change_tick;

    for entity in removed_materials_query.read() {
        if let Entry::Occupied(occupied_entry) = material_instances.instances.entry(entity.into()) {

            if occupied_entry.get().last_change_tick != last_change_tick {
                occupied_entry.remove();
            }
        }
    }
}

pub fn late_sweep_material_instances(
    mut material_instances: ResMut<RenderMaterialInstances>,
    mut removed_meshes_query: Extract<RemovedComponents<Mesh3d>>,
) {
    let last_change_tick = material_instances.current_change_tick;

    for entity in removed_meshes_query.read() {
        if let Entry::Occupied(occupied_entry) = material_instances.instances.entry(entity.into()) {

            if occupied_entry.get().last_change_tick != last_change_tick {
                occupied_entry.remove();
            }
        }
    }

    material_instances
        .current_change_tick
        .set(last_change_tick.get() + 1);
}

pub fn extract_entities_needs_specialization<M>(
    entities_needing_specialization: Extract<Res<EntitiesNeedingSpecialization<M>>>,
    mut dirty_specializations: ResMut<DirtySpecializations>,
) where
    M: Material,
{

    for entity in entities_needing_specialization.changed.iter() {
        dirty_specializations
            .changed_renderables
            .insert(MainEntity::from(*entity));
    }
}

pub fn extract_entities_that_need_specializations_removed<M>(
    entities_needing_specialization: Extract<Res<EntitiesNeedingSpecialization<M>>>,
    mut dirty_specializations: ResMut<DirtySpecializations>,
) where
    M: Material,
{
    for entity in entities_needing_specialization.removed.iter() {
        dirty_specializations
            .removed_renderables
            .insert(MainEntity::from(*entity));
    }
}

#[derive(Resource, Clone, Debug)]
pub struct EntitiesNeedingSpecialization<M> {

    pub changed: Vec<Entity>,

    pub removed: Vec<Entity>,
    _marker: PhantomData<M>,
}

impl<M> Default for EntitiesNeedingSpecialization<M> {
    fn default() -> Self {
        Self {
            changed: Default::default(),
            removed: Default::default(),
            _marker: Default::default(),
        }
    }
}

#[derive(Resource, Deref, DerefMut, Default)]
pub struct SpecializedMaterialPipelineCache {

    #[deref]
    map: HashMap<RetainedViewEntity, SpecializedMaterialViewPipelineCache>,
}

#[derive(Deref, DerefMut, Default)]
pub struct SpecializedMaterialViewPipelineCache {

    #[deref]
    map: MainEntityHashMap<CachedRenderPipelineId>,
}

pub fn check_entities_needing_specialization<M>(
    needs_specialization: Query<
        Entity,
        (
            Or<(
                Changed<Mesh3d>,
                AssetChanged<Mesh3d>,
                Changed<MeshMaterial3d<M>>,
                AssetChanged<MeshMaterial3d<M>>,
            )>,
            With<MeshMaterial3d<M>>,
        ),
    >,
    mut par_local: Local<Parallel<Vec<Entity>>>,
    mut entities_needing_specialization: ResMut<EntitiesNeedingSpecialization<M>>,
    mut removed_mesh_3d_components: RemovedComponents<Mesh3d>,
    mut removed_mesh_material_3d_components: RemovedComponents<MeshMaterial3d<M>>,
) where
    M: Material,
{
    entities_needing_specialization.changed.clear();
    entities_needing_specialization.removed.clear();

    needs_specialization
        .par_iter()
        .for_each(|entity| par_local.borrow_local_mut().push(entity));
    par_local.drain_into(&mut entities_needing_specialization.changed);

    for entity in removed_mesh_3d_components
        .read()
        .chain(removed_mesh_material_3d_components.read())
    {
        entities_needing_specialization.removed.push(entity);
    }
}

pub(crate) struct SpecializationWorkItem {
    visible_entity: MainEntity,
    retained_view_entity: RetainedViewEntity,
    mesh_key: MeshPipelineKey,
    layout: MeshVertexBufferLayoutRef,
    properties: Arc<MaterialProperties>,
    material_type_id: TypeId,
}

#[derive(Default, Deref, DerefMut, Resource)]
pub struct PendingMeshMaterialQueues(pub PendingQueues);

#[derive(SystemParam)]
pub(crate) struct SpecializeMaterialMeshesSystemParam<'w, 's> {
    render_meshes: Res<'w, RenderAssets<RenderMesh>>,
    render_materials: Res<'w, ErasedRenderAssets<PreparedMaterial>>,
    render_mesh_instances: Res<'w, RenderMeshInstances>,
    render_material_instances: Res<'w, RenderMaterialInstances>,
    render_lightmaps: Res<'w, RenderLightmaps>,
    render_visibility_ranges: Res<'w, RenderVisibilityRanges>,
    opaque_render_phases: Res<'w, ViewBinnedRenderPhases<Opaque3d>>,
    alpha_mask_render_phases: Res<'w, ViewBinnedRenderPhases<AlphaMask3d>>,
    transmissive_render_phases: Res<'w, ViewSortedRenderPhases<Transmissive3d>>,
    transparent_render_phases: Res<'w, ViewSortedRenderPhases<Transparent3d>>,
    views: Query<'w, 's, (&'static ExtractedView, &'static RenderVisibleEntities)>,
    view_key_cache: Res<'w, ViewKeyCache>,
    specialized_material_pipeline_cache: ResMut<'w, SpecializedMaterialPipelineCache>,
    pending_mesh_material_queues: ResMut<'w, PendingMeshMaterialQueues>,
    dirty_specializations: Res<'w, DirtySpecializations>,
}

pub(crate) fn specialize_material_meshes(
    world: &mut World,
    state: &mut SystemState<SpecializeMaterialMeshesSystemParam>,
    mut work_items: Local<Vec<SpecializationWorkItem>>,
    mut all_views: Local<HashSet<RetainedViewEntity, FixedHasher>>,
) {
    work_items.clear();
    all_views.clear();

    {
        let SpecializeMaterialMeshesSystemParam {
            render_meshes,
            render_materials,
            render_mesh_instances,
            render_material_instances,
            render_lightmaps,
            render_visibility_ranges,
            opaque_render_phases,
            alpha_mask_render_phases,
            transmissive_render_phases,
            transparent_render_phases,
            views,
            view_key_cache,
            mut specialized_material_pipeline_cache,
            mut pending_mesh_material_queues,
            dirty_specializations,
        } = state.get_mut(world).unwrap();

        for (view, visible_entities) in &views {
            all_views.insert(view.retained_view_entity);

            if !transparent_render_phases.contains_key(&view.retained_view_entity)
                && !opaque_render_phases.contains_key(&view.retained_view_entity)
                && !alpha_mask_render_phases.contains_key(&view.retained_view_entity)
                && !transmissive_render_phases.contains_key(&view.retained_view_entity)
            {
                continue;
            }

            let Some(view_key) = view_key_cache.get(&view.retained_view_entity) else {
                continue;
            };

            let Some(render_visible_mesh_entities) = visible_entities.get::<Mesh3d>() else {
                continue;
            };

            let mut maybe_specialized_material_pipeline_cache =
                specialized_material_pipeline_cache.get_mut(&view.retained_view_entity);

            if let Some(ref mut specialized_material_pipeline_cache) =
                maybe_specialized_material_pipeline_cache
            {
                if dirty_specializations
                    .must_wipe_specializations_for_view(view.retained_view_entity)
                {
                    specialized_material_pipeline_cache.clear();
                } else {
                    for &renderable_entity in dirty_specializations.iter_to_despecialize() {
                        specialized_material_pipeline_cache.remove(&renderable_entity);
                    }
                }
            }

            let view_pending_mesh_material_queues =
                pending_mesh_material_queues.prepare_for_new_frame(view.retained_view_entity);

            for (render_entity, visible_entity) in dirty_specializations.iter_to_specialize(
                view.retained_view_entity,
                render_visible_mesh_entities,
                &view_pending_mesh_material_queues.prev_frame,
            ) {
                if maybe_specialized_material_pipeline_cache
                    .as_ref()
                    .is_some_and(|specialized_material_pipeline_cache| {
                        specialized_material_pipeline_cache.contains_key(visible_entity)
                    })
                {
                    continue;
                }

                let Some(material_instance) =
                    render_material_instances.instances.get(visible_entity)
                else {
                    view_pending_mesh_material_queues
                        .current_frame
                        .insert((*render_entity, *visible_entity));
                    continue;
                };
                let Some(mesh_instance) =
                    render_mesh_instances.render_mesh_queue_data(*visible_entity)
                else {
                    view_pending_mesh_material_queues
                        .current_frame
                        .insert((*render_entity, *visible_entity));
                    continue;
                };
                let Some(mesh) = render_meshes.get(mesh_instance.mesh_asset_id()) else {
                    continue;
                };
                let Some(material) = render_materials.get(material_instance.asset_id) else {
                    view_pending_mesh_material_queues
                        .current_frame
                        .insert((*render_entity, *visible_entity));
                    continue;
                };

                let mut mesh_pipeline_key_bits: MeshPipelineKey =
                    material.properties.mesh_pipeline_key_bits.downcast();
                mesh_pipeline_key_bits.insert(alpha_mode_pipeline_key(
                    material.properties.alpha_mode,
                    &Msaa::from_samples(view_key.msaa_samples()),
                ));
                let mut mesh_key = *view_key
                    | MeshPipelineKey::from_bits_retain(mesh.key_bits.bits())
                    | mesh_pipeline_key_bits;

                if let Some(lightmap) = render_lightmaps.render_lightmaps.get(visible_entity) {
                    mesh_key |= MeshPipelineKey::LIGHTMAPPED;

                    if lightmap.bicubic_sampling {
                        mesh_key |= MeshPipelineKey::LIGHTMAP_BICUBIC_SAMPLING;
                    }
                }

                if render_visibility_ranges
                    .entity_has_crossfading_visibility_ranges(*visible_entity)
                {
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

                work_items.push(SpecializationWorkItem {
                    visible_entity: *visible_entity,
                    retained_view_entity: view.retained_view_entity,
                    mesh_key,
                    layout: mesh.layout.clone(),
                    properties: material.properties.clone(),
                    material_type_id: material_instance.asset_id.type_id(),
                });
            }
        }

        pending_mesh_material_queues.expire_stale_views(&all_views);
    }

    for item in work_items.drain(..) {
        let key = ErasedMaterialPipelineKey {
            type_id: item.material_type_id,
            mesh_key: ErasedMeshPipelineKey::new(item.mesh_key),
            material_key: item.properties.material_key.clone(),
        };

        let Some(base_specialize) = item.properties.base_specialize else {
            continue;
        };
        match base_specialize(world, key, &item.layout, &item.properties) {
            Ok(pipeline_id) => {
                world
                    .resource_mut::<SpecializedMaterialPipelineCache>()
                    .entry(item.retained_view_entity)
                    .or_default()
                    .insert(item.visible_entity, pipeline_id);
            }
            Err(err) => error!("{}", err),
        }
    }

    world
        .resource_mut::<SpecializedMaterialPipelineCache>()
        .retain(|view, _| all_views.contains(view));
}

pub fn queue_material_meshes(
    render_materials: Res<ErasedRenderAssets<PreparedMaterial>>,
    render_mesh_instances: Res<RenderMeshInstances>,
    render_material_instances: Res<RenderMaterialInstances>,
    mesh_assets: Res<RenderAssets<RenderMesh>>,
    mesh_allocator: Res<MeshAllocator>,
    gpu_preprocessing_support: Res<GpuPreprocessingSupport>,
    maybe_batched_instance_buffers: Option<
        Res<BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>,
    >,
    mut opaque_render_phases: ResMut<ViewBinnedRenderPhases<Opaque3d>>,
    mut alpha_mask_render_phases: ResMut<ViewBinnedRenderPhases<AlphaMask3d>>,
    mut transmissive_render_phases: ResMut<ViewSortedRenderPhases<Transmissive3d>>,
    mut transparent_render_phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    mut pending_mesh_material_queues: ResMut<PendingMeshMaterialQueues>,
    views: Query<(&ExtractedView, &RenderVisibleEntities)>,
    specialized_material_pipeline_cache: ResMut<SpecializedMaterialPipelineCache>,
    dirty_specializations: Res<DirtySpecializations>,
) {
    for (view, visible_entities) in &views {
        let (
            Some(opaque_phase),
            Some(alpha_mask_phase),
            Some(transmissive_phase),
            Some(transparent_phase),
        ) = (
            opaque_render_phases.get_mut(&view.retained_view_entity),
            alpha_mask_render_phases.get_mut(&view.retained_view_entity),
            transmissive_render_phases.get_mut(&view.retained_view_entity),
            transparent_render_phases.get_mut(&view.retained_view_entity),
        )
        else {
            continue;
        };

        let Some(view_specialized_material_pipeline_cache) =
            specialized_material_pipeline_cache.get(&view.retained_view_entity)
        else {
            continue;
        };

        let Some(render_visible_mesh_entities) = visible_entities.get::<Mesh3d>() else {
            continue;
        };

        for &main_entity in dirty_specializations
            .iter_to_dequeue(view.retained_view_entity, render_visible_mesh_entities)
        {
            opaque_phase.remove(main_entity);
            alpha_mask_phase.remove(main_entity);
            transmissive_phase.remove(Entity::PLACEHOLDER, main_entity);
            transparent_phase.remove(Entity::PLACEHOLDER, main_entity);
        }

        let view_pending_mesh_material_queues = pending_mesh_material_queues
            .get_mut(&view.retained_view_entity)
            .expect(
                "View pending mesh material queues should have been created in \
                 `specialize_material_meshes`",
            );

        for (render_entity, visible_entity) in dirty_specializations.iter_to_queue(
            view.retained_view_entity,
            render_visible_mesh_entities,
            &view_pending_mesh_material_queues.prev_frame,
        ) {
            let Some(pipeline_id) = view_specialized_material_pipeline_cache
                .get(visible_entity)
                .copied()
            else {
                continue;
            };

            let Some(material_instance) = render_material_instances.instances.get(visible_entity)
            else {
                view_pending_mesh_material_queues
                    .current_frame
                    .insert((*render_entity, *visible_entity));
                continue;
            };
            let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*visible_entity)
            else {
                view_pending_mesh_material_queues
                    .current_frame
                    .insert((*render_entity, *visible_entity));
                continue;
            };
            let Some(material) = render_materials.get(material_instance.asset_id) else {
                view_pending_mesh_material_queues
                    .current_frame
                    .insert((*render_entity, *visible_entity));
                continue;
            };

            let Some(mesh_slabs) = mesh_allocator.mesh_slabs(&mesh_instance.mesh_asset_id()) else {
                continue;
            };

            match material.properties.render_phase_type {
                RenderPhaseType::Transmissive => {
                    let Some(draw_function) = material
                        .properties
                        .get_draw_function(MainPassTransmissiveDrawFunction)
                    else {
                        continue;
                    };
                    transmissive_phase.add_retained(Transmissive3d {
                        sorting_info: TransparentSortingInfo3d::Sorted {
                            mesh_center: get_mesh_instance_world_from_local(
                                *visible_entity,
                                mesh_instance.current_uniform_index,
                                &render_mesh_instances,
                                maybe_batched_instance_buffers.as_deref(),
                            )
                            .transform_point3(
                                mesh_assets
                                    .get(mesh_instance.mesh_asset_id())
                                    .unwrap()
                                    .aabb_center,
                            ),
                            depth_bias: material.properties.depth_bias,
                        },
                        entity: (Entity::PLACEHOLDER, *visible_entity),
                        draw_function,
                        pipeline: pipeline_id,
                        batch_range: 0..1,
                        extra_index: PhaseItemExtraIndex::None,
                        indexed: mesh_slabs.index_slab_id.is_some(),

                        distance: 0.0,
                    });
                }
                RenderPhaseType::Opaque => {
                    if material.properties.render_method == OpaqueRendererMethod::Deferred {

                        opaque_phase.update_cache(*visible_entity, None);
                        continue;
                    }
                    let Some(draw_function) = material
                        .properties
                        .get_draw_function(MainPassOpaqueDrawFunction)
                    else {
                        continue;
                    };
                    let batch_set_key = Opaque3dBatchSetKey {
                        pipeline: pipeline_id,
                        draw_function,
                        material_bind_group_index: Some(material.binding.group.0),
                        slabs: mesh_slabs,
                        lightmap_slab: mesh_instance
                            .shared
                            .lightmap_slab_index()
                            .map(|index| *index),
                    };
                    let bin_key = Opaque3dBinKey {
                        asset_id: mesh_instance.mesh_asset_id().into(),
                    };
                    opaque_phase.add(
                        batch_set_key,
                        bin_key,
                        (Entity::PLACEHOLDER, *visible_entity),
                        mesh_instance.current_uniform_index,
                        BinnedRenderPhaseType::mesh(
                            mesh_instance.should_batch(),
                            &gpu_preprocessing_support,
                        ),
                    );
                }

                RenderPhaseType::AlphaMask => {
                    let Some(draw_function) = material
                        .properties
                        .get_draw_function(MainPassAlphaMaskDrawFunction)
                    else {
                        continue;
                    };
                    let batch_set_key = OpaqueNoLightmap3dBatchSetKey {
                        draw_function,
                        pipeline: pipeline_id,
                        material_bind_group_index: Some(material.binding.group.0),
                        slabs: mesh_slabs,
                    };
                    let bin_key = OpaqueNoLightmap3dBinKey {
                        asset_id: mesh_instance.mesh_asset_id().into(),
                    };
                    alpha_mask_phase.add(
                        batch_set_key,
                        bin_key,
                        (Entity::PLACEHOLDER, *visible_entity),
                        mesh_instance.current_uniform_index,
                        BinnedRenderPhaseType::mesh(
                            mesh_instance.should_batch(),
                            &gpu_preprocessing_support,
                        ),
                    );
                }
                RenderPhaseType::Transparent => {
                    let Some(draw_function) = material
                        .properties
                        .get_draw_function(MainPassTransparentDrawFunction)
                    else {
                        continue;
                    };
                    transparent_phase.add_retained(Transparent3d {
                        sorting_info: TransparentSortingInfo3d::Sorted {
                            mesh_center: get_mesh_instance_world_from_local(
                                *visible_entity,
                                mesh_instance.current_uniform_index,
                                &render_mesh_instances,
                                maybe_batched_instance_buffers.as_deref(),
                            )
                            .transform_point3(
                                mesh_assets
                                    .get(mesh_instance.mesh_asset_id())
                                    .unwrap()
                                    .aabb_center,
                            ),
                            depth_bias: material.properties.depth_bias,
                        },
                        entity: (Entity::PLACEHOLDER, *visible_entity),
                        draw_function,
                        pipeline: pipeline_id,
                        batch_range: 0..1,
                        extra_index: PhaseItemExtraIndex::None,
                        indexed: mesh_slabs.index_slab_id.is_some(),

                        distance: 0.0,
                    });
                }
            }
        }
    }
}

#[derive(Default, Resource, Clone, Debug, ExtractResource, Reflect)]
#[reflect(Resource, Default, Debug, Clone)]
pub struct DefaultOpaqueRendererMethod(OpaqueRendererMethod);

impl DefaultOpaqueRendererMethod {
    pub fn forward() -> Self {
        DefaultOpaqueRendererMethod(OpaqueRendererMethod::Forward)
    }

    pub fn deferred() -> Self {
        DefaultOpaqueRendererMethod(OpaqueRendererMethod::Deferred)
    }

    pub fn set_to_forward(&mut self) {
        self.0 = OpaqueRendererMethod::Forward;
    }

    pub fn set_to_deferred(&mut self) {
        self.0 = OpaqueRendererMethod::Deferred;
    }
}

#[derive(ShaderLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct MaterialVertexShader;

#[derive(ShaderLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct MaterialFragmentShader;

#[derive(ShaderLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct PrepassVertexShader;

#[derive(ShaderLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct PrepassFragmentShader;

#[derive(ShaderLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct DeferredVertexShader;

#[derive(ShaderLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct DeferredFragmentShader;

#[derive(ShaderLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct MeshletFragmentShader;

#[derive(ShaderLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct MeshletPrepassFragmentShader;

#[derive(ShaderLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct MeshletDeferredFragmentShader;

#[derive(DrawFunctionLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct MainPassOpaqueDrawFunction;
#[derive(DrawFunctionLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct MainPassAlphaMaskDrawFunction;
#[derive(DrawFunctionLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct MainPassTransmissiveDrawFunction;
#[derive(DrawFunctionLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct MainPassTransparentDrawFunction;

#[derive(DrawFunctionLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct PrepassOpaqueDrawFunction;
#[derive(DrawFunctionLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct PrepassAlphaMaskDrawFunction;
#[derive(DrawFunctionLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct PrepassOpaqueDepthOnlyDrawFunction;

#[derive(DrawFunctionLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct DeferredOpaqueDrawFunction;
#[derive(DrawFunctionLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct DeferredAlphaMaskDrawFunction;

#[derive(DrawFunctionLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct ShadowsDrawFunction;
#[derive(DrawFunctionLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct ShadowsDepthOnlyDrawFunction;

#[derive(Resource, Default, Deref, DerefMut)]
pub struct RenderMaterialBindings(HashMap<UntypedAssetId, MaterialBindingId>);

pub struct PreparedMaterial {
    pub binding: MaterialBindingId,
    pub properties: Arc<MaterialProperties>,
}

pub fn base_specialize(
    world: &mut World,
    key: ErasedMaterialPipelineKey,
    layout: &MeshVertexBufferLayoutRef,
    properties: &Arc<MaterialProperties>,
) -> Result<CachedRenderPipelineId, SpecializedMeshPipelineError> {
    world.resource_scope(
        |world, mut pipelines: Mut<SpecializedMeshPipelines<MaterialPipelineSpecializer>>| {
            let mesh_pipeline = world.resource::<MeshPipeline>().clone();
            let pipeline_cache = world.resource::<PipelineCache>();

            let specializer = MaterialPipelineSpecializer {
                pipeline: MaterialPipeline { mesh_pipeline },
                properties: properties.clone(),
            };

            pipelines.specialize(pipeline_cache, &specializer, key, layout)
        },
    )
}
fn prepass_specialize(
    world: &mut World,
    key: ErasedMaterialPipelineKey,
    layout: &MeshVertexBufferLayoutRef,
    properties: &Arc<MaterialProperties>,
) -> Result<CachedRenderPipelineId, SpecializedMeshPipelineError> {
    world.resource_scope(
        |world, mut pipelines: Mut<SpecializedMeshPipelines<PrepassPipelineSpecializer>>| {
            let prepass_pipeline = world.resource::<PrepassPipeline>().clone();
            let pipeline_cache = world.resource::<PipelineCache>();

            let specializer = PrepassPipelineSpecializer {
                pipeline: prepass_pipeline,
                properties: properties.clone(),
            };

            pipelines.specialize(pipeline_cache, &specializer, key, layout)
        },
    )
}

fn user_specialize<M: Material>(
    pipeline: &dyn Any,
    descriptor: &mut RenderPipelineDescriptor,
    mesh_layout: &MeshVertexBufferLayoutRef,
    erased_key: ErasedMaterialPipelineKey,
) -> Result<(), SpecializedMeshPipelineError>
where
    M::Data: Hash + Clone,
{
    let pipeline = pipeline.downcast_ref::<MaterialPipeline>().unwrap();
    let material_key = erased_key.material_key.to_key();
    let mesh_key: MeshPipelineKey = erased_key.mesh_key.downcast();
    M::specialize(
        pipeline,
        descriptor,
        mesh_layout,
        MaterialPipelineKey {
            mesh_key,
            bind_group_data: material_key,
        },
    )
}

impl<M: Material> ErasedRenderAsset for MeshMaterial3d<M>
where
    M::Data: PartialEq + Eq + Hash + Clone,
{
    type SourceAsset = M;
    type ErasedAsset = PreparedMaterial;

    type Param = (
        SRes<RenderDevice>,
        SRes<PipelineCache>,
        SRes<DefaultOpaqueRendererMethod>,
        SResMut<MaterialBindGroupAllocators>,
        SResMut<RenderMaterialBindings>,
        SRes<DrawFunctions<Opaque3d>>,
        SRes<DrawFunctions<AlphaMask3d>>,
        SRes<DrawFunctions<Transmissive3d>>,
        SRes<DrawFunctions<Transparent3d>>,
        SRes<DrawFunctions<Opaque3dPrepass>>,
        SRes<DrawFunctions<AlphaMask3dPrepass>>,
        SRes<DrawFunctions<Opaque3dDeferred>>,
        SRes<DrawFunctions<AlphaMask3dDeferred>>,
        SRes<DrawFunctions<Shadow>>,
        SRes<AssetServer>,
        M::Param,
    );

    fn prepare_asset(
        material: Self::SourceAsset,
        material_id: AssetId<Self::SourceAsset>,
        (
            render_device,
            pipeline_cache,
            default_opaque_render_method,
            bind_group_allocators,
            render_material_bindings,
            opaque_draw_functions,
            alpha_mask_draw_functions,
            transmissive_draw_functions,
            transparent_draw_functions,
            opaque_prepass_draw_functions,
            alpha_mask_prepass_draw_functions,
            opaque_deferred_draw_functions,
            alpha_mask_deferred_draw_functions,
            shadow_draw_functions,
            asset_server,
            material_param,
        ): &mut SystemParamItem<Self::Param>,
    ) -> Result<Self::ErasedAsset, PrepareAssetError<Self::SourceAsset>> {
        let material_layout = M::bind_group_layout_descriptor(render_device);
        let actual_material_layout = pipeline_cache.get_bind_group_layout(&material_layout);

        let binding = match material.unprepared_bind_group(
            &actual_material_layout,
            render_device,
            material_param,
            false,
        ) {
            Ok(unprepared) => {
                let bind_group_allocator =
                    bind_group_allocators.get_mut(&TypeId::of::<M>()).unwrap();

                match render_material_bindings.entry(material_id.into()) {
                    Entry::Occupied(mut occupied_entry) => {

                        bind_group_allocator.free(*occupied_entry.get());
                        let new_binding =
                            bind_group_allocator.allocate_unprepared(unprepared, &material_layout);
                        *occupied_entry.get_mut() = new_binding;
                        new_binding
                    }
                    Entry::Vacant(vacant_entry) => *vacant_entry.insert(
                        bind_group_allocator.allocate_unprepared(unprepared, &material_layout),
                    ),
                }
            }
            Err(AsBindGroupError::RetryNextUpdate) => {
                return Err(PrepareAssetError::RetryNextUpdate(material))
            }
            Err(AsBindGroupError::CreateBindGroupDirectly) => {
                match material.as_bind_group(
                    &material_layout,
                    render_device,
                    pipeline_cache,
                    material_param,
                ) {
                    Ok(prepared_bind_group) => {
                        let bind_group_allocator =
                            bind_group_allocators.get_mut(&TypeId::of::<M>()).unwrap();

                        let material_binding_id =
                            bind_group_allocator.allocate_prepared(prepared_bind_group);
                        render_material_bindings.insert(material_id.into(), material_binding_id);
                        material_binding_id
                    }
                    Err(AsBindGroupError::RetryNextUpdate) => {
                        return Err(PrepareAssetError::RetryNextUpdate(material))
                    }
                    Err(other) => return Err(PrepareAssetError::AsBindGroupError(other)),
                }
            }
            Err(other) => return Err(PrepareAssetError::AsBindGroupError(other)),
        };

        let shadows_enabled = M::enable_shadows();
        let prepass_enabled = M::enable_prepass();

        let draw_opaque_pbr = opaque_draw_functions.read().id::<DrawMaterial>();
        let draw_alpha_mask_pbr = alpha_mask_draw_functions.read().id::<DrawMaterial>();
        let draw_transmissive_pbr = transmissive_draw_functions.read().id::<DrawMaterial>();
        let draw_transparent_pbr = transparent_draw_functions.read().id::<DrawMaterial>();
        let draw_opaque_prepass = opaque_prepass_draw_functions.read().id::<DrawPrepass>();
        let draw_alpha_mask_prepass = alpha_mask_prepass_draw_functions.read().id::<DrawPrepass>();
        let draw_opaque_prepass_depth_only = opaque_prepass_draw_functions
            .read()
            .id::<DrawDepthOnlyPrepass>();
        let draw_opaque_deferred = opaque_deferred_draw_functions.read().id::<DrawPrepass>();
        let draw_alpha_mask_deferred = alpha_mask_deferred_draw_functions
            .read()
            .id::<DrawPrepass>();
        let draw_shadows = shadow_draw_functions.read().id::<DrawPrepass>();
        let draw_shadows_depth_only = shadow_draw_functions.read().id::<DrawDepthOnlyPrepass>();

        let draw_functions = SmallVec::from_iter([
            (MainPassOpaqueDrawFunction.intern(), draw_opaque_pbr),
            (MainPassAlphaMaskDrawFunction.intern(), draw_alpha_mask_pbr),
            (
                MainPassTransmissiveDrawFunction.intern(),
                draw_transmissive_pbr,
            ),
            (
                MainPassTransparentDrawFunction.intern(),
                draw_transparent_pbr,
            ),
            (PrepassOpaqueDrawFunction.intern(), draw_opaque_prepass),
            (
                PrepassAlphaMaskDrawFunction.intern(),
                draw_alpha_mask_prepass,
            ),
            (
                PrepassOpaqueDepthOnlyDrawFunction.intern(),
                draw_opaque_prepass_depth_only,
            ),
            (DeferredOpaqueDrawFunction.intern(), draw_opaque_deferred),
            (
                DeferredAlphaMaskDrawFunction.intern(),
                draw_alpha_mask_deferred,
            ),
            (ShadowsDrawFunction.intern(), draw_shadows),
            (
                ShadowsDepthOnlyDrawFunction.intern(),
                draw_shadows_depth_only,
            ),
        ]);

        let render_method = match material.opaque_render_method() {
            OpaqueRendererMethod::Forward => OpaqueRendererMethod::Forward,
            OpaqueRendererMethod::Deferred => OpaqueRendererMethod::Deferred,
            OpaqueRendererMethod::Auto => default_opaque_render_method.0,
        };

        let mut mesh_pipeline_key_bits = MeshPipelineKey::empty();
        mesh_pipeline_key_bits.set(
            MeshPipelineKey::READS_VIEW_TRANSMISSION_TEXTURE,
            material.reads_view_transmission_texture(),
        );

        let reads_view_transmission_texture =
            mesh_pipeline_key_bits.contains(MeshPipelineKey::READS_VIEW_TRANSMISSION_TEXTURE);

        let mesh_pipeline_key_bits = ErasedMeshPipelineKey::new(mesh_pipeline_key_bits);

        let render_phase_type = match material.alpha_mode() {
            AlphaMode::Blend | AlphaMode::Premultiplied | AlphaMode::Add | AlphaMode::Multiply => {
                RenderPhaseType::Transparent
            }
            _ if reads_view_transmission_texture => RenderPhaseType::Transmissive,
            AlphaMode::Opaque | AlphaMode::AlphaToCoverage => RenderPhaseType::Opaque,
            AlphaMode::Mask(_) => RenderPhaseType::AlphaMask,
        };

        let mut shaders = SmallVec::new();
        let mut add_shader = |label: InternedShaderLabel, shader_ref: ShaderRef| {
            let mayber_shader = match shader_ref {
                ShaderRef::Default => None,
                ShaderRef::Handle(handle) => Some(handle),
                ShaderRef::Path(path) => Some(asset_server.load(path)),
            };
            if let Some(shader) = mayber_shader {
                shaders.push((label, shader));
            }
        };
        add_shader(MaterialVertexShader.intern(), M::vertex_shader());
        add_shader(MaterialFragmentShader.intern(), M::fragment_shader());
        add_shader(PrepassVertexShader.intern(), M::prepass_vertex_shader());
        add_shader(PrepassFragmentShader.intern(), M::prepass_fragment_shader());
        add_shader(DeferredVertexShader.intern(), M::deferred_vertex_shader());
        add_shader(
            DeferredFragmentShader.intern(),
            M::deferred_fragment_shader(),
        );

        #[cfg(feature = "meshlet")]
        {
            add_shader(
                MeshletFragmentShader.intern(),
                M::meshlet_mesh_fragment_shader(),
            );
            add_shader(
                MeshletPrepassFragmentShader.intern(),
                M::meshlet_mesh_prepass_fragment_shader(),
            );
            add_shader(
                MeshletDeferredFragmentShader.intern(),
                M::meshlet_mesh_deferred_fragment_shader(),
            );
        }

        let bindless = material_uses_bindless_resources::<M>(render_device);
        let bind_group_data = material.bind_group_data();
        let material_key = ErasedMaterialKey::new(bind_group_data);

        Ok(PreparedMaterial {
            binding,
            properties: Arc::new(MaterialProperties {
                alpha_mode: material.alpha_mode(),
                depth_bias: material.depth_bias(),
                reads_view_transmission_texture,
                render_phase_type,
                render_method,
                mesh_pipeline_key_bits,
                material_layout: Some(material_layout),
                draw_functions,
                shaders,
                bindless,
                base_specialize: Some(base_specialize),
                prepass_specialize: Some(prepass_specialize),
                user_specialize: Some(user_specialize::<M>),
                material_key,
                shadows_enabled,
                prepass_enabled,
            }),
        })
    }

    fn unload_asset(
        source_asset: AssetId<Self::SourceAsset>,
        (_, _, _, bind_group_allocators, render_material_bindings, ..): &mut SystemParamItem<
            Self::Param,
        >,
    ) {
        let Some(material_binding_id) = render_material_bindings.remove(&source_asset.untyped())
        else {
            return;
        };
        let bind_group_allactor = bind_group_allocators.get_mut(&TypeId::of::<M>()).unwrap();
        bind_group_allactor.free(material_binding_id);
    }
}

pub fn prepare_material_bind_groups(
    mut allocators: ResMut<MaterialBindGroupAllocators>,
    render_device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    fallback_image: Res<FallbackImage>,
    fallback_resources: Res<FallbackBindlessResources>,
) {
    for (_, allocator) in allocators.iter_mut() {
        allocator.prepare_bind_groups(
            &render_device,
            &pipeline_cache,
            &fallback_resources,
            &fallback_image,
        );
    }
}

pub fn write_material_bind_group_buffers(
    mut allocators: ResMut<MaterialBindGroupAllocators>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    for (_, allocator) in allocators.iter_mut() {
        allocator.write_buffers(&render_device, &render_queue);
    }
}

pub fn get_mesh_instance_world_from_local(
    entity: MainEntity,
    current_uniform_index: InputUniformIndex,
    render_mesh_instances: &RenderMeshInstances,
    maybe_batched_instance_buffers: Option<&BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>,
) -> Affine3 {

    match *render_mesh_instances {
        RenderMeshInstances::CpuBuilding(ref render_mesh_instances_cpu) => {
            let Some(render_mesh_instance) = render_mesh_instances_cpu.get(&entity) else {
                return Affine3::IDENTITY;
            };
            render_mesh_instance.transforms.world_from_local
        }
        RenderMeshInstances::GpuBuilding(_) => {
            let Some(batched_instance_buffers) = maybe_batched_instance_buffers else {
                return Affine3::IDENTITY;
            };
            let Some(mesh_input_uniform) = batched_instance_buffers
                .current_input_buffer
                .get(current_uniform_index.0)
            else {
                return Affine3::IDENTITY;
            };
            Affine3::from_transpose(mesh_input_uniform.world_from_local)
        }
    }
}

pub(crate) trait MaterialPropertiesExt {
    fn prepass_reads_material(&self) -> bool;
}

impl MaterialPropertiesExt for MaterialProperties {
    fn prepass_reads_material(&self) -> bool {

        self.get_shader(PrepassVertexShader).is_some()
            || self.get_shader(PrepassFragmentShader).is_some()
    }
}
