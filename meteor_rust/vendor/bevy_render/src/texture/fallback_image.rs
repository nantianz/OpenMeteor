use crate::{
    render_resource::*,
    renderer::{RenderDevice, RenderQueue},
    texture::{DefaultImageSampler, GpuImage},
};
use bevy_asset::RenderAssetUsages;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    prelude::{FromWorld, Res, ResMut},
    resource::Resource,
    system::SystemParam,
};
use bevy_image::{Image, ImageSampler, TextureFormatPixelInfo};
use bevy_platform::collections::HashMap;

#[derive(Resource)]
pub struct FallbackImage {

    pub d1: GpuImage,

    pub d2: GpuImage,

    pub d2_array: GpuImage,

    pub cube: GpuImage,

    pub cube_array: GpuImage,

    pub d3: GpuImage,
}

impl FallbackImage {

    pub fn get(&self, texture_dimension: TextureViewDimension) -> &GpuImage {
        match texture_dimension {
            TextureViewDimension::D1 => &self.d1,
            TextureViewDimension::D2 => &self.d2,
            TextureViewDimension::D2Array => &self.d2_array,
            TextureViewDimension::Cube => &self.cube,
            TextureViewDimension::CubeArray => &self.cube_array,
            TextureViewDimension::D3 => &self.d3,
        }
    }
}

#[derive(Resource, Deref)]
pub struct FallbackImageZero(GpuImage);

#[derive(Resource, Deref)]
pub struct FallbackImageCubemap(GpuImage);

fn fallback_image_new(
    render_device: &RenderDevice,
    render_queue: &RenderQueue,
    default_sampler: &DefaultImageSampler,
    format: TextureFormat,
    dimension: TextureViewDimension,
    samples: u32,
    value: u8,
) -> GpuImage {

    let extents = Extent3d {
        width: 1,
        height: 1,
        depth_or_array_layers: match dimension {
            TextureViewDimension::Cube | TextureViewDimension::CubeArray => 6,
            _ => 1,
        },
    };

    let create_texture_with_data = !format.is_depth_stencil_format() && samples == 1;

    let image_dimension = dimension.compatible_texture_dimension();
    let mut image = if create_texture_with_data {
        let data = vec![value; format.pixel_size().unwrap_or(0)];
        Image::new_fill(
            extents,
            image_dimension,
            &data,
            format,
            RenderAssetUsages::RENDER_WORLD,
        )
    } else {
        let mut image = Image::default_uninit();
        image.texture_descriptor.dimension = TextureDimension::D2;
        image.texture_descriptor.size = extents;
        image.texture_descriptor.format = format;
        image
    };
    image.texture_descriptor.sample_count = samples;
    if image_dimension == TextureDimension::D2 {
        image.texture_descriptor.usage |= TextureUsages::RENDER_ATTACHMENT;
    }

    let texture = if create_texture_with_data {
        render_device.create_texture_with_data(
            render_queue,
            &image.texture_descriptor,
            TextureDataOrder::default(),
            &image.data.expect("Image has no data"),
        )
    } else {
        render_device.create_texture(&image.texture_descriptor)
    };

    let texture_view = texture.create_view(&TextureViewDescriptor {
        dimension: Some(dimension),
        array_layer_count: Some(extents.depth_or_array_layers),
        ..TextureViewDescriptor::default()
    });
    let sampler = match image.sampler {
        ImageSampler::Default => (**default_sampler).clone(),
        ImageSampler::Descriptor(ref descriptor) => {
            render_device.create_sampler(&descriptor.as_wgpu())
        }
    };
    GpuImage {
        texture,
        texture_view,
        sampler,
        texture_descriptor: image.texture_descriptor,
        texture_view_descriptor: image.texture_view_descriptor,
        had_data: true,
    }
}

impl FromWorld for FallbackImage {
    fn from_world(world: &mut bevy_ecs::prelude::World) -> Self {
        let render_device = world.resource::<RenderDevice>();
        let render_queue = world.resource::<RenderQueue>();
        let default_sampler = world.resource::<DefaultImageSampler>();
        Self {
            d1: fallback_image_new(
                render_device,
                render_queue,
                default_sampler,
                TextureFormat::Rgba8UnormSrgb,
                TextureViewDimension::D1,
                1,
                255,
            ),
            d2: fallback_image_new(
                render_device,
                render_queue,
                default_sampler,
                TextureFormat::Rgba8UnormSrgb,
                TextureViewDimension::D2,
                1,
                255,
            ),
            d2_array: fallback_image_new(
                render_device,
                render_queue,
                default_sampler,
                TextureFormat::Rgba8UnormSrgb,
                TextureViewDimension::D2Array,
                1,
                255,
            ),
            cube: fallback_image_new(
                render_device,
                render_queue,
                default_sampler,
                TextureFormat::Rgba8UnormSrgb,
                TextureViewDimension::Cube,
                1,
                255,
            ),
            cube_array: fallback_image_new(
                render_device,
                render_queue,
                default_sampler,
                TextureFormat::Rgba8UnormSrgb,
                TextureViewDimension::CubeArray,
                1,
                255,
            ),
            d3: fallback_image_new(
                render_device,
                render_queue,
                default_sampler,
                TextureFormat::Rgba8UnormSrgb,
                TextureViewDimension::D3,
                1,
                255,
            ),
        }
    }
}

impl FromWorld for FallbackImageZero {
    fn from_world(world: &mut bevy_ecs::prelude::World) -> Self {
        let render_device = world.resource::<RenderDevice>();
        let render_queue = world.resource::<RenderQueue>();
        let default_sampler = world.resource::<DefaultImageSampler>();
        Self(fallback_image_new(
            render_device,
            render_queue,
            default_sampler,
            TextureFormat::Rgba8UnormSrgb,
            TextureViewDimension::D2,
            1,
            0,
        ))
    }
}

impl FromWorld for FallbackImageCubemap {
    fn from_world(world: &mut bevy_ecs::prelude::World) -> Self {
        let render_device = world.resource::<RenderDevice>();
        let render_queue = world.resource::<RenderQueue>();
        let default_sampler = world.resource::<DefaultImageSampler>();
        Self(fallback_image_new(
            render_device,
            render_queue,
            default_sampler,
            TextureFormat::Rgba8UnormSrgb,
            TextureViewDimension::Cube,
            1,
            255,
        ))
    }
}

#[derive(Resource, Deref, DerefMut, Default)]
pub struct FallbackImageFormatMsaaCache(HashMap<(u32, TextureFormat), GpuImage>);

#[derive(SystemParam)]
pub struct FallbackImageMsaa<'w> {
    cache: ResMut<'w, FallbackImageFormatMsaaCache>,
    render_device: Res<'w, RenderDevice>,
    render_queue: Res<'w, RenderQueue>,
    default_sampler: Res<'w, DefaultImageSampler>,
}

impl<'w> FallbackImageMsaa<'w> {
    pub fn image_for_samplecount(&mut self, sample_count: u32, format: TextureFormat) -> &GpuImage {
        self.cache.entry((sample_count, format)).or_insert_with(|| {
            fallback_image_new(
                &self.render_device,
                &self.render_queue,
                &self.default_sampler,
                format,
                TextureViewDimension::D2,
                sample_count,
                255,
            )
        })
    }
}
