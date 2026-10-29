#![cfg(feature = "original-assets")]
use meteor_asset::pack_repository::PackRepository;
use std::path::Path;
fn resources() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools/packs")
}

#[test]
fn complete_release_resources_are_indexed_and_verified_without_original_directory() {
    let repository = PackRepository::open(&resources()).unwrap();
    repository.verify_all().unwrap();
    for name in [
        "fonts/ui.ttf",
        "ui/studio-logo.png",
        "image/sheng.bmp",
        "common/meteor.res",
        "common/character.act",
        "common/character.amb",
        "sfx/pos253.ef",
        "pmodel/p0.skc",
        "pmodel/p19.pos",
        "stage/sn01.gmb",
        "stage/sn22.des",
        "level/sn03.wp",
        "compatibility/stock.json",
    ] {
        assert!(!repository
            .read(name)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .is_empty());
    }
    assert!(repository.read("../origin_res/Meteor.res").is_err());
    assert!(repository.read("missing/not-real.png").is_err());
}

#[test]
fn corrupted_container_fails_before_exposing_entries() {
    let source = resources();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("meteor-mpk-corrupt-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(source.join("packs.json")).unwrap()).unwrap();
    let descriptor = index["packs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["packId"] == "compatibility")
        .unwrap()
        .clone();
    let filename = descriptor["fileName"].as_str().unwrap();
    let mut bytes = std::fs::read(source.join(filename)).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(root.join(filename), bytes).unwrap();
    index["packs"] = serde_json::json!([descriptor]);
    std::fs::write(root.join("packs.json"), serde_json::to_vec(&index).unwrap()).unwrap();
    let repository = PackRepository::open(&root).unwrap();
    assert!(repository
        .read("compatibility/stock.json")
        .unwrap_err()
        .contains("SHA-256 mismatch"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn dependency_cycles_and_unsafe_container_paths_are_rejected() {
    use meteor_asset::pack_manifest::parse_pack_index;
    let item = |id: &str, file: &str, dependencies: &[&str]| {
        serde_json::json!({
            "packId": id, "url": format!("/packs/{file}"), "fileName": file,
            "size": 1, "hash": format!("sha256:{}", "0".repeat(64)), "requires": dependencies
        })
    };
    let cycle = serde_json::json!({"formatVersion":1,"packs":[item("a","a.mpk", &["b"]),item("b","b.mpk", &["a"])]});
    assert!(parse_pack_index(&cycle.to_string())
        .unwrap_err()
        .contains("cycle"));
    let missing = serde_json::json!({"formatVersion":1,"packs":[item("a","a.mpk", &["missing"])]});
    assert!(parse_pack_index(&missing.to_string())
        .unwrap_err()
        .contains("missing"));
    for name in ["../bad.mpk", "C:bad.mpk", "a/b.mpk", "a\\b.mpk"] {
        let index = serde_json::json!({"formatVersion":1,"packs":[item("a",name,&[])]});
        assert!(parse_pack_index(&index.to_string()).is_err(), "{name}");
    }
}

#[test]
fn mobile_registration_deduplicates_and_releases_only_obsolete_stage_containers() {
    use meteor_asset::runtime_assets as runtime;
    let root = resources();
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("packs.json")).unwrap()).unwrap();
    for id in ["compatibility", "stage/sn01", "stage/sn03"] {
        let file = index["packs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["packId"] == id)
            .unwrap()["fileName"]
            .as_str()
            .unwrap();
        let bytes = std::fs::read(root.join(file)).unwrap();
        runtime::register_mpk1(&bytes).unwrap();
        let count = runtime::registered_pack_count();
        runtime::register_mpk1(&bytes).unwrap();
        assert_eq!(runtime::registered_pack_count(), count);
    }
    runtime::retain_stage("sn03");
    assert!(!runtime::contains_pack("stage/sn01"));
    assert!(runtime::contains_pack("stage/sn03"));
    assert!(runtime::contains_pack("compatibility"));
    assert!(runtime::read_legacy_path("sn03/sn03.gmb").is_some());
    assert!(runtime::read_legacy_path("sn01/sn01.gmb").is_none());
    runtime::retain_stage("");
    assert!(!runtime::contains_pack("stage/sn03"));
    assert!(runtime::stock_checksums().is_ok());
}
