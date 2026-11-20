use super::render_tests::capture;
use super::*;
use crate::app_state::{AppState, Settings};
use crate::plugins::stage_material::{
    scene_normals, CameraOcclusionParams, FrameUniforms, GmdlMaterialParams, GmdlMaterialPlugin,
};
use crate::plugins::{pak_textures, render_plugin};
use bevy::render::{
    gpu_readback::{Readback, ReadbackComplete},
    render_resource::TextureUsages,
    RenderPlugin,
};
use bevy::{app::PluginsState, camera::RenderTarget};
use std::sync::{Arc, Mutex};

#[derive(Resource, Default)]
struct TestCameraFade(CameraOcclusionParams);

fn render_original_liquid(stage: &str, eye: Vec3, aim: Vec3, live_reflection: bool) {
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
        scene_normal_maps: true,
        ..default()
    })
    .add_plugins(GmdlMaterialPlugin)
    .init_resource::<TestCameraFade>()
    .add_systems(
        PostUpdate,
        |fade: Res<TestCameraFade>, mut frame: ResMut<FrameUniforms>| {
            frame.camera = fade.0;
        },
    );
    crate::plugins::frame_work::install(&mut app);
    let mut image = Image::new_target_texture(512, 288, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    let world = |p: Vec3| Vec3::new(p.x, p.z, -p.y) * MODEL_SCALE;
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera {
                clear_color: ClearColorConfig::Custom(Color::BLACK),
                ..default()
            },
            RenderTarget::Image(target.clone().into()),
            Msaa::Off,
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            Transform::from_translation(world(eye)).looking_at(world(aim), Vec3::Y),
        ))
        .id();
    if live_reflection {
        app.world_mut().entity_mut(camera).insert((
            bevy::core_pipeline::prepass::DepthPrepass,
            bevy::pbr::ScreenSpaceTransmission {
                steps: 1,
                ..default()
            },
        ));
    }
    app.world_mut().spawn((
        DirectionalLight {
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(40.0, 80.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    app.world_mut().resource_mut::<FrameUniforms>().lighting = Vec4::new(1.0, 0.65, 0.55, 0.0);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let model = meteor_asset::gmdl::decode_gmdl(
        &std::fs::read(root.join(format!("{stage}/{stage}.gmb"))).unwrap(),
    )
    .unwrap();
    let des_bytes = std::fs::read(root.join(format!("{stage}/{stage}.des"))).unwrap();
    let (des_text, _, _) = encoding_rs::GBK.decode(&des_bytes);
    let des = meteor_asset::des::parse_stage_description(&des_text);
    let mut paks = pak_textures::PakCache::default();
    let mut texture_cache = HashMap::new();
    let mut normal_cache = HashMap::new();
    let mut water_cache = WaterCache::default();
    let mut water_count = 0;
    for object in &model.objects {
        let description = des.scene_objects.iter().find(|o| o.name == object.name);
        if description.is_some_and(|o| o.skip_render() || o.billboard != 0) {
            continue;
        }
        let mut groups: HashMap<i32, Vec<u32>> = HashMap::new();
        for (face, &material) in object.face_mat.iter().enumerate() {
            if material >= 0 {
                groups
                    .entry(material)
                    .or_default()
                    .extend_from_slice(&object.indices[face * 3..face * 3 + 3]);
            }
        }
        for (index, indices) in groups {
            let raw = &model.materials[index as usize];
            let (mode, opacity, cutoff) = render_plugin::classify_gmdl_material(raw);
            let name = model.resolve_texture_name(raw).unwrap_or("");
            let is_water = source(stage, &object.name, name).is_some();

            let lower_poison =
                stage == "sn10" && matches!(object.name.as_str(), "Virus01" | "Virus03");
            if mode != GmdlMode::Opaque && !is_water && !lower_poison {
                continue;
            }
            if name.to_ascii_lowercase().ends_with(".ifl") {
                continue;
            }
            let texture = if name.is_empty() {
                None
            } else {
                texture_cache
                    .entry(name.to_owned())
                    .or_insert_with(|| {
                        paks.resolve(&root, stage, name).and_then(|b| {
                            let companion = pak_textures::legacy_alpha_companion_name(name)
                                .and_then(|n| paks.resolve_exact(&root, stage, &n));
                            pak_textures::image_handle_with_legacy_style_quality(
                                &mut app.world_mut().resource_mut::<Assets<Image>>(),
                                &b,
                                companion.as_deref(),
                                raw.blend == "ALPHACHANNEL",
                                raw.blend == "COLORKEY",
                                0,
                            )
                        })
                    })
                    .clone()
            };
            let mut material = GmdlMaterial::new_scene(
                GmdlMaterialParams {
                    opacity,
                    alpha_cutoff: cutoff,
                    mode: mode.as_u32(),
                    has_texture: u32::from(texture.is_some()),
                    uv_offset: Vec2::ZERO,
                },
                texture,
                raw.two_side,
                mode,
            );
            let (mut mesh, transform) =
                render_plugin::build_scene_mesh(object, &indices, [0.0; 3], description);
            if let Some(normal) = scene_normals::load_scene_normal(
                stage,
                name,
                &mut app.world_mut().resource_mut::<Assets<Image>>(),
                &mut normal_cache,
                0,
            ) {
                if mesh.generate_tangents().is_ok() {
                    material.normal_texture = Some(normal);
                }
            }
            if is_water {
                let anim = &description.unwrap().texture_animation;
                material.uv_scroll = Vec4::new(anim.u, anim.v, 0.0, 0.0);
                assert!(water_cache.configure(
                    &mut material,
                    stage,
                    &object.name,
                    name,
                    [0.0; 3],
                    &mut app.world_mut().resource_mut::<Assets<Image>>(),
                    None,
                ));
                if stage == "sn13" {
                    assert_eq!(
                        material.water_optics.control.w, 1.0,
                        "sn13 uses deep optics"
                    );
                }
                material.water_live_reflection = live_reflection && material.water.x == 1.0;
                water_count += 1;
            }
            let wave = if stage == "sn13" && is_water {
                let (refined, mut wave) = super::waves::WaterWaveMesh::refine(&mesh, &indices)
                    .expect("audited sn13 water must refine within the vertex budget");
                wave.configure_shallow(
                    &material,
                    app.world().resource::<Assets<Image>>(),
                    &transform,
                );
                mesh = refined;
                Some(wave)
            } else {
                None
            };
            let material = app
                .world_mut()
                .resource_mut::<Assets<GmdlMaterial>>()
                .add(material);
            let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
            let entity = app
                .world_mut()
                .spawn((Mesh3d(mesh), MeshMaterial3d(material), transform))
                .id();
            if let Some(wave) = wave {
                app.world_mut().entity_mut(entity).insert(wave);
            }
        }
    }
    assert_eq!(water_count, if stage == "sn14" { 4 } else { 1 });
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
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    app.finish();
    app.cleanup();
    let off = capture(&mut app, &pixels);
    app.world_mut().resource_mut::<Settings>().water_enhance = true;
    let on = capture(&mut app, &pixels);
    if stage == "sn13" && !live_reflection {
        let water = app
            .world()
            .resource::<Assets<GmdlMaterial>>()
            .iter()
            .find(|(_, material)| material.water.x == 1.0)
            .unwrap()
            .0;
        let point = world(aim);
        let camera = world(eye);
        app.world_mut().resource_mut::<TestCameraFade>().0 = CameraOcclusionParams {
            eye: camera.extend(0.8),
            player: (point + (point - camera).normalize() * 4.0).extend(3.4),
            ..default()
        };
        let enhanced_with_tunnel = capture(&mut app, &pixels);
        app.world_mut()
            .resource_mut::<Assets<GmdlMaterial>>()
            .get_mut(water)
            .unwrap()
            .camera_occlusion_enabled = false;
        let enhanced_without_tunnel = capture(&mut app, &pixels);
        app.world_mut().resource_mut::<Settings>().water_enhance = false;
        let authored_without_tunnel = capture(&mut app, &pixels);
        app.world_mut()
            .resource_mut::<Assets<GmdlMaterial>>()
            .get_mut(water)
            .unwrap()
            .camera_occlusion_enabled = true;
        let authored_with_tunnel = capture(&mut app, &pixels);
        app.world_mut().resource_mut::<Settings>().water_enhance = true;
        app.world_mut().resource_mut::<TestCameraFade>().0 = default();
        let difference = |a: &[u8], b: &[u8]| {
            a.chunks_exact(4)
                .zip(b.chunks_exact(4))
                .filter(|(a, b)| a != b)
                .count()
        };
        let hole = difference(&authored_with_tunnel, &authored_without_tunnel);
        let regression = difference(&enhanced_with_tunnel, &enhanced_without_tunnel);
        eprintln!("sn13 camera fade: {hole} changed pixels on authored water, {regression} on enhanced water");
        assert!(hole > 100, "test camera tunnel must intersect the river");
        assert_eq!(
            regression, 0,
            "configured water must not be punched by camera fade"
        );
    }
    if live_reflection {
        let water_material = app
            .world()
            .resource::<Assets<GmdlMaterial>>()
            .iter()
            .find(|(_, material)| material.water_live_reflection)
            .map(|(id, _)| id)
            .expect("live sn13 water material");
        app.world_mut()
            .resource_mut::<Assets<GmdlMaterial>>()
            .get_mut(water_material)
            .unwrap()
            .water_live_reflection = false;
        let static_only = capture(&mut app, &pixels);
        app.world_mut()
            .resource_mut::<Assets<GmdlMaterial>>()
            .get_mut(water_material)
            .unwrap()
            .water_live_reflection = true;
        let live_pixels = on
            .chunks_exact(4)
            .zip(static_only.chunks_exact(4))
            .enumerate()
            .filter(|(pixel, (a, b))| pixel / 512 >= 145 && (0..3).any(|i| a[i].abs_diff(b[i]) > 2))
            .count();
        assert!(
            live_pixels > 1000,
            "original sn13 architecture must affect water reflection: {live_pixels}"
        );
        if let Ok(dir) = std::env::var("METEOR_WATER_CAPTURE_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            Image::new(
                Extent3d {
                    width: 512,
                    height: 288,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                static_only,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::MAIN_WORLD,
            )
            .try_into_dynamic()
            .unwrap()
            .save(std::path::Path::new(&dir).join("sn13-static-only.png"))
            .unwrap();
        }
    }
    if stage == "sn13" {
        let reflection = app
            .world()
            .resource::<Assets<GmdlMaterial>>()
            .iter()
            .find_map(|(_, material)| material.water_reflection.clone())
            .expect("sn13 has a baked architectural probe");
        let original_alpha = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            let mut image = images.get_mut(&reflection).unwrap();
            let data = image.data.as_mut().unwrap();
            let original: Vec<_> = data.chunks_exact(4).map(|pixel| pixel[3]).collect();
            for pixel in data.chunks_exact_mut(4) {
                pixel[3] = 0;
            }
            original
        };
        let without_reflection = capture(&mut app, &pixels);
        {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            let mut image = images.get_mut(&reflection).unwrap();
            let data = image.data.as_mut().unwrap();
            for (pixel, alpha) in data.chunks_exact_mut(4).zip(original_alpha) {
                pixel[3] = alpha;
            }
        }
        let reflected_pixels = on
            .chunks_exact(4)
            .zip(without_reflection.chunks_exact(4))
            .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 2))
            .count();
        assert!(
            reflected_pixels > 3000,
            "the actual scene probe must visibly contribute to the river: {reflected_pixels}"
        );
        let handle = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<&Mesh3d, With<super::waves::WaterWaveMesh>>();
            query.single(world).unwrap().0.clone()
        };
        let meshes = app.world().resource::<Assets<Mesh>>();
        let bevy::mesh::VertexAttributeValues::Float32x3(positions) = meshes
            .get(&handle)
            .unwrap()
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .unwrap()
        else {
            panic!("water positions");
        };
        let (lo, hi) = positions
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                (lo.min(p[2]), hi.max(p[2]))
            });
        assert!(
            (hi - lo) * MODEL_SCALE > 0.22,
            "sn13 surface must visibly rise and fall"
        );
    }
    assert_eq!(on.len(), 512 * 288 * 4);
    let changed = off
        .chunks_exact(4)
        .zip(on.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 2))
        .count();
    if let Ok(dir) = std::env::var("METEOR_WATER_CAPTURE_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in [("off", off.clone()), ("on", on.clone())] {
            let image = Image::new(
                Extent3d {
                    width: 512,
                    height: 288,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                bytes,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::MAIN_WORLD,
            );
            image
                .try_into_dynamic()
                .unwrap()
                .save(std::path::Path::new(&dir).join(format!("{stage}-{name}.png")))
                .unwrap();
        }
    }
    if stage == "sn13" {
        assert_eq!(
            &on[..512 * 90 * 4],
            &off[..512 * 90 * 4],
            "banks/bridge must remain unchanged"
        );

        let average = |frame: &[u8]| {
            let mut sum = 0.0_f64;
            for y in 190..280 {
                for x in 50..460 {
                    let pixel = &frame[(y * 512 + x) * 4..][..3];
                    sum += pixel.iter().map(|v| f64::from(*v)).sum::<f64>();
                }
            }
            sum / (3.0 * 90.0 * 410.0)
        };
        if !live_reflection {
            assert!(
                average(&on) < average(&off) * 0.95,
                "deep water must attenuate the bright riverbed"
            );
        }
    } else {
        let mut total = Vec3::ZERO;
        for (a, b) in off.chunks_exact(4).zip(on.chunks_exact(4)) {
            if (0..3).any(|i| a[i].abs_diff(b[i]) > 2) {
                total += Vec3::new(b[0] as f32, b[1] as f32, b[2] as f32);
            }
        }
        if stage == "sn08" || stage == "sn10" {
            assert!(
                total.y > total.x * 1.15 && total.y > total.z * 1.15,
                "poison must stay green"
            );
        }
        if stage == "sn14" {
            assert!(
                total.x > total.y * 1.15 && total.x > total.z * 1.15,
                "lava must stay orange/red"
            );
        }
    }
    assert!(
        changed > 1000,
        "real water fixture must visibly change: {changed}"
    );
    if stage == "sn10" || stage == "sn13" {
        app.world_mut().resource_mut::<FrameUniforms>().seconds = 2.0;
        let animated = capture(&mut app, &pixels);
        let moving = animated
            .chunks_exact(4)
            .zip(on.chunks_exact(4))
            .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 2))
            .count();
        assert!(
            moving > 1000,
            "{stage} liquid surface must animate: {moving}"
        );
        app.world_mut().resource_mut::<FrameUniforms>().seconds = 0.0;
    }
    app.world_mut().resource_mut::<Settings>().water_enhance = false;
    assert_eq!(
        capture(&mut app, &pixels),
        off,
        "real scene toggle must fully restore"
    );
    eprintln!("{stage} original-material fixture: {changed} changed pixels");
}

