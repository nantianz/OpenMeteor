use bevy::prelude::*;
use std::collections::HashMap;

mod generated {
    #[cfg(not(feature = "original-assets"))]
    pub(super) const NORMAL_MAPS: &[(&str, &str, &[u8])] = &[];
    #[cfg(feature = "original-assets")]
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../asset-tools/generated/scene-normals/catalog.rs"
    ));
}

pub(crate) fn load_scene_normal(
    stage: &str,
    name: &str,
    images: &mut Assets<Image>,
    cache: &mut HashMap<String, Handle<Image>>,
    quality: u8,
) -> Option<Handle<Image>> {
    let name = name.replace('\\', "/");
    let base = name.rsplit('/').next()?.to_ascii_lowercase();
    let (_, _, bytes) = generated::NORMAL_MAPS
        .iter()
        .find(|(scope, texture, _)| scope.eq_ignore_ascii_case(stage) && *texture == base)?;
    let key = format!("{}/{base}", stage.to_ascii_lowercase());
    if let Some(handle) = cache.get(&key) {
        return Some(handle.clone());
    }

    let handle = super::super::pak_textures::image_handle_with_quality(images, bytes, quality)?;
    cache.insert(key, handle.clone());
    Some(handle)
}

pub(crate) fn load_hd_scene_normal(
    stage: &str,
    name: &str,
    bytes: &[u8],
    images: &mut Assets<Image>,
    cache: &mut HashMap<String, Handle<Image>>,
    quality: u8,
) -> Option<Handle<Image>> {
    let base = name.replace('\\', "/");
    let base = base.rsplit('/').next()?.to_ascii_lowercase();
    let key = format!("hd/{}/{base}", stage.to_ascii_lowercase());
    if let Some(handle) = cache.get(&key) {
        return Some(handle.clone());
    }
    let handle = super::super::pak_textures::image_handle_with_quality(images, bytes, quality)?;
    cache.insert(key, handle.clone());
    Some(handle)
}

pub(super) fn sync_normal_setting(
    settings: Res<crate::app_state::Settings>,
    mut frame: ResMut<super::FrameUniforms>,
) {
    frame.scene_normal_maps = settings.scene_normal_maps;
    frame.water_enhance = settings.water_enhance;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "original-assets")]
    #[test]
    fn curated_normals_decode_are_linear_and_do_not_match_other_scopes() {
        let mut images = Assets::<Image>::default();
        let mut cache = HashMap::new();
        for (stage, name, bytes) in generated::NORMAL_MAPS {
            let decoded = meteor_asset::texture::decode_image(bytes).unwrap();
            for pixel in decoded.rgba.chunks_exact(4) {
                let n = Vec3::new(pixel[0] as f32, pixel[1] as f32, pixel[2] as f32) / 127.5
                    - Vec3::ONE;
                assert!((n.length() - 1.0).abs() < 0.015);
                assert!(n.z > 0.0);
            }
            let handle = load_scene_normal(stage, name, &mut images, &mut cache, 0).unwrap();
            let repeat =
                load_scene_normal(stage, &name.to_uppercase(), &mut images, &mut cache, 0).unwrap();
            assert_eq!(handle, repeat);
            let image = images.get(&handle).unwrap();
            assert_eq!(
                image.texture_descriptor.format,
                bevy::render::render_resource::TextureFormat::Rgba8Unorm
            );
            assert!(image.texture_descriptor.mip_level_count > 1);
            assert!(load_scene_normal("sn99", name, &mut images, &mut cache, 0).is_none());
        }
        assert!(load_scene_normal("sn01", "sn01sky.jpg", &mut images, &mut cache, 0).is_none());
        assert!(load_scene_normal("sn01", "s53f100.jpg", &mut images, &mut cache, 0).is_none());
    }

    #[test]
    fn hd_scene_normals_accept_arbitrary_map_scopes_and_cache_by_map() {
        let mut images = Assets::<Image>::default();
        let mut cache = HashMap::new();
        let bytes: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 2, 0, 0, 0, 253, 212, 154, 115, 0, 0, 0, 16, 73, 68, 65, 84, 120, 156, 99, 104, 104, 248, 15, 68, 12, 16, 10, 0, 53, 246, 7, 253, 73, 40, 67, 204, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130];
        let hd = load_hd_scene_normal(
            "custom_fortress",
            "WALL.BMP",
            bytes,
            &mut images,
            &mut cache,
            0,
        )
        .unwrap();
        let repeated = load_hd_scene_normal(
            "CUSTOM_FORTRESS",
            "wall.bmp",
            bytes,
            &mut images,
            &mut cache,
            0,
        )
        .unwrap();
        let another_map =
            load_hd_scene_normal("another_map", "wall.bmp", bytes, &mut images, &mut cache, 0)
                .unwrap();
        assert_eq!(hd, repeated);
        assert_ne!(hd, another_map);
        assert_eq!(
            images.get(&hd).unwrap().texture_descriptor.format,
            bevy::render::render_resource::TextureFormat::Rgba8Unorm
        );
        assert!(load_hd_scene_normal(
            "custom_fortress",
            "broken.bmp",
            b"invalid image",
            &mut images,
            &mut cache,
            0
        )
        .is_none());
    }
}

#[cfg(all(test, feature = "original-assets"))]
mod render_tests;
