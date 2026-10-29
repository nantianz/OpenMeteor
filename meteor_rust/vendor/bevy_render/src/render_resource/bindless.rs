

use alloc::borrow::Cow;
use core::{
    num::{NonZeroU32, NonZeroU64},
    ops::Range,
};

use bevy_derive::{Deref, DerefMut};
use wgpu::{
    BindGroupLayoutEntry, SamplerBindingType, ShaderStages, TextureSampleType, TextureViewDimension,
};

use bevy_material::bind_group_layout_entries::binding_types::{
    sampler, storage_buffer_read_only_sized, texture_1d, texture_2d, texture_2d_array, texture_3d,
    texture_cube, texture_cube_array,
};

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub const AUTO_BINDLESS_SLAB_RESOURCE_LIMIT: u32 = 64;

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub const AUTO_BINDLESS_SLAB_RESOURCE_LIMIT: u32 = 2048;

pub static BINDING_NUMBERS: [(BindlessResourceType, BindingNumber); 9] = [
    (BindlessResourceType::SamplerFiltering, BindingNumber(1)),
    (BindlessResourceType::SamplerNonFiltering, BindingNumber(2)),
    (BindlessResourceType::SamplerComparison, BindingNumber(3)),
    (BindlessResourceType::Texture1d, BindingNumber(4)),
    (BindlessResourceType::Texture2d, BindingNumber(5)),
    (BindlessResourceType::Texture2dArray, BindingNumber(6)),
    (BindlessResourceType::Texture3d, BindingNumber(7)),
    (BindlessResourceType::TextureCube, BindingNumber(8)),
    (BindlessResourceType::TextureCubeArray, BindingNumber(9)),
];

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub enum BindlessSlabResourceLimit {

    #[default]
    Auto,

    Custom(u32),
}

pub struct BindlessDescriptor {

    pub resources: Cow<'static, [BindlessResourceType]>,

    pub buffers: Cow<'static, [BindlessBufferDescriptor]>,

    pub index_tables: Cow<'static, [BindlessIndexTableDescriptor]>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum BindlessResourceType {

    None,

    Buffer,

    SamplerFiltering,

    SamplerNonFiltering,

    SamplerComparison,

    Texture1d,

    Texture2d,

    Texture2dArray,

    Texture3d,

    TextureCube,

    TextureCubeArray,

    DataBuffer,
}

#[derive(Clone, Copy, Debug)]
pub struct BindlessBufferDescriptor {

    pub binding_number: BindingNumber,

    pub bindless_index: BindlessIndex,

    pub size: Option<usize>,
}

#[derive(Clone)]
pub struct BindlessIndexTableDescriptor {

    pub indices: Range<BindlessIndex>,

    pub binding_number: BindingNumber,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Deref, DerefMut)]
pub struct BindingNumber(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Hash, Debug, Deref, DerefMut)]
pub struct BindlessIndex(pub u32);

pub fn create_bindless_bind_group_layout_entries(
    bindless_index_table_length: u32,
    bindless_slab_resource_limit: u32,
    bindless_index_table_binding_number: BindingNumber,
    used_resource_types: &[BindlessResourceType],
) -> Vec<BindGroupLayoutEntry> {
    let bindless_slab_resource_limit =
        NonZeroU32::new(bindless_slab_resource_limit).expect("Bindless slot count must be nonzero");

    let stages = ShaderStages::FRAGMENT | ShaderStages::VERTEX | ShaderStages::COMPUTE;

    let mut entries = vec![

        storage_buffer_read_only_sized(
            false,
            NonZeroU64::new(bindless_index_table_length as u64 * size_of::<u32>() as u64),
        )
        .build(*bindless_index_table_binding_number, stages),
    ];

    for &(resource_type, ref binding_number) in BINDING_NUMBERS.iter() {
        if !used_resource_types.contains(&resource_type) {
            continue;
        }
        let Some(binding_type) = (match resource_type {
            BindlessResourceType::SamplerFiltering => Some(sampler(SamplerBindingType::Filtering)),
            BindlessResourceType::SamplerNonFiltering => {
                Some(sampler(SamplerBindingType::NonFiltering))
            }
            BindlessResourceType::SamplerComparison => {
                Some(sampler(SamplerBindingType::Comparison))
            }
            BindlessResourceType::Texture1d => {
                Some(texture_1d(TextureSampleType::Float { filterable: true }))
            }
            BindlessResourceType::Texture2d => {
                Some(texture_2d(TextureSampleType::Float { filterable: true }))
            }
            BindlessResourceType::Texture2dArray => {
                Some(texture_2d_array(TextureSampleType::Float {
                    filterable: true,
                }))
            }
            BindlessResourceType::Texture3d => {
                Some(texture_3d(TextureSampleType::Float { filterable: true }))
            }
            BindlessResourceType::TextureCube => {
                Some(texture_cube(TextureSampleType::Float { filterable: true }))
            }
            BindlessResourceType::TextureCubeArray => {
                Some(texture_cube_array(TextureSampleType::Float {
                    filterable: true,
                }))
            }
            BindlessResourceType::None
            | BindlessResourceType::Buffer
            | BindlessResourceType::DataBuffer => None,
        }) else {
            continue;
        };
        entries.push(
            binding_type
                .count(bindless_slab_resource_limit)
                .build(**binding_number, stages),
        );
    }

    entries
}

impl BindlessSlabResourceLimit {

    pub fn resolve(&self) -> u32 {
        match *self {
            BindlessSlabResourceLimit::Auto => AUTO_BINDLESS_SLAB_RESOURCE_LIMIT,
            BindlessSlabResourceLimit::Custom(limit) => limit,
        }
    }
}

impl BindlessResourceType {

    pub fn binding_number(&self) -> Option<&'static BindingNumber> {
        match BINDING_NUMBERS.binary_search_by_key(self, |(key, _)| *key) {
            Ok(binding_number) => Some(&BINDING_NUMBERS[binding_number].1),
            Err(_) => None,
        }
    }
}

impl From<TextureViewDimension> for BindlessResourceType {
    fn from(texture_view_dimension: TextureViewDimension) -> Self {
        match texture_view_dimension {
            TextureViewDimension::D1 => BindlessResourceType::Texture1d,
            TextureViewDimension::D2 => BindlessResourceType::Texture2d,
            TextureViewDimension::D2Array => BindlessResourceType::Texture2dArray,
            TextureViewDimension::Cube => BindlessResourceType::TextureCube,
            TextureViewDimension::CubeArray => BindlessResourceType::TextureCubeArray,
            TextureViewDimension::D3 => BindlessResourceType::Texture3d,
        }
    }
}

impl From<SamplerBindingType> for BindlessResourceType {
    fn from(sampler_binding_type: SamplerBindingType) -> Self {
        match sampler_binding_type {
            SamplerBindingType::Filtering => BindlessResourceType::SamplerFiltering,
            SamplerBindingType::NonFiltering => BindlessResourceType::SamplerNonFiltering,
            SamplerBindingType::Comparison => BindlessResourceType::SamplerComparison,
        }
    }
}

impl From<u32> for BindlessIndex {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<u32> for BindingNumber {
    fn from(value: u32) -> Self {
        Self(value)
    }
}
