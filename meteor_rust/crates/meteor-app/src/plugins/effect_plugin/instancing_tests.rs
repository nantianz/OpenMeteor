use super::*;
use bevy::{
    app::PluginsState,
    diagnostic::DiagnosticsStore,
    render::{
        gpu_readback::{Readback, ReadbackComplete},
        render_phase::ViewSortedRenderPhases,
        render_resource::{PollType, TextureFormat, TextureUsages},
        renderer::RenderDevice,
        Render, RenderApp, RenderSystems,
    },
};
use std::sync::{Arc, Mutex};

#[derive(Resource, Clone, Default)]
struct BatchCounts(
    Arc<Mutex<(usize, usize, usize)>>,
    Arc<std::sync::atomic::AtomicUsize>,
);

fn capture_batches(
    phases: Res<ViewSortedRenderPhases<bevy::core_pipeline::core_3d::Transparent3d>>,
    counts: Res<BatchCounts>,
    pages: Res<dynamic_uniforms::ParamBuffers>,
    support: Res<bevy::render::batching::gpu_preprocessing::GpuPreprocessingSupport>,
) {
    assert!(
        !support.is_available(),
        "CPU mesh batches must remain selected after RenderStartup capability detection"
    );
    counts.1.store(
        pages.particle_page_count(),
        std::sync::atomic::Ordering::Relaxed,
    );
    let mut n = (0, 0, 0);
    for phase in phases.values() {
        let mut index = 0;
        while let Some(item) = phase.items.get_index(index).map(|(_, item)| item) {
            let instances = item.batch_range.len();
            if instances == 0 {
                index += 1;
                continue;
            }
            n.0 += instances;
            n.1 += 1;
            n.2 = n.2.max(instances);
            index += instances;
        }
    }
    *counts.0.lock().unwrap() = n;
}

