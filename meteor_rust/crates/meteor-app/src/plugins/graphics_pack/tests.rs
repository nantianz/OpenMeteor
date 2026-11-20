use super::loader::{load_with_reader, parse_index};
use super::*;
use crate::plugins::stage_material::{FrameUniforms, GmdlMaterial, GmdlMaterialParams, GmdlMode};
use serde_json::json;
fn config_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config")
}
pub(super) fn starter() -> GraphicsPacks {
    let mut packs = load_with_reader(
        br#"{"schema_version":1,"enabled":true,"packs":["graphics-packs/starter/pack.json"]}"#,
        |name| std::fs::read(config_root().join(name)).map_err(|e| e.to_string()),
    )
    .unwrap();

    for scene in &mut packs.packs[0].config.scenes {
        if let Some(sky) = &mut scene.sky {
            sky.time = None;
        }
    }
    packs
}

#[test]
fn sn06_nextgen_sample_loads_stage_and_longsword_resources() {
    let base = config_root();
    let mut packs = super::loader::load_catalog_with_reader(
        &std::fs::read(base.join("graphics-enhancement-sn06-nextgen.json")).unwrap(),
        |name| std::fs::read(base.join(name)).map_err(|e| e.to_string()),
    )
    .unwrap()
    .with_test_loose_root(base);
    packs.prepare_stage("sn06");
    let sample = packs
        .packs
        .iter()
        .find(|pack| pack.config.id == "sn06-nextgen-sample")
        .unwrap();
    assert!(sample.config.scenes[0].sky.as_ref().unwrap().enabled);
    assert!(sample.config.scenes[0].grass.as_ref().unwrap().enabled);
    let sword = packs
        .packs
        .iter()
        .find(|pack| pack.config.id == "longsword-pbr-preview")
        .unwrap();
    assert!(sword.images.contains_key("longsword-orm"));
    assert_eq!(sword.config.scenes[0].weapons[0].weapon, 6);
}

#[test]
fn additive_texture_only_matches_selected_additive_surfaces() {
    let base = config_root();
    let packs = load_with_reader(
        &std::fs::read(base.join("graphics-enhancement-sn06-waterfall-experiment.json")).unwrap(),
        |name| std::fs::read(base.join(name)).map_err(|e| e.to_string()),
    )
    .unwrap();
    let mut images = Assets::<Image>::default();
    let mut scene = SceneGraphics::new(Some(&packs), "sn06");
    let mut material = GmdlMaterial::new(
        GmdlMaterialParams {
            opacity: 1.,
            alpha_cutoff: 0.,
            mode: 4,
            has_texture: 0,
            uv_offset: Vec2::ZERO,
        },
        None,
        false,
        GmdlMode::SrcColorOne,
    );
    scene.configure_additive_texture("wrong", Some("sn06sh.jpg"), &mut material, &mut images);
    assert!(material.texture.is_none());
    scene.configure_additive_texture("plane20", Some("SN06SH.JPG"), &mut material, &mut images);
    assert!(material.texture.is_some());
    assert_eq!(material.params.has_texture, 1);
    assert!(scene
        .configure_additive_texture("Plane19", Some("sn06sh2.jpg"), &mut material, &mut images)
        .is_some());
    let mut opaque = plain();
    scene.configure_additive_texture("Plane20", Some("sn06sh.jpg"), &mut opaque, &mut images);
    assert!(opaque.texture.is_none());
}

