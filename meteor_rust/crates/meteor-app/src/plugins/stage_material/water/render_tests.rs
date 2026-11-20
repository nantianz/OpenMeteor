use super::*;
use crate::app_state::{AppState, Settings};
use crate::plugins::stage_material::{FrameUniforms, GmdlMaterialParams, GmdlMaterialPlugin};
use bevy::app::PluginsState;
use bevy::camera::RenderTarget;
use bevy::render::render_resource::{PollType, TextureUsages};
use bevy::render::{
    gpu_readback::{Readback, ReadbackComplete},
    renderer::RenderDevice,
    RenderPlugin,
};
use std::sync::{Arc, Mutex};

pub(super) fn capture(app: &mut App, pixels: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
    for _ in 0..20 {
        app.update();
        app.world()
            .resource::<RenderDevice>()
            .poll(PollType::Poll)
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for pipeline in app
        .sub_app(bevy::render::RenderApp)
        .world()
        .resource::<bevy::render::render_resource::PipelineCache>()
        .pipelines()
    {
        if let bevy::render::render_resource::CachedPipelineState::Err(e) = &pipeline.state {
            panic!("water pipeline: {e}");
        }
    }
    let result = pixels.lock().unwrap().clone();
    assert!(!result.is_empty(), "GPU readback must contain pixels");
    result
}

fn changed(a: &[u8], b: &[u8]) -> usize {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 1))
        .count()
}

#[derive(Resource, Default)]
struct TestWaterUpload(super::dynamics::WaterUploads);

