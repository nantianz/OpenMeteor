

use arrayvec::ArrayVec;
use bevy_math::Mat4;
use bevy_mesh::morph::MAX_MORPH_WEIGHTS;
use bevy_render::{
    mesh::morph::MorphTargetsResource,
    render_resource::*,
    renderer::{RenderAdapter, RenderDevice},
};

use crate::{binding_arrays_are_usable, render::skin::MAX_JOINTS, skin, LightmapSlab};

const MORPH_WEIGHT_SIZE: usize = size_of::<f32>();

pub const MORPH_BUFFER_SIZE: usize = MAX_MORPH_WEIGHTS * MORPH_WEIGHT_SIZE;

const JOINT_SIZE: usize = size_of::<Mat4>();
pub(crate) const JOINT_BUFFER_SIZE: usize = MAX_JOINTS * JOINT_SIZE;

mod layout_entry {
    use core::num::NonZeroU32;

    use super::{JOINT_BUFFER_SIZE, MORPH_BUFFER_SIZE};
    use crate::{render::skin, GpuMorphDescriptor, MeshUniform, LIGHTMAPS_PER_SLAB};
    use bevy_mesh::morph::MorphAttributes;
    use bevy_render::{
        render_resource::{
            binding_types::{
                sampler, storage_buffer_read_only, storage_buffer_read_only_sized, texture_2d,
                texture_3d, uniform_buffer_sized,
            },
            BindGroupLayoutEntryBuilder, BufferSize, GpuArrayBuffer, SamplerBindingType,
            ShaderStages, TextureSampleType,
        },
        settings::WgpuLimits,
    };

    pub(super) fn model(limits: &WgpuLimits) -> BindGroupLayoutEntryBuilder {
        GpuArrayBuffer::<MeshUniform>::binding_layout(limits)
            .visibility(ShaderStages::VERTEX_FRAGMENT)
    }
    pub(super) fn skinning(limits: &WgpuLimits) -> BindGroupLayoutEntryBuilder {

        let size = BufferSize::new(JOINT_BUFFER_SIZE as u64);
        if skin::skins_use_uniform_buffers(limits) {
            uniform_buffer_sized(true, size)
        } else {
            storage_buffer_read_only_sized(false, size)
        }
    }
    pub(super) fn weights(limits: &WgpuLimits) -> BindGroupLayoutEntryBuilder {
        if skin::skins_use_uniform_buffers(limits) {
            uniform_buffer_sized(true, BufferSize::new(MORPH_BUFFER_SIZE as u64))
        } else {
            storage_buffer_read_only::<f32>(false)
        }
    }
    pub(super) fn targets(limits: &WgpuLimits) -> BindGroupLayoutEntryBuilder {
        if skin::skins_use_uniform_buffers(limits) {
            texture_3d(TextureSampleType::Float { filterable: false })
        } else {
            storage_buffer_read_only::<MorphAttributes>(false)
        }
    }
    pub(super) fn morph_descriptors() -> BindGroupLayoutEntryBuilder {
        storage_buffer_read_only::<GpuMorphDescriptor>(false)
    }
    pub(super) fn lightmaps_texture_view() -> BindGroupLayoutEntryBuilder {
        texture_2d(TextureSampleType::Float { filterable: true }).visibility(ShaderStages::FRAGMENT)
    }
    pub(super) fn lightmaps_sampler() -> BindGroupLayoutEntryBuilder {
        sampler(SamplerBindingType::Filtering).visibility(ShaderStages::FRAGMENT)
    }
    pub(super) fn lightmaps_texture_view_array() -> BindGroupLayoutEntryBuilder {
        texture_2d(TextureSampleType::Float { filterable: true })
            .visibility(ShaderStages::FRAGMENT)
            .count(NonZeroU32::new(LIGHTMAPS_PER_SLAB as u32).unwrap())
    }
    pub(super) fn lightmaps_sampler_array() -> BindGroupLayoutEntryBuilder {
        sampler(SamplerBindingType::Filtering)
            .visibility(ShaderStages::FRAGMENT)
            .count(NonZeroU32::new(LIGHTMAPS_PER_SLAB as u32).unwrap())
    }
}

