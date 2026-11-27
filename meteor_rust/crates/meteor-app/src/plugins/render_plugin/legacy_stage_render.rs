use bevy::{prelude::*, render::render_resource::ShaderType};
use meteor_asset::gmdl::GmdlMesh;
use meteor_core::constants::MODEL_SCALE;
use meteor_scene::scene_script_runtime::SceneScriptRuntime;
use serde::Deserialize;

use crate::plugins::{camera_plugin::CAMERA_FAR, graphics_pack::effects::glob_match};

const POINT_LIGHTS: usize = 255;
const POINT_LIGHTS_PER_CELL: usize = 16;
const POINT_GRID_SIDE: usize = 16;
const POINT_GRID_CELLS: usize = POINT_GRID_SIDE * POINT_GRID_SIDE;
pub(super) const CONFIGURED: u32 = 1;
pub(super) const UNLIT: u32 = 2;
pub(super) const SKY: u32 = 4;

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct StageRenderConfig {
    version: u32,
    global: Global,
    lights: Vec<Light>,
    objects: Vec<ObjectRule>,
}

#[derive(Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Global {
    enabled: bool,
    ambient_color: Option<[u8; 4]>,
    shadowmap_size: Option<String>,
    enable_hollow_effect: bool,
    hollow_threshold: f32,
    strict_render_two_side: Option<bool>,
    fog_type: FogType,
    fog_start: f32,
    fog_end: f32,
    fog_color: [f32; 3],
    fog_height: f32,
    fog_height_scale: f32,
}

impl Default for Global {
    fn default() -> Self {
        Self {
            enabled: true,
            ambient_color: None,
            shadowmap_size: None,
            enable_hollow_effect: false,
            hollow_threshold: 0.0,
            strict_render_two_side: None,
            fog_type: FogType::None,
            fog_start: 0.0,
            fog_end: 1.0,
            fog_color: [0.0; 3],
            fog_height: 0.0,
            fog_height_scale: 0.0,
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize)]
enum FogType {
    Common,
    Height,
    #[default]
    #[serde(other)]
    None,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize)]
