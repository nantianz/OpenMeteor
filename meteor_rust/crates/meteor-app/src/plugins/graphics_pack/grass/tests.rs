use super::*;
use bevy::ecs::system::RunSystemOnce;
use meteor_asset::gmdl::{GmdlMaterial, GmdlMesh, GmdlObject, GmdlTexture};
use std::collections::BTreeSet;

fn save_preview(dir: &std::path::Path, name: &str, data: Vec<u8>, width: u32, height: u32) {
    std::fs::create_dir_all(dir).unwrap();
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
}

fn spawn_stock_preview(app: &mut App, stage: &'static str) -> Option<[Transform; 2]> {
    use crate::plugins::stage_material::{
        GmdlMaterial as StageMaterial, GmdlMaterialParams, GmdlMode,
    };
    use crate::plugins::{pak_textures, render_plugin, stage_collider::StageMeshCollider};
    use meteor_physics::collision::MeshCollider;
    use std::collections::HashMap;

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let read = |path: &str| std::fs::read(root.join(path)).unwrap();
    let source = meteor_asset::gmdl::decode_gmdl(
        &std::fs::read(root.join(format!("{stage}/{stage}.gmb"))).ok()?,
    )
    .unwrap();
    let desc = meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(&read(
        &format!("{stage}/{stage}.des"),
    )));
    let cob = meteor_asset::cob::parse_cob(&read(&format!("{stage}/{stage}.cob"))).unwrap();
    let center = meteor_scene::stage_loader::get_stage_center(
        stage,
        Some(&crate::text_codec::decode_text(&read(&format!(
            "Level/{stage}.wp"
        )))),
    );
    let support = StageMeshCollider::from_gmdl(&source, &desc, Some(&cob.face_ids), center);
    let cameras = if stage == "sn06" {
        let center = Vec3::new(2.67 - center[0] * 0.12, 0., 1.04 + center[1] * 0.12);
        let ground = support
            .sample_ground_hit(center.x, center.z, -8.0, 4.0)
            .map_or(-2.0, |hit| hit.y);
        let center = center.with_y(ground);
        [
            Transform::from_translation(center + Vec3::new(18., 97., 88.))
                .looking_at(center, Vec3::Y),
            Transform::from_translation(center + Vec3::new(-47., 4.6, 44.))
                .looking_at(center + Vec3::new(35., 0.1, -40.), Vec3::Y),
        ]
    } else {
        [
            (Vec2::new(-84.4, -49.2), Vec2::new(-68.4, -44.7)),
            (Vec2::new(118.1, -0.9), Vec2::new(117.9, 36.5)),
        ]
        .map(|(eye, target)| {
            let height = support
                .sample_ground_hit(eye.x, eye.y, -6.0, 2.0)
                .map_or(0., |hit| hit.y);
            Transform::from_xyz(eye.x, height + 3.4, eye.y)
                .looking_at(Vec3::new(target.x, height - 0.5, target.y), Vec3::Y)
        })
    };
    let packs = super::super::tests::starter();
    app.world_mut()
        .run_system_once(
            move |mut commands: Commands,
                  old: Query<Entity, With<PlayingSceneEntity>>,
                  mut meshes: ResMut<Assets<Mesh>>,
                  mut images: ResMut<Assets<Image>>,
                  mut grass: ResMut<Assets<GrassMaterial>>,
                  mut materials: ResMut<Assets<StageMaterial>>| {
                for entity in &old {
                    commands.entity(entity).despawn();
                }
                let profile = spawn(
                    &mut commands,
                    &mut meshes,
                    &mut images,
                    &mut grass,
                    &packs,
                    stage,
                    &source,
                    Some(&desc),
                    &cob.face_ids,
                    Some(&support),
                    center,
                )
                .expect("stock grass profile");
                let mut cache = pak_textures::PakCache::default();
                let mut textures = HashMap::new();
                for object in &source.objects {
                    let description = desc
                        .scene_objects
                        .iter()
                        .find(|value| value.name == object.name);
                    if description.is_some_and(|value| value.skip_render() || value.billboard == 1)
                    {
                        continue;
                    }
                    let mut groups = std::collections::BTreeMap::<usize, Vec<u32>>::new();
                    for (&face, indices) in
                        object.face_mat.iter().zip(object.indices.chunks_exact(3))
                    {
                        if face >= 0 {
                            groups
                                .entry(face as usize)
                                .or_default()
                                .extend_from_slice(indices);
                        }
                    }
                    for (index, indices) in groups {
                        let raw = &source.materials[index];
                        let (mode, opacity, alpha_cutoff) =
                            render_plugin::classify_gmdl_material(raw);
                        if mode != GmdlMode::Opaque {
                            continue;
                        }
                        let name = source.resolve_texture_name(raw).unwrap_or_default();
                        let texture = textures
                            .entry(name)
                            .or_insert_with(|| {
                                cache.resolve(&root, stage, name).and_then(|bytes| {
                                    pak_textures::image_handle_with_quality(&mut images, &bytes, 0)
                                })
                            })
                            .clone();
                        let mut material = StageMaterial::new_scene(
                            GmdlMaterialParams {
                                opacity,
                                alpha_cutoff,
                                mode: mode.as_u32(),
                                has_texture: u32::from(texture.is_some()),
                                uv_offset: Vec2::ZERO,
                            },
                            texture,
                            raw.two_side,
                            mode,
                        );
                        if alpha_cutoff == 0.0 {
                            profile.configure(&object.name, name, &mut material);
                        }
                        let (mesh, transform) =
                            render_plugin::build_scene_mesh(object, &indices, center, description);
                        commands.spawn((
                            PlayingSceneEntity,
                            Mesh3d(meshes.add(mesh)),
                            MeshMaterial3d(materials.add(material)),
                            transform,
                        ));
                    }
                }
                commands.insert_resource(crate::plugins::stage_collider::StageCollider {
                    mesh: Some(support.clone()),
                });
            },
        )
        .unwrap();
    Some(cameras)
}

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
    let (tiles, _) = placement::place(
        profile,
        &mesh,
        Some(&desc),
        &cob.face_ids,
        Some(&ground),
        center,
    )
    .unwrap();
    let count: usize = tiles.iter().map(|tile| tile.roots.len()).sum();
    assert!(count > 32_000 && tiles.len() <= 512);
    let mut species = [0; 3];
    for root in tiles.iter().flat_map(|tile| &tile.roots) {
        species[((root.w as u32 - 1) >> 8 & 3) as usize] += 1;
    }
    assert!(species.into_iter().all(|count| count > 1000));
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
        "sn03 grass: {count} clumps in {} tiles, near {waypoints_near_grass} waypoints, species {species:?}", tiles.len()
    );
}