#[test]
fn waterfall_lighting_only_marks_selected_additive_surface() {
    let packs = load_with_reader(
        br#"{"schema_version":1,"enabled":true,"packs":["p.json"]}"#,
        |_| Ok(br#"{"schema_version":1,"id":"test","scenes":[{"stage":"sn06","surfaces":[{"texture":"sn06sh2.jpg","waterfall_lighting":true}]}]}"#.to_vec()),
    ).unwrap();
    let mut scene = SceneGraphics::new(Some(&packs), "sn06");
    let mut images = Assets::<Image>::default();
    let mut additive = GmdlMaterial::new(
        GmdlMaterialParams {
            opacity: 1.,
            alpha_cutoff: 0.,
            mode: 4,
            has_texture: 0,
            uv_offset: Vec2::ZERO,
        },
        None,
        false,
        GmdlMode::SrcColorOne,
    );
    scene.configure_additive_texture("Plane19", Some("other.jpg"), &mut additive, &mut images);
    assert_eq!(additive.water.x, 0.0);
    scene.configure_additive_texture("Plane19", Some("SN06SH2.JPG"), &mut additive, &mut images);
    assert_eq!(additive.water.x, 7.0);
    let mut opaque = plain();
    scene.configure_additive_texture("Plane19", Some("sn06sh2.jpg"), &mut opaque, &mut images);
    assert_eq!(opaque.water.x, 0.0);
}

#[test]
fn water_sky_uses_manifest_selectors_and_disabled_overrides() {
    let mut profile = json!({
        "surface": {"object":"Plane37", "texture":"sn914.jpg"}, "enabled":true,
        "visibility_resolution":24, "visibility_directions":8,
        "wave_distortion":0.2, "refraction_damping":0.4, "grazing_boost":0.5,
        "grazing_range":[0.1, 0.6], "horizon_fade":[0.01, 0.05]
    });
    let preset = json!({"schema_version":1, "id":"preset", "scenes":[{
        "stage":"sn09", "sky":{"enabled":true, "replace_skybox":true,
            "skybox":[{"object":"TestSky", "texture":"sky.bmp"}]},
        "water_sky_reflections":[profile.clone()]
    }]});
    let load = |index: &[u8], override_pack: &serde_json::Value| {
        load_with_reader(index, |name| {
            Ok(if name == "preset.json" {
                &preset
            } else {
                override_pack
            }
            .to_string()
            .into_bytes())
        })
    };
    let packs = load(
        br#"{"schema_version":1,"enabled":true,"packs":["preset.json"]}"#,
        &json!(null),
    )
    .unwrap();
    let selected = packs
        .water_sky("SN09", "plane37", "sn09\\SN914.JPG")
        .unwrap();
    assert_eq!(selected.profile.visibility_resolution, 24);
    assert_eq!(selected.profile.visibility_directions, 8);
    assert_eq!(selected.profile.grazing_boost, 0.5);
    assert_eq!(selected.skybox[0].object, "TestSky");
    assert!(packs.water_sky("sn13", "Plane37", "sn914.jpg").is_none());
    assert!(packs.water_sky("sn09", "Plane02", "sn914.jpg").is_none());
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    assert_eq!(
        packs
            .water_sky_for_bake("sn09", "Plane37", "sn914.jpg")
            .unwrap()
            .profile
            .visibility_directions,
        8
    );

    profile["enabled"] = json!(false);
    let mut override_pack = json!({"schema_version":1, "id":"override", "scenes":[{
        "stage":"sn09", "water_sky_reflections":[profile]
    }]});
    let layered = br#"{"schema_version":1,"enabled":true,"packs":["preset.json","override.json"]}"#;
    let packs = load(layered, &override_pack).unwrap();
    assert!(packs.water_sky("sn09", "Plane37", "sn914.jpg").is_none());
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    assert!(packs
        .water_sky_for_bake("sn09", "Plane37", "sn914.jpg")
        .is_none());
    override_pack["scenes"][0]["water_sky_reflections"][0]["visibility_directions"] = json!(6);
    assert!(load(layered, &override_pack).is_err());
}
fn plain() -> GmdlMaterial {
    GmdlMaterial::new_scene(
        GmdlMaterialParams {
            opacity: 1.,
            alpha_cutoff: 0.,
            mode: 0,
            has_texture: 0,
            uv_offset: Vec2::ZERO,
        },
        None,
        false,
        GmdlMode::Opaque,
    )
}
#[test]
fn disabled_never_reads_assets_and_invalid_schema_is_diagnostic() {
    let p = load_with_reader(
        br#"{"schema_version":1,"enabled":false,"packs":["missing.json"]}"#,
        |_| panic!("disabled must not load files"),
    )
    .unwrap();
    assert!(p.packs.is_empty());
    for v in [
        json!({"schema_version":2,"enabled":false,"packs":[]}),
        json!({"schema_version":1,"enabled":true,"packs":["../bad"]}),
        json!({"schema_version":1,"enabled":true,"packs":["x","x"]}),
        json!({"schema_version":1,"enabled":true,"packs":[],"typo":1}),
    ] {
        assert!(parse_index(v.to_string().as_bytes()).is_err());
    }
    let index = br#"{"schema_version":1,"enabled":true,"packs":["p.json"]}"#;
    assert!(load_with_reader(index, |_| Err("missing p.json".into()))
        .err()
        .unwrap()
        .contains("p.json"));
    let invalid = json!({"schema_version":1,"id":"bad","scenes":[{"stage":"*","fog":[{"center":[0,0,0],"radius":-1,"color":[1,1,1],"density":0.1}]}]});
    assert!(load_with_reader(index, |_| Ok(invalid.to_string().into_bytes())).is_err());
}

#[test]
fn sky_time_config_accepts_only_complete_and_unambiguous_modes() {
    let load = |sky| {
        let pack = json!({
            "schema_version": 1, "id": "time", "scenes": [{"stage": "sn13", "sky": sky}]
        });
        load_with_reader(
            br#"{"schema_version":1,"enabled":true,"packs":["p.json"]}"#,
            |_| Ok(pack.to_string().into_bytes()),
        )
    };
    for source in [
        json!({"enabled":true, "time":{"mode":"cycle", "start_time":"06:00", "time_scale":60}}),
        json!({"enabled":true, "time":{"mode":"system"}}),
        json!({"enabled":true, "time":{"mode":"fixed", "time":"23:30"}}),
        json!({"enabled":true, "fixed_time":"23:30"}),
    ] {
        assert!(
            sky::scene(&load(source.clone()).unwrap(), "sn13").is_some(),
            "{source}"
        );
    }
    for source in [
        json!({"time":{"mode":"cycle", "start_time":"24:00", "time_scale":60}}),
        json!({"time":{"mode":"cycle", "start_time":"06:00", "time_scale":0}}),
        json!({"time":{"mode":"cycle", "start_time":"06:00", "time_scale":-1}}),
        json!({"time":{"mode":"cycle", "start_time":"06:00"}}),
        json!({"time":{"mode":"system", "time":"06:00"}}),
        json!({"time":{"mode":"fixed", "time":"12:60"}}),
        json!({"time":{"mode":"system"}, "fixed_time":"06:00"}),
    ] {
        assert!(load(source.clone()).is_err(), "{source}");
    }
}

#[test]
fn shipped_sky_presets_use_live_system_time() {
    for (path, stage) in [
        ("graphics-packs/starter/pack.json", "sn13"),
        ("graphics-packs/sn03-mist-ravine/pack.json", "sn03"),
        ("graphics-packs/sn06-moss-valley/pack.json", "sn06"),
        ("graphics-packs/sn06-nextgen-sample/pack.json", "sn06"),
    ] {
        let pack: config::Pack =
            serde_json::from_slice(&std::fs::read(config_root().join(path)).unwrap()).unwrap();
        pack.validate().unwrap();
        let sky = pack
            .scenes
            .iter()
            .find(|scene| scene.stage == stage)
            .unwrap()
            .sky
            .as_ref()
            .unwrap();
        assert!(
            matches!(sky.time, Some(config::SkyTime::System {})),
            "{path}"
        );
    }
}
#[test]
fn starter_resources_are_real_and_surface_scope_and_color_spaces_are_correct() {
    let packs = starter();
    let mut images = Assets::<Image>::default();
    let mut mesh = Plane3d::default().mesh().size(4., 4.).build();
    let positions = mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap().clone();
    let mut material = plain();
    let original = material.param_snapshot();
    let mut other = SceneGraphics::new(Some(&packs), "sn01");
    assert!(other
        .configure(
            "floor",
            Some("sn064.jpg"),
            &mut material,
            &mut mesh,
            true,
            true,
            &mut images
        )
        .is_none());
    assert_eq!(material.param_snapshot(), original);
    let mut selected = SceneGraphics::new(Some(&packs), "sn06");
    selected.configure(
        "floor",
        Some("SN064.JPG"),
        &mut material,
        &mut mesh,
        true,
        true,
        &mut images,
    );
    assert_eq!(material.pbr.x, 1.);
    assert_eq!(material.pbr_textures.x, 1.);
    assert_eq!(
        images
            .get(material.texture.as_ref().unwrap())
            .unwrap()
            .texture_descriptor
            .format,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb
    );
    assert_eq!(
        images
            .get(material.orm_texture.as_ref().unwrap())
            .unwrap()
            .texture_descriptor
            .format,
        bevy::render::render_resource::TextureFormat::Rgba8Unorm
    );
    assert_eq!(
        images
            .get(material.env_specular.as_ref().unwrap())
            .unwrap()
            .texture_descriptor
            .mip_level_count,
        6
    );
    assert_eq!(
        format!("{positions:?}"),
        format!("{:?}", mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap())
    );
    let mut duplicate = plain();
    selected.configure(
        "floor",
        Some("sn064.jpg"),
        &mut duplicate,
        &mut mesh,
        true,
        true,
        &mut images,
    );
    assert_eq!(
        material.env_specular, duplicate.env_specular,
        "one environment asset per stage"
    );
    let mut smoke = plain();
    smoke.mode = GmdlMode::Add;
    selected.configure(
        "floor",
        Some("sn064.jpg"),
        &mut smoke,
        &mut mesh,
        false,
        false,
        &mut images,
    );
    assert_eq!(smoke.pbr.x, 0.);
}
impl GmdlMaterial {
    fn param_snapshot(&self) -> [f32; 6] {
        [
            self.pbr.x,
            self.params.opacity,
            self.params.alpha_cutoff,
            self.normal_strength,
            self.normal_contrast,
            self.pbr_textures.w,
        ]
    }
}
#[test]
fn frame_configuration_clears_on_scene_exit_and_keeps_gameplay_resources_read_only() {
    use crate::app_state::{AppState, LoadedStageInfo};
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
        .insert_state(AppState::Playing)
        .insert_resource(starter())
        .init_resource::<FrameUniforms>()
        .insert_resource(LoadedStageInfo {
            stage_id: "sn06".into(),
            ..default()
        })
        .init_resource::<crate::plugins::stage_collider::StageCollider>()
        .add_systems(Update, runtime::update_frame);
    let before = app
        .world()
        .resource::<crate::plugins::stage_collider::StageCollider>()
        .mesh
        .is_none();
    app.update();
    assert_eq!(
        app.world().resource::<FrameUniforms>().graphics.counts.x,
        1.
    );
    app.insert_resource(crate::app_state::Settings {
        volumetric_fog: false,
        ..default()
    });
    app.update();
    assert_eq!(
        app.world().resource::<FrameUniforms>().graphics.counts.x,
        0.
    );
    app.world_mut()
        .resource_mut::<crate::app_state::Settings>()
        .volumetric_fog = true;
    app.update();
    assert_eq!(
        app.world().resource::<FrameUniforms>().graphics.counts.x,
        1.
    );
    app.world_mut().resource_mut::<LoadedStageInfo>().stage_id = "sn03".into();
    app.update();
    assert_eq!(
        app.world().resource::<FrameUniforms>().graphics.counts.x,
        0.
    );
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Menu);
    app.update();
    assert_eq!(
        app.world().resource::<FrameUniforms>().graphics,
        GraphicsFrame::default()
    );
    assert_eq!(
        app.world()
            .resource::<crate::plugins::stage_collider::StageCollider>()
            .mesh
            .is_none(),
        before
    );
}

