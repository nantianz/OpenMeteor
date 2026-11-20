use super::*;
use crate::{
    app_state::{AppState, LoadedStageAssets, LoadedStageInfo, Settings},
    components::fighter::{Fighter, Health, MotionState},
    plugins::{
        camera_plugin::GameplayCamera,
        escort_assets::LoadedEscort,
        graphics_pack::snow::{self, SnowField, SnowMaterial, SnowSurfaces},
        hazard_plugin::StageHazardRuntime,
        render_plugin::{setup_stage_scene, StageCollision},
        stage_collider::StageCollider,
        stage_material::GmdlMaterialPlugin,
        stage_plugin::StageScriptRuntime,
    },
};
use bevy::{
    asset::RenderAssetUsages,
    ecs::system::RunSystemOnce,
    render::render_resource::{Extent3d, TextureDimension},
    time::TimeUpdateStrategy,
};
use meteor_core::MovementMode;
use meteor_physics::collision::MeshCollider;
use std::time::{Duration, Instant};

#[test]
fn sn04_snow_stamps_real_snow_and_renders_only_on_contact() {
    let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let src = origin.join("sn04");
    if !src.join("sn04.gmb").is_file() {
        return;
    }
    let read = |name| std::fs::read(src.join(name)).unwrap();
    let wp = crate::text_codec::decode_text(&std::fs::read(origin.join("Level/sn04.wp")).unwrap());
    let center = meteor_scene::stage_loader::get_stage_center("sn04", Some(&wp));
    let to_world = |x: f32, y: f32| Vec3::new(x - center[0], -center[2], center[1] - y) * 0.12;
    let mut packs = load_with_reader(
        br#"{"schema_version":1,"enabled":true,"packs":["graphics-packs/sn04-interactive-snow/pack.json"]}"#,
        |name| std::fs::read(config_root().join(name)).map_err(|e| e.to_string()),
    )
    .unwrap();
    assert!(SnowSurfaces::for_stage(&packs, "sn06").is_none());
    let export = std::env::var_os("METEOR_SN04_CAPTURE_DIR").map(std::path::PathBuf::from);
    let benchmark = std::env::var_os("METEOR_SN04_SNOW_BENCH").is_some();
    let short_recovery = export.is_none() && !benchmark;
    if short_recovery {
        packs.packs[0].config.scenes[0]
            .snow
            .as_mut()
            .unwrap()
            .recovery_seconds = 4.0;
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
    .add_plugins(GmdlMaterialPlugin)
    .insert_state(AppState::Playing)
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
        0.04,
    )))
    .insert_resource(Settings {
        environment_lighting: false,
        ..default()
    })
    .insert_resource(LoadedStageInfo {
        stage_id: "sn04".into(),
        center,
        ..default()
    })
    .insert_resource(LoadedStageAssets {
        gmb: Some(read("sn04.gmb")),
        des: Some(read("sn04.des")),
        cob: Some(read("sn04.cob")),
        ..default()
    })
    .insert_resource(packs)
    .init_resource::<LoadedEscort>()
    .init_resource::<StageHazardRuntime>()
    .init_resource::<StageCollision>()
    .init_resource::<StageCollider>()
    .init_resource::<StageScriptRuntime>();
    snow::install(&mut app);

    let (width, height) = if benchmark {
        (1920, 1080)
    } else if export.is_some() {
        (1024, 704)
    } else {
        (256, 256)
    };
    let mut image = Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    let start = to_world(415.0, 110.0);
    let look = start + Vec3::new(0.4, 0.23, 0.25);
    let camera = app
        .world_mut()
        .spawn((
            GameplayCamera,
            Camera3d::default(),
            Msaa::Off,
            Projection::Perspective(PerspectiveProjection {
                fov: 52.0_f32.to_radians(),
                ..default()
            }),
            RenderTarget::Image(target.clone().into()),
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            Transform::from_translation(look + Vec3::new(-0.55, 1.65, -4.25))
                .looking_at(look, Vec3::Y),
        ))
        .id();
    let pixels = Arc::new(Mutex::new(Vec::new()));
    let output = pixels.clone();
    let readback = app
        .world_mut()
        .spawn(Readback::texture(target))
        .observe(move |e: On<ReadbackComplete>| {
            *output.lock().unwrap() = e.data.clone();
        })
        .id();
    let deadline = Instant::now() + Duration::from_secs(40);
    while app.plugins_state() != PluginsState::Ready {
        assert!(Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    app.world_mut().run_system_once(setup_stage_scene).unwrap();

    let (effect, powder_mesh) = app
        .world_mut()
        .query_filtered::<(Entity, &Mesh3d), With<SnowField>>()
        .single(app.world())
        .map(|(id, mesh)| (id, mesh.0.clone()))
        .unwrap();
    let tiles = app
        .world_mut()
        .query_filtered::<Entity, With<MeshMaterial3d<SnowMaterial>>>()
        .iter(app.world())
        .collect::<Vec<_>>();
    assert!(tiles.len() > 100 && tiles.len() <= 1024);
    let at = |x, y, h| to_world(x, y) + Vec3::Y * h;
    let supported = app
        .world()
        .resource::<StageCollider>()
        .mesh
        .as_ref()
        .unwrap()
        .sweep_foot_segment(start.x, start.y + 0.4, start.z, start.y - 0.4)
        .unwrap();
    assert!(supported.y.abs() < 0.1);
    let actor = fighter(&mut app, at(389.0, 20.0, 0.0));
    let powder = |app: &App| {
        app.world()
            .resource::<Assets<Mesh>>()
            .get(&powder_mesh)
            .unwrap()
            .count_vertices()
    };
    let disturbed = |app: &App| {
        app.world()
            .get::<SnowField>(effect)
            .unwrap()
            .resident_count()
    };
    let step = |app: &mut App, x, y, height, mode| {
        move_fighter(app, actor, at(x, y, height), mode);
        app.update();
    };
    let capture_case = |app: &mut App, name: &str| {
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        let rgba = capture_size(app, &pixels, width, height);
        app.world_mut().resource_mut::<Time<Virtual>>().unpause();
        if let Some(dir) = &export {
            std::fs::create_dir_all(dir).unwrap();
            Image::new(
                Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                rgba.clone(),
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::MAIN_WORLD,
            )
            .try_into_dynamic()
            .unwrap()
            .save(dir.join(name))
            .unwrap();
        }
        rgba
    };

    draw_tiles(&mut app, &tiles, false);
    let original = capture_case(&mut app, "01-original.png");
    draw_tiles(&mut app, &tiles, true);
    let untouched = capture_case(&mut app, "02-thick-snow.png");
    assert!(
        changed(&original, &untouched) > (width * height / 12) as usize,
        "the untouched snow is a raised surface"
    );

    step(&mut app, 389.0, 20.0, 0.0, MovementMode::Walk);
    step(&mut app, 396.0, 20.0, 0.0, MovementMode::Walk);
    assert_eq!(disturbed(&app), 0, "exposed stone");
    step(&mut app, 391.0, 110.0, 0.0, MovementMode::Walk);
    assert_eq!(disturbed(&app), 0, "teleport across the map");
    for i in 1..=14 {
        step(
            &mut app,
            391.0 + i as f32 * 1.42,
            110.0,
            0.0,
            MovementMode::Walk,
        );
    }
    for i in 1..=13 {
        step(
            &mut app,
            410.88 + i as f32 * 2.8,
            110.0,
            0.0,
            MovementMode::Run,
        );
    }
    assert!(disturbed(&app) > 0);
    *app.world_mut().get_mut::<Visibility>(effect).unwrap() = Visibility::Hidden;
    let tracked = capture_case(&mut app, "03-walk-and-run.png");
    assert!(
        changed(&untouched, &tracked) > 80,
        "ground geometry changes with powder hidden"
    );
    *app.world_mut().get_mut::<Visibility>(effect).unwrap() = Visibility::Inherited;

    recover(&mut app, camera, actor, readback, short_recovery);
    assert_eq!(disturbed(&app), 0);
    step(&mut app, 413.0, 110.0, 1.8, MovementMode::Airborne);
    step(&mut app, 414.0, 110.0, 0.82, MovementMode::Airborne);
    assert_eq!(disturbed(&app), 0, "no imprint before touchdown");
    step(&mut app, 415.0, 110.0, 0.0, MovementMode::Idle);
    for _ in 0..4 {
        app.update();
    }
    assert!(
        disturbed(&app) > 0 && powder(&app) >= 72,
        "landing crater and displaced powder"
    );
    let landed = capture_case(&mut app, "04-landing.png");
    assert!(changed(&untouched, &landed) > 40);

    recover(&mut app, camera, actor, readback, short_recovery);
    step(&mut app, 392.0, 110.0, 0.0, MovementMode::Dodge);
    for i in 1..=15 {
        step(
            &mut app,
            392.0 + i as f32 * 3.0,
            110.0,
            0.0,
            MovementMode::Dodge,
        );
    }
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Paused);
    let slide = capture_case(&mut app, "05-roll.png");
    assert!(changed(&untouched, &slide) > 80);
    let clock = app.world().get::<SnowField>(effect).unwrap().clock();
    let paused = capture_size(&mut app, &pixels, width, height);
    assert_eq!(clock, app.world().get::<SnowField>(effect).unwrap().clock());
    let changed_while_paused = changed(&slide, &paused);
    assert!(
        changed_while_paused <= (width * height / 2000) as usize,
        "{changed_while_paused} pixels changed while paused",
    );
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Playing);
    app.world_mut().despawn(readback);
    if benchmark {
        benchmark_sn04(&mut app, &tiles, effect, actor, camera);
    }
}

