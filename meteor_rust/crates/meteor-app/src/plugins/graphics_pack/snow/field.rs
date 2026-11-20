use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{Origin3d, TexelCopyBufferLayout, TextureFormat};
use bevy::render::renderer::RenderQueue;
use bevy::render::texture::GpuImage;
use bevy::render::{Render, RenderApp, RenderSystems};

use super::terrain::{extent, field_image, Coverage, Layout, CONTACT_SIDE, CONTACT_STEP};
use super::{smoothstep, Contact, Imprint};

const MAX_PATCHES: usize = 64;
pub(super) const MAX_TILE_WRITES: usize = 12;
const PIXELS: usize = (CONTACT_SIDE * CONTACT_SIDE) as usize;
const PATCH_BYTES: usize = 2 * PIXELS;
const FADE_INTERVAL: f32 = 0.2;
const FADE_START: f32 = 0.38;

#[derive(Clone, Copy, Default)]
struct Cell {
    raw: [u8; 2],
    born: f32,
}

impl Cell {
    fn value(self, clock: f32, life: f32) -> [u8; 2] {
        let age = (clock - self.born) / life;
        let fade = 1.0 - smoothstep(FADE_START, 1.0, age);
        self.raw.map(|v| (f32::from(v) * fade).round() as u8)
    }
}

struct Patch {
    cells: Box<[Cell]>,
    touched: f32,
    refresh_at: f32,
}

impl Patch {
    fn new(clock: f32) -> Self {
        Self {
            cells: vec![Cell::default(); PIXELS].into_boxed_slice(),
            touched: clock,
            refresh_at: f32::INFINITY,
        }
    }

    fn encode(&mut self, clock: f32, life: f32) -> Option<Vec<u8>> {
        let mut pixels = vec![0; PATCH_BYTES];
        self.refresh_at = f32::INFINITY;
        for (cell, output) in self.cells.iter_mut().zip(pixels.chunks_exact_mut(2)) {
            if cell.raw == [0, 0] {
                continue;
            }
            let value = cell.value(clock, life);
            if value == [0, 0] {
                cell.raw = [0, 0];
                continue;
            }
            let fade_start = cell.born + life * FADE_START;
            let next = if fade_start <= clock {
                clock + FADE_INTERVAL
            } else {
                fade_start
            };
            self.refresh_at = self.refresh_at.min(next);
            output.copy_from_slice(&value);
        }
        self.refresh_at.is_finite().then_some(pixels)
    }
}

pub(super) struct Field {
    pub image: Handle<Image>,
    patches: HashMap<UVec2, Patch>,
    pending: VecDeque<UVec2>,
    life: f32,
    strength: f32,
}

impl Field {
    pub fn new(layout: Layout, images: &mut Assets<Image>, life: f32, strength: f32) -> Self {
        let size = layout.contact_size();
        Self {
            image: images.add(field_image(
                size,
                vec![0; (size.x * size.y * 2) as usize],
                TextureFormat::Rg8Unorm,
            )),
            patches: HashMap::new(),
            pending: VecDeque::new(),
            life,
            strength,
        }
    }

    pub fn disturbed(&self, tile: UVec2) -> bool {
        self.patches.contains_key(&tile)
    }

    fn queue(&mut self, tile: UVec2) {
        if !self.pending.contains(&tile) {
            self.pending.push_back(tile);
        }
    }

    fn insert(&mut self, tile: UVec2, clock: f32) {
        if !self.patches.contains_key(&tile) && self.patches.len() == MAX_PATCHES {
            let old = self
                .patches
                .iter()
                .min_by(|(a, p), (b, q)| {
                    p.touched
                        .total_cmp(&q.touched)
                        .then_with(|| a.to_array().cmp(&b.to_array()))
                })
                .map(|(&key, _)| key)
                .unwrap();
            self.patches.remove(&old);
            self.queue(old);
        }
        self.patches
            .entry(tile)
            .or_insert_with(|| Patch::new(clock))
            .touched = clock;
        self.queue(tile);
    }

