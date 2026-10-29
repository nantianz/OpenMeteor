use super::*;
use bevy::{
    app::PluginsState,
    camera::{RenderTarget, ScalingMode},
    core_pipeline::tonemapping::Tonemapping,
    prelude::*,
    render::{
        gpu_readback::{Readback, ReadbackComplete},
        render_resource::{TextureFormat, TextureUsages},
        renderer::RenderAdapterInfo,
    },
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires the prepared x64 ANGLE DLLs on PATH; automated, no game window"]
fn angle_dx11_offscreen_smoke() {
    let settings = resolve(
        &Settings {
            graphics_backend: GL_LABEL.into(),
            ..Default::default()
        },
        &[("ANGLE".into(), GL_LABEL.into())],
    );
    assert_eq!(settings.backends, Some(wgpu::Backends::GL));
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(bevy::render::RenderPlugin {
                render_creation: settings.into(),
                synchronous_pipeline_compilation: true,
                ..default()
            })
            .set(bevy::pbr::PbrPlugin {
                use_gpu_instance_buffer_builder: false,
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::audio::AudioPlugin>(),
    );
    app.add_plugins(crate::CpuMeshPreprocessingPlugin);
    let mut image = Image::new_target_texture(64, 64, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.0, 0.0, 1.0)),
            ..default()
        },
        RenderTarget::Image(target.clone().into()),
        Msaa::Off,
        Tonemapping::None,
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: 4.0,
                height: 4.0,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 0.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(2.0, 2.0, 1.0));
    let material = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.0, 0.0),
            unlit: true,
            ..default()
        });
    app.world_mut()
        .spawn((Mesh3d(mesh), MeshMaterial3d(material), Transform::default()));
    let captured = Arc::new(Mutex::new(Vec::<u8>::new()));
    let output = captured.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |event: On<ReadbackComplete>| {
            *output.lock().unwrap() = event.data.clone();
        });
    let deadline = Instant::now() + Duration::from_secs(60);
    while app.plugins_state() != PluginsState::Ready {
        assert!(
            Instant::now() < deadline,
            "ANGLE renderer initialization timed out"
        );
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(Duration::from_millis(5));
    }
    app.finish();
    app.cleanup();
    let adapter = app.world().resource::<RenderAdapterInfo>();
    assert_eq!(adapter.backend, wgpu::Backend::Gl);
    assert!(
        adapter.name.contains("Direct3D11"),
        "unexpected renderer: {}",
        adapter.name
    );
    println!("ANGLE adapter: {} / {:?}", adapter.name, adapter.backend);
    loop {
        assert!(
            Instant::now() < deadline,
            "ANGLE Bevy material/readback timed out"
        );
        app.update();
        assert!(
            app.should_exit().is_none(),
            "renderer requested exit; inspect the first validation error above"
        );
        let data = captured.lock().unwrap();
        if data.len() == 64 * 64 * 4 {
            let center = &data[(32 * 64 + 32) * 4..][..4];
            let corner = &data[..4];
            if center[0] > 240 && center[1] < 10 && center[2] < 10 {
                assert!(
                    corner[0] < 10 && corner[1] < 10 && corner[2] > 240,
                    "background: {corner:?}"
                );
                println!("ANGLE readback: center={center:?}, corner={corner:?}");
                break;
            }
        }
        drop(data);
        std::thread::sleep(Duration::from_millis(10));
    }
}
