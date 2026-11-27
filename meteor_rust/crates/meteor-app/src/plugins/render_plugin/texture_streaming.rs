use super::{decode_stage_texture, PlayingSceneEntity};
use crate::app_state::Settings;
use crate::plugins::{
    camera_plugin::GameplayCamera,
    pak_textures::{
        apply_texture_quality, prepare_quality_image, sync_texture_samplers, texture_device_side,
        PakCache,
    },
    stage_material::GmdlMaterial,
};
use bevy::{
    asset::AssetId,
    camera::primitives::{Aabb, Frustum},
    prelude::*,
    tasks::{block_on, futures_lite::future::poll_once, AsyncComputeTaskPool, Task},
};
use meteor_asset::texture::DecodedImage;
use std::{collections::HashMap, path::PathBuf};

const FALLBACK_SIDE: u32 = 1024;
const MIN_STREAM_BYTES: usize = 16 * 1024 * 1024;
const DETAIL_BUDGET: usize = 384 * 1024 * 1024;
const SCAN_INTERVAL: f64 = 0.2;
const RETAIN_SECONDS: f64 = 2.0;
const PROMOTE_PIXELS: f32 = 512.0;
const KEEP_PIXELS: f32 = 384.0;

#[derive(Clone)]
struct SourceTexture {
    name: String,
    color_key: bool,
    alpha_channel: bool,
}

struct Detail {
    side: u32,
    handle: Handle<Image>,
}

struct Entry {
    source: SourceTexture,
    low: Handle<Image>,
    high: Option<Detail>,
    full_size: (u32, u32),
    materials: Vec<AssetId<GmdlMaterial>>,
    demand: f32,
    retain_until: f64,
    failed: bool,
}

#[derive(Resource)]
pub(super) struct StageTextureStream {
    root: PathBuf,
    scope: String,
    entries: Vec<Entry>,
    by_image: HashMap<AssetId<Image>, usize>,
    by_material: HashMap<AssetId<GmdlMaterial>, usize>,
    pending: Option<(usize, u32, Task<Option<Image>>)>,
    upload: Option<(usize, Detail)>,
    next_scan: f64,
}

impl StageTextureStream {
    pub(super) fn new(root: PathBuf, scope: String) -> Self {
        Self {
            root,
            scope,
            entries: Vec::new(),
            by_image: HashMap::new(),
            by_material: HashMap::new(),
            pending: None,
            upload: None,
            next_scan: 0.0,
        }
    }

    pub(super) fn prepare(
        &mut self,
        images: &mut Assets<Image>,
        decoded: DecodedImage,
        name: &str,
        color_key: bool,
        alpha_channel: bool,
        quality: u8,
    ) -> Option<Handle<Image>> {
        let device_side = texture_device_side();
        let (target, full_size) = stream_target_side(decoded.width, decoded.height, device_side);
        let handle = images.add(prepare_quality_image(decoded, quality, target)?);
        if let Some(full_size) = full_size {
            self.register(&handle, name, color_key, alpha_channel, full_size);
        }
        Some(handle)
    }

    pub(super) fn register_prepared(
        &mut self,
        low: &Handle<Image>,
        name: &str,
        color_key: bool,
        alpha_channel: bool,
        full_size: (u32, u32),
    ) {
        self.register(low, name, color_key, alpha_channel, full_size);
    }

    pub(super) fn use_cached_mip_tail(
        &mut self,
        images: &mut Assets<Image>,
        handle: &Handle<Image>,
        name: &str,
        color_key: bool,
        alpha_channel: bool,
    ) {
        if self.by_image.contains_key(&handle.id()) {
            return;
        }
        let Some(image) = images.get(handle) else {
            return;
        };
        let full_size = (image.width(), image.height());
        let full_bytes = mip_bytes(full_size.0, full_size.1);
        if full_size.0.max(full_size.1) <= FALLBACK_SIDE || full_bytes < MIN_STREAM_BYTES {
            return;
        }

        let (width, height) = fitted_size(full_size.0, full_size.1, FALLBACK_SIDE);
        let low_bytes = mip_bytes(width, height);
        let Some(full) = image.data.as_ref().filter(|data| data.len() == full_bytes) else {
            return;
        };
        let mut low = Vec::new();
        if low.try_reserve_exact(low_bytes).is_err() {
            return;
        }
        low.extend_from_slice(&full[full_bytes - low_bytes..]);
        if let Some(mut image) = images.get_mut(handle) {
            image.data = Some(low);
            image.texture_descriptor.size.width = width;
            image.texture_descriptor.size.height = height;
            image.texture_descriptor.mip_level_count =
                u32::BITS - width.max(height).leading_zeros();
            self.register(handle, name, color_key, alpha_channel, full_size);
        }
    }

