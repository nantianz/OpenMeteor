use super::{GmdlMaterial, GmdlMode};
use crate::plugins::graphics_pack::{
    config::{SkyIdentity, WaterFiles, WaterSkySelection},
    GraphicsPacks,
};
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use meteor_asset::texture::DecodedImage;
use meteor_core::constants::MODEL_SCALE;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BakedEntry {
    stage: String,
    object: String,
    texture: String,
    kind: u8,
    additive: bool,
    bounds: [f32; 4],
    max_depth: f32,
    reflection_face: usize,
    vertex_alpha: f32,
    reflection: Option<String>,
    depth: Option<String>,
    sky_visibility: Option<BakedSky>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BakedSky {
    file: String,
    layout: [usize; 2],
    skybox: Vec<SkyIdentity>,
}
struct WaterSource {
    entry: BakedEntry,
    reflection: Option<DecodedImage>,
    depth: Option<DecodedImage>,
    sky_visibility: Option<DecodedImage>,
}

impl WaterSource {
    fn matches(&self, stage: &str, object: &str, texture: &str) -> bool {
        self.entry.stage.eq_ignore_ascii_case(stage)
            && self.entry.object == object
            && self
                .entry
                .texture
                .eq_ignore_ascii_case(texture.rsplit(['/', '\\']).next().unwrap_or(""))
    }
    fn sky_visibility_matches(&self, sky: WaterSkySelection<'_>) -> bool {
        self.entry.sky_visibility.as_ref().is_some_and(|baked| {
            baked.layout
                == [
                    sky.profile.visibility_resolution,
                    sky.profile.visibility_directions,
                ]
                && baked.skybox.len() == sky.skybox.len()
                && baked.skybox.iter().zip(sky.skybox).all(|(baked, target)| {
                    baked.object.eq_ignore_ascii_case(&target.object)
                        && baked.texture.eq_ignore_ascii_case(&target.texture)
                })
        })
    }
}
pub(crate) struct WaterPackage {
    config: config::WaterConfig,
    profiles: profiles::Profiles,
    ripples: DecodedImage,
    baked: Vec<WaterSource>,
}
impl WaterPackage {
    pub(crate) fn load(
        files: &WaterFiles,
        mut read: impl FnMut(&str) -> Result<Vec<u8>, String>,
    ) -> Result<Self, String> {
        let mut read_text = |name: &str| {
            let bytes = read(name)?;
            if bytes.len() > 1_048_576 {
                return Err(format!("{name}: water JSON exceeds 1 MiB"));
            }
            String::from_utf8(bytes).map_err(|error| format!("{name}: {error}"))
        };
        let config = config::WaterConfig::from_json(&read_text(&files.config)?)
            .map_err(|error| format!("{}: {error}", files.config))?;
        let profiles =
            profiles::Profiles::parse(&read_text(&files.deep)?, &read_text(&files.shallow)?)?;
        let entries: Vec<BakedEntry> = if let Some(file) = &files.baked {
            serde_json::from_str(&read_text(file)?).map_err(|error| format!("{file}: {error}"))?
        } else {
            Vec::new()
        };
        if entries.len() > 128 {
            return Err("water bake catalog exceeds 128 surfaces".into());
        }
        let ripples = super::super::pak_textures::decode_quality_image(&read(&files.ripples)?)
            .ok_or_else(|| format!("{}: invalid water ripple texture", files.ripples))?;
        let mut baked = Vec::with_capacity(entries.len());
        for entry in entries {
            if !entry.bounds.iter().all(|value| value.is_finite())
                || ![32, 64, 128, 256].contains(&entry.reflection_face)
            {
                return Err(format!(
                    "invalid water bake for {}/{}",
                    entry.stage, entry.object
                ));
            }
            let mut image = |name: &str, width: usize, height: usize| {
                let bytes = read(name)?;
                let dimensions =
                    [(width as u32).to_be_bytes(), (height as u32).to_be_bytes()].concat();
                if bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n")
                    || bytes.get(16..24) != Some(dimensions.as_slice())
                {
                    return Err(format!("{name}: expected {width} × {height} water PNG"));
                }
                meteor_asset::texture::decode_image(&bytes)
                    .map_err(|error| format!("{name}: {error}"))
            };
            let tile = entry.reflection_face + 2;
            let reflection = entry
                .reflection
                .as_deref()
                .map(|name| image(name, tile * 3, tile * 2))
                .transpose()?;
            let depth = entry
                .depth
                .as_deref()
                .map(|name| image(name, 128, 128))
                .transpose()?;
            let sky_visibility = entry
                .sky_visibility
                .as_ref()
                .map(|sky| {
                    let [cells, sectors] = sky.layout;
                    if !(1..=128).contains(&cells)
                        || !(4..=32).contains(&sectors)
                        || sectors % 4 != 0
                    {
                        return Err(format!("{}: invalid water sky visibility layout", sky.file));
                    }
                    image(&sky.file, cells * (sectors / 4), cells)
                })
                .transpose()?;
            baked.push(WaterSource {
                entry,
                reflection,
                depth,
                sky_visibility,
            });
        }
        Ok(Self {
            config,
            profiles,
            ripples,
            baked,
        })
    }

    pub(crate) fn enabled(&self) -> bool {
        self.config.surfaces.iter().any(|surface| surface.enabled)
    }
    pub(crate) fn live_reflections(&self) -> bool {
        self.enabled() && self.config.live_reflections
    }
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    pub(crate) fn depth_hierarchy(&self) -> bool {
        self.live_reflections() && self.config.depth_hierarchy
    }
    #[cfg(test)]
    fn fixture() -> &'static Self {
        static FIXTURE: std::sync::OnceLock<WaterPackage> = std::sync::OnceLock::new();
        FIXTURE.get_or_init(|| {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../config/graphics-packs/water");
            let pack: crate::plugins::graphics_pack::config::Pack =
                serde_json::from_slice(&std::fs::read(root.join("pack.json")).unwrap()).unwrap();
            Self::load(&pack.water.unwrap(), |name| {
                std::fs::read(root.join(name)).map_err(|error| error.to_string())
            })
            .unwrap()
        })
    }
}
#[cfg(test)]
fn source(stage: &str, object: &str, texture: &str) -> Option<&'static WaterSource> {
    WaterPackage::fixture()
        .baked
        .iter()
        .find(|source| source.matches(stage, object, texture))
}
fn data_image(decoded: &DecodedImage, images: &mut Assets<Image>) -> Handle<Image> {
    let mut image = Image::new(
        Extent3d {
            width: decoded.width,
            height: decoded.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        decoded.rgba.clone(),
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    images.add(image)
}

pub(crate) struct WaterCache<'a> {
    package: Option<&'a WaterPackage>,
    ripple: Option<Handle<Image>>,
    maps: HashMap<(String, String, String), WaterImages>,
}

struct WaterImages {
    reflection: Handle<Image>,
    depth: Handle<Image>,
    sky_visibility: Option<Handle<Image>>,
}

impl WaterImages {
    fn attach(&self, material: &mut GmdlMaterial) {
        material.water_reflection = Some(self.reflection.clone());
        material.water_depth = Some(self.depth.clone());
        material
            .water_sky_visibility
            .clone_from(&self.sky_visibility);
    }
}

#[cfg(test)]
impl Default for WaterCache<'static> {
    fn default() -> Self {
        Self::new(Some(WaterPackage::fixture()))
    }
}
impl<'a> WaterCache<'a> {
    pub(crate) fn new(package: Option<&'a WaterPackage>) -> Self {
        Self {
            package,
            ripple: None,
            maps: HashMap::new(),
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn configure(
        &mut self,
        material: &mut GmdlMaterial,
        stage: &str,
        object: &str,
        texture: &str,
        center: [f32; 3],
        images: &mut Assets<Image>,
        sky: Option<WaterSkySelection<'_>>,
    ) -> bool {
        if !matches!(material.mode, GmdlMode::Blend | GmdlMode::SrcColorOne)
            || material.params.alpha_cutoff > 0.0
        {
            return false;
        }
        let Some(package) = self.package else {
            return false;
        };
        let Some(setting) = package
            .config
            .surfaces
            .iter()
            .find(|s| s.enabled && s.matches(stage, object, texture))
        else {
            return false;
        };
        let sky = sky.filter(|_| setting.kind == 1);
        let additive = setting.blend_mode == config::BlendMode::SrcColorOne;
        let expected_mode = if additive {
            GmdlMode::SrcColorOne
        } else {
            GmdlMode::Blend
        };
        if material.mode != expected_mode {
            return false;
        }
        let key = (
            setting.stage.clone(),
            setting.object.clone(),
            setting.texture.clone(),
        );
        let (bounds, vertex_alpha);
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        let prepared = bake_cache::PREPARED
            .get()
            .and_then(|surfaces| surfaces.iter().find(|surface| surface.source == *setting));
        #[cfg(any(target_arch = "wasm32", target_os = "android"))]
        let prepared: Option<()> = None;
        material.water_reflection = None;
        material.water_depth = None;
        material.water_sky_visibility = None;
        if let Some(_prepared) = prepared {
            #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
            {
                bounds = _prepared.maps.bounds;
                vertex_alpha = _prepared.maps.vertex_alpha;
                if let (Some(reflection), Some(depth)) =
                    (&_prepared.maps.reflection, &_prepared.maps.depth)
                {
                    let maps = self.maps.entry(key).or_insert_with(|| WaterImages {
                        reflection: raw_image(reflection, images),
                        depth: raw_image(depth, images),
                        sky_visibility: sky
                            .and(_prepared.maps.sky_visibility.as_ref())
                            .map(|map| raw_image(map, images)),
                    });
                    maps.attach(material);
                }
            }
            #[cfg(any(target_arch = "wasm32", target_os = "android"))]
            unreachable!();
        } else {
            let Some(baked) = package.baked.iter().find(|baked| {
                baked.matches(stage, object, texture)
                    && baked.entry.kind == setting.kind
                    && baked.entry.additive == additive
                    && (!setting.needs_maps()
                        || (baked.entry.max_depth == setting.max_depth
                            && baked.entry.reflection_face == setting.reflection_face))
            }) else {
                return false;
            };
            bounds = baked.entry.bounds;
            vertex_alpha = baked.entry.vertex_alpha;
            if setting.needs_maps() {
                let (Some(reflection), Some(depth)) = (&baked.reflection, &baked.depth) else {
                    return false;
                };
                let maps = self.maps.entry(key).or_insert_with(|| WaterImages {
                    reflection: data_image(reflection, images),
                    depth: data_image(depth, images),
                    sky_visibility: sky
                        .filter(|&sky| baked.sky_visibility_matches(sky))
                        .and(baked.sky_visibility.as_ref())
                        .map(|map| data_image(map, images)),
                });
                maps.attach(material);
            }
        }
        let ripple = if let Some(ripple) = &self.ripple {
            ripple.clone()
        } else {
            let Some(ripple) = super::super::pak_textures::image_from_decoded_with_quality(
                images,
                package.ripples.clone(),
                0,
            ) else {
                warn!("水面波纹贴图无法解码");
                return false;
            };
            self.ripple = Some(ripple.clone());
            ripple
        };
        material.water_normal = Some(ripple);
        configure_optics(material, setting, &package.profiles, vertex_alpha);
        material.water_sky_reflection = sky.is_some() && material.water_sky_visibility.is_some();
        if let Some(sky) = sky {
            let p = sky.profile;
            material.water_sky_style = Vec4::new(
                p.wave_distortion,
                p.refraction_damping,
                p.grazing_boost,
                0.0,
            );
            material.water_sky_ranges = Vec4::new(
                p.grazing_range[0],
                p.grazing_range[1],
                p.horizon_fade[0],
                p.horizon_fade[1],
            );
        }
        material.water_bounds = Vec4::new(
            bounds[0] - center[0] * MODEL_SCALE,
            bounds[1] + center[1] * MODEL_SCALE,
            bounds[2],
            bounds[3],
        );
        true
    }
}

fn configure_optics(
    material: &mut GmdlMaterial,
    setting: &config::Surface,
    profiles: &profiles::Profiles,
    vertex_alpha: f32,
) {
    use profiles::WaterProfile;
    let optics = |p: profiles::Optics, style| super::WaterOptics {
        absorption_ior: Vec3::from_array(p.absorption).extend(p.ior),
        scattering_g: Vec3::from_array(p.scattering).extend(p.anisotropy),
        control: Vec4::new(p.roughness, p.refraction_strength, p.max_path, style),
    };
    let (optics, depth_or_extinction) = match setting.clear_profile() {
        Some(WaterProfile::Classic) => (
            super::WaterOptics {
                control: Vec4::W * 3.0,
                ..Default::default()
            },
            setting.extinction,
        ),
        Some(WaterProfile::Deep) => (optics(profiles.deep, 1.0), setting.max_depth),
        Some(WaterProfile::Shallow) => (
            optics(profiles.shallow_optics, 2.0),
            profiles.shallow.fallback_depth.min(setting.max_depth),
        ),
        _ => (Default::default(), setting.extinction),
    };
    material.water_optics = optics;
    material.water = Vec4::new(
        setting.kind as f32,
        setting.max_depth,
        vertex_alpha,
        depth_or_extinction,
    );
}

#[cfg(all(test, feature = "original-assets"))]
mod tests {
    use super::*;
    use crate::plugins::stage_material::GmdlMaterialParams;

    #[test]
    fn graphics_index_controls_water_material_admission() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
        for enabled in [false, true] {
            let index = serde_json::json!({
                "schema_version": 1,
                "enabled": enabled,
                "packs": ["graphics-packs/water/pack.json"]
            });
            let mut reads = 0;
            let graphics = crate::plugins::graphics_pack::load_with_reader(
                &serde_json::to_vec(&index).unwrap(),
                |name| {
                    reads += 1;
                    let bytes =
                        std::fs::read(root.join(name)).map_err(|error| error.to_string())?;
                    if name.ends_with("/water-enhancement.json")
                        || name.ends_with("/baked/catalog.json")
                    {
                        return Ok(String::from_utf8(bytes)
                            .unwrap()
                            .replace("\"sn13\"", "\"custom-water-map\"")
                            .into_bytes());
                    }
                    Ok(bytes)
                },
            )
            .unwrap();
            assert_eq!(reads != 0, enabled);
            assert_eq!(
                graphics.water().is_some_and(|water| water.enabled()),
                enabled
            );
            let mut cache = WaterCache::new(graphics.water().map(std::sync::Arc::as_ref));
            let mut images = Assets::default();
            let mut material = GmdlMaterial::new_scene(
                GmdlMaterialParams {
                    opacity: 0.6,
                    alpha_cutoff: 0.0,
                    mode: 1,
                    has_texture: 0,
                    uv_offset: Vec2::ZERO,
                },
                None,
                false,
                GmdlMode::Blend,
            );
            assert_eq!(
                cache.configure(
                    &mut material,
                    "custom-water-map",
                    "Plane02",
                    "sn13w01.jpg",
                    [0.0; 3],
                    &mut images,
                    None,
                ),
                enabled
            );
            assert_eq!(material.water_reflection.is_some(), enabled);
            assert!(!cache.configure(
                &mut material,
                "sn13",
                "Plane02",
                "sn13w01.jpg",
                [0.0; 3],
                &mut images,
                None,
            ));
        }
    }
    #[test]
    fn water_allowlist_rejects_smoke_clouds_waterfalls_and_lower_overlap() {
        for (stage, object, texture) in [
            ("sn06", "wu", "Asn06wu.jpg"),
            ("sn08", "Box06", "asn08fog.jpg"),
            ("sn08", "s03", "asn08fog.jpg"),
            ("sn08", "Plane02", "sn08du.jpg"),
            ("sn14", "Cylinder01", "Asn14019.jpg"),
            ("sn14", "Cylinder03", "Asn14020.jpg"),
            ("sn14", "Plane17", "sn14012.jpg"),
            ("sn06", "wu04", "Asn06wu.jpg"),
            ("sn06", "Plane30", "Asn06wu1.jpg"),
            ("sn13", "Sphere02", "am2sky1.jpg"),
            ("sn13", "Sphere03", "am2sky1.jpg"),
            ("sn06", "Plane20", "sn06sh.jpg"),
            ("sn06", "Plane19", "sn06sh2.jpg"),
            ("sn09", "Plane02", "sn914.jpg"),
            ("sn99", "Plane02", "sn13w01.jpg"),
            ("sn13", "Smoke", "sn13w01.jpg"),
            ("sn10", "Virus01", "sn1009.JPG"),
            ("sn10", "Virus03", "sn1009.JPG"),
            ("sn10", "fog01", "asn1008.bmp"),
        ] {
            assert!(
                source(stage, object, texture).is_none(),
                "{stage}/{object}/{texture}"
            );
        }
        assert!(source("SN13", "Plane02", "SN13\\SN13W01.JPG").is_some());
    }

    #[test]
    fn original_scene_water_matches_only_audited_upward_batches() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        for stage in ["sn06", "sn09", "sn13", "sn08", "sn10", "sn14"] {
            let gmdl = meteor_asset::gmdl::decode_gmdl(
                &std::fs::read(root.join(format!("{stage}/{stage}.gmb"))).unwrap(),
            )
            .unwrap();
            let bytes = std::fs::read(root.join(format!("{stage}/{stage}.des"))).unwrap();
            let (text, _, _) = encoding_rs::GBK.decode(&bytes);
            let des = meteor_asset::des::parse_stage_description(&text);
            let mut count = 0;
            let mut excluded_scrolling = 0;
            for object in &gmdl.objects {
                let Some(description) = des.scene_objects.iter().find(|s| s.name == object.name)
                else {
                    continue;
                };
                for index in object
                    .face_mat
                    .iter()
                    .copied()
                    .collect::<std::collections::HashSet<_>>()
                {
                    if index < 0 {
                        continue;
                    }
                    let mat = &gmdl.materials[index as usize];
                    let Some(name) = gmdl.resolve_texture_name(mat) else {
                        continue;
                    };
                    if let Some(s) = source(stage, &object.name, name).map(|source| &source.entry) {
                        assert!(description.texture_animation.enabled);
                        assert!(!description.skip_render());
                        assert_eq!(description.billboard, 0);
                        assert!(description.deform.is_none() || s.kind == 2);
                        let (mode, _, cutoff) =
                            crate::plugins::render_plugin::classify_gmdl_material(mat);
                        assert_eq!(
                            mode,
                            if s.additive {
                                GmdlMode::SrcColorOne
                            } else {
                                GmdlMode::Blend
                            }
                        );
                        assert_eq!(cutoff, 0.0);
                        if s.kind < 3 {
                            assert!(object.normals.chunks_exact(3).all(|n| n[2] > 0.8));
                        }
                        let alpha = object.colors.chunks_exact(4).map(|v| v[3]).max().unwrap()
                            as f32
                            / 255.0;
                        assert!((s.vertex_alpha - alpha).abs() < 0.00001);
                        let min_x = object
                            .positions
                            .chunks_exact(3)
                            .map(|p| p[0])
                            .fold(f32::INFINITY, f32::min)
                            * MODEL_SCALE;
                        assert!((s.bounds[0] - min_x).abs() < 0.001);
                        count += 1;
                    } else if description.texture_animation.enabled {
                        excluded_scrolling += 1;
                    }
                }
            }
            assert_eq!(
                count,
                if stage == "sn14" { 4 } else { 1 },
                "audited upper liquid layers in {stage}"
            );
            assert!(
                excluded_scrolling >= 2,
                "original non-water animation must remain excluded"
            );
        }
    }

