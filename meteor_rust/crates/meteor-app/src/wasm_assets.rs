#![cfg(target_arch = "wasm32")]

use crate::app_state::{AppState, LaunchRequest};
use crate::pack_manifest::{match_pack_roots, parse_pack_index, resolve_pack_order};
use bevy::prelude::*;
use js_sys::Uint8Array;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::Response;

#[derive(Debug, Clone)]
enum LoadPhase {
    Idle,
    Loading,
    Ready(String),
    Failed(String, String),
}

struct LoaderShared {
    phase: LoadPhase,
    loaded: HashSet<String>,
    generation: u64,
}

impl Default for LoaderShared {
    fn default() -> Self {
        Self {
            phase: LoadPhase::Idle,
            loaded: HashSet::new(),
            generation: 0,
        }
    }
}

#[derive(Resource, Clone, Default)]
pub struct WasmAssetLoader(Arc<Mutex<LoaderShared>>);

fn match_key(request: &LaunchRequest) -> String {
    format!("{}:p{}", request.stage_id, request.character_id)
}

async fn response(url: &str) -> Result<Response, String> {
    let window = web_sys::window().ok_or_else(|| "browser window unavailable".to_string())?;
    let value = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|error| format!("fetch {url}: {error:?}"))?;
    let response: Response = value
        .dyn_into()
        .map_err(|_| format!("fetch {url}: invalid Response"))?;
    if !response.ok() {
        return Err(format!("fetch {url}: HTTP {}", response.status()));
    }
    Ok(response)
}

async fn fetch_text(url: &str) -> Result<String, String> {
    let response = response(url).await?;
    JsFuture::from(
        response
            .text()
            .map_err(|error| format!("read {url}: {error:?}"))?,
    )
    .await
    .map_err(|error| format!("read {url}: {error:?}"))?
    .as_string()
    .ok_or_else(|| format!("read {url}: response is not text"))
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let response = response(url).await?;
    let buffer = JsFuture::from(
        response
            .array_buffer()
            .map_err(|error| format!("read {url}: {error:?}"))?,
    )
    .await
    .map_err(|error| format!("read {url}: {error:?}"))?;
    Ok(Uint8Array::new(&buffer).to_vec())
}

