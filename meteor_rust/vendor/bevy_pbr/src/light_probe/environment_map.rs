

use bevy_asset::AssetId;
use bevy_ecs::{
    query::{QueryData, QueryItem},
    system::lifetimeless::Read,
};
use bevy_image::Image;
use bevy_light::{EnvironmentMapLight, ParallaxCorrection};
use bevy_math::{Affine3A, Quat, Vec3};
use bevy_render::{
    extract_instances::ExtractInstance,
    render_asset::RenderAssets,
    render_resource::{
        binding_types, BindGroupLayoutEntryBuilder, Sampler, SamplerBindingType, TextureSampleType,
        TextureView,
    },
    renderer::{RenderAdapter, RenderDevice},
    texture::{FallbackImage, GpuImage},
};

use core::{num::NonZero, ops::Deref};

use crate::{
    add_cubemap_texture_view, binding_arrays_are_usable, RenderLightProbeFlags,
    MAX_VIEW_LIGHT_PROBES,
};

use super::{LightProbeComponent, RenderViewLightProbes};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnvironmentMapIds {

    pub diffuse: AssetId<Image>,

    pub specular: AssetId<Image>,
}

pub(crate) enum RenderViewEnvironmentMapBindGroupEntries<'a> {

    Single {

        diffuse_texture_view: &'a TextureView,

        specular_texture_view: &'a TextureView,

        sampler: &'a Sampler,
    },

    Multiple {

        diffuse_texture_views: Vec<&'a <TextureView as Deref>::Target>,

        specular_texture_views: Vec<&'a <TextureView as Deref>::Target>,

        sampler: &'a Sampler,
    },
}

pub struct EnvironmentMapViewLightProbeInfo {

    pub(crate) cubemap_index: i32,

    pub(crate) smallest_specular_mip_level: u32,

    pub(crate) intensity: f32,

    pub(crate) affects_lightmapped_mesh_diffuse: bool,

    pub(crate) rotation: Quat,
}

impl ExtractInstance for EnvironmentMapIds {
    type QueryData = Read<EnvironmentMapLight>;

    type QueryFilter = ();

    fn extract(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self> {
        Some(EnvironmentMapIds {
            diffuse: item.diffuse_map.id(),
            specular: item.specular_map.id(),
        })
    }
}

pub(crate) fn get_bind_group_layout_entries(
    render_device: &RenderDevice,
    render_adapter: &RenderAdapter,
) -> [BindGroupLayoutEntryBuilder; 3] {
    let mut texture_cube_binding =
        binding_types::texture_cube(TextureSampleType::Float { filterable: true });
    if binding_arrays_are_usable(render_device, render_adapter) {
        texture_cube_binding =
            texture_cube_binding.count(NonZero::<u32>::new(MAX_VIEW_LIGHT_PROBES as _).unwrap());
    }

    [
        texture_cube_binding,
        texture_cube_binding,
        binding_types::sampler(SamplerBindingType::Filtering),
    ]
}

impl<'a> RenderViewEnvironmentMapBindGroupEntries<'a> {

    pub(crate) fn get(
        render_view_environment_maps: Option<&RenderViewLightProbes<EnvironmentMapLight>>,
        images: &'a RenderAssets<GpuImage>,
        fallback_image: &'a FallbackImage,
        render_device: &RenderDevice,
        render_adapter: &RenderAdapter,
    ) -> RenderViewEnvironmentMapBindGroupEntries<'a> {
        if binding_arrays_are_usable(render_device, render_adapter) {

            let mut diffuse_texture_views = vec![];
            let mut specular_texture_views = vec![];
            let mut sampler = None;

            if let Some(environment_maps) = render_view_environment_maps {
                for &cubemap_id in &environment_maps.binding_index_to_textures {
                    add_cubemap_texture_view(
                        &mut diffuse_texture_views,
                        &mut sampler,
                        cubemap_id.diffuse,
                        images,
                        fallback_image,
                    );
                    add_cubemap_texture_view(
                        &mut specular_texture_views,
                        &mut sampler,
                        cubemap_id.specular,
                        images,
                        fallback_image,
                    );
                }
            }

            diffuse_texture_views.resize(MAX_VIEW_LIGHT_PROBES, &*fallback_image.cube.texture_view);
            specular_texture_views
                .resize(MAX_VIEW_LIGHT_PROBES, &*fallback_image.cube.texture_view);

            return RenderViewEnvironmentMapBindGroupEntries::Multiple {
                diffuse_texture_views,
                specular_texture_views,
                sampler: sampler.unwrap_or(&fallback_image.cube.sampler),
            };
        }

