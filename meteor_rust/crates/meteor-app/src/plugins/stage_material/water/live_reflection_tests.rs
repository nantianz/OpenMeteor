use super::render_tests::capture;
use super::*;
use crate::app_state::{AppState, Settings};
use crate::plugins::stage_material::{GmdlMaterialParams, GmdlMaterialPlugin};
use bevy::app::PluginsState;
use bevy::camera::RenderTarget;
use bevy::core_pipeline::prepass::DepthPrepass;
use bevy::pbr::ScreenSpaceTransmission;
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use bevy::render::{
    gpu_readback::{Readback, ReadbackComplete},
    RenderPlugin,
};
use std::sync::{Arc, Mutex};

fn plain_image(width: u32, height: u32, color: [u8; 4]) -> Image {
    Image::new(
        bevy::render::render_resource::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        color.repeat((width * height) as usize),
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
}

fn colored_mesh(mut mesh: Mesh, color: [f32; 4]) -> Mesh {
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![color; mesh.count_vertices()]);
    mesh
}

#[test]
fn on_screen_wall_and_actor_contribute_to_clear_water() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .set(RenderPlugin {
                synchronous_pipeline_compilation: true,
                ..default()
            })
            .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>()
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::audio::AudioPlugin>(),
    )
    .insert_state(AppState::Playing)
    .insert_resource(Settings {
        water_enhance: true,
        ..default()
    })
    .add_plugins(GmdlMaterialPlugin);
    crate::plugins::frame_work::install(&mut app);
    let mut target = Image::new_target_texture(640, 360, TextureFormat::Rgba8UnormSrgb, None);
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.2, 0.3, 0.4)),
            ..default()
        },
        RenderTarget::Image(target.clone().into()),
        ScreenSpaceTransmission {
            steps: 1,
            ..default()
        },
        DepthPrepass,
        Msaa::Sample4,
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Transform::from_xyz(0.0, 2.3, 6.0).looking_at(Vec3::new(0.0, 0.0, -1.0), Vec3::Y),
    ));
    let image_handles = {
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        (
            images.add(plain_image(198, 132, [0, 0, 0, 0])),
            images.add(plain_image(1, 1, [100, 255, 255, 255])),
            images.add(plain_image(1, 1, [128, 128, 255, 255])),
        )
    };
    let mut water = GmdlMaterial::new_scene(
        GmdlMaterialParams {
            opacity: 0.75,
            alpha_cutoff: 0.0,
            mode: GmdlMode::Blend.as_u32(),
            has_texture: 0,
            uv_offset: Vec2::ZERO,
        },
        None,
        false,
        GmdlMode::Blend,
    );
    water.water = Vec4::new(1.0, 12.0, 1.0, 1.5);
    water.water_bounds = Vec4::new(-5.0, -5.0, 10.0, 10.0);
    water.water_reflection = Some(image_handles.0);
    water.water_depth = Some(image_handles.1);
    water.water_normal = Some(image_handles.2);
    water.water_live_reflection = true;
    water.water_optics = super::super::WaterOptics {
        absorption_ior: Vec4::new(0.30, 0.12, 0.06, 1.333),
        scattering_g: Vec4::new(0.05, 0.12, 0.17, 0.35),
        control: Vec4::new(0.16, 1.0, 12.0, 1.0),
    };
    let water = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(water);
    let plane = colored_mesh(
        Plane3d::default().mesh().size(12.0, 12.0).build(),
        [0.3, 0.5, 0.5, 1.0],
    );
    let plane = app.world_mut().resource_mut::<Assets<Mesh>>().add(plane);
    app.world_mut().spawn((
        Mesh3d(plane),
        MeshMaterial3d(water.clone()),
        Transform::IDENTITY,
    ));
    let opaque =
        app.world_mut()
            .resource_mut::<Assets<GmdlMaterial>>()
            .add(GmdlMaterial::new_scene(
                GmdlMaterialParams {
                    opacity: 1.0,
                    alpha_cutoff: 0.0,
                    mode: GmdlMode::Opaque.as_u32(),
                    has_texture: 0,
                    uv_offset: Vec2::ZERO,
                },
                None,
                false,
                GmdlMode::Opaque,
            ));
    let wall = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(colored_mesh(
            Mesh::from(Cuboid::new(5.0, 2.0, 0.3)),
            [0.7, 0.7, 0.7, 1.0],
        ));
    let wall = app
        .world_mut()
        .spawn((
            Mesh3d(wall),
            MeshMaterial3d(opaque.clone()),
            Transform::from_xyz(0.0, 1.0, -2.0),
            Visibility::Hidden,
        ))
        .id();
    let actor = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(colored_mesh(
            Mesh::from(Cuboid::new(0.45, 1.55, 0.45)),
            [0.95, 0.1, 0.1, 1.0],
        ));
    let actor = app
        .world_mut()
        .spawn((
            Mesh3d(actor),
            MeshMaterial3d(opaque),
            Transform::from_xyz(0.0, 0.78, -1.0),
            Visibility::Hidden,
        ))
        .id();
    let pixels = Arc::new(Mutex::new(Vec::new()));
    let output = pixels.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |event: On<ReadbackComplete>| {
            *output.lock().unwrap() = event.data.clone();
        });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    app.finish();
    app.cleanup();
    let empty = capture(&mut app, &pixels);
    *app.world_mut().get_mut::<Visibility>(wall).unwrap() = Visibility::Visible;
    let wall_frame = capture(&mut app, &pixels);
    *app.world_mut().get_mut::<Visibility>(actor).unwrap() = Visibility::Visible;
    let actor_frame = capture(&mut app, &pixels);
    app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&water)
        .unwrap()
        .water_live_reflection = false;
    let actor_without_reflection = capture(&mut app, &pixels);
    *app.world_mut().get_mut::<Visibility>(actor).unwrap() = Visibility::Hidden;
    let wall_without_reflection = capture(&mut app, &pixels);
    if let Ok(dir) = std::env::var("METEOR_WATER_CAPTURE_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        for (name, data) in [
            ("empty", &empty),
            ("wall", &wall_frame),
            ("actor", &actor_frame),
            ("wall-static", &wall_without_reflection),
            ("actor-static", &actor_without_reflection),
        ] {
            Image::new(
                bevy::render::render_resource::Extent3d {
                    width: 640,
                    height: 360,
                    depth_or_array_layers: 1,
                },
                bevy::render::render_resource::TextureDimension::D2,
                data.clone(),
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::MAIN_WORLD,
            )
            .try_into_dynamic()
            .unwrap()
            .save(std::path::Path::new(&dir).join(format!("{name}.png")))
            .unwrap();
        }
    }
    let reflected_changes = |a: &[u8], b: &[u8], start: usize, end: usize| {
        (start..end)
            .flat_map(|y| (0..640).map(move |x| (y * 640 + x) * 4))
            .filter(|&pixel| (0..3).any(|i| a[pixel + i].abs_diff(b[pixel + i]) > 4))
            .count()
    };
    assert!(
        reflected_changes(&wall_frame, &wall_without_reflection, 180, 250) > 160,
        "wall must change only when the current-frame reflection is enabled"
    );
    assert!(
        reflected_changes(&actor_frame, &actor_without_reflection, 248, 280) > 80,
        "actor must reflect below its directly visible silhouette"
    );

    *app.world_mut().get_mut::<Visibility>(wall).unwrap() = Visibility::Hidden;
    let opaque = app
        .world()
        .get::<MeshMaterial3d<GmdlMaterial>>(wall)
        .unwrap()
        .clone();
    let bed = colored_mesh(
        Mesh::from(Cuboid::new(1.4, 0.12, 3.0)),
        [0.9, 0.4, 0.12, 1.0],
    );
    let bed = app.world_mut().resource_mut::<Assets<Mesh>>().add(bed);
    app.world_mut()
        .spawn((Mesh3d(bed), opaque, Transform::from_xyz(0.4, -1.0, -0.5)));
    {
        let mut materials = app.world_mut().resource_mut::<Assets<GmdlMaterial>>();
        let mut material = materials.get_mut(&water).unwrap();
        material.water_live_reflection = true;
        material.water_optics = super::super::WaterOptics {
            absorption_ior: Vec4::new(0.30, 0.12, 0.06, 1.333),
            scattering_g: Vec4::new(0.05, 0.12, 0.17, 0.35),
            control: Vec4::new(0.16, 1.0, 12.0, 1.0),
        };
    }
    let undistorted_bed = capture(&mut app, &pixels);
    app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&water)
        .unwrap()
        .water_screen_refraction = true;
    let refracted_bed = capture(&mut app, &pixels);
    assert!(
        reflected_changes(&undistorted_bed, &refracted_bed, 190, 310) > 100,
        "enhanced refraction must change how the submerged bed appears"
    );

    let refraction = |app: &mut App, enabled| {
        let mut materials = app.world_mut().resource_mut::<Assets<GmdlMaterial>>();
        let mut material = materials.get_mut(&water).unwrap();
        material.water_screen_refraction = enabled;
        material.water_live_reflection = false;
    };
    let mut stand = app.world_mut().get_mut::<Transform>(actor).unwrap();
    stand.translation = Vec3::new(0.24, 0.83, 1.4);
    stand.rotation = Quat::from_rotation_y(0.45);
    refraction(&mut app, false);
    let flat = capture(&mut app, &pixels);
    *app.world_mut().get_mut::<Visibility>(actor).unwrap() = Visibility::Visible;
    let actor_flat = capture(&mut app, &pixels);
    refraction(&mut app, true);
    let actor_bent = capture(&mut app, &pixels);
    *app.world_mut().get_mut::<Visibility>(actor).unwrap() = Visibility::Hidden;
    let bent = capture(&mut app, &pixels);
    let actors_alone = flat.chunks_exact(4).zip(actor_flat.chunks_exact(4));
    let ghosts = bent
        .chunks_exact(4)
        .zip(actor_bent.chunks_exact(4))
        .zip(actors_alone)
        .filter(|((bent, actor_bent), (flat, actor_flat))| {
            flat == actor_flat && (0..3).any(|i| bent[i].abs_diff(actor_bent[i]) > 15)
        })
        .count();
    eprintln!("refraction changed {ghosts} pixels outside the directly visible actor silhouette");
    assert!(
        ghosts <= 10,
        "the water must not reproduce a foreground actor silhouette"
    );
}
