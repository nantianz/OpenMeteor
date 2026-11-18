use super::*;
use crate::components::fighter::{
    EntryBlend, EntryBlendKind, Fighter, MotionState, PoseRuntime, SmState, VisualRootPin,
};
use crate::plugins::character_plugin::{
    drive_fighter_animation, load_character_asset_for_test, reveal_ready_fighter_skins,
    spawn_fighter_model_for_test, CharacterAssetsSet, CommonAmb,
};
use bevy::{
    app::PluginsState,
    camera::RenderTarget,
    render::{
        gpu_readback::{Readback, ReadbackComplete},
        render_resource::{
            CachedPipelineState, PipelineCache, PollType, TextureFormat, TextureUsages,
        },
        renderer::RenderDevice,
        RenderApp, RenderPlugin,
    },
    time::TimeUpdateStrategy,
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const WIDTH: u32 = 768;
const HEIGHT: u32 = 640;

fn capture(app: &mut App, pixels: &Mutex<Vec<u8>>) -> Vec<u8> {
    for _ in 0..16 {
        app.update();
        app.world()
            .resource::<RenderDevice>()
            .poll(PollType::Poll)
            .unwrap();
        std::thread::sleep(Duration::from_millis(4));
    }
    for pipeline in app
        .sub_app(RenderApp)
        .world()
        .resource::<PipelineCache>()
        .pipelines()
    {
        if let CachedPipelineState::Err(error) = &pipeline.state {
            panic!("outfit shader: {error}");
        }
    }
    let output = pixels.lock().unwrap().clone();
    assert_eq!(output.len(), (WIDTH * HEIGHT * 4) as usize);
    output
}

fn fixture(character_id: u8) -> (App, Entity, Entity, Arc<Mutex<Vec<u8>>>) {
    let character_key = format!("p{character_id}");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let original = workspace.join("origin_res");
    let config = std::env::var_os("METEOR_OUTFIT_CONFIG_DIR").map_or_else(
        || workspace.join("meteor_rust/config"),
        std::path::PathBuf::from,
    );
    let pack = crate::plugins::graphics_pack::load_with_reader(
        br#"{"schema_version":1,"enabled":true,"packs":["graphics-packs/starter/pack.json"]}"#,
        |file| std::fs::read(config.join(file)).map_err(|error| error.to_string()),
    )
    .expect("starter pack");
    let character =
        load_character_asset_for_test(&[original.clone()], &character_key).expect("stock model");
    let common = meteor_asset::amb::parse_amb(
        &std::fs::read(original.join("Common/character.amb")).expect("shared motions"),
    )
    .unwrap();

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
    app.add_plugins(crate::plugins::stage_material::GmdlMaterialPlugin)
        .insert_state(AppState::Playing)
        .insert_resource(Settings {
            new_character_models: false,
            ..default()
        })
        .insert_resource(crate::plugins::launcher_ui::UiFont(Handle::default()))
        .insert_resource(LoadedStageInfo {
            stage_id: "outfit-preview".into(),
            ..default()
        })
        .insert_resource(pack)
        .insert_resource(CharacterAssetsSet([(character_key, character)].into()))
        .insert_resource(CommonAmb(common))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1. / 60.,
        )))
        .add_systems(
            Update,
            (drive_fighter_animation, reveal_ready_fighter_skins).chain(),
        );
    crate::plugins::graphics_pack::install(&mut app);
    install(&mut app);

    let world = app.world_mut();
    let fighter = world
        .spawn((
            Fighter {
                character_id,
                player_slot: 0,
                weapon: 0,
                weapon_slots: [0, 0],
                weapon_slot: 0,
                weapon_slot_variants: [0; 2],
                weapon_variant: 0,
            },
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.,
            },
            PoseRuntime::idle(),
            EntryBlend::default(),
            SmState::default(),
            VisualRootPin::default(),
            Transform::IDENTITY,
            Visibility::Inherited,
        ))
        .id();
    spawn_fighter_model_for_test(world, fighter);
    let mut image = Image::new_target_texture(WIDTH, HEIGHT, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = world.resource_mut::<Assets<Image>>().add(image);
    let camera = world
        .spawn((
            Camera3d::default(),
            Camera {
                clear_color: ClearColorConfig::Custom(Color::srgb(0.08, 0.105, 0.13)),
                ..default()
            },
            RenderTarget::Image(target.clone().into()),
            Msaa::Off,
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            Transform::IDENTITY,
            GameplayCamera,
        ))
        .id();
    let pixels = Arc::new(Mutex::new(Vec::new()));
    let output = pixels.clone();
    world
        .spawn(Readback::texture(target))
        .observe(move |e: On<ReadbackComplete>| {
            *output.lock().unwrap() = e.data.clone();
        });
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(Instant::now() < deadline, "renderer initialization timeout");
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(Duration::from_millis(2));
    }
    app.finish();
    app.cleanup();

    (app, fighter, camera, pixels)
}

