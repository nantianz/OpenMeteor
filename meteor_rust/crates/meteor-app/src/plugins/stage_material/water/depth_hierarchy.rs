use super::interaction::LiquidSurface;
use super::waves::WaterWaveMesh;
use crate::app_state::Settings;
use crate::plugins::{
    camera_plugin::GameplayCamera, render_plugin::SceneDeform, stage_material::GmdlMaterial,
};
use bevy::asset::{AssetEventSystems, RenderAssetUsages};
use bevy::camera::{primitives::Frustum, visibility::VisibilitySystems, CameraUpdateSystems};
use bevy::core_pipeline::{
    prepass::ViewPrepassTextures,
    schedule::{Core3d, Core3dSystems},
};
use bevy::prelude::*;
use bevy::render::{
    extract_resource::{ExtractResource, ExtractResourcePlugin},
    render_asset::RenderAssets,
    render_resource::{binding_types::*, *},
    renderer::{RenderAdapterInfo, RenderContext, RenderDevice, ViewQuery},
    sync_world::MainEntity,
    texture::GpuImage,
    view::Msaa,
    RenderApp, RenderStartup,
};

const MIPS: u32 = 4;
const TILE: u32 = 4;
const SHADER: Handle<Shader> = bevy::asset::uuid_handle!("dac82e8b-8849-43a9-a9e8-acaacf363152");

pub(in crate::plugins::stage_material) fn supported(device: &RenderDevice) -> bool {
    let limits = device.limits();
    super::config::depth_hierarchy_enabled()
        && limits.max_storage_textures_per_shader_stage >= MIPS
        && limits.max_sampled_textures_per_shader_stage >= 32
}

#[cfg(test)]
pub(crate) static TEST_DISPATCHES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[derive(Clone, Resource, Default, ExtractResource)]
struct Hierarchy(Option<DepthImage>);

#[derive(Clone)]
struct DepthImage {
    camera: Entity,
    source_size: UVec2,
    image: Handle<Image>,
    visible: bool,
}

pub(in crate::plugins::stage_material) fn install(app: &mut App) {
    if !super::config::depth_hierarchy_enabled() || app.get_sub_app(RenderApp).is_none() {
        return;
    }
    bevy::asset::load_internal_asset!(app, SHADER, "depth_hierarchy.wgsl", Shader::from_wgsl);
    app.init_resource::<Hierarchy>()
        .add_plugins(ExtractResourcePlugin::<Hierarchy>::default())
        .add_systems(
            PostUpdate,
            prepare_image
                .after(CameraUpdateSystems)
                .after(VisibilitySystems::UpdateFrusta)
                .after(super::screen_reflection::prepare_session)
                .before(AssetEventSystems),
        );
    app.sub_app_mut(RenderApp)
        .add_systems(RenderStartup, initialize)
        .add_systems(
            Core3d,
            build_hierarchy
                .after(Core3dSystems::Prepass)
                .before(Core3dSystems::MainPass),
        );
}

