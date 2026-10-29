

use bevy_image::Image;
use bevy_light::IrradianceVolume;
use bevy_render::{
    render_asset::RenderAssets,
    render_resource::{
        binding_types, BindGroupLayoutEntryBuilder, Sampler, SamplerBindingType, TextureSampleType,
        TextureView,
    },
    renderer::{RenderAdapter, RenderDevice},
    texture::{FallbackImage, GpuImage},
};
use core::{num::NonZero, ops::Deref};

use bevy_asset::AssetId;

use crate::{
    add_cubemap_texture_view, binding_arrays_are_usable, RenderLightProbeFlags,
    RenderViewLightProbes, MAX_VIEW_LIGHT_PROBES,
};

use super::LightProbeComponent;

pub(crate) const IRRADIANCE_VOLUMES_ARE_USABLE: bool = cfg!(not(target_arch = "wasm32"));

pub(crate) enum RenderViewIrradianceVolumeBindGroupEntries<'a> {

    Single {

        texture_view: &'a TextureView,

        sampler: &'a Sampler,
    },

    Multiple {

        texture_views: Vec<&'a <TextureView as Deref>::Target>,

        sampler: &'a Sampler,
    },
}

impl<'a> RenderViewIrradianceVolumeBindGroupEntries<'a> {

    pub(crate) fn get(
        render_view_irradiance_volumes: Option<&RenderViewLightProbes<IrradianceVolume>>,
        images: &'a RenderAssets<GpuImage>,
        fallback_image: &'a FallbackImage,
        render_device: &RenderDevice,
        render_adapter: &RenderAdapter,
    ) -> RenderViewIrradianceVolumeBindGroupEntries<'a> {
        if binding_arrays_are_usable(render_device, render_adapter) {
            RenderViewIrradianceVolumeBindGroupEntries::get_multiple(
                render_view_irradiance_volumes,
                images,
                fallback_image,
            )
        } else {
            RenderViewIrradianceVolumeBindGroupEntries::single(
                render_view_irradiance_volumes,
                images,
                fallback_image,
            )
        }
    }

    fn get_multiple(
        render_view_irradiance_volumes: Option<&RenderViewLightProbes<IrradianceVolume>>,
        images: &'a RenderAssets<GpuImage>,
        fallback_image: &'a FallbackImage,
    ) -> RenderViewIrradianceVolumeBindGroupEntries<'a> {
        let mut texture_views = vec![];
        let mut sampler = None;

        if let Some(irradiance_volumes) = render_view_irradiance_volumes {
            for &cubemap_id in &irradiance_volumes.binding_index_to_textures {
                add_cubemap_texture_view(
                    &mut texture_views,
                    &mut sampler,
                    cubemap_id,
                    images,
                    fallback_image,
                );
            }
        }

        texture_views.resize(MAX_VIEW_LIGHT_PROBES, &*fallback_image.d3.texture_view);

        RenderViewIrradianceVolumeBindGroupEntries::Multiple {
            texture_views,
            sampler: sampler.unwrap_or(&fallback_image.d3.sampler),
        }
    }

    fn single(
        render_view_irradiance_volumes: Option<&RenderViewLightProbes<IrradianceVolume>>,
        images: &'a RenderAssets<GpuImage>,
        fallback_image: &'a FallbackImage,
    ) -> RenderViewIrradianceVolumeBindGroupEntries<'a> {
        if let Some(irradiance_volumes) = render_view_irradiance_volumes
            && let Some(irradiance_volume) = irradiance_volumes.render_light_probes.first()
            && irradiance_volume.texture_index >= 0
            && let Some(image_id) = irradiance_volumes
                .binding_index_to_textures
                .get(irradiance_volume.texture_index as usize)
            && let Some(image) = images.get(*image_id)
        {
            return RenderViewIrradianceVolumeBindGroupEntries::Single {
                texture_view: &image.texture_view,
                sampler: &image.sampler,
            };
        }

        RenderViewIrradianceVolumeBindGroupEntries::Single {
            texture_view: &fallback_image.d3.texture_view,
            sampler: &fallback_image.d3.sampler,
        }
    }
}

pub(crate) fn get_bind_group_layout_entries(
    render_device: &RenderDevice,
    render_adapter: &RenderAdapter,
) -> [BindGroupLayoutEntryBuilder; 2] {
    let mut texture_3d_binding =
        binding_types::texture_3d(TextureSampleType::Float { filterable: true });
    if binding_arrays_are_usable(render_device, render_adapter) {
        texture_3d_binding =
            texture_3d_binding.count(NonZero::<u32>::new(MAX_VIEW_LIGHT_PROBES as _).unwrap());
    }

    [
        texture_3d_binding,
        binding_types::sampler(SamplerBindingType::Filtering),
    ]
}

impl LightProbeComponent for IrradianceVolume {
    type AssetId = AssetId<Image>;

    type ViewLightProbeInfo = ();

    type QueryData = ();

    fn id(&self, image_assets: &RenderAssets<GpuImage>) -> Option<Self::AssetId> {
        if image_assets.get(&self.voxels).is_none() {
            None
        } else {
            Some(self.voxels.id())
        }
    }

    fn intensity(&self) -> f32 {
        self.intensity
    }

    fn flags(&self, _: &Self::QueryData) -> RenderLightProbeFlags {
        if self.affects_lightmapped_meshes {
            RenderLightProbeFlags::AFFECTS_LIGHTMAPPED_MESH_DIFFUSE
        } else {
            RenderLightProbeFlags::empty()
        }
    }

    fn create_render_view_light_probes(
        _: Option<&Self>,
        _: &RenderAssets<GpuImage>,
    ) -> RenderViewLightProbes<Self> {
        RenderViewLightProbes::new()
    }
}
