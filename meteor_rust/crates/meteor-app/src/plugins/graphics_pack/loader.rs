use super::cloth_binding::{DetailMesh, Pinning};
use super::config::*;
use bevy::prelude::*;
use meteor_asset::texture::DecodedImage;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
use std::path::{Path, PathBuf};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

pub struct LoadedPack {
    pub config: Pack,
    pub images: BTreeMap<String, Arc<DecodedImage>>,
    pub cloth_meshes: BTreeMap<String, super::cloth_obj::ClothObj>,
    pub cloth_details: BTreeMap<(String, String), Arc<DetailMesh>>,
    pub(super) prefix: String,
}
#[derive(Resource, Default)]
pub struct GraphicsPacks {
    pub packs: Vec<LoadedPack>,
    pub error: Option<String>,
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    pub(super) loose_root: Option<PathBuf>,
}

pub fn parse_index(bytes: &[u8]) -> Result<Index, String> {
    if bytes.len() > 1_048_576 {
        return Err("graphics index exceeds 1 MiB".into());
    }
    let index: Index = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if index.schema_version != 1
        || index.packs.len() > 16
        || index.packs.iter().any(|p| !relative_path(p))
    {
        return Err("invalid graphics schema_version/packs".into());
    }
    let unique: HashSet<_> = index.packs.iter().collect();
    if unique.len() != index.packs.len() {
        return Err("duplicate graphics pack path".into());
    }
    Ok(index)
}

pub fn decode_resource(bytes: &[u8]) -> Result<DecodedImage, String> {
    if bytes.len() > 16 * 1024 * 1024 || bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("resource must be a PNG, at most 16 MiB".into());
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return Err("PNG dimensions must be 1..4096".into());
    }
    meteor_asset::texture::decode_image(bytes)
}

pub fn load_with_reader(
    index: &[u8],
    mut read: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<GraphicsPacks, String> {
    let index = parse_index(index)?;
    if !index.enabled {
        return Ok(GraphicsPacks::default());
    }
    let mut packs = vec![];
    let mut ids = HashSet::new();
    let mut decoded_bytes = 0usize;
    for path in index.packs {
        let bytes = read(&path)?;
        if bytes.len() > 1_048_576 {
            return Err(format!("{path}: manifest exceeds 1 MiB"));
        }
        let config: Pack = serde_json::from_slice(&bytes).map_err(|e| format!("{path}: {e}"))?;
        config.validate().map_err(|e| format!("{path}: {e}"))?;
        if !ids.insert(config.id.clone()) {
            return Err(format!("duplicate pack id: {}", config.id));
        }
        let prefix = path.rsplit_once('/').map_or("", |(p, _)| p);
        let mut images = BTreeMap::new();
        for (id, file) in &config.resources {
            let full = if prefix.is_empty() {
                file.clone()
            } else {
                format!("{prefix}/{file}")
            };
            let bytes = read(&full).map_err(|e| format!("{full}: {e}"))?;
            let image = decode_resource(&bytes).map_err(|e| format!("{full}: {e}"))?;
            decoded_bytes += image.rgba.len();
            if decoded_bytes > 256 * 1024 * 1024 {
                return Err("graphics packs exceed 256 MiB decoded texture budget".into());
            }
            images.insert(id.clone(), Arc::new(image));
        }
        let mut cloth_meshes = BTreeMap::new();
        for (id, file) in &config.cloth_meshes {
            let full = if prefix.is_empty() {
                file.clone()
            } else {
                format!("{prefix}/{file}")
            };
            let bytes = read(&full).map_err(|e| format!("{full}: {e}"))?;
            let mesh =
                super::cloth_obj::ClothObj::parse(&bytes).map_err(|e| format!("{full}: {e}"))?;
            cloth_meshes.insert(id.clone(), mesh);
        }
        let mut cloth_details = BTreeMap::new();
        for s in &config.scenes {
            for cloth in s.character_cloth.iter().filter(|cloth| cloth.enabled) {
                if let Some(rigid) = &cloth.rigid_mesh {
                    let material = &config.materials[&rigid.material];
                    if !cloth_meshes[&rigid.id].has_uvs
                        && [&material.base_color, &material.normal, &material.orm]
                            .iter()
                            .any(|id| id.is_some())
                    {
                        return Err(format!(
                            "{path}: textured rigid OBJ requires UVs for all faces"
                        ));
                    }
                }
                if let Some(source) = &cloth.mesh {
                    let mesh = &cloth_meshes[&source.id];
                    Pinning::prepare(mesh, source).map_err(|e| format!("{path}: {e}"))?;
                    let render = if let Some(id) = &source.render_id {
                        let render = &cloth_meshes[id];
                        let key = (source.id.clone(), id.clone());
                        if !cloth_details.contains_key(&key) {
                            if cloth_details.len() == 16 {
                                return Err(format!("{path}: too many cloth mesh pairs"));
                            }
                            let detail = DetailMesh::prepare(mesh, render)
                                .map_err(|e| format!("{path}: {id}: {e}"))?;
                            cloth_details.insert(key, Arc::new(detail));
                        }
                        render
                    } else {
                        mesh
                    };
                    let material = &config.materials[&cloth.material];
                    if !render.has_uvs
                        && [&material.base_color, &material.normal, &material.orm]
                            .iter()
                            .any(|id| id.is_some())
                    {
                        return Err(format!(
                            "{path}: textured cloth OBJ requires UVs for all faces"
                        ));
                    }
                }
            }
            if let Some(e) = &s.environment {
                if let Some(id) = &e.diffuse {
                    let m = &images[id];
                    if m.width != m.height * 2 {
                        return Err(format!("{id}: environment must be 2:1 equirectangular"));
                    }
                }
                let mut size = None;
                for id in &e.specular_mips {
                    let m = &images[id];
                    if m.width != m.height * 2
                        || size.is_some_and(|(w, h)| m.width != w || m.height != h)
                    {
                        return Err(format!(
                            "{id}: specular mips must be 2:1 and halve each level"
                        ));
                    }
                    size = Some(((m.width / 2).max(1), (m.height / 2).max(1)));
                }
            }
        }
        packs.push(LoadedPack {
            config,
            images,
            cloth_meshes,
            cloth_details,
            prefix: prefix.into(),
        });
    }
    Ok(GraphicsPacks { packs, ..default() })
}
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub fn read_bounded(root: &Path, name: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    if !relative_path(name) {
        return Err("invalid relative path".into());
    }
    let path = root
        .join(name)
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.join(name).display()))?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(&root) {
        return Err("resource symlink escapes config root".into());
    }
    let mut data = Vec::new();
    std::fs::File::open(&path)
        .and_then(|f| f.take(16 * 1024 * 1024 + 1).read_to_end(&mut data))
        .map_err(|e| e.to_string())?;
    if data.len() > 16 * 1024 * 1024 {
        return Err("file exceeds 16 MiB".into());
    }
    Ok(data)
}
impl GraphicsPacks {
    #[cfg(all(test, not(any(target_arch = "wasm32", target_os = "android"))))]
    pub(crate) fn with_test_loose_root(mut self, root: PathBuf) -> Self {
        self.loose_root = Some(root);
        self
    }