#[test]
fn headless_water_changes_animates_and_restores_while_scrolling_smoke_is_unchanged() {
    let capture_size = if std::env::var_os("METEOR_SPLASH_CAPTURE_DIR").is_some() {
        512
    } else {
        128
    };
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
        water_enhance: false,
        scene_normal_maps: false,
        ..default()
    })
    .add_plugins(GmdlMaterialPlugin);
    crate::plugins::frame_work::install(&mut app);
    let mut image = Image::new_target_texture(
        capture_size,
        capture_size,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera {
                clear_color: ClearColorConfig::Custom(Color::srgb(0.16, 0.20, 0.23)),
                ..default()
            },
            RenderTarget::Image(target.clone().into()),
            Msaa::Off,
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            Transform::from_xyz(0.0, 3.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id();
    let mut material = GmdlMaterial::new_scene(
        GmdlMaterialParams {
            opacity: 0.6,
            alpha_cutoff: 0.0,
            mode: 1,
            has_texture: 0,
            uv_offset: Vec2::ZERO,
        },
        None,
        false,
        GmdlMode::Blend,
    );
    material.uv_scroll = Vec4::new(0.05, 0.03, 0.0, 0.0);
    let mut cache = WaterCache::default();
    cache.configure(
        &mut material,
        "sn13",
        "Plane02",
        "sn13w01.jpg",
        [0.0; 3],
        &mut app.world_mut().resource_mut::<Assets<Image>>(),
        None,
    );

    material.water_bounds.x = -material.water_bounds.z * 0.5;
    material.water_bounds.y = -material.water_bounds.w * 0.5;
    let material = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(material);
    let mut mesh = Plane3d::default().mesh().size(16.0, 16.0).build();
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        vec![[0.25_f32, 0.32, 0.34, 1.0]; mesh.count_vertices()],
    );
    mesh.generate_tangents().unwrap();
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
    app.world_mut().spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material.clone()),
        Transform::IDENTITY,
    ));
    let pixels = Arc::new(Mutex::new(Vec::new()));
    let output = pixels.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |event: On<ReadbackComplete>| {
            *output.lock().unwrap() = event.data.clone();
        });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    app.finish();
    app.cleanup();
    for eye in [Vec3::new(0.0, 3.0, 9.0), Vec3::new(0.0, 9.0, 3.0)] {
        *app.world_mut().get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(eye).looking_at(Vec3::ZERO, Vec3::Y);
        let baseline = capture(&mut app, &pixels);
        app.world_mut().resource_mut::<Settings>().water_enhance = true;
        let water = capture(&mut app, &pixels);
        let delta = changed(&baseline, &water);
        assert!(delta > 100, "water must change at both angles: {delta}");
        app.world_mut().resource_mut::<FrameUniforms>().seconds = 2.0;
        let animated = capture(&mut app, &pixels);
        assert!(changed(&water, &animated) > 10, "water must animate");
        app.world_mut().resource_mut::<FrameUniforms>().seconds = 0.0;
        app.world_mut().resource_mut::<Settings>().water_enhance = false;
        assert_eq!(
            capture(&mut app, &pixels),
            baseline,
            "off restores authored pixels"
        );
        eprintln!("water eye={eye:?}: {delta} changed pixels");
    }

    if capture_size > 128 {
        *app.world_mut().get_mut::<Transform>(camera).unwrap() =
            Transform::from_xyz(0.0, 3.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y);
    }
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::ZERO,
    ));
    app.world_mut().resource_mut::<Settings>().water_enhance = true;
    let calm = capture(&mut app, &pixels);
    app.world_mut()
        .resource_mut::<interaction::LiquidInteractions>()
        .emit(Vec3::ZERO, 1.0, 1.2);
    let disturbed = capture(&mut app, &pixels);
    assert!(
        changed(&calm, &disturbed) > 5,
        "contact ripple must change visible water pixels"
    );

    use super::dynamics::{Upload, WaterUploads};
    let mut field = super::physics::WaveField::new(&[
        [
            Vec2::splat(-10.0),
            Vec2::new(10.0, -10.0),
            Vec2::splat(10.0),
        ],
        [
            Vec2::splat(-10.0),
            Vec2::splat(10.0),
            Vec2::new(-10.0, 10.0),
        ],
    ]);
    let (width, height, bounds) = field.layout();
    let image = Image::new(
        bevy::render::render_resource::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        field.pixels(),
        TextureFormat::Rgba8Unorm,
        bevy::asset::RenderAssetUsages::MAIN_WORLD | bevy::asset::RenderAssetUsages::RENDER_WORLD,
    );
    let image = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    {
        let mut materials = app.world_mut().resource_mut::<Assets<GmdlMaterial>>();
        let mut water = materials.get_mut(&material).unwrap();
        water.water_dynamics = Some(image.clone());
        water.water_dynamics_bounds = bounds;
    }
    app.init_resource::<TestWaterUpload>().add_systems(
        Update,
        (|input: Res<TestWaterUpload>, mut output: ResMut<WaterUploads>| {
            if input.0.epoch > 0 {
                output.clone_from(&input.0);
            }
        })
        .after(super::waves::update),
    );
    let field_calm = capture(&mut app, &pixels);
    field.impulse_at_radius(Vec2::ZERO, 1.6, Vec2::X, 0.8);
    for _ in 0..24 {
        field.advance(1.0 / 120.0);
    }
    app.world_mut().resource_mut::<TestWaterUpload>().0 = WaterUploads {
        epoch: 10_001,
        textures: vec![Upload {
            image: image.clone(),
            width,
            height,
            pixels: Arc::new(field.pixels()),
        }],
    };
    let field_impact = capture(&mut app, &pixels);
    assert!(
        changed(&field_calm, &field_impact) > 15,
        "simulated slope and foam must reach the GPU"
    );
    field.clear();
    app.world_mut().resource_mut::<TestWaterUpload>().0 = WaterUploads {
        epoch: 10_002,
        textures: vec![Upload {
            image,
            width,
            height,
            pixels: Arc::new(field.pixels()),
        }],
    };
    assert_eq!(
        capture(&mut app, &pixels),
        field_calm,
        "settled physics restores the same calm image"
    );
    app.world_mut().resource_mut::<Settings>().water_enhance = false;

    app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap()
        .water = Vec4::ZERO;
    let smoke = capture(&mut app, &pixels);
    app.world_mut().resource_mut::<Settings>().water_enhance = true;
    assert_eq!(
        capture(&mut app, &pixels),
        smoke,
        "scrolling smoke must be pixel identical"
    );

    let no_spray = capture(&mut app, &pixels);
    {
        let mut contacts = app
            .world_mut()
            .resource_mut::<interaction::LiquidInteractions>();
        contacts.emit_with_splash(Vec3::ZERO, 1.0, 1.2, 1.0);
        contacts.set_latest_age(0.2);
    }
    let spray = capture(&mut app, &pixels);
    assert!(
        changed(&no_spray, &spray) > 3,
        "landing spray must be visible"
    );

    for _ in 0..2 {
        app.world_mut()
            .resource_mut::<interaction::LiquidInteractions>()
            .set_latest_age(1.8);
        assert_eq!(
            capture(&mut app, &pixels),
            no_spray,
            "expired spray must disappear without changing the background"
        );
        {
            let mut contacts = app
                .world_mut()
                .resource_mut::<interaction::LiquidInteractions>();
            contacts.emit_with_splash(Vec3::ZERO, 1.0, 1.2, 1.0);
            contacts.set_latest_age(0.2);
        }
        assert!(
            changed(&no_spray, &capture(&mut app, &pixels)) > 3,
            "the pooled spray must draw again after expiry"
        );
    }
    if let Ok(dir) = std::env::var("METEOR_SPLASH_CAPTURE_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        let mut screenshots = vec![
            ("plain".to_string(), no_spray),
            ("spray".to_string(), spray),
            ("physical-calm".to_string(), field_calm),
            ("physical-impact".to_string(), field_impact),
        ];
        for (name, velocity, radius, depth) in [
            ("slow", Vec3::X * 2.0, 0.245, 0.25),
            ("run", Vec3::X * 24.0, 0.245, 0.25),
            ("fall", Vec3::new(3.0, -40.0, 0.0), 0.455, 0.45),
            ("sweep", Vec3::new(35.0, -10.0, 0.0), 0.25, 0.20),
        ] {
            for age in [0.03, 0.08, 0.16] {
                let mut contacts = interaction::LiquidInteractions::default();
                contacts.disturb(
                    Entity::from_bits(991),
                    Vec3::ZERO,
                    1.0,
                    super::spray::Impact {
                        velocity,
                        radius,
                        depth,
                    },
                );
                if contacts.splashes().next().is_some() {
                    contacts.set_latest_age(age);
                }
                app.insert_resource(contacts);
                let frame = capture(&mut app, &pixels);
                screenshots.push((format!("{name}-{age:.2}"), frame));
            }
        }
        for (name, bytes) in screenshots {
            Image::new(
                bevy::render::render_resource::Extent3d {
                    width: capture_size,
                    height: capture_size,
                    depth_or_array_layers: 1,
                },
                bevy::render::render_resource::TextureDimension::D2,
                bytes,
                TextureFormat::Rgba8UnormSrgb,
                bevy::asset::RenderAssetUsages::MAIN_WORLD,
            )
            .try_into_dynamic()
            .unwrap()
            .save(std::path::Path::new(&dir).join(format!("{name}.png")))
            .unwrap();
        }
    }
}