fn preview_output() -> impl Fn(&str, &[u8]) {
    let output_dir = std::env::var_os("METEOR_OUTFIT_PREVIEW_DIR").map(std::path::PathBuf::from);
    move |name: &str, data: &[u8]| {
        if let Some(dir) = &output_dir {
            std::fs::create_dir_all(dir).unwrap();
            image::save_buffer(
                dir.join(format!("{name}.png")),
                data,
                WIDTH,
                HEIGHT,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }
}

#[test]
#[ignore = "requires native GPU and original P0 assets"]
fn headless_p0_outfit_on_original_poses() {
    let (mut app, fighter, camera, pixels) = fixture(0);
    let save = preview_output();
    let mut visibility = app.world_mut().query::<(&Cape, &Visibility, &Mesh3d)>();
    for (name, pose_id, fraction, eye) in [
        ("idle-front", 0, 0.18, Vec3::new(-4.0, 3.10, 7.8)),
        ("idle-back", 0, 0.18, Vec3::new(3.6, 3.10, -8.0)),
        ("run", 144, 0.44, Vec3::new(5.2, 3.0, 7.2)),
        ("dodge", 164, 0.57, Vec3::new(-5.5, 3.1, 7.4)),
    ] {
        let world = app.world_mut();
        let mut pose = PoseRuntime::idle();
        world.resource::<CharacterAssetsSet>().0["p0"].apply_pose(&mut pose, pose_id, true);
        pose.frame = pose.start as f32 + (pose.end - pose.start) as f32 * fraction;
        world.entity_mut(fighter).insert(pose);
        *world.get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(eye).looking_at(Vec3::new(0., 2.15, 0.), Vec3::Y);
        world.resource_mut::<Settings>().character_motion = true;
        let dressed = capture(&mut app, &pixels);
        save(name, &dressed);
        let world = app.world();
        let (cape, visible, mesh) = visibility.single(world).unwrap();
        let triangle_count = cape.detail.as_ref().unwrap().triangles.len();
        assert_eq!(*visible, Visibility::Inherited, "{name}");
        assert!(triangle_count > cape.topology.triangles.len() * 5);
        let mesh = world.resource::<Assets<Mesh>>().get(&mesh.0).unwrap();
        assert_eq!(mesh.indices().unwrap().len(), triangle_count * 3, "{name}");
        app.world_mut().resource_mut::<Settings>().character_motion = false;
        let plain = capture(&mut app, &pixels);
        if name == "idle-front" {
            save("stock-front", &plain);
        }
        let changed = dressed
            .chunks_exact(4)
            .zip(plain.chunks_exact(4))
            .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 8))
            .count();
        assert!(
            changed > 1800,
            "{name}: only {changed} visible clothing pixels"
        );
        println!("{name}: {triangle_count} triangles submitted, {changed} clothing pixels");
    }

    let (running_offset, final_offset, swing) = preview_motion(
        &mut app,
        fighter,
        camera,
        &pixels,
        &save,
        "",
        Vec3::new(-7.0, 2.95, -5.4),
    );
    assert!(
        swing > 0.10,
        "cloth tip should keep moving after the stop: {swing}"
    );
    println!("run-to-stop: mean tip offset {running_offset:.3} m while running and {final_offset:.3} m in the last 0.2 s, peak deviation {swing:.3} m from the latter after stopping");
}

