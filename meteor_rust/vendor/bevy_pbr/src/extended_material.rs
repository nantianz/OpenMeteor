use alloc::borrow::Cow;

use bevy_asset::Asset;
use bevy_ecs::system::SystemParamItem;
use bevy_material::{AlphaMode, OpaqueRendererMethod};
use bevy_mesh::MeshVertexBufferLayoutRef;
use bevy_platform::{collections::HashSet, hash::FixedHasher};
use bevy_reflect::{impl_type_path, Reflect};
use bevy_render::{
    render_resource::{
        AsBindGroup, AsBindGroupError, BindGroupLayout, BindGroupLayoutEntry, BindlessDescriptor,
        BindlessResourceType, BindlessSlabResourceLimit, RenderPipelineDescriptor,
        SpecializedMeshPipelineError, UnpreparedBindGroup,
    },
    renderer::RenderDevice,
};
use bevy_shader::ShaderRef;

use crate::{Material, MaterialPipeline, MaterialPipelineKey, MeshPipeline, MeshPipelineKey};

pub struct MaterialExtensionPipeline {
    pub mesh_pipeline: MeshPipeline,
}

pub struct MaterialExtensionKey<E: MaterialExtension> {
    pub mesh_key: MeshPipelineKey,
    pub bind_group_data: E::Data,
}

pub trait MaterialExtension: Asset + AsBindGroup + Clone + Sized {

    fn vertex_shader() -> ShaderRef {
        ShaderRef::Default
    }

    fn fragment_shader() -> ShaderRef {
        ShaderRef::Default
    }

    fn alpha_mode() -> Option<AlphaMode> {
        None
    }

    #[inline]
    fn enable_prepass() -> bool {
        true
    }

    #[inline]
    fn enable_shadows() -> bool {
        true
    }

    fn prepass_vertex_shader() -> ShaderRef {
        ShaderRef::Default
    }

    fn prepass_fragment_shader() -> ShaderRef {
        ShaderRef::Default
    }

    fn deferred_vertex_shader() -> ShaderRef {
        ShaderRef::Default
    }

    fn deferred_fragment_shader() -> ShaderRef {
        ShaderRef::Default
    }

    #[cfg(feature = "meshlet")]
    fn meshlet_mesh_fragment_shader() -> ShaderRef {
        ShaderRef::Default
    }

    #[cfg(feature = "meshlet")]
    fn meshlet_mesh_prepass_fragment_shader() -> ShaderRef {
        ShaderRef::Default
    }

    #[cfg(feature = "meshlet")]
    fn meshlet_mesh_deferred_fragment_shader() -> ShaderRef {
        ShaderRef::Default
    }

    #[expect(
        unused_variables,
        reason = "The parameters here are intentionally unused by the default implementation; however, putting underscores here will result in the underscores being copied by rust-analyzer's tab completion."
    )]
    #[inline]
    fn specialize(
        pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        Ok(())
    }
}

#[derive(Asset, Clone, Debug, Reflect)]
#[reflect(type_path = false)]
#[reflect(Clone)]
pub struct ExtendedMaterial<B: Material, E: MaterialExtension> {
    pub base: B,
    pub extension: E,
}

