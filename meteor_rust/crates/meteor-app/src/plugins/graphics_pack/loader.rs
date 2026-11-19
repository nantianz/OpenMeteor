use super::cloth_binding::{DetailMesh, Pinning};
use super::config::*;
use bevy::prelude::*;
use meteor_asset::texture::DecodedImage;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
use std::path::{Path, PathBuf};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    sync::Arc,
};

struct PackManifest {
    path: String,
    config: Pack,
}

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
    catalog: Arc<[PackManifest]>,
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

fn png_decoded_size(bytes: &[u8]) -> Result<usize, String> {
    if bytes.len() > 16 * 1024 * 1024 || bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("resource must be a PNG, at most 16 MiB".into());
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return Err("PNG dimensions must be 1..4096".into());
    }
    Ok(w as usize * h as usize * 4)
}
#[cfg(test)]
pub fn decode_resource(bytes: &[u8]) -> Result<DecodedImage, String> {
    png_decoded_size(bytes)?;
    meteor_asset::texture::decode_image(bytes)
}

pub(super) fn load_catalog_with_reader(
    index: &[u8],
    mut read: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<GraphicsPacks, String> {
    let index = parse_index(index)?;
    if !index.enabled {
        return Ok(GraphicsPacks::default());
    }
    let mut catalog = Vec::new();
    let mut ids = HashSet::new();
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
        catalog.push(PackManifest { path, config });
    }
    Ok(GraphicsPacks {
        catalog: catalog.into(),
        ..default()
    })
}

fn stage_resources(config: &Pack, meshlets: bool) -> (BTreeSet<&str>, BTreeSet<&str>) {
    let mut images = BTreeSet::new();
    let mut cloth_meshes = BTreeSet::new();
    let mut materials = BTreeSet::new();
    for scene in &config.scenes {
        materials.extend(
            scene
                .surfaces
                .iter()
                .filter_map(|surface| surface.material.as_deref()),
        );
        if let Some(environment) = &scene.environment {
            images.extend(environment.diffuse.as_deref());
            images.extend(environment.specular_mips.iter().map(String::as_str));
        }
        for cloth in scene.character_cloth.iter().filter(|cloth| cloth.enabled) {
            materials.insert(cloth.material.as_str());
            if let Some(mesh) = &cloth.mesh {
                cloth_meshes.insert(mesh.id.as_str());
                cloth_meshes.extend(mesh.render_id.as_deref());
            }
            if let Some(rigid) = &cloth.rigid_mesh {
                cloth_meshes.insert(rigid.id.as_str());
                materials.insert(rigid.material.as_str());
            }
        }
        if meshlets {
            let models = scene
                .meshlet_replacements
                .iter()
                .map(|rule| &rule.model)
                .chain(scene.meshlet_props.iter().map(|rule| &rule.model));
            for model in models {
                materials.extend(
                    config.meshlet_models[model]
                        .materials
                        .values()
                        .map(String::as_str),
                );
            }
        }
    }
    for id in materials {
        let material = &config.materials[id];
        images.extend(
            [&material.base_color, &material.normal, &material.orm]
                .into_iter()
                .filter_map(|id| id.as_deref()),
        );
    }
    (images, cloth_meshes)
}

