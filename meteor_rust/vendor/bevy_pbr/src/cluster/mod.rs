use core::{iter, num::NonZero};

use bevy_camera::Camera;
use bevy_ecs::{entity::EntityHashMap, prelude::*};
use bevy_light::{
    cluster::{
        ClusterableObjectCounts, ClusterableObjects, Clusters, GlobalClusterGpuSettings,
        GlobalClusterSettings,
    },
    ClusteredDecal, EnvironmentMapLight, IrradianceVolume, PointLight, SpotLight,
};
use bevy_math::{uvec4, UVec3, UVec4, Vec4};
use bevy_render::{
    render_resource::{
        BindingResource, BufferBindingType, BufferUsages, DownlevelFlags, RawBufferVec, ShaderSize,
        ShaderType, StorageBuffer, UniformBuffer,
    },
    renderer::{RenderAdapter, RenderDevice, RenderQueue},
    sync_world::{MainEntity, RenderEntity},
    Extract,
};
use bytemuck::{Pod, Zeroable};
use tracing::{error, info, trace, warn};

use crate::{MeshPipeline, RenderViewLightProbes};

pub(crate) mod gpu;

pub const MAX_UNIFORM_BUFFER_CLUSTERABLE_OBJECTS: usize = 204;

const _: () =
    assert!(size_of::<GpuClusteredLight>() * MAX_UNIFORM_BUFFER_CLUSTERABLE_OBJECTS <= 16384);

pub const CLUSTERED_FORWARD_STORAGE_BUFFER_COUNT: u32 = 3;

const CLUSTER_COUNT_SIZE: u32 = 9;

const CLUSTER_OFFSET_MASK: u32 = (1 << (32 - (CLUSTER_COUNT_SIZE * 2))) - 1;
const CLUSTER_COUNT_MASK: u32 = (1 << CLUSTER_COUNT_SIZE) - 1;

pub const GPU_CLUSTERING_INITIAL_Z_SLICE_LIST_CAPACITY: usize = 1024;

pub const GPU_CLUSTERING_INITIAL_INDEX_LIST_CAPACITY: usize = 65536;

pub(crate) fn make_global_cluster_settings(world: &World) -> GlobalClusterSettings {
    let device = world.resource::<RenderDevice>();
    let adapter = world.resource::<RenderAdapter>();
    let clustered_decals_are_usable =
        crate::decal::clustered::clustered_decals_are_usable(device, adapter);
    let supports_storage_buffers = matches!(
        device.get_supported_read_only_binding_type(CLUSTERED_FORWARD_STORAGE_BUFFER_COUNT),
        BufferBindingType::Storage { .. }
    );

    let gpu_clustering_supported = !(cfg!(target_os = "android") || cfg!(target_abi = "sim"))
        && adapter
            .get_downlevel_capabilities()
            .flags
            .contains(DownlevelFlags::COMPUTE_SHADERS)
        && device.limits().max_storage_buffers_per_shader_stage > 0;

    let gpu_clustering = if gpu_clustering_supported {
        info!("GPU clustering is supported on this device.");
        Some(GlobalClusterGpuSettings {
            initial_z_slice_list_capacity: GPU_CLUSTERING_INITIAL_Z_SLICE_LIST_CAPACITY,
            initial_index_list_capacity: GPU_CLUSTERING_INITIAL_INDEX_LIST_CAPACITY,
        })
    } else {
        info!("GPU clustering isn't supported on this device; falling back to CPU clustering.");
        None
    };

    GlobalClusterSettings {
        supports_storage_buffers,
        clustered_decals_are_usable,
        gpu_clustering,
        max_uniform_buffer_clusterable_objects: MAX_UNIFORM_BUFFER_CLUSTERABLE_OBJECTS,
        view_cluster_bindings_max_indices: ViewClusterBindings::MAX_INDICES,
    }
}

#[derive(Copy, Clone, ShaderType, Default, Pod, Zeroable, Debug)]
#[repr(C)]
pub struct GpuClusteredLight {

    pub(crate) light_custom_data: Vec4,
    pub(crate) color_inverse_square_range: Vec4,
    pub(crate) position_radius: Vec4,
    pub(crate) flags: u32,
    pub(crate) shadow_depth_bias: f32,
    pub(crate) shadow_normal_bias: f32,
    pub(crate) spot_light_tan_angle: f32,
    pub(crate) soft_shadow_size: f32,
    pub(crate) shadow_map_near_z: f32,