enum LightType {
    Directional,
    Point,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Light {
    name: String,
    enabled: bool,
    light_type: LightType,
    position: [f32; 3],
    rotation: [f32; 4],
    color: [u8; 4],
    color_full_range: Option<[f32; 4]>,
    brightness: f32,
    range: f32,
    bind_to_object: bool,
    shadows_enabled: bool,
    shadow_strength: f32,
    pcf_row: u32,
    normal_offset: Option<f32>,
    shadow_split_log_factor: f32,
    blend_across_cascade: bool,
    shadow_max_distance: f32,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            light_type: LightType::Unknown,
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            color: [255; 4],
            color_full_range: None,
            brightness: 1.0,
            range: 0.0,
            bind_to_object: false,
            shadows_enabled: false,
            shadow_strength: 0.0,
            pcf_row: 0,
            normal_offset: None,
            shadow_split_log_factor: 0.9,
            blend_across_cascade: false,
            shadow_max_distance: 1.0,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct ObjectRule {
    name: String,
    pub(super) affected_by_light: Option<bool>,
    pub(super) is_sky_box: bool,
    #[serde(flatten)]
    pub(super) shadows: ShadowRule,
}

#[derive(Component, Clone, Copy, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ShadowRule {
    pub(crate) cast_shadows: Option<bool>,
    pub(crate) receive_shadows: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub(crate) struct StageRenderFrame {
    pub ambient: Vec4,

    pub sun: Vec4,
    pub sun_color: Vec4,
    pub fog_params: Vec4,

    pub fog_color: Vec4,
    pub point_count: Vec4,
    pub point_positions: [Vec4; POINT_LIGHTS],
    pub point_colors: [Vec4; POINT_LIGHTS],

    pub point_grid: Vec4,

    pub point_cells: [UVec4; POINT_GRID_CELLS],
}

impl Default for StageRenderFrame {
    fn default() -> Self {
        Self {
            ambient: Vec4::ZERO,
            sun: Vec4::ZERO,
            sun_color: Vec4::ZERO,
            fog_params: Vec4::ZERO,
            fog_color: Vec4::ZERO,
            point_count: Vec4::ZERO,
            point_positions: [Vec4::ZERO; POINT_LIGHTS],
            point_colors: [Vec4::ZERO; POINT_LIGHTS],
            point_grid: Vec4::ZERO,
            point_cells: [UVec4::ZERO; POINT_GRID_CELLS],
        }
    }
}

#[derive(Resource)]
pub(crate) struct SceneRenderProfile {
    pub(crate) frame: StageRenderFrame,
    pub(crate) sun: Option<SceneSun>,
    point_bindings: Vec<(usize, usize)>,
}

impl SceneRenderProfile {
    pub(crate) fn frame_at(&self, script: Option<&SceneScriptRuntime>) -> StageRenderFrame {
        let mut frame = self.frame;
        if let Some(script) = script {
            for &(light, object) in &self.point_bindings {
                if script
                    .host()
                    .instance(object)
                    .is_some_and(|object| object.get("active").as_f64() == 0.0)
                {
                    frame.point_colors[light] = Vec4::ZERO;
                }
            }
        }
        frame
    }
}

const HEAVY_SCENE_SHADOW_FACES: usize = 80_000;

pub(crate) struct SceneSun {
    pub(crate) direction: Vec3,
    pub(crate) shadows_enabled: bool,
    pub(crate) shadowmap_size: Option<usize>,
    pub(crate) normal_offset: Option<f32>,
    pub(crate) heavy_scene: bool,
    split_log_factor: f32,
    blend_across_cascade: bool,
    max_distance: f32,
}

impl SceneSun {
    pub(crate) fn cascades(&self) -> bevy::light::CascadeShadowConfig {
        let near = 2.0 * MODEL_SCALE;
        let far = (self.max_distance.clamp(0.0, 1.0) * CAMERA_FAR).max(near + 1.0);
        let log_weight = self.split_log_factor.clamp(0.0, 1.0);

        let bounds = (1..=4)
            .map(|index| {
                let t = index as f32 * 0.25;
                let linear = near + (far - near) * t;
                let logarithmic = near * (far / near).powf(t);
                linear + (logarithmic - linear) * log_weight
            })
            .collect();
        bevy::light::CascadeShadowConfig {
            bounds,
            overlap_proportion: if self.blend_across_cascade { 0.1 } else { 0.0 },
            minimum_distance: near,
        }
    }
}

impl StageRenderConfig {
    pub(super) fn load(stage_id: &str) -> Option<Self> {
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        let original_directory = meteor_asset::runtime_assets::original_directory_active();
        #[cfg(any(target_arch = "wasm32", target_os = "android"))]
        let original_directory = false;
        if !original_directory && meteor_scene::stage_registry::is_official_stage_id(stage_id) {
            return None;
        }
        let bytes = crate::resource_io::read(format!("{stage_id}/render.json")).ok()?;
        match Self::parse(&bytes) {
            Ok(config) => config,
            Err(error) => {
                warn!("invalid {stage_id}/render.json: {error}");
                None
            }
        }
    }

    fn parse(bytes: &[u8]) -> Result<Option<Self>, serde_json::Error> {
        let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
        let mut config: Self = serde_json::from_slice(bytes)?;
        for rule in &mut config.objects {
            rule.name.make_ascii_lowercase();
        }
        Ok((config.version == 0 && config.global.enabled).then_some(config))
    }

    pub(super) fn hollow_cutoff(&self) -> Option<f32> {
        let cutoff = self.global.hollow_threshold;
        (self.global.enable_hollow_effect && cutoff > 0.0 && cutoff <= 1.0).then_some(cutoff)
    }

    pub(super) fn two_side(&self, authored: bool) -> bool {
        authored || self.global.strict_render_two_side == Some(false)
    }

    pub(super) fn object_rule(&self, name: &str) -> Option<&ObjectRule> {
        let name = name.to_ascii_lowercase();
        self.objects
            .iter()
            .rev()
            .find(|rule| rule.name == name)
            .or_else(|| {
                self.objects
                    .iter()
                    .rev()
                    .find(|rule| !rule.name.is_empty() && glob_match(&rule.name, &name))
            })
    }

    pub(super) fn material_flags(rule: Option<&ObjectRule>) -> u32 {
        let mut flags = CONFIGURED;
        if let Some(rule) = rule {
            if rule.affected_by_light == Some(false) {
                flags |= UNLIT;
            }
            if rule.is_sky_box {
                flags |= SKY;
            }
        }
        flags
    }

    pub(super) fn prepare(
        &self,
        stage: &GmdlMesh,
        center: [f32; 3],
        script: Option<&SceneScriptRuntime>,
    ) -> SceneRenderProfile {
        let global = &self.global;
        let mut frame = StageRenderFrame::default();
        let fog_mode = match global.fog_type {
            FogType::None => 0.0,
            FogType::Common => 1.0,
            FogType::Height => 2.0,
        };
        if global.fog_end > global.fog_start {
            frame.fog_params = Vec4::new(
                global.fog_end,
                (global.fog_end - global.fog_start).recip(),
                (global.fog_height - center[2]) * MODEL_SCALE,
                global.fog_height_scale / MODEL_SCALE,
            );
            frame.fog_color = Vec3::from_array(global.fog_color).extend(fog_mode);
        }
        if let Some(color) = global.ambient_color {
            frame.ambient = byte_color(color).extend(1.0);
        }
        let total_faces: usize = stage.objects.iter().map(|object| object.num_faces).sum();
        let heavy_scene = total_faces > HEAVY_SCENE_SHADOW_FACES;
        let sun = self
            .lights
            .iter()
            .find(|light| light.enabled && light.light_type == LightType::Directional)
            .map(|light| {
                let rotation = Quat::from_array(light.rotation);
                let direction = if rotation.length_squared() > f32::EPSILON {
                    rotation.normalize() * Vec3::Z
                } else {
                    Vec3::Z
                };
                let direction = original_axes(direction);
                frame.sun = direction.extend(light.shadow_strength.clamp(0.0, 1.0));
                frame.sun_color = light.rgb().extend(1.0);
                frame.ambient.w = 1.0;
                frame.point_count.z = light.pcf_row.min(5).max(1) as f32;
                SceneSun {
                    direction,
                    shadows_enabled: light.shadows_enabled,
                    shadowmap_size: global.shadowmap_size.as_deref().and_then(shadow_resolution),
                    normal_offset: light.normal_offset,
                    heavy_scene,
                    split_log_factor: light.shadow_split_log_factor,
                    blend_across_cascade: light.blend_across_cascade,
                    max_distance: light.shadow_max_distance,
                }
            });
        let mut point_count = 0;
        let mut point_bindings = Vec::new();
        for light in self.lights.iter().filter(|light| {
            light.enabled && light.light_type == LightType::Point && light.range > 0.0
        }) {
            let mut position = Vec3::from_array(light.position);
            if light.bind_to_object {
                let Some(anchor) = object_center(stage, &light.name) else {
                    continue;
                };
                position += anchor;
            }
            if point_count >= POINT_LIGHTS {
                warn!("render.json point light limit reached ({POINT_LIGHTS})");
                break;
            }
            if light.bind_to_object {
                if let Some(object) =
                    script.and_then(|script| script.host().resolve_name(&light.name))
                {
                    point_bindings.push((point_count, object));
                }
            }
            let position = original_axes(position - Vec3::from_array(center)) * MODEL_SCALE;
            frame.point_positions[point_count] =
                position.extend((light.range * MODEL_SCALE).recip());
            frame.point_colors[point_count] = light.rgb().extend(0.0);
            point_count += 1;
        }
        frame.point_count.x = point_count as f32;
        frame.point_count.y = CAMERA_FAR.recip();
        if point_count != 0 {
            frame.ambient.w = 1.0;
        }
        if point_count > POINT_LIGHTS_PER_CELL {
            let (min, max) =
                self.point_bounds(stage, center, &frame.point_positions[..point_count]);
            frame.build_point_cells(min, max, point_count);
        }
        SceneRenderProfile {
            frame,
            sun,
            point_bindings,
        }
    }

    fn point_bounds(&self, stage: &GmdlMesh, center: [f32; 3], lights: &[Vec4]) -> (Vec2, Vec2) {
        let (mut min, mut max) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
        for object in &stage.objects {
            if self
                .object_rule(&object.name)
                .is_some_and(|rule| rule.is_sky_box)
            {
                continue;
            }
            for position in object.positions.chunks_exact(3) {
                let position =
                    Vec2::new(position[0] - center[0], center[1] - position[1]) * MODEL_SCALE;
                min = min.min(position);
                max = max.max(position);
            }
        }
        if !min.is_finite() || !max.is_finite() {
            for light in lights {
                min = min.min(light.xz());
                max = max.max(light.xz());
            }
        }
        (min, max)
    }
}

impl StageRenderFrame {
    fn build_point_cells(&mut self, min: Vec2, max: Vec2, count: usize) {
        let cell_size = (max - min).max(Vec2::ONE) / POINT_GRID_SIDE as f32;
        self.point_grid = min.extend(cell_size.x.recip()).extend(cell_size.y.recip());
        let mut candidates = Vec::with_capacity(count);
        for z in 0..POINT_GRID_SIDE {
            for x in 0..POINT_GRID_SIDE {
                let lower = min + Vec2::new(x as f32, z as f32) * cell_size;
                let upper = lower + cell_size;
                candidates.clear();
                for index in 0..count {
                    let light = self.point_positions[index];
                    let position = light.xz();
                    let closest = position.clamp(lower, upper);
                    let normalized = (closest - position) * light.w;
                    let weight = (1.0 - normalized.length_squared())
                        * self.point_colors[index].truncate().max_element();
                    if weight > 0.0 {
                        candidates.push((index, weight));
                    }
                }

                candidates.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                let cell = &mut self.point_cells[z * POINT_GRID_SIDE + x];
                *cell = UVec4::ZERO;
                for (slot, &(index, _)) in candidates.iter().take(POINT_LIGHTS_PER_CELL).enumerate()
                {
                    cell[slot / 4] |= (index as u32 + 1) << ((slot % 4) * 8);
                }
            }
        }
    }
}

impl Light {
    fn rgb(&self) -> Vec3 {
        let color = self.color_full_range.map_or_else(
            || byte_color(self.color),
            |rgb| Vec4::from_array(rgb).truncate(),
        );
        color.max(Vec3::ZERO) * self.brightness.max(0.0)
    }
}

fn byte_color(color: [u8; 4]) -> Vec3 {
    Vec4::from_array(color.map(|value| f32::from(value) / 255.0)).truncate()
}

fn original_axes(vector: Vec3) -> Vec3 {
    Vec3::new(vector.x, vector.z, -vector.y)
}

fn object_center(stage: &GmdlMesh, name: &str) -> Option<Vec3> {
    let object = stage
        .objects
        .iter()
        .find(|object| object.name.eq_ignore_ascii_case(name))?;
    let mut points = object
        .positions
        .chunks_exact(3)
        .map(|pos| Vec3::from_slice(pos));
    let first = points.next()?;
    let (min, max) = points.fold((first, first), |(min, max), point| {
        (min.min(point), max.max(point))
    });
    Some((min + max) * 0.5)
}

fn shadow_resolution(size: &str) -> Option<usize> {
    let (width, height) = size.split_once('x')?;
    let width: usize = width.parse().ok()?;
    let height = height.parse::<usize>().ok()?;

    (width == height && width.is_power_of_two() && (256..=4096).contains(&width))
        .then_some(width / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "original-assets")]
    #[test]
    fn original_render_json_profiles_resolve_lighting_fog_and_object_rules() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        for stage in 1..=22 {
            let id = format!("sn{stage:02}");
            let dir = root.join(&id);
            let bytes = std::fs::read(dir.join("render.json")).unwrap();
            let config = StageRenderConfig::parse(&bytes).unwrap().unwrap();
            let gmb = std::fs::read(dir.join(format!("{id}.gmb"))).unwrap();
            let model = meteor_asset::gmdl::decode_gmdl(&gmb).unwrap();
            let profile = config.prepare(&model, [0.0; 3], None);
            assert!(profile.sun.is_some(), "{id}");
            assert!(profile.frame.ambient.w > 0.0, "{id}");
            if stage == 1 {
                let rule = config.object_rule("plane08").unwrap();
                assert!(rule.is_sky_box);
                assert!(profile.frame.point_count.x >= 6.0);
            }
            if stage == 6 {
                let sun = profile.sun.unwrap();
                assert_eq!(sun.shadowmap_size, Some(512));
                assert_eq!(sun.cascades().bounds[3], CAMERA_FAR);
                assert_eq!(profile.frame.fog_color.w, 1.0);
                assert!((profile.frame.sun_color.x - 0.79692 * 0.8).abs() < 0.00001);
                assert_eq!(config.hollow_cutoff(), Some(0.3));
                assert!(config.two_side(false));
            }
            if stage == 13 {
                assert_eq!(profile.frame.fog_color.w, 2.0);
            }
        }
    }

