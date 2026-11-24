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
        let path = library.as_ref().and_then(|lib| lib.0.get(id))?;
        let baked = load_bake(path)?;
        let size = baked.size().as_vec2();
        let handle = assets.add(baked);
        cached_handles.insert(id.to_owned(), Some((handle.clone(), size)));
        Some((handle, size))
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
        "场景实景"
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

fn load_bake(path: &std::path::Path) -> Option<Image> {
    let bytes = std::fs::read(path).ok()?;
    Image::from_buffer(
        &bytes,
        ImageType::Extension("jpg"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::linear(),
        RenderAssetUsages::default(),
    )
    .ok()
}
