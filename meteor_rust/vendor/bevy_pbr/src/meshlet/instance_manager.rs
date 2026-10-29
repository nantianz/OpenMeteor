use super::{meshlet_mesh_manager::MeshletMeshManager, MeshletMesh, MeshletMesh3d};
use crate::DUMMY_MESH_MATERIAL;
use crate::{
    meshlet::asset::MeshletAabb, MaterialBindingId, MeshFlags, MeshTransforms, MeshUniform,
    PreviousGlobalTransform, RenderMaterialBindings, RenderMaterialInstances,
};
use bevy_asset::{AssetEvent, AssetServer, Assets, UntypedAssetId};
use bevy_camera::visibility::RenderLayers;
use bevy_ecs::{
    entity::{Entities, Entity, EntityHashMap},
    message::MessageReader,
    query::Has,
    resource::Resource,
    system::{Local, Query, Res, ResMut, SystemState},
};
use bevy_light::{NotShadowCaster, NotShadowReceiver};
use bevy_platform::collections::{HashMap, HashSet};
use bevy_render::{render_resource::StorageBuffer, sync_world::MainEntity, MainWorld};
use bevy_transform::components::GlobalTransform;
use core::ops::DerefMut;

#[derive(Resource)]
pub struct InstanceManager {

    pub scene_instance_count: u32,

    pub max_bvh_depth: u32,

    pub instances: Vec<(MainEntity, RenderLayers, bool)>,

    pub instance_uniforms: StorageBuffer<Vec<MeshUniform>>,

    pub instance_aabbs: StorageBuffer<Vec<MeshletAabb>>,

    pub instance_material_ids: StorageBuffer<Vec<u32>>,

    pub instance_bvh_root_nodes: StorageBuffer<Vec<u32>>,

    pub view_instance_visibility: EntityHashMap<StorageBuffer<Vec<u32>>>,

    next_material_id: u32,

    material_id_lookup: HashMap<UntypedAssetId, u32>,

    material_ids_present_in_scene: HashSet<u32>,
}

impl InstanceManager {
    pub fn new() -> Self {
        Self {
            scene_instance_count: 0,
            max_bvh_depth: 0,

            instances: Vec::new(),
            instance_uniforms: {
                let mut buffer = StorageBuffer::default();
                buffer.set_label(Some("meshlet_instance_uniforms"));
                buffer
            },
            instance_aabbs: {
                let mut buffer = StorageBuffer::default();
                buffer.set_label(Some("meshlet_instance_aabbs"));
                buffer
            },
            instance_material_ids: {
                let mut buffer = StorageBuffer::default();
                buffer.set_label(Some("meshlet_instance_material_ids"));
                buffer
            },
            instance_bvh_root_nodes: {
                let mut buffer = StorageBuffer::default();
                buffer.set_label(Some("meshlet_instance_bvh_root_nodes"));
                buffer
            },
            view_instance_visibility: EntityHashMap::default(),

            next_material_id: 0,
            material_id_lookup: HashMap::default(),
            material_ids_present_in_scene: HashSet::default(),
        }
    }

    pub fn add_instance(
        &mut self,
        instance: MainEntity,
        root_bvh_node: u32,
        aabb: MeshletAabb,
        bvh_depth: u32,
        transform: &GlobalTransform,
        previous_transform: Option<&PreviousGlobalTransform>,
        render_layers: Option<&RenderLayers>,
        mesh_material_ids: &RenderMaterialInstances,
        render_material_bindings: &RenderMaterialBindings,
        not_shadow_receiver: bool,
        not_shadow_caster: bool,
    ) {

        let transform = transform.affine();
        let previous_transform = previous_transform.map(|t| t.0).unwrap_or(transform);
        let mut flags = if not_shadow_receiver {
            MeshFlags::empty()
        } else {
            MeshFlags::SHADOW_RECEIVER
        };
        if transform.matrix3.determinant().is_sign_positive() {
            flags |= MeshFlags::SIGN_DETERMINANT_MODEL_3X3;
        }
        let transforms = MeshTransforms {
            world_from_local: transform.into(),
            previous_world_from_local: previous_transform.into(),
            flags: flags.bits(),
        };

        let mesh_material = mesh_material_ids.mesh_material(instance);
        let mesh_material_binding_id = if mesh_material != DUMMY_MESH_MATERIAL.untyped() {
            render_material_bindings
                .get(&mesh_material)
                .cloned()
                .unwrap_or_default()
        } else {

            MaterialBindingId::default()
        };

        let mesh_uniform = MeshUniform::new(
            &transforms,
            0,
            mesh_material_binding_id.slot,
            None,
            None,
            None,
            None,
        );

        self.instances.push((
            instance,
            render_layers.cloned().unwrap_or(RenderLayers::default()),
            not_shadow_caster,
        ));
        self.instance_uniforms.get_mut().push(mesh_uniform);
        self.instance_aabbs.get_mut().push(aabb);
        self.instance_material_ids.get_mut().push(0);
        self.instance_bvh_root_nodes.get_mut().push(root_bvh_node);

        self.scene_instance_count += 1;
        self.max_bvh_depth = self.max_bvh_depth.max(bvh_depth);
    }