    #[test]
    fn render_json_version_and_global_switch_disable_the_profile() {
        assert!(StageRenderConfig::parse(br#"{"version":1}"#)
            .unwrap()
            .is_none());
        assert!(StageRenderConfig::parse(br#"{"global":{"enabled":false}}"#)
            .unwrap()
            .is_none());
    }

    #[test]
    fn authored_light_grid_limits_each_cell_without_losing_the_last_point_index() {
        let mut frame = StageRenderFrame::default();
        for position in &mut frame.point_positions[..20] {
            *position = Vec4::new(1.0, 0.0, 1.0, 0.1);
        }
        for position in &mut frame.point_positions[20..254] {
            *position = Vec4::new(151.0, 0.0, 151.0, 0.1);
        }
        frame.point_positions[254] = frame.point_positions[0];
        frame.point_colors = [Vec4::ONE; POINT_LIGHTS];
        frame.point_colors[254] = Vec4::splat(2.0);
        frame.build_point_cells(Vec2::ZERO, Vec2::splat(160.0), POINT_LIGHTS);
        let cell = frame.point_cells[0]
            .to_array()
            .map(u32::to_le_bytes)
            .concat();
        assert_eq!(cell.len(), POINT_LIGHTS_PER_CELL);
        assert_eq!(cell[0], 255);
        assert_eq!(&cell[1..], (1u8..=15).collect::<Vec<_>>().as_slice());
        assert_eq!(frame.point_cells[8 * POINT_GRID_SIDE + 8], UVec4::ZERO);
        let distant = frame.point_cells[POINT_GRID_CELLS - 1]
            .to_array()
            .map(u32::to_le_bytes)
            .concat();
        assert!(distant.iter().all(|&index| (21..=254).contains(&index)));
        assert!(crate::plugins::graphics_pack::GraphicsFrame::min_size().get() <= 16 * 1024);
    }
}
