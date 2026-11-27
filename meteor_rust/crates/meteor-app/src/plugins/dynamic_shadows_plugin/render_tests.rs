use super::*;
use crate::plugins::stage_material::{GmdlMaterialParams, GmdlMaterialPlugin};
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
            panic!("render pipeline failed: {error}");
        }
    }
    let result = pixels.lock().unwrap().clone();
    assert_eq!(result.len(), 128 * 128 * 4);
    result
}

fn save_capture(name: &str, data: &[u8]) {
    let Some(dir) = std::env::var_os("METEOR_SHADOW_CAPTURE_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    Image::new(
        bevy::render::render_resource::Extent3d {
            width: 128,
            height: 128,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        data.to_vec(),
        TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    )
    .try_into_dynamic()
    .unwrap()
    .save(dir.join(format!("{name}.png")))
    .unwrap();
}

#[cfg(feature = "original-assets")]
#[test]
fn headless_gmdl_casts_shadows_and_lighting_changes_pixels() {
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
        scene_shadows: false,
        dynamic_shadows: true,
        ..default()
    })
    .add_plugins((GmdlMaterialPlugin, DynamicShadowsPlugin));
    crate::plugins::frame_work::install(&mut app);
    let mut image = Image::new_target_texture(128, 128, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
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
            Transform::from_xyz(9.0, 10.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id();
    let material = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(GmdlMaterial::new(
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
        ));
    let plane = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Plane3d::default().mesh().size(20.0, 20.0));
    app.world_mut().spawn((
        Mesh3d(plane),
        MeshMaterial3d(material.clone()),
        Transform::IDENTITY,
    ));
    let cube = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(2.0, 3.0, 2.0));
    let caster = app
        .world_mut()
        .spawn((
            Mesh3d(cube),
            MeshMaterial3d(material.clone()),
            Transform::from_xyz(0.0, 1.5, 0.0),
        ))
        .id();
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
    let shadowed = capture(&mut app, &pixels);
    let shadow_projection = |app: &mut App| {
        let sun = app
            .world_mut()
            .query_filtered::<&Cascades, With<MeteorDynamicSun>>()
            .single(app.world())
            .unwrap();
        assert_eq!(sun.cascades[&camera].len(), 1);
        sun.cascades[&camera][0].clip_from_world
    };
    let original_projection = shadow_projection(&mut app);
    for eye in [Vec3::new(-15.0, 6.0, 8.0), Vec3::new(4.0, 25.0, -20.0)] {
        *app.world_mut().get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(eye).looking_at(Vec3::ZERO, Vec3::Y);
        if let Some(mut projection) = app.world_mut().get_mut::<Projection>(camera) {
            if let Projection::Perspective(p) = &mut *projection {
                p.fov = 0.6;
            }
        }
        capture(&mut app, &pixels);
        assert_eq!(
            shadow_projection(&mut app),
            original_projection,
            "orbit and zoom must not move world shadow texels or depth bias"
        );
    }
    *app.world_mut().get_mut::<Transform>(camera).unwrap() =
        Transform::from_xyz(9.0, 10.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y);
    *app.world_mut().get_mut::<Projection>(camera).unwrap() = Projection::default();
    capture(&mut app, &pixels);
    app.world_mut().entity_mut(caster).insert(NotShadowCaster);
    let unshadowed = capture(&mut app, &pixels);
    let darker = shadowed
        .chunks_exact(4)
        .zip(unshadowed.chunks_exact(4))
        .filter(|(a, b)| i32::from(b[0]) - i32::from(a[0]) > 15)
        .count();
    assert!(
        darker > 30,
        "caster must darken receiving geometry: {darker} pixels"
    );
    app.world_mut()
        .resource_mut::<Settings>()
        .environment_lighting = true;
    let lit = capture(&mut app, &pixels);
    assert!(
        lit.chunks_exact(4)
            .zip(unshadowed.chunks_exact(4))
            .filter(|(a, b)| a[0].abs_diff(b[0]) > 10)
            .count()
            > 100
    );

    app.world_mut()
        .entity_mut(caster)
        .remove::<NotShadowCaster>();
    let cutout = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(GmdlMaterial::new(
            GmdlMaterialParams {
                opacity: 0.0,
                alpha_cutoff: 0.5,
                mode: 0,
                has_texture: 0,
                uv_offset: Vec2::ZERO,
            },
            None,
            true,
            GmdlMode::Opaque,
        ));
    app.world_mut()
        .entity_mut(caster)
        .insert(MeshMaterial3d(cutout));
    let transparent_shadow = capture(&mut app, &pixels);
    app.world_mut().entity_mut(caster).insert(NotShadowCaster);
    let transparent_no_shadow = capture(&mut app, &pixels);
    assert_eq!(
        transparent_shadow, transparent_no_shadow,
        "discard must also apply in shadow prepass"
    );
    app.world_mut().despawn(caster);
    app.world_mut()
        .resource_mut::<Settings>()
        .environment_lighting = false;

    let plane_shadow = capture(&mut app, &pixels);
    app.world_mut().resource_mut::<Settings>().dynamic_shadows = false;
    let plane_no_shadow = capture(&mut app, &pixels);
    let acne_pixels = plane_shadow
        .chunks_exact(4)
        .zip(plane_no_shadow.chunks_exact(4))
        .filter(|(a, b)| i32::from(b[0]) - i32::from(a[0]) > 8)
        .count();
    save_capture("plane-shadow", &plane_shadow);
    save_capture("plane-no-shadow", &plane_no_shadow);
    assert_eq!(
        acne_pixels, 0,
        "flat ground must not produce shadow stripes"
    );
    app.world_mut().resource_mut::<Settings>().dynamic_shadows = true;

    use crate::plugins::stage_plugin::{tick_stage_script, StageScriptRuntime};
    use meteor_scene::scene_script_runtime::{PstValue, SceneItemInstance, SceneScriptRuntime};
    let script = SceneScriptRuntime::load(
        "",
        vec![SceneItemInstance {
            name: "D_shadow".into(),
            index: 0,
            props: std::collections::HashMap::from([
                ("active".into(), PstValue::Num(1.0)),
                ("visible".into(), PstValue::Num(1.0)),
            ]),
        }],
        1000,
    )
    .unwrap();
    app.init_resource::<crate::plugins::network_plugin::NetState>()
        .init_resource::<crate::plugins::stage_collider::StageCollider>()
        .insert_resource(StageScriptRuntime::from_script(script))
        .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>()
        .add_systems(Update, tick_stage_script);
    let patch_material =
        app.world_mut()
            .resource_mut::<Assets<GmdlMaterial>>()
            .add(GmdlMaterial::new(
                GmdlMaterialParams {
                    opacity: 1.0,
                    alpha_cutoff: 0.0,
                    mode: GmdlMode::MultiplyShadow.as_u32(),
                    has_texture: 0,
                    uv_offset: Vec2::ZERO,
                },
                None,
                true,
                GmdlMode::MultiplyShadow,
            ));
    let patch_mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Plane3d::default().mesh().size(8.0, 8.0));
    let patch = app
        .world_mut()
        .spawn((
            Mesh3d(patch_mesh),
            MeshMaterial3d(patch_material),
            Transform::from_xyz(0.0, 0.02, 0.0),
            crate::plugins::item_plugin::StageProp {
                name: "D_shadow".into(),
                model: "sn08b03".into(),
                center_offset: Vec3::ZERO,
                half_extents: Vec3::ONE,
                hit_count: 0,
                respawn_timer: 0.0,
                interactive: false,
                collision_enabled: false,
                breakable: false,
                broken: false,
            },
        ))
        .id();
    let suppressed = capture(&mut app, &pixels);
    assert_eq!(
        *app.world().get::<Visibility>(patch).unwrap(),
        Visibility::Visible
    );
    assert_eq!(
        plane_shadow, suppressed,
        "PST writes must not defeat render suppression"
    );
    app.world_mut().resource_mut::<Settings>().dynamic_shadows = false;
    let baked = capture(&mut app, &pixels);
    assert!(
        baked
            .chunks_exact(4)
            .zip(plane_no_shadow.chunks_exact(4))
            .filter(|(a, b)| i32::from(b[0]) - i32::from(a[0]) > 15)
            .count()
            > 100
    );
    app.world_mut()
        .resource_mut::<StageScriptRuntime>()
        .script_mut()
        .unwrap()
        .set_item_property("D_shadow", "active", PstValue::Num(0.0));
    app.world_mut().resource_mut::<Settings>().dynamic_shadows = true;
    capture(&mut app, &pixels);
    app.world_mut().resource_mut::<Settings>().dynamic_shadows = false;
    let inactive = capture(&mut app, &pixels);
    assert_eq!(
        *app.world().get::<Visibility>(patch).unwrap(),
        Visibility::Hidden
    );
    assert_eq!(
        plane_no_shadow, inactive,
        "turning shadows off must preserve PST hiding"
    );
    app.world_mut().despawn(patch);
    app.world_mut().resource_mut::<Settings>().dynamic_shadows = true;
    app.init_resource::<crate::plugins::pak_textures::QualityTextureHandles>();
    use crate::plugins::character_plugin::spawn_live_portrait_fixture;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::mesh::skinning::SkinnedMesh;
    app.world_mut().spawn((
        crate::components::fighter::Fighter {
            character_id: 1,
            player_slot: 0,
            weapon: 0,
            weapon_slots: [0, 0],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        },
        Transform::IDENTITY,
        Visibility::Inherited,
    ));
    app.world_mut()
        .run_system_once(spawn_live_portrait_fixture)
        .unwrap();

    capture(&mut app, &pixels);
    let skins: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<SkinnedMesh>>()
        .iter(app.world())
        .collect();
    assert!(!skins.is_empty());
    for entity in &skins {
        app.world_mut()
            .entity_mut(*entity)
            .insert(Visibility::Inherited);
    }
    let character_shadow = capture(&mut app, &pixels);
    for entity in &skins {
        app.world_mut().entity_mut(*entity).insert(NotShadowCaster);
    }
    let character_no_shadow = capture(&mut app, &pixels);
    let count_dark = |a: &[u8], b: &[u8]| {
        a.chunks_exact(4)
            .zip(b.chunks_exact(4))
            .filter(|(a, b)| i32::from(b[0]) - i32::from(a[0]) > 15)
            .count()
    };
    save_capture("character-ground-shadow", &character_shadow);
    save_capture("character-ground-no-shadow", &character_no_shadow);
    let ground_pixels = count_dark(&character_shadow, &character_no_shadow);
    assert!(
        ground_pixels > 10,
        "real SKC character must cast onto ground: {ground_pixels}"
    );

    let wall_mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Rectangle::new(12.0, 12.0));
    app.world_mut().spawn((
        Mesh3d(wall_mesh),
        MeshMaterial3d(material),
        Transform::from_xyz(0.0, 4.0, -1.8),
    ));

    for mut transform in app
        .world_mut()
        .query_filtered::<&mut Transform, With<MeteorDynamicSun>>()
        .iter_mut(app.world_mut())
    {
        *transform = Transform::from_xyz(0.0, 3.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y);
    }
    let wall_no_shadow = capture(&mut app, &pixels);
    for entity in &skins {
        app.world_mut()
            .entity_mut(*entity)
            .remove::<NotShadowCaster>();
    }
    let wall_shadow = capture(&mut app, &pixels);
    save_capture("character-wall-shadow", &wall_shadow);
    save_capture("character-wall-no-shadow", &wall_no_shadow);
    let wall_pixels = count_dark(&wall_shadow, &wall_no_shadow);
    assert!(
        wall_pixels > 10,
        "real SKC character must cast onto wall: {wall_pixels}"
    );
    eprintln!("projected shadow pixels: ground={ground_pixels}, wall={wall_pixels}");
}