#[test]
fn atmosphere_lighting_requires_the_replaced_sky_and_restores_pack_lighting() {
    use crate::app_state::{AppState, LoadedStageInfo, Settings};
    let packs = starter();
    let (profile, light, _) = sky::scene(&packs, "sn13").unwrap();
    let atmosphere = sky::atmosphere_lighting(&profile, &light);
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
        .insert_state(AppState::Playing)
        .insert_resource(packs)
        .insert_resource(Settings {
            environment_lighting: false,
            ..default()
        })
        .insert_resource(LoadedStageInfo {
            stage_id: "sn13".into(),
            ..default()
        })
        .init_resource::<FrameUniforms>()
        .add_systems(Update, runtime::update_frame);

    let dome = app.world_mut().spawn(sky::CloudSky(None)).id();
    app.update();
    let baseline = app.world().resource::<FrameUniforms>().graphics;
    assert_eq!(baseline.counts.w, 0.0);
    app.world_mut()
        .entity_mut(dome)
        .insert(sky::CloudSky(Some(atmosphere)));
    app.update();
    let applied = app.world().resource::<FrameUniforms>().graphics;
    assert_eq!(applied.counts.w, 1.0);
    assert_eq!(applied.sky.truncate(), atmosphere.sky);
    assert_eq!(applied.sun, atmosphere.sun);
    assert_ne!(applied.sky, baseline.sky);
    assert_ne!(applied.sun, baseline.sun);
    app.world_mut().despawn(dome);
    app.update();
    assert_eq!(app.world().resource::<FrameUniforms>().graphics, baseline);
}

