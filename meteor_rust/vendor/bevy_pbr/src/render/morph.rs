use core::{iter, mem};

use bevy_camera::visibility::ViewVisibility;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::prelude::*;
use bevy_mesh::morph::{MeshMorphWeights, MorphWeights, MAX_MORPH_WEIGHTS};
use bevy_platform::collections::hash_map::Entry;
use bevy_render::mesh::allocator::MeshAllocator;
use bevy_render::mesh::RenderMesh;
use bevy_render::render_asset::RenderAssets;
use bevy_render::render_resource::ShaderType;
use bevy_render::sync_world::{MainEntity, MainEntityHashMap};
use bevy_render::{
    batching::NoAutomaticBatching,
    render_resource::{BufferUsages, RawBufferVec},
    renderer::{RenderDevice, RenderQueue},
    Extract,
};
use bytemuck::{NoUninit, Pod, Zeroable};

use crate::{skin, RenderMeshInstances};

#[derive(Component)]
pub struct MorphIndex {
    pub index: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Deref, DerefMut)]
pub struct MorphDescriptorIndex(pub u32);

#[derive(Resource)]
pub enum MorphIndices {

    Uniform {

        current: MainEntityHashMap<MorphIndex>,

        prev: MainEntityHashMap<MorphIndex>,
    },

    Storage {

        morph_weights_info: MainEntityHashMap<MorphWeightsInfo>,

        gpu_descriptor_indices: MainEntityHashMap<MorphDescriptorIndex>,

        gpu_descriptor_free_list: Vec<MorphDescriptorIndex>,
    },
}

#[derive(Clone, Copy)]
pub struct MorphWeightsInfo {

    current_weight_offset: u32,

    pub(crate) prev_weight_offset: Option<u32>,

    weight_count: u32,
}

impl FromWorld for MorphIndices {
    fn from_world(world: &mut World) -> MorphIndices {
        let render_device = world.resource::<RenderDevice>();

        if skin::skins_use_uniform_buffers(&render_device.limits()) {
            MorphIndices::Uniform {
                current: MainEntityHashMap::default(),
                prev: MainEntityHashMap::default(),
            }
        } else {
            MorphIndices::Storage {
                morph_weights_info: MainEntityHashMap::default(),
                gpu_descriptor_indices: MainEntityHashMap::default(),
                gpu_descriptor_free_list: vec![],
            }
        }
    }
}

#[derive(Resource)]
pub struct MorphUniforms {

    pub current_buffer: RawBufferVec<f32>,

    pub prev_buffer: RawBufferVec<f32>,

    pub descriptors_buffer: Option<RawBufferVec<GpuMorphDescriptor>>,
}

impl FromWorld for MorphUniforms {
    fn from_world(world: &mut World) -> MorphUniforms {
        let render_device = world.resource::<RenderDevice>();

        let skins_use_uniform_buffers = skin::skins_use_uniform_buffers(&render_device.limits());

        let buffer_usages = BufferUsages::COPY_DST
            | (if skins_use_uniform_buffers {
                BufferUsages::UNIFORM
            } else {
                BufferUsages::STORAGE
            });

        MorphUniforms {
            current_buffer: RawBufferVec::new(buffer_usages),
            prev_buffer: RawBufferVec::new(buffer_usages),
            descriptors_buffer: if skins_use_uniform_buffers {
                None
            } else {
                Some(RawBufferVec::new(
                    BufferUsages::COPY_DST | BufferUsages::STORAGE,
                ))
            },
        }
    }
}

impl MorphUniforms {

    fn prepare_for_new_frame(&mut self) {
        mem::swap(&mut self.current_buffer, &mut self.prev_buffer);
        self.current_buffer.clear();
    }
}

impl MorphIndices {

    pub fn morph_descriptor_index(&self, main_entity: MainEntity) -> Option<MorphDescriptorIndex> {
        match *self {
            MorphIndices::Uniform { .. } => None,
            MorphIndices::Storage {
                ref gpu_descriptor_indices,
                ..
            } => gpu_descriptor_indices.get(&main_entity).copied(),
        }
    }
}