    pub(crate) decal_index: u32,

    pub(crate) range: f32,
}

#[derive(Resource)]
pub struct GlobalClusterableObjectMeta {

    pub gpu_clustered_lights: GpuClusteredLights,

    pub entity_to_index: EntityHashMap<usize>,
}

pub struct GpuClusteredLights {
    data: RawBufferVec<GpuClusteredLight>,
    is_storage_buffer: bool,
}

#[derive(Component)]
pub struct ExtractedClusterConfig {

    pub(crate) near: f32,
    pub(crate) far: f32,

    pub(crate) dimensions: UVec3,
}

impl<'a> From<&'a Clusters> for ExtractedClusterConfig {
    fn from(clusters: &'a Clusters) -> Self {
        Self {
            near: clusters.near,
            far: clusters.far,
            dimensions: clusters.dimensions,
        }
    }
}

enum ExtractedClusterableObjectElement {

    ClusterHeader(ClusterableObjectCounts),

    Light(Entity),

    ReflectionProbe(MainEntity),

    IrradianceVolume(MainEntity),

    Decal(Entity),
}

#[derive(Component)]
pub struct ExtractedClusterableObjects {
    data: Vec<ExtractedClusterableObjectElement>,
}

#[derive(ShaderType)]
struct GpuClusterOffsetsAndCountsUniform {
    data: Box<[UVec4; ViewClusterBindings::MAX_UNIFORM_ITEMS]>,
}

#[derive(ShaderType, Default)]
struct GpuClusterableObjectIndexListsStorage {
    #[shader(size(runtime))]
    data: Vec<u32>,
}

#[derive(ShaderType, Default)]
struct GpuClusterOffsetsAndCountsStorage {

    #[shader(size(runtime))]
    data: Vec<GpuClusterOffsetAndCounts>,
}

type GpuClusterOffsetAndCounts = [UVec4; 2];

enum ViewClusterBuffers {
    Uniform {

        clusterable_object_index_lists: UniformBuffer<GpuClusterableObjectIndexListsUniform>,

        cluster_offsets_and_counts: UniformBuffer<GpuClusterOffsetsAndCountsUniform>,
    },
    Storage {
        clusterable_object_index_lists: StorageBuffer<GpuClusterableObjectIndexListsStorage>,
        cluster_offsets_and_counts: StorageBuffer<GpuClusterOffsetsAndCountsStorage>,
    },
}

#[derive(Component)]
pub struct ViewClusterBindings {
    n_indices: usize,
    n_offsets: usize,
    buffers: ViewClusterBuffers,
}

pub fn init_global_clusterable_object_meta(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
) {
    commands.insert_resource(GlobalClusterableObjectMeta::new(
        render_device.get_supported_read_only_binding_type(CLUSTERED_FORWARD_STORAGE_BUFFER_COUNT),
    ));
}

impl GlobalClusterableObjectMeta {
    pub fn new(buffer_binding_type: BufferBindingType) -> Self {
        Self {
            gpu_clustered_lights: GpuClusteredLights::new(buffer_binding_type),
            entity_to_index: EntityHashMap::default(),
        }
    }
}

impl GpuClusteredLights {
    fn new(buffer_binding_type: BufferBindingType) -> Self {
        match buffer_binding_type {
            BufferBindingType::Storage { .. } => Self::storage(),
            BufferBindingType::Uniform => Self::uniform(),
        }
    }

    fn uniform() -> Self {
        GpuClusteredLights {
            data: RawBufferVec::new(BufferUsages::UNIFORM),
            is_storage_buffer: false,
        }
    }