fn fighter(app: &mut App, position: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 0,
                weapon_slots: [0; 2],
                weapon_slot: 0,
                weapon_slot_variants: [0; 2],
                weapon_variant: 0,
            },
            Health::default(),
            Transform::from_translation(position),
            MotionState {
                movement_mode: MovementMode::Idle,
                facing: std::f32::consts::FRAC_PI_2,
                velocity: Vec3::ZERO,
            },
        ))
        .id()
}

fn move_fighter(app: &mut App, actor: Entity, position: Vec3, mode: MovementMode) {
    app.world_mut()
        .get_mut::<Transform>(actor)
        .unwrap()
        .translation = position;
    app.world_mut()
        .get_mut::<MotionState>(actor)
        .unwrap()
        .movement_mode = mode;
}

fn draw_tiles(app: &mut App, tiles: &[Entity], enabled: bool) {
    let visibility = if enabled {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for &tile in tiles {
        *app.world_mut().get_mut::<Visibility>(tile).unwrap() = visibility;
    }
}

fn recover(app: &mut App, camera: Entity, actor: Entity, readback: Entity, short_recovery: bool) {
    app.world_mut().get_mut::<Camera>(camera).unwrap().is_active = false;
    move_fighter(app, actor, Vec3::splat(10_000.0), MovementMode::Idle);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        100,
    )));
    let readback_config = app
        .world_mut()
        .entity_mut(readback)
        .take::<Readback>()
        .unwrap();
    for _ in 0..if short_recovery { 45 } else { 365 } {
        app.update();
    }
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        40,
    )));
    app.world_mut().get_mut::<Camera>(camera).unwrap().is_active = true;
    app.world_mut().entity_mut(readback).insert(readback_config);
}

