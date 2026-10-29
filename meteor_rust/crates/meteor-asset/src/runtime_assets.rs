use crate::pak::PakArchive;
use sha2::{Digest, Sha256};
struct RegisteredPack {
    id: String,
    hash: [u8; 32],
    archive: PakArchive,
}
use std::sync::{OnceLock, RwLock};

static RUNTIME_PACKS: OnceLock<RwLock<Vec<RegisteredPack>>> = OnceLock::new();

fn packs() -> &'static RwLock<Vec<RegisteredPack>> {
    RUNTIME_PACKS.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn register_mpk1(bytes: &[u8]) -> Result<usize, String> {
    let pak = PakArchive::from_mpk1(bytes).map_err(|error| error.to_string())?;
    let entry_count = pak.len();
    let length = u32::from_le_bytes(bytes[4..8].try_into().map_err(|_| "MPK header")?) as usize;
    let manifest: serde_json::Value =
        serde_json::from_slice(&bytes[8..8 + length]).map_err(|e| e.to_string())?;
    let hash: [u8; 32] = Sha256::digest(bytes).into();
    let id = manifest["packId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("fixture-{hash:x?}"));
    let mut packs = packs()
        .write()
        .map_err(|_| "runtime asset registry poisoned".to_string())?;
    if let Some(index) = packs.iter().position(|p| p.id == id) {
        if packs[index].hash == hash {
            return Ok(entry_count);
        }
        packs.remove(index);
    }
    packs.push(RegisteredPack {
        id,
        hash,
        archive: pak,
    });
    Ok(entry_count)
}

pub fn registered_pack_count() -> usize {
    packs().read().map(|packs| packs.len()).unwrap_or_default()
}

pub fn read_entry(name: &str) -> Option<Vec<u8>> {
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    if let Some(source) = ORIGINAL_DIRECTORY.get() {
        return source.read(name).ok();
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(repository) = NATIVE_REPOSITORY.get() {
        if !repository.contains(name) {
            return None;
        }
        return match repository.read(name) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                log::error!("{error}");
                None
            }
        };
    }
    packs()
        .read()
        .ok()?
        .iter()
        .rev()
        .find_map(|pak| pak.archive.read_entry(name))
}

pub fn retain_stage(stage: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(repository) = NATIVE_REPOSITORY.get() {
        repository.retain_stage(stage);
    }
    if let Ok(mut packs) = packs().write() {
        packs.retain(|pack| !pack.id.starts_with("stage/") || pack.id == format!("stage/{stage}"));
    }
}
pub fn contains_pack(id: &str) -> bool {
    packs()
        .read()
        .map(|packs| packs.iter().any(|p| p.id == id))
        .unwrap_or(false)
}

pub fn entry_name_for_legacy_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let lower = normalized.trim_start_matches('/').to_lowercase();
    let mut parts = lower.split('/');
    let head = parts.next()?;
    let tail = parts.collect::<Vec<_>>().join("/");
    if tail.is_empty() {
        return match head {
            "character.amb" | "character.act" => Some(format!("common/{head}")),
            "meteor.res" => Some("common/meteor.res".to_string()),
            _ => None,
        };
    }
    match head {
        "common" => Some(format!("common/{tail}")),
        "pmodel" | "cmodel" | "sfx" | "level" | "pscriptlib" | "music" | "fonts" | "image"
        | "ui" | "compatibility" | "sound" | "texture" | "texture-masks" | "stage" => {
            Some(format!("{head}/{tail}"))
        }
        stage
            if stage.len() == 4
                && stage.starts_with("sn")
                && stage[2..].chars().all(|c| c.is_ascii_digit()) =>
        {
            let basename = tail.rsplit('/').next()?;
            Some(format!("stage/{basename}"))
        }
        stage
            if ["gmb", "des", "cob", "pst"]
                .iter()
                .any(|ext| tail == format!("{stage}.{ext}")) =>
        {
            Some(format!("stage/{tail}"))
        }
        _ => None,
    }
}