    fn storage() -> Self {
        GpuClusteredLights {
            data: RawBufferVec::new(BufferUsages::STORAGE),
            is_storage_buffer: true,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.data.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }

    pub(crate) fn add(&mut self, light: GpuClusteredLight) {
        if self.is_storage_buffer || self.data.len() < MAX_UNIFORM_BUFFER_CLUSTERABLE_OBJECTS {
            self.data.push(light);
        }
    }

    pub(crate) fn write_buffer(
        &mut self,
        render_device: &RenderDevice,
        render_queue: &RenderQueue,
    ) {
        if self.is_storage_buffer {
            if self.data.is_empty() {
                self.data.push(GpuClusteredLight::default());
            }
        } else {
            while self.data.len() < MAX_UNIFORM_BUFFER_CLUSTERABLE_OBJECTS {
                self.data.push(GpuClusteredLight::default());
            }
        }

        self.data.write_buffer(render_device, render_queue);
    }

    pub fn binding(&self) -> Option<BindingResource<'_>> {
        self.data.binding()
    }

    pub fn min_size(buffer_binding_type: BufferBindingType) -> NonZero<u64> {
        match buffer_binding_type {
            BufferBindingType::Storage { .. } => GpuClusteredLight::min_size(),
            BufferBindingType::Uniform => NonZero::try_from(
                u64::from(GpuClusteredLight::min_size())
                    * MAX_UNIFORM_BUFFER_CLUSTERABLE_OBJECTS as u64,
            )
            .unwrap(),
        }
    }

    pub fn max_clustered_lights(&self) -> Option<usize> {
        if self.is_storage_buffer {
            None
        } else {
            Some(MAX_UNIFORM_BUFFER_CLUSTERABLE_OBJECTS)
        }
    }
}

type ClusterExtractionMapperQueryFlags = (
    Has<PointLight>,
    Has<SpotLight>,
    Has<EnvironmentMapLight>,
    Has<IrradianceVolume>,
    Has<ClusteredDecal>,
);

type ClusterExtractionMapperQueryFilter = Or<(
    With<PointLight>,
    With<SpotLight>,
    With<EnvironmentMapLight>,
    With<IrradianceVolume>,
    With<ClusteredDecal>,
)>;

pub fn gpu_clustering_is_enabled_during_extraction(
    global_cluster_settings: Extract<Res<GlobalClusterSettings>>,
) -> bool {
    global_cluster_settings.gpu_clustering.is_some()
}

pub fn gpu_clustering_is_enabled(global_cluster_settings: Res<GlobalClusterSettings>) -> bool {
    global_cluster_settings.gpu_clustering.is_some()
}

pub fn extract_clusters_for_cpu_clustering(
    mut commands: Commands,
    views: Extract<Query<(RenderEntity, &Clusters, &Camera)>>,
    mapper: Extract<
        Query<
            (Option<&RenderEntity>, ClusterExtractionMapperQueryFlags),
            ClusterExtractionMapperQueryFilter,
        >,
    >,
    global_cluster_settings: Extract<Res<GlobalClusterSettings>>,
) {
    for (entity, clusters, camera) in &views {
        let mut entity_commands = commands
            .get_entity(entity)
            .expect("Clusters entity wasn't synced.");
        if !camera.is_active {
            entity_commands.remove::<(ExtractedClusterableObjects, ExtractedClusterConfig)>();
            continue;
        }

        let clusterable_objects = match clusters.clusterable_objects {
            ClusterableObjects::Cpu(ref cpu_clusterable_objects) => cpu_clusterable_objects,
            ClusterableObjects::Gpu => {
                error!("Clusterable objects must have been in CPU mode if doing CPU clustering");
                continue;
            }
        };

        let mut data = vec![];
        for cluster_objects in clusterable_objects {
            data.push(ExtractedClusterableObjectElement::ClusterHeader(
                cluster_objects.counts,
            ));
            for clusterable_entity in cluster_objects.iter() {
                let Ok((
                    maybe_render_entity,
                    (
                        is_point_light,
                        is_spot_light,
                        is_reflection_probe,
                        is_irradiance_volume,
                        is_clustered_decal,
                    ),
                )) = mapper.get(*clusterable_entity)
                else {
                    error!(
                        "Couldn't find clustered object {:?} in the main world",
                        clusterable_entity
                    );
                    continue;
                };

                if let Some(render_entity) = maybe_render_entity {
                    if is_clustered_decal {
                        data.push(ExtractedClusterableObjectElement::Decal(**render_entity));
                    } else if is_point_light || is_spot_light {
                        data.push(ExtractedClusterableObjectElement::Light(**render_entity));
                    }
                }
                if is_reflection_probe {
                    data.push(ExtractedClusterableObjectElement::ReflectionProbe(
                        MainEntity::from(*clusterable_entity),
                    ));
                }
                if is_irradiance_volume {
                    data.push(ExtractedClusterableObjectElement::IrradianceVolume(
                        MainEntity::from(*clusterable_entity),
                    ));
                }
            }
        }

        entity_commands.insert((
            ExtractedClusterableObjects { data },
            ExtractedClusterConfig::from(clusters),
        ));
    }

    commands.insert_resource(global_cluster_settings.clone());
}

