#![cfg(not(any(target_arch = "wasm32", target_os = "android")))]

use meteor_app::plugins::pak_textures::PakCache;
use meteor_asset::{original_directory::OriginalDirectory, runtime_assets};
use std::path::Path;

#[test]
fn original_scene_texture_chain_uses_indexed_entries_without_unpacking_the_archive() {
    let Some(root) = std::env::var_os("METEOR_TEST_ORIGINAL_DIR") else {
        return;
    };
    let directory = OriginalDirectory::open(Path::new(&root), &[]).unwrap();
    let expected = directory.read("texture/sn06/asn06wu.jpg").unwrap();
    runtime_assets::initialize_original(directory).unwrap();

    let mut cache = PakCache::default();
    assert!(cache.chain(Path::new(""), "sn06").is_empty());
    assert_eq!(
        cache.resolve(Path::new(""), "sn06", "Asn06wu.jpg"),
        Some(expected)
    );
    assert!(cache
        .resolve(Path::new(""), "sn06", "not-installed.jpg")
        .is_none());
    assert!(runtime_assets::take_original_read_error().is_none());
}