#[test]
fn fog_color_follows_day_night_without_changing_density() {
    use crate::app_state::{AppState, LoadedStageInfo, Settings};
    let packs = starter();
    let (profile, light, _) = sky::scene(&packs, "sn13").unwrap();
    let mut atmosphere = sky::atmosphere_lighting(&profile, &light);
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
        .insert_state(AppState::Playing)
        .insert_resource(packs)
        .insert_resource(Settings {
            volumetric_fog: true,
            ..default()
        })
        .insert_resource(LoadedStageInfo {
            stage_id: "sn06".into(),
            ..default()
        })
        .init_resource::<FrameUniforms>()
        .add_systems(Update, runtime::update_frame);
    atmosphere.night_factor = 0.0;
    let dome = app.world_mut().spawn(sky::CloudSky(Some(atmosphere))).id();
    app.update();
    let day = app.world().resource::<FrameUniforms>().graphics.fog_colors[0];
    atmosphere.night_factor = 1.0;
    app.world_mut()
        .entity_mut(dome)
        .insert(sky::CloudSky(Some(atmosphere)));
    app.update();
    let night = app.world().resource::<FrameUniforms>().graphics.fog_colors[0];
    assert!(night.truncate().length() < day.truncate().length() * 0.3);
    assert_eq!(night.w, day.w);
}