fn benchmark_sn04(app: &mut App, tiles: &[Entity], effect: Entity, actor: Entity, camera: Entity) {
    let adapter = app
        .world()
        .resource::<bevy::render::renderer::RenderAdapterInfo>();
    eprintln!("SN04 1080p, {:?} / {}", adapter.0.backend, adapter.0.name);
    let world = app.world();
    let eye = world.get::<Transform>(camera).unwrap().translation;
    let snow = world.get::<SnowField>(effect).unwrap();
    let ground = world.resource::<StageCollider>().mesh.as_ref().unwrap();
    let mut anchors = tiles
        .iter()
        .filter_map(|&tile| {
            let center = world.get::<Transform>(tile)?.translation + Vec3::new(4.0, 0.0, 4.0);
            let distance = center.distance_squared(eye);
            if distance > 38.0 * 38.0
                || ![-1.6, -0.8, 0.0, 0.8, 1.6]
                    .into_iter()
                    .all(|dx| snow.has_deep_snow(center.xz() + Vec2::X * dx))
            {
                return None;
            }
            let floor =
                ground.sweep_foot_segment(center.x, center.y + 0.15, center.z, center.y - 0.25)?;
            (floor.ny >= 0.6).then_some((distance, center.with_y(floor.y)))
        })
        .collect::<Vec<_>>();
    anchors.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    assert!(anchors.len() >= 12, "select 12 distinct real snow tiles");
    let anchors = anchors
        .into_iter()
        .take(12)
        .map(|(_, p)| p)
        .collect::<Vec<_>>();
    let actors = std::iter::once(actor)
        .chain(anchors[1..].iter().map(|&position| fighter(app, position)))
        .collect::<Vec<_>>();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )));
    let mut samples = [Vec::<f64>::new(), Vec::new()];
    let mut peak_patches = 0;
    for enabled in [false, true, true, false] {
        draw_tiles(app, tiles, enabled);
        *app.world_mut().get_mut::<Visibility>(effect).unwrap() = if enabled {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(if enabled {
                AppState::Playing
            } else {
                AppState::Paused
            });
        for frame in 0..92 {
            for (i, &actor) in actors.iter().enumerate() {
                let travel = (frame as f32 * 0.18 + i as f32 * 0.23).sin() * 1.48;
                move_fighter(
                    app,
                    actor,
                    anchors[i] + Vec3::X * travel,
                    if i % 5 == 0 {
                        MovementMode::Dodge
                    } else {
                        MovementMode::Run
                    },
                );
            }
            let start = Instant::now();
            app.update();
            app.world()
                .resource::<RenderDevice>()
                .poll(PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(5)),
                })
                .unwrap();
            peak_patches = peak_patches.max(
                app.world()
                    .get::<SnowField>(effect)
                    .unwrap()
                    .resident_count(),
            );
            if frame >= 28 {
                samples[usize::from(enabled)].push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    for (i, result) in samples.iter_mut().enumerate() {
        result.sort_unstable_by(f64::total_cmp);
        eprintln!(
            "SN04 snow={}: frame end-to-end p50 {:.3} ms p95 {:.3} ms ({} samples)",
            i != 0,
            result[result.len() / 2],
            result[(result.len() * 95).div_ceil(100) - 1],
            result.len()
        );
    }
    eprintln!("SN04 12 scripted fighters; peak disturbed blocks {peak_patches}");
}