pub fn prepare_clusters_for_cpu_clustering(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mesh_pipeline: Res<MeshPipeline>,
    global_clusterable_object_meta: Res<GlobalClusterableObjectMeta>,
    views: Query<(
        Entity,
        &ExtractedClusterableObjects,
        Option<&RenderViewLightProbes<EnvironmentMapLight>>,
        Option<&RenderViewLightProbes<IrradianceVolume>>,
    )>,
) {
    let render_device = render_device.into_inner();
    let supports_storage_buffers = matches!(
        mesh_pipeline.clustered_forward_buffer_binding_type,
        BufferBindingType::Storage { .. }
    );
    for (entity, extracted_clusters, maybe_environment_maps, maybe_irradiance_volumes) in &views {
        let mut view_clusters_bindings =
            ViewClusterBindings::new(mesh_pipeline.clustered_forward_buffer_binding_type);
        view_clusters_bindings.clear();

        for record in &extracted_clusters.data {
            match record {
                ExtractedClusterableObjectElement::ClusterHeader(counts) => {
                    let offset = view_clusters_bindings.n_indices();
                    view_clusters_bindings.push_offset_and_counts(offset, counts);
                }

                ExtractedClusterableObjectElement::Light(entity)
                | ExtractedClusterableObjectElement::Decal(entity) => {
                    if let Some(clusterable_object_index) =
                        global_clusterable_object_meta.entity_to_index.get(entity)
                    {
                        if view_clusters_bindings.n_indices() >= ViewClusterBindings::MAX_INDICES
                            && !supports_storage_buffers
                        {
                            warn!(
                                "Clusterable object index lists are full! The clusterable \
                                 objects in the view are present in too many clusters."
                            );
                            break;
                        }
                        view_clusters_bindings.push_index(*clusterable_object_index);
                    } else {

                        error!(
                            "Clustered light or decal {:?} had no assigned index!",
                            entity
                        );

                        debug_assert!(false);
                        view_clusters_bindings.push_dummy_index();
                    }
                }

                ExtractedClusterableObjectElement::ReflectionProbe(main_entity) => {
                    match maybe_environment_maps.and_then(|environment_maps| {
                        environment_maps
                            .main_entity_to_render_light_probe_index
                            .get(main_entity)
                    }) {
                        Some(render_light_probe_index) => {
                            view_clusters_bindings.push_index(*render_light_probe_index as usize);
                        }
                        None => {

                            trace!(
                                "Clustered reflection probe {:?} had no assigned index",
                                main_entity,
                            );
                            view_clusters_bindings.push_dummy_index();
                        }
                    }
                }

                ExtractedClusterableObjectElement::IrradianceVolume(main_entity) => {
                    match maybe_irradiance_volumes.and_then(|irradiance_volumes| {
                        irradiance_volumes
                            .main_entity_to_render_light_probe_index
                            .get(main_entity)
                    }) {
                        Some(render_light_probe_index) => {
                            view_clusters_bindings.push_index(*render_light_probe_index as usize);
                        }
                        None => {
                            trace!(
                                "Clustered irradiance volume {:?} had no assigned index",
                                main_entity
                            );
                            view_clusters_bindings.push_dummy_index();
                        }
                    }
                }
            }
        }

        view_clusters_bindings.write_buffers(render_device, &render_queue);

        commands.entity(entity).insert(view_clusters_bindings);
    }
}

impl ViewClusterBindings {
    pub const MAX_OFFSETS: usize = 16384 / 4;
    const MAX_UNIFORM_ITEMS: usize = Self::MAX_OFFSETS / 4;
    pub const MAX_INDICES: usize = 16384;

