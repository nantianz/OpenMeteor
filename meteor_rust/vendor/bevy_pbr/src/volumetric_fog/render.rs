

use core::array;

use bevy_asset::{load_embedded_asset, AssetId, AssetServer, Handle};
use bevy_camera::Camera3d;
use bevy_color::ColorToComponents as _;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    component::Component,
    entity::Entity,
    query::With,
    resource::Resource,
    system::{Commands, Local, Query, Res, ResMut},
};
use bevy_image::Image;
use bevy_light::{FogVolume, VolumetricFog, VolumetricLight};
use bevy_math::{vec4, Affine3A, Mat4, Vec3, Vec3A, Vec4};
use bevy_mesh::{Mesh, MeshVertexBufferLayoutRef};
use bevy_render::{
    mesh::{allocator::MeshAllocator, RenderMesh, RenderMeshBufferInfo},
    render_asset::RenderAssets,
    render_resource::{
        binding_types::{
            sampler, texture_3d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer,
        },
        BindGroupLayoutDescriptor, BindGroupLayoutEntries, BindingResource, BlendComponent,
        BlendFactor, BlendOperation, BlendState, CachedRenderPipelineId, ColorTargetState,
        ColorWrites, DynamicBindGroupEntries, DynamicUniformBuffer, Face, FragmentState, LoadOp,
        Operations, PipelineCache, PrimitiveState, RenderPassColorAttachment, RenderPassDescriptor,
        RenderPipelineDescriptor, SamplerBindingType, ShaderStages, ShaderType,
        SpecializedRenderPipeline, SpecializedRenderPipelines, StoreOp, TextureFormat,
        TextureSampleType, TextureUsages, VertexState,
    },
    renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
    sync_world::RenderEntity,
    texture::GpuImage,
    view::{ExtractedView, Msaa, ViewDepthTexture, ViewTarget},
    Extract,
};
use bevy_shader::Shader;
use bevy_transform::components::GlobalTransform;
use bevy_utils::prelude::default;
use bitflags::bitflags;

use crate::{MeshPipelineViewLayoutKey, MeshPipelineViewLayouts, MeshViewBindGroup, ViewKeyCache};

use super::FogAssets;

bitflags! {

    #[derive(Clone, Copy, PartialEq)]
    struct VolumetricFogBindGroupLayoutKey: u8 {

        const MULTISAMPLED = 0x1;

        const DENSITY_TEXTURE = 0x2;
    }
}

const VOLUMETRIC_FOG_BIND_GROUP_LAYOUT_COUNT: usize =
    VolumetricFogBindGroupLayoutKey::all().bits() as usize + 1;

static UVW_FROM_LOCAL: Mat4 = Mat4::from_cols(
    vec4(1.0, 0.0, 0.0, 0.0),
    vec4(0.0, 1.0, 0.0, 0.0),
    vec4(0.0, 0.0, 1.0, 0.0),
    vec4(0.5, 0.5, 0.5, 1.0),
);

#[derive(Resource)]
pub struct VolumetricFogPipeline {

    mesh_view_layouts: MeshPipelineViewLayouts,

    volumetric_view_bind_group_layouts:
        [BindGroupLayoutDescriptor; VOLUMETRIC_FOG_BIND_GROUP_LAYOUT_COUNT],

    shader: Handle<Shader>,
}

#[derive(Component)]
pub struct ViewVolumetricFogPipelines {

    pub textureless: CachedRenderPipelineId,

    pub textured: CachedRenderPipelineId,
}

#[derive(PartialEq, Eq, Hash, Clone)]
pub struct VolumetricFogPipelineKey {

    mesh_pipeline_view_key: MeshPipelineViewLayoutKey,

    vertex_buffer_layout: MeshVertexBufferLayoutRef,

    target_format: TextureFormat,

    has_density_texture: bool,
}

#[derive(ShaderType)]
pub struct VolumetricFogUniform {
    clip_from_local: Mat4,

    uvw_from_world: Mat4,

    far_planes: [Vec4; 6],

    fog_color: Vec3,
    light_tint: Vec3,
    ambient_color: Vec3,
    ambient_intensity: f32,
    step_count: u32,

    bounding_radius: f32,

    absorption: f32,
    scattering: f32,
    density: f32,
    density_texture_offset: Vec3,
    scattering_asymmetry: f32,
    light_intensity: f32,
    jitter_strength: f32,
}

#[derive(Component, Deref, DerefMut)]
pub struct ViewVolumetricFog(Vec<ViewFogVolume>);

