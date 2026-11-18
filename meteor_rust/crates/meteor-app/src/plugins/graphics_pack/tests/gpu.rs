use super::*;
use bevy::{
    app::PluginsState,
    camera::RenderTarget,
    render::{
        gpu_readback::{Readback, ReadbackComplete},
        render_resource::{PollType, TextureFormat, TextureUsages},
        renderer::RenderDevice,
        RenderPlugin,
    },
};
use std::sync::{Arc, Mutex};
#[cfg(feature = "original-assets")]
mod sky_benchmark;
fn capture(app: &mut App, pixels: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
    capture_size(app, pixels, 128, 128)
}
fn capture_size(app: &mut App, pixels: &Arc<Mutex<Vec<u8>>>, width: u32, height: u32) -> Vec<u8> {
    for _ in 0..24 {
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
            panic!("graphics pack pipeline: {e}");
        }
    }
    let data = pixels.lock().unwrap().clone();
    assert_eq!(data.len(), (width * height * 4) as usize);
    data
}
fn changed(a: &[u8], b: &[u8]) -> usize {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 2))
        .count()
}

#[test]
fn headless_cloud_sky_renders_and_respects_opaque_depth() {
    use crate::app_state::{AppState, LoadedStageInfo, Settings};
    use crate::plugins::graphics_pack::{config, sky};

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
    );
    sky::install(&mut app);
    app.add_plugins(crate::plugins::stage_material::GmdlMaterialPlugin)
        .add_systems(PostUpdate, runtime::update_frame)
        .insert_state(AppState::Playing)
        .insert_resource(Settings {
            environment_lighting: false,
            ..default()
        })
        .insert_resource(LoadedStageInfo {
            stage_id: "sn13".into(),
            ..default()
        });
    crate::plugins::frame_work::install(&mut app);
    let mut packs = starter();
    app.insert_resource(starter());
    let mut image = Image::new_target_texture(128, 128, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        RenderTarget::Image(target.clone().into()),
        Msaa::Off,
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Transform::IDENTITY.looking_at(Vec3::new(0., 0.65, -1.), Vec3::Y),
    ));
    let (configured, light, _) = sky::scene(&packs, "sn13").unwrap();
    let profile = config::Sky {
        density: 0.0,
        ..configured.clone()
    };
    let noise = app
        .world_mut()
        .resource_scope(|world, mut noise: Mut<sky::CloudNoise>| {
            noise.get(&mut world.resource_mut::<Assets<Image>>())
        });
    let cloud = app
        .world_mut()
        .resource_mut::<Assets<sky::CloudSkyMaterial>>()
        .add(sky::material(
            &profile,
            &light,
            noise.clone(),
            packs.wind("sn13"),
        ));
    let shell = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Sphere::new(1.).mesh().uv(24, 12));
    let dome = app
        .world_mut()
        .spawn((
            sky::CloudSky(None),
            bevy::camera::visibility::NoFrustumCulling,
            Mesh3d(shell),
            MeshMaterial3d(cloud.clone()),
            Transform::IDENTITY,
        ))
        .id();
    let solid = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(plain());
    let mut mesh = Cuboid::new(0.6, 0.6, 0.6).mesh().build();
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        vec![[0.45, 0.45, 0.45, 1.0]; mesh.count_vertices()],
    );
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
    app.world_mut().spawn((
        Mesh3d(mesh),
        MeshMaterial3d(solid),
        Transform::from_xyz(0., 1.3, -2.),
    ));
    let pixels = Arc::new(Mutex::new(Vec::new()));
    let output = pixels.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |e: On<ReadbackComplete>| {
            *output.lock().unwrap() = e.data.clone();
        });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline, "render plugins");
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    app.finish();
    app.cleanup();
    let clear_sky = capture(&mut app, &pixels);
    *app.world_mut()
        .resource_mut::<Assets<sky::CloudSkyMaterial>>()
        .get_mut(&cloud)
        .unwrap() = sky::material(&configured, &light, noise.clone(), packs.wind("sn13"));
    let cloudy_sky = capture(&mut app, &pixels);
    assert!(
        sky::cache_ready(&app),
        "cloud panorama must publish, not stay on direct rendering"
    );
    assert!(changed(&clear_sky, &cloudy_sky) > 1024);
    let middle = (64 * 128 + 64) * 4;
    assert_eq!(clear_sky[middle], clear_sky[middle + 2]);
    assert!((90..230).contains(&clear_sky[middle]));
    assert_eq!(
        &clear_sky[middle..middle + 4],
        &cloudy_sky[middle..middle + 4]
    );
    app.world_mut()
        .entity_mut(dome)
        .insert(sky::CloudSky(Some(sky::atmosphere_lighting(
            &configured,
            &light,
        ))));
    let lit = capture(&mut app, &pixels);
    assert!(
        lit[middle + 2] > lit[middle] + 8,
        "sky must tint the opaque material"
    );
    app.world_mut().resource_mut::<FrameUniforms>().seconds = 14.0;
    let moved = capture(&mut app, &pixels);
    assert!(
        changed(&lit, &moved) > 1024,
        "shared clock must move the clouds"
    );
    assert_eq!(lit[middle..middle + 4], moved[middle..middle + 4]);
    let mut calm = packs.wind("sn13").unwrap().clone();
    calm.strength = 0.0;
    *app.world_mut()
        .resource_mut::<Assets<sky::CloudSkyMaterial>>()
        .get_mut(&cloud)
        .unwrap() = sky::material(&configured, &light, noise.clone(), Some(&calm));
    let still = capture(&mut app, &pixels);
    app.world_mut().resource_mut::<FrameUniforms>().seconds = 28.0;
    assert_eq!(
        capture(&mut app, &pixels),
        still,
        "calm cloud cache must stay still"
    );

    packs.packs[0]
        .config
        .scenes
        .iter_mut()
        .find(|scene| scene.stage == "sn13")
        .unwrap()
        .sky
        .as_mut()
        .unwrap()
        .time = Some(config::SkyTime::Cycle {
        start_time: "00:00".into(),
        time_scale: 60.0,
    });
    let (midnight, light, cycle) = sky::scene(&packs, "sn13").unwrap();
    *app.world_mut()
        .resource_mut::<Assets<sky::CloudSkyMaterial>>()
        .get_mut(&cloud)
        .unwrap() = sky::material(&midnight, &light, noise, Some(&calm));
    app.world_mut().entity_mut(dome).insert((
        sky::CloudSky(Some(sky::atmosphere_lighting(&midnight, &light))),
        cycle.unwrap(),
    ));
    let night = capture(&mut app, &pixels);
    assert!(sky::cache_ready(&app));
    app.world_mut().resource_mut::<FrameUniforms>().seconds += 720.0;
    app.update();
    assert!(
        sky::cache_ready(&app),
        "a lighting change must retain the existing cloud panorama"
    );
    let noon = capture(&mut app, &pixels);
    assert!(changed(&night, &noon) > 1024);
    assert!(
        noon[middle + 2] > night[middle + 2] + 8,
        "the cycle must update scene fill"
    );
    app.world_mut().entity_mut(dome).remove::<sky::SkyCycle>();
    app.world_mut().resource_mut::<FrameUniforms>().seconds = 0.0;

    #[cfg(feature = "original-assets")]
    let dir = std::env::var_os("METEOR_SKY_CAPTURE_DIR").map(Into::into);
    #[cfg(feature = "original-assets")]
    let times = std::env::var_os("METEOR_SKY_TIME_CAPTURE").is_some();
    #[cfg(feature = "original-assets")]
    let benchmark = std::env::var_os("METEOR_SKY_BENCH").is_some();
    #[cfg(feature = "original-assets")]
    if dir.is_some() || benchmark {
        run_sn13(&mut app, dir, benchmark, times);
    }
}