    pub fn new(buffer_binding_type: BufferBindingType) -> Self {
        Self {
            n_indices: 0,
            n_offsets: 0,
            buffers: ViewClusterBuffers::new(buffer_binding_type),
        }
    }

    pub fn clear(&mut self) {
        match &mut self.buffers {
            ViewClusterBuffers::Uniform {
                clusterable_object_index_lists,
                cluster_offsets_and_counts,
            } => {
                *clusterable_object_index_lists.get_mut().data =
                    [UVec4::ZERO; Self::MAX_UNIFORM_ITEMS];
                *cluster_offsets_and_counts.get_mut().data = [UVec4::ZERO; Self::MAX_UNIFORM_ITEMS];
            }
            ViewClusterBuffers::Storage {
                clusterable_object_index_lists,
                cluster_offsets_and_counts,
                ..
            } => {
                clusterable_object_index_lists.get_mut().data.clear();
                cluster_offsets_and_counts.get_mut().data.clear();
            }
        }
    }

    fn push_offset_and_counts(&mut self, offset: usize, counts: &ClusterableObjectCounts) {
        match &mut self.buffers {
            ViewClusterBuffers::Uniform {
                cluster_offsets_and_counts,
                ..
            } => {
                let array_index = self.n_offsets >> 2;
                if array_index >= Self::MAX_UNIFORM_ITEMS {
                    warn!("cluster offset and count out of bounds!");
                    return;
                }
                let component = self.n_offsets & ((1 << 2) - 1);
                let packed =
                    pack_offset_and_counts(offset, counts.point_lights, counts.spot_lights);

                cluster_offsets_and_counts.get_mut().data[array_index][component] = packed;
            }
            ViewClusterBuffers::Storage {
                cluster_offsets_and_counts,
                ..
            } => {
                cluster_offsets_and_counts.get_mut().data.push([
                    uvec4(
                        offset as u32,
                        counts.point_lights,
                        counts.spot_lights,
                        counts.reflection_probes,
                    ),
                    uvec4(counts.irradiance_volumes, counts.decals, 0, 0),
                ]);
            }
        }

        self.n_offsets += 1;
    }

    pub fn n_indices(&self) -> usize {
        self.n_indices
    }

    fn push_raw_index(&mut self, index: u32) {
        match &mut self.buffers {
            ViewClusterBuffers::Uniform {
                clusterable_object_index_lists,
                ..
            } => {
                let array_index = self.n_indices >> 4;
                let component = (self.n_indices >> 2) & ((1 << 2) - 1);
                let sub_index = self.n_indices & ((1 << 2) - 1);

                clusterable_object_index_lists.get_mut().data[array_index][component] |=
                    index << (8 * sub_index);
            }
            ViewClusterBuffers::Storage {
                clusterable_object_index_lists,
                ..
            } => {
                clusterable_object_index_lists.get_mut().data.push(index);
            }
        }

        self.n_indices += 1;
    }

    pub fn push_index(&mut self, index: usize) {
        self.push_raw_index(index as u32);
    }

    pub fn push_dummy_index(&mut self) {
        self.push_raw_index(!0);
    }

    pub fn reserve_clusters(&mut self, clusters: usize) {
        match &mut self.buffers {
            ViewClusterBuffers::Uniform { .. } => {
                error!("`reserve_clusters` should only be called in GPU clustering, which requires a storage buffer");
            }
            ViewClusterBuffers::Storage {
                cluster_offsets_and_counts,
                ..
            } => {
                cluster_offsets_and_counts
                    .get_mut()
                    .data
                    .extend(iter::repeat_n(
                        GpuClusterOffsetAndCounts::default(),
                        clusters,
                    ));
                self.n_offsets += clusters;
            }
        }
    }

    pub fn reserve_indices(&mut self, elements: usize) {
        match &mut self.buffers {
            ViewClusterBuffers::Uniform { .. } => {
                error!("`reserve_indices` should only be called in GPU clustering, which requires a storage buffer");
            }
            ViewClusterBuffers::Storage {
                clusterable_object_index_lists,
                ..
            } => {
                clusterable_object_index_lists
                    .get_mut()
                    .data
                    .extend(iter::repeat_n(0, elements));
                self.n_indices += elements;
            }
        }
    }