    pub fn load() -> Self {
        match Self::try_load() {
            Ok(p) => {
                info!("graphics packs: {} enabled", p.packs.len());
                p
            }
            Err(error) => {
                error!("图形增强包加载失败: {error}");
                Self {
                    packs: vec![],
                    error: Some(error),
                    ..default()
                }
            }
        }
    }
    fn try_load() -> Result<Self, String> {
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        {
            let explicit = std::env::var_os("METEOR_GRAPHICS_CONFIG").map(PathBuf::from);
            let mut paths = vec![];
            if let Some(dir) = std::env::var_os("METEOR_USER_DATA_DIR") {
                paths.push(PathBuf::from(dir).join("config/graphics-enhancement.json"));
            }
            paths.extend([
                PathBuf::from("config/graphics-enhancement.json"),
                PathBuf::from("meteor_rust/config/graphics-enhancement.json"),
            ]);
            if let Ok(exe) = std::env::current_exe() {
                if let Some(parent) = exe.parent() {
                    paths.push(parent.join("config/graphics-enhancement.json"));
                }
            }
            if let Some(path) = explicit.or_else(|| paths.into_iter().find(|p| p.is_file())) {
                let root = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                let filename = path
                    .file_name()
                    .and_then(|p| p.to_str())
                    .ok_or("invalid graphics config filename")?;
                let bytes = read_bounded(root, filename)?;
                info!("graphics config: {}", path.display());
                let mut packs = load_with_reader(&bytes, |name| read_bounded(root, name))?;
                packs.loose_root = Some(root.to_owned());
                return Ok(packs);
            }
        }
        let bundled = include_bytes!("../../../../../config/graphics-enhancement.json");

        let bytes = meteor_asset::runtime_assets::read_entry("graphics/graphics-enhancement.json");
        load_with_reader(bytes.as_deref().unwrap_or(bundled), |name| {
            meteor_asset::runtime_assets::read_entry(&format!("graphics/{name}"))
                .ok_or_else(|| format!("missing graphics/{name}"))
        })
    }
    pub(super) fn has_loose_model_root(&self) -> bool {
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        {
            self.loose_root.is_some()
        }
        #[cfg(any(target_arch = "wasm32", target_os = "android"))]
        {
            false
        }
    }
    pub(super) fn read_meshlet_model(
        &self,
        pack: &LoadedPack,
        file: &str,
    ) -> Result<Vec<u8>, String> {
        let path = if pack.prefix.is_empty() {
            file.to_owned()
        } else {
            format!("{}/{file}", pack.prefix)
        };
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        if let Some(root) = &self.loose_root {
            return read_bounded(root, &path);
        }
        Err(format!(
            "{path}: Meshlet models require desktop loose configuration"
        ))
    }
    pub fn scenes<'a>(
        &'a self,
        stage: &'a str,
    ) -> impl Iterator<Item = (&'a LoadedPack, &'a super::config::Scene)> {
        self.packs.iter().flat_map(move |p| {
            p.config
                .scenes
                .iter()
                .filter(move |s| s.stage == "*" || s.stage.eq_ignore_ascii_case(stage))
                .map(move |s| (p, s))
        })
    }
}