#[test]
fn uniform_only_particles_really_instance_and_keep_independent_colors() {
    const CHILD: &str = "METEOR_PARTICLE_GPU_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "plugins::effect_plugin::instancing_tests::uniform_only_particles_really_instance_and_keep_independent_colors", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        eprintln!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.success(),
            "isolated uniform-buffer GPU test failed"
        );
        return;
    }
    let mut app = App::new();
    app.add_plugins(
        crate::android_render_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: None,
            exit_condition: bevy::window::ExitCondition::DontExit,
            ..default()
        }))
        .disable::<bevy::winit::WinitPlugin>()
        .disable::<bevy::audio::AudioPlugin>(),
    )
    .add_plugins(MaterialPlugin::<PonEfMaterial>::default())
    .init_resource::<DiagnosticsStore>();
    dynamic_uniforms::install(&mut app);
    crate::plugins::frame_work::install(&mut app);
    crate::plugins::frame_work::serial_post_update(&mut app);
    for (handle, source, path) in [
        (
            PON_EF_SHADER_HANDLE,
            include_str!("../pon_ef_material.wgsl"),
            "pon_ef_material.wgsl",
        ),
        (
            PON_EF_PARTICLE_SHADER_HANDLE,
            include_str!("../pon_ef_particle.wgsl"),
            "pon_ef_particle.wgsl",
        ),
    ] {
        app.world_mut()
            .resource_mut::<Assets<Shader>>()
            .insert(handle.id(), Shader::from_wgsl(source, path))
            .unwrap();
    }
    let counts = BatchCounts::default();
    app.sub_app_mut(RenderApp)
        .insert_resource(counts.clone())
        .add_systems(
            Render,
            capture_batches
                .after(RenderSystems::Prepare)
                .before(RenderSystems::Render),
        );
    let mut image = Image::new_target_texture(260, 8, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().spawn((
        Camera3d::default(),
        Msaa::Off,
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::linear_rgb(0.0, 0.0, 1.0)),
            ..default()
        },
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: bevy::camera::ScalingMode::Fixed {
                width: 130.0,
                height: 4.0,
            },
            ..OrthographicProjection::default_3d()
        }),
        bevy::camera::RenderTarget::Image(target.clone().into()),
        Transform::from_xyz(0.0, 0.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Rectangle::new(1.0, 4.0));
    let mut page = Handle::default();
    let mut first_page = None;
    let mut entities = Vec::new();
    for i in 0..130 {
        if i % 128 == 0 {
            page = app
                .world_mut()
                .resource_mut::<Assets<PonEfMaterial>>()
                .add(PonEfMaterial {
                    params: PonEfMaterialParams {
                        color: Vec4::ONE,
                        has_texture: 0,
                    },
                    texture: None,
                    blend: PonEfBlendMode::SrcAlphaInvSrcAlpha,
                    two_sided: true,
                    uniform_id: dynamic_uniforms::new_uniform_id(),
                    particle_colors: Some(Box::new(std::array::from_fn(|slot| {
                        if slot % 2 == 0 {
                            Vec4::new(1.0, 0.0, 0.0, 0.5)
                        } else {
                            Vec4::new(0.0, 1.0, 0.0, 0.5)
                        }
                    }))),
                });
            if first_page.is_none() {
                first_page = Some(page.id());
            }
        }
        entities.push(
            app.world_mut()
                .spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(page.clone()),
                    bevy::mesh::MeshTag(i % 128),
                    Transform::from_xyz(i as f32 - 64.5, 0.0, 0.0),
                ))
                .id(),
        );
    }
    let pixels = Arc::new(Mutex::new(Vec::new()));
    let captured = pixels.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |event: On<ReadbackComplete>| {
            *captured.lock().unwrap() = event.data.clone();
        });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    app.finish();
    assert!(matches!(
        app.sub_app(RenderApp)
            .world()
            .resource::<bevy::pbr::RenderMeshInstances>(),
        bevy::pbr::RenderMeshInstances::CpuBuilding(_)
    ));
    app.cleanup();
    assert_eq!(
        app.world()
            .resource::<RenderDevice>()
            .limits()
            .max_storage_buffers_per_shader_stage,
        0
    );
    for phase in 0..4 {
        if phase == 1 {
            app.world_mut()
                .resource_mut::<Assets<PonEfMaterial>>()
                .get_mut_untracked(first_page.unwrap())
                .unwrap()
                .particle_colors
                .as_mut()
                .unwrap()[0] = Vec4::new(0.0, 1.0, 0.0, 0.5);
            app.world_mut()
                .get_mut::<Visibility>(entities[1])
                .unwrap()
                .set_if_neq(Visibility::Hidden);
        }
        if phase == 2 {
            for (entity, z) in [(entities[0], 2.0), (entities[128], 1.0), (entities[1], 0.0)] {
                *app.world_mut().get_mut::<Transform>(entity).unwrap() =
                    Transform::from_xyz(-64.5, 0.0, z);
                *app.world_mut().get_mut::<Visibility>(entity).unwrap() = Visibility::Visible;
            }
            let mut materials = app.world_mut().resource_mut::<Assets<PonEfMaterial>>();
            materials
                .get_mut_untracked(first_page.unwrap())
                .unwrap()
                .particle_colors
                .as_mut()
                .unwrap()[0] = Vec4::new(1.0, 0.0, 0.0, 0.5);
            materials
                .get_mut_untracked(&page)
                .unwrap()
                .particle_colors
                .as_mut()
                .unwrap()[0] = Vec4::new(0.0, 0.0, 1.0, 0.5);
        }
        if phase == 3 {
            let texture = Image::new_fill(
                bevy::render::render_resource::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                bevy::render::render_resource::TextureDimension::D2,
                &[0, 255, 255, 255],
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            );
            let image = app.world_mut().resource_mut::<Assets<Image>>().add(texture);
            let mut materials = app.world_mut().resource_mut::<Assets<PonEfMaterial>>();
            let mut material = materials.get_mut(first_page.unwrap()).unwrap();
            material.texture = Some(image);
            material.params.has_texture = 1;
        }
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "particle GPU output/batch regression, counts={:?}",
                counts.0.lock().unwrap()
            );
            app.update();
            app.world()
                .resource::<RenderDevice>()
                .poll(PollType::Poll)
                .unwrap();
            let data = pixels.lock().unwrap();

            let color = |i: usize| -> Option<[u8; 3]> {
                let start = 4 * 1280 + (i * 2 + 1) * 4;
                (data.len() > start + 2).then(|| [data[start], data[start + 1], data[start + 2]])
            };
            let red = |c: Option<[u8; 3]>| c.is_some_and(|[r, g, b]| r > 150 && g < 10 && b > 150);
            let green =
                |c: Option<[u8; 3]>| c.is_some_and(|[r, g, b]| r < 10 && g > 150 && b > 150);
            let blue = |c: Option<[u8; 3]>| c.is_some_and(|[r, g, b]| r < 10 && g < 10 && b > 240);
            let valid = match phase {
                0 => red(color(0)) && green(color(1)) && red(color(128)),
                1 => green(color(0)) && blue(color(1)) && red(color(128)),
                2 => {
                    color(0).is_some_and(|[r, g, b]| {
                        (180..200).contains(&r) && (90..110).contains(&g) && (155..175).contains(&b)
                    }) && blue(color(1))
                        && blue(color(128))
                }
                _ => {
                    color(0).is_some_and(|[r, g, b]| {
                        r < 10 && (90..110).contains(&g) && (155..175).contains(&b)
                    }) && color(126)
                        .is_some_and(|[r, g, b]| r < 10 && g < 10 && (180..200).contains(&b))
                }
            };
            if valid && (phase == 3 || red(color(126))) && green(color(127)) && green(color(129)) {
                let n = *counts.0.lock().unwrap();
                assert_eq!(n.0, if phase == 1 { 129 } else { 130 });
                assert!(
                    n.1 < 16 && n.2 >= 16,
                    "expected real instanced draws, got {n:?}"
                );
                eprintln!(
                    "particle-instancing instances={} batches={} largest={}",
                    n.0, n.1, n.2
                );
                break;
            }
            drop(data);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    for entity in entities {
        app.world_mut().despawn(entity);
    }

    for (x, color, alpha_mode) in [
        (-64.5, Color::linear_rgb(1.0, 0.0, 0.0), AlphaMode::Opaque),
        (
            -63.5,
            Color::linear_rgb(0.0, 1.0, 0.0),
            AlphaMode::Mask(0.5),
        ),
    ] {
        let material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: color,
                unlit: true,
                alpha_mode,
                ..default()
            });
        app.world_mut().spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material),
            Transform::from_xyz(x, 0.0, 0.0),
        ));
    }
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "CPU opaque/masked batches did not render"
        );
        app.update();
        app.world()
            .resource::<RenderDevice>()
            .poll(PollType::Poll)
            .unwrap();
        let data = pixels.lock().unwrap();

        let first = 4 * 1280 + 4;
        let second = first + 8;
        if data.len() > second + 2
            && data[first] > 240
            && data[first + 1] < 10
            && data[first + 2] < 10
            && data[second] < 10
            && data[second + 1] > 240
            && data[second + 2] < 10
        {
            eprintln!("CPU opaque and alpha-masked batches: pixel verification passed");
            break;
        }
        drop(data);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    drop(page);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "retired particle pages were not released"
        );
        app.update();
        app.world()
            .resource::<RenderDevice>()
            .poll(PollType::Poll)
            .unwrap();
        if counts.1.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
