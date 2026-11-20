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
            assert!(water.water_live_reflection && water.water_screen_refraction);
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
