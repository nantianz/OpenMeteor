use crate::{
    ExtendedMaterial, Material, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
    MaterialPlugin, StandardMaterial,
};
use bevy_app::{App, Plugin};
use bevy_asset::{Asset, Assets, Handle};
use bevy_ecs::{
    component::Component, lifecycle::HookContext, resource::Resource, world::DeferredWorld,
};
use bevy_material::AlphaMode;
use bevy_math::{prelude::Rectangle, Quat, Vec2, Vec3};
use bevy_mesh::{Mesh, Mesh3d, MeshBuilder, MeshVertexBufferLayoutRef, Meshable};
use bevy_reflect::{Reflect, TypePath};
use bevy_render::{
    render_asset::RenderAssets,
    render_resource::{
        AsBindGroup, AsBindGroupShaderType, CompareFunction, RenderPipelineDescriptor, ShaderType,
        SpecializedMeshPipelineError,
    },
    texture::GpuImage,
    RenderDebugFlags,
};
use bevy_shader::load_shader_library;

pub struct ForwardDecalPlugin;

impl Plugin for ForwardDecalPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "forward_decal.wgsl");

        let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(
            Rectangle::from_size(Vec2::ONE)
                .mesh()
                .build()
                .rotated_by(Quat::from_rotation_arc(Vec3::Z, Vec3::Y))
                .with_generated_tangents()
                .unwrap(),
        );

        app.insert_resource(ForwardDecalMesh(mesh));

        app.add_plugins(MaterialPlugin::<ForwardDecalMaterial<StandardMaterial>> {
            debug_flags: RenderDebugFlags::default(),
            ..Default::default()
        });
    }
}

#[derive(Component, Reflect)]
#[require(Mesh3d)]
#[component(on_add=forward_decal_set_mesh)]
pub struct ForwardDecal;

#[expect(type_alias_bounds, reason = "Type alias generics not yet stable")]
pub type ForwardDecalMaterial<B: Material> = ExtendedMaterial<B, ForwardDecalMaterialExt>;

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
#[uniform(200, ForwardDecalMaterialExtUniform)]
pub struct ForwardDecalMaterialExt {

    pub depth_fade_factor: f32,
}

#[derive(Clone, Default, ShaderType)]
pub struct ForwardDecalMaterialExtUniform {
    pub inv_depth_fade_factor: f32,
}

impl AsBindGroupShaderType<ForwardDecalMaterialExtUniform> for ForwardDecalMaterialExt {
    fn as_bind_group_shader_type(
        &self,
        _images: &RenderAssets<GpuImage>,
    ) -> ForwardDecalMaterialExtUniform {
        ForwardDecalMaterialExtUniform {
            inv_depth_fade_factor: 1.0 / self.depth_fade_factor.max(0.001),
        }
    }
}

impl MaterialExtension for ForwardDecalMaterialExt {
    fn alpha_mode() -> Option<AlphaMode> {
        Some(AlphaMode::Blend)
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.depth_stencil.as_mut().unwrap().depth_compare = Some(CompareFunction::Always);

        descriptor.vertex.shader_defs.push("FORWARD_DECAL".into());

        if let Some(fragment) = &mut descriptor.fragment {
            fragment.shader_defs.push("FORWARD_DECAL".into());
        }

        if let Some(label) = &mut descriptor.label {
            *label = format!("forward_decal_{label}").into();
        }

        Ok(())
    }
}

impl Default for ForwardDecalMaterialExt {
    fn default() -> Self {
        Self {
            depth_fade_factor: 8.0,
        }
    }
}

#[derive(Resource)]
struct ForwardDecalMesh(Handle<Mesh>);

fn forward_decal_set_mesh(mut world: DeferredWorld, HookContext { entity, .. }: HookContext) {
    let decal_mesh = world.resource::<ForwardDecalMesh>().0.clone();
    let mut entity = world.entity_mut(entity);
    let mut entity_mesh = entity.get_mut::<Mesh3d>().unwrap();

    if **entity_mesh == Handle::default() {
        entity_mesh.0 = decal_mesh;
    }
}