#[cfg(feature = "original-assets")]
fn run_sn13(app: &mut App, dir: Option<std::path::PathBuf>, benchmark: bool, times: bool) {
    use crate::app_state::{LoadedStageAssets, LoadedStageInfo};
    use crate::plugins::{
        escort_assets::LoadedEscort, hazard_plugin::StageHazardRuntime,
        render_plugin::StageCollision, stage_collider::StageCollider,
        stage_plugin::StageScriptRuntime,
    };

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let src = root.join("sn13");
    let read = |name: &str| std::fs::read(src.join(name)).unwrap();
    let wp = crate::text_codec::decode_text(&std::fs::read(root.join("Level/sn13.wp")).unwrap());
    let center = meteor_scene::stage_loader::get_stage_center("sn13", Some(&wp));
    let mut init = app
        .world_mut()
        .query_filtered::<Entity, Or<(With<Camera3d>, With<Mesh3d>, With<Readback>)>>();
    for e in init.iter(app.world()).collect::<Vec<_>>() {
        app.world_mut().despawn(e);
    }
    app.insert_resource(LoadedStageInfo {
        stage_id: "sn13".into(),
        center,
        ..default()
    })
    .insert_resource(LoadedStageAssets {
        gmb: Some(read("sn13.gmb")),
        des: Some(read("sn13.des")),
        cob: Some(read("sn13.cob")),
        ..default()
    })
    .init_resource::<LoadedEscort>()
    .init_resource::<StageHazardRuntime>()
    .init_resource::<StageCollision>()
    .init_resource::<StageCollider>()
    .init_resource::<StageScriptRuntime>();

    let width = 960;
    let height = 576;
    let mut image = Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    let to_world = |p: Vec3| {
        let p = p - Vec3::from_array(center);
        Vec3::new(p.x, p.z, -p.y) * 0.12
    };
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera::default(),
            RenderTarget::Image(target.clone().into()),
            Msaa::Off,
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            Transform::IDENTITY,
        ))
        .id();
    if let Some(dir) = dir {
        capture_sn13(app, dir, camera, &target, to_world, width, height, times);
    }
    if benchmark {
        sky_benchmark::run(app, camera, &target, to_world);
    }
}

