use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{Extent3d, TexelCopyBufferLayout};
use bevy::render::renderer::RenderQueue;
use bevy::render::texture::GpuImage;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct Upload {
    pub image: Handle<Image>,
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<Vec<u8>>,
}

#[derive(Resource, Default, Clone)]
pub(super) struct WaterUploads {
    pub epoch: u64,
    pub textures: Vec<Upload>,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<WaterUploads>();
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<WaterUploads>()
            .add_systems(ExtractSchedule, extract)
            .add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
    }
}

fn extract(source: Extract<Res<WaterUploads>>, mut target: ResMut<WaterUploads>) {
    target.clone_from(&source);
}

fn upload(
    batch: Res<WaterUploads>,
    images: Res<RenderAssets<GpuImage>>,
    queue: Res<RenderQueue>,
    mut previous: Local<u64>,
) {
    if *previous == batch.epoch {
        return;
    }
    let mut ready = true;
    for update in &batch.textures {
        let Some(image) = images.get(&update.image) else {
            ready = false;
            continue;
        };
        queue.write_texture(
            image.texture.as_image_copy(),
            &update.pixels,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(update.width * 4),
                rows_per_image: Some(update.height),
            },
            Extent3d {
                width: update.width,
                height: update.height,
                depth_or_array_layers: 1,
            },
        );
    }
    if ready {
        *previous = batch.epoch;
    }
}