#[test]
fn grass_uses_active_light_direction_and_keeps_legacy_fallback() {
    let mut frame = GraphicsFrame::default();
    assert_eq!(
        frame.grass_light_direction(),
        Vec4::new(0.78, 0.63, 1.0, 0.0)
    );
    frame.counts.w = 1.0;
    frame.sun = Vec4::new(1.0, 0.5, 0.0, 1.0);
    assert_eq!(frame.grass_light_direction().xy(), Vec2::X);
    frame.sun = Vec4::new(0.0, 0.5, -1.0, 1.0);
    assert_eq!(frame.grass_light_direction().xy(), Vec2::NEG_Y);
    frame.sun = Vec4::Y;
    assert!(frame.grass_light_direction().is_finite());
    assert_eq!(frame.grass_light_direction().z, 0.0);
}
#[test]
fn wind_is_bounded_and_shared_across_display_consumers() {
    let wind = config::Wind::default();
    for t in 0..1000 {
        let v = wind_at(&wind, Vec3::new(3., 4., 5.), t as f64 / 60.);
        assert!(v.is_finite());
        assert!(v.length() <= wind.strength * (1. + wind.gust) + 1e-5);
    }
    let wrap = std::f64::consts::TAU / f64::from(wind.frequency);
    assert!(
        (wind_at(&wind, Vec3::ZERO, wrap - 0.00001) - wind_at(&wind, Vec3::ZERO, wrap + 0.00001))
            .length()
            < 0.0001
    );
    assert_eq!(
        wind_at(&wind, Vec3::ZERO, 12.),
        wind_at(&wind, Vec3::ZERO, 12.)
    );
}