#[test]
fn headless_sn13_water_with_original_riverbed() {
    render_original_liquid(
        "sn13",
        Vec3::new(-190.0, -820.0, -140.0),
        Vec3::new(-190.0, -500.0, -186.0),
        false,
    );
}
#[test]
fn headless_sn13_current_frame_wall_reflection() {
    render_original_liquid(
        "sn13",
        Vec3::new(-190.0, -820.0, -140.0),
        Vec3::new(-190.0, -500.0, -186.0),
        true,
    );
}
#[test]
fn headless_sn13_water_near_surface_profile() {
    render_original_liquid(
        "sn13",
        Vec3::new(-190.0, -820.0, -169.0),
        Vec3::new(-190.0, -500.0, -186.0),
        false,
    );
}
#[test]
fn headless_sn08_poison_keeps_green_surface() {
    render_original_liquid(
        "sn08",
        Vec3::new(-380.0, -420.0, 50.0),
        Vec3::new(-500.0, -480.0, -15.0),
        false,
    );
}
#[test]
fn headless_sn10_upper_poison_preserves_additive_layers() {
    render_original_liquid(
        "sn10",
        Vec3::new(0.0, -300.0, 40.0),
        Vec3::new(0.0, -180.0, -40.0),
        false,
    );
}
#[test]
fn headless_sn14_lava_preserves_orange_layers() {
    render_original_liquid(
        "sn14",
        Vec3::new(-1887.0, 510.0, 80.0),
        Vec3::new(-1887.0, 579.0, -120.0),
        false,
    );
}
