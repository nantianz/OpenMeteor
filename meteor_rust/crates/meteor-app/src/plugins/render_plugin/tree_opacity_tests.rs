use super::*;
use crate::app_state::{LoadedStageAssets, LoadedStageInfo, Settings};
use bevy::ecs::system::RunSystemOnce;
use bevy::material::AlphaMode;
use bevy::mesh::VertexAttributeValues;
use bevy::pbr::Material;

fn original_scene() -> (World, GmdlMesh, [f32; 3]) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let read = |name: &str| std::fs::read(root.join(name)).unwrap();
    let gmb = read("sn03/sn03.gmb");
    let model = decode_gmdl(&gmb).unwrap();
    let center = meteor_scene::stage_loader::get_stage_center(
        "sn03",
        Some(&String::from_utf8(read("Level/sn03.wp")).unwrap()),
    );
    let mut world = World::new();
    world.init_resource::<Assets<Mesh>>();
    world.init_resource::<Assets<GmdlMaterial>>();
    world.init_resource::<Assets<StandardMaterial>>();
    world.init_resource::<Assets<Image>>();
    world.init_resource::<StageCollision>();
    world.init_resource::<StageCollider>();
    world.init_resource::<super::super::hazard_plugin::StageHazardRuntime>();
    world.init_resource::<super::super::escort_assets::LoadedEscort>();
    world.init_resource::<super::super::stage_plugin::StageScriptRuntime>();
    world.insert_resource(Settings {
        texture_quality: 2,
        scene_normal_maps: false,
        water_enhance: false,
        ..default()
    });
    world.insert_resource(LoadedStageInfo {
        stage_id: "sn03".into(),
        center,
        ready: true,
        ..default()
    });
    world.insert_resource(LoadedStageAssets {
        gmb: Some(gmb),
        des: Some(read("sn03/sn03.des")),
        cob: Some(read("sn03/sn03.cob")),
        ..default()
    });
    world.run_system_once(setup_stage_scene).unwrap();
    (world, model, center)
}

fn material_for_object(
    world: &mut World,
    model: &GmdlMesh,
    center: [f32; 3],
    name: &str,
) -> GmdlMaterial {
    let object = model.objects.iter().find(|o| o.name == name).unwrap();
    let original = &object.positions[..3];
    let first = Vec3::new(
        original[0] - center[0],
        original[2] - center[2],
        -(original[1] - center[1]),
    ) * MODEL_SCALE;
    let rows: Vec<_> = world
        .query::<(&Mesh3d, &MeshMaterial3d<GmdlMaterial>, &Transform)>()
        .iter(world)
        .map(|(mesh, material, transform)| (mesh.0.clone(), material.0.clone(), *transform))
        .collect();
    let meshes = world.resource::<Assets<Mesh>>();
    let (_, material, _) = rows
        .iter()
        .find(|(mesh, _, transform)| {
            let mesh = meshes.get(mesh).unwrap();
            let Some(VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                return false;
            };
            positions.len() == object.num_verts
                && mesh.indices().unwrap().len() == object.num_faces * 3
                && transform
                    .transform_point(Vec3::from_array(positions[0]))
                    .distance(first)
                    < 0.001
        })
        .unwrap_or_else(|| panic!("production mesh missing: {name}"));
    world
        .resource::<Assets<GmdlMaterial>>()
        .get(material)
        .unwrap()
        .clone()
}

#[cfg(feature = "original-assets")]
#[test]
fn production_sn03_tree_meshes_keep_opaque_bark_and_alpha_tested_canopy() {
    let (mut world, model, center) = original_scene();
    for name in ["Object37", "Object38"] {
        let material = material_for_object(&mut world, &model, center, name);
        assert_eq!(material.mode, GmdlMode::Opaque, "{name}");
        assert_eq!(
            material.params.alpha_cutoff, 0.0,
            "{name} solid trunk must not use a silhouette mask"
        );
        assert_eq!(material.alpha_mode(), AlphaMode::Opaque, "{name}");
        let image = world
            .resource::<Assets<Image>>()
            .get(material.texture.as_ref().unwrap())
            .unwrap();
        assert!(
            image
                .data
                .as_ref()
                .unwrap()
                .chunks_exact(4)
                .all(|p| p[3] == 255),
            "{name} bark alpha"
        );
    }
    let leaves = material_for_object(&mut world, &model, center, "inder40");
    assert!(leaves.two_side);
    assert_eq!(leaves.alpha_mode(), AlphaMode::Mask(127.0 / 255.0));
    let image = world
        .resource::<Assets<Image>>()
        .get(leaves.texture.as_ref().unwrap())
        .unwrap();
    let pixels = image.data.as_ref().unwrap();
    assert!(
        pixels.chunks_exact(4).any(|p| p[3] == 0),
        "canopy background stays cut out"
    );
    assert!(
        pixels.chunks_exact(4).any(|p| p[3] == 255),
        "canopy leaves stay visible"
    );
}

