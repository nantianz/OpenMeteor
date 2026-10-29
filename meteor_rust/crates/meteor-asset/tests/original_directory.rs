#![cfg(feature = "original-assets")]
#![cfg(not(any(target_arch = "wasm32", target_os = "android")))]
use meteor_asset::original_directory::OriginalDirectory;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "meteor-original-{}-{} 中文目录",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn pak(&self, name: &str, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(self.0.join(name)).unwrap());
        for (name, bytes) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }
    fn complete() -> Self {
        let fixture = Self::new();
        for name in [
            "COMMON/Character.AMB",
            "COMMON/Character.ACT",
            "Meteor.RES",
            "IMAGE/Sheng.BMP",
            "IMAGE/Fu.BMP",
            "IMAGE/He.BMP",
        ] {
            fixture.write(name, b"fixture");
        }
        for hero in 0..20 {
            for ext in ["POS", "BNC", "AMB", "SKC"] {
                fixture.write(&format!("PMODEL/P{hero}.{ext}"), b"line\r\n\xff");
            }
        }
        for stage in 1..=22 {
            for ext in ["GMB", "DES", "PST"] {
                fixture.write(
                    &format!("SN{stage:02}/SN{stage:02}.{ext}"),
                    b"stage\r\n\x80",
                );
            }
        }
        let mut mask = vec![0; 54];
        mask[..2].copy_from_slice(b"BM");
        mask[28] = 8;
        fixture.pak(
            "CTexture.PAK",
            &[
                ("ctexture/Art.JPG", b"colour"),
                ("ctexture/KArt.BMP", &mask),
                ("ctexture/KOrphan.BMP", &mask),
            ],
        );
        for name in ["PTexture.PAK", "ETexture.PAK", "UITexture.PAK"] {
            fixture.pak(name, &[("nested/Other.TGA", b"texture")]);
        }
        fixture.pak(
            "Waves.PAK",
            &[("WAVES/Hit.WAV", b"old"), ("WAVES/Only.WAV", b"original")],
        );
        fixture.pak("TexturePatch.PAK", &[("Patch/Art.JPG", b"patched colour")]);
        fixture.write("PATCH/HIT.wav", b"patched sound");
        fixture
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn direct_source_preserves_bytes_aliases_masks_and_patch_overrides_without_packs() {
    let fixture = Fixture::complete();
    let source = OriginalDirectory::open(
        &fixture.0,
        &[
            ("fonts/ui.ttf", b"bundled font"),
            ("ui/studio-logo.png", b"logo"),
        ],
    )
    .unwrap();
    source.verify_all().unwrap();
    assert_eq!(source.read(r"pmodel\p0.pos").unwrap(), b"line\r\n\xff");
    assert_eq!(source.read("common/meteor.res").unwrap(), b"fixture");
    assert_eq!(source.read("stage/sn01.pst").unwrap(), b"stage\r\n\x80");
    assert_eq!(source.read("texture/ctexture/ART.jpg").unwrap(), b"colour");
    assert_eq!(
        source.read("texture/patch/art.jpg").unwrap(),
        b"patched colour"
    );
    assert_eq!(source.read("sound/hit.wav").unwrap(), b"patched sound");
    assert_eq!(source.read("sound/only.wav").unwrap(), b"original");
    assert_eq!(source.read("fonts/ui.ttf").unwrap(), b"bundled font");
    let masks: serde_json::Value =
        serde_json::from_slice(&source.read("texture-masks/ctexture.json").unwrap()).unwrap();
    assert_eq!(masks["masks"], serde_json::json!(["art"]));
    let metadata: serde_json::Value =
        serde_json::from_slice(&source.read("compatibility/stock.json").unwrap()).unwrap();

    assert_eq!(metadata["heroes"][0]["checksum"], 1170);
    assert_eq!(metadata["stages"][0]["checksum"], 679);
    assert!(!fixture.0.join("packs").exists());
    for name in [
        "../Meteor.RES",
        "/meteor.res",
        "C:\\Meteor.res",
        "texture/../meteor.res",
    ] {
        assert!(source.read(name).is_err(), "{name}");
    }
    assert!(source.read("sound/missing.wav").is_err());
    meteor_asset::runtime_assets::initialize_original(source).unwrap();
    assert!(meteor_asset::runtime_assets::initialize_native(Path::new("missing-packs")).is_err());
    assert_eq!(
        meteor_asset::runtime_assets::read_file("Common/Character.ACT").unwrap(),
        b"fixture"
    );
    assert_eq!(
        meteor_asset::runtime_assets::read_file("stage/sn01.pst").unwrap(),
        b"stage\r\n\x80"
    );
    assert!(meteor_asset::runtime_assets::read_file("waves.pak").is_ok());
    assert_eq!(
        meteor_asset::runtime_assets::stock_checksums().unwrap()[0],
        1170
    );
    assert_eq!(
        meteor_asset::runtime_assets::stock_stage_checksums().unwrap()["sn01"],
        679
    );
}

#[test]
fn missing_and_corrupt_resources_report_the_source_path() {
    let fixture = Fixture::new();
    let error = OriginalDirectory::open(&fixture.0, &[]).err().unwrap();
    assert!(error.contains("pmodel/p0.pos"));
    assert!(error.contains("中文目录"));
    fixture.write("ctexture.pak", b"broken zip");
    assert!(OriginalDirectory::open(&fixture.0, &[])
        .err()
        .unwrap()
        .contains("ctexture.pak"));
}

#[test]
fn reopening_recomputes_metadata_from_the_selected_installation() {
    let fixture = Fixture::complete();
    let first = OriginalDirectory::open(&fixture.0, &[])
        .unwrap()
        .read("compatibility/stock.json")
        .unwrap();
    fixture.write("PMODEL/P0.POS", b"different\n");
    let second = OriginalDirectory::open(&fixture.0, &[])
        .unwrap()
        .read("compatibility/stock.json")
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(
        fs::read(fixture.0.join("PMODEL/P0.POS")).unwrap(),
        b"different\n"
    );
}

#[test]
fn repository_original_resources_match_existing_packed_profile_and_decode_raw_textures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let original = root.join("origin_res");
    if !original.is_dir() {
        return;
    }
    let source = OriginalDirectory::open(&original, &[]).unwrap();
    source.verify_all().unwrap();
    let packs = root.join("asset-tools/packs");
    if packs.join("packs.json").is_file() {
        let packed = meteor_asset::pack_repository::PackRepository::open(&packs).unwrap();
        let actual: serde_json::Value =
            serde_json::from_slice(&source.read("compatibility/stock.json").unwrap()).unwrap();
        let expected: serde_json::Value =
            serde_json::from_slice(&packed.read("compatibility/stock.json").unwrap()).unwrap();
        assert_eq!(actual, expected);
        for name in packed.names().into_iter().filter(|n| {
            n.starts_with("texture/")
                && [".bmp", ".jpg", ".jpeg", ".png", ".tga", ".dds"]
                    .iter()
                    .any(|ext| n.ends_with(ext))
        }) {
            let raw = source.read(&name).unwrap();
            let decoded =
                meteor_asset::texture::decode_image(&raw).unwrap_or_else(|e| panic!("{name}: {e}"));
            let converted =
                meteor_asset::texture::decode_image(&packed.read(&name).unwrap()).unwrap();
            assert_eq!(
                (decoded.width, decoded.height),
                (converted.width, converted.height),
                "{name}"
            );
            assert!(
                decoded.rgba == converted.rgba,
                "pixel mismatch: {name}; first difference {:?}",
                decoded
                    .rgba
                    .iter()
                    .zip(&converted.rgba)
                    .enumerate()
                    .find(|(_, (a, b))| a != b)
            );
        }
    }
}