#[test]
fn stage_wind_overrides_and_cumulative_drift_follow_the_same_gust() {
    let mut packs = starter();
    let wind = packs.wind("sn13").unwrap();
    assert_eq!(wind.strength, 0.7);
    assert_eq!(packs.wind("sn03").unwrap().strength, 0.35);
    let (time, step) = (1.25, 0.0001);
    let derivative = ((wind_travel(wind, time + step, 1.0) - wind_travel(wind, time - step, 1.0))
        / (2.0 * step))
        .as_vec2();
    assert!((derivative - wind_at(wind, Vec3::ZERO, time)).length() < 1e-5);
    let mut reverse = wind.clone();
    reverse.direction = wind.direction.map(|value| -value);
    assert_eq!(
        wind_travel(&reverse, time, 2.0),
        -wind_travel(wind, time, 2.0)
    );
    reverse.strength = 0.0;
    assert_eq!(wind_travel(&reverse, time, 2.0), bevy::math::DVec2::ZERO);
    let scene = packs.packs[0]
        .config
        .scenes
        .iter_mut()
        .find(|scene| scene.stage == "sn13")
        .unwrap();
    scene
        .wind
        .as_mut()
        .unwrap()
        .response
        .as_mut()
        .unwrap()
        .cloud = 4.01;
    assert!(packs.packs[0].config.validate().is_err());
}

#[cfg(feature = "original-assets")]
#[test]
fn sn18_cloth_stays_anchored_and_reverses_with_level_wind() {
    use crate::app_state::{AppState, LoadedStageInfo};
    use crate::plugins::render_plugin::build_scene_mesh;
    use bevy::mesh::VertexAttributeValues;

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let bytes = match std::fs::read(root.join("sn18/sn18.gmb")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("{error}"),
    };
    let model = meteor_asset::gmdl::decode_gmdl(&bytes).unwrap();
    let packs = starter();
    let mut scene = SceneGraphics::new(Some(&packs), "sn18");
    let object = model
        .objects
        .iter()
        .find(|object| object.name == "Plane72")
        .unwrap();
    let faces: Vec<u32> = object
        .face_mat
        .iter()
        .zip(object.indices.chunks_exact(3))
        .filter(|(material, _)| {
            model
                .resolve_texture_name(&model.materials[**material as usize])
                .is_some_and(|name| name.eq_ignore_ascii_case("km3cloth.jpg"))
        })
        .flat_map(|(_, face)| face.iter().copied())
        .collect();
    assert_eq!(faces.len(), 30);
    let (mut mesh, transform) = build_scene_mesh(object, &faces, [0.0; 3], None);
    let cloth = scene
        .configure(
            &object.name,
            Some("km3cloth.jpg"),
            &mut plain(),
            &mut mesh,
            false,
            true,
            &mut Assets::default(),
        )
        .unwrap();
    let read = |mesh: &Mesh| {
        let Some(VertexAttributeValues::Float32x3(values)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("sn18 positions")
        };
        values.clone()
    };
    let baseline = read(&mesh);
    let mut app = App::new();
    app.insert_resource(packs)
        .insert_resource(State::new(AppState::Playing))
        .insert_resource(LoadedStageInfo {
            stage_id: "sn18".into(),
            ..default()
        })
        .insert_resource(Time::<()>::default())
        .init_resource::<FrameUniforms>()
        .init_resource::<Assets<Mesh>>()
        .add_systems(Update, surfaces::update_wind);
    let handle = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
    app.world_mut().spawn((
        Mesh3d(handle.clone()),
        GlobalTransform::from(transform),
        cloth,
    ));
    let capture = |app: &mut App| {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(40));
        app.update();
        read(app.world().resource::<Assets<Mesh>>().get(&handle).unwrap())
    };
    let blown = capture(&mut app);
    let mut pack = app.world_mut().resource_mut::<GraphicsPacks>();
    let wind = pack.packs[0].config.scenes[0].wind.as_mut().unwrap();
    wind.direction = wind.direction.map(|axis| -axis);
    let reversed = capture(&mut app);
    let mut anchored = 0;
    let mut bent = 0;
    for vertex in faces.into_iter().map(|i| i as usize) {
        let original = Vec3::from_array(baseline[vertex]);
        let first = Vec3::from_array(blown[vertex]) - original;
        let second = Vec3::from_array(reversed[vertex]) - original;
        if transform.transform_point(original).y >= 36.65 {
            assert_eq!((first, second), (Vec3::ZERO, Vec3::ZERO));
            anchored += 1;
        } else if first.length() > 0.05 {
            assert!(first.normalize_or_zero().dot(second.normalize_or_zero()) < -0.999);
            bent += 1;
        }
    }
    assert!(anchored > 0 && bent > 0);
    app.world_mut().resource_mut::<GraphicsPacks>().packs[0]
        .config
        .scenes[0]
        .wind
        .as_mut()
        .unwrap()
        .strength = 0.0;
    assert_eq!(capture(&mut app), baseline);
}
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
mod gpu;