fn preview_motion(
    app: &mut App,
    fighter: Entity,
    camera: Entity,
    pixels: &Mutex<Vec<u8>>,
    save: &impl Fn(&str, &[u8]),
    slot: &str,
    eye: Vec3,
) -> (f32, f32, f32) {
    let mut cloth = app.world_mut().query::<(&Cape, &Visibility)>();
    let world = app.world_mut();
    world.resource_mut::<Settings>().character_motion = true;
    let mut idle = PoseRuntime::idle();
    let character_key = format!("p{}", world.get::<Fighter>(fighter).unwrap().character_id);
    let character = &world.resource::<CharacterAssetsSet>().0[&character_key];
    character.apply_pose(&mut idle, 0, true);
    idle.frame = idle.start as f32 + (idle.end - idle.start) as f32 * 0.18;
    world
        .entity_mut(fighter)
        .insert((idle.clone(), EntryBlend::default()));
    let look = Vec3::new(0., 2.20, -0.2);
    *world.get_mut::<Transform>(camera).unwrap() =
        Transform::from_translation(eye).looking_at(look, Vec3::Y);
    let _ = capture(app, pixels);

    let mut position = Vec3::ZERO;
    let mut previous_pose = 0;
    let mut offsets = Vec::new();
    let mut deep_skirt_faces = 0;
    let mut worst_frame = 0;
    for frame in 0..144 {
        let world = app.world_mut();
        let (pose_id, speed) = match frame {
            0..18 => (0, 0.),
            18..90 => (144, 6.0 * ((frame - 18) as f32 / 12.).min(1.)),
            _ => (0, 0.),
        };
        if previous_pose != pose_id {
            let mut pose = world.get::<PoseRuntime>(fighter).unwrap().clone();
            let mut blend = EntryBlend::default();
            world.resource::<CharacterAssetsSet>().0[&character_key].apply_pose_with_blend(
                &mut pose,
                &mut blend,
                pose_id,
                true,
                Some(0.10),
                EntryBlendKind::Generic,
                false,
                true,
                None,
            );
            if pose_id == 0 {
                pose.frame = idle.frame;
            }
            world.entity_mut(fighter).insert((pose, blend));
            previous_pose = pose_id;
        }
        if pose_id == 144 {
            let mut pose = world.get_mut::<PoseRuntime>(fighter).unwrap();
            let start = pose.loop_start as f32;
            let span = (pose.loop_end - pose.loop_start).max(1) as f32;
            pose.frame = start + ((frame - 18) as f32 * 0.5).rem_euclid(span);
        }
        position.z += speed / 60.;
        world.get_mut::<Transform>(fighter).unwrap().translation = position;
        world.get_mut::<MotionState>(fighter).unwrap().velocity = Vec3::Z * speed;
        *world.get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(position + eye).looking_at(position + look, Vec3::Y);
        app.update();
        app.world()
            .resource::<RenderDevice>()
            .poll(PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(5)),
            })
            .unwrap();
        let (cape, visible) = cloth
            .iter(app.world())
            .find(|(cape, _)| cape.profile.slot == slot)
            .unwrap();
        assert_eq!(*visible, Visibility::Inherited);
        let tip = cape
            .topology
            .points
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.y.total_cmp(&b.y))
            .unwrap()
            .0;
        offsets.push((cape.simulation.positions[tip] - cape.targets[tip]).z);
        if slot == "pink_skirt" && frame % 6 == 0 {
            let deep = deep_body_faces(cape);
            deep_skirt_faces += deep;
            worst_frame = worst_frame.max(deep);
        }
        if frame % 2 == 0 {
            save(&format!("motion-{:03}", frame / 2), &pixels.lock().unwrap());
        }
    }
    let mean = |values: &[f32]| values.iter().sum::<f32>() / values.len() as f32;
    let running_offset = mean(&offsets[74..90]);
    let final_offset = mean(&offsets[132..144]);
    let swing = offsets[90..130]
        .iter()
        .map(|offset| (offset - final_offset).abs())
        .fold(0.0_f32, f32::max);
    if slot == "pink_skirt" {
        println!("P1 skirt: {deep_skirt_faces} face/capture overlaps deeper than 0.025 m; worst sampled frame {worst_frame}");
    }
    (running_offset, final_offset, swing)
}

fn deep_body_faces(cape: &Cape) -> usize {
    use super::super::cloth::contact::CapsuleSurface;
    let capsules = cape
        .capsules
        .iter()
        .map(|&c| CapsuleSurface::new(c, 0.))
        .collect::<Vec<_>>();
    cape.detail
        .as_ref()
        .unwrap()
        .triangles
        .iter()
        .filter(|face| {
            let triangle = face.map(|i| cape.render_world[i]);
            capsules
                .iter()
                .any(|capsule| capsule.depth(triangle) > 0.025)
        })
        .count()
}

fn sheet_vertex_count(points: usize, triangles: &[[usize; 3]]) -> usize {
    let mut adjacent = vec![Vec::new(); points];
    for &[a, b, c] in triangles {
        for (a, b) in [(a, b), (b, c), (c, a)] {
            adjacent[a].push(b);
            adjacent[b].push(a);
        }
    }
    let mut stack = vec![0];
    let mut visited = vec![false; points];
    while let Some(i) = stack.pop() {
        if !visited[i] {
            visited[i] = true;
            stack.extend(adjacent[i].iter().copied());
        }
    }
    visited.into_iter().filter(|&v| v).count()
}