    pub fn write_buffers(&mut self, render_device: &RenderDevice, render_queue: &RenderQueue) {
        match &mut self.buffers {
            ViewClusterBuffers::Uniform {
                clusterable_object_index_lists,
                cluster_offsets_and_counts,
            } => {
                clusterable_object_index_lists.write_buffer(render_device, render_queue);
                cluster_offsets_and_counts.write_buffer(render_device, render_queue);
            }
            ViewClusterBuffers::Storage {
                clusterable_object_index_lists,
                cluster_offsets_and_counts,
            } => {
                clusterable_object_index_lists.write_buffer(render_device, render_queue);
                cluster_offsets_and_counts.write_buffer(render_device, render_queue);
            }
        }
    }

    pub fn clusterable_object_index_lists_binding(&self) -> Option<BindingResource<'_>> {
        match &self.buffers {
            ViewClusterBuffers::Uniform {
                clusterable_object_index_lists,
                ..
            } => clusterable_object_index_lists.binding(),
            ViewClusterBuffers::Storage {
                clusterable_object_index_lists,
                ..
            } => clusterable_object_index_lists.binding(),
        }
    }

    pub fn offsets_and_counts_binding(&self) -> Option<BindingResource<'_>> {
        match &self.buffers {
            ViewClusterBuffers::Uniform {
                cluster_offsets_and_counts,
                ..
            } => cluster_offsets_and_counts.binding(),
            ViewClusterBuffers::Storage {
                cluster_offsets_and_counts,
                ..
            } => cluster_offsets_and_counts.binding(),
        }
    }

    pub fn min_size_clusterable_object_index_lists(
        buffer_binding_type: BufferBindingType,
    ) -> NonZero<u64> {
        match buffer_binding_type {
            BufferBindingType::Storage { .. } => GpuClusterableObjectIndexListsStorage::min_size(),
            BufferBindingType::Uniform => GpuClusterableObjectIndexListsUniform::min_size(),
        }
    }

    pub fn min_size_cluster_offsets_and_counts(
        buffer_binding_type: BufferBindingType,
    ) -> NonZero<u64> {
        match buffer_binding_type {
            BufferBindingType::Storage { .. } => GpuClusterOffsetsAndCountsStorage::min_size(),
            BufferBindingType::Uniform => GpuClusterOffsetsAndCountsUniform::min_size(),
        }
    }
}

impl ViewClusterBuffers {
    fn new(buffer_binding_type: BufferBindingType) -> Self {
        match buffer_binding_type {
            BufferBindingType::Storage { .. } => Self::storage(),
            BufferBindingType::Uniform => Self::uniform(),
        }
    }

    fn uniform() -> Self {
        ViewClusterBuffers::Uniform {
            clusterable_object_index_lists: UniformBuffer::default(),
            cluster_offsets_and_counts: UniformBuffer::default(),
        }
    }

    fn storage() -> Self {
        ViewClusterBuffers::Storage {
            clusterable_object_index_lists: StorageBuffer::default(),
            cluster_offsets_and_counts: StorageBuffer::default(),
        }
    }
}

fn pack_offset_and_counts(offset: usize, point_count: u32, spot_count: u32) -> u32 {
    ((offset as u32 & CLUSTER_OFFSET_MASK) << (CLUSTER_COUNT_SIZE * 2))
        | ((point_count & CLUSTER_COUNT_MASK) << CLUSTER_COUNT_SIZE)
        | (spot_count & CLUSTER_COUNT_MASK)
}

#[derive(ShaderType)]
struct GpuClusterableObjectIndexListsUniform {
    data: Box<[UVec4; ViewClusterBindings::MAX_UNIFORM_ITEMS]>,
}

const _: () = assert!(GpuClusterableObjectIndexListsUniform::SHADER_SIZE.get() <= 16384);

impl Default for GpuClusterableObjectIndexListsUniform {
    fn default() -> Self {
        Self {
            data: Box::new([UVec4::ZERO; ViewClusterBindings::MAX_UNIFORM_ITEMS]),
        }
    }
}

impl Default for GpuClusterOffsetsAndCountsUniform {
    fn default() -> Self {
        Self {
            data: Box::new([UVec4::ZERO; ViewClusterBindings::MAX_UNIFORM_ITEMS]),
        }
    }
}
