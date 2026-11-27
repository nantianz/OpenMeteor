use super::*;
use crate::app_state::Settings;
use crate::plugins::{
    camera_plugin::GameplayCamera,
    stage_material::water::interaction::{LiquidSurface, UnderwaterView},
};
use bevy::{
    asset::RenderAssetUsages,
    core_pipeline::prepass::DepthPrepass,
    pbr::ScreenSpaceTransmission,
    render::render_resource::{Extent3d, TextureDimension},
    window::{PrimaryWindow, WindowResolution},
};
use std::path::Path;

const REFLECTION_EYE: Vec3 = Vec3::new(-380.0, -1600.0, -154.0);
const REFLECTION_HEADING: Vec3 = Vec3::new(0.04, 1.0, -0.14);
const REFLECTION_FOV: f32 = std::f32::consts::FRAC_PI_3;

fn set_reflection_fov(app: &mut App, camera: Entity) {
    if let Projection::Perspective(projection) = app
        .world_mut()
        .get_mut::<Projection>(camera)
        .unwrap()
        .as_mut()
    {
        projection.fov = REFLECTION_FOV;
    }
}

pub(super) fn capture(
    app: &mut App,
    output: &Path,
    camera: Entity,
    target: &Handle<Image>,
    to_world: impl Fn(Vec3) -> Vec3,
    width: u32,
    height: u32,
) {
    std::fs::create_dir_all(output).unwrap();
    app.world_mut().spawn((
        Window {
            resolution: WindowResolution::new(width, height),
            ..default()
        },
        PrimaryWindow,
    ));
    app.world_mut().entity_mut(camera).insert((
        GameplayCamera,
        ScreenSpaceTransmission {
            steps: 2,
            ..default()
        },
    ));
    if std::env::var_os("METEOR_SN13_RETURN_PROBE").is_some() {
        probe_repeated_returns(app, camera, to_world);
        return;
    }
    if std::env::var_os("METEOR_SN13_RENDER_BENCH").is_some() {
        benchmark_render_paths(app, output, camera, target, to_world);
        return;
    }
    app.world_mut().resource_mut::<Time<Virtual>>().pause();
    let readback = Arc::new(Mutex::new(Vec::new()));
    let pixels = readback.clone();
    let observer = app
        .world_mut()
        .spawn(Readback::texture(target.clone()))
        .observe(move |event: On<ReadbackComplete>| *pixels.lock().unwrap() = event.data.clone())
        .id();

    load_sn13_variant(app, true);
    let mut query = app
        .world_mut()
        .query::<(&LiquidSurface, &MeshMaterial3d<GmdlMaterial>)>();
    let (surface, material) = query
        .single(app.world())
        .expect("one authored liquid batch");
    assert!(surface.is_clear_water());
    let material = material.0.clone();
    assert!(
        app.world()
            .resource::<Assets<GmdlMaterial>>()
            .get(&material)
            .unwrap()
            .water_sky_reflection
    );
    let save = |name: &str, suffix: &str, data: Vec<u8>| {
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
        .save(output.join(format!("{name}-{suffix}.png")))
        .unwrap();
    };

    for (name, position, heading) in [
        ("city-bank", [-190.0, -820.0, -140.0], [0.0, 1.0, 0.08]),
        (
            "outer-channel",
            [-520.0, -1700.0, -147.0],
            [0.48, 0.86, 0.11],
        ),
        ("east-basin", [710.0, -520.0, -140.0], [-0.90, -0.43, 0.12]),
        (
            "river-reflection",
            REFLECTION_EYE.to_array(),
            REFLECTION_HEADING.to_array(),
        ),
        (
            "basin-reflection",
            [708.0, -520.0, -151.0],
            [-0.90, -0.43, -0.10],
        ),
    ] {
        if name == "river-reflection" {
            set_reflection_fov(app, camera);
        }
        let eye = Vec3::from_array(position);
        *app.world_mut().get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(to_world(eye))
                .looking_at(to_world(eye + Vec3::from_array(heading)), Vec3::Y);
        app.world_mut().resource_mut::<Settings>().water_enhance = false;
        let stock = capture_size(app, &readback, width, height);
        assert!(sky::cache_ready(app));

        app.world_mut().resource_mut::<Settings>().water_enhance = true;
        let live = capture_size(app, &readback, width, height);
        assert_eq!(
            *app.world().resource::<UnderwaterView>(),
            UnderwaterView::default()
        );
        assert!(app.world().get::<DepthPrepass>(camera).is_some());
        app.world_mut()
            .resource_mut::<Assets<GmdlMaterial>>()
            .get_mut(&material)
            .unwrap()
            .water_sky_reflection = false;
        let without_clouds = capture_size(app, &readback, width, height);
        {
            let mut assets = app.world_mut().resource_mut::<Assets<GmdlMaterial>>();
            let mut water = assets.get_mut(&material).unwrap();
            assert!(water.water_live_reflection);
            assert!(
                !water.water_screen_refraction,
                "classic shading needs no refraction pass"
            );
            water.water_live_reflection = false;
            water.water_sky_reflection = true;
        }
        let static_only = capture_size(app, &readback, width, height);
        let upper_rows = if name.ends_with("-reflection") {
            16
        } else {
            height / 3
        };
        let upper_bytes = (width * upper_rows * 4) as usize;
        let sky_delta = changed(&stock[..upper_bytes], &live[..upper_bytes]);
        let water_delta = changed(&stock, &live);
        let reflection_delta = changed(&static_only, &live);
        let cloud_delta = changed(&without_clouds, &live);
        let visible_cloud_delta = without_clouds
            .chunks_exact(4)
            .zip(live.chunks_exact(4))
            .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) >= 18))
            .count();
        eprintln!("sn13 {name}: {sky_delta} upper pixels, {water_delta} enhancement pixels, {reflection_delta} live reflection pixels, {cloud_delta} sky reflection pixels ({visible_cloud_delta} clearly changed)");
        save(name, "original-water", stock);
        save(name, "refraction", static_only);
        assert_eq!(&without_clouds[..upper_bytes], &live[..upper_bytes]);
        save(name, "no-sky-reflection", without_clouds);
        save(name, "reflection", live);
        if name == "river-reflection" {
            assert!(
                visible_cloud_delta > 10_000,
                "the reflected sky must read at screenshot scale"
            );
        }
        if name == "city-bank" {
            assert_eq!(cloud_delta, 0, "bridge underside stays occluded");
        }
        assert_eq!(sky_delta, 0, "water must leave the upward rays unchanged");
        assert!(
            water_delta > 1000,
            "the view must actually see the water surface"
        );
    }

    let eye = Vec3::new(-190.0, -820.0, -196.0);
    *app.world_mut().get_mut::<Transform>(camera).unwrap() =
        Transform::from_translation(to_world(eye))
            .looking_at(to_world(eye + Vec3::new(0.0, 1.0, 0.16)), Vec3::Y);
    let underwater = capture_size(app, &readback, width, height);
    let submersion = *app.world().resource::<UnderwaterView>();
    assert_eq!(submersion.kind, 1.0);
    eprintln!("sn13 underwater control: depth {:.2}", submersion.depth);
    save("underwater", "effect", underwater);
    app.world_mut().despawn(observer);

    if std::env::var_os("METEOR_SN13_WATER_SKY_BENCH").is_some() {
        benchmark(app, camera, target, &material, to_world);
    }
}

