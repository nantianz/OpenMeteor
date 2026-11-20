use super::{GmdlMaterial, GmdlMode};
use crate::plugins::graphics_pack::config::WaterSkySelection;
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use meteor_core::constants::MODEL_SCALE;
use std::collections::HashMap;

struct WaterSource {
    stage: &'static str,
    object: &'static str,
    texture: &'static str,

    bounds: [f32; 4],
    kind: f32,
    additive: bool,
    extinction: f32,
    max_depth: f32,
    vertex_alpha: f32,
    reflection: &'static [u8],
    depth: &'static [u8],
    sky_visibility: &'static [u8],
    sky_visibility_layout: [usize; 2],
    sky_visibility_skybox: &'static [(&'static str, &'static str)],
}

impl WaterSource {
    fn sky_visibility_matches(&self, sky: WaterSkySelection<'_>) -> bool {
        self.sky_visibility_layout
            == [
                sky.profile.visibility_resolution,
                sky.profile.visibility_directions,
            ]
            && self.sky_visibility_skybox.len() == sky.skybox.len()
            && self.sky_visibility_skybox.iter().zip(sky.skybox).all(
                |((object, texture), target)| {
                    object.eq_ignore_ascii_case(&target.object)
                        && texture.eq_ignore_ascii_case(&target.texture)
                },
            )
    }
}
#[cfg(not(feature = "original-assets"))]
const SURFACES: &[WaterSource] = &[];
#[cfg(feature = "original-assets")]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../asset-tools/generated/water/catalog.rs"
));
mod ripple_map;

fn source(stage: &str, object: &str, texture: &str) -> Option<&'static WaterSource> {
    let normalized = texture.replace('\\', "/");
    let texture = normalized.rsplit('/').next()?;
    SURFACES.iter().find(|s| {
        s.stage.eq_ignore_ascii_case(stage)
            && s.object == object
            && s.texture.eq_ignore_ascii_case(texture)
    })
}

fn data_image(bytes: &[u8], images: &mut Assets<Image>) -> Handle<Image> {
    let decoded = meteor_asset::texture::decode_image(bytes).expect("embedded water map");
    let mut image = Image::new(
        Extent3d {
            width: decoded.width,
            height: decoded.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        decoded.rgba,
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

#[derive(Default)]
pub(crate) struct WaterCache {
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

impl WaterCache {
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
        let Some(setting) = config::get()
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
        let prepared = bake_cache::PREPARED.get();
        #[cfg(any(target_arch = "wasm32", target_os = "android"))]
        let prepared: Option<()> = None;
        if prepared.is_some() {
            #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
            {
                let Some(s) = prepared
                    .unwrap()
                    .iter()
                    .find(|s| s.source.matches(stage, object, texture))
                else {
                    return false;
                };
                bounds = s.maps.bounds;
                vertex_alpha = s.maps.vertex_alpha;
                if let (Some(reflection), Some(depth)) = (&s.maps.reflection, &s.maps.depth) {
                    let maps = self.maps.entry(key).or_insert_with(|| WaterImages {
                        reflection: raw_image(reflection, images),
                        depth: raw_image(depth, images),
                        sky_visibility: sky
                            .and(s.maps.sky_visibility.as_ref())
                            .map(|map| raw_image(map, images)),
                    });
                    maps.attach(material);
                } else {
                    material.water_reflection = None;
                    material.water_depth = None;
                    material.water_sky_visibility = None;
                }
            }
            #[cfg(any(target_arch = "wasm32", target_os = "android"))]
            {
                unreachable!();
            }
        } else {
            let Some(s) = source(stage, object, texture) else {
                return false;
            };
            bounds = s.bounds;
            vertex_alpha = s.vertex_alpha;
            if setting.needs_maps() {
                let maps = self.maps.entry(key).or_insert_with(|| WaterImages {
                    reflection: data_image(s.reflection, images),
                    depth: data_image(s.depth, images),
                    sky_visibility: sky
                        .filter(|&sky| s.sky_visibility_matches(sky))
                        .map(|_| data_image(s.sky_visibility, images)),
                });
                maps.attach(material);
            } else {
                material.water_reflection = None;
                material.water_depth = None;
                material.water_sky_visibility = None;
            }
        }
        let ripple = self.ripple.get_or_insert_with(|| {
            super::super::pak_textures::image_from_decoded_with_quality(
                images,
                &ripple_map::generate(),
                0,
            )
        });
        material.water_normal = Some(ripple.clone());
        use profiles::WaterProfile;
        let profiles = config::profiles();
        let profile = match setting.clear_profile() {
            Some(WaterProfile::Deep) => Some((profiles.deep, 1.0, setting.max_depth)),
            Some(WaterProfile::Shallow) => Some((
                profiles.shallow_optics,
                2.0,
                profiles.shallow.fallback_depth.min(setting.max_depth),
            )),
            _ => None,
        };
        material.water = Vec4::new(
            setting.kind as f32,
            setting.max_depth,
            vertex_alpha,
            profile.map_or(setting.extinction, |(_, _, fallback_depth)| fallback_depth),
        );
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
        material.water_optics =
            profile.map_or_else(Default::default, |(p, style, _)| super::WaterOptics {
                absorption_ior: Vec3::from_array(p.absorption).extend(p.ior),
                scattering_g: Vec3::from_array(p.scattering).extend(p.anisotropy),
                control: Vec4::new(p.roughness, p.refraction_strength, p.max_path, style),
            });
        material.water_bounds = Vec4::new(
            bounds[0] - center[0] * MODEL_SCALE,
            bounds[1] + center[1] * MODEL_SCALE,
            bounds[2],
            bounds[3],
        );
        true
    }
}

#[cfg(all(test, feature = "original-assets"))]
mod tests {
    use super::*;
    use crate::plugins::stage_material::GmdlMaterialParams;

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
                    if let Some(s) = source(stage, &object.name, name) {
                        assert!(description.texture_animation.enabled);
                        assert!(!description.skip_render());
                        assert_eq!(description.billboard, 0);
                        assert!(description.deform.is_none() || s.kind == 2.0);
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
                        if s.kind < 3.0 {
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
        for s in SURFACES {
            mat.mode = if s.additive {
                GmdlMode::SrcColorOne
            } else {
                GmdlMode::Blend
            };
            mat.params.mode = mat.mode.as_u32();
            assert!(cache.configure(
                &mut mat,
                s.stage,
                s.object,
                s.texture,
                [10.0, 20.0, 0.0],
                &mut images,
                None,
            ));
            assert_eq!(mat.water_bounds.x, s.bounds[0] - 10.0 * MODEL_SCALE);
            assert_eq!(mat.water_bounds.y, s.bounds[1] + 20.0 * MODEL_SCALE);
            if s.kind < 3.0 && !s.additive {
                let image = images.get(mat.water_reflection.as_ref().unwrap()).unwrap();
                let face_size = if s.stage == "sn13" { 128 } else { 64 };
                assert_eq!(image.texture_descriptor.size.width, (face_size + 2) * 3);
                assert_eq!(image.texture_descriptor.size.height, (face_size + 2) * 2);
                assert_eq!(image.texture_descriptor.format, TextureFormat::Rgba8Unorm);
                assert_eq!(image.texture_descriptor.mip_level_count, 1);
                if s.stage == "sn13" {
                    let depth = meteor_asset::texture::decode_image(s.depth).unwrap();
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

pub(super) fn install(_app: &mut App) {
    config::initialize();
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
