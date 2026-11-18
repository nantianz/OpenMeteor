use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    pub schema_version: u32,
    pub enabled: bool,
    pub packs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pack {
    pub schema_version: u32,
    pub id: String,
    #[serde(default)]
    pub resources: BTreeMap<String, String>,
    #[serde(default)]
    pub materials: BTreeMap<String, Pbr>,
    #[serde(default)]
    pub scenes: Vec<Scene>,
    #[serde(default)]
    pub meshlet_models: BTreeMap<String, MeshletModel>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Pbr {
    pub base_color: Option<String>,
    pub normal: Option<String>,
    pub orm: Option<String>,
    pub tint: [f32; 3],
    pub metallic: f32,
    pub roughness: f32,
    pub normal_strength: f32,
    pub vertex_color: f32,
}
impl Default for Pbr {
    fn default() -> Self {
        Self {
            base_color: None,
            normal: None,
            orm: None,
            tint: [1.; 3],
            metallic: 0.,
            roughness: 0.8,
            normal_strength: 1.,
            vertex_color: 0.,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub stage: String,
    #[serde(default)]
    pub sky: Option<Sky>,
    #[serde(default)]
    pub grass: Option<Grass>,
    #[serde(default)]
    pub environment: Option<Environment>,
    #[serde(default)]
    pub wind: Option<Wind>,
    #[serde(default)]
    pub clothing: Option<Clothing>,
    #[serde(default)]
    pub character_cloth: Vec<CharacterCloth>,
    #[serde(default)]
    pub contact_shadows: Option<ContactShadows>,
    #[serde(default)]
    pub fog: Option<Vec<Fog>>,
    #[serde(default)]
    pub surfaces: Vec<Surface>,
    #[serde(default)]
    pub meshlet_replacements: Vec<MeshletReplacement>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Grass {
    pub enabled: bool,
    pub surfaces: Vec<GrassSurface>,
    pub bounds: Option<[[f32; 2]; 2]>,
    pub density: f32,
    pub tile_size: f32,
    pub height: [f32; 2],
    pub width: [f32; 2],
    pub root_color: [f32; 3],
    pub tip_color: [f32; 3],
    pub min_up: f32,
    pub near_distance: f32,
    pub mid_distance: f32,
    pub max_distance: f32,
    pub max_visible_tiles: usize,
    pub max_triangles: usize,
    pub clump_radius: f32,
    pub patch_size: f32,
    pub ground_cover: f32,
    pub species: GrassSpecies,
    pub wind_strength: f32,
    pub interaction_radius: f32,
    pub recovery_seconds: f32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrassSurface {
    pub object: String,
    pub texture: String,
}

impl GrassSurface {
    pub fn matches(&self, object: &str, texture: &str) -> bool {
        self.object.eq_ignore_ascii_case(object) && self.texture.eq_ignore_ascii_case(texture)
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GrassSpecies {
    pub grass: f32,
    pub plumes: f32,
    pub broadleaf: f32,
}

impl Default for GrassSpecies {
    fn default() -> Self {
        Self {
            grass: 0.67,
            plumes: 0.18,
            broadleaf: 0.15,
        }
    }
}

impl Default for Grass {
    fn default() -> Self {
        Self {
            enabled: true,
            surfaces: Vec::new(),
            bounds: None,
            density: 5.0,
            tile_size: 6.0,
            height: [0.38, 1.12],
            width: [0.07, 0.16],
            root_color: [0.033, 0.070, 0.022],
            tip_color: [0.23, 0.35, 0.095],
            min_up: 0.72,
            near_distance: 18.0,
            mid_distance: 33.0,
            max_distance: 56.0,
            max_visible_tiles: 96,
            max_triangles: 104_000,
            clump_radius: 0.26,
            patch_size: 3.8,
            ground_cover: 0.90,
            species: GrassSpecies::default(),
            wind_strength: 0.23,
            interaction_radius: 1.05,
            recovery_seconds: 1.45,
        }
    }
}

impl Grass {
    pub(super) fn lod_distances(&self) -> [f32; 4] {
        [
            self.near_distance,
            self.mid_distance,
            self.mid_distance + (self.max_distance - self.mid_distance) * 0.38,
            self.max_distance,
        ]
    }

    fn validate(&self, stage: &str) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        if stage == "*"
            || !(1..=24).contains(&self.surfaces.len())
            || self.surfaces.iter().any(|target| {
                [&target.object, &target.texture].into_iter().any(|name| {
                    name.trim().is_empty() || name.len() > 120 || name.contains(['*', '?'])
                }) || !seen.insert((
                    target.object.to_ascii_lowercase(),
                    target.texture.to_ascii_lowercase(),
                ))
            })
        {
            return Err(format!("{stage}: grass requires exact surface selectors"));
        }
        range("grass density", self.density, 0.1, 24.0)?;
        range("grass tile size", self.tile_size, 4.0, 24.0)?;
        range("grass minimum height", self.height[0], 0.08, 2.0)?;
        range("grass maximum height", self.height[1], self.height[0], 2.0)?;
        range("grass minimum width", self.width[0], 0.015, 0.35)?;
        range("grass maximum width", self.width[1], self.width[0], 0.35)?;
        color("grass root color", self.root_color)?;
        color("grass tip color", self.tip_color)?;
        range("grass ground slope", self.min_up, 0.55, 1.0)?;
        range("grass near distance", self.near_distance, 4.0, 42.0)?;
        range(
            "grass mid distance",
            self.mid_distance,
            self.near_distance + 2.0,
            66.0,
        )?;
        range(
            "grass view distance",
            self.max_distance,
            self.mid_distance + 3.0,
            240.0,
        )?;
        if !(4..=512).contains(&self.max_visible_tiles) {
            return Err("grass max_visible_tiles must be 4..=512".into());
        }
        if !(8_000..=180_000).contains(&self.max_triangles) {
            return Err("grass max_triangles must be 8000..=180000".into());
        }
        range("grass clump radius", self.clump_radius, 0.05, 0.6)?;
        range("grass patch size", self.patch_size, 0.5, 16.0)?;
        range("grass ground cover", self.ground_cover, 0.0, 1.0)?;
        let species = [
            self.species.grass,
            self.species.plumes,
            self.species.broadleaf,
        ];
        for weight in species {
            range("grass species weight", weight, 0.0, 1.0)?;
        }
        if species.into_iter().sum::<f32>() <= 0.0 {
            return Err("grass species must include a positive weight".into());
        }
        range("grass wind", self.wind_strength, 0.0, 0.55)?;
        range(
            "grass interaction radius",
            self.interaction_radius,
            0.5,
            2.4,
        )?;
        range("grass recovery", self.recovery_seconds, 0.25, 5.0)?;
        if let Some([min, max]) = self.bounds {
            for axis in 0..2 {
                range("grass bounds minimum", min[axis], -4096.0, 4096.0)?;
                range("grass bounds extent", max[axis] - min[axis], 1.0, 512.0)?;
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sky {
    pub enabled: bool,
    pub replace_skybox: bool,
    pub skybox: Vec<SkyIdentity>,
    pub horizon: [f32; 3],
    pub zenith: [f32; 3],
    pub cloud_color: [f32; 3],
    pub base_height: f32,
    pub thickness: f32,
    pub density: f32,
    pub coverage: f32,
    pub scale: f32,
    pub wind: [f32; 2],
    pub steps: u32,
    pub cloud_update_hz: u32,
    pub sun: Option<SkySun>,
    pub time: Option<SkyTime>,
    pub fixed_time: Option<String>,
    pub ambient_strength: f32,
    pub ground_bounce: f32,
}

impl Default for Sky {
    fn default() -> Self {
        Self {
            enabled: false,
            replace_skybox: false,
            skybox: Vec::new(),
            horizon: [0.54, 0.64, 0.78],
            zenith: [0.09, 0.23, 0.49],
            cloud_color: [0.90, 0.94, 1.0],
            base_height: 100.0,
            thickness: 38.0,
            density: 0.13,
            coverage: 0.57,
            scale: 0.017,
            wind: [1.7, 0.6],
            steps: 24,
            cloud_update_hz: 15,
            sun: None,
            time: None,
            fixed_time: None,
            ambient_strength: 1.0,
            ground_bounce: 0.24,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum SkyTime {
    Cycle { start_time: String, time_scale: f64 },
    System {},
    Fixed { time: String },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkyIdentity {
    pub object: String,
    pub texture: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkySun {
    pub direction: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
}

impl Sky {
    fn validate(&self, stage: &str) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        if (self.replace_skybox && (stage == "*" || self.skybox.is_empty()))
            || self.skybox.len() > 32
            || self.skybox.iter().any(|target| {
                [&target.object, &target.texture]
                    .into_iter()
                    .any(|s| s.trim().is_empty() || s.len() > 120 || s.contains(['*', '?']))
                    || !seen.insert((
                        target.object.to_ascii_lowercase(),
                        target.texture.to_ascii_lowercase(),
                    ))
            })
        {
            return Err(format!("{stage}: invalid skybox selectors"));
        }
        if !(8..=48).contains(&self.steps) {
            return Err(format!("{stage}: sky steps must be 8..=48"));
        }
        if !(1..=30).contains(&self.cloud_update_hz) {
            return Err(format!("{stage}: cloud_update_hz must be 1..=30"));
        }
        if self.time.is_some() && self.fixed_time.is_some() {
            return Err(format!("{stage}: specify either sky.time or fixed_time"));
        }
        if self
            .fixed_time
            .as_deref()
            .is_some_and(|value| sky_minutes(value).is_none())
        {
            return Err(format!(
                "{stage}: fixed_time must be HH:mm in 00:00..=23:59"
            ));
        }
        match &self.time {
            Some(SkyTime::Cycle {
                start_time,
                time_scale,
            }) => {
                if sky_minutes(start_time).is_none() {
                    return Err(format!(
                        "{stage}: sky.time.start_time must be HH:mm in 00:00..=23:59"
                    ));
                }
                if !time_scale.is_finite() || *time_scale <= 0.0 {
                    return Err(format!(
                        "{stage}: sky.time.time_scale must be finite and positive"
                    ));
                }
            }
            Some(SkyTime::Fixed { time }) if sky_minutes(time).is_none() => {
                return Err(format!(
                    "{stage}: sky.time.time must be HH:mm in 00:00..=23:59"
                ));
            }
            _ => {}
        }
        color("sky horizon", self.horizon)?;
        color("sky zenith", self.zenith)?;
        color("cloud color", self.cloud_color)?;
        range("cloud height", self.base_height, -100., 3000.)?;
        range("cloud thickness", self.thickness, 1., 1000.)?;
        range("cloud density", self.density, 0., 1.)?;
        range("cloud coverage", self.coverage, 0., 1.)?;
        range("cloud scale", self.scale, 0.0001, 1.)?;
        range("sky ambient strength", self.ambient_strength, 0., 2.)?;
        range("sky ground bounce", self.ground_bounce, 0., 1.)?;
        if let Some(sun) = &self.sun {
            validate_sun_direction(sun.direction)?;
            color("sky sun color", sun.color)?;
            range("sky sun intensity", sun.intensity, 0., 32.)?;
        }
        for speed in self.wind {
            range("cloud wind", speed, -40., 40.)?;
        }
        Ok(())
    }
}

pub(super) fn sky_minutes(value: &str) -> Option<u16> {
    let [h0, h1, b':', m0, m1] = *value.as_bytes() else {
        return None;
    };
    let digit = |byte: u8| byte.is_ascii_digit().then(|| u16::from(byte - b'0'));
    let hours = 10 * digit(h0)? + digit(h1)?;
    let minutes = 10 * digit(m0)? + digit(m1)?;
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshletModel {
    pub file: String,
    pub materials: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshletReplacement {
    pub object: String,
    pub model: String,
    #[serde(default)]
    pub placement: Placement,
    #[serde(default)]
    pub source_sha256: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Placement {
    pub offset: [f32; 3],
    pub yaw_degrees: f32,
    pub scale: f32,
}
impl Default for Placement {
    fn default() -> Self {
        Self {
            offset: [0.; 3],
            yaw_degrees: 0.,
            scale: 1.,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Environment {
    pub sky: [f32; 3],
    pub ground: [f32; 3],
    pub intensity: f32,
    pub diffuse: Option<String>,

    pub specular_mips: Vec<String>,
    pub rotation_degrees: f32,
    pub sun_direction: [f32; 3],
    pub sun_color: [f32; 3],
    pub sun_intensity: f32,
    pub exposure: f32,
}
impl Default for Environment {
    fn default() -> Self {
        Self {
            sky: [0.35, 0.42, 0.55],
            ground: [0.12, 0.10, 0.08],
            intensity: 1.,
            diffuse: None,
            specular_mips: vec![],
            rotation_degrees: 0.,
            sun_direction: [0.4, 0.8, 0.3],
            sun_color: [1., 0.94, 0.85],
            sun_intensity: 2.,
            exposure: 1.,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Wind {
    pub direction: [f32; 2],
    pub strength: f32,
    pub gust: f32,
    pub frequency: f32,
    pub spatial_scale: f32,
    pub response: Option<WindResponse>,
}
impl Default for Wind {
    fn default() -> Self {
        Self {
            direction: [1., 0.3],
            strength: 0.3,
            gust: 0.25,
            frequency: 0.7,
            spatial_scale: 0.08,
            response: None,
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindResponse {
    pub cloud: f32,
    pub water: f32,
    pub grass: f32,
    pub cloth: f32,
}
impl Default for WindResponse {
    fn default() -> Self {
        Self {
            cloud: 1.0,
            water: 1.0,
            grass: 1.0,
            cloth: 1.0,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Clothing {
    pub enabled: bool,

    pub density: f32,
    pub stretch_compliance: f32,
    pub bend_compliance: f32,

    pub damping: f32,
    pub wind_drag: f32,
    pub gravity_scale: f32,
    pub collision_friction: f32,
    pub thickness: f32,
    pub motion_limit: f32,
    pub iterations: u32,
    pub self_collision: bool,
    pub max_characters: usize,
    pub max_distance: f32,
}
impl Default for Clothing {
    fn default() -> Self {
        Self {
            enabled: true,
            density: 0.25,
            stretch_compliance: 0.0000001,
            bend_compliance: 100.,
            damping: 2.,
            wind_drag: 0.6,
            gravity_scale: 1.0,
            collision_friction: 0.0,
            thickness: 0.008,
            motion_limit: 0.4,
            iterations: 4,
            self_collision: false,
            max_characters: if cfg!(any(target_arch = "wasm32", target_os = "android")) {
                2
            } else {
                4
            },
            max_distance: 55.,
        }
    }
}

impl Clothing {
    fn validate(&self) -> Result<(), String> {
        range("clothing density", self.density, 0.05, 2.)?;
        range(
            "clothing stretch_compliance",
            self.stretch_compliance,
            0.,
            0.0001,
        )?;
        range("clothing bend_compliance", self.bend_compliance, 0., 10000.)?;
        range("clothing damping", self.damping, 0., 20.)?;
        range("clothing wind_drag", self.wind_drag, 0., 3.)?;
        range("clothing gravity_scale", self.gravity_scale, 0., 3.)?;
        range(
            "clothing collision_friction",
            self.collision_friction,
            0.,
            1.,
        )?;
        range("clothing thickness", self.thickness, 0.001, 0.03)?;
        range("clothing motion_limit", self.motion_limit, 0.02, 0.8)?;
        if !(1..=8).contains(&self.iterations) {
            return Err("clothing iterations must be 1–8".into());
        }
        if !(1..=16).contains(&self.max_characters) {
            return Err("clothing max_characters must be 1–16".into());
        }
        range("clothing max_distance", self.max_distance, 5., 100.)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CharacterCloth {
    pub character: String,
    pub material: String,
    pub enabled: bool,
    pub bones: [String; 4],
    pub grid: [usize; 2],
    pub width: f32,
    pub length: f32,
    pub back_offset: f32,
    pub collision_margin: f32,
    pub physics: Option<Clothing>,
}

impl Default for CharacterCloth {
    fn default() -> Self {
        Self {
            character: String::new(),
            material: String::new(),
            enabled: true,
            bones: ["bau_Neck", "bad_Pelvis", "bau_L_UpperArm", "bau_R_UpperArm"]
                .map(str::to_owned),
            grid: [7, 9],
            width: 0.95,
            length: 1.2,
            back_offset: 0.33,
            collision_margin: 0.016,
            physics: None,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContactShadows {
    pub strength: f32,
    pub radius: f32,
    pub max_height: f32,
    pub max_distance: f32,
}
impl Default for ContactShadows {
    fn default() -> Self {
        Self {
            strength: 0.3,
            radius: 1.2,
            max_height: 3.,
            max_distance: 80.,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fog {
    pub center: [f32; 3],
    pub radius: f32,
    pub color: [f32; 3],
    pub density: f32,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Surface {
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub texture: Option<String>,
    #[serde(default)]
    pub material: Option<String>,
    #[serde(default)]
    pub wind: Option<WindSurface>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindSurface {
    pub amplitude: f32,

    pub start_height: f32,
    pub end_height: f32,
    #[serde(default)]
    pub anchor_top: bool,
}
impl Surface {
    pub fn matches(&self, object: &str, texture: Option<&str>) -> bool {
        self.object
            .as_ref()
            .is_none_or(|v| v.eq_ignore_ascii_case(object))
            && self
                .texture
                .as_ref()
                .is_none_or(|v| texture.is_some_and(|t| v.eq_ignore_ascii_case(t)))
    }
}
pub fn relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['\\', ':', '\0'])
        && !value.starts_with('/')
        && value
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != "..")
}
fn range(name: &str, v: f32, min: f32, max: f32) -> Result<(), String> {
    if v.is_finite() && (min..=max).contains(&v) {
        Ok(())
    } else {
        Err(format!("{name}: expected {min}..{max}, got {v}"))
    }
}
fn color(name: &str, v: [f32; 3]) -> Result<(), String> {
    for x in v {
        range(name, x, 0., 16.)?;
    }
    Ok(())
}
fn validate_sun_direction(direction: [f32; 3]) -> Result<(), String> {
    if direction.iter().any(|v| !v.is_finite() || v.abs() > 10000.)
        || direction.iter().map(|v| v * v).sum::<f32>() < 0.001
    {
        return Err("invalid sun direction".into());
    }
    Ok(())
}
impl Pack {
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.schema_version, 1 | 2) || self.id.is_empty() || self.id.len() > 80 {
            return Err("invalid pack schema_version/id".into());
        }
        if self.resources.len() > 128 || self.materials.len() > 128 || self.scenes.len() > 64 {
            return Err("pack exceeds entry budget".into());
        }
        for (id, path) in &self.resources {
            if id.is_empty() || !relative_path(path) {
                return Err(format!("invalid resource {id}: {path}"));
            }
        }
        let has_meshlets = !self.meshlet_models.is_empty()
            || self
                .scenes
                .iter()
                .any(|s| !s.meshlet_replacements.is_empty());
        if has_meshlets && self.schema_version != 2 {
            return Err("meshlet replacements require pack schema_version 2".into());
        }
        if self.meshlet_models.len() > 64 {
            return Err("too many meshlet models".into());
        }
        for (id, model) in &self.meshlet_models {
            if id.is_empty()
                || id.len() > 80
                || !relative_path(&model.file)
                || !model.file.ends_with(".mlmesh")
                || !(1..=32).contains(&model.materials.len())
                || model.materials.iter().any(|(slot, material)| {
                    slot.is_empty() || slot.len() > 80 || !self.materials.contains_key(material)
                })
            {
                return Err(format!("invalid meshlet model {id}"));
            }
        }
        for scene in &self.scenes {
            if scene.meshlet_replacements.is_empty() {
                continue;
            }
            if scene.stage == "*" || scene.meshlet_replacements.len() > 32 {
                return Err(
                    "meshlet replacements require an exact stage and at most 32 targets".into(),
                );
            }
            for target in &scene.meshlet_replacements {
                if target.object.is_empty()
                    || target.object.len() > 120
                    || target.object.contains('*')
                    || !self.meshlet_models.contains_key(&target.model)
                    || target.source_sha256.as_ref().is_some_and(|hash| {
                        hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                {
                    return Err(format!("invalid meshlet target {}", target.object));
                }
                for offset in target.placement.offset {
                    range("meshlet offset", offset, -1000., 1000.)?;
                }
                range("meshlet yaw", target.placement.yaw_degrees, -360., 360.)?;
                range("meshlet scale", target.placement.scale, 0.01, 100.)?;
            }
        }
        let resource = |id: &str| {
            if self.resources.contains_key(id) {
                Ok(())
            } else {
                Err(format!("unknown resource: {id}"))
            }
        };
        for p in self.materials.values() {
            for id in [&p.base_color, &p.normal, &p.orm].into_iter().flatten() {
                resource(id)?;
            }
            color("tint", p.tint)?;
            range("metallic", p.metallic, 0., 1.)?;
            range("roughness", p.roughness, 0.045, 1.)?;
            range("normal_strength", p.normal_strength, 0., 4.)?;
            range("vertex_color", p.vertex_color, 0., 1.)?;
        }
        for s in &self.scenes {
            if s.stage.is_empty() || s.surfaces.len() > 256 {
                return Err("invalid stage/surface budget".into());
            }
            if let Some(sky) = &s.sky {
                sky.validate(&s.stage)?;
            }
            if let Some(grass) = &s.grass {
                grass.validate(&s.stage)?;
            }
            if let Some(e) = &s.environment {
                color("sky", e.sky)?;
                color("ground", e.ground)?;
                color("sun_color", e.sun_color)?;
                range("environment intensity", e.intensity, 0., 16.)?;
                range("sun intensity", e.sun_intensity, 0., 32.)?;
                range("exposure", e.exposure, 0.01, 8.)?;
                range("rotation", e.rotation_degrees, -360., 360.)?;
                validate_sun_direction(e.sun_direction)?;
                if let Some(id) = &e.diffuse {
                    resource(id)?;
                }
                if e.specular_mips.len() > 12 {
                    return Err("too many environment mips".into());
                }
                for id in &e.specular_mips {
                    resource(id)?;
                }
            }
            if let Some(w) = &s.wind {
                if w.direction
                    .iter()
                    .any(|v| !v.is_finite() || v.abs() > 10000.)
                    || w.direction.iter().map(|v| v * v).sum::<f32>() < 0.001
                {
                    return Err("invalid wind direction".into());
                }
                range("wind strength", w.strength, 0., 4.)?;
                range("wind gust", w.gust, 0., 1.)?;
                range("wind frequency", w.frequency, 0., 4.)?;
                range("wind spatial_scale", w.spatial_scale, 0., 1.)?;
                if let Some(response) = w.response {
                    for gain in [
                        response.cloud,
                        response.water,
                        response.grass,
                        response.cloth,
                    ] {
                        range("wind response", gain, 0., 4.)?;
                    }
                }
            }
            if let Some(c) = &s.clothing {
                c.validate()?;
            }
            let mut characters = std::collections::HashSet::new();
            if s.character_cloth.len() > 16 {
                return Err("at most 16 character_cloth entries per scene".into());
            }
            for cloth in &s.character_cloth {
                let character = cloth.character.to_ascii_lowercase();
                if !character.strip_prefix('p').is_some_and(|id| {
                    !id.is_empty() && id.len() <= 3 && id.bytes().all(|b| b.is_ascii_digit())
                }) || !characters.insert(character)
                {
                    return Err(format!(
                        "invalid or duplicate character_cloth character {}",
                        cloth.character
                    ));
                }
                if !cloth.enabled {
                    continue;
                }
                if !self.materials.contains_key(&cloth.material) {
                    return Err(format!(
                        "unknown character_cloth material: {}",
                        cloth.material
                    ));
                }
                if cloth
                    .bones
                    .iter()
                    .any(|name| name.is_empty() || name.len() > 80)
                    || cloth
                        .bones
                        .iter()
                        .collect::<std::collections::HashSet<_>>()
                        .len()
                        != 4
                    || !(3..=12).contains(&cloth.grid[0])
                    || !(3..=16).contains(&cloth.grid[1])
                {
                    return Err("invalid character_cloth bones or grid".into());
                }
                range("character_cloth width", cloth.width, 0.25, 1.8)?;
                range("character_cloth length", cloth.length, 0.25, 2.)?;
                range("character_cloth back_offset", cloth.back_offset, 0.1, 0.7)?;
                range(
                    "character_cloth collision_margin",
                    cloth.collision_margin,
                    0.0,
                    0.08,
                )?;
                if let Some(c) = &cloth.physics {
                    c.validate()?;
                }
            }
            if let Some(c) = &s.contact_shadows {
                range("contact strength", c.strength, 0., 0.8)?;
                range("contact radius", c.radius, 0.1, 4.)?;
                range("contact height", c.max_height, 0.1, 8.)?;
                range("contact distance", c.max_distance, 1., 160.)?;
            }
            if let Some(fog) = &s.fog {
                if fog.len() > 8 {
                    return Err("at most 8 local fog volumes".into());
                }
                for f in fog {
                    color("fog color", f.color)?;
                    range("fog radius", f.radius, 0.1, 1000.)?;
                    range("fog density", f.density, 0., 0.5)?;
                    if f.center
                        .iter()
                        .any(|v| !v.is_finite() || v.abs() > 1_000_000.)
                    {
                        return Err("invalid fog center".into());
                    }
                }
            }
            for r in &s.surfaces {
                if r.object.as_ref().is_none_or(|s| s.is_empty())
                    && r.texture.as_ref().is_none_or(|s| s.is_empty())
                {
                    return Err("surface requires object or texture".into());
                }
                if let Some(id) = &r.material {
                    if !self.materials.contains_key(id) {
                        return Err(format!("unknown material: {id}"));
                    }
                }
                if let Some(w) = &r.wind {
                    range("wind amplitude", w.amplitude, 0., 2.)?;
                    if !w.start_height.is_finite()
                        || !w.end_height.is_finite()
                        || w.end_height - w.start_height < 0.0001
                        || w.start_height.abs() > 1_000_000.
                        || w.end_height.abs() > 1_000_000.
                    {
                        return Err("invalid wind height range".into());
                    }
                }
            }
        }
        Ok(())
    }
}
