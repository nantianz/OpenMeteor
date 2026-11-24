use crate::{
    app_state::{AppState, LoadedStageAssets, LoadedStageInfo, Settings},
    plugins::{
        camera_plugin::GameplayCamera,
        escort_assets::LoadedEscort,
        hazard_plugin::StageHazardRuntime,
        render_plugin::{setup_stage_scene, MapPreviewScene, PlayingSceneEntity, StageCollision},
        stage_collider::{StageCollider, StageMeshCollider},
        stage_material::GmdlMaterialPlugin,
        stage_plugin::StageScriptRuntime,
    },
};
use bevy::{
    app::PluginsState,
    asset::RenderAssetUsages,
    camera::RenderTarget,
    ecs::system::RunSystemOnce,
    prelude::*,
    render::{
        gpu_readback::{Readback, ReadbackComplete},
        render_resource::{Extent3d, PollType, TextureDimension, TextureFormat, TextureUsages},
        renderer::RenderDevice,
        RenderPlugin,
    },
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const WIDTH: u32 = 768;
const HEIGHT: u32 = 336;

pub(crate) fn bake() -> Result<(), String> {
    let mut stages = super::desktop::library().0;
    if let Ok(wanted) = std::env::var("METEOR_STAGE_PREVIEW_STAGE") {
        stages.retain(|id, _| *id == wanted);
    }
    stages.retain(|_, path| !path.is_file());
    if stages.is_empty() {
        return Ok(());
    }
    for output in stages.values() {
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
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
    .insert_resource(MapPreviewScene)
    .insert_resource(Settings::default())
    .init_resource::<LoadedEscort>()
    .init_resource::<StageCollision>()
    .init_resource::<StageCollider>()
    .init_resource::<StageHazardRuntime>()
    .init_resource::<StageScriptRuntime>();
    crate::plugins::frame_work::install(&mut app);

    let mut image = Image::new_target_texture(WIDTH, HEIGHT, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera {
                clear_color: ClearColorConfig::Custom(Color::srgb(0.070, 0.078, 0.084)),
                ..default()
            },
            GameplayCamera,
            Projection::Perspective(PerspectiveProjection {
                fov: 43_f32.to_radians(),
                near: 0.15,
                ..default()
            }),
            Msaa::Sample4,
            RenderTarget::Image(target.clone().into()),
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            Transform::default(),
        ))
        .id();
    let readback = Arc::new(Mutex::new(Vec::new()));
    let buffer = readback.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |event: On<ReadbackComplete>| {
            *buffer.lock().unwrap() = event.data.clone();
        });
    let deadline = Instant::now() + Duration::from_secs(45);
    while app.plugins_state() != PluginsState::Ready {
        assert!(
            Instant::now() < deadline,
            "render plugins did not initialize"
        );
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(Duration::from_millis(2));
    }
    app.finish();
    app.cleanup();

    let mut failed = Vec::new();
    for (stage, output) in stages {
        let read = |ext| crate::resource_io::read(format!("{stage}/{stage}.{ext}")).ok();
        let (Some(gmb), Some(des)) = (read("gmb"), read("des")) else {
            failed.push(stage);
            continue;
        };
        let route = crate::resource_io::read(format!("Level/{stage}.wp"))
            .ok()
            .map(|bytes| crate::text_codec::decode_text(&bytes));
        let center = meteor_scene::stage_loader::get_stage_center(&stage, route.as_deref());
        let route = meteor_scene::stage_loader::parse_waypoint_graph(
            route.as_deref().unwrap_or_default(),
            center,
        );
        let anchors: Vec<Vec3> = route
            .points
            .iter()
            .map(|point| {
                let [x, y, z] = point.position;
                Vec3::new(x - center[0], z - center[2], center[1] - y)
                    * meteor_core::constants::MODEL_SCALE
            })
            .filter(|p| p.is_finite())
            .collect();
        app.world_mut().insert_resource(LoadedStageInfo {
            stage_id: stage.clone(),
            center,
            ..default()
        });
        app.world_mut().insert_resource(LoadedStageAssets {
            gmb: Some(gmb),
            des: Some(des),
            cob: read("cob"),
            ..default()
        });
        app.world_mut().run_system_once(setup_stage_scene).unwrap();
        if let Some(error) = meteor_asset::runtime_assets::take_original_read_error() {
            return Err(format!("{stage}: {error}"));
        }

        let shots = app
            .world()
            .resource::<StageCollider>()
            .mesh
            .as_ref()
            .map(|mesh| camera_candidates(mesh, &anchors))
            .unwrap_or_default();
        let mut best = None;
        for shot in shots.into_iter().take(3) {
            *app.world_mut().get_mut::<Transform>(camera).unwrap() = shot;

            for _ in 0..10 {
                app.update();
                app.world()
                    .resource::<RenderDevice>()
                    .poll(PollType::Poll)
                    .map_err(|e| e.to_string())?;
                std::thread::sleep(Duration::from_millis(4));
            }
            let pixels = readback.lock().unwrap().clone();
            if pixels.len() != (WIDTH * HEIGHT * 4) as usize {
                continue;
            }
            let score = composition_score(&pixels);
            if best.as_ref().is_none_or(|(previous, _)| score > *previous) {
                best = Some((score, pixels));
            }
        }
        if let Some((_, pixels)) = best.filter(|(score, _)| *score > 0) {
            let temp = output.with_extension(format!("{}.tmp.jpg", std::process::id()));
            let image = Image::new(
                Extent3d {
                    width: WIDTH,
                    height: HEIGHT,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                pixels,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::MAIN_WORLD,
            )
            .try_into_dynamic()
            .map_err(|e| e.to_string())?;
            let saved = image
                .into_rgb8()
                .save(&temp)
                .map_err(|e| e.to_string())
                .and_then(|_| std::fs::rename(&temp, &output).map_err(|e| e.to_string()));
            if let Err(error) = saved {
                let _ = std::fs::remove_file(&temp);
                return Err(error);
            }
            println!("baked map preview {stage}");
        } else {
            failed.push(stage);
        }
        meteor_asset::runtime_assets::retain_stage("");
        let entities = app
            .world_mut()
            .query_filtered::<Entity, With<PlayingSceneEntity>>()
            .iter(app.world())
            .collect::<Vec<_>>();
        for entity in entities {
            app.world_mut().despawn(entity);
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("no usable preview for {}", failed.join(", ")))
    }
}

fn composition_score(pixels: &[u8]) -> u32 {
    let row = WIDTH as usize * 4;

    pixels
        .chunks_exact(row)
        .skip(HEIGHT as usize / 6)
        .take(HEIGHT as usize * 2 / 3)
        .map(|line| {
            line.chunks_exact(4)
                .skip(WIDTH as usize / 8)
                .take(WIDTH as usize * 3 / 4)
                .step_by(4)
                .zip(
                    line[4..]
                        .chunks_exact(4)
                        .skip(WIDTH as usize / 8)
                        .step_by(4),
                )
                .map(|(a, b)| u32::from(a[0].abs_diff(b[0]).max(a[1].abs_diff(b[1])).min(12)))
                .sum::<u32>()
        })
        .sum()
}

fn camera_candidates(mesh: &StageMeshCollider, route: &[Vec3]) -> Vec<Transform> {
    use meteor_physics::collision::{MeshCollider, Point3};
    let point = |p: Vec3| Point3::new(p.x, p.y, p.z);
    let ray = |start, dir, length| {
        mesh.raycast_hit(point(start), point(dir), length)
            .map(|hit| hit.t)
    };

    let floors: Vec<Vec3> = mesh
        .floor_tris
        .iter()
        .filter_map(|&tri| {
            let index = tri as usize * 3;
            let vertices = mesh.indices.get(index..index + 3)?;
            let a = Vec3::from_array(*mesh.positions.get(vertices[0] as usize)?);
            let b = Vec3::from_array(*mesh.positions.get(vertices[1] as usize)?);
            let c = Vec3::from_array(*mesh.positions.get(vertices[2] as usize)?);
            Some((a + b + c) / 3.0).filter(|p| p.is_finite())
        })
        .collect();
    if floors.is_empty() && route.is_empty() {
        return Vec::new();
    }

    let domain: Vec<Vec3> = floors.iter().chain(route).copied().collect();
    let limits = |axis: usize| {
        let mut values: Vec<f32> = domain.iter().map(|p| p[axis]).collect();
        values.sort_unstable_by(f32::total_cmp);
        let trim = (values.len() / 40).min(values.len().saturating_sub(2) / 2);
        [values[trim], values[values.len() - 1 - trim]]
    };
    let [left, right] = limits(0);
    let [front, rear] = limits(2);
    let span = Vec2::new(right - left, rear - front).length().max(4.0);
    let heights = if route.is_empty() { &floors[..] } else { route };
    let mut height: Vec<f32> = heights.iter().map(|p| p.y).collect();
    height.sort_unstable_by(f32::total_cmp);
    let target = Vec3::new(
        (left + right) * 0.5,
        height[height.len() / 2],
        (front + rear) * 0.5,
    );
    let mut shots = Vec::new();
    for edge in [
        Vec2::new(left, front),
        Vec2::new(right, front),
        Vec2::new(left, rear),
        Vec2::new(right, rear),
        Vec2::new(left, target.z),
        Vec2::new(right, target.z),
        Vec2::new(target.x, front),
        Vec2::new(target.x, rear),
    ] {
        for inset in [0.10, 0.24, 0.38] {
            let xz = edge.lerp(Vec2::new(target.x, target.z), inset);
            let Some(nearest) = heights.iter().min_by(|a, b| {
                Vec2::new(a.x, a.z)
                    .distance_squared(xz)
                    .total_cmp(&Vec2::new(b.x, b.z).distance_squared(xz))
            }) else {
                continue;
            };
            let base = Vec3::new(xz.x, nearest.y + 0.5, xz.y);
            let ceiling = ray(base, Vec3::Y, span).map_or(span, |h| (h - 0.5).max(0.0));
            for rise in [0.34, 0.54, 0.78] {
                let eye = base + Vec3::Y * ((span * rise).min(ceiling));
                if eye.y <= target.y + 0.6 {
                    continue;
                }
                let dir = (target - eye).normalize_or_zero();
                if dir.length_squared() < 0.5 {
                    continue;
                }
                let side = dir.cross(Vec3::Y).normalize_or_zero();
                if side.length_squared() < 0.5 {
                    continue;
                }
                let up = side.cross(dir);
                let focus = eye.distance(target);
                let aim_clearance = ray(eye, dir, focus).unwrap_or(focus) / focus;
                let mut coverage = 0.0;
                let mut near = 0.0;
                for x in [-0.72, -0.36, 0.0, 0.36, 0.72] {
                    for y in [-0.30, 0.0, 0.30] {
                        let direction = (dir + side * x + up * y).normalize_or_zero();
                        if let Some(t) = ray(eye, direction, span * 2.4) {
                            coverage += 1.0;
                            if t < focus * 0.28 {
                                near += 1.0;
                            }
                        }
                    }
                }
                let pitch = (-dir.y - 0.62).abs();
                let score = coverage * 0.9 - near * 1.2 + aim_clearance * 6.0 - pitch * 5.0;
                shots.push((
                    score,
                    Transform::from_translation(eye).looking_at(target, Vec3::Y),
                ));
            }
        }
    }
    shots.sort_by(|(a, _), (b, _)| b.total_cmp(a));
    let mut chosen: Vec<Transform> = Vec::new();
    for (_, shot) in shots {
        if chosen
            .iter()
            .all(|other| other.translation.distance(shot.translation) > span * 0.10)
        {
            chosen.push(shot);
        }
        if chosen.len() == 3 {
            break;
        }
    }
    chosen
}