#[cfg(feature = "original-assets")]
fn load_sn13_variant(app: &mut App, enabled: bool) {
    use crate::app_state::Settings;
    use crate::plugins::render_plugin::{setup_stage_scene, PlayingSceneEntity};
    use bevy::ecs::system::RunSystemOnce;

    app.insert_resource(Settings {
        sky_enhance: enabled,
        ..default()
    });
    let old = app
        .world_mut()
        .query_filtered::<Entity, With<PlayingSceneEntity>>()
        .iter(app.world())
        .collect::<Vec<_>>();
    for entity in old {
        app.world_mut().despawn(entity);
    }
    app.world_mut().run_system_once(setup_stage_scene).unwrap();
    let activated = app
        .world_mut()
        .query::<&sky::CloudSky>()
        .iter(app.world())
        .any(|sky| sky.0.is_some());
    assert_eq!(activated, enabled);
}

#[cfg(feature = "original-assets")]
fn capture_sn13(
    app: &mut App,
    dir: std::path::PathBuf,
    camera: Entity,
    target: &Handle<Image>,
    to_world: impl Fn(Vec3) -> Vec3,
    width: u32,
    height: u32,
    times: bool,
) {
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension};

    let pixels = Arc::new(Mutex::new(Vec::new()));
    let output = pixels.clone();
    let readback = app
        .world_mut()
        .spawn(Readback::texture(target.clone()))
        .observe(move |e: On<ReadbackComplete>| {
            *output.lock().unwrap() = e.data.clone();
        })
        .id();
    std::fs::create_dir_all(&dir).unwrap();
    let save = |name: &str, data| {
        Image::new(
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD,
        )
        .try_into_dynamic()
        .unwrap()
        .save(dir.join(name))
        .unwrap();
    };
    if times {
        let eye = Vec3::new(-794., -178., 63.);
        let look = Vec3::new(-0.58, 0.81, 0.37);
        *app.world_mut().get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(to_world(eye)).looking_at(to_world(eye + look), Vec3::Y);
        let scenes = &app.world().resource::<GraphicsPacks>().packs[0]
            .config
            .scenes;
        let index = scenes
            .iter()
            .position(|scene| scene.stage == "sn13")
            .unwrap();
        let previous = scenes[index].sky.as_ref().unwrap().fixed_time.clone();
        for time in ["07:00", "12:00", "18:30", "23:30"] {
            app.world_mut().resource_mut::<GraphicsPacks>().packs[0]
                .config
                .scenes[index]
                .sky
                .as_mut()
                .unwrap()
                .fixed_time = Some(time.to_owned());
            app.world().resource::<GraphicsPacks>().packs[0]
                .config
                .validate()
                .unwrap();
            load_sn13_variant(app, true);
            let data = capture_size(app, &pixels, width, height);
            assert!(
                sky::cache_ready(app),
                "time preview must use the cloud cache"
            );
            let (profile, light, _) =
                sky::scene(app.world().resource::<GraphicsPacks>(), "sn13").unwrap();
            let expected = sky::atmosphere_lighting(&profile, &light);
            let graphics = app.world().resource::<FrameUniforms>().graphics;
            assert_eq!(graphics.sun, expected.sun);
            assert_eq!(graphics.sky.truncate(), expected.sky);
            if time == "23:30" {
                assert_eq!(graphics.sun.w, 0.0, "deep night has no direct sunlight");
                assert!(
                    graphics.sky.truncate().min_element() > 0.0,
                    "night keeps sky fill"
                );
            }
            save(&format!("sn13-{}.png", time.replace(':', "-")), data);
        }
        app.world_mut().resource_mut::<GraphicsPacks>().packs[0]
            .config
            .scenes[index]
            .sky
            .as_mut()
            .unwrap()
            .fixed_time = previous;
        app.world_mut().despawn(readback);
        return;
    }
    for enabled in [false, true] {
        load_sn13_variant(app, enabled);
        for (index, (eye, direction)) in [
            (Vec3::new(-794., -178., 63.), Vec3::new(-0.58, 0.81, 0.50)),
            (Vec3::new(24., -964., 60.), Vec3::new(-0.17, 0.98, 0.48)),
            (Vec3::new(-550., -145., 365.), Vec3::new(0.78, -0.63, 0.56)),
        ]
        .into_iter()
        .enumerate()
        {
            *app.world_mut().get_mut::<Transform>(camera).unwrap() =
                Transform::from_translation(to_world(eye))
                    .looking_at(to_world(eye + direction), Vec3::Y);
            let data = capture_size(app, &pixels, width, height);
            assert_eq!(
                app.world().resource::<FrameUniforms>().graphics.counts.w,
                f32::from(enabled)
            );
            save(
                &format!(
                    "sn13-{}-{index}.png",
                    if enabled { "enhanced" } else { "original" }
                ),
                data,
            );
        }
    }
    app.world_mut().despawn(readback);
}