    pub fn stamp(&mut self, coverage: &Coverage, contact: Contact, clock: f32) -> bool {
        let layout = coverage.layout;
        let sweep = contact.imprint == Imprint::Slide;
        let start = contact.from.xz();
        let end = contact.position.xz();
        if !sweep && coverage.at_world(end) == 0 {
            return false;
        }
        let extent = contact.imprint.extent();
        let radius = extent.max_element() * 1.42;
        let direction = contact.direction;
        let lateral = Vec2::new(direction.y, -direction.x);
        let path = end - start;
        let path2 = path.length_squared().max(0.00001);
        let scale = (end - layout.origin) / CONTACT_STEP;
        let lo = ((start.min(end) - layout.origin - radius) / CONTACT_STEP)
            .floor()
            .as_uvec2();
        let hi = ((start.max(end) - layout.origin + radius) / CONTACT_STEP)
            .ceil()
            .as_uvec2()
            .min(layout.contact_size());
        if lo.cmpge(hi).any() {
            return false;
        }
        let first_tile = lo / CONTACT_SIDE;
        let last_tile = (hi - UVec2::ONE) / CONTACT_SIDE;
        let mut stamped = false;
        for tz in first_tile.y..=last_tile.y {
            for tx in first_tile.x..=last_tile.x {
                let tile = UVec2::new(tx, tz);
                let offset = tile * CONTACT_SIDE;
                let lo = lo.max(offset);
                let hi = hi.min(offset + CONTACT_SIDE);
                let mut queued = false;
                for z in lo.y..hi.y {
                    for x in lo.x..hi.x {
                        let coords = UVec2::new(x, z);
                        if coverage.at_contact_texel(coords) == 0 {
                            continue;
                        }
                        let texel = coords.as_vec2() + Vec2::splat(0.5);
                        let relative = (texel - scale) * CONTACT_STEP;
                        let delta = if sweep {
                            let t = ((relative + path).dot(path) / path2).clamp(0.0, 1.0);
                            relative + (1.0 - t) * path
                        } else {
                            relative
                        };
                        let u = delta.dot(lateral) / extent.x;
                        let v = delta.dot(direction) / extent.y;
                        let radius = contact
                            .imprint
                            .shape(Vec2::new(if contact.left { -u } else { u }, v));
                        let base_pressure = match contact.imprint {
                            Imprint::Sole => 0.88,
                            Imprint::Landing => 1.0,
                            Imprint::Slide => 0.78,
                        };
                        let packed = base_pressure * (1.0 - smoothstep(0.44, 1.05, radius));
                        let lip =
                            smoothstep(0.76, 1.01, radius) * (1.0 - smoothstep(1.01, 1.34, radius));
                        let value = [packed, lip * base_pressure]
                            .map(|v| (v * self.strength * 255.0).round() as u8);
                        if value == [0, 0] {
                            continue;
                        }
                        if !queued {
                            self.insert(tile, clock);
                            queued = true;
                        }
                        let i = ((z - offset.y) * CONTACT_SIDE + x - offset.x) as usize;
                        let cell = &mut self.patches.get_mut(&tile).unwrap().cells[i];
                        let prior = cell.value(clock, self.life);
                        let loose = u16::from(prior[1]) * u16::from(255 - value[0]) / 255;
                        *cell = Cell {
                            raw: [prior[0].max(value[0]), (loose as u8).max(value[1])],
                            born: clock,
                        };
                        stamped = true;
                    }
                }
            }
        }
        stamped
    }

    pub fn flush(&mut self, clock: f32, upload: &mut SnowUpload) -> usize {
        let due: Vec<_> = self
            .patches
            .iter()
            .filter_map(|(&key, patch)| (patch.refresh_at <= clock).then_some(key))
            .collect();
        for tile in due {
            self.queue(tile);
        }
        if upload.image.as_ref() != Some(&self.image) {
            *upload = SnowUpload {
                image: Some(self.image.clone()),
                ..Default::default()
            };
        }
        let count = self.pending.len().min(MAX_TILE_WRITES);
        if count == 0 {
            return 0;
        }
        upload.revision = upload.revision.wrapping_add(1);
        let writes = Arc::make_mut(&mut upload.writes);
        for _ in 0..count {
            let tile = self.pending.pop_front().unwrap();
            let data = self
                .patches
                .get_mut(&tile)
                .and_then(|patch| patch.encode(clock, self.life));
            if data.is_none() {
                self.patches.remove(&tile);
            }
            writes.insert(
                tile,
                Write {
                    revision: upload.revision,
                    data: data.map(Arc::from),
                },
            );
        }
        count
    }

    #[cfg(test)]
    pub fn resident_count(&self) -> usize {
        self.patches.len()
    }
}

#[derive(Clone)]
struct Write {
    revision: u64,
    data: Option<Arc<[u8]>>,
}

#[derive(Resource, Clone, Default, ExtractResource)]
pub(super) struct SnowUpload {
    image: Option<Handle<Image>>,
    revision: u64,
    writes: Arc<HashMap<UVec2, Write>>,
}

impl SnowUpload {
    pub fn is_bound(&self) -> bool {
        self.image.is_some()
    }
}

#[derive(Default)]
struct Uploaded {
    image: Option<AssetId<Image>>,
    revision: u64,
    tiles: HashMap<UVec2, u64>,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<SnowUpload>()
        .add_plugins(ExtractResourcePlugin::<SnowUpload>::default());
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
    }
}