fn load_resources(
    catalog: &[PackManifest],
    stage: Option<&str>,
    meshlets: bool,
    mut read: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<Vec<LoadedPack>, String> {
    let mut packs = Vec::new();
    let mut decoded_bytes = 0;
    for PackManifest { path, config } in catalog {
        let mut config = config.clone();
        if let Some(stage) = stage {
            config
                .scenes
                .retain(|scene| scene.stage == "*" || scene.stage.eq_ignore_ascii_case(stage));
            if config.scenes.is_empty() {
                continue;
            }
        }
        let (image_ids, mesh_ids) = if stage.is_some() {
            stage_resources(&config, meshlets)
        } else {
            (
                config.resources.keys().map(String::as_str).collect(),
                config.cloth_meshes.keys().map(String::as_str).collect(),
            )
        };
        let prefix = path.rsplit_once('/').map_or("", |(p, _)| p);
        let mut read_file = |file: &str| {
            let full = resource_path(prefix, file);
            let bytes = read(&full).map_err(|e| format!("{full}: {e}"))?;
            Ok::<_, String>((full, bytes))
        };
        let mut images = BTreeMap::new();
        for id in image_ids {
            let (full, bytes) = read_file(&config.resources[id])?;
            decoded_bytes += png_decoded_size(&bytes).map_err(|e| format!("{full}: {e}"))?;
            if decoded_bytes > 256 * 1024 * 1024 {
                return Err(
                    "graphics packs exceed 256 MiB decoded texture budget for this stage".into(),
                );
            }
            let image =
                meteor_asset::texture::decode_image(&bytes).map_err(|e| format!("{full}: {e}"))?;
            images.insert(id.to_owned(), Arc::new(image));
        }
        let mut cloth_meshes = BTreeMap::new();
        for id in mesh_ids {
            let (full, bytes) = read_file(&config.cloth_meshes[id])?;
            let mesh =
                super::cloth_obj::ClothObj::parse(&bytes).map_err(|e| format!("{full}: {e}"))?;
            cloth_meshes.insert(id.to_owned(), mesh);
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
    Ok(packs)
}

#[cfg(test)]
pub fn load_with_reader(
    index: &[u8],
    mut read: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<GraphicsPacks, String> {
    let mut packs = load_catalog_with_reader(index, &mut read)?;
    packs.packs = load_resources(&packs.catalog, None, true, read)?;
    Ok(packs)
}

fn resource_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
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
                info!("graphics packs: {} manifests enabled", p.catalog.len());
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
                let mut packs = load_catalog_with_reader(&bytes, |name| read_bounded(root, name))?;
                packs.loose_root = Some(root.to_owned());
                return Ok(packs);
            }
        }
        let bundled = include_bytes!("../../../../../config/graphics-enhancement.json");

        let bytes = meteor_asset::runtime_assets::read_entry("graphics/graphics-enhancement.json");
        load_catalog_with_reader(bytes.as_deref().unwrap_or(bundled), read_mounted_resource)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn catalog_snapshot(&self) -> Self {
        Self {
            catalog: self.catalog.clone(),
            error: self.error.clone(),
            #[cfg(not(target_os = "android"))]
            loose_root: self.loose_root.clone(),
            ..default()
        }
    }

    pub(crate) fn prepare_stage(&mut self, stage: &str) {
        self.packs.clear();
        match load_resources(
            &self.catalog,
            Some(stage),
            self.has_loose_model_root(),
            |name| {
                #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
                if let Some(root) = &self.loose_root {
                    return read_bounded(root, name);
                }
                read_mounted_resource(name)
            },
        ) {
            Ok(packs) => {
                if !packs.is_empty() {
                    let decoded_bytes: usize = packs
                        .iter()
                        .flat_map(|pack| pack.images.values())
                        .map(|image| image.rgba.len())
                        .sum();
                    info!(
                        "graphics stage {stage}: {} packs, {:.1} MiB decoded textures",
                        packs.len(),
                        decoded_bytes as f64 / (1024.0 * 1024.0)
                    );
                }
                self.packs = packs;
            }
            Err(error) => warn!("graphics stage {stage}: {error}; using original visuals"),
        }
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
        let path = resource_path(&pack.prefix, file);
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

fn read_mounted_resource(name: &str) -> Result<Vec<u8>, String> {
    let name = format!("graphics/{name}");
    meteor_asset::runtime_assets::read_entry(&name).ok_or_else(|| format!("missing {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::AppState;

    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    #[test]
    fn sn03_and_sn06_share_a_catalog_without_sharing_the_texture_budget() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
        let sn03 = "graphics-packs/sn03-mist-ravine/pack.json";
        let sn06 = "graphics-packs/sn06-moss-valley/pack.json";
        let index = serde_json::json!({
            "schema_version": 1,
            "enabled": true,
            "packs": ["graphics-packs/starter/pack.json", sn03, sn06],
        });
        let bytes = serde_json::to_vec(&index).unwrap();
        let mut graphics = load_catalog_with_reader(&bytes, |name| {
            assert!(
                name.ends_with("pack.json"),
                "unexpected startup read: {name}"
            );
            read_bounded(&root, name)
        })
        .unwrap()
        .with_test_loose_root(root.clone());
        assert!(graphics.packs.is_empty());

        let all_decoded_bytes: usize = graphics
            .catalog
            .iter()
            .flat_map(|pack| {
                let prefix = pack.path.rsplit_once('/').unwrap().0;
                pack.config
                    .resources
                    .values()
                    .map(move |file| resource_path(prefix, file))
            })
            .map(|name| png_decoded_size(&read_bounded(&root, &name).unwrap()).unwrap())
            .sum();
        assert!(all_decoded_bytes > 256 * 1024 * 1024);

        for (stage, expected, other) in [
            ("sn03", "sn03-mist-ravine", "sn06-moss-valley"),
            ("sn06", "sn06-moss-valley", "sn03-mist-ravine"),
        ] {
            graphics.prepare_stage(stage);
            let loaded: Vec<_> = graphics
                .packs
                .iter()
                .map(|p| p.config.id.as_str())
                .collect();
            assert!(loaded.contains(&expected), "{stage}: loaded {loaded:?}");
            assert!(!loaded.contains(&other), "{stage}: loaded {loaded:?}");
        }
    }

    #[test]
    fn graphics_resources_are_selected_per_stage_and_released_on_leaving_play() {
        let index = br#"{"schema_version":1,"enabled":true,"packs":["pack/pack.json"]}"#;
        let config = serde_json::json!({
            "schema_version": 1,
            "id": "stage-assets",
            "resources": {"common":"common.png", "valley":"valley.png", "city":"city.png"},
            "materials": {"common":{"base_color":"common"}, "valley":{"normal":"valley"}, "city":{"orm":"city"}},
            "scenes": [
                {"stage":"*", "surfaces":[{"object":"common","material":"common"}]},
                {"stage":"sn06", "surfaces":[{"object":"valley","material":"valley"}]},
                {"stage":"sn13", "surfaces":[{"object":"city","material":"city"}]}
            ]
        });
        let mut graphics = load_catalog_with_reader(index, |name| {
            assert_eq!(name, "pack/pack.json");
            Ok(config.to_string().into_bytes())
        })
        .unwrap();
        assert!(graphics.packs.is_empty());
        let image =
            include_bytes!("../../../../../config/graphics-packs/starter/textures/stone-color.png");
        let mut reads = Vec::new();
        graphics.packs = load_resources(&graphics.catalog, Some("SN06"), true, |path| {
            reads.push(path.to_owned());
            Ok(image.to_vec())
        })
        .unwrap();
        assert_eq!(reads, ["pack/common.png", "pack/valley.png"]);
        let texture = Arc::downgrade(&graphics.packs[0].images["valley"]);
        let next = load_resources(&graphics.catalog, Some("sn13"), true, |path| {
            reads.push(path.to_owned());
            Ok(image.to_vec())
        })
        .unwrap();
        assert_eq!(&reads[2..], ["pack/city.png", "pack/common.png"]);
        assert!(!next[0].images.contains_key("valley"));

        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .insert_state(AppState::Playing)
            .insert_resource(graphics)
            .add_systems(
                OnExit(AppState::Playing),
                super::super::release_stage_assets.run_if(crate::plugins::ends_playing_session),
            )
            .add_systems(
                OnTransition {
                    exited: AppState::Paused,
                    entered: AppState::Menu,
                },
                crate::plugins::cleanup_suspended_playing_session,
            );
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Paused);
        app.update();
        assert!(texture.upgrade().is_some());
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Menu);
        app.update();
        assert!(texture.upgrade().is_none());
        assert!(app.world().resource::<GraphicsPacks>().packs.is_empty());
        assert_eq!(app.world().resource::<GraphicsPacks>().catalog.len(), 1);
    }
}