        if let Some(environment_maps) = render_view_environment_maps
            && let Some(cubemap) = environment_maps.binding_index_to_textures.first()
            && let (Some(diffuse_image), Some(specular_image)) =
                (images.get(cubemap.diffuse), images.get(cubemap.specular))
        {
            return RenderViewEnvironmentMapBindGroupEntries::Single {
                diffuse_texture_view: &diffuse_image.texture_view,
                specular_texture_view: &specular_image.texture_view,
                sampler: &diffuse_image.sampler,
            };
        }

        RenderViewEnvironmentMapBindGroupEntries::Single {
            diffuse_texture_view: &fallback_image.cube.texture_view,
            specular_texture_view: &fallback_image.cube.texture_view,
            sampler: &fallback_image.cube.sampler,
        }
    }
}

impl LightProbeComponent for EnvironmentMapLight {
    type AssetId = EnvironmentMapIds;

    type ViewLightProbeInfo = EnvironmentMapViewLightProbeInfo;

    type QueryData = Option<Read<ParallaxCorrection>>;

    fn id(&self, image_assets: &RenderAssets<GpuImage>) -> Option<Self::AssetId> {
        if image_assets.get(&self.diffuse_map).is_none()
            || image_assets.get(&self.specular_map).is_none()
        {
            None
        } else {
            Some(EnvironmentMapIds {
                diffuse: self.diffuse_map.id(),
                specular: self.specular_map.id(),
            })
        }
    }

    fn intensity(&self) -> f32 {
        self.intensity
    }

    fn flags(
        &self,
        maybe_parallax_correction: &<Self::QueryData as QueryData>::Item<'_, '_>,
    ) -> RenderLightProbeFlags {
        let mut flags = RenderLightProbeFlags::empty();
        if self.affects_lightmapped_mesh_diffuse {
            flags.insert(RenderLightProbeFlags::AFFECTS_LIGHTMAPPED_MESH_DIFFUSE);
        }
        if maybe_parallax_correction.is_some_and(|parallax_correction| {
            !matches!(*parallax_correction, ParallaxCorrection::None)
        }) {
            flags.insert(RenderLightProbeFlags::ENABLE_PARALLAX_CORRECTION);
        }
        flags
    }

    fn create_render_view_light_probes(
        view_component: Option<&EnvironmentMapLight>,
        image_assets: &RenderAssets<GpuImage>,
    ) -> RenderViewLightProbes<Self> {
        let mut render_view_light_probes = RenderViewLightProbes::new();

        if let Some(EnvironmentMapLight {
            diffuse_map: diffuse_map_handle,
            specular_map: specular_map_handle,
            intensity,
            affects_lightmapped_mesh_diffuse,
            rotation,
            ..
        }) = view_component
            && let (Some(_), Some(specular_map)) = (
                image_assets.get(diffuse_map_handle),
                image_assets.get(specular_map_handle),
            )
        {
            render_view_light_probes.view_light_probe_info =
                Some(EnvironmentMapViewLightProbeInfo {
                    cubemap_index: render_view_light_probes.get_or_insert_cubemap(
                        &EnvironmentMapIds {
                            diffuse: diffuse_map_handle.id(),
                            specular: specular_map_handle.id(),
                        },
                    ) as i32,
                    smallest_specular_mip_level: specular_map.texture_descriptor.mip_level_count
                        - 1,
                    intensity: *intensity,
                    affects_lightmapped_mesh_diffuse: *affects_lightmapped_mesh_diffuse,
                    rotation: *rotation,
                });
        };

        render_view_light_probes
    }

    fn get_world_from_light_matrix(&self, original_transform: &Affine3A) -> Affine3A {

        *original_transform * Affine3A::from_quat(self.rotation)
    }

    fn parallax_correction_bounds(
        &self,
        maybe_parallax_correction: &<Self::QueryData as QueryData>::Item<'_, '_>,
    ) -> Vec3 {
        match *maybe_parallax_correction {
            Some(&ParallaxCorrection::Custom(bounds)) => bounds,
            Some(&ParallaxCorrection::Auto) => Vec3::splat(0.5),
            Some(&ParallaxCorrection::None) | None => Vec3::ZERO,
        }
    }
}

impl Default for EnvironmentMapViewLightProbeInfo {
    fn default() -> Self {
        Self {
            cubemap_index: -1,
            smallest_specular_mip_level: 0,
            intensity: 1.0,
            affects_lightmapped_mesh_diffuse: true,
            rotation: Quat::IDENTITY,
        }
    }
}
