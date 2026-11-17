use super::*;
use bevy::{
    app::PluginsState,
    camera::RenderTarget,
    render::{
        gpu_readback::{Readback, ReadbackComplete},
        render_resource::{PollType, TextureFormat, TextureUsages},
        renderer::RenderDevice,
        RenderPlugin,
    },
};
use std::sync::{Arc, Mutex};
fn capture(app: &mut App, pixels: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
    for _ in 0..24 {
        app.update();
        app.world()
            .resource::<RenderDevice>()
            .poll(PollType::Poll)
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for pipeline in app
        .sub_app(bevy::render::RenderApp)
        .world()
        .resource::<bevy::render::render_resource::PipelineCache>()
        .pipelines()
    {
        if let bevy::render::render_resource::CachedPipelineState::Err(e) = &pipeline.state {
            panic!("graphics pack pipeline: {e}");
        }
    }
    let data = pixels.lock().unwrap().clone();
    assert_eq!(data.len(), 128 * 128 * 4);
    data
}
fn changed(a: &[u8], b: &[u8]) -> usize {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 2))
        .count()
}
#[test]
fn headless_pbr_environment_fog_contacts_and_disabled_restoration() {
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
    .insert_resource(crate::app_state::Settings {
        scene_normal_maps: false,
        ..default()
    })
    .add_plugins(crate::plugins::stage_material::GmdlMaterialPlugin);
    crate::plugins::frame_work::install(&mut app);
    let mut image = Image::new_target_texture(128, 128, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        RenderTarget::Image(target.clone().into()),
        Msaa::Off,
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Transform::from_xyz(0., 6., 8.).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let mut mesh = Plane3d::default().mesh().size(12., 12.).build();
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        vec![[0.5f32, 0.5, 0.5, 1.]; mesh.count_vertices()],
    );
    mesh.generate_tangents().unwrap();
    let original = plain();
    let mut enhanced = original.clone();
    let mut packs = starter();

    let profile = packs.packs[0].config.materials.get_mut("stone").unwrap();
    profile.normal_strength = 4.;
    profile.metallic = 1.;
    SceneGraphics::new(Some(&packs), "sn06").configure(
        "floor",
        Some("sn064.jpg"),
        &mut enhanced,
        &mut mesh,
        true,
        false,
        &mut app.world_mut().resource_mut::<Assets<Image>>(),
    );
    let mh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
    let material = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(original.clone());
    app.world_mut().spawn((
        Mesh3d(mh),
        MeshMaterial3d(material.clone()),
        Transform::IDENTITY,
    ));
    let pixels = Arc::new(Mutex::new(vec![]));
    let out = pixels.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |e: On<ReadbackComplete>| *out.lock().unwrap() = e.data.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    app.finish();
    app.cleanup();
    let baseline = capture(&mut app, &pixels);
    *app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap() = enhanced.clone();
    let g = GraphicsFrame {
        sky: Vec4::new(0.4, 0.5, 0.7, 0.),
        ground: Vec4::new(0.1, 0.1, 0.1, 0.),
        sun: Vec3::new(0.4, 0.8, 0.3).normalize().extend(2.),
        sun_color: Vec4::ONE,
        environment: Vec4::new(1., 0., 1., 5.),
        ..default()
    };
    app.world_mut().resource_mut::<FrameUniforms>().graphics = g;
    let pbr = capture(&mut app, &pixels);
    assert!(
        changed(&baseline, &pbr) > 1000,
        "PBR must change rendered pixels"
    );
    app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap()
        .normal_texture = None;
    let no_normal = capture(&mut app, &pixels);
    assert!(
        changed(&pbr, &no_normal) > 100,
        "external normal map contributes to PBR shading: {} changed pixels",
        changed(&pbr, &no_normal)
    );
    *app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap() = enhanced.clone();
    app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap()
        .pbr_textures
        .x = 0.;
    let no_orm = capture(&mut app, &pixels);
    assert!(
        changed(&pbr, &no_orm) > 30,
        "external ORM contributes to PBR shading: {} changed pixels",
        changed(&pbr, &no_orm)
    );
    *app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap() = enhanced.clone();
    app.world_mut()
        .resource_mut::<FrameUniforms>()
        .graphics
        .environment
        .x = 0.;
    let no_environment = capture(&mut app, &pixels);
    assert!(
        changed(&pbr, &no_environment) > 1000,
        "external environment contributes light"
    );
    app.world_mut().resource_mut::<FrameUniforms>().graphics = g;
    {
        let mut f = app.world_mut().resource_mut::<FrameUniforms>();
        f.graphics.counts.y = 1.;
        f.graphics.contacts[0] = Vec4::new(0., 0., 0., 3.);
        f.graphics.contact_normals[0] = Vec4::new(0., 1., 0., 0.6);
    }
    let contacts = capture(&mut app, &pixels);
    assert!(
        changed(&pbr, &contacts) > 30,
        "contact shadow must be visible"
    );
    {
        let mut f = app.world_mut().resource_mut::<FrameUniforms>();
        f.graphics = g;
        f.graphics.counts.x = 1.;
        f.graphics.fog_bounds[0] = Vec4::new(0., 3., 3., 5.);
        f.graphics.fog_colors[0] = Vec4::new(0.1, 0.3, 0.8, 0.2);
    }
    let fog = capture(&mut app, &pixels);
    assert!(
        changed(&pbr, &fog) > 1000,
        "local fog must integrate along view rays"
    );
    *app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap() = original;
    app.world_mut().resource_mut::<FrameUniforms>().graphics = GraphicsFrame::default();
    let restored = capture(&mut app, &pixels);
    assert_eq!(
        baseline, restored,
        "disabled pack restores original shader output exactly"
    );
}
