use super::{
    bake::{BakedSurface, Inputs},
    config::{Surface, WaterConfig},
};
use crate::plugins::graphics_pack::{config::WaterSkySelection, GraphicsPacks};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub(super) struct Prepared {
    pub source: Surface,
    pub maps: BakedSurface,
}
pub(super) static PREPARED: OnceLock<Vec<Prepared>> = OnceLock::new();
#[derive(Serialize, Deserialize)]
struct Entry {
    key: String,
    digest: String,
    maps: BakedSurface,
}
fn digest(maps: &BakedSurface) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(maps).unwrap()))
}
fn valid(m: &BakedSurface, s: &Surface, sky: Option<WaterSkySelection<'_>>) -> bool {
    m.bounds.iter().all(|v| v.is_finite())
        && m.bounds[2] > 0.0
        && m.bounds[3] > 0.0
        && (0.0..=1.0).contains(&m.vertex_alpha)
        && if let Some(sky) = sky {
            let cells = sky.profile.visibility_resolution;
            m.sky_visibility.as_ref().is_some_and(|map| {
                map.valid(cells * (sky.profile.visibility_directions / 4), cells)
            })
        } else {
            m.sky_visibility.is_none()
        }
        && if s.needs_maps() {
            let tile = s.reflection_face + 2;
            m.reflection
                .as_ref()
                .is_some_and(|m| m.valid(tile * 3, tile * 2))
                && m.depth.as_ref().is_some_and(|m| m.valid(128, 128))
        } else {
            m.reflection.is_none() && m.depth.is_none()
        }
}
fn read(
    path: &Path,
    key: &str,
    s: &Surface,
    sky: Option<WaterSkySelection<'_>>,
) -> Option<BakedSurface> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(20_000_001)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 20_000_000 {
        return None;
    }
    let e: Entry = serde_json::from_slice(&bytes).ok()?;
    (e.key == key && e.digest == digest(&e.maps) && valid(&e.maps, s, sky)).then_some(e.maps)
}
fn write(path: &Path, key: &str, maps: &BakedSurface) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let e = Entry {
        key: key.into(),
        digest: digest(maps),
        maps: maps.clone(),
    };
    let result = (|| {
        let mut f = std::fs::File::create(&temporary)?;
        f.write_all(&serde_json::to_vec(&e)?)?;
        f.sync_all()?;

        if path.exists() {
            std::fs::remove_file(path)?;
        }
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|e: std::io::Error| e.to_string())
}
pub(super) fn cached(
    inputs: &Inputs,
    s: &Surface,
    sky: Option<WaterSkySelection<'_>>,
    root: &Path,
    progress: impl FnMut(f32),
) -> Result<(BakedSurface, bool), String> {
    let key = inputs.cache_key(s, sky);
    let path = root.join(format!("{key}.json"));
    if let Some(m) = read(&path, &key, s, sky) {
        return Ok((m, true));
    }
    let maps = inputs.bake(s, sky, progress)?;
    write(&path, &key, &maps).map_err(|e| format!("无法保存水面缓存 {}: {e}", path.display()))?;
    Ok((maps, false))
}
pub(super) fn prepare(
    c: &WaterConfig,
    graphics: &GraphicsPacks,
    root: &Path,
    mut progress: impl FnMut(f32, String),
) -> Result<Vec<Prepared>, String> {
    let mut result = Vec::new();
    let surfaces: Vec<_> = c.surfaces.iter().filter(|s| s.enabled).collect();
    let mut current: Option<(String, Inputs)> = None;
    for (index, s) in surfaces.iter().enumerate() {
        let total = surfaces.len() as f32;
        progress(
            index as f32 / total,
            format!("正在检查水面资源：{}/{}", s.stage, s.object),
        );
        if current.as_ref().is_none_or(|(stage, _)| stage != &s.stage) {
            current = Some((s.stage.clone(), Inputs::load(&s.stage)?));
        }
        let inputs = &current.as_ref().unwrap().1;
        let sky = graphics
            .water_sky_for_bake(&s.stage, &s.object, &s.texture)
            .filter(|_| s.kind == 1);
        let (maps, hit) = cached(inputs, s, sky, root, |p| {
            progress(
                (index as f32 + p) / total,
                format!("正在自动烘焙水面：{}/{}", s.stage, s.object),
            )
        })?;
        bevy::log::info!(
            "水面 {}/{}: {}",
            s.stage,
            s.object,
            if hit { "复用缓存" } else { "烘焙完成" }
        );
        result.push(Prepared {
            source: (*s).clone(),
            maps,
        });
    }
    progress(1.0, "水面准备完成".into());
    Ok(result)
}
pub(super) fn directory() -> PathBuf {
    if let Some(path) = std::env::var_os("METEOR_WATER_CACHE") {
        return path.into();
    }
    if let Some(path) = std::env::var_os("METEOR_USER_DATA_DIR") {
        return PathBuf::from(path).join("cache/water");
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Library/Caches/OpenMeteor/water");
    }
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(path).join("OpenMeteor/cache/water");
    }
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(path).join("openmeteor/water");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache/openmeteor/water");
    }
    std::env::temp_dir().join("openmeteor/water")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incomplete_corrupt_or_wrong_resolution_cache_is_never_reused() {
        let c = WaterConfig::bundled();
        let s = &c.surfaces[0];
        let tile = s.reflection_face + 2;
        let map = |w: usize, h: usize| super::super::bake::Map {
            width: w as u32,
            height: h as u32,
            rgba: vec![0; w * h * 4],
        };
        let maps = BakedSurface {
            bounds: [0., 0., 10., 10.],
            vertex_alpha: 0.5,
            reflection: Some(map(tile * 3, tile * 2)),
            depth: Some(map(128, 128)),
            sky_visibility: None,
        };
        let dir = std::env::temp_dir().join(format!("water-cache-test-{}", std::process::id()));
        let p = dir.join("entry.json");
        write(&p, "one", &maps).unwrap();
        assert!(read(&p, "one", s, None).is_some());
        assert!(read(&p, "two", s, None).is_none());
        let mut changed = s.clone();
        changed.reflection_face = 128;
        assert!(read(&p, "one", &changed, None).is_none());
        let mut e: Entry = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        e.maps.depth.as_mut().unwrap().rgba[0] = 1;
        std::fs::write(&p, serde_json::to_vec(&e).unwrap()).unwrap();
        assert!(read(&p, "one", s, None).is_none());
        std::fs::write(&p, b"{\"partial\":").unwrap();
        assert!(read(&p, "one", s, None).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