#[test]
fn contact_shadows_skip_hidden_dead_airborne_and_leave_fighters_and_collision_unchanged() {
    use crate::{
        app_state::{AppState, LoadedStageInfo},
        components::fighter::{Concealment, Fighter, Health, MotionState},
        plugins::stage_collider::{StageCollider, StageMeshCollider},
    };
    let mut collider = StageMeshCollider::default();
    collider.append_world_triangles(
        &[
            [-10., 0., -10.],
            [-10., 0., 10.],
            [10., 0., 10.],
            [10., 0., -10.],
        ],
        &[0, 1, 2, 0, 2, 3],
    );
    let before = format!("{collider:?}");
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
        .insert_state(AppState::Playing)
        .insert_resource(starter())
        .insert_resource(LoadedStageInfo {
            stage_id: "sn06".into(),
            ..default()
        })
        .insert_resource(StageCollider {
            mesh: Some(collider),
        })
        .init_resource::<FrameUniforms>()
        .add_systems(Update, runtime::update_frame);
    let mut actors = vec![];
    for i in 0..4 {
        let position = Vec3::new(i as f32, if i == 3 { 4. } else { 0. }, 0.);
        let actor = app
            .world_mut()
            .spawn((
                Fighter {
                    character_id: 0,
                    player_slot: i,
                    weapon: 6,
                    weapon_slots: [6, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                GlobalTransform::from_translation(position),
                Transform::from_translation(position),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::new(1., 2., 3.),
                    facing: 0.5,
                },
                Health {
                    current: if i == 2 { 0. } else { 100. },
                    max: 100.,
                },
                Concealment {
                    remaining: if i == 1 { 1. } else { 0. },
                },
            ))
            .id();
        actors.push((actor, position));
    }
    app.update();
    assert_eq!(
        app.world().resource::<FrameUniforms>().graphics.counts.y,
        1.
    );
    for (actor, position) in actors {
        assert_eq!(
            app.world().get::<Transform>(actor).unwrap().translation,
            position
        );
        assert_eq!(
            app.world().get::<MotionState>(actor).unwrap().velocity,
            Vec3::new(1., 2., 3.)
        );
    }
    assert_eq!(
        format!(
            "{:?}",
            app.world()
                .resource::<StageCollider>()
                .mesh
                .as_ref()
                .unwrap()
        ),
        before
    );
}

#[test]
fn selected_wind_mesh_bends_only_display_and_freezes_when_paused() {
    use crate::app_state::{AppState, LoadedStageInfo};
    use bevy::mesh::VertexAttributeValues;
    let manifest = json!({"schema_version":1,"id":"wind-test","scenes":[{"stage":"sn06","wind":{"strength":1,"gust":0},"surfaces":[{"object":"test-flag","wind":{"amplitude":0.25,"start_height":0,"end_height":1}}]}]});
    let packs = load_with_reader(
        br#"{"schema_version":1,"enabled":true,"packs":["pack.json"]}"#,
        |_| Ok(manifest.to_string().into_bytes()),
    )
    .unwrap();
    let mut mesh = Mesh::from(Cuboid::new(2., 2., 2.));
    let mut images = Assets::<Image>::default();
    let wind = SceneGraphics::new(Some(&packs), "sn06")
        .configure(
            "test-flag",
            None,
            &mut plain(),
            &mut mesh,
            false,
            true,
            &mut images,
        )
        .unwrap();
    let baseline = wind.positions.clone();
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
        .insert_state(AppState::Playing)
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f32(1. / 30.),
        ))
        .insert_resource(packs)
        .insert_resource(LoadedStageInfo {
            stage_id: "sn06".into(),
            ..default()
        })
        .init_resource::<Assets<Mesh>>()
        .add_systems(Update, surfaces::update_wind);
    let handle = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
    let entity = app
        .world_mut()
        .spawn((Mesh3d(handle.clone()), GlobalTransform::IDENTITY, wind))
        .id();
    for _ in 0..4 {
        app.update();
    }
    let positions = |app: &App| -> Vec<[f32; 3]> {
        let meshes = app.world().resource::<Assets<Mesh>>();
        let Some(VertexAttributeValues::Float32x3(v)) = meshes
            .get(&handle)
            .unwrap()
            .attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions")
        };
        v.clone()
    };
    let bent = positions(&app);
    assert_ne!(bent, baseline);
    for (p, b) in bent.iter().zip(&baseline).filter(|(_, b)| b[1] <= 0.) {
        assert_eq!(p, b, "base remains anchored");
    }
    assert_eq!(
        app.world()
            .get::<surfaces::WindMesh>(entity)
            .unwrap()
            .positions,
        baseline
    );
    assert_eq!(
        *app.world().get::<GlobalTransform>(entity).unwrap(),
        GlobalTransform::IDENTITY
    );
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Paused);
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(positions(&app), bent);
    let converted = GlobalTransform::from(
        Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
            .with_scale(Vec3::splat(0.12)),
    );
    app.world_mut().entity_mut(entity).insert(converted);
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Playing);
    for _ in 0..4 {
        app.update();
    }
    let changed = positions(&app);
    let mut moved = 0;
    for (p, b) in changed.iter().zip(&baseline) {
        let old_world = converted.transform_point(Vec3::from_array(*b));
        let new_world = converted.transform_point(Vec3::from_array(*p));
        assert!(
            (old_world.y - new_world.y).abs() < 1e-6,
            "horizontal wind preserves world height"
        );
        if old_world.y <= 0. {
            assert_eq!(p, b, "Z-up lower half stays anchored");
        } else {
            assert!(new_world.distance(old_world) > 0.001);
            moved += 1;
        }
    }
    assert!(moved > 0);
}