async fn sha256(bytes: &[u8]) -> Result<String, String> {
    let window = web_sys::window().ok_or_else(|| "browser window unavailable".to_string())?;
    let crypto = window
        .crypto()
        .map_err(|error| format!("Web Crypto unavailable: {error:?}"))?;
    let input = Uint8Array::from(bytes);
    let digest = crypto
        .subtle()
        .digest_with_str_and_buffer_source("SHA-256", &input)
        .map_err(|error| format!("SHA-256: {error:?}"))?;
    let buffer = JsFuture::from(digest)
        .await
        .map_err(|error| format!("SHA-256: {error:?}"))?;
    let digest = Uint8Array::new(&buffer).to_vec();
    Ok(format!(
        "sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

async fn load_root_packs(
    roots: Vec<String>,
    already_loaded: &HashSet<String>,
    generation: Option<(Arc<Mutex<LoaderShared>>, u64)>,
) -> Result<HashSet<String>, String> {
    let index = match meteor_asset::runtime_assets::manifest() {
        Some(index) => index,
        None => {
            let index = fetch_text("./packs/packs.json").await?;
            parse_pack_index(&index)?;
            meteor_asset::runtime_assets::pin_manifest(index.clone())?;
            index
        }
    };
    let descriptors = parse_pack_index(&index)?;
    let ordered = resolve_pack_order(&descriptors, &roots)?;

    let mut loaded = already_loaded.clone();
    for id in ordered {
        if loaded.contains(&id) && meteor_asset::runtime_assets::contains_pack(&id) {
            continue;
        }
        let descriptor = descriptors
            .get(&id)
            .ok_or_else(|| format!("packs.json: pack {id} disappeared"))?;

        let pack_url = descriptor
            .url
            .strip_prefix("/packs/")
            .map(|name| format!("./packs/{name}"))
            .unwrap_or_else(|| descriptor.url.clone());
        let bytes = fetch_bytes(&pack_url).await?;
        if bytes.len() != descriptor.size {
            return Err(format!(
                "pack {id}: size {} != declared {}",
                bytes.len(),
                descriptor.size
            ));
        }
        let actual_hash = sha256(&bytes).await?;
        if actual_hash != descriptor.hash {
            return Err(format!(
                "pack {id}: hash {actual_hash} != declared {}",
                descriptor.hash
            ));
        }
        if let Some((shared, expected)) = &generation {
            if shared.lock().map_err(|_| "asset lock poisoned")?.generation != *expected {
                return Err("resource load cancelled".into());
            }
        }
        meteor_asset::runtime_assets::register_mpk1(&bytes)
            .map_err(|error| format!("pack {id}: {error}"))?;
        loaded.insert(id);
    }
    Ok(loaded)
}

pub fn prepare_match_assets(
    request: Res<LaunchRequest>,
    loader: Res<WasmAssetLoader>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let key = match_key(&request);
    let mut shared = match loader.0.lock() {
        Ok(shared) => shared,
        Err(_) => {
            error!("WASM asset loader lock poisoned");
            next_state.set(AppState::Error);
            return;
        }
    };
    match &shared.phase {
        LoadPhase::Loading => return,
        LoadPhase::Ready(active) if active == &key => return,
        LoadPhase::Failed(active, error) if active == &key => {
            error!("WASM assets failed for {active}: {error}");
            next_state.set(AppState::Error);
            return;
        }
        _ => {}
    }
    let stage_id = request.stage_id.clone();
    let loaded = shared.loaded.clone();
    let generation = shared.generation;
    shared.phase = LoadPhase::Loading;
    drop(shared);

    let target = loader.0.clone();
    spawn_local(async move {
        let roots = match_pack_roots(&stage_id);
        let result = load_root_packs(roots, &loaded, Some((target.clone(), generation))).await;
        if let Ok(mut shared) = target.lock() {
            if shared.generation != generation {
                return;
            }
            match result {
                Ok(loaded) => {
                    meteor_asset::runtime_assets::retain_stage(&stage_id);
                    shared.loaded.extend(loaded);
                    shared
                        .loaded
                        .retain(|id| meteor_asset::runtime_assets::contains_pack(id));
                    shared.phase = LoadPhase::Ready(key);
                }
                Err(error) => shared.phase = LoadPhase::Failed(key, error),
            }
        }
    });
}

pub fn prepare_menu_assets(loader: Res<WasmAssetLoader>) {
    let mut shared = match loader.0.lock() {
        Ok(shared) => shared,
        Err(_) => return,
    };
    if shared.loaded.contains("music") {
        return;
    }
    match &shared.phase {
        LoadPhase::Loading => return,
        LoadPhase::Failed(active, _) if active == "menu" => return,
        _ => {}
    }
    let loaded = shared.loaded.clone();
    let generation = shared.generation;
    shared.phase = LoadPhase::Loading;
    drop(shared);
    let target = loader.0.clone();
    spawn_local(async move {
        let result = load_root_packs(
            vec!["music".to_string()],
            &loaded,
            Some((target.clone(), generation)),
        )
        .await;
        if let Ok(mut shared) = target.lock() {
            if shared.generation != generation {
                return;
            }
            match result {
                Ok(loaded) => {
                    shared.loaded.extend(loaded);
                    shared.phase = LoadPhase::Ready("menu".to_string());
                }
                Err(error) => shared.phase = LoadPhase::Failed("menu".to_string(), error),
            }
        }
    });
}

pub fn menu_music_ready(loader: Res<WasmAssetLoader>) -> bool {
    loader
        .0
        .lock()
        .map(|shared| shared.loaded.contains("music"))
        .unwrap_or(false)
}

pub fn match_assets_ready(request: Res<LaunchRequest>, loader: Res<WasmAssetLoader>) -> bool {
    let key = match_key(&request);
    loader
        .0
        .lock()
        .map(|shared| matches!(&shared.phase, LoadPhase::Ready(active) if active == &key))
        .unwrap_or(false)
}

pub(crate) async fn load_network_models(
    characters: &[u8],
    loader: WasmAssetLoader,
) -> Result<(), String> {
    let loaded = loader
        .0
        .lock()
        .map_err(|_| "资源加载锁失效".to_owned())?
        .loaded
        .clone();
    let loaded = load_root_packs(
        crate::pack_manifest::network_model_pack_roots(characters),
        &loaded,
        None,
    )
    .await?;
    loader
        .0
        .lock()
        .map_err(|_| "资源加载锁失效".to_owned())?
        .loaded
        .extend(loaded);
    Ok(())
}

pub async fn bootstrap() -> Result<(), String> {
    load_root_packs(vec!["startup".into()], &HashSet::new(), None)
        .await
        .map(|_| ())
}

pub fn cancel_pending_assets(loader: Res<WasmAssetLoader>) {
    if let Ok(mut shared) = loader.0.lock() {
        if matches!(shared.phase, LoadPhase::Loading) {
            shared.generation = shared.generation.wrapping_add(1);
            shared.phase = LoadPhase::Idle;
            meteor_asset::runtime_assets::retain_stage("");
            shared
                .loaded
                .retain(|id| meteor_asset::runtime_assets::contains_pack(id));
        }
    }
}