pub struct ViewFogVolume {

    density_texture: Option<AssetId<Image>>,

    uniform_buffer_offset: u32,

    exterior: bool,
}

#[derive(Resource, Default, Deref, DerefMut)]
pub struct VolumetricFogUniformBuffer(pub DynamicUniformBuffer<VolumetricFogUniform>);

pub fn init_volumetric_fog_pipeline(
    mut commands: Commands,
    mesh_view_layouts: Res<MeshPipelineViewLayouts>,
    asset_server: Res<AssetServer>,
) {

    let base_bind_group_layout_entries = &BindGroupLayoutEntries::single(
        ShaderStages::VERTEX_FRAGMENT,

        uniform_buffer::<VolumetricFogUniform>(true),
    );

    let bind_group_layouts = array::from_fn(|bits| {
        let flags = VolumetricFogBindGroupLayoutKey::from_bits_retain(bits as u8);

        let mut bind_group_layout_entries = base_bind_group_layout_entries.to_vec();

        bind_group_layout_entries.extend_from_slice(&BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            ((
                1,
                if flags.contains(VolumetricFogBindGroupLayoutKey::MULTISAMPLED) {
                    texture_depth_2d_multisampled()
                } else {
                    texture_depth_2d()
                },
            ),),
        ));

        if flags.contains(VolumetricFogBindGroupLayoutKey::DENSITY_TEXTURE) {
            bind_group_layout_entries.extend_from_slice(&BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                (
                    (2, texture_3d(TextureSampleType::Float { filterable: true })),
                    (3, sampler(SamplerBindingType::Filtering)),
                ),
            ));
        }

        let description = flags.bind_group_layout_description();
        BindGroupLayoutDescriptor::new(description, &bind_group_layout_entries)
    });

    commands.insert_resource(VolumetricFogPipeline {
        mesh_view_layouts: mesh_view_layouts.clone(),
        volumetric_view_bind_group_layouts: bind_group_layouts,
        shader: load_embedded_asset!(asset_server.as_ref(), "volumetric_fog.wgsl"),
    });
}

pub fn extract_volumetric_fog(
    mut commands: Commands,
    view_targets: Extract<Query<(RenderEntity, &VolumetricFog)>>,
    fog_volumes: Extract<Query<(RenderEntity, &FogVolume, &GlobalTransform)>>,
    volumetric_lights: Extract<Query<(RenderEntity, &VolumetricLight)>>,
) {
    if volumetric_lights.is_empty() {

        for (entity, ..) in view_targets.iter() {
            commands
                .entity(entity)
                .remove::<(VolumetricFog, ViewVolumetricFogPipelines, ViewVolumetricFog)>();
        }
        for (entity, ..) in fog_volumes.iter() {
            commands.entity(entity).remove::<FogVolume>();
        }
        return;
    }

    for (entity, volumetric_fog) in view_targets.iter() {
        commands
            .get_entity(entity)
            .expect("Volumetric fog entity wasn't synced.")
            .insert(*volumetric_fog);
    }

    for (entity, fog_volume, fog_transform) in fog_volumes.iter() {
        commands
            .get_entity(entity)
            .expect("Fog volume entity wasn't synced.")
            .insert((*fog_volume).clone())
            .insert(*fog_transform);
    }

    for (entity, volumetric_light) in volumetric_lights.iter() {
        commands
            .get_entity(entity)
            .expect("Volumetric light entity wasn't synced.")
            .insert(*volumetric_light);
    }
}

