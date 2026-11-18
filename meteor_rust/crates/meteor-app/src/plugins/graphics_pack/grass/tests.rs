use super::*;
use bevy::ecs::system::RunSystemOnce;
use meteor_asset::gmdl::{GmdlMaterial, GmdlMesh, GmdlObject, GmdlTexture};
use std::collections::BTreeSet;

#[test]
fn stock_grass_uses_the_authored_ground_and_stays_inside_the_budget() {
    use meteor_physics::collision::MeshCollider;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let bytes = match std::fs::read(dir.join("sn03/sn03.gmb")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("{error}"),
    };
    let mesh = meteor_asset::gmdl::decode_gmdl(&bytes).unwrap();
    let des = std::fs::read(dir.join("sn03/sn03.des")).unwrap();
    let desc = meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(&des));
    let cob =
        meteor_asset::cob::parse_cob(&std::fs::read(dir.join("sn03/sn03.cob")).unwrap()).unwrap();
    let wp = std::fs::read(dir.join("Level/sn03.wp")).unwrap();
    let wp = crate::text_codec::decode_text(&wp);
    let center = meteor_scene::stage_loader::get_stage_center("sn03", Some(&wp));
    let pack = super::super::tests::starter();
    let profile = pack
        .scenes("sn03")
        .find_map(|(_, scene)| scene.grass.as_ref())
        .unwrap();
    let ground = crate::plugins::stage_collider::StageMeshCollider::from_gmdl(
        &mesh,
        &desc,
        Some(&cob.face_ids),
        center,
    );
    let tiles = placement::place(
        profile,
        &mesh,
        Some(&desc),
        &cob.face_ids,
        Some(&ground),
        center,
    )
    .unwrap();
    let count: usize = tiles.iter().map(|tile| tile.roots.len()).sum();
    assert!(count > 500 && tiles.len() <= 512);
    let waypoints_near_grass = wp
        .lines()
        .filter_map(|line| {
            let values: Vec<f32> = line
                .strip_prefix("Pos ")?
                .split_whitespace()
                .filter_map(|value| value.parse().ok())
                .collect();
            let [x, y, z] = values.as_slice() else {
                return None;
            };
            let mut point = Vec3::new(x - center[0], z - center[2], center[1] - y)
                * meteor_core::constants::MODEL_SCALE;
            point.y = ground
                .sample_ground_hit(point.x, point.z, point.y - 6.0, point.y)?
                .y;
            Some(point)
        })
        .filter(|point| {
            tiles.iter().flat_map(|tile| &tile.roots).any(|root| {
                let delta = root.truncate() - *point;
                delta.xz().length_squared() < 4.0 && delta.y.abs() < 0.6
            })
        })
        .count();
    assert!(
        waypoints_near_grass >= 4,
        "grass is near only {waypoints_near_grass} waypoints"
    );
    for tile in &tiles {
        assert!(tile.roots.len() <= ROOTS_PER_TILE);
        for root in tile.roots.iter().step_by(17) {
            let point = root.truncate();
            let hit = ground
                .sweep_foot_segment(point.x, point.y + 0.6, point.z, point.y - 0.08)
                .unwrap();
            assert!(
                (point.y - hit.y).abs() < 0.065,
                "{point:?}, found {}",
                hit.y
            );
        }
    }
    eprintln!(
        "sn03 grass: {count} roots in {} tiles, near {waypoints_near_grass} waypoints",
        tiles.len()
    );
}

fn fixture_ground() -> GmdlMesh {
    let positions = [
        [-18., 18., 0.],
        [-18., -18., 0.],
        [18., -18., 0.],
        [18., 18., 0.],
    ]
    .into_iter()
    .flatten()
    .map(|value| value / meteor_core::constants::MODEL_SCALE)
    .collect();
    GmdlMesh {
        textures: vec![GmdlTexture {
            name: "sn302.jpg".into(),
        }],
        materials: vec![GmdlMaterial {
            tex_index: 0,
            blend: "NORMAL".into(),
            state: "DISABLE".into(),
            alpha: 1.,
            two_side: false,
        }],
        objects: vec![GmdlObject {
            name: "ground".into(),
            num_verts: 4,
            num_faces: 2,
            positions,
            normals: vec![],
            uvs: vec![],
            colors: vec![],
            face_mat: vec![0, 0],
            indices: vec![0, 1, 2, 0, 2, 3],
        }],
    }
}