fn upload(
    data: Res<SnowUpload>,
    images: Res<RenderAssets<GpuImage>>,
    queue: Res<RenderQueue>,
    mut uploaded: Local<Uploaded>,
) {
    let id = data.image.as_ref().map(Handle::id);
    if uploaded.image != id {
        *uploaded = Uploaded {
            image: id,
            ..Default::default()
        };
    }
    if uploaded.revision == data.revision {
        return;
    }
    let Some(image) = id.and_then(|id| images.get(id)) else {
        return;
    };
    let zero = [0; PATCH_BYTES];
    for (&tile, write) in data.writes.iter() {
        if uploaded.tiles.get(&tile) == Some(&write.revision) {
            continue;
        }
        let start = tile * CONTACT_SIDE;
        let mut target = image.texture.as_image_copy();
        target.origin = Origin3d {
            x: start.x,
            y: start.y,
            z: 0,
        };
        queue.write_texture(
            target,
            write.data.as_deref().unwrap_or(&zero),
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(CONTACT_SIDE * 2),
                rows_per_image: Some(CONTACT_SIDE),
            },
            extent(UVec2::splat(CONTACT_SIDE)),
        );
        uploaded.tiles.insert(tile, write.revision);
    }
    uploaded.revision = data.revision;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::graphics_pack::config::Snow;

    fn rectangle(width: f32, height: f32) -> Coverage {
        let a = Vec3::ZERO;
        let b = Vec3::X * width;
        let c = Vec3::Z * height;
        Coverage::build(&[[a, c, b], [b, c, b + c]], &Snow::default()).unwrap()
    }

    fn pressure(field: &Field, layout: Layout, position: Vec2) -> [u8; 2] {
        let texel = ((position - layout.origin) / CONTACT_STEP)
            .floor()
            .as_uvec2();
        let cell = texel % CONTACT_SIDE;
        field
            .patches
            .get(&(texel / CONTACT_SIDE))
            .map_or([0, 0], |patch| {
                patch.cells[(cell.y * CONTACT_SIDE + cell.x) as usize].raw
            })
    }

    #[test]
    fn snow_roll_is_continuous_across_tiles_with_a_raised_rim() {
        let cover = rectangle(24.0, 18.0);
        let mut field = Field::new(cover.layout, &mut Assets::default(), 4.0, 1.0);
        let contact = Contact {
            from: Vec3::new(6.45, 0.0, 6.50),
            position: Vec3::new(9.65, 0.0, 6.50),
            imprint: Imprint::Slide,
            direction: Vec2::X,
            left: false,
            powder: 0,
        };
        assert!(field.stamp(&cover, contact, 0.0));
        for cell in 0..24 {
            let x = 6.50 + cell as f32 * CONTACT_STEP;
            let [dent, _] = pressure(&field, cover.layout, Vec2::new(x, 6.50));
            assert!(dent > 175, "the trough remains deep at {x}");
        }
        let [_, lip] = pressure(&field, cover.layout, Vec2::new(8.50, 6.91));
        assert!(lip > 110, "displaced snow collects at the trough edge");
        assert_eq!(
            pressure(&field, cover.layout, Vec2::new(8.50, 7.30)),
            [0, 0]
        );

        let mut upload = SnowUpload::default();
        assert_eq!(field.flush(0.0, &mut upload), 2);
        assert_eq!(
            field.flush(0.5, &mut upload),
            0,
            "fresh idle patches don't upload"
        );
        assert_eq!(field.flush(4.1, &mut upload), 2);
        assert!(field.patches.is_empty());
        assert!(upload.writes.values().all(|write| write.data.is_none()));
    }

    #[test]
    fn snow_sparse_state_and_dirty_transfers_remain_bounded() {
        let cover = rectangle(104.0, 88.0);
        let mut field = Field::new(cover.layout, &mut Assets::default(), 36.0, 1.0);
        for i in 0..80 {
            let position = Vec3::new(
                3.0 + (i % 10) as f32 * 8.0,
                0.0,
                3.0 + (i / 10) as f32 * 8.0,
            );
            assert!(field.stamp(
                &cover,
                Contact {
                    from: position,
                    position,
                    direction: Vec2::X,
                    imprint: Imprint::Landing,
                    left: false,
                    powder: 0,
                },
                i as f32 * 0.01
            ));
        }
        assert_eq!(field.resident_count(), MAX_PATCHES);
        let mut upload = SnowUpload::default();
        assert_eq!(field.flush(1.0, &mut upload), MAX_TILE_WRITES);
        for _ in 0..8 {
            assert!(field.flush(1.0, &mut upload) <= MAX_TILE_WRITES);
        }
        assert_eq!(
            upload
                .writes
                .values()
                .filter(|write| write.data.is_some())
                .count(),
            MAX_PATCHES
        );
    }
}
