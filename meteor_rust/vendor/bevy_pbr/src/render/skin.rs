use core::mem::{self, size_of};

use bevy_asset::{prelude::AssetChanged, Assets};
use bevy_camera::visibility::ViewVisibility;
use bevy_ecs::prelude::*;
use bevy_math::Mat4;
use bevy_mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy_render::render_resource::{Buffer, BufferDescriptor};
use bevy_render::settings::WgpuLimits;
use bevy_render::sync_world::{MainEntity, MainEntityHashMap};
use bevy_render::{
    batching::NoAutomaticBatching,
    render_resource::BufferUsages,
    renderer::{RenderDevice, RenderQueue},
    Extract,
};
use bevy_transform::prelude::GlobalTransform;
use offset_allocator::{Allocation, Allocator};
use tracing::error;

pub const MAX_JOINTS: usize = 256;

const MAX_TOTAL_JOINTS: u32 = 1024 * 1024 * 1024;

const JOINTS_PER_ALLOCATION_UNIT: u32 = (256 / size_of::<Mat4>()) as u32;

#[derive(Clone, Copy)]
pub struct SkinByteOffset {

    pub byte_offset: u32,
}

impl SkinByteOffset {

    const fn from_index(index: usize) -> Self {
        SkinByteOffset {
            byte_offset: (index * size_of::<Mat4>()) as u32,
        }
    }

    pub fn index(&self) -> u32 {
        self.byte_offset / size_of::<Mat4>() as u32
    }
}

#[derive(Resource)]
pub struct SkinUniforms {

    pub current_staging_buffer: Vec<Mat4>,

    pub current_buffer: Buffer,

    pub prev_buffer: Buffer,

    allocator: Allocator,

    skin_uniform_info: MainEntityHashMap<SkinUniformInfo>,

    total_joints: usize,
}

pub fn skin_uniforms_from_world(device: Res<RenderDevice>, mut commands: Commands) {
    let buffer_usages = (if skins_use_uniform_buffers(&device.limits()) {
        BufferUsages::UNIFORM
    } else {
        BufferUsages::STORAGE
    }) | BufferUsages::COPY_DST;

    let current_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("skin uniform buffer"),
        size: MAX_JOINTS as u64 * size_of::<Mat4>() as u64,
        usage: buffer_usages,
        mapped_at_creation: false,
    });
    let prev_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("skin uniform buffer"),
        size: MAX_JOINTS as u64 * size_of::<Mat4>() as u64,
        usage: buffer_usages,
        mapped_at_creation: false,
    });

    let res = SkinUniforms {
        current_staging_buffer: vec![],
        current_buffer,
        prev_buffer,
        allocator: Allocator::new(MAX_TOTAL_JOINTS),
        skin_uniform_info: MainEntityHashMap::default(),
        total_joints: 0,
    };

    commands.insert_resource(res);
}

impl SkinUniforms {

    pub fn skin_index(&self, skin: MainEntity) -> Option<u32> {
        self.skin_uniform_info
            .get(&skin)
            .map(SkinUniformInfo::offset)
    }

    pub fn skin_byte_offset(&self, skin: MainEntity) -> Option<SkinByteOffset> {
        self.skin_uniform_info.get(&skin).map(|skin_uniform_info| {
            SkinByteOffset::from_index(skin_uniform_info.offset() as usize)
        })
    }

    pub fn all_skins(&self) -> impl Iterator<Item = &MainEntity> {
        self.skin_uniform_info.keys()
    }
}

struct SkinUniformInfo {

    allocation: Allocation,

    joints: Vec<MainEntity>,
}

impl SkinUniformInfo {

    fn offset(&self) -> u32 {
        self.allocation.offset * JOINTS_PER_ALLOCATION_UNIT
    }
}

pub fn skins_use_uniform_buffers(limits: &WgpuLimits) -> bool {
    bevy_render::storage_buffers_are_unsupported(limits)
}