mod entry {
    use crate::render::skin;
    use bevy_render::mesh::morph::MorphTargetsResource;

    use super::{JOINT_BUFFER_SIZE, MORPH_BUFFER_SIZE};
    use bevy_render::{
        render_resource::{
            BindGroupEntry, BindingResource, Buffer, BufferBinding, BufferSize, Sampler,
            TextureView, WgpuSampler, WgpuTextureView,
        },
        renderer::RenderDevice,
    };

    fn entry(binding: u32, size: Option<u64>, buffer: &Buffer) -> BindGroupEntry<'_> {
        BindGroupEntry {
            binding,
            resource: BindingResource::Buffer(BufferBinding {
                buffer,
                offset: 0,
                size: size.map(|size| BufferSize::new(size).unwrap()),
            }),
        }
    }
    pub(super) fn model(binding: u32, resource: BindingResource) -> BindGroupEntry {
        BindGroupEntry { binding, resource }
    }
    pub(super) fn skinning<'a>(
        render_device: &RenderDevice,
        binding: u32,
        buffer: &'a Buffer,
    ) -> BindGroupEntry<'a> {
        let size = if skin::skins_use_uniform_buffers(&render_device.limits()) {
            Some(JOINT_BUFFER_SIZE as u64)
        } else {
            None
        };
        entry(binding, size, buffer)
    }
    pub(super) fn weights<'a>(
        render_device: &'_ RenderDevice,
        binding: u32,
        buffer: &'a Buffer,
    ) -> BindGroupEntry<'a> {
        if skin::skins_use_uniform_buffers(&render_device.limits()) {
            entry(binding, Some(MORPH_BUFFER_SIZE as u64), buffer)
        } else {
            entry(binding, None, buffer)
        }
    }
    pub(super) fn targets(binding: u32, targets: MorphTargetsResource<'_>) -> BindGroupEntry<'_> {
        BindGroupEntry {
            binding,
            resource: match targets {
                MorphTargetsResource::Texture(texture_view) => {
                    BindingResource::TextureView(texture_view)
                }
                MorphTargetsResource::Storage(buffer) => buffer.as_entire_binding(),
            },
        }
    }
    pub(super) fn morph_descriptors(binding: u32, buffer: &Buffer) -> BindGroupEntry<'_> {
        entry(binding, None, buffer)
    }
    pub(super) fn lightmaps_texture_view(
        binding: u32,
        texture: &TextureView,
    ) -> BindGroupEntry<'_> {
        BindGroupEntry {
            binding,
            resource: BindingResource::TextureView(texture),
        }
    }
    pub(super) fn lightmaps_sampler(binding: u32, sampler: &Sampler) -> BindGroupEntry<'_> {
        BindGroupEntry {
            binding,
            resource: BindingResource::Sampler(sampler),
        }
    }
    pub(super) fn lightmaps_texture_view_array<'a>(
        binding: u32,
        textures: &'a [&'a WgpuTextureView],
    ) -> BindGroupEntry<'a> {
        BindGroupEntry {
            binding,
            resource: BindingResource::TextureViewArray(textures),
        }
    }
    pub(super) fn lightmaps_sampler_array<'a>(
        binding: u32,
        samplers: &'a [&'a WgpuSampler],
    ) -> BindGroupEntry<'a> {
        BindGroupEntry {
            binding,
            resource: BindingResource::SamplerArray(samplers),
        }
    }
}

#[derive(Clone)]
pub struct MeshLayouts {

    pub model_only: BindGroupLayoutDescriptor,

    pub lightmapped: BindGroupLayoutDescriptor,

    pub skinned: BindGroupLayoutDescriptor,

    pub skinned_motion: BindGroupLayoutDescriptor,

    pub morphed: BindGroupLayoutDescriptor,

    pub morphed_motion: BindGroupLayoutDescriptor,

    pub morphed_skinned: BindGroupLayoutDescriptor,

    pub morphed_skinned_motion: BindGroupLayoutDescriptor,
}

impl MeshLayouts {

