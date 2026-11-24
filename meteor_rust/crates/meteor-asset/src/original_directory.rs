use crate::runtime_assets::ROOM_STAGE_EXTENSIONS;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};

const CACHE_BYTES: usize = 128 * 1024 * 1024;
const MAX_CACHED_ENTRY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
enum Entry {
    File(PathBuf),
    Zip { archive: String, index: usize },
    Generated(Vec<u8>),
    Bundled(&'static [u8]),
}

pub struct OriginalDirectory {
    root: PathBuf,
    entries: BTreeMap<String, Entry>,
    archives: Mutex<BTreeMap<String, zip::ZipArchive<File>>>,
    cache: Mutex<VecDeque<(String, Vec<u8>)>>,
}

impl OriginalDirectory {
    pub fn open(root: &Path, supplemental: &[(&str, &'static [u8])]) -> Result<Self, String> {
        let root = root
            .canonicalize()
            .map_err(|e| format!("Original directory {}: {e}", root.display()))?;
        if !root.is_dir() {
            return Err(format!(
                "Original directory is not a directory: {}",
                root.display()
            ));
        }
        let mut files = BTreeMap::new();
        index_files(&root, &root, &mut files)?;
        let mut result = Self {
            root,
            entries: files
                .iter()
                .map(|(name, path)| (name.clone(), Entry::File(path.clone())))
                .collect(),
            archives: Mutex::new(BTreeMap::new()),
            cache: Mutex::new(VecDeque::new()),
        };

        if let Some(entry) = result.entries.get("meteor.res").cloned() {
            result.entries.insert("common/meteor.res".into(), entry);
        }
        for (name, path) in &files {
            if let Some((dir, file)) = name.split_once('/') {
                if is_stage(dir) && !file.contains('/') {
                    result
                        .entries
                        .insert(format!("stage/{file}"), Entry::File(path.clone()));
                }
            } else if name.ends_with(".pak") {
                result.index_archive(name, path)?;
            }
        }
        for (name, path) in &files {
            if let Some(file) = name
                .strip_prefix("patch/")
                .filter(|f| f.ends_with(".wav") && !f.contains('/'))
            {
                result
                    .entries
                    .insert(format!("sound/{file}"), Entry::File(path.clone()));
            }
        }
        for &(name, bytes) in supplemental {
            if !name.starts_with("ui/") && name != "fonts/ui.ttf" {
                return Err(format!("Invalid supplemental UI resource: {name}"));
            }

            result
                .entries
                .entry(name.into())
                .or_insert(Entry::Bundled(bytes));
        }
        result.validate_required()?;
        let compatibility = result.compatibility()?;
        result.entries.insert(
            "compatibility/stock.json".into(),
            Entry::Generated(compatibility),
        );
        result.add_mask_lists()?;
        Ok(result)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn stage_ids(&self) -> Vec<String> {
        self.entries.keys()
            .filter_map(|name| {
                let (id, file) = name.split_once('/')?;
                (is_stage(id) && id != "sn00" && file == format!("{id}.gmb"))
                    .then_some(id)
            })
            .filter(|id| ROOM_STAGE_EXTENSIONS.iter().all(|ext| {
                matches!(self.entries.get(&format!("{id}/{id}.{ext}")), Some(Entry::File(path)) if path.is_file())
            }))
            .map(str::to_owned)
            .collect()
    }

    pub fn has_mission(&self, stage: &str) -> bool {
        is_stage(stage)
            && self.entries.contains_key(&format!("level/{stage}_.pst"))
            && ROOM_STAGE_EXTENSIONS
                .iter()
                .all(|ext| self.entries.contains_key(&format!("{stage}/{stage}.{ext}")))
    }

    pub fn stage_preview_revision(&self, stage: &str) -> String {
        let mut hash = Sha256::new();
        hash.update(self.root.as_os_str().as_encoded_bytes());
        let stage_path = format!("{stage}/");
        for (name, entry) in &self.entries {
            if !(name.starts_with(&stage_path)
                || name.starts_with("cmodel/")
                || name == &format!("{stage}.pak")
                || name == &format!("level/{stage}.wp")
                || matches!(
                    name.as_str(),
                    "meteor.res" | "ctexture.pak" | "texturepatch.pak"
                ))
            {
                continue;
            }
            if let Entry::File(path) = entry {
                hash.update(name.as_bytes());
                if let Ok(metadata) = path.metadata() {
                    hash.update(metadata.len().to_le_bytes());
                    if let Ok(modified) = metadata.modified().and_then(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .map_err(std::io::Error::other)
                    }) {
                        hash.update(modified.as_nanos().to_le_bytes());
                    }
                }
            }
        }
        format!("{:x}", hash.finalize())
    }

    fn index_archive(&mut self, name: &str, path: &Path) -> Result<(), String> {
        let scope = match name {
            "texturepatch.pak" => "patch",
            "waves.pak" => "sound",
            _ => name.trim_end_matches(".pak"),
        };
        if ![
            "patch",
            "sound",
            "ctexture",
            "ptexture",
            "etexture",
            "uitexture",
        ]
        .contains(&scope)
            && !is_stage(scope)
        {
            return Ok(());
        }
        let mut archive = zip::ZipArchive::new(File::open(path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Original PAK {}: {e}", path.display()))?;
        for index in 0..archive.len() {
            let file = archive
                .by_index_raw(index)
                .map_err(|e| format!("{name}: {e}"))?;
            if file.is_dir() {
                continue;
            }
            let normalized = file.name().replace('\\', "/").to_lowercase();
            let base = normalized.rsplit('/').next().unwrap_or_default();
            if base.is_empty() || base.ends_with(".db") {
                continue;
            }
            let key = if scope == "sound" {
                if !base.ends_with(".wav") {
                    continue;
                }
                format!("sound/{base}")
            } else {
                format!("texture/{scope}/{base}")
            };
            self.entries.entry(key).or_insert(Entry::Zip {
                archive: name.into(),
                index,
            });
        }
        self.archives
            .get_mut()
            .map_err(|_| "Original archive lock poisoned")?
            .insert(name.into(), archive);
        Ok(())
    }

    fn validate_required(&self) -> Result<(), String> {
        let mut names: Vec<String> = [
            "common/character.amb",
            "common/character.act",
            "meteor.res",
            "ctexture.pak",
            "ptexture.pak",
            "etexture.pak",
            "uitexture.pak",
            "waves.pak",
            "image/sheng.bmp",
            "image/fu.bmp",
            "image/he.bmp",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        for hero in 0..20 {
            for ext in ["pos", "bnc", "amb", "skc"] {
                names.push(format!("pmodel/p{hero}.{ext}"));
            }
        }
        let missing: Vec<_> = names
            .into_iter()
            .filter(|name| !self.entries.contains_key(name))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "Original directory {} is missing required resources: {}",
                self.root.display(),
                missing.join(", ")
            ));
        }
        Ok(())
    }

    fn compatibility(&self) -> Result<Vec<u8>, String> {
        let mut heroes = Vec::new();
        for hero in 0..20 {
            let entry = format!("pmodel/p{hero}.pos");
            let bytes = self.read(&entry)?;
            let hash = format!("{:x}", Sha256::digest(&bytes));
            heroes.push(serde_json::json!({"hero":hero,"entry":entry,"checksum":checksum(&bytes),"sourceHash":hash,"packedHash":hash}));
        }
        let mut stages = Vec::new();
        for id in self.stage_ids() {
            let name = format!("stage/{id}.pst");
            let bytes = self.read(&format!("{id}/{id}.pst"))?;
            stages.push(serde_json::json!({"stage":id,"entry":name,"length":bytes.len(),"checksum":checksum(&bytes),"packedHash":format!("{:x}", Sha256::digest(&bytes))}));
        }
        serde_json::to_vec(&serde_json::json!({"version":1,"profile":"source-bytes","heroes":heroes,"stages":stages})).map_err(|e| e.to_string())
    }

    fn add_mask_lists(&mut self) -> Result<(), String> {
        let scopes: BTreeSet<_> = self
            .entries
            .keys()
            .filter_map(|name| {
                name.strip_prefix("texture/")?
                    .split_once('/')
                    .map(|(scope, _)| scope.to_owned())
            })
            .collect();
        let mut masks: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for scope in &scopes {
            let prefix = format!("texture/{scope}/");
            let stems: BTreeSet<_> = self
                .entries
                .keys()
                .filter_map(|name| {
                    name.strip_prefix(&prefix)?
                        .rsplit_once('.')
                        .map(|(stem, _)| stem)
                })
                .collect();
            for name in self
                .entries
                .keys()
                .filter(|name| name.starts_with(&prefix) && name.ends_with(".bmp"))
            {
                let base = name.strip_prefix(&prefix).unwrap().trim_end_matches(".bmp");
                let Some(stem) = base.strip_prefix("_k").or_else(|| base.strip_prefix('k')) else {
                    continue;
                };
                if !stems.contains(stem) {
                    continue;
                }
                if self.is_palette_mask(name)? {
                    masks.entry(scope.clone()).or_default().insert(stem.into());
                }
            }
        }
        for scope in scopes {
            let chain: Vec<&str> = match scope.as_str() {
                "uitexture" => vec!["uitexture"],
                "etexture" => vec!["etexture", "patch"],
                s if is_stage(s) => vec![s, "patch", "ctexture", "ptexture"],
                _ => vec!["patch", "ctexture", "ptexture"],
            };
            let combined: BTreeSet<_> = chain
                .into_iter()
                .filter_map(|s| masks.get(s))
                .flatten()
                .collect();
            let bytes = serde_json::to_vec(&serde_json::json!({"masks":combined}))
                .map_err(|e| e.to_string())?;
            self.entries.insert(
                format!("texture-masks/{scope}.json"),
                Entry::Generated(bytes),
            );
        }
        Ok(())
    }

    pub fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        self.read_if_present(name)?
            .ok_or_else(|| format!("Original resource missing: {name}"))
    }

    pub(crate) fn read_if_present(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let name = normalize(name)?;
        let name = match name.as_str() {
            "character.amb" | "character.act" => format!("common/{name}"),
            _ => name,
        };
        let Some(entry) = self.entries.get(&name) else {
            return Ok(None);
        };
        let bytes = match entry {
            Entry::File(path) => {
                fs::read(path).map_err(|e| format!("Original resource {}: {e}", path.display()))
            }
            Entry::Generated(bytes) => Ok(bytes.clone()),
            Entry::Bundled(bytes) => Ok(bytes.to_vec()),
            Entry::Zip { archive, index } => {
                let mut cache = self
                    .cache
                    .lock()
                    .map_err(|_| "Original resource cache poisoned")?;
                if let Some(index) = cache.iter().position(|(key, _)| key == &name) {
                    let hit = cache.remove(index).unwrap();
                    let bytes = copy_entry(&hit.1);
                    cache.push_back(hit);
                    return bytes
                        .map(Some)
                        .map_err(|error| format!("Original PAK {archive} / {name}: {error}"));
                }
                let bytes = self.with_zip_entry(archive, *index, &name, read_zip_entry)?;
                if bytes.len() <= MAX_CACHED_ENTRY_BYTES {
                    while cache.iter().map(|(_, b)| b.len()).sum::<usize>() + bytes.len()
                        > CACHE_BYTES
                    {
                        cache.pop_front();
                    }
                    if let Ok(cached) = copy_entry(&bytes) {
                        cache.push_back((name, cached));
                    }
                }
                Ok(bytes)
            }
        }?;
        Ok(Some(bytes))
    }

    fn with_zip_entry<T>(
        &self,
        archive: &str,
        index: usize,
        name: &str,
        read: impl FnOnce(&mut zip::read::ZipFile<'_>) -> std::io::Result<T>,
    ) -> Result<T, String> {
        let mut archives = self
            .archives
            .lock()
            .map_err(|_| "Original archive lock poisoned")?;
        let zip = archives
            .get_mut(archive)
            .ok_or("Original archive missing")?;
        let mut file = zip
            .by_index(index)
            .map_err(|error| format!("Original PAK {archive} / {name}: {error}"))?;
        read(&mut file).map_err(|error| format!("Original PAK {archive} / {name}: {error}"))
    }

    fn is_palette_mask(&self, name: &str) -> Result<bool, String> {
        if let Some(Entry::Zip { archive, index }) = self.entries.get(name) {
            return self.with_zip_entry(archive, *index, name, |file| {
                if file.size() <= 30 {
                    return Ok(false);
                }
                let mut header = [0u8; 30];
                file.read_exact(&mut header)?;
                Ok(is_mask_header(&header))
            });
        }
        self.read(name)
            .map(|bytes| bytes.len() > 30 && is_mask_header(&bytes))
    }

    pub fn verify_all(&self) -> Result<(), String> {
        for (name, entry) in &self.entries {
            if name.ends_with(".pak") {
                continue;
            }
            let size = if let Entry::Zip { archive, index } = entry {
                self.with_zip_entry(archive, *index, name, |file| {
                    let expected = file.size();
                    let size = std::io::copy(
                        &mut file.take(expected.saturating_add(1)),
                        &mut std::io::sink(),
                    )?;
                    if size != expected {
                        return Err(std::io::Error::other(
                            "uncompressed length differs from ZIP directory",
                        ));
                    }
                    Ok(size)
                })?
            } else {
                self.read(name)?.len() as u64
            };
            if size == 0 && (name.ends_with(".pos") || name.ends_with(".gmb")) {
                return Err(format!("Empty original resource: {name}"));
            }
        }
        Ok(())
    }
}

fn copy_entry(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len())
        .map_err(std::io::Error::other)?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}

fn read_zip_entry(file: &mut zip::read::ZipFile<'_>) -> std::io::Result<Vec<u8>> {
    let size = usize::try_from(file.size())
        .map_err(|_| std::io::Error::other("entry exceeds the process address space"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|error| std::io::Error::other(format!("cannot allocate {size} bytes: {error}")))?;
    let mut chunk = [0; 64 * 1024];
    loop {
        let count = file.read(&mut chunk)?;
        if count == 0 && bytes.len() == size {
            return Ok(bytes);
        }
        if count == 0 || count > size - bytes.len() {
            return Err(std::io::Error::other(
                "uncompressed length differs from ZIP directory",
            ));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}

fn is_mask_header(header: &[u8]) -> bool {
    header.starts_with(b"BM") && u16::from_le_bytes([header[28], header[29]]) == 8
}

fn is_stage(name: &str) -> bool {
    name.len() == 4 && name.starts_with("sn") && name.as_bytes()[2..].iter().all(u8::is_ascii_digit)
}

fn normalize(name: &str) -> Result<String, String> {
    let name = name.replace('\\', "/").to_lowercase();
    let mut name = name.as_str();
    while let Some(rest) = name.strip_prefix("./") {
        name = rest;
    }
    if name.is_empty()
        || name.starts_with('/')
        || name.contains(':')
        || name.split('/').any(|part| part == ".." || part.is_empty())
    {
        return Err(format!("Invalid original resource path: {name}"));
    }
    Ok(name.into())
}

fn index_files(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    for item in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let item = item.map_err(|e| e.to_string())?;
        let ty = item.file_type().map_err(|e| e.to_string())?;
        let path = item.path();
        let name = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/")
            .to_lowercase();

        if ty.is_dir()
            && (dir != root
                || [
                    "common",
                    "pmodel",
                    "cmodel",
                    "sfx",
                    "level",
                    "pscriptlib",
                    "music",
                    "fonts",
                    "image",
                    "patch",
                    "texture",
                ]
                .contains(&name.as_str())
                || is_stage(&name))
        {
            index_files(root, &path, files)?;
        } else if ty.is_file()
            && (dir != root || name.ends_with(".pak") || name.ends_with(".res"))
            && files.insert(name.clone(), path).is_some()
        {
            return Err(format!(
                "Ambiguous original resource differing only in case: {name}"
            ));
        }
    }
    Ok(())
}

fn checksum(bytes: &[u8]) -> u32 {
    bytes.iter().enumerate().fold(0u32, |sum, (index, &byte)| {
        sum.wrapping_add((byte as i8 as i32 as u32).wrapping_mul((index + 1) as u32))
    })
}
