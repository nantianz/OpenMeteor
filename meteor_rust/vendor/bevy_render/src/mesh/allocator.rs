

use alloc::borrow::Cow;
use bevy_mesh::Indices;

use bevy_app::{App, Plugin};
use bevy_asset::AssetId;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    resource::Resource,
    schedule::IntoScheduleConfigs as _,
    system::{Res, ResMut},
    world::{FromWorld, World},
};
use wgpu::{BufferUsages, DownlevelFlags, COPY_BUFFER_ALIGNMENT};

#[cfg(feature = "morph")]
use bevy_mesh::morph::MorphAttributes;

use crate::{
    mesh::{Mesh, MeshVertexBufferLayouts, RenderMesh},
    render_asset::{prepare_assets, ExtractedAssets},
    renderer::{RenderAdapter, RenderDevice, RenderQueue},
    slab_allocator::{
        Slab, SlabAllocationBufferSlice, SlabAllocator, SlabAllocatorSettings, SlabId, SlabItem,
        SlabItemLayout,
    },
    GpuResourceAppExt, Render, RenderApp, RenderSystems,
};

pub struct MeshAllocatorPlugin;

#[derive(Resource, Deref, DerefMut)]
pub struct MeshAllocator {

    #[deref]
    slab_allocator: SlabAllocator<MeshSlabItem>,

    general_vertex_slabs_supported: bool,
}

#[derive(Resource, Deref, DerefMut)]
pub struct MeshAllocatorSettings {
    #[deref]
    pub slab_allocator_settings: SlabAllocatorSettings,

    pub extra_buffer_usages: BufferUsages,
}

impl Default for MeshAllocatorSettings {
    fn default() -> MeshAllocatorSettings {
        MeshAllocatorSettings {
            slab_allocator_settings: SlabAllocatorSettings::default(),
            extra_buffer_usages: BufferUsages::empty(),
        }
    }
}

#[cfg(feature = "morph")]
static MORPH_ATTRIBUTE_ELEMENT_LAYOUT: ElementLayout = ElementLayout {
    class: ElementClass::MorphTarget,
    size: size_of::<MorphAttributes>() as u64,
    elements_per_slot: 1,
};

pub type MeshSlabId = SlabId<MeshSlabItem>;

pub type MeshBufferSlice<'a> = SlabAllocationBufferSlice<'a, MeshSlabItem>;

pub struct MeshSlabItem;

impl SlabItem for MeshSlabItem {
    type Key = MeshAllocationKey;
    type Layout = ElementLayout;
    fn label() -> Cow<'static, str> {
        "mesh".into()
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct MeshSlabs {

    pub vertex_slab_id: MeshSlabId,

    pub index_slab_id: Option<MeshSlabId>,

    #[cfg(feature = "morph")]
    pub morph_target_slab_id: Option<MeshSlabId>,
}

impl Slab<MeshSlabItem> {