#[cfg(feature = "original-assets")]
#[test]
fn custom_map_hollow_setting_consumes_tga_alpha_from_runtime_pack() {
    let config = br#"{"global":{"enabled":true,"enableHollowEffect":true,"hollowThreshold":0.3}}"#;
    let mut leaf_tga = vec![0; 18];
    leaf_tga[2] = 2;
    leaf_tga[12] = 2;
    leaf_tga[14] = 1;
    leaf_tga[16] = 32;
    leaf_tga[17] = 8;
    leaf_tga.extend([16, 128, 32, 0, 16, 128, 32, 255]);
    let mut leaf_png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_raw(2, 1, vec![32, 128, 16, 0, 32, 128, 16, 255])
        .unwrap()
        .write_to(&mut leaf_png, image::ImageFormat::Png)
        .unwrap();
    let mut solid_tga = leaf_tga.clone();
    solid_tga[21] = 255;
    register_texture_fixture(
        "test-sn95-hollow",
        [
            ("stage/sn95.render.json", config.to_vec()),
            ("texture/sn95/hollow-leaves.tga", leaf_tga),
            ("texture/sn95/hollow-converted.tga", leaf_png.into_inner()),
            ("texture/sn95/solid-trunk.tga", solid_tga),
        ],
    );
    let model = GmdlMesh {
        textures: [
            "Hollow-Leaves.TGA",
            "hollow-converted.tga",
            "solid-trunk.tga",
            "missing.tga",
        ]
        .map(|name| meteor_asset::gmdl::GmdlTexture { name: name.into() })
        .to_vec(),
        materials: [
            (0, "DISABLE 1 0"),
            (1, "DISABLE 1 0"),
            (2, "DISABLE 1 0"),
            (0, "ENABLE sa 1-sa"),
            (3, "DISABLE 1 0"),
        ]
        .map(|(tex_index, state)| RawGmdlMaterial {
            tex_index,
            blend: "NORMAL".into(),
            two_side: true,
            state: state.into(),
            alpha: 1.0,
        })
        .to_vec(),
        objects: Vec::new(),
    };
    let cutoff = legacy_stage_render::StageRenderConfig::load("sn95")
        .unwrap()
        .hollow_cutoff()
        .unwrap();
    assert_eq!(cutoff, 0.3);
    assert!(legacy_stage_render::StageRenderConfig::load("sn03").is_none());
    assert_eq!(
        hollow_gmdl_materials(
            &model,
            cutoff,
            &mut Assets::default(),
            &mut HashMap::new(),
            &mut PakCache::default(),
            std::path::Path::new("."),
            "sn95",
            2,
            &SceneTexturePack::default(),
        ),
        HashSet::from([0, 1])
    );
}
#[cfg(feature = "original-assets")]
#[test]
fn runtime_pack_sn03_bark_ignores_obsolete_mask_metadata() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let path = std::fs::read_dir(root.join("asset-tools/packs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("stage-sn03.")
                && path.extension().is_some_and(|extension| extension == "mpk")
        })
        .expect("generated sn03 MPK fixture");
    let pack = PakArchive::from_bytes(&std::fs::read(path).unwrap()).unwrap();
    let color = pack.read_entry("texture/sn03/sn3tr.JPG").unwrap();
    let mask = pack.read_entry("texture/sn03/ksn3tr.bmp").unwrap();
    assert!(
        mask.starts_with(b"\x89PNG"),
        "runtime BMP entry was converted to PNG"
    );

    register_texture_fixture(
        "test-sn03-tree-opacity",
        [
            ("texture/sn97/sn3tr-opacity-regression.JPG", color.clone()),
            ("texture/sn97/ksn3tr-opacity-regression.bmp", mask),
            (
                "texture-masks/sn97.json",
                br#"{"masks":["sn3tr-opacity-regression"]}"#.to_vec(),
            ),
        ],
    );
    let mut images = Assets::<Image>::default();
    let loaded = load_or_cache_texture(
        &mut images,
        &mut HashMap::new(),
        &mut PakCache::default(),
        &root.join("origin_res"),
        "sn97",
        "sn3tr-opacity-regression.JPG",
        false,
        false,
        2,
    );
    let expected = meteor_asset::texture::decode_image(&color).unwrap();
    let image = images.get(loaded.handle.as_ref().unwrap()).unwrap();
    let data = image.data.as_ref().unwrap();
    assert!(
        data[..expected.rgba.len()] == expected.rgba,
        "registered MPK mask metadata cannot alter an authored opaque JPEG"
    );
    assert!(
        data.chunks_exact(4).all(|pixel| pixel[3] == 255),
        "packed bark and all mip levels must stay opaque"
    );
}
fn register_texture_fixture(
    pack_id: &str,
    entries: impl IntoIterator<Item = (&'static str, Vec<u8>)>,
) {
    let mut payload = Vec::new();
    let mut manifest_entries = Vec::new();
    for (name, data) in entries {
        manifest_entries.push(serde_json::json!({
            "name": name, "off": payload.len(), "len": data.len(), "mime": ""
        }));
        payload.extend(data);
    }
    let manifest = serde_json::to_vec(&serde_json::json!({
        "formatVersion": 1, "kind": "test", "packId": pack_id,
        "entries": manifest_entries
    }))
    .unwrap();
    let mut bytes = b"MPK1".to_vec();
    bytes.extend_from_slice(&(manifest.len() as u32).to_le_bytes());
    bytes.extend(manifest);
    bytes.extend(payload);
    meteor_asset::runtime_assets::register_mpk1(&bytes).unwrap();
}
