#![cfg(feature = "original-assets")]
use super::*;

#[test]
fn remake_mpk_namespace_reaches_the_typed_loader_with_stock_bones() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools/packs");
    let repository = meteor_asset::pack_repository::PackRepository::open(&root).unwrap();
    let bone_bytes = repository.read("pmodel/p0.bnc").unwrap();
    let skeleton =
        Skeleton::from_bone_data(&parse_bones(std::str::from_utf8(&bone_bytes).unwrap()));
    let visual = prepared(
        &skeleton,
        repository.read("pmodel/p0-remake-metadata.json").unwrap(),
        |name| repository.read(&format!("pmodel/p0-remake-{name}")).ok(),
    )
    .unwrap();
    assert_eq!(skeleton.skin_bone_count(), 30);
    assert_eq!(visual.controls.len(), 8);
    assert!(visual.skin().vertex_count > 1290 && visual.skin().triangle_count <= 48000);
    for color in ["neutral", "red", "blue"] {
        let bytes = repository
            .read(&format!("pmodel/p0-remake-low-{color}.jpg"))
            .unwrap();
        let image = meteor_asset::texture::decode_image(&bytes).unwrap();
        assert_eq!((image.width, image.height), (1024, 1024));
    }
}