    #[cfg(feature = "morph")]
    pub fn element_class(&self) -> ElementClass {
        self.element_layout().class
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeshAllocationKey {

    pub mesh_id: AssetId<Mesh>,

    pub class: ElementClass,
}

impl MeshAllocationKey {

    pub fn new(mesh_id: AssetId<Mesh>, class: ElementClass) -> Self {
        Self { mesh_id, class }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ElementClass {

    Vertex,

    Index,
    #[cfg(feature = "morph")]

    MorphTarget,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementLayout {

    class: ElementClass,

    size: u64,

    elements_per_slot: u32,
}

impl Plugin for MeshAllocatorPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_resource::<MeshAllocatorSettings>()
            .add_systems(
                Render,
                allocate_and_free_meshes
                    .in_set(RenderSystems::PrepareAssets)
                    .before(prepare_assets::<RenderMesh>),
            );
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app.init_gpu_resource::<MeshAllocator>();
    }
}

impl FromWorld for MeshAllocator {
    fn from_world(world: &mut World) -> Self {

        let render_adapter = world.resource::<RenderAdapter>();
        let general_vertex_slabs_supported = render_adapter
            .get_downlevel_capabilities()
            .flags
            .contains(DownlevelFlags::BASE_VERTEX);

        let mesh_allocator_settings = world.resource::<MeshAllocatorSettings>();
        let mut slab_allocator = SlabAllocator::new();
        slab_allocator.extra_buffer_usages |= mesh_allocator_settings.extra_buffer_usages;

        Self {
            slab_allocator,
            general_vertex_slabs_supported,
        }
    }
}

pub fn allocate_and_free_meshes(
    mut mesh_allocator: ResMut<MeshAllocator>,
    mesh_allocator_settings: Res<MeshAllocatorSettings>,
    extracted_meshes: Res<ExtractedAssets<RenderMesh>>,
    mut mesh_vertex_buffer_layouts: ResMut<MeshVertexBufferLayouts>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {

    mesh_allocator.free_meshes(&extracted_meshes);

    mesh_allocator.allocate_meshes(
        &mesh_allocator_settings,
        &extracted_meshes,
        &mut mesh_vertex_buffer_layouts,
        &render_device,
        &render_queue,
    );
}

impl MeshAllocator {

    pub fn mesh_vertex_slice(&self, mesh_id: &AssetId<Mesh>) -> Option<MeshBufferSlice<'_>> {
        self.slab_allocation_slice(
            &MeshAllocationKey::new(*mesh_id, ElementClass::Vertex),
            *self.mesh_id_to_vertex_slab(mesh_id)?,
        )
    }

    pub fn mesh_index_slice(&self, mesh_id: &AssetId<Mesh>) -> Option<MeshBufferSlice<'_>> {
        self.slab_allocation_slice(
            &MeshAllocationKey::new(*mesh_id, ElementClass::Index),
            *self.mesh_id_to_index_slab(mesh_id)?,
        )
    }

    #[cfg(feature = "morph")]
    pub fn mesh_morph_target_slice(&self, mesh_id: &AssetId<Mesh>) -> Option<MeshBufferSlice<'_>> {
        self.slab_allocation_slice(
            &MeshAllocationKey::new(*mesh_id, ElementClass::MorphTarget),
            *self.mesh_id_to_morph_target_slab(mesh_id)?,
        )
    }

    pub fn mesh_slabs(&self, mesh_id: &AssetId<Mesh>) -> Option<MeshSlabs> {
        Some(MeshSlabs {
            vertex_slab_id: self.mesh_id_to_vertex_slab(mesh_id).cloned()?,
            index_slab_id: self.mesh_id_to_index_slab(mesh_id).cloned(),
            #[cfg(feature = "morph")]
            morph_target_slab_id: self.mesh_id_to_morph_target_slab(mesh_id).cloned(),
        })
    }

    pub fn index_allocation_count(&self) -> usize {
        self.key_to_slab
            .keys()
            .filter(|key| key.class == ElementClass::Index)
            .count()
    }

    fn mesh_id_to_vertex_slab(&self, mesh_id: &AssetId<Mesh>) -> Option<&SlabId<MeshSlabItem>> {
        self.key_to_slab
            .get(&MeshAllocationKey::new(*mesh_id, ElementClass::Vertex))
    }

    fn mesh_id_to_index_slab(&self, mesh_id: &AssetId<Mesh>) -> Option<&SlabId<MeshSlabItem>> {
        self.key_to_slab
            .get(&MeshAllocationKey::new(*mesh_id, ElementClass::Index))
    }

    #[cfg(feature = "morph")]
    fn mesh_id_to_morph_target_slab(
        &self,
        mesh_id: &AssetId<Mesh>,
    ) -> Option<&SlabId<MeshSlabItem>> {
        self.key_to_slab
            .get(&MeshAllocationKey::new(*mesh_id, ElementClass::MorphTarget))
    }

    #[cfg(feature = "morph")]
    pub fn morph_target_slabs(&self) -> impl Iterator<Item = MeshSlabId> {
        self.slabs.iter().filter_map(|(slab_id, slab)| {
            if matches!(slab.element_class(), ElementClass::MorphTarget) {
                Some(*slab_id)
            } else {
                None
            }
        })
    }

    fn allocate_meshes(
        &mut self,
        mesh_allocator_settings: &MeshAllocatorSettings,
        extracted_meshes: &ExtractedAssets<RenderMesh>,
        mesh_vertex_buffer_layouts: &mut MeshVertexBufferLayouts,
        render_device: &RenderDevice,
        render_queue: &RenderQueue,
    ) {
        let mut allocation_stage = self.slab_allocator.stage_allocation();

        for (mesh_id, mesh) in &extracted_meshes.extracted {
            let vertex_buffer_size = mesh.get_vertex_buffer_size() as u64;
            if vertex_buffer_size == 0 {
                continue;
            }

            let vertex_element_layout = ElementLayout::vertex(mesh_vertex_buffer_layouts, mesh);
            if self.general_vertex_slabs_supported {
                allocation_stage.allocate(
                    &MeshAllocationKey::new(*mesh_id, ElementClass::Vertex),
                    vertex_buffer_size,
                    vertex_element_layout,
                    mesh_allocator_settings,
                );
            } else {
                allocation_stage.allocate_large(
                    &MeshAllocationKey::new(*mesh_id, ElementClass::Vertex),
                    vertex_element_layout,
                );
            }

            if let (Some(index_buffer_data), Some(index_element_layout)) =
                (mesh.get_index_buffer_bytes(), ElementLayout::index(mesh))
            {
                allocation_stage.allocate(
                    &MeshAllocationKey::new(*mesh_id, ElementClass::Index),
                    index_buffer_data.len() as u64,
                    index_element_layout,
                    mesh_allocator_settings,
                );
            }

            #[cfg(feature = "morph")]
            if let Some(morph_targets) = mesh.get_morph_targets() {
                allocation_stage.allocate(
                    &MeshAllocationKey::new(*mesh_id, ElementClass::MorphTarget),
                    morph_targets.len() as u64 * size_of::<MorphAttributes>() as u64,
                    MORPH_ATTRIBUTE_ELEMENT_LAYOUT,
                    mesh_allocator_settings,
                );
            }
        }

        allocation_stage.commit(render_device, render_queue);

        for (mesh_id, mesh) in &extracted_meshes.extracted {
            self.copy_mesh_vertex_data(mesh_id, mesh, render_device, render_queue);
            self.copy_mesh_index_data(mesh_id, mesh, render_device, render_queue);
            #[cfg(feature = "morph")]
            self.copy_mesh_morph_target_data(mesh_id, mesh, render_device, render_queue);
        }
    }

    fn copy_mesh_vertex_data(
        &mut self,
        mesh_id: &AssetId<Mesh>,
        mesh: &Mesh,
        render_device: &RenderDevice,
        render_queue: &RenderQueue,
    ) {

        self.copy_element_data(
            &MeshAllocationKey::new(*mesh_id, ElementClass::Vertex),
            mesh.get_vertex_buffer_size(),
            |slice| mesh.write_packed_vertex_buffer_data(slice),
            render_device,
            render_queue,
        );
    }

    fn copy_mesh_index_data(
        &mut self,
        mesh_id: &AssetId<Mesh>,
        mesh: &Mesh,
        render_device: &RenderDevice,
        render_queue: &RenderQueue,
    ) {
        let Some(index_data) = mesh.get_index_buffer_bytes() else {
            return;
        };

        self.copy_element_data(
            &MeshAllocationKey::new(*mesh_id, ElementClass::Index),
            index_data.len(),
            |mut slice| slice.copy_from_slice(index_data),
            render_device,
            render_queue,
        );
    }

    #[cfg(feature = "morph")]
    fn copy_mesh_morph_target_data(
        &mut self,
        mesh_id: &AssetId<Mesh>,
        mesh: &Mesh,
        render_device: &RenderDevice,
        render_queue: &RenderQueue,
    ) {
        let Some(morph_targets) = mesh.get_morph_targets() else {
            return;
        };

        self.copy_element_data(
            &MeshAllocationKey::new(*mesh_id, ElementClass::MorphTarget),
            size_of_val(morph_targets),
            |mut slice| slice.copy_from_slice(bytemuck::cast_slice(morph_targets)),
            render_device,
            render_queue,
        );
    }

    fn free_meshes(&mut self, extracted_meshes: &ExtractedAssets<RenderMesh>) {
        let mut deallocation_stage = self.slab_allocator.stage_deallocation();

        let meshes_to_free = extracted_meshes
            .removed
            .iter()
            .chain(extracted_meshes.modified.iter())
            .chain(extracted_meshes.added.iter());

        for mesh_id in meshes_to_free {
            deallocation_stage.free(&MeshAllocationKey::new(*mesh_id, ElementClass::Vertex));
            deallocation_stage.free(&MeshAllocationKey::new(*mesh_id, ElementClass::Index));
            #[cfg(feature = "morph")]
            deallocation_stage.free(&MeshAllocationKey::new(*mesh_id, ElementClass::MorphTarget));
        }

        deallocation_stage.commit();
    }
}

impl ElementLayout {