impl<B, E> Default for ExtendedMaterial<B, E>
where
    B: Material + Default,
    E: MaterialExtension + Default,
{
    fn default() -> Self {
        Self {
            base: B::default(),
            extension: E::default(),
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(C, packed)]
pub struct MaterialExtensionBindGroupData<B, E> {
    pub base: B,
    pub extension: E,
}

impl_type_path!((in bevy_pbr::extended_material) ExtendedMaterial<B: Material, E: MaterialExtension>);

impl<B: Material, E: MaterialExtension> AsBindGroup for ExtendedMaterial<B, E> {
    type Data = MaterialExtensionBindGroupData<B::Data, E::Data>;
    type Param = (<B as AsBindGroup>::Param, <E as AsBindGroup>::Param);

    fn bindless_slot_count() -> Option<BindlessSlabResourceLimit> {

        match (B::bindless_slot_count()?, E::bindless_slot_count()?) {
            (BindlessSlabResourceLimit::Auto, BindlessSlabResourceLimit::Auto) => {
                Some(BindlessSlabResourceLimit::Auto)
            }
            (BindlessSlabResourceLimit::Auto, BindlessSlabResourceLimit::Custom(limit))
            | (BindlessSlabResourceLimit::Custom(limit), BindlessSlabResourceLimit::Auto) => {
                Some(BindlessSlabResourceLimit::Custom(limit))
            }
            (
                BindlessSlabResourceLimit::Custom(base_limit),
                BindlessSlabResourceLimit::Custom(extended_limit),
            ) => Some(BindlessSlabResourceLimit::Custom(
                base_limit.min(extended_limit),
            )),
        }
    }

    fn bindless_supported(render_device: &RenderDevice) -> bool {
        B::bindless_supported(render_device) && E::bindless_supported(render_device)
    }

    fn label() -> &'static str {
        E::label()
    }

    fn bind_group_data(&self) -> Self::Data {
        MaterialExtensionBindGroupData {
            base: self.base.bind_group_data(),
            extension: self.extension.bind_group_data(),
        }
    }

    fn unprepared_bind_group(
        &self,
        layout: &BindGroupLayout,
        render_device: &RenderDevice,
        (base_param, extended_param): &mut SystemParamItem<'_, '_, Self::Param>,
        mut force_non_bindless: bool,
    ) -> Result<UnpreparedBindGroup, AsBindGroupError> {
        force_non_bindless = force_non_bindless || Self::bindless_slot_count().is_none();

        let UnpreparedBindGroup { mut bindings } = B::unprepared_bind_group(
            &self.base,
            layout,
            render_device,
            base_param,
            force_non_bindless,
        )?;
        let UnpreparedBindGroup {
            bindings: extension_bindings,
        } = E::unprepared_bind_group(
            &self.extension,
            layout,
            render_device,
            extended_param,
            force_non_bindless,
        )?;

        bindings.extend(extension_bindings.0);

        Ok(UnpreparedBindGroup { bindings })
    }

    fn bind_group_layout_entries(
        render_device: &RenderDevice,
        mut force_non_bindless: bool,
    ) -> Vec<BindGroupLayoutEntry>
    where
        Self: Sized,
    {
        force_non_bindless = force_non_bindless || Self::bindless_slot_count().is_none();

        let base_entries = B::bind_group_layout_entries(render_device, force_non_bindless);
        let extension_entries = E::bind_group_layout_entries(render_device, force_non_bindless);

        let mut seen_bindings = HashSet::<u32>::with_hasher(FixedHasher);

        base_entries
            .into_iter()
            .chain(extension_entries)
            .filter(|entry| seen_bindings.insert(entry.binding))
            .collect()
    }

    fn bindless_descriptor() -> Option<BindlessDescriptor> {

        let base_bindless_descriptor = B::bindless_descriptor()?;
        let extended_bindless_descriptor = E::bindless_descriptor()?;

        let mut buffers = base_bindless_descriptor.buffers.to_vec();
        let mut index_tables = base_bindless_descriptor.index_tables.to_vec();

        buffers.extend(extended_bindless_descriptor.buffers.iter().cloned());
        index_tables.extend(extended_bindless_descriptor.index_tables.iter().cloned());

        let max_bindless_index = base_bindless_descriptor
            .resources
            .len()
            .max(extended_bindless_descriptor.resources.len());
        let mut resources = Vec::with_capacity(max_bindless_index);
        for bindless_index in 0..max_bindless_index {

            match base_bindless_descriptor.resources.get(bindless_index) {
                None | Some(&BindlessResourceType::None) => resources.push(
                    extended_bindless_descriptor
                        .resources
                        .get(bindless_index)
                        .copied()
                        .unwrap_or(BindlessResourceType::None),
                ),
                Some(&resource_type) => resources.push(resource_type),
            }
        }

        Some(BindlessDescriptor {
            resources: Cow::Owned(resources),
            buffers: Cow::Owned(buffers),
            index_tables: Cow::Owned(index_tables),
        })
    }
}

impl<B: Material, E: MaterialExtension> Material for ExtendedMaterial<B, E> {
    fn vertex_shader() -> ShaderRef {
        match E::vertex_shader() {
            ShaderRef::Default => B::vertex_shader(),
            specified => specified,
        }
    }

    fn fragment_shader() -> ShaderRef {
        match E::fragment_shader() {
            ShaderRef::Default => B::fragment_shader(),
            specified => specified,
        }
    }

    fn alpha_mode(&self) -> AlphaMode {
        match E::alpha_mode() {
            Some(specified) => specified,
            None => B::alpha_mode(&self.base),
        }
    }

    fn opaque_render_method(&self) -> OpaqueRendererMethod {
        B::opaque_render_method(&self.base)
    }

    fn depth_bias(&self) -> f32 {
        B::depth_bias(&self.base)
    }

    fn reads_view_transmission_texture(&self) -> bool {
        B::reads_view_transmission_texture(&self.base)
    }

    fn enable_prepass() -> bool {
        E::enable_prepass()
    }

    fn enable_shadows() -> bool {
        E::enable_shadows()
    }

    fn prepass_vertex_shader() -> ShaderRef {
        match E::prepass_vertex_shader() {
            ShaderRef::Default => B::prepass_vertex_shader(),
            specified => specified,
        }
    }

    fn prepass_fragment_shader() -> ShaderRef {
        match E::prepass_fragment_shader() {
            ShaderRef::Default => B::prepass_fragment_shader(),
            specified => specified,
        }
    }

    fn deferred_vertex_shader() -> ShaderRef {
        match E::deferred_vertex_shader() {
            ShaderRef::Default => B::deferred_vertex_shader(),
            specified => specified,
        }
    }

    fn deferred_fragment_shader() -> ShaderRef {
        match E::deferred_fragment_shader() {
            ShaderRef::Default => B::deferred_fragment_shader(),
            specified => specified,
        }
    }

    #[cfg(feature = "meshlet")]
    fn meshlet_mesh_fragment_shader() -> ShaderRef {
        match E::meshlet_mesh_fragment_shader() {
            ShaderRef::Default => B::meshlet_mesh_fragment_shader(),
            specified => specified,
        }
    }

    #[cfg(feature = "meshlet")]
    fn meshlet_mesh_prepass_fragment_shader() -> ShaderRef {
        match E::meshlet_mesh_prepass_fragment_shader() {
            ShaderRef::Default => B::meshlet_mesh_prepass_fragment_shader(),
            specified => specified,
        }
    }

    #[cfg(feature = "meshlet")]
    fn meshlet_mesh_deferred_fragment_shader() -> ShaderRef {
        match E::meshlet_mesh_deferred_fragment_shader() {
            ShaderRef::Default => B::meshlet_mesh_deferred_fragment_shader(),
            specified => specified,
        }
    }

    fn specialize(
        pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {

        let base_key = MaterialPipelineKey::<B> {
            mesh_key: key.mesh_key,
            bind_group_data: key.bind_group_data.base,
        };
        B::specialize(pipeline, descriptor, layout, base_key)?;

        E::specialize(
            &MaterialExtensionPipeline {
                mesh_pipeline: pipeline.mesh_pipeline.clone(),
            },
            descriptor,
            layout,
            MaterialExtensionKey {
                mesh_key: key.mesh_key,
                bind_group_data: key.bind_group_data.extension,
            },
        )
    }
}
