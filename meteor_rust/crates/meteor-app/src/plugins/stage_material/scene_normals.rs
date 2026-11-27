use bevy::prelude::*;
use std::collections::HashMap;

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

#[cfg(all(test, not(any(target_arch = "wasm32", target_os = "android"))))]
mod tests {
    use super::*;

    #[cfg(feature = "original-assets")]
    #[test]
    fn hd_scene_normals_accept_arbitrary_map_scopes_and_cache_by_map() {
        let mut images = Assets::<Image>::default();
        let mut cache = HashMap::new();
        let bytes = &std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/graphics-packs/scene-normals/textures/sn01/sn01-s.jpg.normal.png",
        ))
        .unwrap();
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

#[cfg(all(test, not(any(target_arch = "wasm32", target_os = "android"))))]
mod render_tests;