    fn new(class: ElementClass, size: u64) -> ElementLayout {
        const {
            assert!(4 == COPY_BUFFER_ALIGNMENT);
        }

        let elements_per_slot = [1, 4, 2, 4][size as usize & 3];
        ElementLayout {
            class,
            size,

            elements_per_slot,
        }
    }

    fn vertex(
        mesh_vertex_buffer_layouts: &mut MeshVertexBufferLayouts,
        mesh: &Mesh,
    ) -> ElementLayout {
        let mesh_vertex_buffer_layout =
            mesh.get_mesh_vertex_buffer_layout(mesh_vertex_buffer_layouts);
        ElementLayout::new(
            ElementClass::Vertex,
            mesh_vertex_buffer_layout.0.layout().array_stride,
        )
    }

    fn index(mesh: &Mesh) -> Option<ElementLayout> {
        let size = match mesh.indices()? {
            Indices::U16(_) => 2,
            Indices::U32(_) => 4,
        };
        Some(ElementLayout::new(ElementClass::Index, size))
    }
}

impl SlabItemLayout for ElementLayout {
    fn size(&self) -> u64 {
        self.size
    }

    fn elements_per_slot(&self) -> u32 {
        self.elements_per_slot
    }

    fn buffer_usages(&self) -> BufferUsages {
        self.class.buffer_usages()
    }
}

impl ElementClass {