#[derive(Clone, Copy, Default, ShaderType, Pod, Zeroable)]
#[repr(C)]
pub struct GpuMorphDescriptor {

    pub current_weights_offset: u32,

    pub prev_weights_offset: u32,

    pub targets_offset: u32,

    pub vertex_count: u32,

    pub weight_count: u32,
}

pub fn write_morph_buffers(
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut uniform: ResMut<MorphUniforms>,
) {
    if uniform.current_buffer.is_empty() {
        return;
    }
    let len = uniform.current_buffer.len();
    uniform.current_buffer.reserve(len, &render_device);
    uniform
        .current_buffer
        .write_buffer(&render_device, &render_queue);

    if let Some(ref mut descriptors_buffer) = uniform.descriptors_buffer {
        if descriptors_buffer.is_empty() {
            descriptors_buffer.push(GpuMorphDescriptor::default());
        }
        descriptors_buffer.write_buffer(&render_device, &render_queue);
    }
}

const fn can_align(step: usize, target: usize) -> bool {
    step.is_multiple_of(target) || target.is_multiple_of(step)
}

const WGPU_MIN_ALIGN: usize = 256;

fn add_to_alignment<T: NoUninit + Default>(buffer: &mut RawBufferVec<T>) {
    let n = WGPU_MIN_ALIGN;
    let t_size = size_of::<T>();
    if !can_align(n, t_size) {

        panic!(
            "RawBufferVec should contain only types with a size multiple or divisible by {n}, \
            {} has a size of {t_size}, which is neither multiple or divisible by {n}",
            core::any::type_name::<T>()
        );
    }

    let buffer_size = buffer.len();
    let byte_size = t_size * buffer_size;
    let bytes_over_n = byte_size % n;
    if bytes_over_n == 0 {
        return;
    }
    let bytes_to_add = n - bytes_over_n;
    let ts_to_add = bytes_to_add / t_size;
    buffer.extend(iter::repeat_with(T::default).take(ts_to_add));
}

pub fn extract_morphs(
    morph_indices: ResMut<MorphIndices>,
    uniform: ResMut<MorphUniforms>,
    query: Extract<Query<(Entity, &ViewVisibility, &MeshMorphWeights)>>,
    weights_query: Extract<Query<&MorphWeights>>,
    render_device: Res<RenderDevice>,
) {

    let (morph_indices, uniform) = (morph_indices.into_inner(), uniform.into_inner());

    let morphs_use_uniform_buffers = skin::skins_use_uniform_buffers(&render_device.limits());

    let maybe_old_morph_target_info = match *morph_indices {
        MorphIndices::Uniform {
            ref mut current,
            ref mut prev,
        } => {
            mem::swap(current, prev);
            current.clear();
            None
        }
        MorphIndices::Storage {
            morph_weights_info: ref mut morph_target_info,
            ..
        } => Some(mem::take(morph_target_info)),
    };

    uniform.prepare_for_new_frame();

    for (entity, view_visibility, mesh_weights) in &query {
        if !view_visibility.get() {
            continue;
        }
        let Ok(weights) = (match mesh_weights {
            MeshMorphWeights::Reference(entity) => {
                weights_query.get(*entity).map(MorphWeights::weights)
            }
            MeshMorphWeights::Value { weights } => Ok(weights.as_slice()),
        }) else {
            continue;
        };

        let start = uniform.current_buffer.len();
        if morphs_use_uniform_buffers {
            let legal_weights = weights
                .iter()
                .chain(iter::repeat(&0.0))
                .take(MAX_MORPH_WEIGHTS)
                .copied();
            uniform.current_buffer.extend(legal_weights);
            add_to_alignment::<f32>(&mut uniform.current_buffer);
        } else {
            uniform.current_buffer.extend(weights.iter().copied());
        }

        let maybe_prev_weights_offset =
            maybe_old_morph_target_info
                .as_ref()
                .and_then(|old_morph_target_info| {
                    old_morph_target_info
                        .get(&MainEntity::from(entity))
                        .map(|morph_target_info| morph_target_info.current_weight_offset)
                });

        match *morph_indices {
            MorphIndices::Uniform {
                ref mut current, ..
            } => {
                let index = (start * size_of::<f32>()) as u32;
                current.insert(entity.into(), MorphIndex { index });
            }
            MorphIndices::Storage {
                morph_weights_info: ref mut morph_target_info,
                ..
            } => {
                morph_target_info.insert(
                    entity.into(),
                    MorphWeightsInfo {
                        current_weight_offset: start as u32,
                        prev_weight_offset: maybe_prev_weights_offset,
                        weight_count: weights.len() as u32,
                    },
                );
            }
        }
    }
}

