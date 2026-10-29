#![cfg(target_os = "android")]

use crate::app_state::LaunchRequest;
use crate::pack_manifest::{
    match_pack_roots, parse_pack_index, resolve_pack_order, PackDescriptor,
};
use bevy::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::io::Read;

#[derive(Resource, Default, Clone)]
pub(crate) struct AndroidAssetLoader {
    descriptors: Option<HashMap<String, PackDescriptor>>,
    loaded: HashSet<String>,
    active_key: Option<String>,
    error: Option<String>,
}

impl AndroidAssetLoader {
    pub(crate) fn error_for(&self, request: &LaunchRequest) -> Option<&str> {
        let key = match_key(request);
        (self.active_key.as_deref() == Some(key.as_str()))
            .then(|| self.error.as_deref())
            .flatten()
    }
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<AndroidAssetLoader>();
}

fn match_key(request: &LaunchRequest) -> String {
    format!("{}:p{}", request.stage_id, request.character_id)
}

fn read_asset(path: &str) -> Result<Vec<u8>, String> {
    let app = bevy::android::ANDROID_APP
        .get()
        .ok_or_else(|| "Android app handle is not initialized".to_string())?;
    let path_c = CString::new(path).map_err(|_| format!("invalid Android asset path: {path}"))?;
    let mut asset = app
        .asset_manager()
        .open(&path_c)
        .ok_or_else(|| format!("Android asset missing: {path}"))?;
    if let Ok(bytes) = asset.buffer() {
        return Ok(bytes.to_vec());
    }
    let mut bytes = Vec::with_capacity(asset.length());
    asset
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read Android asset {path}: {error}"))?;
    Ok(bytes)
}

pub(crate) fn read_authoring_asset(stage_id: &str, path: &str) -> Option<Vec<u8>> {
    use std::sync::{Mutex, OnceLock};
    type EntryCache = HashMap<(String, String), Option<Vec<u8>>>;
    static CACHE: OnceLock<Mutex<EntryCache>> = OnceLock::new();
    let key = (stage_id.to_string(), path.to_string());
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    if let Some(bytes) = cache.get(&key) {
        return bytes.clone();
    }
    let index = read_asset("packs/packs.json").ok()?;
    let descriptors = parse_pack_index(std::str::from_utf8(&index).ok()?).ok()?;
    let entry = meteor_asset::runtime_assets::entry_name_for_legacy_path(path)?;
    let result = [format!("stage/{stage_id}"), "common".to_string()]
        .into_iter()
        .find_map(|id| {
            let descriptor = descriptors.get(&id)?;
            let bytes = read_asset(&format!("packs/{}", descriptor.file_name)).ok()?;
            if bytes.len() != descriptor.size
                || format!("sha256:{:x}", Sha256::digest(&bytes)) != descriptor.hash
            {
                return None;
            }
            meteor_asset::pak::PakArchive::from_mpk1(&bytes)
                .ok()?
                .read_entry(&entry)
        });
    cache.insert(key, result.clone());
    result
}

pub(crate) fn authoring_manifest() -> Option<String> {
    String::from_utf8(read_asset("packs/packs.json").ok()?).ok()
}

fn descriptors(loader: &mut AndroidAssetLoader) -> Result<HashMap<String, PackDescriptor>, String> {
    if loader.descriptors.is_none() {
        let bytes = read_asset("packs/packs.json")?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|error| format!("packs/packs.json is not UTF-8: {error}"))?;
        loader.descriptors = Some(parse_pack_index(text)?);
        meteor_asset::runtime_assets::pin_manifest(text.to_string())?;
    }
    Ok(loader.descriptors.clone().unwrap_or_default())
}

fn load_root_packs(roots: &[String], loader: &mut AndroidAssetLoader) -> Result<(), String> {
    let descriptors = descriptors(loader)?;
    let ordered = resolve_pack_order(&descriptors, roots)?;
    let before = loader.loaded.len();
    let mut bytes_loaded = 0usize;
    for id in ordered {
        if loader.loaded.contains(&id) && meteor_asset::runtime_assets::contains_pack(&id) {
            continue;
        }
        let descriptor = descriptors
            .get(&id)
            .ok_or_else(|| format!("packs.json: pack {id} disappeared"))?;
        let path = format!("packs/{}", descriptor.file_name);
        let bytes = read_asset(&path)?;
        if bytes.len() != descriptor.size {
            return Err(format!(
                "pack {id}: size {} != declared {}",
                bytes.len(),
                descriptor.size
            ));
        }
        let actual_hash = format!("sha256:{:x}", Sha256::digest(&bytes));
        if actual_hash != descriptor.hash {
            return Err(format!(
                "pack {id}: hash {actual_hash} != declared {}",
                descriptor.hash
            ));
        }
        meteor_asset::runtime_assets::register_mpk1(&bytes)
            .map_err(|error| format!("pack {id}: {error}"))?;
        bytes_loaded += bytes.len();
        loader.loaded.insert(id);
    }
    info!(
        "Android MPK assets ready: {} new packs, {} total, {:.1} MiB read",
        loader.loaded.len().saturating_sub(before),
        loader.loaded.len(),
        bytes_loaded as f64 / (1024.0 * 1024.0)
    );
    Ok(())
}

pub(crate) fn load_menu_assets(mut loader: ResMut<AndroidAssetLoader>) {
    if loader.loaded.contains("music") {
        return;
    }
    if let Err(error) = load_root_packs(&["music".to_string()], &mut loader) {
        warn!("Android menu assets unavailable: {error}");
    }
}

pub(crate) fn load_match_assets(
    request: Res<LaunchRequest>,
    mut loader: ResMut<AndroidAssetLoader>,
) {
    let key = match_key(&request);
    if loader.active_key.as_deref() == Some(key.as_str()) && loader.error.is_none() {
        return;
    }
    let result = load_root_packs(&match_pack_roots(&request.stage_id), &mut loader);
    if result.is_ok() {
        loader
            .loaded
            .retain(|id| meteor_asset::runtime_assets::contains_pack(id));
    }
    loader.active_key = Some(key.clone());
    loader.error = result.err();
    if let Some(error) = &loader.error {
        error!("Android match assets failed for {key}: {error}");
    }
}

pub(crate) fn load_network_models(
    characters: &[u8],
    loader: &mut AndroidAssetLoader,
) -> Result<(), String> {
    load_root_packs(
        &crate::pack_manifest::network_model_pack_roots(characters),
        loader,
    )
}

pub(crate) fn bootstrap() -> Result<(), String> {
    load_root_packs(
        &["startup".into(), "common".into(), "skeleton".into()],
        &mut AndroidAssetLoader::default(),
    )
}
