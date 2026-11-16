use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};

#[derive(Deserialize)]
struct Manifest {
    version: u32,
    textures: BTreeMap<String, TextureFiles>,
    #[serde(default)]
    normal_style: NormalStyle,
}

#[derive(Deserialize, Clone, Copy)]
pub(crate) struct NormalStyle {
    pub strength: f32,
    pub contrast: f32,
}

impl Default for NormalStyle {
    fn default() -> Self {
        Self {
            strength: 1.0,
            contrast: 0.8,
        }
    }
}

#[derive(Deserialize)]
struct TextureFiles {
    diffuse: String,
    normal: Option<String>,
}

struct TextureData {
    diffuse: Vec<u8>,
    normal: Option<Vec<u8>>,
}

#[derive(Default)]
pub(crate) struct SceneTexturePack {
    scope: String,
    textures: HashMap<String, TextureData>,
    normal_style: NormalStyle,
}

fn local_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', ':', '\0'])
}

impl SceneTexturePack {
    pub(crate) fn load(stage: &str, enabled: bool) -> Self {
        Self::load_with_reader(stage, enabled, meteor_asset::runtime_assets::read_entry)
    }

    pub(crate) fn load_with_reader(
        stage: &str,
        enabled: bool,
        mut read: impl FnMut(&str) -> Option<Vec<u8>>,
    ) -> Self {
        let scope = stage.to_ascii_lowercase();
        let mut pack = Self {
            scope: scope.clone(),
            ..Default::default()
        };
        if !enabled || !local_name(&scope) {
            return pack;
        }
        let prefix = format!("scene-hd/{scope}/");
        let Some(bytes) = read(&format!("{prefix}manifest.json")) else {
            return pack;
        };
        let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) else {
            bevy::log::warn!("invalid HD scene manifest for {stage}; using original textures");
            return pack;
        };
        if manifest.version != 1 {
            return pack;
        }
        if manifest.normal_style.strength.is_finite() && manifest.normal_style.contrast.is_finite()
        {
            pack.normal_style = NormalStyle {
                strength: manifest.normal_style.strength.clamp(0.0, 8.0),
                contrast: manifest.normal_style.contrast.clamp(0.0, 2.5),
            };
        }
        for (name, files) in manifest.textures {
            if !local_name(&name)
                || !local_name(&files.diffuse)
                || files
                    .normal
                    .as_deref()
                    .is_some_and(|name| !local_name(name))
            {
                bevy::log::warn!(
                    "invalid HD scene texture path for {stage}/{name}; using original"
                );
                continue;
            }

            let Some(diffuse) = read(&format!("{prefix}{}", files.diffuse)) else {
                continue;
            };
            let normal = files
                .normal
                .and_then(|name| read(&format!("{prefix}{name}")));
            pack.textures
                .insert(name.to_ascii_lowercase(), TextureData { diffuse, normal });
        }
        pack
    }

    fn texture(&self, scope: &str, name: &str) -> Option<&TextureData> {
        if !scope.eq_ignore_ascii_case(&self.scope) {
            return None;
        }
        let name = name.replace('\\', "/");
        let base = match name.rsplit_once('/') {
            Some((prefix, base)) if prefix.eq_ignore_ascii_case(&self.scope) => base,
            Some(_) => return None,
            None => name.as_str(),
        };
        self.textures.get(&base.to_ascii_lowercase())
    }

    pub(crate) fn diffuse(&self, scope: &str, name: &str) -> Option<&[u8]> {
        Some(self.texture(scope, name)?.diffuse.as_slice())
    }

    pub(crate) fn normal(&self, scope: &str, name: &str) -> Option<&[u8]> {
        self.texture(scope, name)?.normal.as_deref()
    }

    pub(crate) fn normal_style(&self) -> NormalStyle {
        self.normal_style
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hd_scene_supports_arbitrary_maps_and_same_named_textures_without_leaking() {
        let manifest = br#"{"version":1,"textures":{"wall.bmp":{"diffuse":"wall-hd.png","normal":"wall-normal.png"}}}"#;
        for stage in ["sn01", "custom_fortress"] {
            let prefix = format!("scene-hd/{stage}/");
            let pack = SceneTexturePack::load_with_reader(stage, true, |path| {
                if path == format!("{prefix}manifest.json") {
                    Some(manifest.to_vec())
                } else if path == format!("{prefix}wall-hd.png") {
                    Some(stage.as_bytes().to_vec())
                } else if path == format!("{prefix}wall-normal.png") {
                    Some(b"normal".to_vec())
                } else {
                    None
                }
            });
            assert_eq!(pack.diffuse(stage, "WALL.BMP"), Some(stage.as_bytes()));
            assert_eq!(pack.normal(stage, "wall.bmp"), Some(b"normal".as_slice()));
            assert!(pack.diffuse("another_map", "wall.bmp").is_none());
            assert!(pack.diffuse(stage, "another_map/wall.bmp").is_none());
            assert_eq!(
                pack.diffuse(stage, &format!("{stage}/wall.bmp")),
                Some(stage.as_bytes())
            );
            assert!(pack.diffuse(stage, "missing.jpg").is_none());
        }
    }

    #[test]
    fn hd_scene_disabled_missing_and_partial_packs_leave_original_fallback_available() {
        let disabled = SceneTexturePack::load_with_reader("custom_map", false, |_| {
            panic!("disabled reads HD data")
        });
        assert!(disabled.diffuse("custom_map", "wall.jpg").is_none());
        let absent = SceneTexturePack::load_with_reader("custom_map", true, |_| None);
        assert!(absent.diffuse("custom_map", "wall.jpg").is_none());
        let partial = SceneTexturePack::load_with_reader("custom_map", true, |path| {
            if path.ends_with("manifest.json") {
                Some(
                    br#"{"version":1,"textures":{
                    "wall.jpg":{"diffuse":"wall.png","normal":"absent.png"},
                    "floor.jpg":{"diffuse":"absent.png"},
                    "unsafe.jpg":{"diffuse":"../wall.png"}}}"#
                        .to_vec(),
                )
            } else if path.ends_with("/wall.png") {
                Some(b"wall".to_vec())
            } else {
                None
            }
        });
        assert_eq!(
            partial.diffuse("custom_map", "wall.jpg"),
            Some(b"wall".as_slice())
        );
        assert!(partial.normal("custom_map", "wall.jpg").is_none());
        assert!(partial.diffuse("custom_map", "floor.jpg").is_none());
        assert!(partial.diffuse("custom_map", "unsafe.jpg").is_none());
        let invalid = SceneTexturePack::load_with_reader("custom_map", true, |_| {
            Some(b"invalid json".to_vec())
        });
        assert!(invalid.diffuse("custom_map", "wall.jpg").is_none());
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn hd_scene_real_map_pack_contains_decodable_diffuse_and_normal_resources() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools/packs");
        let repository = meteor_asset::pack_repository::PackRepository::open(&root).unwrap();
        let pack =
            SceneTexturePack::load_with_reader("sn06", true, |name| repository.read(name).ok());
        assert_eq!(pack.textures.len(), 7);
        for data in pack.textures.values() {
            meteor_asset::texture::decode_image(&data.diffuse).unwrap();
            meteor_asset::texture::decode_image(data.normal.as_deref().unwrap()).unwrap();
        }
    }
}