#[test]
#[ignore = "requires native GPU and original P1 assets"]
fn headless_p1_veil_hat_and_pink_skirt() {
    let (mut app, fighter, camera, pixels) = fixture(1);
    let save = preview_output();
    let mut clothing = app
        .world_mut()
        .query::<(&Cape, &Visibility, &MeshMaterial3d<GmdlMaterial>)>();
    for (name, pose_id, fraction, eye, look) in [
        (
            "p1-front",
            0,
            0.18,
            Vec3::new(-3.1, 3.0, 8.5),
            Vec3::new(0., 2.33, 0.),
        ),
        (
            "p1-back",
            0,
            0.18,
            Vec3::new(4.9, 3.0, -7.6),
            Vec3::new(0., 2.33, 0.),
        ),
        (
            "p1-detail",
            0,
            0.18,
            Vec3::new(-1.8, 3.65, 4.15),
            Vec3::new(0., 3.55, 0.),
        ),
        (
            "p1-dodge",
            164,
            0.57,
            Vec3::new(-4.8, 3.23, 8.6),
            Vec3::new(0., 2.37, 0.6),
        ),
    ] {
        let world = app.world_mut();
        let mut pose = PoseRuntime::idle();
        world.resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, pose_id, true);
        pose.frame = pose.start as f32 + (pose.end - pose.start) as f32 * fraction;
        world.entity_mut(fighter).insert(pose);
        *world.get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(eye).looking_at(look, Vec3::Y);
        world.resource_mut::<Settings>().character_motion = true;
        let dressed = capture(&mut app, &pixels);
        save(name, &dressed);
        let world = app.world();
        let mut count = 0;
        for (cape, visibility, material) in clothing.iter(world) {
            assert_eq!(
                *visibility,
                Visibility::Inherited,
                "{name}: {}",
                cape.profile.slot
            );
            let material = world
                .resource::<Assets<GmdlMaterial>>()
                .get(&material.0)
                .unwrap();
            if cape.profile.slot == "veil_hat" {
                assert_eq!(material.mode, GmdlMode::Blend);
                assert!((0.3..0.8).contains(&material.params.opacity));
                let hat = cape.rigid.unwrap();
                let hat_material = world.get::<MeshMaterial3d<GmdlMaterial>>(hat).unwrap();
                assert_eq!(
                    world
                        .resource::<Assets<GmdlMaterial>>()
                        .get(&hat_material.0)
                        .unwrap()
                        .mode,
                    GmdlMode::Opaque
                );
                let transform = world.get::<GlobalTransform>(hat).unwrap();
                let pinned = cape
                    .topology
                    .freedom
                    .iter()
                    .position(|&free| free == 0.0)
                    .unwrap();
                assert!(
                    transform
                        .transform_point(cape.topology.points[pinned])
                        .distance(cape.simulation.positions[pinned])
                        < 1e-3
                );
            } else {
                assert_eq!(cape.profile.slot, "pink_skirt");
                assert_eq!(material.mode, GmdlMode::Opaque);
                if name == "p1-front" {
                    assert_eq!(
                        sheet_vertex_count(cape.topology.points.len(), &cape.topology.triangles),
                        cape.topology.points.len()
                    );
                    assert_eq!(
                        sheet_vertex_count(
                            cape.render_world.len(),
                            &cape.detail.as_ref().unwrap().triangles
                        ),
                        cape.render_world.len()
                    );
                    let up = cape.previous_pose.unwrap().1
                        * rotation_matrix(cape.profile.rotation_degrees)
                        * Vec3::Y;
                    assert!(
                        up.angle_between(Vec3::Y).to_degrees() < 8.,
                        "idle skirt waist pitch: {up:?}"
                    );
                }
            }
            count += 1;
        }
        assert_eq!(count, 2);
        app.world_mut().resource_mut::<Settings>().character_motion = false;
        let plain = capture(&mut app, &pixels);
        if name == "p1-front" {
            save("p1-stock", &plain);
        }
        let changed = dressed
            .chunks_exact(4)
            .zip(plain.chunks_exact(4))
            .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 8))
            .count();
        assert!(changed > 3500, "P1 {name}: {changed} changed pixels");
        println!("P1 {name}: {changed} costume pixels");
    }
    let (_, _, sway) = preview_motion(
        &mut app,
        fighter,
        camera,
        &pixels,
        &save,
        "pink_skirt",
        Vec3::new(-6.15, 3.14, 7.80),
    );
    assert!(sway > 0.08, "skirt should sway after stopping: {sway}");
    println!("P1 skirt run-to-stop deviation: {sway:.3} m");
}
