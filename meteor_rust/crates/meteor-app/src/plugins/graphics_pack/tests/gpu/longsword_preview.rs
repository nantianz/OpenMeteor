use super::*;
use crate::plugins::graphics_pack::loader::load_with_reader;
use crate::plugins::stage_material::GmdlMode;
use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    render::render_resource::{Extent3d, PrimitiveTopology, TextureDimension},
};

#[test]
fn longsword_original_model_pbr_preview() {
    let Some(output) = std::env::var_os("METEOR_LONGSWORD_PREVIEW") else {
        return;
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output).unwrap();
    let text = std::fs::read_to_string(root.join("origin_res/cmodel/w0_0.GMC")).unwrap();
    let gmc = meteor_asset::gmc::parse_gmc(&text);
    let object = gmc
        .objects
        .iter()
        .find(|o| o.name.eq_ignore_ascii_case("wpnLS01"))
        .unwrap();
    let indices: Vec<u32> = object
        .faces
        .iter()
        .filter(|face| face.shader == 0)
        .flat_map(|face| face.indices)
        .collect();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        object
            .positions
            .chunks_exact(3)
            .map(|v| [v[0], v[1], v[2]])
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        object
            .normals
            .chunks_exact(3)
            .map(|v| [v[0], v[1], v[2]])
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_UV_0,
        object
            .uvs
            .chunks_exact(2)
            .map(|v| [v[0], v[1]])
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_COLOR,
        vec![[1.0f32; 4]; object.positions.len() / 3],
    )
    .with_inserted_indices(Indices::U32(indices));
    let bytes = std::fs::read(root.join("origin_res/ctexture.pak")).unwrap();
    let pak = meteor_asset::pak::PakArchive::from_bytes(&bytes).unwrap();
    let image =
        meteor_asset::texture::decode_image(&pak.read_entry("ctexture/WpnLS01.jpg").unwrap())
            .unwrap();
    let config = root.join("meteor_rust/config");
    let packs = load_with_reader(
        &std::fs::read(config.join("graphics-enhancement-longsword-preview.json")).unwrap(),
        |name| std::fs::read(config.join(name)).map_err(|e| e.to_string()),
    )
    .unwrap();
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
    crate::plugins::frame_work::install(&mut app);
    let texture = crate::plugins::pak_textures::image_from_decoded(
        &mut app.world_mut().resource_mut::<Assets<Image>>(),
        &image,
    );
    let original = GmdlMaterial::new(
        GmdlMaterialParams {
            opacity: 1.,
            alpha_cutoff: 0.,
            mode: 0,
            has_texture: 1,
            uv_offset: Vec2::ZERO,
        },
        Some(texture),
        false,
        GmdlMode::Opaque,
    );
    let mut enhanced = original.clone();
    SceneGraphics::new(Some(&packs), "sn06").apply_pbr(
        0,
        "longsword",
        &object.name,
        &mut enhanced,
        &mut mesh,
        &mut app.world_mut().resource_mut::<Assets<Image>>(),
    );
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
    let material = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(original);
    app.world_mut().spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material.clone()),
        Transform::IDENTITY,
    ));
    let (width, height) = (1024, 1024);
    let mut target = Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None);
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.055, 0.065, 0.085)),
            ..default()
        },
        RenderTarget::Image(target.clone().into()),
        Msaa::Off,
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Transform::from_xyz(0., -8., 32.).looking_at(Vec3::new(0., -8., 0.), Vec3::Y),
    ));
    let pixels = Arc::new(Mutex::new(vec![]));
    let readback = pixels.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |e: On<ReadbackComplete>| *readback.lock().unwrap() = e.data.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    let baseline = capture_size(&mut app, &pixels, width, height);
    *app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&material)
        .unwrap() = enhanced;
    app.world_mut().resource_mut::<FrameUniforms>().graphics = GraphicsFrame {
        sky: Vec4::new(0.7, 0.75, 0.85, 0.),
        ground: Vec4::new(0.22, 0.20, 0.17, 0.),
        sun: Vec3::new(0.4, 0.7, 0.7).normalize().extend(2.0),
        sun_color: Vec4::ONE,
        environment: Vec4::new(1., 0., 1., 5.),
        ..default()
    };
    let pbr = capture_size(&mut app, &pixels, width, height);
    assert!(changed(&baseline, &pbr) > 200);
    for (name, data) in [("original.png", baseline), ("pbr.png", pbr)] {
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
        .save(output.join(name))
        .unwrap();
    }
}