fn prepare_image(
    adapter: Option<Res<RenderAdapterInfo>>,
    device: Option<Res<RenderDevice>>,
    settings: Option<Res<Settings>>,
    cameras: Query<(Entity, &Camera, Has<GameplayCamera>, Option<&Frustum>), With<Camera3d>>,
    surfaces: Query<(
        &LiquidSurface,
        &MeshMaterial3d<GmdlMaterial>,
        Option<&WaterWaveMesh>,
        Has<SceneDeform>,
    )>,
    mut hierarchy: ResMut<Hierarchy>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
) {
    let camera = if adapter.is_some_and(|adapter| adapter.0.backend == wgpu::Backend::Vulkan)
        && device.is_some_and(|device| supported(&device))
        && settings.is_some_and(|settings| settings.water_enhance)
        && surfaces.iter().any(|(_, handle, _, _)| {
            materials
                .get(&handle.0)
                .is_some_and(|m| m.water_live_reflection)
        }) {
        let mut active = cameras.iter().filter(|(_, camera, _, _)| camera.is_active);
        let primary = active.next();
        primary
            .filter(|(_, camera, game, _)| {
                *game && camera.viewport.is_none() && active.next().is_none()
            })
            .and_then(|(entity, camera, _, frustum)| {
                camera
                    .physical_target_size()
                    .map(|size| (entity, size, frustum))
            })
    } else {
        None
    };
    if let Some((camera, source_size, frustum)) =
        camera.filter(|(_, size, _)| size.x > 0 && size.y > 0)
    {
        let visible = surfaces.iter().any(|(surface, handle, wave, deformed)| {
            materials
                .get(&handle.0)
                .is_some_and(|m| m.water_live_reflection)
                && surface.may_be_visible(frustum, wave, deformed)
        });
        if !hierarchy
            .0
            .as_ref()
            .is_some_and(|current| current.camera == camera && current.source_size == source_size)
        {
            let alignment = 1 << (MIPS - 1);
            let mut image = Image::new_uninit(
                Extent3d {
                    width: source_size.x.div_ceil(TILE).next_multiple_of(alignment),
                    height: source_size.y.div_ceil(TILE).next_multiple_of(alignment),
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                TextureFormat::R32Float,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            );
            image.texture_descriptor.mip_level_count = MIPS;
            image.texture_descriptor.usage =
                TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING;
            hierarchy.0 = Some(DepthImage {
                camera,
                source_size,
                image: images.add(image),
                visible,
            });
        } else if hierarchy
            .0
            .as_ref()
            .is_some_and(|current| current.visible != visible)
        {
            hierarchy.0.as_mut().unwrap().visible = visible;
        }
    } else if hierarchy.0.is_some() {
        hierarchy.0 = None;
    }
    for (surface, handle, _, _) in &surfaces {
        if !surface.is_clear_water() {
            continue;
        }
        let next = hierarchy.0.as_ref().map(|image| &image.image);
        if materials
            .get(&handle.0)
            .is_some_and(|material| material.water_depth_hierarchy.as_ref() != next)
        {
            materials.get_mut(&handle.0).unwrap().water_depth_hierarchy = next.cloned();
        }
    }
}

#[derive(Resource)]
struct Pipelines(Pass, Pass);

struct Pass {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
}

fn initialize(
    mut commands: Commands,
    adapter: Res<RenderAdapterInfo>,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
) {
    if adapter.0.backend != wgpu::Backend::Vulkan || !supported(&device) {
        return;
    }
    let init = |label: &'static str, input, shader_defs| {
        let mip = || texture_storage_2d(TextureFormat::R32Float, StorageTextureAccess::WriteOnly);
        let layout = BindGroupLayoutDescriptor::new(
            label,
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (input, mip(), mip(), mip(), mip()),
            ),
        );
        let pipeline = cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout.clone()],
            shader: SHADER,
            shader_defs,
            entry_point: Some("main".into()),
            ..default()
        });
        Pass { layout, pipeline }
    };
    commands.insert_resource(Pipelines(
        init("water depth tiles", texture_depth_2d(), vec![]),
        init(
            "water MSAA depth tiles",
            texture_depth_2d_multisampled(),
            vec!["MULTISAMPLED".into()],
        ),
    ));
}

fn build_hierarchy(
    view: ViewQuery<(&MainEntity, &ViewPrepassTextures, &Msaa)>,
    source: Res<Hierarchy>,
    pipelines: Option<Res<Pipelines>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    images: Res<RenderAssets<GpuImage>>,
    mut ctx: RenderContext,
) {
    let (entity, prepass, msaa) = view.into_inner();
    let (Some(source), Some(depth), Some(pipelines)) = (&source.0, prepass.depth_view(), pipelines)
    else {
        return;
    };
    if !source.visible
        || source.camera != **entity
        || source.source_size != UVec2::new(prepass.size.width, prepass.size.height)
    {
        return;
    }
    let Some(image) = images.get(&source.image) else {
        return;
    };
    let descriptor = if *msaa == Msaa::Off {
        &pipelines.0
    } else {
        &pipelines.1
    };
    let Some(pipeline) = cache.get_compute_pipeline(descriptor.pipeline) else {
        return;
    };
    let [mip0, mip1, mip2, mip3] = std::array::from_fn(|level| {
        image.texture.create_view(&TextureViewDescriptor {
            base_mip_level: level as u32,
            mip_level_count: Some(1),
            ..default()
        })
    });
    let bindings = device.create_bind_group(
        "water depth hierarchy",
        &cache.get_bind_group_layout(&descriptor.layout),
        &BindGroupEntries::sequential((depth, &mip0, &mip1, &mip2, &mip3)),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("water depth hierarchy"),
            ..default()
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &bindings, &[]);
    pass.dispatch_workgroups(image.texture.width() / 8, image.texture.height() / 8, 1);
    #[cfg(test)]
    TEST_DISPATCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}
