use crate::{
    pack_manifest::{parse_pack_index, PackDescriptor},
    PakArchive,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const CACHE_BYTES: usize = 128 * 1024 * 1024;
struct CachedPack {
    id: String,
    size: usize,
    archive: Arc<PakArchive>,
}

pub struct PackRepository {
    root: PathBuf,
    manifest: String,
    descriptors: HashMap<String, PackDescriptor>,
    entries: HashMap<String, String>,
    cache: Mutex<VecDeque<CachedPack>>,
}

impl PackRepository {
    pub fn open(root: &Path) -> Result<Self, String> {
        let manifest = std::fs::read_to_string(root.join("packs.json"))
            .map_err(|e| format!("MPK manifest {}: {e}", root.display()))?;
        let descriptors = parse_pack_index(&manifest)?;
        let json: serde_json::Value = serde_json::from_str(&manifest).map_err(|e| e.to_string())?;
        let mut entries = HashMap::new();
        for pack in json["packs"].as_array().ok_or("packs array missing")? {
            let id = pack["packId"].as_str().ok_or("packId missing")?;
            let names = pack["entries"]
                .as_array()
                .ok_or_else(|| format!("pack {id}: entry index missing; rebuild resource packs"))?;
            for value in names {
                let name = value.as_str().ok_or("invalid entry name")?;
                if name.starts_with('/')
                    || name.contains('\\')
                    || name.split('/').any(|p| p == ".." || p.is_empty())
                    || name != name.to_lowercase()
                {
                    return Err(format!("pack {id}: unsafe/noncanonical entry {name}"));
                }
                if let Some(previous) = entries.insert(name.to_string(), id.to_string()) {
                    return Err(format!("duplicate entry {name} in {previous} and {id}"));
                }
            }
        }
        Ok(Self {
            root: root.into(),
            manifest,
            descriptors,
            entries,
            cache: Mutex::new(VecDeque::new()),
        })
    }

    pub fn retain_stage(&self, stage: &str) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.retain(|p| !p.id.starts_with("stage/") || p.id == format!("stage/{stage}"));
        }
    }
    pub fn manifest(&self) -> &str {
        &self.manifest
    }
    pub fn contains(&self, entry: &str) -> bool {
        self.entries.contains_key(&entry.to_lowercase())
    }
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.entries.keys().cloned().collect();
        names.sort();
        names
    }
    pub fn pack(&self, id: &str) -> Result<Arc<PakArchive>, String> {
        let mut cache = self.cache.lock().map_err(|_| "MPK cache poisoned")?;
        if let Some(index) = cache.iter().position(|p| p.id == id) {
            let hit = cache.remove(index).unwrap();
            let result = hit.archive.clone();
            cache.push_back(hit);
            return Ok(result);
        }
        let descriptor = self
            .descriptors
            .get(id)
            .ok_or_else(|| format!("unknown pack {id}"))?;
        let bytes = std::fs::read(self.root.join(&descriptor.file_name))
            .map_err(|e| format!("pack {id}: {e}"))?;
        verify_bytes(descriptor, &bytes)?;
        let archive =
            Arc::new(PakArchive::from_mpk1(&bytes).map_err(|e| format!("pack {id}: {e}"))?);
        let mut actual: Vec<_> = archive.entries().map(|e| e.name.clone()).collect();
        actual.sort();
        let mut declared: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, p)| p.as_str() == id)
            .map(|(name, _)| name.clone())
            .collect();
        declared.sort();
        if actual != declared {
            return Err(format!("pack {id}: entry index differs from container"));
        }
        while !cache.is_empty()
            && cache.iter().map(|p| p.size).sum::<usize>() + bytes.len() > CACHE_BYTES
        {
            cache.pop_front();
        }
        cache.push_back(CachedPack {
            id: id.into(),
            size: bytes.len(),
            archive: archive.clone(),
        });
        Ok(archive)
    }
    pub fn read(&self, entry: &str) -> Result<Vec<u8>, String> {
        let name = entry.to_lowercase();
        let id = self
            .entries
            .get(&name)
            .ok_or_else(|| format!("MPK entry missing: {name}"))?;
        self.pack(id)?
            .read_entry(&name)
            .ok_or_else(|| format!("pack {id}: indexed entry missing: {name}"))
    }
    pub fn verify_all(&self) -> Result<(), String> {
        for id in self.descriptors.keys() {
            self.pack(id)?;
        }
        for name in self.names() {
            self.read(&name)?;
        }
        Ok(())
    }
}

pub fn verify_bytes(descriptor: &PackDescriptor, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() != descriptor.size {
        return Err(format!("pack {}: size mismatch", descriptor.file_name));
    }
    if format!("sha256:{:x}", Sha256::digest(bytes)) != descriptor.hash {
        return Err(format!("pack {}: SHA-256 mismatch", descriptor.file_name));
    }
    Ok(())
}

pub fn default_pack_directory() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("METEOR_PACKS_DIR") {
        return Ok(path.into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(exe
        .parent()
        .ok_or("executable parent missing")?
        .join("packs"))
}