pub fn read_legacy_path(path: &str) -> Option<Vec<u8>> {
    read_entry(&entry_name_for_legacy_path(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_only_known_web_pack_paths() {
        assert_eq!(
            entry_name_for_legacy_path("sn01/sn01.GMB").as_deref(),
            Some("stage/sn01.gmb")
        );
        assert_eq!(
            entry_name_for_legacy_path(r"Level\sn01_.pst").as_deref(),
            Some("level/sn01_.pst")
        );
        assert_eq!(
            entry_name_for_legacy_path("pmodel/P12.POS").as_deref(),
            Some("pmodel/p12.pos")
        );
        assert_eq!(
            entry_name_for_legacy_path("Common/character.amb").as_deref(),
            Some("common/character.amb")
        );
        assert!(entry_name_for_legacy_path("ptexture.pak").is_none());
        assert_eq!(
            entry_name_for_legacy_path("editor_trial/editor_trial.des").as_deref(),
            Some("stage/editor_trial.des")
        );
        assert_eq!(
            entry_name_for_legacy_path("Meteor.res").as_deref(),
            Some("common/meteor.res")
        );
        assert_eq!(
            entry_name_for_legacy_path("music/knife.mp3").as_deref(),
            Some("music/knife.mp3")
        );
    }

    #[test]
    fn registered_mpk_is_available_through_legacy_path() {
        let payload = b"runtime-stage";
        let manifest = serde_json::json!({
            "formatVersion": 1,
            "kind": "test",
            "entries": [{
                "name": "stage/sn98.gmb",
                "off": 0,
                "len": payload.len(),
                "mime": ""
            }]
        });
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MPK1");
        bytes.extend_from_slice(&(manifest.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&manifest);
        bytes.extend_from_slice(payload);

        let before = registered_pack_count();
        assert_eq!(register_mpk1(&bytes), Ok(1));
        assert_eq!(registered_pack_count(), before + 1);
        assert_eq!(
            read_legacy_path("sn98/sn98.gmb").as_deref(),
            Some(payload.as_slice())
        );
    }
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
static ORIGINAL_DIRECTORY: OnceLock<crate::original_directory::OriginalDirectory> = OnceLock::new();

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub fn initialize_original(
    source: crate::original_directory::OriginalDirectory,
) -> Result<(), String> {
    if NATIVE_REPOSITORY.get().is_some() {
        return Err("MPK source already initialized; restart to switch resource sources".into());
    }
    ORIGINAL_DIRECTORY
        .set(source)
        .map_err(|_| "Original source already initialized".into())
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub fn original_directory_active() -> bool {
    ORIGINAL_DIRECTORY.get().is_some()
}

#[cfg(not(target_arch = "wasm32"))]
static NATIVE_REPOSITORY: OnceLock<crate::pack_repository::PackRepository> = OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
pub fn initialize_native(root: &std::path::Path) -> Result<(), String> {
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    if original_directory_active() {
        return Err(
            "Original source already initialized; restart to switch resource sources".into(),
        );
    }
    if NATIVE_REPOSITORY.get().is_some() {
        return Ok(());
    }
    let repository = crate::pack_repository::PackRepository::open(root)?;
    repository.read("compatibility/stock.json")?;
    NATIVE_REPOSITORY
        .set(repository)
        .map_err(|_| "MPK repository already initialized".to_string())
}

static PINNED_MANIFEST: OnceLock<String> = OnceLock::new();
pub fn pin_manifest(text: String) -> Result<(), String> {
    if let Some(previous) = PINNED_MANIFEST.get() {
        return if previous == &text {
            Ok(())
        } else {
            Err("resource manifest changed; restart to switch versions".into())
        };
    }
    PINNED_MANIFEST
        .set(text)
        .map_err(|_| "resource manifest already pinned".into())
}
pub fn manifest() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(repository) = NATIVE_REPOSITORY.get() {
        return Some(repository.manifest().into());
    }
    PINNED_MANIFEST.get().cloned()
}

pub fn read_file(path: impl AsRef<std::path::Path>) -> std::io::Result<Vec<u8>> {
    let path = path.as_ref();
    let name = path.to_string_lossy().replace('\\', "/");
    if path.is_absolute() || name.split('/').any(|c| c == "..") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "resource path must be relative to MPK root",
        ));
    }
    let name = name.trim_start_matches("./");
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    if let Some(source) = ORIGINAL_DIRECTORY.get() {
        return source
            .read(name)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::NotFound, error));
    }
    read_legacy_path(name).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("MPK resource missing: {name}"),
        )
    })
}
pub fn read_text_file(path: impl AsRef<std::path::Path>) -> std::io::Result<String> {
    String::from_utf8(read_file(path)?)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
pub fn file_exists(path: impl AsRef<std::path::Path>) -> bool {
    read_file(path).is_ok()
}

pub fn stock_checksums() -> Result<[u32; 20], String> {
    let bytes =
        read_entry("compatibility/stock.json").ok_or("MPK stock compatibility metadata missing")?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if json["version"] != 1 || json["profile"] != "source-bytes" {
        return Err("unsupported stock resource profile".into());
    }
    let heroes = json["heroes"]
        .as_array()
        .filter(|h| h.len() == 20)
        .ok_or("invalid hero metadata")?;
    let mut sums = [0; 20];
    for (i, hero) in heroes.iter().enumerate() {
        if hero["hero"].as_u64() != Some(i as u64) {
            return Err("unordered hero metadata".into());
        }
        sums[i] = hero["checksum"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("invalid stock checksum")?;
    }
    Ok(sums)
}

pub fn stock_stage_checksums() -> Result<std::collections::BTreeMap<String, u32>, String> {
    let bytes =
        read_entry("compatibility/stock.json").ok_or("MPK stock compatibility metadata missing")?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if json["version"] != 1 || json["profile"] != "source-bytes" {
        return Err("unsupported stock resource profile".into());
    }
    let stages = json["stages"]
        .as_array()
        .filter(|s| !s.is_empty())
        .ok_or("missing stage checksum metadata; rebuild MPK packs")?;
    let mut sums = std::collections::BTreeMap::new();
    for stage in stages {
        let id = stage["stage"].as_str().ok_or("invalid stage id")?;
        if id.len() != 4
            || !id.starts_with("sn")
            || !id.as_bytes()[2..].iter().all(u8::is_ascii_digit)
            || stage["entry"].as_str() != Some(format!("stage/{id}.pst").as_str())
        {
            return Err("invalid stage checksum entry".into());
        }
        let checksum = stage["checksum"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("invalid stage checksum")?;
        if sums.insert(id.to_owned(), checksum).is_some() {
            return Err("duplicate stage checksum".into());
        }
    }
    Ok(sums)
}