pub fn prepare_skins(
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    uniform: ResMut<SkinUniforms>,
) {
    let uniform = uniform.into_inner();

    if uniform.current_staging_buffer.is_empty() {
        return;
    }

    mem::swap(&mut uniform.current_buffer, &mut uniform.prev_buffer);

    let needed_size = (uniform.current_staging_buffer.len() as u64 + MAX_JOINTS as u64)
        * size_of::<Mat4>() as u64;
    if uniform.current_buffer.size() < needed_size {
        let mut new_size = uniform.current_buffer.size();
        while new_size < needed_size {

            new_size = (new_size + new_size / 2).next_multiple_of(4);
        }

        let buffer_usages = if skins_use_uniform_buffers(&render_device.limits()) {
            BufferUsages::UNIFORM
        } else {
            BufferUsages::STORAGE
        } | BufferUsages::COPY_DST;
        uniform.current_buffer = render_device.create_buffer(&BufferDescriptor {
            label: Some("skin uniform buffer"),
            usage: buffer_usages,
            size: new_size,
            mapped_at_creation: false,
        });
        uniform.prev_buffer = render_device.create_buffer(&BufferDescriptor {
            label: Some("skin uniform buffer"),
            usage: buffer_usages,
            size: new_size,
            mapped_at_creation: false,
        });

        render_queue.write_buffer(
            &uniform.prev_buffer,
            0,
            bytemuck::must_cast_slice(&uniform.current_staging_buffer[..]),
        );
    }

    render_queue.write_buffer(
        &uniform.current_buffer,
        0,
        bytemuck::must_cast_slice(&uniform.current_staging_buffer[..]),
    );

}

pub fn extract_skins(
    skin_uniforms: ResMut<SkinUniforms>,
    skinned_meshes: Extract<Query<(Entity, &SkinnedMesh)>>,
    changed_skinned_meshes: Extract<
        Query<
            (Entity, &ViewVisibility, &SkinnedMesh),
            Or<(
                Changed<ViewVisibility>,
                Changed<SkinnedMesh>,
                AssetChanged<SkinnedMesh>,
            )>,
        >,
    >,
    skinned_mesh_inverse_bindposes: Extract<Res<Assets<SkinnedMeshInverseBindposes>>>,
    changed_transforms: Extract<Query<(Entity, &GlobalTransform), Changed<GlobalTransform>>>,
    joints: Extract<Query<&GlobalTransform>>,
    mut removed_skinned_meshes_query: Extract<RemovedComponents<SkinnedMesh>>,
) {
    let skin_uniforms = skin_uniforms.into_inner();

    add_or_delete_skins(
        skin_uniforms,
        &changed_skinned_meshes,
        &skinned_mesh_inverse_bindposes,
        &joints,
    );

    for (skin_entity, skin) in &skinned_meshes {
        extract_joints_for_skin(
            skin_entity.into(),
            skin,
            skin_uniforms,
            &changed_skinned_meshes,
            &skinned_mesh_inverse_bindposes,
            &changed_transforms,
        );
    }

    for skinned_mesh_entity in removed_skinned_meshes_query.read() {

        if !changed_skinned_meshes.contains(skinned_mesh_entity) {
            remove_skin(skin_uniforms, skinned_mesh_entity.into());
        }
    }
}

fn add_or_delete_skins(
    skin_uniforms: &mut SkinUniforms,
    changed_skinned_meshes: &Query<
        (Entity, &ViewVisibility, &SkinnedMesh),
        Or<(
            Changed<ViewVisibility>,
            Changed<SkinnedMesh>,
            AssetChanged<SkinnedMesh>,
        )>,
    >,
    skinned_mesh_inverse_bindposes: &Assets<SkinnedMeshInverseBindposes>,
    joints: &Query<&GlobalTransform>,
) {

    for (skinned_mesh_entity, skinned_mesh_view_visibility, skinned_mesh) in changed_skinned_meshes
    {

        let skinned_mesh_entity = MainEntity::from(skinned_mesh_entity);
        remove_skin(skin_uniforms, skinned_mesh_entity);

        if !(*skinned_mesh_view_visibility).get() {
            continue;
        }

        add_skin(
            skinned_mesh_entity,
            skinned_mesh,
            skin_uniforms,
            skinned_mesh_inverse_bindposes,
            joints,
        );
    }
}