#[test]
fn headless_pbr_environment_fog_contacts_and_disabled_restoration() {
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
    .insert_state(crate::app_state::AppState::Playing)
    .insert_resource(crate::app_state::Settings {
        scene_normal_maps: false,
        ..default()
    })
    .add_plugins(crate::plugins::stage_material::GmdlMaterialPlugin);
    crate::plugins::frame_work::install(&mut app);
    let mut image = Image::new_target_texture(128, 128, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        RenderTarget::Image(target.clone().into()),
        Msaa::Off,
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Transform::from_xyz(0., 6., 8.).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let mut mesh = Plane3d::default().mesh().size(12., 12.).build();
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        vec![[0.5f32, 0.5, 0.5, 1.]; mesh.count_vertices()],
    );
    mesh.generate_tangents().unwrap();
    let original = plain();
    let mut enhanced = original.clone();
    let mut packs = starter();

    let profile = packs.packs[0].config.materials.get_mut("stone").unwrap();
    profile.normal_strength = 4.;
    profile.metallic = 1.;
    SceneGraphics::new(Some(&packs), "sn06").configure(
        "floor",
        Some("sn064.jpg"),
        &mut enhanced,
        &mut mesh,
        true,
        false,
        &mut app.world_mut().resource_mut::<Assets<Image>>(),
    );
    let mh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
    let material = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(original.clone());
    app.world_mut().spawn((
        Mesh3d(mh),
        MeshMaterial3d(material.clone()),
        Transform::IDENTITY,
    ));
    let pixels = Arc::new(Mutex::new(vec![]));
    let out = pixels.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |e: On<ReadbackComplete>| *out.lock().unwrap() = e.data.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    app.finish();
    app.cleanup();
    let baseline = capture(&mut app, &pixels);
    *app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap() = enhanced.clone();
    let g = GraphicsFrame {
        sky: Vec4::new(0.4, 0.5, 0.7, 0.),
        ground: Vec4::new(0.1, 0.1, 0.1, 0.),
        sun: Vec3::new(0.4, 0.8, 0.3).normalize().extend(2.),
        sun_color: Vec4::ONE,
        environment: Vec4::new(1., 0., 1., 5.),
        ..default()
    };
    app.world_mut().resource_mut::<FrameUniforms>().graphics = g;
    let pbr = capture(&mut app, &pixels);
    assert!(
        changed(&baseline, &pbr) > 1000,
        "PBR must change rendered pixels"
    );
    app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap()
        .normal_texture = None;
    let no_normal = capture(&mut app, &pixels);
    assert!(
        changed(&pbr, &no_normal) > 100,
        "external normal map contributes to PBR shading: {} changed pixels",
        changed(&pbr, &no_normal)
    );
    *app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap() = enhanced.clone();
    app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap()
        .pbr_textures
        .x = 0.;
    let no_orm = capture(&mut app, &pixels);
    assert!(
        changed(&pbr, &no_orm) > 30,
        "external ORM contributes to PBR shading: {} changed pixels",
        changed(&pbr, &no_orm)
    );
    *app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap() = enhanced.clone();
    app.world_mut()
        .resource_mut::<FrameUniforms>()
        .graphics
        .environment
        .x = 0.;
    let no_environment = capture(&mut app, &pixels);
    assert!(
        changed(&pbr, &no_environment) > 1000,
        "external environment contributes light"
    );
    app.world_mut().resource_mut::<FrameUniforms>().graphics = g;
    {
        let mut f = app.world_mut().resource_mut::<FrameUniforms>();
        f.graphics.counts.y = 1.;
        f.graphics.contacts[0] = Vec4::new(0., 0., 0., 3.);
        f.graphics.contact_normals[0] = Vec4::new(0., 1., 0., 0.6);
    }
    let contacts = capture(&mut app, &pixels);
    assert!(
        changed(&pbr, &contacts) > 30,
        "contact shadow must be visible"
    );
    {
        let mut f = app.world_mut().resource_mut::<FrameUniforms>();
        f.graphics = g;
        f.graphics.counts.x = 1.;
        f.graphics.fog_bounds[0] = Vec4::new(0., 3., 3., 5.);
        f.graphics.fog_colors[0] = Vec4::new(0.1, 0.3, 0.8, 0.2);
    }
    let fog = capture(&mut app, &pixels);
    assert!(
        changed(&pbr, &fog) > 1000,
        "local fog must integrate along view rays"
    );
    *app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap() = original;
    app.world_mut().resource_mut::<FrameUniforms>().graphics = GraphicsFrame::default();
    let restored = capture(&mut app, &pixels);
    assert_eq!(
        baseline, restored,
        "disabled pack restores original shader output exactly"
    );
}

