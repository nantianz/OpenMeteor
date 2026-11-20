use super::*;
fn fixture(stage: &str) -> Inputs {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let archives: Vec<_> = [
        format!("{stage}.pak"),
        "TexturePatch.pak".into(),
        "ctexture.pak".into(),
        "ptexture.pak".into(),
    ]
    .iter()
    .filter_map(|n| std::fs::read(root.join(n)).ok())
    .map(|bytes| meteor_asset::pak::PakArchive::from_bytes(&bytes).unwrap())
    .collect();
    let refs: Vec<_> = archives.iter().collect();
    Inputs::read(
        stage,
        |n| std::fs::read(root.join(n)).map_err(|e| e.to_string()),
        |n| crate::plugins::pak_textures::resolve_pak_texture_exact(&refs, n),
    )
    .unwrap()
}
#[test]
fn native_bake_real_water_cache_and_invalidation() {
    let inputs = fixture("sn13");
    let s = super::super::config::WaterConfig::bundled()
        .surfaces
        .into_iter()
        .find(|s| s.stage == "sn13")
        .unwrap();
    let pack: crate::plugins::graphics_pack::config::Pack =
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/graphics-packs/starter/pack.json"
        )))
        .unwrap();
    let scene = pack
        .scenes
        .iter()
        .find(|scene| scene.stage == s.stage)
        .unwrap();
    let profile = &scene.water_sky_reflections[0];
    let skybox = &scene.sky.as_ref().unwrap().skybox;
    let sky = Some(WaterSkySelection { profile, skybox });
    let root = std::env::temp_dir().join(format!("water-real-bake-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut progress = Vec::new();
    let (maps, hit) =
        super::super::bake_cache::cached(&inputs, &s, sky, &root, |p| progress.push(p)).unwrap();
    assert!(!hit);
    assert_eq!(progress.last(), Some(&1.0));
    assert!(progress.windows(2).all(|p| p[0] <= p[1]));
    let original = super::super::source("sn13", "Plane02", "sn13w01.jpg").unwrap();
    for (a, b) in maps.bounds.iter().zip(original.bounds) {
        assert!((a - b).abs() < 0.001);
    }
    assert!((maps.vertex_alpha - original.vertex_alpha).abs() < 1e-5);
    let reflection = maps.reflection.as_ref().unwrap();
    assert!(reflection.valid(390, 260));
    assert!(reflection.rgba.chunks_exact(4).any(|p| p[3] == 255));
    let skyline = maps.sky_visibility.as_ref().unwrap();
    let cells = profile.visibility_resolution;
    let tiles = profile.visibility_directions / 4;
    assert!(skyline.valid(cells * tiles, cells));
    let north = |x, y| skyline.rgba[((y * tiles + 3) * cells + x) * 4];
    assert_eq!(
        north(9usize, 10usize),
        255,
        "bridge underside stays occluded"
    );
    assert!(
        north(5usize, 25usize) < 120,
        "open channel sees sky above its wall"
    );
    assert!(reflection
        .rgba
        .chunks_exact(4)
        .any(|p| p[3] > 0 && p[..3] != [0, 0, 0]));
    let depth = maps.depth.as_ref().unwrap();
    let water: Vec<_> = depth.rgba.chunks_exact(4).filter(|p| p[2] == 255).collect();
    assert!(!water.is_empty());
    assert!(water.iter().all(|p| p[1] == 255));
    assert!(water.iter().any(|p| p[3] < 64));
    assert!(water.iter().any(|p| p[3] > 240));
    let (_, hit) = super::super::bake_cache::cached(&inputs, &s, sky, &root, |_| {
        panic!("cache hit must not bake")
    })
    .unwrap();
    assert!(hit);
    let key = inputs.cache_key(&s, sky);
    assert_ne!(key, inputs.cache_key(&s, None));
    let mut style = profile.clone();
    style.grazing_boost = 0.0;
    assert_eq!(
        key,
        inputs.cache_key(
            &s,
            Some(WaterSkySelection {
                profile: &style,
                skybox
            })
        )
    );
    style.visibility_resolution = 32;
    assert_ne!(
        key,
        inputs.cache_key(
            &s,
            Some(WaterSkySelection {
                profile: &style,
                skybox
            })
        )
    );
    let mut changed = s.clone();
    changed.extinction = 3.0;
    assert_eq!(key, inputs.cache_key(&changed, sky));
    changed.max_depth = 6.0;
    assert_ne!(key, inputs.cache_key(&changed, sky));
    changed = s.clone();
    changed.reflection_face = 64;
    assert_ne!(key, inputs.cache_key(&changed, sky));
    let mut changed_inputs = fixture("sn13");
    changed_inputs.hash.update(b"changed source texture");
    assert_ne!(key, changed_inputs.cache_key(&s, sky));
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn smoke_cannot_be_baked_by_a_mistyped_entry() {
    let inputs = fixture("sn13");
    let mut s = super::super::config::WaterConfig::bundled().surfaces[2].clone();
    s.object = "missing-object".into();
    assert!(inputs.bake(&s, None, |_| {}).is_err());
}
#[test]
fn all_configured_liquids_have_valid_geometry_and_no_extra_maps_for_lava() {
    for s in super::super::config::WaterConfig::bundled().surfaces {
        let inputs = fixture(&s.stage);
        let (water, _, _) = inputs.triangles(&s, &[]).unwrap();
        assert!(!water.triangles.is_empty());
        if !s.needs_maps() {
            let result = inputs.bake(&s, None, |_| {}).unwrap();
            assert!(result.depth.is_none() && result.reflection.is_none());
        }
    }
}

#[test]
fn release_pack_inputs_bake_without_original_files_or_external_tools() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools/packs");
    let packs = meteor_asset::pack_repository::PackRepository::open(&root).unwrap();
    let stage = "sn13";
    let inputs = Inputs::read(
        stage,
        |name| {
            let entry = meteor_asset::runtime_assets::entry_name_for_legacy_path(name).unwrap();
            packs.read(&entry)
        },
        |name| {
            let normalized = name.replace('\\', "/");
            let base = normalized.rsplit('/').next().unwrap();
            [stage, "patch", "ctexture", "ptexture"]
                .iter()
                .find_map(|scope| packs.read(&format!("texture/{scope}/{base}")).ok())
        },
    )
    .unwrap();
    assert!(
        !inputs.textures.is_empty(),
        "published packs must contain bake textures"
    );
    let mut surface = super::super::config::WaterConfig::bundled().surfaces[2].clone();
    surface.reflection_face = 32;
    let result = inputs.bake(&surface, None, |_| {}).unwrap();
    assert!(result
        .reflection
        .unwrap()
        .rgba
        .chunks_exact(4)
        .any(|p| p[3] > 0));
    assert!(result
        .depth
        .unwrap()
        .rgba
        .chunks_exact(4)
        .any(|p| p[1] == 255));
}