    pub fn new(render_device: &RenderDevice, render_adapter: &RenderAdapter) -> Self {
        MeshLayouts {
            model_only: Self::model_only_layout(render_device),
            lightmapped: Self::lightmapped_layout(render_device, render_adapter),
            skinned: Self::skinned_layout(render_device),
            skinned_motion: Self::skinned_motion_layout(render_device),
            morphed: Self::morphed_layout(render_device),
            morphed_motion: Self::morphed_motion_layout(render_device),
            morphed_skinned: Self::morphed_skinned_layout(render_device),
            morphed_skinned_motion: Self::morphed_skinned_motion_layout(render_device),
        }
    }

    fn model_only_layout(render_device: &RenderDevice) -> BindGroupLayoutDescriptor {
        BindGroupLayoutDescriptor::new(
            "mesh_layout",
            &BindGroupLayoutEntries::single(
                ShaderStages::empty(),
                layout_entry::model(&render_device.limits()),
            ),
        )
    }

    fn skinned_layout(render_device: &RenderDevice) -> BindGroupLayoutDescriptor {
        BindGroupLayoutDescriptor::new(
            "skinned_mesh_layout",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::VERTEX,
                (
                    (0, layout_entry::model(&render_device.limits())),

                    (1, layout_entry::skinning(&render_device.limits())),
                ),
            ),
        )
    }

    fn skinned_motion_layout(render_device: &RenderDevice) -> BindGroupLayoutDescriptor {
        BindGroupLayoutDescriptor::new(
            "skinned_motion_mesh_layout",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::VERTEX,
                (
                    (0, layout_entry::model(&render_device.limits())),

                    (1, layout_entry::skinning(&render_device.limits())),

                    (6, layout_entry::skinning(&render_device.limits())),
                ),
            ),
        )
    }

    fn morphed_layout(render_device: &RenderDevice) -> BindGroupLayoutDescriptor {
        let limits = render_device.limits();

        let mut entries: ArrayVec<BindGroupLayoutEntry, 4> = ArrayVec::new();

        entries.extend(
            [
                (0, layout_entry::model(&limits)),

                (2, layout_entry::weights(&limits)),
                (3, layout_entry::targets(&limits)),
            ]
            .iter()
            .map(|(binding, entry)| entry.build(*binding, ShaderStages::VERTEX)),
        );

        if !skin::skins_use_uniform_buffers(&render_device.limits()) {
            entries.push(layout_entry::morph_descriptors().build(8, ShaderStages::VERTEX));
        }

        BindGroupLayoutDescriptor::new("morphed_mesh_layout", &entries)
    }

    fn morphed_motion_layout(render_device: &RenderDevice) -> BindGroupLayoutDescriptor {
        let limits = render_device.limits();

        let mut entries: ArrayVec<BindGroupLayoutEntry, 5> = ArrayVec::new();

        entries.extend(
            [
                (0, layout_entry::model(&limits)),

                (2, layout_entry::weights(&limits)),
                (3, layout_entry::targets(&limits)),

                (7, layout_entry::weights(&limits)),
            ]
            .iter()
            .map(|(binding, entry)| entry.build(*binding, ShaderStages::VERTEX)),
        );

        if !skin::skins_use_uniform_buffers(&render_device.limits()) {
            entries.push(layout_entry::morph_descriptors().build(8, ShaderStages::VERTEX));
        }

        BindGroupLayoutDescriptor::new("morphed_motion_layout", &entries)
    }

    fn morphed_skinned_layout(render_device: &RenderDevice) -> BindGroupLayoutDescriptor {
        let limits = render_device.limits();

        let mut entries: ArrayVec<BindGroupLayoutEntry, 5> = ArrayVec::new();

        entries.extend(
            [
                (0, layout_entry::model(&limits)),

                (1, layout_entry::skinning(&limits)),

                (2, layout_entry::weights(&limits)),
                (3, layout_entry::targets(&limits)),
            ]
            .iter()
            .map(|(binding, entry)| entry.build(*binding, ShaderStages::VERTEX)),
        );

        if !skin::skins_use_uniform_buffers(&render_device.limits()) {
            entries.push(layout_entry::morph_descriptors().build(8, ShaderStages::VERTEX));
        }

        BindGroupLayoutDescriptor::new("morphed_skinned_mesh_layout", &entries)
    }

    fn morphed_skinned_motion_layout(render_device: &RenderDevice) -> BindGroupLayoutDescriptor {
        let limits = render_device.limits();

        let mut entries: ArrayVec<BindGroupLayoutEntry, 7> = ArrayVec::new();

        entries.extend(
            [
                (0, layout_entry::model(&limits)),

                (1, layout_entry::skinning(&limits)),

                (2, layout_entry::weights(&limits)),
                (3, layout_entry::targets(&limits)),

                (6, layout_entry::skinning(&limits)),

                (7, layout_entry::weights(&limits)),
            ]
            .iter()
            .map(|(binding, entry)| entry.build(*binding, ShaderStages::VERTEX)),
        );

        if !skin::skins_use_uniform_buffers(&render_device.limits()) {
            entries.push(layout_entry::morph_descriptors().build(8, ShaderStages::VERTEX));
        }

        BindGroupLayoutDescriptor::new("morphed_skinned_motion_mesh_layout", &entries)
    }

    fn lightmapped_layout(
        render_device: &RenderDevice,
        render_adapter: &RenderAdapter,
    ) -> BindGroupLayoutDescriptor {
        if binding_arrays_are_usable(render_device, render_adapter) {
            BindGroupLayoutDescriptor::new(
                "lightmapped_mesh_layout",
                &BindGroupLayoutEntries::with_indices(
                    ShaderStages::VERTEX,
                    (
                        (0, layout_entry::model(&render_device.limits())),
                        (4, layout_entry::lightmaps_texture_view_array()),
                        (5, layout_entry::lightmaps_sampler_array()),
                    ),
                ),
            )
        } else {
            BindGroupLayoutDescriptor::new(
                "lightmapped_mesh_layout",
                &BindGroupLayoutEntries::with_indices(
                    ShaderStages::VERTEX,
                    (
                        (0, layout_entry::model(&render_device.limits())),
                        (4, layout_entry::lightmaps_texture_view()),
                        (5, layout_entry::lightmaps_sampler()),
                    ),
                ),
            )
        }
    }

    pub fn model_only(
        &self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        model: &BindingResource,
    ) -> BindGroup {
        render_device.create_bind_group(
            "model_only_mesh_bind_group",
            &pipeline_cache.get_bind_group_layout(&self.model_only),
            &[entry::model(0, model.clone())],
        )
    }

    pub fn lightmapped(
        &self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        model: &BindingResource,
        lightmap_slab: &LightmapSlab,
        bindless_lightmaps: bool,
    ) -> BindGroup {
        if bindless_lightmaps {
            let (texture_views, samplers) = lightmap_slab.build_binding_arrays();
            render_device.create_bind_group(
                "lightmapped_mesh_bind_group",
                &pipeline_cache.get_bind_group_layout(&self.lightmapped),
                &[
                    entry::model(0, model.clone()),
                    entry::lightmaps_texture_view_array(4, &texture_views),
                    entry::lightmaps_sampler_array(5, &samplers),
                ],
            )
        } else {
            let (texture_view, sampler) = lightmap_slab.bindings_for_first_lightmap();
            render_device.create_bind_group(
                "lightmapped_mesh_bind_group",
                &pipeline_cache.get_bind_group_layout(&self.lightmapped),
                &[
                    entry::model(0, model.clone()),
                    entry::lightmaps_texture_view(4, texture_view),
                    entry::lightmaps_sampler(5, sampler),
                ],
            )
        }
    }

    pub fn skinned(
        &self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        model: &BindingResource,
        current_skin: &Buffer,
    ) -> BindGroup {
        render_device.create_bind_group(
            "skinned_mesh_bind_group",
            &pipeline_cache.get_bind_group_layout(&self.skinned),
            &[
                entry::model(0, model.clone()),
                entry::skinning(render_device, 1, current_skin),
            ],
        )
    }

    pub fn skinned_motion(
        &self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        model: &BindingResource,
        current_skin: &Buffer,
        prev_skin: &Buffer,
    ) -> BindGroup {
        render_device.create_bind_group(
            "skinned_motion_mesh_bind_group",
            &pipeline_cache.get_bind_group_layout(&self.skinned_motion),
            &[
                entry::model(0, model.clone()),
                entry::skinning(render_device, 1, current_skin),
                entry::skinning(render_device, 6, prev_skin),
            ],
        )
    }

    pub fn morphed(
        &self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        model: &BindingResource,
        current_weights: &Buffer,
        targets: MorphTargetsResource,
        maybe_morph_descriptors: Option<&Buffer>,
    ) -> BindGroup {
        let mut entries: ArrayVec<BindGroupEntry, 4> = ArrayVec::new();

        entries.extend([
            entry::model(0, model.clone()),
            entry::weights(render_device, 2, current_weights),
            entry::targets(3, targets),
        ]);

        if let Some(morph_descriptors) = maybe_morph_descriptors {
            entries.push(entry::morph_descriptors(8, morph_descriptors));
        }

        render_device.create_bind_group(
            "morphed_mesh_bind_group",
            &pipeline_cache.get_bind_group_layout(&self.morphed),
            &entries,
        )
    }

    pub fn morphed_motion(
        &self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        model: &BindingResource,
        current_weights: &Buffer,
        prev_weights: &Buffer,
        targets: MorphTargetsResource,
        maybe_morph_descriptors: Option<&Buffer>,
    ) -> BindGroup {
        let mut entries: ArrayVec<BindGroupEntry, 5> = ArrayVec::new();

        entries.extend([
            entry::model(0, model.clone()),
            entry::weights(render_device, 2, current_weights),
            entry::targets(3, targets),
            entry::weights(render_device, 7, prev_weights),
        ]);

        if let Some(morph_descriptors) = maybe_morph_descriptors {
            entries.push(entry::morph_descriptors(8, morph_descriptors));
        }

        render_device.create_bind_group(
            "morphed_motion_mesh_bind_group",
            &pipeline_cache.get_bind_group_layout(&self.morphed_motion),
            &entries,
        )
    }

    pub fn morphed_skinned(
        &self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        model: &BindingResource,
        current_skin: &Buffer,
        current_weights: &Buffer,
        targets: MorphTargetsResource,
        maybe_morph_descriptors: Option<&Buffer>,
    ) -> BindGroup {
        let mut entries: ArrayVec<BindGroupEntry, 5> = ArrayVec::new();

        entries.extend([
            entry::model(0, model.clone()),
            entry::skinning(render_device, 1, current_skin),
            entry::weights(render_device, 2, current_weights),
            entry::targets(3, targets),
        ]);

        if let Some(morph_descriptors) = maybe_morph_descriptors {
            entries.push(entry::morph_descriptors(8, morph_descriptors));
        }

        render_device.create_bind_group(
            "morphed_skinned_mesh_bind_group",
            &pipeline_cache.get_bind_group_layout(&self.morphed_skinned),
            &entries,
        )
    }

    pub fn morphed_skinned_motion(
        &self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        model: &BindingResource,
        current_skin: &Buffer,
        current_weights: &Buffer,
        targets: MorphTargetsResource,
        prev_skin: &Buffer,
        prev_weights: &Buffer,
        morph_descriptors: Option<&Buffer>,
    ) -> BindGroup {
        let mut entries: ArrayVec<BindGroupEntry, 7> = ArrayVec::new();

        entries.extend([
            entry::model(0, model.clone()),
            entry::skinning(render_device, 1, current_skin),
            entry::weights(render_device, 2, current_weights),
            entry::targets(3, targets),
            entry::skinning(render_device, 6, prev_skin),
            entry::weights(render_device, 7, prev_weights),
        ]);

        if let Some(morph_descriptors) = morph_descriptors {
            entries.push(entry::morph_descriptors(8, morph_descriptors));
        }

        render_device.create_bind_group(
            "morphed_skinned_motion_mesh_bind_group",
            &pipeline_cache.get_bind_group_layout(&self.morphed_skinned_motion),
            &entries,
        )
    }
}
