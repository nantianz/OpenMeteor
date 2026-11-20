use super::*;
use crate::app_state::{AppState, LoadedStageAssets, LoadedStageInfo, Settings};
use crate::plugins::{
    camera_plugin::GameplayCamera,
    escort_assets::LoadedEscort,
    graphics_pack::{grass, loader, meshlets, sky},
    hazard_plugin::StageHazardRuntime,
    render_plugin::{setup_stage_scene, PlayingSceneEntity, StageCollision},
    stage_collider::StageCollider,
    stage_material::GmdlMaterial,
    stage_plugin::StageScriptRuntime,
};
use bevy::{
    asset::RenderAssetUsages,
    ecs::system::RunSystemOnce,
    render::render_resource::{Extent3d, TextureDimension},
};
use std::path::Path;

#[test]
fn headless_sn06_scan_enhancement_preview() {
    let Some(output_dir) = std::env::var_os("METEOR_SN06_SCAN_PREVIEW") else {
        return;
    };
    let output_dir = Path::new(&output_dir);
    std::fs::create_dir_all(output_dir).unwrap();
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let base = project.join("meteor_rust/config");
    let baseline = std::env::var_os("METEOR_SN06_SCAN_BASELINE").is_some();
    let capture_only = baseline || std::env::var_os("METEOR_SN06_SCAN_CAPTURE_ONLY").is_some();
    let root = project.join("origin_res");
    let read = |name: &str| std::fs::read(root.join(name)).unwrap();
    let wp = crate::text_codec::decode_text(&read("Level/sn06.wp"));
    let center = meteor_scene::stage_loader::get_stage_center("sn06", Some(&wp));
    let index = std::env::var_os("METEOR_SN06_SCAN_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| base.join("graphics-enhancement-sn06.json"));
    let mut packs = loader::load_catalog_with_reader(&std::fs::read(index).unwrap(), |name| {
        std::fs::read(base.join(name)).map_err(|e| e.to_string())
    })
    .unwrap()
    .with_test_loose_root(base);
    assert!(packs.packs.is_empty());
    packs.prepare_stage("sn03");
    assert!(
        packs
            .packs
            .iter()
            .all(|pack| pack.config.id != "sn06-moss-valley"),
        "sn03 must not load the sn06 pack"
    );
    packs.prepare_stage("sn06");
    if let Some(time) = std::env::var_os("METEOR_SN06_SKY_TIME") {
        let time = time.into_string().unwrap();
        for pack in &mut packs.packs {
            for scene in &mut pack.config.scenes {
                if let Some(sky) = &mut scene.sky {
                    sky.fixed_time = None;
                    sky.time = Some(crate::plugins::graphics_pack::config::SkyTime::Fixed {
                        time: time.clone(),
                    });
                    if std::env::var_os("METEOR_SN06_SKY_CLEAR").is_some() {
                        sky.coverage = 0.0;
                        sky.density = 0.0;
                    }
                }
            }
            pack.config.validate().unwrap();
        }
    }
    if baseline {
        packs = GraphicsPacks::default();
    }
    let expected_cliffs = if baseline {
        Vec::new()
    } else {
        let pack = &packs.packs[1];
        let scene = &pack.config.scenes[0];
        let original = meteor_asset::gmdl::decode_gmdl(&read("sn06/sn06.gmb")).unwrap();
        assert_eq!(scene.meshlet_replacements.len(), 3);
        scene
            .meshlet_replacements
            .iter()
            .map(|rule| {
                let object = original
                    .objects
                    .iter()
                    .find(|o| o.name == rule.object)
                    .unwrap();
                let anchor = meshlets::source_anchor(object, center).unwrap();
                let position = anchor + Vec3::from_array(rule.placement.offset);
                let definition = &pack.config.meshlet_models[&rule.model];
                let bytes = packs.read_meshlet_model(pack, &definition.file).unwrap();
                let model = meteor_asset::mlmesh::Model::decode(&bytes).unwrap();
                let clusters = model
                    .primitives
                    .iter()
                    .map(|p| p.meshlets.len())
                    .sum::<usize>();
                (rule.object.clone(), position, clusters)
            })
            .collect::<Vec<_>>()
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
        stage_id: "sn06".into(),
        center,
        ..default()
    })
    .insert_resource(LoadedStageAssets {
        gmb: Some(read("sn06/sn06.gmb")),
        des: Some(read("sn06/sn06.des")),
        cob: Some(read("sn06/sn06.cob")),
        ..default()
    })
    .init_resource::<LoadedEscort>()
    .init_resource::<StageHazardRuntime>()
    .init_resource::<StageCollision>()
    .init_resource::<StageCollider>()
    .init_resource::<StageScriptRuntime>()
    .insert_resource(packs)
    .add_systems(PostUpdate, runtime::update_frame);
    sky::install(&mut app);
    grass::install(&mut app);
    crate::plugins::frame_work::install(&mut app);
    let (width, height) = (960, 576);
    let mut target = Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None);
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    let eye = Vec3::new(-80.0, 90.0, 24.0) - Vec3::from_array(center);
    let eye = Vec3::new(eye.x, eye.z, -eye.y) * meteor_core::constants::MODEL_SCALE;
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera::default(),
            GameplayCamera,
            Msaa::Sample4,
            RenderTarget::Image(target.clone().into()),
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            Transform::from_translation(eye).looking_at(eye + Vec3::new(0.75, 0.10, 0.66), Vec3::Y),
        ))
        .id();
    if std::env::var_os("METEOR_SN06_HDR_PREVIEW").is_some() {
        let exposure = {
            let graphics = app.world().resource::<GraphicsPacks>();
            let mut exposure = 1.0f32;
            let mut ev = 0.0;
            let night = sky::scene(graphics, "sn06")
                .map(|(profile, light, _)| sky::atmosphere_lighting(&profile, &light).night_factor)
                .unwrap_or(0.0);
            for (_, scene) in graphics.scenes("sn06") {
                if let Some(environment) = &scene.environment {
                    exposure = environment.exposure;
                }
                if let Some(pipeline) = &scene.color_pipeline {
                    ev = pipeline.exposure_ev + pipeline.night_exposure_ev * night;
                }
            }
            ev + exposure.log2()
        };
        let mut grading = bevy::render::view::ColorGrading::default();
        grading.global.exposure = exposure;
        app.world_mut().entity_mut(camera).insert((
            bevy::camera::Hdr,
            bevy::core_pipeline::tonemapping::Tonemapping::AcesFitted,
            grading,
            crate::plugins::graphics_pack::color_pipeline::test_marker(),
        ));
    }
    let pixels = Arc::new(Mutex::new(Vec::new()));
    let output = pixels.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |e: On<ReadbackComplete>| {
            *output.lock().unwrap() = e.data.clone();
        });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    app.world_mut().run_system_once(setup_stage_scene).unwrap();
    if std::env::var_os("METEOR_SN06_EXPECT_WATERFALL_LAYERS").is_some() {
        use crate::plugins::graphics_pack::waterfalls::WaterfallLayer;
        let layers: Vec<_> = app
            .world_mut()
            .query::<(&WaterfallLayer, &Transform)>()
            .iter(app.world())
            .map(|(layer, transform)| (*layer, transform.translation))
            .collect();
        assert_eq!(
            layers
                .iter()
                .filter(|(layer, _)| *layer == WaterfallLayer::Curtain)
                .count(),
            12
        );
        assert_eq!(
            layers
                .iter()
                .filter(|(layer, _)| *layer == WaterfallLayer::Mist)
                .count(),
            6
        );
        assert!(layers
            .iter()
            .filter(|(layer, _)| *layer == WaterfallLayer::Mist)
            .all(|(_, p)| p.y >= 0.));
    }
    app.world_mut().resource_mut::<Time<Virtual>>().pause();
    let expected = if baseline {
        0
    } else {
        app.world().resource::<GraphicsPacks>().packs[1]
            .config
            .scenes[0]
            .meshlet_props[0]
            .placements
            .len()
    };
    let rendered_props = app
        .world_mut()
        .query_filtered::<&Transform, (With<PlayingSceneEntity>, With<Mesh3d>)>()
        .iter(app.world())
        .filter(|transform| transform.scale.x >= 54.)
        .count();
    if !baseline {
        assert_eq!(
            rendered_props,
            expected * 83,
            "every mountain prop uses the converted scan"
        );
    }
    let mut transforms = app
        .world_mut()
        .query_filtered::<&Transform, (With<PlayingSceneEntity>, With<Mesh3d>)>();
    for (object, position, clusters) in expected_cliffs {
        let rendered = transforms
            .iter(app.world())
            .filter(|transform| transform.translation.distance_squared(position) < 0.001)
            .count();
        assert_eq!(rendered, clusters, "rendered scan clusters for {object}");
    }
    let mut grass_meshes = app
        .world_mut()
        .query_filtered::<(Entity, &Mesh3d), With<MeshMaterial3d<grass::GrassMaterial>>>();
    let mut first_allocation = None;
    let mut meadow = Vec::new();
    for (view, direction) in [
        ("east", Vec3::new(0.82, 0.07, 0.57)),
        ("west", Vec3::new(-0.75, 0.13, 0.66)),
        ("north", Vec3::new(-0.26, 0.22, -0.97)),
        ("falls", Vec3::new(0.73, 0.17, -0.68)),
        ("skyline", Vec3::new(-0.26, 0.61, -0.97)),
        ("celestial", Vec3::new(-0.30, 0.93, 0.20)),
        ("meadow", Vec3::new(0.82, -0.14, 0.57)),
    ] {
        *app.world_mut().get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(eye).looking_at(eye + direction, Vec3::Y);
        let data = capture_size(&mut app, &pixels, width, height);
        if view == "falls" && std::env::var_os("METEOR_SN06_EXPECT_WATERFALL_LAYERS").is_some() {
            app.world_mut().resource_mut::<FrameUniforms>().seconds = 1.5;
            let animated = capture_size(&mut app, &pixels, width, height);
            let mut moving = 0usize;
            for y in 140..440 {
                for x in 450..550 {
                    let index = ((y * width + x) * 4) as usize;
                    moving += usize::from(
                        (0..3).any(|c| data[index + c].abs_diff(animated[index + c]) > 2),
                    );
                }
            }
            assert!(
                moving > 50,
                "waterfall must animate, only {moving} pixels changed"
            );
            Image::new(
                Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                animated,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::MAIN_WORLD,
            )
            .try_into_dynamic()
            .unwrap()
            .save(output_dir.join("sn06-falls-animated.png"))
            .unwrap();
            app.world_mut().resource_mut::<FrameUniforms>().seconds = 0.;
        }
        if capture_only {
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
            .save(output_dir.join(format!("sn06-{view}.png")))
            .unwrap();
            continue;
        }
        let mut allocation: Vec<_> = grass_meshes
            .iter(app.world())
            .map(|(tile, mesh)| (tile, mesh.0.id()))
            .collect();
        allocation.sort_unstable_by_key(|(tile, _)| *tile);
        assert!(!allocation.is_empty());
        if let Some(first) = &first_allocation {
            assert_eq!(
                &allocation, first,
                "yaw and pitch must not change the dense field's LODs"
            );
        } else {
            first_allocation = Some(allocation);
        }
        assert_eq!(
            app.world().resource::<FrameUniforms>().graphics.counts,
            Vec4::new(2., 0., 1., 1.)
        );
        if view == "meadow" {
            meadow.clone_from(&data);
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
        .save(output_dir.join(format!("sn06-{view}.png")))
        .unwrap();
    }
    if capture_only {
        return;
    }
    let first_row = height * 3 / 4;
    let foreground = (first_row * width * 4) as usize;
    let original_lighting: Vec<_> = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .iter_mut()
        .filter(|(_, material)| material.pbr_lighting != Vec3::ONE)
        .map(|(id, material)| (id, std::mem::replace(&mut material.pbr_lighting, Vec3::ONE)))
        .collect();
    assert!(!original_lighting.is_empty());
    let neutral = capture_size(&mut app, &pixels, width, height);
    assert!(changed(&meadow[..foreground], &neutral[..foreground]) > 12_000);
    assert!(
        meadow[foreground..] == neutral[foreground..],
        "the near grass must keep its original lighting"
    );
    for (id, lighting) in original_lighting {
        app.world_mut()
            .resource_mut::<Assets<GmdlMaterial>>()
            .get_mut(id)
            .unwrap()
            .pbr_lighting = lighting;
    }

    for (tile, _) in first_allocation.unwrap() {
        app.world_mut().despawn(tile);
    }
    let ground = capture_size(&mut app, &pixels, width, height);
    let first_column = width / 8;
    let last_column = width - first_column;
    let mut different = 0;
    for y in first_row..height {
        for x in first_column..last_column {
            let index = ((y * width + x) * 4) as usize;
            different += usize::from((index..index + 3).any(|i| meadow[i].abs_diff(ground[i]) > 3));
        }
    }
    let coverage = different as f32 / ((height - first_row) * (last_column - first_column)) as f32;
    let percent = coverage * 100.0;
    eprintln!("sn06 blades change {percent:.1}% of the foreground pixels");
    assert!(coverage >= 0.9, "foreground grass coverage: {percent:.1}%");
}
