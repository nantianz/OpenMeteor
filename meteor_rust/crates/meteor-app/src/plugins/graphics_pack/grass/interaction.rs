use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    Extent3d, TexelCopyBufferLayout, TextureDimension, TextureFormat,
};
use bevy::render::renderer::RenderQueue;
use bevy::render::texture::GpuImage;
use bevy::render::{Render, RenderApp, RenderSystems};
use std::sync::Arc;

const SIZE: i32 = 64;
pub(super) const SPACING: f32 = 0.6;
pub(super) const RANGE: f32 = 17.0;
const EMPTY: [u8; 4] = [128, 128, 0, 0];

#[derive(Clone, Copy, Default)]
struct Cell {
    world: IVec2,
    bend: Vec2,
    pressure: f32,
    height: f32,
}

pub(super) struct BendMap {
    cells: Vec<Cell>,
    active: bool,
}

impl Default for BendMap {
    fn default() -> Self {
        Self {
            cells: vec![Cell::default(); (SIZE * SIZE) as usize],
            active: false,
        }
    }
}

impl BendMap {
    pub fn image() -> Image {
        let mut image = Image::new(
            extent(),
            TextureDimension::D2,
            EMPTY.repeat((SIZE * SIZE) as usize),
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        );
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            ..default()
        });
        image
    }

    pub fn decay(&mut self, eye: Vec2, factor: f32) -> bool {
        if !self.active {
            return false;
        }
        let center = (eye / SPACING).floor().as_ivec2();
        self.active = false;
        for cell in &mut self.cells {
            if cell.pressure == 0.0 {
                continue;
            }
            cell.pressure *= factor;
            cell.bend *= factor;
            if (cell.world - center).abs().max_element() >= SIZE / 2 || cell.pressure < 1.0 / 255.0
            {
                *cell = Cell::default();
            } else {
                self.active = true;
            }
        }
        true
    }

    pub fn stamp(&mut self, from: Vec3, to: Vec3, eye: Vec2, radius: f32) -> bool {
        self.stamp_scaled(from, to, eye, radius, 0.68)
    }

    pub(super) fn stamp_scaled(
        &mut self,
        from: Vec3,
        to: Vec3,
        eye: Vec2,
        radius: f32,
        strength: f32,
    ) -> bool {
        let a = from.xz();
        let b = to.xz();
        let path = b - a;
        let path_len = path.length_squared().max(0.0001);

        let path_direction = path / (path.length() + SPACING * 0.5);
        let center = (eye / SPACING).floor().as_ivec2();
        let lo = ((a.min(b) - Vec2::splat(radius)) / SPACING)
            .floor()
            .as_ivec2()
            .max(center - IVec2::splat(SIZE / 2 - 1));
        let hi = ((a.max(b) + Vec2::splat(radius)) / SPACING)
            .ceil()
            .as_ivec2()
            .min(center + IVec2::splat(SIZE / 2));
        let mut stamped = false;
        for y in lo.y..hi.y {
            for x in lo.x..hi.x {
                let world = IVec2::new(x, y);
                let point = (world.as_vec2() + Vec2::splat(0.5)) * SPACING;
                let t = ((point - a).dot(path) / path_len).clamp(0.0, 1.0);
                let delta = point - (a + path * t);
                let influence = (1.0 - delta.length_squared() / (radius * radius)).max(0.0);
                let pressure = influence * strength;
                let index = (y.rem_euclid(SIZE) * SIZE + x.rem_euclid(SIZE)) as usize;
                let cell = &mut self.cells[index];
                if pressure <= 1.0 / 255.0 || (cell.world == world && pressure < cell.pressure) {
                    continue;
                }

                let radial = delta / (delta.length_squared() + (SPACING * 0.35).powi(2)).sqrt();
                let direction = radial + 0.45 * path_direction;
                let direction = direction / direction.length().max(1.0);
                *cell = Cell {
                    world,
                    bend: pressure * direction,
                    pressure,
                    height: from.y + (to.y - from.y) * t,
                };
                stamped = true;
            }
        }
        self.active |= stamped;
        stamped
    }

    pub fn pixels(&self, height_min: f32, height_scale: f32) -> Vec<u8> {
        self.cells
            .iter()
            .flat_map(|cell| {
                if cell.pressure == 0.0 {
                    return EMPTY;
                }
                let unorm = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
                [
                    (128.0 + cell.bend.x * 127.0).round() as u8,
                    (128.0 + cell.bend.y * 127.0).round() as u8,
                    unorm(cell.pressure),
                    unorm(
                        ((cell.height - height_min) / height_scale).clamp(0.0, 1.0) * cell.pressure,
                    ),
                ]
            })
            .collect()
    }
}

#[derive(Resource, Clone, Default, ExtractResource)]
pub(super) struct BendUpload {
    pub revision: u64,
    pub data: Option<(Handle<Image>, Arc<Vec<u8>>)>,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<BendUpload>()
        .add_plugins(ExtractResourcePlugin::<BendUpload>::default());
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
    }
}

fn extent() -> Extent3d {
    Extent3d {
        width: SIZE as u32,
        height: SIZE as u32,
        depth_or_array_layers: 1,
    }
}

fn upload(
    update: Res<BendUpload>,
    images: Res<RenderAssets<GpuImage>>,
    queue: Res<RenderQueue>,
    mut revision: Local<u64>,
) {
    if *revision == update.revision {
        return;
    }
    let Some((handle, pixels)) = &update.data else {
        return;
    };
    let Some(image) = images.get(handle) else {
        return;
    };
    queue.write_texture(
        image.texture.as_image_copy(),
        pixels,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(SIZE as u32 * 4),
            rows_per_image: Some(SIZE as u32),
        },
        extent(),
    );
    *revision = update.revision;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swept_contact_is_local_decays_and_is_cleared_when_the_camera_jumps() {
        let mut map = BendMap::default();
        let eye = Vec2::ZERO;
        assert!(map.stamp(Vec3::new(-2., 3., 0.), Vec3::new(2., 3., 0.), eye, 0.9));
        let pixels = map.pixels(0., 10.);
        let center = &pixels[..4];
        assert!(center[2] > 96 && (f32::from(center[3]) / f32::from(center[2]) - 0.3).abs() < 0.01);
        assert_eq!(&pixels[(20 * SIZE + 20) as usize * 4..][..4], &EMPTY);
        map.decay(eye, 0.5);
        assert!(map.pixels(0., 10.)[2] < center[2]);
        map.decay(Vec2::splat(90.), 1.0);
        assert!(map.pixels(0., 10.).chunks_exact(4).all(|p| p == EMPTY));
    }

    #[test]
    fn idle_contact_jitter_does_not_flip_the_bend_direction() {
        let mut map = BendMap::default();
        let center = Vec3::new(0.3, 3.0, 0.3);
        map.stamp(center, center, Vec2::ZERO, 1.6);
        let mut previous = center;
        let mut before = map.pixels(0.0, 10.0);
        for step in 0..20 {
            let position = center + Vec3::X * if step % 2 == 0 { 0.001 } else { -0.001 };
            map.decay(Vec2::ZERO, 0.9);
            map.stamp(previous, position, Vec2::ZERO, 1.6);
            let after = map.pixels(0.0, 10.0);
            let max_change = before
                .chunks_exact(4)
                .zip(after.chunks_exact(4))
                .map(|(a, b)| a[0].abs_diff(b[0]).max(a[1].abs_diff(b[1])))
                .max()
                .unwrap();
            assert!(
                max_change <= 3,
                "idle contact changed a bend channel by {max_change}"
            );
            before = after;
            previous = position;
        }
    }
}