pub fn prepare_morph_descriptors(
    mut morph_indices: ResMut<MorphIndices>,
    mut morph_uniforms: ResMut<MorphUniforms>,
    render_mesh_instances: Res<RenderMeshInstances>,
    meshes: Res<RenderAssets<RenderMesh>>,
    mesh_allocator: Res<MeshAllocator>,
) {

    let (
        &mut MorphIndices::Storage {
            morph_weights_info: ref morph_target_info,
            ref mut gpu_descriptor_indices,
            ref mut gpu_descriptor_free_list,
        },
        &mut Some(ref mut descriptors_buffer),
    ) = (&mut *morph_indices, &mut morph_uniforms.descriptors_buffer)
    else {
        return;
    };

    for (&morph_target_main_entity, morph_target_info) in morph_target_info {
        let Some(mesh_id) = render_mesh_instances.mesh_asset_id(morph_target_main_entity) else {
            continue;
        };
        let Some(mesh) = meshes.get(mesh_id) else {
            continue;
        };
        let Some(morph_targets_slice) = mesh_allocator.mesh_morph_target_slice(&mesh_id) else {
            continue;
        };

        let morph_descriptor = GpuMorphDescriptor {
            current_weights_offset: morph_target_info.current_weight_offset,
            prev_weights_offset: morph_target_info.prev_weight_offset.unwrap_or(!0),
            targets_offset: morph_targets_slice.range.start,
            vertex_count: mesh.vertex_count,
            weight_count: morph_target_info.weight_count,
        };

        let descriptor_index;
        match gpu_descriptor_indices.entry(morph_target_main_entity) {
            Entry::Occupied(occupied_entry) => {
                descriptor_index = *occupied_entry.get();
                descriptors_buffer.set(descriptor_index.0, morph_descriptor);
            }
            Entry::Vacant(vacant_entry) => {
                match gpu_descriptor_free_list.pop() {
                    Some(free_descriptor_index) => {
                        descriptor_index = free_descriptor_index;
                        descriptors_buffer.set(descriptor_index.0, morph_descriptor);
                    }
                    None => {
                        descriptor_index =
                            MorphDescriptorIndex(descriptors_buffer.push(morph_descriptor) as u32);
                    }
                }
                vacant_entry.insert(descriptor_index);
            }
        };

        gpu_descriptor_indices.insert(morph_target_main_entity, descriptor_index);
    }

    gpu_descriptor_indices.retain(|morph_target_main_entity, descriptor_index| {
        let live = morph_target_info.contains_key(morph_target_main_entity);
        if !live {
            gpu_descriptor_free_list.push(*descriptor_index);
        }
        live
    });
}

pub fn no_automatic_morph_batching(
    mut commands: Commands,
    query: Query<Entity, (With<MeshMorphWeights>, Without<NoAutomaticBatching>)>,
    render_device: Res<RenderDevice>,
) {

    if !skin::skins_use_uniform_buffers(&render_device.limits()) {
        return;
    }

    for entity in &query {
        commands.entity(entity).try_insert(NoAutomaticBatching);
    }
}