#[test]
fn malformed_resources_fail_before_use_and_color_mips_preserve_energy() {
    assert!(super::loader::decode_resource(b"not a png").is_err());
    let index = br#"{"schema_version":1,"enabled":true,"packs":["p.json"]}"#;
    let manifest = json!({"schema_version":1,"id":"bad","materials":{"x":{"normal":"missing"}}});
    assert!(
        load_with_reader(index, |_| Ok(manifest.to_string().into_bytes()))
            .err()
            .unwrap()
            .contains("unknown resource")
    );
    super::surfaces::test_mips();
}

#[test]
fn cloth_config_accepts_xpbd_and_rejects_unstable_values_and_legacy_springs() {
    let index = br#"{"schema_version":1,"enabled":true,"packs":["p.json"]}"#;
    for cloth in [
        json!({"density":0}),
        json!({"iterations":0}),
        json!({"iterations":9}),
        json!({"max_characters":0}),
        json!({"max_characters":17}),
        json!({"stretch_compliance":-0.1}),
        json!({"bend_compliance":10001}),
        json!({"thickness":0}),
        json!({"motion_limit":9}),
        json!({"damping":30}),
        json!({"frequency_scale":1}),
    ] {
        let pack =
            json!({"schema_version":1,"id":"cloth","scenes":[{"stage":"*","clothing":cloth}]});
        assert!(load_with_reader(index, |_| Ok(pack.to_string().into_bytes())).is_err());
    }
    let config = starter().packs[0].config.scenes[0]
        .clothing
        .clone()
        .unwrap();
    assert_eq!(config.iterations, 4);
    assert_eq!(config.density, 0.25);
}