fn extract_joints_for_skin(
    skin_entity: MainEntity,
    skin: &SkinnedMesh,
    skin_uniforms: &mut SkinUniforms,
    changed_skinned_meshes: &Query<
        (Entity, &ViewVisibility, &SkinnedMesh),
        Or<(
            Changed<ViewVisibility>,
            Changed<SkinnedMesh>,
            AssetChanged<SkinnedMesh>,
        )>,
    >,
    skinned_mesh_inverse_bindposes: &Assets<SkinnedMeshInverseBindposes>,
    changed_transforms: &Query<(Entity, &GlobalTransform), Changed<GlobalTransform>>,
) {

    if changed_skinned_meshes.contains(*skin_entity) {
        return;
    }

    let Some(skin_uniform_info) = skin_uniforms.skin_uniform_info.get(&skin_entity) else {
        return;
    };
    let Some(skinned_mesh_inverse_bindposes) =
        skinned_mesh_inverse_bindposes.get(&skin.inverse_bindposes)
    else {
        return;
    };

    for (joint_index, (&joint, skinned_mesh_inverse_bindpose)) in skin
        .joints
        .iter()
        .zip(skinned_mesh_inverse_bindposes.iter())
        .enumerate()
    {

        let Ok((_, joint_transform)) = changed_transforms.get(joint) else {
            continue;
        };

        let joint_matrix = joint_transform.affine() * *skinned_mesh_inverse_bindpose;
        skin_uniforms.current_staging_buffer[skin_uniform_info.offset() as usize + joint_index] =
            joint_matrix;
    }
}

fn add_skin(
    skinned_mesh_entity: MainEntity,
    skinned_mesh: &SkinnedMesh,
    skin_uniforms: &mut SkinUniforms,
    skinned_mesh_inverse_bindposes: &Assets<SkinnedMeshInverseBindposes>,
    joints: &Query<&GlobalTransform>,
) {

    let Some(allocation) = skin_uniforms.allocator.allocate(
        skinned_mesh
            .joints
            .len()
            .div_ceil(JOINTS_PER_ALLOCATION_UNIT as usize) as u32,
    ) else {
        error!(
            "Out of space for skin: {:?}. Tried to allocate space for {:?} joints.",
            skinned_mesh_entity,
            skinned_mesh.joints.len()
        );
        return;
    };

    let skin_uniform_info = SkinUniformInfo {
        allocation,
        joints: skinned_mesh
            .joints
            .iter()
            .map(|entity| MainEntity::from(*entity))
            .collect(),
    };

    let skinned_mesh_inverse_bindposes =
        skinned_mesh_inverse_bindposes.get(&skinned_mesh.inverse_bindposes);

    for (joint_index, &joint) in skinned_mesh.joints.iter().enumerate() {

        let skinned_mesh_inverse_bindpose =
            skinned_mesh_inverse_bindposes.and_then(|skinned_mesh_inverse_bindposes| {
                skinned_mesh_inverse_bindposes.get(joint_index)
            });
        let joint_matrix = match (skinned_mesh_inverse_bindpose, joints.get(joint)) {
            (Some(skinned_mesh_inverse_bindpose), Ok(transform)) => {
                transform.affine() * *skinned_mesh_inverse_bindpose
            }
            _ => Mat4::IDENTITY,
        };

        let buffer_index = skin_uniform_info.offset() as usize + joint_index;
        if skin_uniforms.current_staging_buffer.len() < buffer_index + 1 {
            skin_uniforms
                .current_staging_buffer
                .resize(buffer_index + 1, Mat4::IDENTITY);
        }
        skin_uniforms.current_staging_buffer[buffer_index] = joint_matrix;
    }

    skin_uniforms.total_joints += skinned_mesh.joints.len();

    skin_uniforms
        .skin_uniform_info
        .insert(skinned_mesh_entity, skin_uniform_info);
}

fn remove_skin(skin_uniforms: &mut SkinUniforms, skinned_mesh_entity: MainEntity) {
    let Some(old_skin_uniform_info) = skin_uniforms.skin_uniform_info.remove(&skinned_mesh_entity)
    else {
        return;
    };

    skin_uniforms
        .allocator
        .free(old_skin_uniform_info.allocation);

    skin_uniforms.total_joints -= old_skin_uniform_info.joints.len();
}

pub fn no_automatic_skin_batching(
    mut commands: Commands,
    query: Query<Entity, (With<SkinnedMesh>, Without<NoAutomaticBatching>)>,
    render_device: Res<RenderDevice>,
) {
    if !skins_use_uniform_buffers(&render_device.limits()) {
        return;
    }

    for entity in &query {
        commands.entity(entity).try_insert(NoAutomaticBatching);
    }
}