    #[test]
    fn water_maps_are_linear_clamped_shared_and_centered() {
        let mut images = Assets::default();
        let mut cache = WaterCache::default();
        let mut mat = GmdlMaterial::new_scene(
            GmdlMaterialParams {
                opacity: 0.6,
                alpha_cutoff: 0.0,
                mode: 1,
                has_texture: 0,
                uv_offset: Vec2::ZERO,
            },
            None,
            false,
            GmdlMode::Blend,
        );
        for baked in &WaterPackage::fixture().baked {
            let s = &baked.entry;
            mat.mode = if s.additive {
                GmdlMode::SrcColorOne
            } else {
                GmdlMode::Blend
            };
            mat.params.mode = mat.mode.as_u32();
            assert!(cache.configure(
                &mut mat,
                &s.stage,
                &s.object,
                &s.texture,
                [10.0, 20.0, 0.0],
                &mut images,
                None,
            ));
            assert_eq!(mat.water_bounds.x, s.bounds[0] - 10.0 * MODEL_SCALE);
            assert_eq!(mat.water_bounds.y, s.bounds[1] + 20.0 * MODEL_SCALE);
            if s.kind < 3 && !s.additive {
                let image = images.get(mat.water_reflection.as_ref().unwrap()).unwrap();
                let face_size = if s.stage == "sn13" { 128 } else { 64 };
                assert_eq!(image.texture_descriptor.size.width, (face_size + 2) * 3);
                assert_eq!(image.texture_descriptor.size.height, (face_size + 2) * 2);
                assert_eq!(image.texture_descriptor.format, TextureFormat::Rgba8Unorm);
                assert_eq!(image.texture_descriptor.mip_level_count, 1);
                if s.stage == "sn13" {
                    let depth = baked.depth.as_ref().unwrap();
                    let valid: Vec<_> = depth.rgba.chunks_exact(4).filter(|p| p[2] > 200).collect();
                    assert!(
                        valid.iter().any(|p| p[3] < 64),
                        "bank must remain transparent"
                    );
                    assert!(
                        valid.iter().any(|p| p[3] > 240),
                        "river center must retain body color"
                    );
                }
            } else {
                assert!(mat.water_reflection.is_none());
                assert!(mat.water_depth.is_none());
            }
        }
        let count = images.len();
        cache.configure(
            &mut mat,
            "sn13",
            "Plane02",
            "sn13w01.jpg",
            [0.0; 3],
            &mut images,
            None,
        );
        assert_eq!(images.len(), count);
        mat.mode = GmdlMode::Add;
        assert!(!cache.configure(
            &mut mat,
            "sn13",
            "Plane02",
            "sn13w01.jpg",
            [0.0; 3],
            &mut images,
            None,
        ));
    }
}

#[cfg(all(test, feature = "original-assets"))]
mod render_tests;

#[cfg(all(
    test,
    feature = "original-assets",
    not(any(target_arch = "wasm32", target_os = "android"))
))]
mod live_reflection_tests;
#[cfg(all(test, feature = "original-assets"))]
mod scene_render_tests;

pub(crate) mod interaction;

pub(crate) mod waves;

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub(crate) mod depth_hierarchy;
mod profiles;
pub(crate) mod screen_reflection;
mod shallow;
mod splash;
mod spray;

mod physics;

pub(crate) mod combat;

mod dynamics;

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
mod bake;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
mod bake_cache;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
mod bake_rays;
mod config;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub(crate) mod startup;

pub(super) fn install(app: &mut App) {
    if let Some(water) = app
        .world()
        .get_resource::<GraphicsPacks>()
        .and_then(GraphicsPacks::water)
    {
        config::initialize(water.clone());
    }
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn raw_image(map: &bake::Map, images: &mut Assets<Image>) -> Handle<Image> {
    let mut image = Image::new(
        Extent3d {
            width: map.width,
            height: map.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        map.rgba.clone(),
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    images.add(image)
}