    fn buffer_usages(&self) -> BufferUsages {
        match *self {
            ElementClass::Vertex => BufferUsages::VERTEX,
            ElementClass::Index => BufferUsages::INDEX,
            #[cfg(feature = "morph")]
            ElementClass::MorphTarget => BufferUsages::STORAGE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::create_dummy_device;
    use bevy_asset::{uuid::Uuid, RenderAssetUsages};
    use bevy_math::bounding::Aabb3d;
    use bevy_mesh::PrimitiveTopology;
    use glam::Vec3;

    fn test_mesh() -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32, 0.0, 0.0]; 64]);
        mesh
    }

    fn full_mesh() -> Mesh {
        let mut mesh = test_mesh();
        mesh.insert_indices(Indices::U32((0..64).collect()));
        mesh.final_aabb = Some(Aabb3d::new(Vec3::ZERO, Vec3::ONE));
        #[cfg(feature = "morph")]
        mesh.set_morph_targets(vec![MorphAttributes::default(); 64]);
        mesh
    }

    fn wider_vertex_mesh() -> Mesh {
        let mut mesh = test_mesh();
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32, 0.0, 1.0]; 64]);
        mesh
    }

    fn mesh_allocator(general_vertex_slabs_supported: bool) -> MeshAllocator {
        MeshAllocator {
            slab_allocator: SlabAllocator::new(),
            general_vertex_slabs_supported,
        }
    }

    fn small_slab_settings() -> MeshAllocatorSettings {
        MeshAllocatorSettings {
            slab_allocator_settings: SlabAllocatorSettings {
                min_slab_size: 1024,
                max_slab_size: 4096,
                large_threshold: 512,
                growth_factor: 1.5,
            },
            extra_buffer_usages: BufferUsages::empty(),
        }
    }

    fn mesh_id(id: u128) -> AssetId<Mesh> {
        AssetId::<Mesh>::Uuid {
            uuid: Uuid::from_u128(id),
        }
    }

    fn has_allocation(
        mesh_allocator: &MeshAllocator,
        mesh_id: AssetId<Mesh>,
        class: ElementClass,
    ) -> bool {
        mesh_allocator
            .key_to_slab
            .contains_key(&MeshAllocationKey::new(mesh_id, class))
    }

    fn extracted_mesh(id: AssetId<Mesh>, mesh: Mesh) -> ExtractedAssets<RenderMesh> {
        let mut extracted_meshes = ExtractedAssets::<RenderMesh>::default();
        extracted_meshes.extracted.push((id, mesh));
        extracted_meshes.added.insert(id);
        extracted_meshes
    }

    fn modified_mesh(id: AssetId<Mesh>, mesh: Mesh) -> ExtractedAssets<RenderMesh> {
        let mut extracted_meshes = extracted_mesh(id, mesh);
        extracted_meshes.modified.insert(id);
        extracted_meshes
    }

    #[test]
    fn free_meshes_releases_reextracted_meshes() {
        let (render_device, render_queue) = create_dummy_device();
        let settings = MeshAllocatorSettings::default();
        let mut mesh_vertex_buffer_layouts = MeshVertexBufferLayouts::default();
        let mut mesh_allocator = mesh_allocator(true);

        let mesh_id = mesh_id(1);
        let extracted_meshes = extracted_mesh(mesh_id, test_mesh());

        mesh_allocator.allocate_meshes(
            &settings,
            &extracted_meshes,
            &mut mesh_vertex_buffer_layouts,
            &render_device,
            &render_queue,
        );
        assert!(mesh_allocator.mesh_vertex_slice(&mesh_id).is_some());

        mesh_allocator.free_meshes(&extracted_meshes);

        assert!(
            mesh_allocator.key_to_slab.is_empty(),
            "a re-extracted mesh was not freed, so its old allocation would leak"
        );
        assert_eq!(mesh_allocator.slab_count(), 0);
    }

    #[test]
    fn free_meshes_releases_modified_meshes_that_were_not_reextracted() {
        let (render_device, render_queue) = create_dummy_device();
        let settings = MeshAllocatorSettings::default();
        let mut mesh_vertex_buffer_layouts = MeshVertexBufferLayouts::default();
        let mut mesh_allocator = mesh_allocator(true);

        let mesh_id = mesh_id(1);
        mesh_allocator.allocate_meshes(
            &settings,
            &extracted_mesh(mesh_id, test_mesh()),
            &mut mesh_vertex_buffer_layouts,
            &render_device,
            &render_queue,
        );
        assert!(mesh_allocator.mesh_vertex_slice(&mesh_id).is_some());

        let mut extracted_meshes = ExtractedAssets::<RenderMesh>::default();
        extracted_meshes.modified.insert(mesh_id);
        mesh_allocator.free_meshes(&extracted_meshes);

        assert!(
            mesh_allocator.key_to_slab.is_empty(),
            "a modified mesh that wasn't re-extracted was not freed"
        );
        assert_eq!(mesh_allocator.slab_count(), 0);
    }

    #[test]
    fn free_meshes_releases_every_element_class() {
        let (render_device, render_queue) = create_dummy_device();
        let settings = MeshAllocatorSettings::default();
        let mut mesh_vertex_buffer_layouts = MeshVertexBufferLayouts::default();
        let mut mesh_allocator = mesh_allocator(true);

        let mesh_id = mesh_id(1);
        let extracted_meshes = extracted_mesh(mesh_id, full_mesh());
        mesh_allocator.allocate_meshes(
            &settings,
            &extracted_meshes,
            &mut mesh_vertex_buffer_layouts,
            &render_device,
            &render_queue,
        );

        assert!(has_allocation(
            &mesh_allocator,
            mesh_id,
            ElementClass::Vertex
        ));
        assert!(has_allocation(
            &mesh_allocator,
            mesh_id,
            ElementClass::Index
        ));
        #[cfg(feature = "morph")]
        assert!(has_allocation(
            &mesh_allocator,
            mesh_id,
            ElementClass::MorphTarget
        ));

        mesh_allocator.free_meshes(&extracted_meshes);

        assert!(
            mesh_allocator.key_to_slab.is_empty(),
            "at least one element class was left allocated, so it would leak"
        );
        assert_eq!(mesh_allocator.slab_count(), 0);
    }

    #[test]
    fn reextracting_a_mesh_that_drops_its_extra_data_frees_those_allocations() {
        let (render_device, render_queue) = create_dummy_device();
        let settings = MeshAllocatorSettings::default();
        let mut mesh_vertex_buffer_layouts = MeshVertexBufferLayouts::default();
        let mut mesh_allocator = mesh_allocator(true);

        let mesh_id = mesh_id(1);
        mesh_allocator.allocate_meshes(
            &settings,
            &extracted_mesh(mesh_id, full_mesh()),
            &mut mesh_vertex_buffer_layouts,
            &render_device,
            &render_queue,
        );
        assert!(has_allocation(
            &mesh_allocator,
            mesh_id,
            ElementClass::Index
        ));

        let extracted_meshes = extracted_mesh(mesh_id, test_mesh());
        mesh_allocator.free_meshes(&extracted_meshes);
        mesh_allocator.allocate_meshes(
            &settings,
            &extracted_meshes,
            &mut mesh_vertex_buffer_layouts,
            &render_device,
            &render_queue,
        );

        assert!(has_allocation(
            &mesh_allocator,
            mesh_id,
            ElementClass::Vertex
        ));
        assert!(
            !has_allocation(&mesh_allocator, mesh_id, ElementClass::Index),
            "index data was dropped by the mesh but its allocation survived"
        );
        #[cfg(feature = "morph")]
        assert!(
            !has_allocation(&mesh_allocator, mesh_id, ElementClass::MorphTarget),
            "morph targets were dropped by the mesh but their allocation survived"
        );
    }

    #[test]
    fn reextracting_a_mesh_with_a_new_vertex_layout_reclaims_the_old_slab() {
        let (render_device, render_queue) = create_dummy_device();
        let settings = MeshAllocatorSettings::default();
        let mut mesh_vertex_buffer_layouts = MeshVertexBufferLayouts::default();
        let mut mesh_allocator = mesh_allocator(true);

        let mesh_id = mesh_id(1);
        mesh_allocator.allocate_meshes(
            &settings,
            &extracted_mesh(mesh_id, test_mesh()),
            &mut mesh_vertex_buffer_layouts,
            &render_device,
            &render_queue,
        );
        assert_eq!(mesh_allocator.slab_count(), 1);
        let original_slab =
            mesh_allocator.key_to_slab[&MeshAllocationKey::new(mesh_id, ElementClass::Vertex)];

        let extracted_meshes = extracted_mesh(mesh_id, wider_vertex_mesh());
        mesh_allocator.free_meshes(&extracted_meshes);
        mesh_allocator.allocate_meshes(
            &settings,
            &extracted_meshes,
            &mut mesh_vertex_buffer_layouts,
            &render_device,
            &render_queue,
        );

        let new_slab =
            mesh_allocator.key_to_slab[&MeshAllocationKey::new(mesh_id, ElementClass::Vertex)];
        assert_ne!(
            new_slab, original_slab,
            "the wider vertex should have landed in a slab with a different layout"
        );
        assert_eq!(
            mesh_allocator.slab_count(),
            1,
            "the slab the mesh moved out of was not reclaimed"
        );
        assert!(mesh_allocator.mesh_vertex_slice(&mesh_id).is_some());
    }

    struct SteadyStateCase {

        build: fn(AssetId<Mesh>, Mesh) -> ExtractedAssets<RenderMesh>,
        build_mesh: fn() -> Mesh,
        settings: MeshAllocatorSettings,
        general_vertex_slabs_supported: bool,
    }

    impl Default for SteadyStateCase {
        fn default() -> Self {
            Self {
                build: extracted_mesh,
                build_mesh: test_mesh,
                settings: MeshAllocatorSettings::default(),
                general_vertex_slabs_supported: true,
            }
        }
    }

    fn assert_steady_state(case: SteadyStateCase, rounds: usize) {
        let SteadyStateCase {
            build,
            build_mesh,
            settings,
            general_vertex_slabs_supported,
        } = case;

        let (render_device, render_queue) = create_dummy_device();
        let mut mesh_vertex_buffer_layouts = MeshVertexBufferLayouts::default();
        let mut mesh_allocator = mesh_allocator(general_vertex_slabs_supported);

        let mesh_id = mesh_id(1);

        let mut baseline = None;
        for _ in 0..rounds {
            let extracted_meshes = build(mesh_id, build_mesh());
            mesh_allocator.free_meshes(&extracted_meshes);
            mesh_allocator.allocate_meshes(
                &settings,
                &extracted_meshes,
                &mut mesh_vertex_buffer_layouts,
                &render_device,
                &render_queue,
            );

            let size = mesh_allocator.slabs_size();
            let slab_count = mesh_allocator.slab_count();
            match baseline {
                None => baseline = Some((size, slab_count)),
                Some(baseline) => {
                    assert_eq!(
                        (size, slab_count),
                        baseline,
                        "slab memory grew across frames"
                    );
                }
            }
        }

        assert!(mesh_allocator.mesh_vertex_slice(&mesh_id).is_some());
    }

    #[test]
    fn reextracting_the_same_mesh_does_not_grow_the_slabs() {
        assert_steady_state(SteadyStateCase::default(), 32);
    }

    #[test]
    fn modifying_a_mesh_in_place_does_not_grow_the_slabs() {
        assert_steady_state(
            SteadyStateCase {
                build: modified_mesh,
                ..SteadyStateCase::default()
            },
            32,
        );
    }

    #[test]
    fn reextracting_a_mesh_with_every_element_class_does_not_grow_the_slabs() {
        assert_steady_state(
            SteadyStateCase {
                build_mesh: full_mesh,
                ..SteadyStateCase::default()
            },
            32,
        );
    }

    #[test]
    fn reextracting_a_mesh_too_large_for_a_general_slab_does_not_grow_the_slabs() {
        assert_steady_state(
            SteadyStateCase {
                build_mesh: full_mesh,
                settings: small_slab_settings(),
                ..SteadyStateCase::default()
            },
            32,
        );
    }

    #[test]
    fn reextracting_a_mesh_without_general_vertex_slabs_does_not_grow_the_slabs() {
        assert_steady_state(
            SteadyStateCase {
                build_mesh: full_mesh,
                general_vertex_slabs_supported: false,
                ..SteadyStateCase::default()
            },
            32,
        );
    }
}
