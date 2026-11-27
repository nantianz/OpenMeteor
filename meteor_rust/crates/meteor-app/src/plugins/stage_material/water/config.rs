use super::profiles::{Profiles, WaterProfile};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    sync::{Arc, OnceLock},
};
#[cfg(test)]
const BUNDLED: &str =
    include_str!("../../../../../../config/graphics-packs/water/water-enhancement.json");
pub(super) const MAX_WAVES: usize = 8;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WaterConfig {
    pub schema_version: u32,
    #[serde(default)]
    pub live_reflections: bool,
    #[serde(default)]
    pub depth_hierarchy: bool,
    pub geometry: Geometry,
    pub wind_waves: Vec<WindWave>,
    pub simulation: Simulation,
    pub interaction: Interaction,
    pub surfaces: Vec<Surface>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Geometry {
    pub subdivisions: usize,
    pub max_vertices: usize,
    pub shore_fade: f32,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WindWave {
    pub direction: [f32; 2],
    pub wavelength: f32,
    pub amplitude: f32,
    pub speed: f32,
    pub warp: [f32; 2],
    pub phase: f32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Simulation {
    pub max_side: usize,
    pub min_spacing: f32,
    pub speed: f32,
    pub damping: f32,
    pub impulse_gain: f32,
    pub foam_gain: f32,
    pub foam_decay: f32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Interaction {
    pub max_distance: f32,
    pub max_actors: usize,
    pub step_distance: f32,
    pub step_interval: f32,
    pub entry_interval: f32,
    pub foot_radius: f32,
    pub strength_scale: f32,
    pub splash_scale: f32,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct Surface {
    pub stage: String,
    pub object: String,
    pub texture: String,
    pub kind: u8,
    pub extinction: f32,
    pub max_depth: f32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "face_size")]
    pub reflection_face: usize,
    #[serde(default)]
    pub blend_mode: BlendMode,
    #[serde(default, skip_serializing_if = "is_auto")]
    pub profile: WaterProfile,
}
fn is_auto(profile: &WaterProfile) -> bool {
    *profile == WaterProfile::Auto
}
fn yes() -> bool {
    true
}
fn face_size() -> usize {
    64
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum BlendMode {
    #[default]
    SrcAlpha,
    SrcColorOne,
}
impl Surface {
    pub fn clear_profile(&self) -> Option<WaterProfile> {
        (self.kind == 1).then_some(match self.profile {
            WaterProfile::Auto => WaterProfile::Deep,
            profile => profile,
        })
    }

    pub fn matches(&self, stage: &str, object: &str, texture: &str) -> bool {
        let normalized = texture.replace('\\', "/");
        self.stage.eq_ignore_ascii_case(stage)
            && self.object == object
            && self
                .texture
                .eq_ignore_ascii_case(normalized.rsplit('/').next().unwrap_or(""))
    }
    pub fn needs_maps(&self) -> bool {
        self.kind < 3 && self.blend_mode == BlendMode::SrcAlpha
    }
}

pub(super) fn range(name: &str, v: f32, lo: f32, hi: f32) -> Result<(), String> {
    if v.is_finite() && (lo..=hi).contains(&v) {
        Ok(())
    } else {
        Err(format!("{name} 必须在 {lo}..={hi} 之间"))
    }
}
impl WaterConfig {
    #[cfg(test)]
    pub fn bundled() -> Self {
        Self::from_json(BUNDLED).expect("bundled water configuration")
    }
    pub fn from_json(json: &str) -> Result<Self, String> {
        if json.len() > 1_048_576 {
            return Err("水面配置超过 1 MiB".into());
        }
        let c: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if c.schema_version != 1 {
            return Err("不支持的水面 schema_version（应为 1）".into());
        }
        if !(2..=64).contains(&c.geometry.subdivisions)
            || !(64..=96_000).contains(&c.geometry.max_vertices)
        {
            return Err("geometry: subdivisions 应为 2..64，max_vertices 应为 64..96000".into());
        }
        range("geometry.shore_fade", c.geometry.shore_fade, 0.1, 100.0)?;
        if c.wind_waves.len() > MAX_WAVES {
            return Err("wind_waves 最多 8 组".into());
        }
        for w in &c.wind_waves {
            for v in w.direction {
                range("direction", v, -1.0, 1.0)?;
            }
            if w.direction[0].hypot(w.direction[1]) < 0.01 {
                return Err("direction 不得为零".into());
            }
            range("wavelength", w.wavelength, 1.0, 500.0)?;
            range("amplitude", w.amplitude, 0.0, 0.5)?;
            range("speed", w.speed, -10.0, 10.0)?;
            for v in w.warp {
                range("warp", v, -1.0, 1.0)?;
            }
            range("phase", w.phase, -100.0, 100.0)?;
        }
        if c.wind_waves.iter().map(|w| w.amplitude).sum::<f32>() > 0.8 {
            return Err("wind_waves 总振幅不得超过 0.8".into());
        }
        let s = &c.simulation;
        if !(16..=512).contains(&s.max_side) {
            return Err("simulation.max_side 应为 16..512".into());
        }
        range("min_spacing", s.min_spacing, 0.1, 10.0)?;
        range("speed", s.speed, 0.1, 30.0)?;
        if s.speed / (120.0 * s.min_spacing) > 0.5 {
            return Err("波速过大或网格过密，超过稳定子步限制".into());
        }
        range("damping", s.damping, 0.01, 20.0)?;
        range("impulse_gain", s.impulse_gain, 0.0, 20.0)?;
        range("foam_gain", s.foam_gain, 0.0, 4.0)?;
        range("foam_decay", s.foam_decay, 0.1, 20.0)?;
        let i = &c.interaction;
        range("max_distance", i.max_distance, 1.0, 200.0)?;
        if !(1..=32).contains(&i.max_actors) {
            return Err("max_actors 应为 1..32".into());
        }
        range("step_distance", i.step_distance, 0.1, 10.0)?;
        range("step_interval", i.step_interval, 0.05, 5.0)?;
        range("entry_interval", i.entry_interval, 0.05, 5.0)?;
        range("foot_radius", i.foot_radius, 0.1, 4.0)?;
        range("strength_scale", i.strength_scale, 0.0, 3.0)?;
        range("splash_scale", i.splash_scale, 0.0, 3.0)?;
        if c.surfaces.len() > 128 {
            return Err("surfaces 最多 128 项".into());
        }
        let mut keys = HashSet::new();
        for s in &c.surfaces {
            if s.stage.is_empty()
                || !s
                    .stage
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || s.object.is_empty()
                || s.object.len() > 128
                || s.texture.is_empty()
                || s.texture.len() > 128
                || s.texture.contains(['/', '\\'])
                || s.texture.to_ascii_lowercase().ends_with(".ifl")
            {
                return Err("surface 名称无效；stage/texture 不得包含目录".into());
            }
            if !keys.insert((
                s.stage.to_ascii_lowercase(),
                &s.object,
                s.texture.to_ascii_lowercase(),
            )) {
                return Err(format!("重复水面 {}/{}", s.stage, s.object));
            }
            if !(1..=3).contains(&s.kind) || (s.blend_mode == BlendMode::SrcColorOne && s.kind != 2)
            {
                return Err("kind 应为 1 清水 / 2 毒液 / 3 岩浆；加色混合仅支持毒液".into());
            }
            if s.profile != WaterProfile::Auto && s.kind != 1 {
                return Err("光学预设仅适用于清水".into());
            }
            range("max_depth", s.max_depth, 0.1, 100.0)?;
            range("extinction", s.extinction, 0.0, 20.0)?;
            if ![32, 64, 128, 256].contains(&s.reflection_face) {
                return Err("reflection_face 应为 32/64/128/256".into());
            }
        }
        Ok(c)
    }
}

static ACTIVE: OnceLock<Arc<super::WaterPackage>> = OnceLock::new();
fn package() -> &'static super::WaterPackage {
    #[cfg(test)]
    {
        ACTIVE
            .get()
            .map(Arc::as_ref)
            .unwrap_or_else(|| super::WaterPackage::fixture())
    }
    #[cfg(not(test))]
    {
        ACTIVE
            .get()
            .expect("water requires an active graphics pack")
    }
}
pub(super) fn get() -> &'static WaterConfig {
    &package().config
}
pub(super) fn profiles() -> &'static Profiles {
    &package().profiles
}
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub(super) fn depth_hierarchy_enabled() -> bool {
    ACTIVE
        .get()
        .is_some_and(|package| package.depth_hierarchy())
}
pub(super) fn initialize(water: Arc<super::WaterPackage>) {
    let _ = ACTIVE.set(water);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_river_profiles_and_legacy_alias() {
        let config = WaterConfig::bundled();
        for surface in config.surfaces {
            let expected = match (surface.kind, surface.stage.as_str()) {
                (1, "sn13") => Some(WaterProfile::Classic),
                (1, _) => Some(WaterProfile::Deep),
                _ => None,
            };
            assert_eq!(surface.clear_profile(), expected);
        }
        let text = BUNDLED.replace("\"profile\": \"classic\"", "\"profile\": \"shallow\"");
        let config = WaterConfig::from_json(&text).unwrap();
        let sn13 = config
            .surfaces
            .iter()
            .find(|surface| surface.stage == "sn13")
            .unwrap();
        assert_eq!(sn13.clear_profile(), Some(WaterProfile::Shallow));
        let text = BUNDLED.replace("\"profile\": \"deep\"", "\"profile\": \"legacy\"");
        let config = WaterConfig::from_json(&text).unwrap();
        let old = config
            .surfaces
            .iter()
            .find(|surface| surface.stage == "sn09")
            .unwrap();
        assert_eq!(old.clear_profile(), Some(WaterProfile::Deep));
    }

    #[test]
    fn rejects_unsafe_and_ambiguous_configuration() {
        let original: serde_json::Value = serde_json::from_str(BUNDLED).unwrap();
        for (key, value) in [
            ("schema_version", serde_json::json!(9)),
            ("typo", serde_json::json!(1)),
        ] {
            let mut v = original.clone();
            v[key] = value;
            assert!(WaterConfig::from_json(&v.to_string()).is_err());
        }
        let mut v = original.clone();
        v["simulation"]["speed"] = serde_json::json!(30);
        v["simulation"]["min_spacing"] = serde_json::json!(0.1);
        assert!(WaterConfig::from_json(&v.to_string()).is_err());
        let mut v = original;
        let first = v["surfaces"][0].clone();
        v["surfaces"].as_array_mut().unwrap().push(first);
        assert!(WaterConfig::from_json(&v.to_string()).is_err());
    }
}