#[test]
fn headless_meshlet_matches_full_indexed_mesh_in_partial_view() {
    use bevy::{
        asset::RenderAssetUsages,
        camera::{visibility::ViewVisibility, ScalingMode},
        mesh::{Indices, PrimitiveTopology},
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
    .insert_state(crate::app_state::AppState::Playing)
    .init_resource::<crate::app_state::Settings>()
    .add_plugins(crate::plugins::stage_material::GmdlMaterialPlugin);
    crate::plugins::frame_work::install(&mut app);

    let file = config_root().join("graphics-packs/meshlet-demo/models/bronze-ornament.mlmesh");
    let model = meteor_asset::mlmesh::Model::decode(&std::fs::read(file).unwrap()).unwrap();
    let p = &model.primitives[0];
    let full_indices = p
        .meshlets
        .iter()
        .flat_map(|m| {
            m.indices
                .iter()
                .map(|&index| m.vertices[usize::from(index)])
        })
        .collect();
    let full = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, p.positions.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, p.normals.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, p.uvs.clone())
    .with_inserted_indices(Indices::U32(full_indices));

    let mut image = Image::new_target_texture(128, 128, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: 0.8,
                height: 1.0,
            },
            ..OrthographicProjection::default_3d()
        }),
        RenderTarget::Image(target.clone().into()),
        Msaa::Off,
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Transform::from_xyz(0.48, 0.43, 3.).looking_at(Vec3::new(0.48, 0.43, 0.), Vec3::Y),
    ));

    let material = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(plain());
    let full = app.world_mut().resource_mut::<Assets<Mesh>>().add(full);
    let source = app
        .world_mut()
        .spawn((
            Mesh3d(full),
            MeshMaterial3d(material.clone()),
            Transform::IDENTITY,
        ))
        .id();
    let pixels = Arc::new(Mutex::new(vec![]));
    let out = pixels.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |event: On<ReadbackComplete>| *out.lock().unwrap() = event.data.clone());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    app.finish();
    app.cleanup();
    let baseline = capture(&mut app, &pixels);
    assert!(
        baseline
            .chunks_exact(4)
            .filter(|c| c[0] > 3 || c[1] > 3 || c[2] > 3)
            .count()
            > 700
    );
    app.world_mut().despawn(source);

    let mut entities = Vec::new();
    for cluster in crate::plugins::graphics_pack::meshlets::split_meshes(p) {
        let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(cluster);
        entities.push(
            app.world_mut()
                .spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(material.clone()),
                    Transform::IDENTITY,
                ))
                .id(),
        );
    }
    let clustered = capture(&mut app, &pixels);
    assert!(
        changed(&baseline, &clustered) <= 4,
        "clustered mesh pixels differ"
    );
    let visible = entities
        .iter()
        .filter(|&&e| app.world().get::<ViewVisibility>(e).unwrap().get())
        .count();
    assert!(
        visible > 0 && visible < entities.len(),
        "expected partial frustum visibility: {visible}/{}",
        entities.len()
    );
    let gpu_culling = app
        .sub_app(bevy::render::RenderApp)
        .world()
        .resource::<bevy::render::batching::gpu_preprocessing::GpuPreprocessingSupport>()
        .is_culling_supported();
    println!(
        "visible meshlets: {visible}/{}; GPU culling available: {gpu_culling}",
        entities.len()
    );
}