#[cfg(feature = "original-assets")]
#[test]
fn si_fang_zhen_courtyard_reaches_all_edges_with_a_bounded_distant_lod() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let Ok(bytes) = std::fs::read(dir.join("sn06/sn06.gmb")) else {
        return;
    };
    let mesh = meteor_asset::gmdl::decode_gmdl(&bytes).unwrap();
    let des = std::fs::read(dir.join("sn06/sn06.des")).unwrap();
    let desc = meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(&des));
    let files = std::fs::read(dir.join("sn06/sn06.cob")).unwrap();
    let cob = meteor_asset::cob::parse_cob(&files).unwrap();
    let center = meteor_scene::stage_loader::get_stage_center("sn06", None);
    let terrain = crate::plugins::stage_collider::StageMeshCollider::from_gmdl(
        &mesh,
        &desc,
        Some(&cob.face_ids),
        center,
    );
    let packs = super::super::tests::starter();
    let profile = packs
        .scenes("sn06")
        .find_map(|(_, scene)| scene.grass.as_ref())
        .unwrap();
    let (tiles, cover) = placement::place(
        profile,
        &mesh,
        Some(&desc),
        &cob.face_ids,
        Some(&terrain),
        center,
    )
    .unwrap();
    let channels = placement::ground_channels(&cover, &tiles);
    let mut bare = 0;
    let mut lush = 0;
    for (&edge, pixel) in cover.depths.iter().zip(channels.chunks_exact(2)) {
        assert_eq!(pixel[0], edge);
        if edge == 0 {
            assert_eq!(pixel[1], 0);
        } else if edge >= 220 {
            bare += usize::from(pixel[1] <= 76);
            lush += usize::from(pixel[1] >= 174);
        }
    }
    assert!(bare > 100 && lush > bare, "bare {bare}, lush {lush}");
    let min = tiles
        .iter()
        .map(|tile| tile.min)
        .fold(Vec3::splat(f32::INFINITY), Vec3::min);
    let max = tiles
        .iter()
        .map(|tile| tile.max)
        .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
    assert!(
        (max.x - min.x) > 115.0 && (max.z - min.z) > 115.0,
        "{min:?} .. {max:?}"
    );
    assert!(tiles.len() >= 125, "only {} tiles", tiles.len());
    assert!(tiles.len() <= profile.max_visible_tiles);
    assert!(LODS[DISTANT].triangles(ROOTS_PER_TILE) * tiles.len() <= 49_152);
    assert!(profile.max_triangles <= 168_000);
    eprintln!("sn06 courtyard: {} occupied grass tiles", tiles.len());
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
    use crate::plugins::stage_material::{
        FrameUniforms, GmdlMaterial as GroundMaterial, GmdlMaterialParams, GmdlMaterialPlugin,
        GmdlMode,
    };
    use bevy::{
        app::PluginsState,
        camera::RenderTarget,
        render::{
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
    .add_plugins(GmdlMaterialPlugin)
    .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_millis(34),
    ));
    install(&mut app);
    crate::plugins::frame_work::install(&mut app);
    app.add_systems(
        PostUpdate,
        (|settings: Res<Settings>, mut frame: ResMut<FrameUniforms>| {
            frame.graphics.counts.z = f32::from(settings.grass_enhance);
        })
        .after(bevy::transform::TransformSystems::Propagate),
    );
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
    let readback = app
        .world_mut()
        .spawn(Readback::texture(target.clone()))
        .observe(move |event: On<ReadbackComplete>| {
            *output.lock().unwrap() = event.data.clone();
        })
        .id();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(Duration::from_millis(2));
    }
    app.finish();
    app.cleanup();
    let mut pack = super::super::tests::starter();
    pack.packs[0].config.scenes[0]
        .wind
        .as_mut()
        .unwrap()
        .strength = 0.0;
    let profile = pack.packs[0]
        .config
        .scenes
        .iter_mut()
        .find(|scene| scene.stage == "sn03")
        .and_then(|scene| scene.grass.as_mut())
        .unwrap();
    profile.tile_size = 6.0;
    profile.max_visible_tiles = 12;
    profile.patch_variation = 0.0;
    let ground = GroundMaterial::new_scene(
        GmdlMaterialParams {
            opacity: 1.0,
            alpha_cutoff: 0.0,
            mode: GmdlMode::Opaque.as_u32(),
            has_texture: 0,
            uv_offset: Vec2::ZERO,
        },
        None,
        true,
        GmdlMode::Opaque,
    );
    let ground = app
        .world_mut()
        .resource_mut::<Assets<GroundMaterial>>()
        .add(ground);
    let mut terrain = Plane3d::default().mesh().size(36., 36.).build();
    terrain.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        vec![[0.30, 0.24, 0.17, 1.]; terrain.count_vertices()],
    );
    let terrain = app.world_mut().resource_mut::<Assets<Mesh>>().add(terrain);
    app.world_mut().spawn((
        PlayingSceneEntity,
        Mesh3d(terrain),
        MeshMaterial3d(ground.clone()),
        Transform::IDENTITY,
    ));
    let fixture_ground_material = ground.clone();
    app.world_mut()
        .run_system_once(
            move |mut commands: Commands,
                  mut meshes: ResMut<Assets<Mesh>>,
                  mut images: ResMut<Assets<Image>>,
                  mut materials: ResMut<Assets<GrassMaterial>>,
                  mut ground_materials: ResMut<Assets<GroundMaterial>>| {
                let coverage = spawn(
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
                )
                .unwrap();
                coverage.configure(
                    "ground",
                    "sn302.jpg",
                    &mut ground_materials.get_mut(&ground).unwrap(),
                );
            },
        )
        .unwrap();
    let capture_size = |app: &mut App, frames, width, height| {
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
        assert_eq!(data.len(), (width * height * 4) as usize);
        data
    };
    let capture = |app: &mut App, frames| capture_size(app, frames, WIDTH, HEIGHT);
    let scanned = capture(&mut app, 20);
    {
        let mut materials = app.world_mut().resource_mut::<Assets<GroundMaterial>>();
        let mut ground = materials.get_mut(&fixture_ground_material).unwrap();
        assert!(ground.grass_underlay.is_some());
        ground.grass_underlay_scale = 0.0;
    }
    let baseline = capture(&mut app, 20);
    let changed_ground = scanned
        .chunks_exact(4)
        .zip(baseline.chunks_exact(4))
        .filter(|(scan, procedural)| (0..3).any(|i| scan[i].abs_diff(procedural[i]) > 12))
        .count();
    assert!(
        changed_ground > 1000,
        "scanned ground only changed {changed_ground} pixels"
    );
    let night = crate::plugins::graphics_pack::GraphicsFrame {
        sky: Vec4::new(0.014, 0.026, 0.047, 0.0),
        environment: Vec4::new(1.0, 0.0, 1.0, 0.0),
        counts: Vec4::W,
        ..default()
    };
    app.world_mut().resource_mut::<FrameUniforms>().graphics = night;
    let night_pixels = capture(&mut app, 12);
    let green_energy = |data: &[u8]| {
        data.iter()
            .skip(1)
            .step_by(4)
            .map(|&v| u64::from(v))
            .sum::<u64>()
    };
    assert!(
        green_energy(&night_pixels) * 2 < green_energy(&baseline),
        "grass blades must get darker at night"
    );
    let mut directional = crate::plugins::graphics_pack::GraphicsFrame {
        sky: Vec4::new(0.69, 0.69, 0.69, 0.0),
        sun: Vec4::new(1.0, 0.5, 0.0, 1.0),
        environment: Vec4::new(1.0, 0.0, 1.0, 0.0),
        counts: Vec4::W,
        ..default()
    };
    app.world_mut().resource_mut::<FrameUniforms>().graphics = directional;
    let east_light = capture(&mut app, 12);
    directional.sun = Vec4::new(0.0, 0.5, 1.0, 1.0);
    app.world_mut().resource_mut::<FrameUniforms>().graphics = directional;
    let north_light = capture(&mut app, 12);
    let changed_direction = east_light
        .chunks_exact(4)
        .zip(north_light.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 2))
        .count();
    assert!(
        changed_direction > 200,
        "grass must follow light direction: {changed_direction}"
    );
    app.world_mut().resource_mut::<FrameUniforms>().graphics = default();
    assert_eq!(capture(&mut app, 12), baseline);

    let blocker = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(5.0, 1.0, 5.0));
    let blocker_material = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(Color::srgb(0.4, 0.2, 0.1));
    let blocker = app
        .world_mut()
        .spawn((
            Mesh3d(blocker),
            MeshMaterial3d(blocker_material),
            Transform::from_xyz(0.0, 5.0, 0.0),
        ))
        .id();
    let sun = app
        .world_mut()
        .spawn((
            DirectionalLight {
                shadow_maps_enabled: true,
                ..default()
            },
            Transform::from_xyz(0.0, 20.0, 4.0).looking_at(Vec3::ZERO, Vec3::Z),
        ))
        .id();
    let unshadowed = capture(&mut app, 30);
    app.world_mut().resource_mut::<FrameUniforms>().lighting.w = 0.8;
    let shadowed = capture(&mut app, 20);
    let darkened = unshadowed
        .chunks_exact(4)
        .zip(shadowed.chunks_exact(4))
        .filter(|(before, after)| {
            before[1] > before[0].saturating_add(8) && before[1] > after[1].saturating_add(6)
        })
        .count();
    assert!(
        darkened > 50,
        "projected shadows must darken grass: {darkened}"
    );
    app.world_mut().despawn(blocker);
    app.world_mut().despawn(sun);
    app.world_mut().resource_mut::<FrameUniforms>().lighting.w = 0.0;
    assert_eq!(capture(&mut app, 20), baseline);
    app.world_mut().resource_mut::<FrameUniforms>().seconds = 19.0;
    assert_eq!(capture(&mut app, 8), baseline, "calm field must stay still");
    let count = |world: &mut World| {
        world
            .query_filtered::<&Visibility, With<GrassTile>>()
            .iter(world)
            .filter(|visibility| **visibility != Visibility::Hidden)
            .count()
    };
    assert!((1..=12).contains(&count(app.world_mut())));
    let allocation = |world: &mut World| {
        let mut entries: Vec<_> = world
            .query_filtered::<(Entity, &Mesh3d, &Visibility), With<GrassTile>>()
            .iter(world)
            .map(|(id, mesh, visibility)| (id, mesh.0.id(), *visibility))
            .collect();
        entries.sort_unstable_by_key(|(id, _, _)| *id);
        entries
    };
    let pose = *app.world().get::<Transform>(camera).unwrap();
    let original = allocation(app.world_mut());
    let mut changed_visibility = false;
    for angle in [-1.0, 0.65, 1.4] {
        let mut rotated = pose;
        rotated.rotate_y(angle);
        *app.world_mut().get_mut::<Transform>(camera).unwrap() = rotated;
        capture(&mut app, 8);
        let turned = allocation(app.world_mut());
        assert_eq!(original.len(), turned.len());
        for ((id, before, visible), (turned_id, after, turned_visible)) in
            original.iter().zip(turned)
        {
            assert_eq!(
                (*id, *before),
                (turned_id, after),
                "turning the camera changed a tile's LOD"
            );
            changed_visibility |= *visible != turned_visible;
        }
    }
    assert!(
        changed_visibility,
        "the turn fixture must exercise frustum culling"
    );
    *app.world_mut().get_mut::<Transform>(camera).unwrap() = pose;
    assert_eq!(
        capture(&mut app, 12),
        baseline,
        "a calm field must return to the same pixels"
    );
    let green_pixels = |frame: &[u8]| {
        frame
            .chunks_exact(4)
            .filter(|p| p[1] > 45 && p[1] > p[0].saturating_add(8))
            .count()
    };
    let greens = green_pixels(&baseline);
    assert!(
        greens > (WIDTH * HEIGHT / 3) as usize,
        "grass pixels: {greens}"
    );
    let submitted = |world: &mut World| {
        let current: Vec<_> = world
            .query::<(&GrassTile, &Visibility, &Mesh3d)>()
            .iter(world)
            .filter(|(_, visible, _)| **visible != Visibility::Hidden)
            .map(|(tile, _, mesh)| (tile.bucket, mesh.0.clone()))
            .collect();
        let mut query = world.query::<&GrassField>();
        let field = query.single(world).unwrap();
        current
            .iter()
            .map(|(bucket, mesh)| {
                let level = field.lods[*bucket]
                    .iter()
                    .position(|handle| handle == mesh)
                    .unwrap();
                LODS[level].triangles(clump_capacity(*bucket))
            })
            .sum::<usize>()
    };
    let triangles = submitted(app.world_mut());
    assert!(
        triangles > 20_000 && triangles <= 104_000,
        "{triangles} submitted triangles"
    );
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
        contact_pixels > 60 && contact_pixels < (WIDTH * HEIGHT / 15) as usize,
        "contact pixels: {contact_pixels}"
    );
    app.world_mut().despawn(fighter);
    let recovered = capture(&mut app, 85);
    assert_eq!(recovered, baseline);
    app.world_mut().resource_mut::<Settings>().grass_enhance = false;
    let off = capture(&mut app, 16);
    assert_eq!(count(app.world_mut()), 0);
    assert_eq!(green_pixels(&off), 0);
    if let Some(dir) = std::env::var_os("METEOR_GRASS_CAPTURE_DIR") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, data) in [
            ("meadow.png", baseline),
            ("meadow-pressed.png", pressed),
            ("meadow-disabled.png", off),
        ] {
            save_preview(&dir, name, data, WIDTH, HEIGHT);
        }
    }
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
    eprintln!(
        "{triangles} submitted triangles, greens on {greens}/{} pixels",
        WIDTH * HEIGHT
    );
    if std::env::var_os("METEOR_GRASS_BENCH").is_some() {
        app.world_mut().entity_mut(readback).remove::<Readback>();
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .get_mut(&target)
            .unwrap()
            .resize(Extent3d {
                width: 1920,
                height: 1080,
                depth_or_array_layers: 1,
            });
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation
            .x -= 150.0;
        let mut fields = app.world_mut().query::<&mut GrassField>();
        fields
            .single_mut(app.world_mut())
            .unwrap()
            .profile
            .max_visible_tiles = 96;
        let mut times = [Vec::new(), Vec::new()];
        for enabled in [false, true, true, false] {
            app.world_mut().resource_mut::<Settings>().grass_enhance = enabled;
            for sample in 0..48 {
                let start = std::time::Instant::now();
                app.update();
                app.world()
                    .resource::<RenderDevice>()
                    .poll(PollType::Wait {
                        submission_index: None,
                        timeout: Some(Duration::from_secs(5)),
                    })
                    .unwrap();
                if sample >= 12 {
                    times[usize::from(enabled)].push(start.elapsed().as_secs_f64() * 1000.0);
                }
            }
            if enabled {
                let count = submitted(app.world_mut());
                assert!(count > 0 && count <= 104_000);
                eprintln!("1080p benchmark submits {count} grass triangles");
            }
        }
        for (enabled, data) in times.iter_mut().enumerate() {
            data.sort_by(f64::total_cmp);
            eprintln!(
                "1080p CPU + GPU completion enabled={enabled}: median {:.3} ms, p95 {:.3} ms ({} frames)",
                data[data.len() / 2], data[data.len() * 95 / 100], data.len(),
            );
        }
    }
    if let Some(dir) = std::env::var_os("METEOR_GRASS_CAPTURE_DIR") {
        let stage = if std::env::var("METEOR_GRASS_SCENE").as_deref() == Ok("sn06") {
            "sn06"
        } else {
            "sn03"
        };
        if let Some(cameras) = spawn_stock_preview(&mut app, stage) {
            let dir = std::path::PathBuf::from(dir);
            let (width, height) = if stage == "sn06" {
                (1280, 720)
            } else {
                (WIDTH, HEIGHT)
            };
            app.world_mut()
                .resource_mut::<Assets<Image>>()
                .get_mut(&target)
                .unwrap()
                .resize(Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                });
            app.world_mut()
                .entity_mut(readback)
                .insert(Readback::texture(target.clone()));
            app.world_mut().resource_mut::<Settings>().grass_enhance = true;
            for (i, transform) in cameras.into_iter().enumerate() {
                *app.world_mut().get_mut::<Transform>(camera).unwrap() = transform;
                let pixels = capture_size(&mut app, 28, width, height);
                save_preview(
                    &dir,
                    &format!("{stage}-meadow-{}.png", i + 1),
                    pixels,
                    width,
                    height,
                );
            }
            if stage == "sn06" {
                let combat_lanes = std::env::var("METEOR_GRASS_COMBAT_BENCH")
                    .ok()
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(0)
                    .min(12);
                let combat_bench = combat_lanes > 0;
                app.world_mut().entity_mut(readback).remove::<Readback>();
                app.world_mut()
                    .resource_mut::<Assets<Image>>()
                    .get_mut(&target)
                    .unwrap()
                    .resize(Extent3d {
                        width: 2560,
                        height: 1440,
                        depth_or_array_layers: 1,
                    });
                for (i, transform) in cameras.into_iter().enumerate() {
                    if combat_bench && i == 0 {
                        continue;
                    }
                    *app.world_mut().get_mut::<Transform>(camera).unwrap() = transform;
                    let actor = combat_bench.then(|| {
                        app.world_mut()
                            .spawn((
                                Fighter {
                                    character_id: 0,
                                    player_slot: 0,
                                    weapon: 0,
                                    weapon_slots: [0; 2],
                                    weapon_slot: 0,
                                    weapon_variant: 0,
                                    weapon_slot_variants: [0; 2],
                                },
                                Transform::from_translation(
                                    transform.translation - Vec3::Y * 4.6 - Vec3::Z * 2.0,
                                ),
                            ))
                            .id()
                    });
                    let mut times = [Vec::new(), Vec::new()];
                    for enabled in [false, true, true, false] {
                        app.world_mut().resource_mut::<Settings>().grass_enhance =
                            combat_bench || enabled;
                        let revision = app.world().resource::<BendUpload>().revision;
                        for sample in 0..96 {
                            let start = std::time::Instant::now();
                            if enabled && combat_bench {
                                let origin = transform.translation - Vec3::Y * 3.8;
                                let jitter = (sample % 12) as f32 * 0.3;
                                let mut combat = app.world_mut().resource_mut::<GrassCombat>();
                                for lane in 0..combat_lanes {
                                    let a = origin + Vec3::new(jitter + lane as f32 * 0.6, 0., -1.);
                                    combat.record_segment(a, a - Vec3::Z * 3.5, 0.22);
                                }
                            }
                            app.update();
                            app.world()
                                .resource::<RenderDevice>()
                                .poll(PollType::Wait {
                                    submission_index: None,
                                    timeout: Some(Duration::from_secs(5)),
                                })
                                .unwrap();
                            if sample >= 24 {
                                times[usize::from(enabled)]
                                    .push(start.elapsed().as_secs_f64() * 1000.0);
                            }
                        }
                        if enabled && combat_bench {
                            assert!(app.world().resource::<BendUpload>().revision > revision);
                        }
                        if enabled {
                            let triangles = submitted(app.world_mut());
                            let tiles = count(app.world_mut());
                            assert!(triangles > 0 && triangles <= 168_000 && tiles <= 512);
                            eprintln!(
                                "sn06 view {}: {triangles} triangles, {tiles} grass tiles",
                                i + 1
                            );
                        }
                    }
                    for (enabled, data) in times.iter_mut().enumerate() {
                        data.sort_by(f64::total_cmp);
                        let label = if combat_bench {
                            "grass-attacks"
                        } else {
                            "grass"
                        };
                        eprintln!("sn06 1440p view {} {label}={enabled}: median {:.3} ms, p95 {:.3} ms ({} frames)",
                            i + 1, data[data.len() / 2], data[data.len() * 95 / 100], data.len());
                    }
                    if let Some(actor) = actor {
                        app.world_mut().despawn(actor);
                    }
                }
            }
        }
    }
}
