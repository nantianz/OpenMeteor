use bevy_asset::AssetId;
use bevy_ecs::{
    resource::Resource,
    world::{FromWorld, World},
};
use bevy_mesh::{
    morph::{MorphAttributes, MorphBuildError, MAX_MORPH_WEIGHTS, MAX_TEXTURE_WIDTH},
    Mesh,
};
use bevy_platform::collections::HashMap;
use wgpu::{
    Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    TextureViewDescriptor,
};
use wgpu_types::TextureDataOrder;

use crate::{
    render_resource::{Buffer, Texture, TextureView},
    renderer::{RenderDevice, RenderQueue},
};

#[derive(Clone, Debug)]
pub struct MorphTargetImage {

    pub texture: Texture,

    pub texture_view: TextureView,
}

impl MorphTargetImage {

    pub fn new(
        render_device: &RenderDevice,
        render_queue: &RenderQueue,
        targets: &[MorphAttributes],
        vertex_count: usize,
    ) -> Result<Self, MorphBuildError> {
        let max = MAX_TEXTURE_WIDTH;
        let target_count = targets.len() / vertex_count;
        if target_count > MAX_MORPH_WEIGHTS {
            return Err(MorphBuildError::TooManyTargets { target_count });
        }
        let component_count = (vertex_count * MorphAttributes::COMPONENT_COUNT) as u32;
        let Some((Rect(width, height), padding)) = lowest_2d(component_count, max) else {
            return Err(MorphBuildError::TooManyAttributes {
                vertex_count,
                component_count,
            });
        };
        let data: Vec<u8> = targets
            .chunks(vertex_count)
            .flat_map(|attributes| {
                let layer_byte_count = (padding + component_count) as usize * size_of::<f32>();
                let mut buffer = Vec::with_capacity(layer_byte_count);
                for to_add in attributes {
                    buffer.extend_from_slice(bytemuck::bytes_of(&[
                        to_add.position,
                        to_add.normal,
                        to_add.tangent,
                    ]));
                }

                buffer.extend(core::iter::repeat_n(0, padding as usize * size_of::<f32>()));
                debug_assert_eq!(buffer.len(), layer_byte_count);
                buffer
            })
            .collect();
        let extents = Extent3d {
            width,
            height,
            depth_or_array_layers: target_count as u32,
        };
        let texture = render_device.create_texture_with_data(
            render_queue,
            &TextureDescriptor {
                label: Some("morph target image"),
                size: extents,
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D3,
                format: TextureFormat::R32Float,
                usage: TextureUsages::COPY_DST | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            TextureDataOrder::LayerMajor,
            &data,
        );
        let texture_view = texture.create_view(&TextureViewDescriptor {
            label: Some("morph target texture view"),
            ..TextureViewDescriptor::default()
        });
        Ok(MorphTargetImage {
            texture,
            texture_view,
        })
    }
}

#[derive(Resource)]
pub enum RenderMorphTargetAllocator {

    Image {

        mesh_id_to_image: HashMap<AssetId<Mesh>, MorphTargetImage>,
    },

    Storage,
}

impl FromWorld for RenderMorphTargetAllocator {
    fn from_world(world: &mut World) -> RenderMorphTargetAllocator {
        let render_device = world.resource::<RenderDevice>();
        if bevy_render::storage_buffers_are_unsupported(&render_device.limits()) {
            RenderMorphTargetAllocator::Image {
                mesh_id_to_image: HashMap::default(),
            }
        } else {
            RenderMorphTargetAllocator::Storage
        }
    }
}

#[derive(Clone, Copy)]
pub enum MorphTargetsResource<'a> {

    Texture(&'a TextureView),

    Storage(&'a Buffer),
}

impl RenderMorphTargetAllocator {

    pub fn allocate(
        &mut self,
        render_device: &RenderDevice,
        render_queue: &RenderQueue,
        mesh_id: AssetId<Mesh>,
        targets: &[MorphAttributes],
        vertex_count: usize,
    ) {
        match *self {
            RenderMorphTargetAllocator::Image {
                ref mut mesh_id_to_image,
            } => {
                if let Ok(morph_target_image) =
                    MorphTargetImage::new(render_device, render_queue, targets, vertex_count)
                {
                    mesh_id_to_image.insert(mesh_id, morph_target_image);
                }
            }

            RenderMorphTargetAllocator::Storage => {

            }
        }
    }

    pub fn free(&mut self, mesh_id: AssetId<Mesh>) {
        match *self {
            RenderMorphTargetAllocator::Image {
                ref mut mesh_id_to_image,
            } => {
                mesh_id_to_image.remove(&mesh_id);
            }
            RenderMorphTargetAllocator::Storage => {

            }
        }
    }

    pub fn get_image(&self, mesh_id: AssetId<Mesh>) -> Option<MorphTargetImage> {
        match *self {
            RenderMorphTargetAllocator::Image {
                ref mesh_id_to_image,
            } => mesh_id_to_image.get(&mesh_id).cloned(),
            RenderMorphTargetAllocator::Storage => None,
        }
    }
}

struct Rect(u32, u32);

fn lowest_2d(min_includes: u32, max_edge: u32) -> Option<(Rect, u32)> {
    (1..=max_edge)
        .filter_map(|a| {
            let b = min_includes.div_ceil(a);
            let diff = (a * b).checked_sub(min_includes)?;
            Some((Rect(a, b), diff))
        })
        .filter_map(|(rect, diff)| (rect.1 <= max_edge).then_some((rect, diff)))
        .min_by_key(|(_, diff)| *diff)
}
