use super::*;
use crate::app_state::{AppState, LoadedStageAssets, LoadedStageInfo, Settings};
use crate::plugins::{
    camera_plugin::GameplayCamera,
    escort_assets::LoadedEscort,
    graphics_pack::{grass, loader, meshlets, sky},
    hazard_plugin::StageHazardRuntime,
    render_plugin::{setup_stage_scene, PlayingSceneEntity, StageCollision},
    stage_collider::StageCollider,
    stage_plugin::StageScriptRuntime,
};
use bevy::{
    asset::RenderAssetUsages,
    ecs::system::RunSystemOnce,
    render::render_resource::{Extent3d, TextureDimension},
};
use std::path::Path;

#[test]
fn headless_sn03_ravine_enhancement_preview() {
    let Some(output) = std::env::var_os("METEOR_SN03_RAVINE_PREVIEW") else {
        return;
    };
    let output = Path::new(&output);
    std::fs::create_dir_all(output).unwrap();
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let base = project.join("meteor_rust/config");
    let origin = project.join("origin_res");
    let read = |name: &str| std::fs::read(origin.join(name)).unwrap();
    let stage = "sn03";
    let wp = crate::text_codec::decode_text(&read("Level/sn03.wp"));
    let center = meteor_scene::stage_loader::get_stage_center(stage, Some(&wp));
    let origin_offset =
        Vec3::new(center[0], center[2], -center[1]) * meteor_core::constants::MODEL_SCALE;
    let gmb = read("sn03/sn03.gmb");
    let des = read("sn03/sn03.des");
    let cob = read("sn03/sn03.cob");
    let stock = meteor_asset::gmdl::decode_gmdl(&gmb).unwrap();
    let baseline = std::env::var_os("METEOR_SN03_RAVINE_BASELINE").is_some();
    let index = if baseline {
        br#"{"schema_version":1,"enabled":true,"packs":["graphics-packs/starter/pack.json"]}"#
            .to_vec()
    } else {
        std::fs::read(base.join("graphics-enhancement-sn03.json")).unwrap()
    };
    let mut packs = loader::load_catalog_with_reader(&index, |name| {
        std::fs::read(base.join(name)).map_err(|error| error.to_string())
    })
    .unwrap()
    .with_test_loose_root(base);
    assert!(packs.packs.is_empty());
    packs.prepare_stage(stage);
    let mut expected = Vec::new();
    if !baseline {
        let pack = &packs.packs[1];
        let scene = &pack.config.scenes[0];
        assert_eq!(scene.stage, stage);
        let clusters = |model: &str| {
            let definition = &pack.config.meshlet_models[model];
            let bytes = packs.read_meshlet_model(pack, &definition.file).unwrap();
            meteor_asset::mlmesh::Model::decode(&bytes)
                .unwrap()
                .primitives
                .iter()
                .map(|part| part.meshlets.len())
                .sum::<usize>()
        };
        for rule in &scene.meshlet_replacements {
            let object = stock
                .objects
                .iter()
                .find(|object| object.name == rule.object)
                .unwrap();
            let anchor = meshlets::source_anchor(object, center).unwrap();
            expected.push((
                anchor + Vec3::from_array(rule.placement.offset),
                clusters(&rule.model),
            ));
        }
        for prop in &scene.meshlet_props {
            let count = clusters(&prop.model);
            for placement in &prop.placements {
                expected.push((Vec3::from_array(placement.offset), count));
            }
        }
        assert!(expected.len() >= 5);
    }
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
    .add_plugins(crate::plugins::stage_material::GmdlMaterialPlugin)
    .insert_state(AppState::Playing)
    .insert_resource(Settings {
        sky_enhance: true,
        grass_enhance: true,
        hd_scene: true,
        volumetric_fog: true,
        ..default()
    })
    .insert_resource(LoadedStageInfo {
        stage_id: stage.into(),
        center,
        ..default()
    })
    .insert_resource(LoadedStageAssets {
        gmb: Some(gmb),
        des: Some(des),
        cob: Some(cob),
        ..default()
    })
    .init_resource::<LoadedEscort>()
    .init_resource::<StageHazardRuntime>()
    .init_resource::<StageCollision>()
    .init_resource::<StageCollider>()
    .init_resource::<StageScriptRuntime>()
    .add_systems(PostUpdate, runtime::update_frame);
    sky::install(&mut app);
    grass::install(&mut app);
    crate::plugins::frame_work::install(&mut app);
    let (width, height) = (960, 576);
    let mut target = Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None);
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera::default(),
            GameplayCamera,
            Msaa::Sample4,
            RenderTarget::Image(target.clone().into()),
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            Transform::IDENTITY,
        ))
        .id();
    let pixels = Arc::new(Mutex::new(Vec::new()));
    let output_pixels = pixels.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |event: On<ReadbackComplete>| {
            *output_pixels.lock().unwrap() = event.data.clone();
        });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    let stock_collider = if baseline {
        None
    } else {
        app.world_mut().insert_resource(GraphicsPacks::default());
        app.world_mut().run_system_once(setup_stage_scene).unwrap();
        let world = app.world_mut();
        let scene = world
            .query_filtered::<Entity, With<PlayingSceneEntity>>()
            .iter(world)
            .collect::<Vec<_>>();
        for entity in scene {
            if world.get_entity(entity).is_ok() {
                world.despawn(entity);
            }
        }
        Some(world.resource_mut::<StageCollider>().mesh.take().unwrap())
    };
    app.world_mut().insert_resource(packs);
    app.world_mut().run_system_once(setup_stage_scene).unwrap();
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / 60.0),
    ));
    app.world_mut().resource_mut::<Time<Virtual>>().pause();

    if let Some(stock) = stock_collider {
        let collider = app
            .world()
            .resource::<StageCollider>()
            .mesh
            .as_ref()
            .unwrap();
        assert!(
            collider.positions == stock.positions,
            "enhancement moved collision vertices"
        );
        assert!(
            collider.indices == stock.indices,
            "enhancement changed collision indices"
        );
        assert!(collider.wall_tris == stock.wall_tris);
        assert!(collider.floor_tris == stock.floor_tris);
        assert!(collider.steep_tris == stock.steep_tris);
        assert!(collider.ceiling_tris == stock.ceiling_tris);
    }
    let mut meshes = app
        .world_mut()
        .query_filtered::<&Transform, (With<PlayingSceneEntity>, With<Mesh3d>)>();
    for (position, clusters) in expected {
        let rendered = meshes
            .iter(app.world())
            .filter(|transform| transform.translation.distance_squared(position) < 0.001)
            .count();
        assert_eq!(rendered, clusters, "missing scan clusters at {position:?}");
    }
    for (name, eye, look) in [
        ("ravine", [-2.65, 7., -24.], [-4.9, 15., -85.]),
        ("approach", [-2.5, 7.2, -112.], [18., 15., -238.]),
        ("gate", [-3.5, 7., -153.], [17., 15., -250.]),
        ("overlook", [-17., 55., -30.], [-17., 64., -176.]),
        ("return", [-3., 7., -72.], [-5., 21., 25.]),
    ] {
        *app.world_mut().get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(Vec3::from_array(eye) - origin_offset)
                .looking_at(Vec3::from_array(look) - origin_offset, Vec3::Y);
        for _ in 0..84 {
            app.update();
        }
        let data = capture_size(&mut app, &pixels, width, height);
        if !baseline {
            assert_eq!(
                app.world().resource::<FrameUniforms>().graphics.counts,
                Vec4::new(2., 0., 1., 1.),
                "the ravine enables both fog layers, grass and atmosphere lighting",
            );
        }
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
        .save(output.join(format!("sn03-{name}.png")))
        .unwrap();
    }
}