    pub fn get_material_id(&mut self, material_asset_id: UntypedAssetId) -> u32 {
        *self
            .material_id_lookup
            .entry(material_asset_id)
            .or_insert_with(|| {
                self.next_material_id += 1;
                self.next_material_id
            })
    }

    pub fn material_present_in_scene(&self, material_id: &u32) -> bool {
        self.material_ids_present_in_scene.contains(material_id)
    }

    pub fn reset(&mut self, entities: &Entities) {
        self.scene_instance_count = 0;
        self.max_bvh_depth = 0;

        self.instances.clear();
        self.instance_uniforms.get_mut().clear();
        self.instance_aabbs.get_mut().clear();
        self.instance_material_ids.get_mut().clear();
        self.instance_bvh_root_nodes.get_mut().clear();
        self.view_instance_visibility
            .retain(|view_entity, _| entities.contains(*view_entity));
        self.view_instance_visibility
            .values_mut()
            .for_each(|b| b.get_mut().clear());

        self.next_material_id = 0;
        self.material_id_lookup.clear();
        self.material_ids_present_in_scene.clear();
    }
}

pub fn extract_meshlet_mesh_entities(
    mut meshlet_mesh_manager: ResMut<MeshletMeshManager>,
    mut instance_manager: ResMut<InstanceManager>,

    mut main_world: ResMut<MainWorld>,
    mesh_material_ids: Res<RenderMaterialInstances>,
    render_material_bindings: Res<RenderMaterialBindings>,
    mut system_state: Local<
        Option<
            SystemState<(
                Query<(
                    Entity,
                    &MeshletMesh3d,
                    &GlobalTransform,
                    Option<&PreviousGlobalTransform>,
                    Option<&RenderLayers>,
                    Has<NotShadowReceiver>,
                    Has<NotShadowCaster>,
                )>,
                Res<AssetServer>,
                ResMut<Assets<MeshletMesh>>,
                MessageReader<AssetEvent<MeshletMesh>>,
            )>,
        >,
    >,
    render_entities: &Entities,
) {

    if system_state.is_none() {
        *system_state = Some(SystemState::new(&mut main_world));
    }
    let system_state = system_state.as_mut().unwrap();
    let (instances_query, asset_server, mut assets, mut asset_events) =
        system_state.get_mut(&mut main_world).unwrap();

    instance_manager.reset(render_entities);

    for asset_event in asset_events.read() {
        if let AssetEvent::Unused { id } | AssetEvent::Modified { id } = asset_event {
            meshlet_mesh_manager.remove(id);
        }
    }

    for (
        instance,
        meshlet_mesh,
        transform,
        previous_transform,
        render_layers,
        not_shadow_receiver,
        not_shadow_caster,
    ) in &instances_query
    {

        if asset_server.is_managed(meshlet_mesh.id())
            && !asset_server.is_loaded_with_dependencies(meshlet_mesh.id())
        {
            continue;
        }

        let (root_bvh_node, aabb, bvh_depth) =
            meshlet_mesh_manager.queue_upload_if_needed(meshlet_mesh.id(), &mut assets);

        instance_manager.add_instance(
            instance.into(),
            root_bvh_node,
            aabb,
            bvh_depth,
            transform,
            previous_transform,
            render_layers,
            &mesh_material_ids,
            &render_material_bindings,
            not_shadow_receiver,
            not_shadow_caster,
        );
    }
}

pub fn queue_material_meshlet_meshes(
    mut instance_manager: ResMut<InstanceManager>,
    render_material_instances: Res<RenderMaterialInstances>,
) {
    let instance_manager = instance_manager.deref_mut();

    for (i, (instance, _, _)) in instance_manager.instances.iter().enumerate() {
        if let Some(material_instance) = render_material_instances.instances.get(instance)
            && let Some(material_id) = instance_manager
                .material_id_lookup
                .get(&material_instance.asset_id)
        {
            instance_manager
                .material_ids_present_in_scene
                .insert(*material_id);
            instance_manager.instance_material_ids.get_mut()[i] = *material_id;
        }
    }
}