#[test]
fn grass_gpu_draws_tracks_recovers_and_honors_visible_tile_limit() {
    use bevy::{
        app::PluginsState,
        camera::RenderTarget,
        diagnostic::{DiagnosticPath, DiagnosticsStore},
        render::{
            diagnostic::RenderDiagnosticsPlugin,
            gpu_readback::{Readback, ReadbackComplete},
            render_resource::{CachedPipelineState, PipelineCache, PollType, TextureUsages},
            renderer::RenderDevice,
            RenderApp, RenderPlugin,
        },
    };
    use std::{sync::Mutex, time::Duration};
    const WIDTH: u32 = 640;
    const HEIGHT: u32 = 360;
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
    .insert_resource(Settings::default())
    .init_resource::<DiagnosticsStore>()
    .add_plugins(RenderDiagnosticsPlugin)
    .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_millis(34),
    ));
    install(&mut app);
    let mut target = Image::new_target_texture(WIDTH, HEIGHT, TextureFormat::Rgba8UnormSrgb, None);
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    let camera = app
        .world_mut()
        .spawn((
            GameplayCamera,
            Camera3d::default(),
            Camera {
                clear_color: ClearColorConfig::Custom(Color::BLACK),
                ..default()
            },
            RenderTarget::Image(target.clone().into()),
            Msaa::Off,
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            Transform::from_xyz(0., 4.5, 9.).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id();
    let pixels = Arc::new(Mutex::new(Vec::new()));
    let output = pixels.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |event: On<ReadbackComplete>| {
            *output.lock().unwrap() = event.data.clone();
        });
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(Duration::from_millis(2));
    }
    app.finish();
    app.cleanup();
    let mut pack = super::super::tests::starter();
    let profile = pack.packs[0]
        .config
        .scenes
        .iter_mut()
        .find_map(|scene| scene.grass.as_mut())
        .unwrap();
    profile.wind_strength = 0.0;
    profile.tile_size = 6.0;
    profile.max_visible_tiles = 12;
    app.world_mut()
        .run_system_once(
            move |mut commands: Commands,
                  mut meshes: ResMut<Assets<Mesh>>,
                  mut images: ResMut<Assets<Image>>,
                  mut materials: ResMut<Assets<GrassMaterial>>| {
                spawn(
                    &mut commands,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    &pack,
                    "sn03",
                    &fixture_ground(),
                    None,
                    &BTreeSet::new(),
                    None,
                    [0.; 3],
                );
            },
        )
        .unwrap();
    let capture = |app: &mut App, frames| {
        for _ in 0..frames {
            app.update();
            app.world()
                .resource::<RenderDevice>()
                .poll(PollType::Poll)
                .unwrap();
            std::thread::sleep(Duration::from_millis(4));
        }
        let pipelines = app.sub_app(RenderApp).world().resource::<PipelineCache>();
        for pipeline in pipelines.pipelines() {
            if let CachedPipelineState::Err(error) = &pipeline.state {
                panic!("grass pipeline: {error}");
            }
        }
        let data = pixels.lock().unwrap().clone();
        assert_eq!(data.len(), (WIDTH * HEIGHT * 4) as usize);
        data
    };
    let baseline = capture(&mut app, 20);
    let gpu_time = |app: &mut App| {
        let path = DiagnosticPath::new("render/main_opaque_pass_3d/elapsed_gpu");
        let mut samples = Vec::new();
        let mut previous = None;
        for _ in 0..36 {
            app.update();
            app.world()
                .resource::<RenderDevice>()
                .poll(PollType::Poll)
                .unwrap();
            if let Some(sample) = app
                .world()
                .resource::<DiagnosticsStore>()
                .get_measurement(&path)
            {
                if previous.replace(sample.time) != Some(sample.time) {
                    samples.push(sample.value);
                }
            }
        }
        samples.sort_by(f64::total_cmp);
        samples.get(samples.len() / 2).copied()
    };
    let on_ms = gpu_time(&mut app);
    let count = |world: &mut World| {
        world
            .query_filtered::<&Visibility, With<GrassTile>>()
            .iter(world)
            .filter(|visibility| **visibility != Visibility::Hidden)
            .count()
    };
    assert_eq!(count(app.world_mut()), 12);
    let grass_pixels = baseline.chunks_exact(4).filter(|p| p[1] > 50).count();
    assert!(grass_pixels > 2500, "grass pixels: {grass_pixels}");
    let fighter = app
        .world_mut()
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
            MotionState {
                movement_mode: MovementMode::Walk,
                velocity: Vec3::X,
                facing: 0.0,
            },
            Transform::IDENTITY,
        ))
        .id();
    let pressed = capture(&mut app, 16);
    let changed = |a: &[u8], b: &[u8]| {
        a.chunks_exact(4)
            .zip(b.chunks_exact(4))
            .filter(|(p, q)| p[1].abs_diff(q[1]) > 3)
            .count()
    };
    let contact_pixels = changed(&baseline, &pressed);
    assert!(
        contact_pixels > 60 && contact_pixels < 6000,
        "contact pixels: {contact_pixels}"
    );
    app.world_mut().despawn(fighter);
    let recovered = capture(&mut app, 85);
    assert_eq!(recovered, baseline);
    app.world_mut().resource_mut::<Settings>().grass_enhance = false;
    let off = capture(&mut app, 16);
    let off_ms = gpu_time(&mut app);
    assert_eq!(count(app.world_mut()), 0);
    assert!(off.chunks_exact(4).all(|p| p[1] == 0));
    app.world_mut().resource_mut::<Settings>().grass_enhance = true;
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation
        .x += 150.;
    capture(&mut app, 16);
    assert_eq!(count(app.world_mut()), 0);
    eprintln!(
        "grass interaction changed {contact_pixels} of {} pixels",
        WIDTH * HEIGHT
    );
    eprintln!("{WIDTH}x{HEIGHT} opaque GPU median with 12 visible tiles: on={on_ms:?} ms, off={off_ms:?} ms");
}
