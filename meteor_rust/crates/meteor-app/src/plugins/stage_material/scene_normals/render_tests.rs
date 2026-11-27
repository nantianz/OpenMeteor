use super::*;
use crate::app_state::{AppState, Settings};
use crate::plugins::stage_material::{
    FrameUniforms, GmdlMaterial, GmdlMaterialParams, GmdlMaterialPlugin, GmdlMode,
};
use bevy::app::PluginsState;
use bevy::camera::RenderTarget;
use bevy::render::{
    gpu_readback::{Readback, ReadbackComplete},
    render_resource::{PollType, TextureFormat, TextureUsages},
    renderer::RenderDevice,
    RenderPlugin,
};
use std::sync::{Arc, Mutex};

fn capture(app: &mut App, pixels: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
    for _ in 0..30 {
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
        if let bevy::render::render_resource::CachedPipelineState::Err(error) = &pipeline.state {
            panic!("normal-map pipeline failed: {error}");
        }
    }
    let result = pixels.lock().unwrap().clone();
    assert_eq!(result.len(), 128 * 128 * 4);
    result
}

#[test]
fn headless_scene_normals_change_pixels_and_toggle_back_exactly() {
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
        scene_normal_maps: false,
        ..default()
    })
    .add_plugins(GmdlMaterialPlugin);
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
        Transform::from_xyz(0.0, 9.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let texture = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/graphics-packs/scene-normals/textures/sn03/sn314.jpg.normal.png",
    ))
    .unwrap();
    let normal = load_hd_scene_normal(
        "sn03",
        "sn314.jpg",
        &texture,
        &mut app.world_mut().resource_mut::<Assets<Image>>(),
        &mut HashMap::new(),
        0,
    )
    .unwrap();
    let mut material = GmdlMaterial::new_scene(
        GmdlMaterialParams {
            opacity: 1.0,
            alpha_cutoff: 0.0,
            mode: 0,
            has_texture: 0,
            uv_offset: Vec2::ZERO,
        },
        None,
        false,
        GmdlMode::Opaque,
    );
    material.normal_texture = Some(normal.clone());
    let material = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(material);
    let mut mesh = Plane3d::default().mesh().size(10.0, 10.0).build();
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        vec![[0.4_f32, 0.4, 0.4, 1.0]; mesh.count_vertices()],
    );
    mesh.generate_tangents().unwrap();
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
    app.world_mut().spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(material.clone()),
        Transform::IDENTITY,
    ));
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
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    app.finish();
    app.cleanup();
    for lit in [false, true] {
        if lit {
            app.world_mut().spawn((
                DirectionalLight {
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_xyz(40.0, 80.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
            ));
            app.world_mut().resource_mut::<FrameUniforms>().lighting =
                Vec4::new(1.0, 0.65, 0.55, 0.0);
        }
        let flat = capture(&mut app, &pixels);
        app.world_mut().resource_mut::<Settings>().scene_normal_maps = true;
        let bumped = capture(&mut app, &pixels);
        let mean_delta = flat
            .chunks_exact(4)
            .zip(bumped.chunks_exact(4))
            .map(|(a, b)| f64::from(a[0].abs_diff(b[0])))
            .sum::<f64>()
            / (128.0 * 128.0);
        eprintln!("floor normal mean pixel delta: environment={lit}, delta={mean_delta:.4}");
        let changed = flat
            .chunks_exact(4)
            .zip(bumped.chunks_exact(4))
            .filter(|(a, b)| a[0].abs_diff(b[0]) > 1)
            .count();
        assert!(
            changed > 50,
            "normals must change visible pixels, environment={lit}, changed={changed}"
        );
        app.world_mut().resource_mut::<Settings>().scene_normal_maps = false;
        assert_eq!(
            capture(&mut app, &pixels),
            flat,
            "toggle must fully restore authored shading"
        );
        eprintln!("normal-map changed pixels: environment={lit}, pixels={changed}");
    }
    #[cfg(feature = "original-assets")]
    {
        let hd_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../asset-tools/generated/scene-textures/sn06");
        let hd_color_bytes = std::fs::read(hd_root.join("sn06mian5.jpg")).unwrap();
        let hd_normal_bytes = std::fs::read(hd_root.join("sn06mian5.jpg.normal.png")).unwrap();
        let hd_color = crate::plugins::pak_textures::image_handle_with_quality(
            &mut app.world_mut().resource_mut::<Assets<Image>>(),
            &hd_color_bytes,
            0,
        )
        .unwrap();
        let hd_normal = load_hd_scene_normal(
            "sn06",
            "sn06mian5.jpg",
            &hd_normal_bytes,
            &mut app.world_mut().resource_mut::<Assets<Image>>(),
            &mut HashMap::new(),
            0,
        )
        .unwrap();
        {
            let mut materials = app.world_mut().resource_mut::<Assets<GmdlMaterial>>();
            let mut material = materials.get_mut(&material).unwrap();
            material.texture = Some(hd_color);
            material.params.has_texture = 1;
            material.normal_texture = Some(hd_normal);
            material.normal_strength = 1.8;
            material.normal_contrast = 1.5;
        }
        for lit in [false, true] {
            app.world_mut().resource_mut::<FrameUniforms>().lighting.x =
                if lit { 1.0 } else { 0.0 };
            app.world_mut().resource_mut::<Settings>().scene_normal_maps = false;
            let flat = capture(&mut app, &pixels);
            app.world_mut().resource_mut::<Settings>().scene_normal_maps = true;
            let relief = capture(&mut app, &pixels);
            let mean = flat
                .chunks_exact(4)
                .zip(relief.chunks_exact(4))
                .map(|(a, b)| f64::from(a[0].abs_diff(b[0])))
                .sum::<f64>()
                / (128.0 * 128.0);
            let changed = flat
                .chunks_exact(4)
                .zip(relief.chunks_exact(4))
                .filter(|(a, b)| a[0].abs_diff(b[0]) > 2)
                .count();
            eprintln!(
            "HD floor relief: environment={lit}, mean_delta={mean:.4}, changed_pixels={changed}"
        );
            assert!(
                mean > 2.0 && changed > 1500,
                "HD floor relief is too weak: mean={mean}, changed={changed}"
            );
            app.world_mut().resource_mut::<Settings>().scene_normal_maps = false;
            assert_eq!(capture(&mut app, &pixels), flat);
        }
    }
    app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap()
        .normal_texture = None;
    let missing = capture(&mut app, &pixels);
    app.world_mut().resource_mut::<Settings>().scene_normal_maps = true;
    assert_eq!(
        capture(&mut app, &pixels),
        missing,
        "unmapped materials must stay unchanged"
    );
    app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap()
        .normal_texture = Some(normal);
    {
        let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
        let mut mesh = meshes.get_mut(&mesh).unwrap();
        let vertex_count = mesh.count_vertices();
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_TANGENT,
            vec![[0.0_f32, 0.0, 0.0, 1.0]; vertex_count],
        );
    }
    assert_eq!(
        capture(&mut app, &pixels),
        missing,
        "collapsed UV basis must keep geometric lighting"
    );
    let base_color = Image::new_fill(
        bevy::render::render_resource::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        &[180, 180, 180, 255],
        TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    );
    let base_color = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(base_color);
    {
        let mut materials = app.world_mut().resource_mut::<Assets<GmdlMaterial>>();
        let mut material = materials.get_mut(&material).unwrap();
        material.texture = Some(base_color);
        material.params.has_texture = 1;
    }
    app.world_mut().resource_mut::<FrameUniforms>().lighting = Vec4::ZERO;
    let baseline = capture(&mut app, &pixels);
    app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap()
        .stage_render_flags = 1;
    app.world_mut()
        .resource_mut::<FrameUniforms>()
        .graphics
        .stage_render
        .ambient = Vec4::new(0.25, 0.25, 0.25, 1.0);
    let ambient = capture(&mut app, &pixels);
    assert!(
        ambient != baseline,
        "authored ambient affects the rendered surface"
    );
    {
        let mut frame = app.world_mut().resource_mut::<FrameUniforms>();
        frame.graphics.stage_render.point_count.x = 1.0;
        frame.graphics.stage_render.point_positions[0] = Vec4::new(0.0, 5.0, 0.0, 0.05);
        frame.graphics.stage_render.point_colors[0] = Vec4::new(0.12, 0.5, 0.08, 0.0);
    }
    let single_light = capture(&mut app, &pixels);
    assert_ne!(
        single_light, ambient,
        "an authored point light changes the pixels"
    );
    {
        let mut frame = app.world_mut().resource_mut::<FrameUniforms>();
        let stage = &mut frame.graphics.stage_render;
        stage.point_count.x = 255.0;
        stage.point_positions[1..].fill(Vec4::new(1024.0, 3.0, 1024.0, 0.05));
        stage.point_colors[1..].fill(Vec4::ONE);
        stage.point_cells[0] = UVec4::new(1, 0, 0, 0);
    }
    assert_eq!(
        capture(&mut app, &pixels),
        single_light,
        "faraway lights stay out of the local loop"
    );
    {
        let mut frame = app.world_mut().resource_mut::<FrameUniforms>();
        frame.graphics.stage_render.fog_params = Vec4::new(0.001, 1000.0, 0.0, 0.0);
        frame.graphics.stage_render.fog_color = Vec4::new(0.75, 0.04, 0.75, 1.0);
        frame.graphics.stage_render.point_count.y = 1.0 / 1200.0;
    }
    assert_ne!(
        capture(&mut app, &pixels),
        single_light,
        "authored fog affects the rendered surface"
    );
    app.world_mut()
        .resource_mut::<FrameUniforms>()
        .graphics
        .stage_render = default();
    assert_eq!(
        capture(&mut app, &pixels),
        baseline,
        "leaving a stage clears its authored shading"
    );
}
