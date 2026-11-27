use bevy::{
    asset::RenderAssetUsages,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    prelude::*,
};
use std::collections::HashMap;

#[derive(Resource, Default)]
pub(super) struct PreviewLibrary(pub std::collections::BTreeMap<String, std::path::PathBuf>);

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
mod bake;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
mod desktop;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub(super) use desktop::StartupProgress;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub(crate) use {bake::bake, desktop::install};

#[derive(Component)]
pub(super) struct MapImage;

#[derive(Component)]
pub(super) struct StageThumbnail(pub String);

#[derive(Component)]
pub(super) struct MapPreviewHint;

pub(super) fn update(
    request: Res<crate::app_state::LaunchRequest>,
    stage_rows: Query<(&crate::app_state::MenuStockStageButton, &Node)>,
    mut previews: Query<(&ComputedNode, &mut ImageNode), (With<MapImage>, Without<StageThumbnail>)>,
    mut thumbnails: Query<(&StageThumbnail, &ComputedNode, &mut ImageNode), Without<MapImage>>,
    mut hints: Query<&mut Text, With<MapPreviewHint>>,
    mut assets: ResMut<Assets<Image>>,
    library: Option<Res<PreviewLibrary>>,
    mut cached_handles: Local<HashMap<String, Option<(Handle<Image>, Vec2)>>>,
) {
    let mut resolve = |id: &str, assets: &mut Assets<Image>| -> Option<(Handle<Image>, Vec2)> {
        if id.is_empty() {
            return None;
        }
        if let Some(entry) = cached_handles.get(id) {
            return entry.clone();
        }
        let resolved = load_provided_preview(id)
            .or_else(|| {
                library
                    .as_ref()
                    .and_then(|lib| lib.0.get(id))
                    .and_then(load_bake)
            })
            .map(|image| {
                let size = image.size().as_vec2();
                (assets.add(image), size)
            });
        cached_handles.insert(id.to_owned(), resolved.clone());
        resolved
    };

    let selected_row = stage_rows
        .iter()
        .find(|(stage, node)| stage.id == request.stage_id && node.display != Display::None)
        .map(|(stage, _)| stage.id.as_str());
    let active_id = selected_row.unwrap_or(request.stage_id.as_str());
    let resolved = resolve(active_id, &mut assets);

    for (computed, mut image) in &mut previews {
        if computed.size().x <= 0.0 || computed.size().y <= 0.0 {
            continue;
        }
        if let Some((ref handle, tex_size)) = resolved {
            let rect = Some(super::ui_theme::cover_rect(tex_size, computed.size()));
            if image.image != *handle {
                image.image = handle.clone();
            }
            if image.rect != rect {
                image.rect = rect;
            }
            image.color = Color::WHITE;
        } else {
            image.image = Handle::default();
            image.color = Color::NONE;
        }
    }

    let message = if resolved.is_some() {
        ""
    } else if selected_row.is_none() && !stage_rows.is_empty() {
        "当前规则无可用地图"
    } else {
        "暂无预览图"
    };
    for mut hint in &mut hints {
        if hint.0 != message {
            message.clone_into(&mut hint.0);
        }
    }

    for (thumb, computed, mut image) in &mut thumbnails {
        if computed.size().x <= 0.0 || computed.size().y <= 0.0 {
            continue;
        }
        if let Some((handle, tex_size)) = resolve(&thumb.0, &mut assets) {
            let rect = Some(super::ui_theme::cover_rect(tex_size, computed.size()));
            if image.image != handle {
                image.image = handle;
            }
            if image.rect != rect {
                image.rect = rect;
            }
            image.color = Color::srgb(0.86, 0.84, 0.80);
        }
    }
}

pub(super) const GENERATED_PREVIEW: &str = "preview.generated.jpg";
fn load_provided_preview(stage: &str) -> Option<Image> {
    if !super::stage_presentation::is_stage_id(stage) {
        return None;
    }
    [
        ("preview.png", "png"),
        ("preview.jpg", "jpg"),
        ("preview.jpeg", "jpg"),
        (GENERATED_PREVIEW, "jpg"),
    ]
    .into_iter()
    .find_map(|(name, extension)| {
        let bytes = crate::resource_io::read(format!("{stage}/{name}")).ok()?;
        decode_preview(&bytes, extension)
    })
}
fn load_bake(path: &std::path::PathBuf) -> Option<Image> {
    decode_preview(&std::fs::read(path).ok()?, "jpg")
}
fn decode_preview(bytes: &[u8], extension: &str) -> Option<Image> {
    Image::from_buffer(
        bytes,
        ImageType::Extension(extension),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::linear(),
        RenderAssetUsages::default(),
    )
    .ok()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_stage_preview_wins_and_bad_images_fall_back_to_generated() {
        let png = include_bytes!("../../assets/ui/campaign-halo.png");
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(&[42, 54, 65], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        let mut data = Vec::new();
        let entries: Vec<_> = [
            ("stage/sn96.preview.png", png.as_slice()),
            ("stage/sn96.preview.generated.jpg", jpeg.as_slice()),
            ("stage/sn95.preview.png", b"broken".as_slice()),
            ("stage/sn95.preview.generated.jpg", jpeg.as_slice()),
        ]
        .into_iter()
        .map(|(name, bytes)| {
            let entry = serde_json::json!({"name": name, "off": data.len(), "len": bytes.len()});
            data.extend_from_slice(bytes);
            entry
        })
        .collect();
        let manifest = serde_json::to_vec(&serde_json::json!({
            "formatVersion": 1, "kind": "test", "packId": "test/stage-previews", "entries": entries,
        }))
        .unwrap();
        let mut pack = b"MPK1".to_vec();
        pack.extend((manifest.len() as u32).to_le_bytes());
        pack.extend(manifest);
        pack.extend(data);
        meteor_asset::runtime_assets::register_mpk1(&pack).unwrap();
        let authored = load_provided_preview("sn96").unwrap();
        assert_eq!(authored.size(), decode_preview(png, "png").unwrap().size());
        let generated = load_provided_preview("sn95").unwrap();
        assert_eq!(generated.size(), UVec2::ONE);
    }
}
