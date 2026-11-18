use serde::{Deserialize, Serialize};

pub(super) const DEEP_FILE: &str = "water-deep.json";
pub(super) const SHALLOW_FILE: &str = "water-shallow.json";
const DEEP: &str = include_str!("../../../../../../config/water-deep.json");
const SHALLOW: &str = include_str!("../../../../../../config/water-shallow.json");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WaterProfile {
    #[default]
    #[serde(alias = "legacy")]
    Auto,
    Deep,
    Shallow,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Optics {
    pub absorption: [f32; 3],
    pub scattering: [f32; 3],
    pub anisotropy: f32,
    pub ior: f32,
    pub roughness: f32,
    pub refraction_strength: f32,
    pub max_path: f32,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Shallow {
    pub gravity: f32,
    pub drag: f32,
    pub min_depth: f32,
    pub fallback_depth: f32,
    pub max_depth: f32,
    pub max_grid_side: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileFile {
    schema_version: u32,
    optics: Optics,
    simulation: Option<Shallow>,
}

pub(super) struct Profiles {
    pub deep: Optics,
    pub shallow_optics: Optics,
    pub shallow: Shallow,
}

fn check(field: &str, value: f32, low: f32, high: f32) -> Result<(), String> {
    super::config::range(field, value, low, high)
}

impl Optics {
    fn validate(self) -> Result<(), String> {
        for value in self.absorption {
            check("absorption", value, 0., 16.)?;
        }
        for value in self.scattering {
            check("scattering", value, 0., 8.)?;
        }
        check("anisotropy", self.anisotropy, -0.85, 0.85)?;
        check("ior", self.ior, 1.01, 2.)?;
        check("roughness", self.roughness, 0.045, 1.)?;
        check("refraction_strength", self.refraction_strength, 0., 1.)?;
        check("max_path", self.max_path, 0.1, 64.)
    }
}

impl Profiles {
    pub fn bundled() -> Self {
        Self::parse(DEEP, SHALLOW).expect("bundled water profiles")
    }

    fn parse(deep: &str, shallow: &str) -> Result<Self, String> {
        let deep: ProfileFile =
            serde_json::from_str(deep).map_err(|e| format!("{DEEP_FILE}: {e}"))?;
        let shallow: ProfileFile =
            serde_json::from_str(shallow).map_err(|e| format!("{SHALLOW_FILE}: {e}"))?;
        if deep.schema_version != 1 || shallow.schema_version != 1 || deep.simulation.is_some() {
            return Err("water profile schema_version must be 1".into());
        }
        let s = shallow
            .simulation
            .ok_or("shallow simulation parameters missing")?;
        deep.optics
            .validate()
            .map_err(|e| format!("{DEEP_FILE}: {e}"))?;
        shallow
            .optics
            .validate()
            .map_err(|e| format!("{SHALLOW_FILE}: {e}"))?;
        check("gravity", s.gravity, 0.1, 20.)?;
        check("drag", s.drag, 0.1, 20.)?;
        check("min_depth", s.min_depth, 0.02, 0.5)?;
        check("max_depth", s.max_depth, s.min_depth, 6.)?;
        check("fallback_depth", s.fallback_depth, s.min_depth, s.max_depth)?;
        if !(16..=192).contains(&s.max_grid_side) {
            return Err("max_grid_side must be 16..=192".into());
        }
        Ok(Self {
            deep: deep.optics,
            shallow_optics: shallow.optics,
            shallow: s,
        })
    }

    #[cfg(all(not(test), not(any(target_arch = "wasm32", target_os = "android"))))]
    pub fn load(directory: &std::path::Path) -> Result<Self, String> {
        use std::io::{ErrorKind, Read};
        let read = |name: &str, bundled: &str| -> Result<String, String> {
            let path = directory.join(name);
            let mut content = String::new();
            match std::fs::File::open(&path)
                .and_then(|f| f.take(16385).read_to_string(&mut content))
            {
                Ok(n) if n <= 16384 => Ok(content),
                Ok(_) => Err(format!("{} exceeds 16 KiB", path.display())),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(bundled.to_owned()),
                Err(e) => Err(format!("{}: {e}", path.display())),
            }
        };
        Self::parse(&read(DEEP_FILE, DEEP)?, &read(SHALLOW_FILE, SHALLOW)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_bounded_and_independent() {
        let profiles = Profiles::bundled();
        assert_ne!(profiles.deep.absorption, profiles.shallow_optics.absorption);
        let mut shallow: serde_json::Value = serde_json::from_str(SHALLOW).unwrap();
        shallow["simulation"]["max_grid_side"] = 512.into();
        assert!(Profiles::parse(DEEP, &shallow.to_string()).is_err());
        let mut deep: serde_json::Value = serde_json::from_str(DEEP).unwrap();
        deep["optics"]["ior"] = 0.into();
        assert!(Profiles::parse(&deep.to_string(), SHALLOW).is_err());
        deep["optics"]["ior"] = 1.333.into();
        deep["unknown"] = 1.into();
        assert!(Profiles::parse(&deep.to_string(), SHALLOW).is_err());
    }
}