fn probe_repeated_returns(app: &mut App, camera: Entity, to_world: impl Fn(Vec3) -> Vec3) {
    use super::sky_benchmark::{summarize, wait};
    use crate::components::fighter::Fighter;
    use std::time::{Duration, Instant};
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_secs_f64(1.0 / 120.0),
    ));
    set_reflection_fov(app, camera);
    app.world_mut().entity_mut(camera).insert(Msaa::Sample4);
    load_sn13_variant(app, true);
    let spawn = Vec3::new(-104.507484, 3.3983998, 225.19263);
    let street_eye = spawn + Vec3::new(0.0, 6.5, 9.8);
    let street_view =
        Transform::from_translation(street_eye).looking_at(spawn + Vec3::Y * 2.0, Vec3::Y);
    let river = Vec3::new(-190.0, -820.0, -140.0);
    let river_view = Transform::from_translation(to_world(river))
        .looking_at(to_world(river + Vec3::new(0.0, 1.0, 0.08)), Vec3::Y);
    let actor = app
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
            Transform::IDENTITY,
        ))
        .id();
    let mut seconds = 0.0;
    for cycle in 0..5 {
        for (view, count, name) in [(street_view, 150, "street"), (river_view, 300, "river")] {
            if cycle == 4 && name == "river" {
                break;
            }
            *app.world_mut().get_mut::<Transform>(camera).unwrap() = view;
            let mut elapsed = Vec::new();
            let mut cpu = Vec::new();
            let mut contact_frames = 0;
            for frame in 0..count {
                let p = if name == "river" {
                    let t = ((frame % 60) as f32 / 60.0) * std::f32::consts::TAU;
                    let start = Vec3::new(
                        -190.0 + 65.0 * cycle as f32,
                        -820.0 - 110.0 * cycle as f32,
                        -187.0,
                    );
                    to_world(
                        start
                            + Vec3::new(11.0 * t.sin(), 11.0 * t.cos(), (t.sin() * 17.0).max(-8.0)),
                    )
                } else {
                    spawn
                };
                app.world_mut()
                    .get_mut::<Transform>(actor)
                    .unwrap()
                    .translation = p;
                seconds += 1.0 / 120.0;
                app.world_mut().resource_mut::<FrameUniforms>().seconds = seconds;
                let begin = Instant::now();
                app.update();
                let update_ms = begin.elapsed().as_secs_f64() * 1000.0;
                wait(app.world().resource::<RenderDevice>());
                if frame >= count - 60 {
                    elapsed.push(begin.elapsed().as_secs_f64() * 1000.0);
                    cpu.push(update_ms);
                }
                contact_frames += u32::from(
                    app.world()
                        .resource::<FrameUniforms>()
                        .liquid_contacts
                        .counts
                        .x
                        > 0.0,
                );
            }
            let device = app.world().resource::<RenderDevice>().wgpu_device();
            let allocation = device.generate_allocator_report().map(|report| {
                (
                    report.allocations.len(),
                    report.allocations.iter().map(|a| a.size).sum::<u64>(),
                )
            });
            let phase = app.world().get::<DepthPrepass>(camera).is_some();
            eprintln!("SN13 RETURN {cycle} {name}: {}, cpu {}, contacts {contact_frames}, depth {phase}, allocations {allocation:?}",
                summarize(&mut elapsed), summarize(&mut cpu));
            if name == "river" {
                assert!(
                    contact_frames > count / 2,
                    "fixture must produce recurring water contacts"
                );
            }
        }
    }
}
fn benchmark_render_paths(
    app: &mut App,
    output: &Path,
    camera: Entity,
    target: &Handle<Image>,
    to_world: impl Fn(Vec3) -> Vec3,
) {
    use super::sky_benchmark::{summarize, wait};
    use crate::plugins::scene_render_optimizations;
    use crate::plugins::stage_plugin::LocalFighter;
    use std::time::{Duration, Instant};
    let (width, height) = if std::env::var_os("METEOR_SN13_ORIGINAL_DIR").is_some() {
        (2560, 1494)
    } else {
        (1920, 1080)
    };
    let size = Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_secs_f64(1.0 / 150.0),
    ));
    app.world_mut().entity_mut(camera).insert(Msaa::Sample4);
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .get_mut::<Window>(window)
        .unwrap()
        .resolution = WindowResolution::new(width, height);
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .get_mut(target)
        .unwrap()
        .resize(size);
    set_reflection_fov(app, camera);
    load_sn13_variant(app, true);
    {
        let mut settings = app.world_mut().resource_mut::<Settings>();
        settings.dynamic_shadows = true;
        settings.environment_lighting = true;
        settings.volumetric_fog = true;
    }
    let actor_mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(0.8, 2.0, 0.8));
    let actor_material = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(Color::srgb(0.7, 0.15, 0.05));
    let actor = app
        .world_mut()
        .spawn((
            LocalFighter,
            Mesh3d(actor_mesh),
            MeshMaterial3d(actor_material),
        ))
        .id();
    let street = Vec3::new(-794.0, -178.0, 63.0);
    let street_eye = to_world(street);
    let street_target = to_world(street + Vec3::new(-0.58, 0.81, -0.13));
    let river_eye = Vec3::new(20.0, -17.0, 55.0);
    let river_target = Vec3::new(12.9, -22.0, 60.7);
    let views = [
        ("street", street_eye, street_target, false),
        ("walking", street_eye, street_target, true),
        ("river", river_eye, river_target, false),
    ];
    for (name, eye, aim, moving) in views {
        let transform = Transform::from_translation(eye).looking_at(aim, Vec3::Y);
        *app.world_mut().get_mut::<Transform>(camera).unwrap() = transform;
        let start = eye + (aim - eye).normalize() * 4.0 - Vec3::Y * 1.9;
        let update = |app: &mut App, frame| {
            let position = start
                + Vec3::X
                    * if moving {
                        f32::sin(frame as f32 / 50.0) * 8.0
                    } else {
                        0.0
                    };
            app.world_mut()
                .get_mut::<Transform>(actor)
                .unwrap()
                .translation = position;
            app.world_mut().resource_mut::<FrameUniforms>().seconds = f64::from(frame) / 150.0;
            app.update();
        };
        let mut serial = Vec::new();
        let mut cpu = Vec::new();
        for frame in 0..150 {
            let begin = Instant::now();
            update(app, frame);
            let update_ms = begin.elapsed().as_secs_f64() * 1000.0;
            wait(app.world().resource::<RenderDevice>());
            if frame >= 60 {
                serial.push(begin.elapsed().as_secs_f64() * 1000.0);
                cpu.push(update_ms);
            }
        }
        assert_eq!(
            app.world().get::<DepthPrepass>(camera).is_some(),
            name == "river"
        );
        let options = scene_render_optimizations::options("sn13");
        if options.shadows && name == "street" {
            let world = app.sub_app_mut(bevy::render::RenderApp).world_mut();
            let mut cache = world.query::<&bevy::pbr::ShadowCacheTarget>();
            assert!(cache
                .iter(world)
                .any(bevy::pbr::ShadowCacheTarget::has_cached_depth));
        }
        let hierarchy = app
            .world()
            .resource::<GraphicsPacks>()
            .water()
            .is_some_and(|water| water.depth_hierarchy());
        if hierarchy && name == "river" {
            use crate::plugins::stage_material::water::depth_hierarchy::TEST_DISPATCHES;
            assert!(TEST_DISPATCHES.load(std::sync::atomic::Ordering::Relaxed) > 60);
        }
        if options.occlusion {
            assert_eq!(
                app.world()
                    .get::<bevy::render::occlusion_culling::OcclusionCulling>(camera)
                    .is_some(),
                name == "river"
            );
        }
        eprintln!(
            "SN13 RENDER {name}: serial {}, cpu {}",
            summarize(&mut serial),
            summarize(&mut cpu)
        );
        let pixels = Arc::new(Mutex::new(Vec::new()));
        let output_pixels = pixels.clone();
        let reader = app
            .world_mut()
            .spawn(Readback::texture(target.clone()))
            .observe(move |e: On<ReadbackComplete>| *output_pixels.lock().unwrap() = e.data.clone())
            .id();
        let data = capture_size(app, &pixels, width, height);
        app.world_mut().despawn(reader);
        Image::new(
            size,
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD,
        )
        .try_into_dynamic()
        .unwrap()
        .save(output.join(format!("{name}.png")))
        .unwrap();
    }
}
fn benchmark(
    app: &mut App,
    camera: Entity,
    target: &Handle<Image>,
    material: &Handle<GmdlMaterial>,
    to_world: impl Fn(Vec3) -> Vec3,
) {
    use super::sky_benchmark::{summarize, wait};
    use std::time::Instant;
    set_reflection_fov(app, camera);
    *app.world_mut().get_mut::<Transform>(camera).unwrap() =
        Transform::from_translation(to_world(REFLECTION_EYE))
            .looking_at(to_world(REFLECTION_EYE + REFLECTION_HEADING), Vec3::Y);
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .get_mut(target)
        .unwrap()
        .resize(Extent3d {
            width: 2560,
            height: 1440,
            depth_or_array_layers: 1,
        });
    let mut times = [Vec::new(), Vec::new()];
    for enabled in [false, true, true, false] {
        {
            let mut materials = app.world_mut().resource_mut::<Assets<GmdlMaterial>>();
            let mut water = materials.get_mut(material).unwrap();
            water.water_sky_reflection = enabled;
            water.water_live_reflection = false;
        }
        for i in 0..48 {
            let start = Instant::now();
            app.update();
            wait(app.world().resource::<RenderDevice>());
            if i >= 16 {
                times[usize::from(enabled)].push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    for (enabled, samples) in times.iter_mut().enumerate() {
        eprintln!(
            "sn13 1440p sky reflection={enabled}: CPU+GPU {}",
            summarize(samples)
        );
    }
}
