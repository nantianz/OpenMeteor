use super::*;

fn half_to_float(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = (bits >> 10) & 31;
    let fraction = bits & 1023;
    match exponent {
        0 => sign * (fraction as f32 / 1024.0) * 2f32.powi(-14),
        31 => {
            if fraction == 0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + fraction as f32 / 1024.0) * 2f32.powi(exponent as i32 - 15),
    }
}

#[test]
fn hdr_pbr_preserves_highlights_before_the_shared_tone_map() {
    use crate::plugins::stage_material::{GmdlMaterial, GmdlMaterialParams, GmdlMode};
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
    .insert_state(crate::app_state::AppState::Playing)
    .insert_resource(crate::app_state::Settings::default())
    .add_plugins(crate::plugins::stage_material::GmdlMaterialPlugin);
    let mut target = Image::new_target_texture(64, 64, TextureFormat::Rgba16Float, None);
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    app.world_mut().spawn((
        Camera3d::default(),
        bevy::camera::Hdr,
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Msaa::Off,
        RenderTarget::Image(target.clone().into()),
        Transform::from_xyz(0.0, 5.0, 2.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Plane3d::default().mesh().size(12.0, 12.0).build());
    let mut material = GmdlMaterial::new_scene(
        GmdlMaterialParams {
            opacity: 1.0,
            alpha_cutoff: 0.0,
            mode: 0,
            has_texture: 0,
            uv_offset: Vec2::ZERO,
        },
        None,
        true,
        GmdlMode::Opaque,
    );
    material.pbr = Vec4::new(1.0, 0.0, 0.8, 0.0);
    material.pbr_tint = Vec4::new(1.0, 1.0, 1.0, 0.0);
    let material = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(material);
    app.world_mut()
        .spawn((Mesh3d(mesh), MeshMaterial3d(material), Transform::IDENTITY));
    let pixels = Arc::new(Mutex::new(Vec::new()));
    let output = pixels.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |event: On<ReadbackComplete>| {
            *output.lock().unwrap() = event.data.clone();
        });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    app.world_mut().resource_mut::<FrameUniforms>().graphics = GraphicsFrame {
        sky: Vec4::new(4.0, 4.0, 4.0, 0.0),
        ground: Vec4::new(4.0, 4.0, 4.0, 0.0),
        sun: Vec4::new(0.0, 1.0, 0.0, 0.0),
        sun_color: Vec4::ONE,
        environment: Vec4::new(1.0, 0.0, 1.0, 0.0),
        color_pipeline: Vec4::X,
        ..default()
    };
    let read = |app: &mut App| {
        for _ in 0..24 {
            app.update();
            app.world()
                .resource::<RenderDevice>()
                .poll(PollType::Poll)
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let data = pixels.lock().unwrap();
        assert_eq!(data.len(), 64 * 64 * 8);
        let offset = (32 * 64 + 32) * 8;
        half_to_float(u16::from_le_bytes([data[offset], data[offset + 1]]))
    };
    let hdr = read(&mut app);
    app.world_mut()
        .resource_mut::<FrameUniforms>()
        .graphics
        .color_pipeline = Vec4::ZERO;
    let legacy = read(&mut app);
    assert!(
        hdr > 1.2,
        "HDR path must preserve radiance above one: {hdr}"
    );
    assert!(
        legacy > 0.0 && legacy <= 1.0,
        "disabled path retains local tone mapping: {legacy}"
    );
}