    fn register(
        &mut self,
        low: &Handle<Image>,
        name: &str,
        color_key: bool,
        alpha_channel: bool,
        full_size: (u32, u32),
    ) {
        let index = self.entries.len();
        self.by_image.insert(low.id(), index);
        self.entries.push(Entry {
            source: SourceTexture {
                name: name.into(),
                color_key,
                alpha_channel,
            },
            low: low.clone(),
            high: None,
            full_size,
            materials: Vec::new(),
            demand: 0.0,
            retain_until: 0.0,
            failed: false,
        });
    }

    pub(super) fn bind(&mut self, image: AssetId<Image>, material: &Handle<GmdlMaterial>) {
        if let Some(&index) = self.by_image.get(&image) {
            let material = material.id();
            if self.by_material.insert(material, index).is_none() {
                self.entries[index].materials.push(material);
            }
        }
    }

    pub(super) fn has_materials(&self) -> bool {
        !self.by_material.is_empty()
    }
}

pub(super) fn stream_target_side(
    width: u32,
    height: u32,
    device_side: u32,
) -> (u32, Option<(u32, u32)>) {
    let (w, h) = fitted_size(width, height, device_side);
    let detail_bytes = mip_bytes(w, h);
    if w.max(h) <= FALLBACK_SIDE || detail_bytes < MIN_STREAM_BYTES {
        (device_side, None)
    } else {
        (device_side.min(FALLBACK_SIDE), Some((w, h)))
    }
}

fn fitted_size(mut width: u32, mut height: u32, max_side: u32) -> (u32, u32) {
    while width.max(height) > max_side {
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
    (width, height)
}

fn mip_bytes(width: u32, height: u32) -> usize {
    let count = u32::BITS - width.max(height).leading_zeros();
    (0..count)
        .map(|level| (width >> level).max(1) as usize * (height >> level).max(1) as usize * 4)
        .sum()
}

fn target_side(entry: &Entry) -> u32 {
    if entry.demand < PROMOTE_PIXELS {
        return FALLBACK_SIDE;
    }
    let max_side = entry.full_size.0.max(entry.full_size.1).next_power_of_two();
    ((2.0 * entry.demand) as u32)
        .clamp(FALLBACK_SIDE * 2, max_side)
        .next_power_of_two()
}

fn detail_bytes(entry: &Entry, side: u32) -> usize {
    let (width, height) = fitted_size(entry.full_size.0, entry.full_size.1, side);
    mip_bytes(width, height)
}

fn wanted(entries: &[Entry], now: f64) -> Vec<(usize, u32)> {
    let mut eligible: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            if entry.failed || entry.materials.is_empty() {
                return None;
            }
            let target = target_side(entry);
            let held = entry.high.as_ref().filter(|_| now < entry.retain_until);
            let side = held.map_or(target, |detail| target.max(detail.side));
            if side == FALLBACK_SIDE {
                return None;
            }

            let priority = if held.is_some() {
                entry.demand * 1.25
            } else {
                entry.demand
            };
            Some((index, priority, side))
        })
        .collect();
    eligible.sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));
    let mut budget = DETAIL_BUDGET;
    let mut selected = Vec::new();
    for (index, _, mut side) in eligible {
        while side > FALLBACK_SIDE {
            if let Some(remaining) = budget.checked_sub(detail_bytes(&entries[index], side)) {
                budget = remaining;
                selected.push((index, side));
                break;
            }
            side /= 2;
        }
    }
    selected
}

fn projected_diameter(
    bounds: &Aabb,
    transform: &GlobalTransform,
    frustum: &Frustum,
    view: &GlobalTransform,
    pixels_per_unit: f32,
) -> f32 {
    if !frustum.intersects_obb(bounds, &transform.affine(), true, true) {
        return 0.0;
    }
    let (scale, _, _) = transform.to_scale_rotation_translation();
    let radius = (Vec3::from(bounds.half_extents) * scale.abs()).length();
    let center = transform.transform_point(bounds.center.into());
    let depth = (center - view.translation()).dot(*view.forward());
    2.0 * radius * pixels_per_unit / (depth - radius).max(0.1)
}

fn bind_level(
    entry: &Entry,
    materials: &mut Assets<GmdlMaterial>,
    from: &Handle<Image>,
    to: &Handle<Image>,
) {
    for &id in &entry.materials {
        if materials
            .get(id)
            .is_none_or(|mat| mat.texture.as_ref() != Some(from))
        {
            continue;
        }
        if let Some(mut material) = materials.get_mut(id) {
            material.texture = Some(to.clone());
        }
    }
}

pub(super) fn clear(mut commands: Commands) {
    commands.remove_resource::<StageTextureStream>();
}