pub fn volumetric_fog(
    view: ViewQuery<(
        &ViewTarget,
        &ViewDepthTexture,
        &ViewVolumetricFogPipelines,
        &ViewVolumetricFog,
        &MeshViewBindGroup,
        &Msaa,
    )>,
    pipeline_cache: Res<PipelineCache>,
    volumetric_lighting_pipeline: Res<VolumetricFogPipeline>,
    volumetric_lighting_uniform_buffers: Res<VolumetricFogUniformBuffer>,
    image_assets: Res<RenderAssets<GpuImage>>,
    mesh_allocator: Res<MeshAllocator>,
    fog_assets: Res<FogAssets>,
    render_meshes: Res<RenderAssets<RenderMesh>>,
    mut ctx: RenderContext,
) {
    let (
        view_target,
        view_depth_texture,
        view_volumetric_lighting_pipelines,
        view_fog_volumes,
        view_bind_group,
        msaa,
    ) = view.into_inner();

    let (
        Some(textureless_pipeline),
        Some(textured_pipeline),
        Some(volumetric_lighting_uniform_buffer_binding),
    ) = (
        pipeline_cache.get_render_pipeline(view_volumetric_lighting_pipelines.textureless),
        pipeline_cache.get_render_pipeline(view_volumetric_lighting_pipelines.textured),
        volumetric_lighting_uniform_buffers.binding(),
    )
    else {
        return;
    };

    let command_encoder = ctx.command_encoder();
    command_encoder.push_debug_group("volumetric_lighting");

    for view_fog_volume in view_fog_volumes.iter() {

        let mesh_handle = if view_fog_volume.exterior {
            fog_assets.cube_mesh.clone()
        } else {
            fog_assets.plane_mesh.clone()
        };

        let Some(vertex_buffer_slice) = mesh_allocator.mesh_vertex_slice(&mesh_handle.id()) else {
            continue;
        };

        let density_image = view_fog_volume
            .density_texture
            .and_then(|density_texture| image_assets.get(density_texture));

        let pipeline = if density_image.is_some() {
            textured_pipeline
        } else {
            textureless_pipeline
        };

        let Some(render_mesh) = render_meshes.get(&mesh_handle) else {
            return;
        };

        let mut bind_group_layout_key = VolumetricFogBindGroupLayoutKey::empty();
        bind_group_layout_key.set(
            VolumetricFogBindGroupLayoutKey::MULTISAMPLED,
            !matches!(*msaa, Msaa::Off),
        );

        let mut bind_group_entries = DynamicBindGroupEntries::sequential((
            volumetric_lighting_uniform_buffer_binding.clone(),
            BindingResource::TextureView(view_depth_texture.view()),
        ));
        if let Some(density_image) = density_image {
            bind_group_layout_key.insert(VolumetricFogBindGroupLayoutKey::DENSITY_TEXTURE);
            bind_group_entries = bind_group_entries.extend_sequential((
                BindingResource::TextureView(&density_image.texture_view),
                BindingResource::Sampler(&density_image.sampler),
            ));
        }

        let volumetric_view_bind_group_layout = &volumetric_lighting_pipeline
            .volumetric_view_bind_group_layouts[bind_group_layout_key.bits() as usize];

        let volumetric_view_bind_group = ctx.render_device().create_bind_group(
            None,
            &pipeline_cache.get_bind_group_layout(volumetric_view_bind_group_layout),
            &bind_group_entries,
        );

        let render_pass_descriptor = RenderPassDescriptor {
            label: Some("volumetric lighting pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: view_target.main_texture_view(),
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Load,
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        };

        let command_encoder = ctx.command_encoder();
        let mut render_pass = command_encoder.begin_render_pass(&render_pass_descriptor);

        render_pass.set_vertex_buffer(0, *vertex_buffer_slice.buffer.slice(..));
        render_pass.set_pipeline(pipeline);

        render_pass.set_bind_group(0, &view_bind_group.main, &view_bind_group.main_offsets);
        render_pass.set_bind_group(
            1,
            &volumetric_view_bind_group,
            &[view_fog_volume.uniform_buffer_offset],
        );

        match &render_mesh.buffer_info {
            RenderMeshBufferInfo::Indexed {
                index_format,
                count,
            } => {
                let Some(index_buffer_slice) = mesh_allocator.mesh_index_slice(&mesh_handle.id())
                else {
                    continue;
                };

                render_pass.set_index_buffer(*index_buffer_slice.buffer.slice(..), *index_format);
                render_pass.draw_indexed(
                    index_buffer_slice.range.start..(index_buffer_slice.range.start + count),
                    vertex_buffer_slice.range.start as i32,
                    0..1,
                );
            }
            RenderMeshBufferInfo::NonIndexed => {
                render_pass.draw(vertex_buffer_slice.range, 0..1);
            }
        }
    }

    ctx.command_encoder().pop_debug_group();
}

impl SpecializedRenderPipeline for VolumetricFogPipeline {
    type Key = VolumetricFogPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {

        let mut shader_defs = vec!["SHADOW_FILTER_METHOD_HARDWARE_2X2".into()];

        let mut bind_group_layout_key = VolumetricFogBindGroupLayoutKey::empty();
        bind_group_layout_key.set(
            VolumetricFogBindGroupLayoutKey::MULTISAMPLED,
            key.mesh_pipeline_view_key
                .contains(MeshPipelineViewLayoutKey::MULTISAMPLED),
        );
        bind_group_layout_key.set(
            VolumetricFogBindGroupLayoutKey::DENSITY_TEXTURE,
            key.has_density_texture,
        );

        let volumetric_view_bind_group_layout =
            self.volumetric_view_bind_group_layouts[bind_group_layout_key.bits() as usize].clone();

        let vertex_format = key
            .vertex_buffer_layout
            .0
            .get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0)])
            .expect("Failed to get vertex layout for volumetric fog hull");

        if key
            .mesh_pipeline_view_key
            .contains(MeshPipelineViewLayoutKey::MULTISAMPLED)
        {
            shader_defs.push("MULTISAMPLED".into());
        }

        if key
            .mesh_pipeline_view_key
            .contains(MeshPipelineViewLayoutKey::ATMOSPHERE)
        {
            shader_defs.push("ATMOSPHERE".into());
        }

        if key.has_density_texture {
            shader_defs.push("DENSITY_TEXTURE".into());
        }

        let layout = self
            .mesh_view_layouts
            .get_view_layout(key.mesh_pipeline_view_key);
        let layout = vec![
            layout.main_layout,
            volumetric_view_bind_group_layout.clone(),
        ];

        RenderPipelineDescriptor {
            label: Some("volumetric lighting pipeline".into()),
            layout,
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: shader_defs.clone(),
                buffers: vec![vertex_format],
                ..default()
            },
            primitive: PrimitiveState {
                cull_mode: Some(Face::Back),
                ..default()
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs,
                targets: vec![Some(ColorTargetState {
                    format: key.target_format,

                    blend: Some(BlendState {
                        color: BlendComponent {
                            src_factor: BlendFactor::One,
                            dst_factor: BlendFactor::OneMinusSrcAlpha,
                            operation: BlendOperation::Add,
                        },
                        alpha: BlendComponent {
                            src_factor: BlendFactor::Zero,
                            dst_factor: BlendFactor::One,
                            operation: BlendOperation::Add,
                        },
                    }),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }
    }
}

pub fn prepare_volumetric_fog_pipelines(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<VolumetricFogPipeline>>,
    volumetric_lighting_pipeline: Res<VolumetricFogPipeline>,
    fog_assets: Res<FogAssets>,
    view_targets: Query<(Entity, &ExtractedView), With<VolumetricFog>>,
    meshes: Res<RenderAssets<RenderMesh>>,
    view_key_cache: Res<ViewKeyCache>,
) {
    let Some(plane_mesh) = meshes.get(&fog_assets.plane_mesh) else {

        return;
    };

    for (entity, view) in view_targets.iter() {
        let Some(mesh_pipeline_key) = view_key_cache.get(&view.retained_view_entity) else {
            continue;
        };

        let textureless_pipeline_key = VolumetricFogPipelineKey {
            mesh_pipeline_view_key: (*mesh_pipeline_key).into(),
            vertex_buffer_layout: plane_mesh.layout.clone(),
            target_format: view.target_format,
            has_density_texture: false,
        };
        let textureless_pipeline_id = pipelines.specialize(
            &pipeline_cache,
            &volumetric_lighting_pipeline,
            textureless_pipeline_key.clone(),
        );
        let textured_pipeline_id = pipelines.specialize(
            &pipeline_cache,
            &volumetric_lighting_pipeline,
            VolumetricFogPipelineKey {
                has_density_texture: true,
                ..textureless_pipeline_key
            },
        );

        commands.entity(entity).insert(ViewVolumetricFogPipelines {
            textureless: textureless_pipeline_id,
            textured: textured_pipeline_id,
        });
    }
}

pub fn prepare_volumetric_fog_uniforms(
    mut commands: Commands,
    mut volumetric_lighting_uniform_buffer: ResMut<VolumetricFogUniformBuffer>,
    view_targets: Query<(Entity, &ExtractedView, &VolumetricFog)>,
    fog_volumes: Query<(Entity, &FogVolume, &GlobalTransform)>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut local_from_world_matrices: Local<Vec<Affine3A>>,
) {

    local_from_world_matrices.clear();
    for (_, _, fog_transform) in fog_volumes.iter() {
        local_from_world_matrices.push(fog_transform.affine().inverse());
    }

    let uniform_count = view_targets.iter().len() * local_from_world_matrices.len();

    let Some(mut writer) =
        volumetric_lighting_uniform_buffer.get_writer(uniform_count, &render_device, &render_queue)
    else {
        return;
    };

    for (view_entity, extracted_view, volumetric_fog) in view_targets.iter() {
        let world_from_view = extracted_view.world_from_view.affine();

        let mut view_fog_volumes = vec![];

        for ((_, fog_volume, _), local_from_world) in
            fog_volumes.iter().zip(local_from_world_matrices.iter())
        {

            let local_from_view = *local_from_world * world_from_view;
            let view_from_local = local_from_view.inverse();

            let interior = camera_is_inside_fog_volume(&local_from_view);
            let hull_clip_from_local = calculate_fog_volume_clip_from_local_transforms(
                interior,
                &extracted_view.clip_from_view,
                &view_from_local,
            );

            let bounding_radius = view_from_local
                .transform_vector3a(Vec3A::splat(0.5))
                .length();

            let uniform_buffer_offset = writer.write(&VolumetricFogUniform {
                clip_from_local: hull_clip_from_local,
                uvw_from_world: UVW_FROM_LOCAL * *local_from_world,
                far_planes: get_far_planes(&view_from_local),
                fog_color: fog_volume.fog_color.to_linear().to_vec3(),
                light_tint: fog_volume.light_tint.to_linear().to_vec3(),
                ambient_color: volumetric_fog.ambient_color.to_linear().to_vec3(),
                ambient_intensity: volumetric_fog.ambient_intensity,
                step_count: volumetric_fog.step_count,
                bounding_radius,
                absorption: fog_volume.absorption,
                scattering: fog_volume.scattering,
                density: fog_volume.density_factor,
                density_texture_offset: fog_volume.density_texture_offset,
                scattering_asymmetry: fog_volume.scattering_asymmetry,
                light_intensity: fog_volume.light_intensity,
                jitter_strength: volumetric_fog.jitter,
            });

            view_fog_volumes.push(ViewFogVolume {
                uniform_buffer_offset,
                exterior: !interior,
                density_texture: fog_volume.density_texture.as_ref().map(Handle::id),
            });
        }

        commands
            .entity(view_entity)
            .insert(ViewVolumetricFog(view_fog_volumes));
    }
}

pub fn prepare_view_depth_textures_for_volumetric_fog(
    mut view_targets: Query<&mut Camera3d>,
    fog_volumes: Query<&VolumetricFog>,
) {
    if fog_volumes.is_empty() {
        return;
    }

    for mut camera in view_targets.iter_mut() {
        camera.depth_texture_usages.0 |= TextureUsages::TEXTURE_BINDING.bits();
    }
}

fn get_far_planes(view_from_local: &Affine3A) -> [Vec4; 6] {
    let (mut far_planes, mut next_index) = ([Vec4::ZERO; 6], 0);

    for &local_normal in &[
        Vec3A::X,
        Vec3A::NEG_X,
        Vec3A::Y,
        Vec3A::NEG_Y,
        Vec3A::Z,
        Vec3A::NEG_Z,
    ] {
        let view_normal = view_from_local
            .transform_vector3a(local_normal)
            .normalize_or_zero();

        let view_position = view_from_local.transform_point3a(-local_normal * 0.5);
        let plane_coords = view_normal.extend(-view_normal.dot(view_position));

        if plane_coords.w <= 0.0 {

            continue;
        }

        far_planes[next_index] = plane_coords;
        next_index += 1;
    }

    far_planes
}

impl VolumetricFogBindGroupLayoutKey {

    fn bind_group_layout_description(&self) -> String {
        if self.is_empty() {
            return "volumetric lighting view bind group layout".to_owned();
        }

        format!(
            "volumetric lighting view bind group layout ({})",
            self.iter()
                .filter_map(|flag| {
                    if flag == VolumetricFogBindGroupLayoutKey::DENSITY_TEXTURE {
                        Some("density texture")
                    } else if flag == VolumetricFogBindGroupLayoutKey::MULTISAMPLED {
                        Some("multisampled")
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn camera_is_inside_fog_volume(local_from_view: &Affine3A) -> bool {
    local_from_view
        .translation
        .abs()
        .cmple(Vec3A::splat(0.5))
        .all()
}

fn calculate_fog_volume_clip_from_local_transforms(
    interior: bool,
    clip_from_view: &Mat4,
    view_from_local: &Affine3A,
) -> Mat4 {
    if !interior {
        return *clip_from_view * Mat4::from(*view_from_local);
    }

    let z_near = clip_from_view.w_axis[2];
    Mat4::from_cols(
        vec4(z_near, 0.0, 0.0, 0.0),
        vec4(0.0, z_near, 0.0, 0.0),
        vec4(0.0, 0.0, 0.0, 0.0),
        vec4(0.0, 0.0, z_near, z_near),
    )
}