pub(super) fn update(
    stream: Option<ResMut<StageTextureStream>>,
    clock: Res<Time<Real>>,
    settings: Res<Settings>,
    camera: Query<(&Camera, &Frustum, &GlobalTransform), With<GameplayCamera>>,
    surfaces: Query<
        (
            &MeshMaterial3d<GmdlMaterial>,
            &Aabb,
            &GlobalTransform,
            &InheritedVisibility,
        ),
        With<PlayingSceneEntity>,
    >,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
) {
    let Some(mut stream) = stream else {
        return;
    };
    let now = clock.elapsed_secs_f64();
    if now < stream.next_scan {
        return;
    }
    let Some((camera, frustum, view)) = camera.iter().find(|(camera, _, _)| camera.is_active)
    else {
        return;
    };
    let Some(viewport) = camera.physical_viewport_size() else {
        return;
    };
    stream.next_scan = now + SCAN_INTERVAL;
    let pixels_per_unit = 0.5 * viewport.y as f32 * camera.clip_from_view().y_axis.y.abs();
    for entry in &mut stream.entries {
        entry.demand = 0.0;
    }
    for (material, bounds, transform, visibility) in &surfaces {
        if !visibility.get() {
            continue;
        }
        if let Some(&index) = stream.by_material.get(&material.0.id()) {
            let demand = projected_diameter(bounds, transform, frustum, view, pixels_per_unit);
            stream.entries[index].demand = stream.entries[index].demand.max(demand);
        }
    }
    for entry in &mut stream.entries {
        if let Some(high) = &entry.high {
            sync_texture_samplers(
                &mut images,
                std::slice::from_ref(&high.handle),
                settings.texture_quality,
            );
            if target_side(entry) >= high.side
                || (high.side == FALLBACK_SIDE * 2 && entry.demand >= KEEP_PIXELS)
            {
                entry.retain_until = now + RETAIN_SECONDS;
            }
        }
    }
    let desired = wanted(&stream.entries, now);
    let desired_side = |index| {
        desired
            .iter()
            .find(|&&(candidate, _)| candidate == index)
            .map_or(FALLBACK_SIDE, |&(_, side)| side)
    };
    for (index, entry) in stream.entries.iter_mut().enumerate() {
        if entry
            .high
            .as_ref()
            .is_some_and(|high| high.side > desired_side(index))
        {
            let high = entry.high.take().unwrap();
            bind_level(entry, &mut materials, &high.handle, &entry.low);
            debug!("released scene texture detail for {}", entry.source.name);
        }
    }

    if let Some((index, detail)) = stream.upload.take() {
        if detail.side <= desired_side(index) {
            let entry = &mut stream.entries[index];
            let previous = entry.high.as_ref().map_or(&entry.low, |high| &high.handle);
            bind_level(entry, &mut materials, previous, &detail.handle);
            debug!(
                "loaded scene texture detail for {} at mip size {}",
                entry.source.name, detail.side
            );
            entry.high = Some(detail);
            entry.retain_until = now + RETAIN_SECONDS;
        }
    }

    let completed = stream.pending.as_mut().and_then(|(index, side, task)| {
        block_on(poll_once(task)).map(|result| (*index, *side, result))
    });
    if let Some((index, side, result)) = completed {
        stream.pending = None;
        let entry = &mut stream.entries[index];
        if let Some(mut image) = result.filter(|image| {
            image
                .data
                .as_ref()
                .is_some_and(|data| data.len() <= detail_bytes(entry, side))
        }) {
            if side <= desired_side(index) {
                apply_texture_quality(&mut image, settings.texture_quality);
                stream.upload = Some((
                    index,
                    Detail {
                        side,
                        handle: images.add(image),
                    },
                ));
            }
        } else {
            entry.failed = true;
            warn!("scene texture detail unavailable: {}", entry.source.name);
        }
    }
    if stream.pending.is_some() || stream.upload.is_some() {
        return;
    }
    if let Some((index, side)) = desired.into_iter().find(|&(index, side)| {
        let entry = &stream.entries[index];
        !entry.failed && entry.high.as_ref().is_none_or(|high| high.side < side)
    }) {
        let root = stream.root.clone();
        let scope = stream.scope.clone();
        let source = stream.entries[index].source.clone();
        let pool = AsyncComputeTaskPool::get();
        let quality = settings.texture_quality;
        stream.pending = Some((
            index,
            side,
            pool.spawn(async move {
                let decoded = decode_stage_texture(
                    &mut PakCache::default(),
                    &root,
                    &scope,
                    &source.name,
                    source.color_key,
                    source.alpha_channel,
                )?;
                prepare_quality_image(decoded, quality, texture_device_side().min(side))
            }),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::stage_material::{GmdlMaterialParams, GmdlMode};
    use bevy::camera::{CameraProjection, PerspectiveProjection};

    fn material(texture: Handle<Image>) -> GmdlMaterial {
        GmdlMaterial::new_scene(
            GmdlMaterialParams {
                opacity: 1.0,
                alpha_cutoff: 0.0,
                mode: GmdlMode::Opaque.as_u32(),
                has_texture: 1,
                uv_offset: Vec2::ZERO,
            },
            Some(texture),
            false,
            GmdlMode::Opaque,
        )
    }

    #[test]
    fn camera_demand_uses_visible_projected_bounds() {
        let view = GlobalTransform::IDENTITY;
        let projection = PerspectiveProjection {
            aspect_ratio: 1.0,
            ..default()
        };
        let frustum = projection.compute_frustum(&view);
        let bounds = Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0));
        let demand = |x, z| {
            projected_diameter(
                &bounds,
                &GlobalTransform::from_translation(Vec3::new(x, 0.0, z)),
                &frustum,
                &view,
                600.0,
            )
        };
        assert!(demand(0.0, -3.0) > PROMOTE_PIXELS);
        assert!(demand(0.0, -100.0) < KEEP_PIXELS);
        assert_eq!(demand(100.0, -5.0), 0.0);
    }

    #[test]
    fn stages_large_texture_and_restores_shared_materials_on_eviction() {
        let mut stream = StageTextureStream::new(PathBuf::new(), "sn01".into());
        let mut images = Assets::<Image>::default();
        let mut materials = Assets::<GmdlMaterial>::default();
        let decoded = DecodedImage {
            width: 2048,
            height: 2048,
            rgba: [14, 26, 45, 191].repeat(2048 * 2048),
        };
        let preloaded = images.add(prepare_quality_image(decoded.clone(), 0, 2048).unwrap());
        stream.use_cached_mip_tail(&mut images, &preloaded, "hollow.tga", false, false);
        let low = stream
            .prepare(&mut images, decoded, "large.tga", false, false, 0)
            .unwrap();
        assert_eq!(
            images.get(&preloaded).unwrap().data,
            images.get(&low).unwrap().data
        );
        let first = materials.add(material(low.clone()));
        let second = materials.add(material(low.clone()));
        stream.bind(low.id(), &first);
        stream.bind(low.id(), &second);
        stream.bind(low.id(), &second);
        let initial = images.get(&low).unwrap();
        assert_eq!(initial.width(), FALLBACK_SIDE);
        assert_eq!(&initial.data.as_ref().unwrap()[..4], &[14, 26, 45, 191]);

        let entry = &mut stream.entries[1];
        assert_eq!(entry.materials.len(), 2);
        assert_eq!(entry.full_size, (2048, 2048));
        entry.demand = 700.0;
        assert_eq!(wanted(&stream.entries, 0.0), [(1, 2048)]);
        let detail = images.add(Image::default());
        {
            let entry = &mut stream.entries[1];
            bind_level(entry, &mut materials, &entry.low, &detail);
            entry.high = Some(Detail {
                side: 2048,
                handle: detail.clone(),
            });
            entry.demand = 0.0;
            entry.retain_until = RETAIN_SECONDS;
        }
        assert_eq!(
            materials.get(&first).unwrap().texture.as_ref(),
            Some(&detail)
        );
        assert_eq!(
            materials.get(&second).unwrap().texture.as_ref(),
            Some(&detail)
        );
        assert_eq!(wanted(&stream.entries, 1.0), [(1, 2048)]);
        assert!(wanted(&stream.entries, RETAIN_SECONDS).is_empty());
        let entry = &mut stream.entries[1];
        bind_level(entry, &mut materials, &detail, &entry.low);
        entry.high = None;
        assert_eq!(materials.get(&first).unwrap().texture.as_ref(), Some(&low));
        assert_eq!(materials.get(&second).unwrap().texture.as_ref(), Some(&low));
    }

    #[test]
    fn residency_budget_prefers_larger_screen_coverage() {
        let entry = |demand| Entry {
            source: SourceTexture {
                name: "large.tga".into(),
                color_key: false,
                alpha_channel: false,
            },
            low: Handle::default(),
            high: None,
            full_size: (8192, 8192),
            materials: vec![AssetId::default()],
            demand,
            retain_until: 0.0,
            failed: false,
        };
        let mut entries = [entry(2200.0), entry(3500.0), entry(200.0)];
        assert_eq!(wanted(&entries, 0.0), [(1, 8192), (0, 2048)]);
        entries[1].demand = 400.0;
        assert_eq!(wanted(&entries, 0.0), [(0, 8192)]);
    }
}
