use super::Grass;
use crate::plugins::render_plugin::classify_gmdl_material;
use crate::plugins::stage_material::GmdlMode;
use bevy::prelude::*;
use meteor_asset::{des::StageDescription, gmdl::GmdlMesh};
use meteor_core::constants::MODEL_SCALE;
use meteor_physics::collision::MeshCollider;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const ROOTS_PER_TILE: usize = 192;
const MAX_TILES: usize = 512;
const MAX_TILE_REFERENCES: usize = 32768;

pub(super) struct Tile {
    pub roots: Vec<Vec4>,
    pub min: Vec3,
    pub max: Vec3,
}

pub(super) fn place(
    grass: &Grass,
    mesh: &GmdlMesh,
    description: Option<&StageDescription>,
    collision_faces: &BTreeSet<u32>,
    support: Option<&dyn MeshCollider>,
    center: [f32; 3],
) -> Result<Vec<Tile>, String> {
    let mut matched = vec![false; grass.surfaces.len()];
    let mut triangles = Vec::new();
    let mut face_base = 0;
    let center = Vec3::from_array(center);
    for object in &mesh.objects {
        let first_face = face_base;
        face_base += object.num_faces as u32;
        if description.is_some_and(|desc| {
            desc.scene_objects
                .iter()
                .any(|meta| meta.name == object.name && (meta.skip_render() || meta.billboard == 1))
        }) {
            continue;
        }
        for (face, (&index, indices)) in object
            .face_mat
            .iter()
            .zip(object.indices.chunks_exact(3))
            .enumerate()
        {
            if !collision_faces.is_empty() && !collision_faces.contains(&(first_face + face as u32))
            {
                continue;
            }
            let Some(material) = mesh.materials.get(index as usize) else {
                continue;
            };
            let Some(texture) = mesh.resolve_texture_name(material) else {
                continue;
            };
            let Some(target) = grass.surfaces.iter().position(|target| {
                target.object.eq_ignore_ascii_case(&object.name)
                    && target.texture.eq_ignore_ascii_case(texture)
            }) else {
                continue;
            };
            let (mode, _, cutoff) = classify_gmdl_material(material);
            if mode != GmdlMode::Opaque || cutoff != 0.0 {
                continue;
            }
            let point = |i: u32| {
                let raw = Vec3::from_slice(&object.positions[i as usize * 3..]) - center;
                Vec3::new(raw.x, raw.z, -raw.y) * MODEL_SCALE
            };
            let [a, b, c] = [point(indices[0]), point(indices[1]), point(indices[2])];
            if !a.is_finite()
                || !b.is_finite()
                || !c.is_finite()
                || (b - a).cross(c - a).normalize_or_zero().y < grass.min_up
            {
                continue;
            }
            matched[target] = true;
            triangles.push([a, b, c]);
            if triangles.len() > 8192 {
                return Err("grass surface exceeds the triangle budget".into());
            }
        }
    }
    if let Some(target) = matched.iter().position(|id| !id) {
        let target = &grass.surfaces[target];
        return Err(format!(
            "grass surface missing or too steep: {} / {}",
            target.object, target.texture
        ));
    }
    distribute(grass, &triangles, support)
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^ (x >> 16)
}

fn unit(seed: u32) -> f32 {
    (hash(seed) >> 8) as f32 / 16_777_216.0
}

fn ground(point: Vec2, [a, b, c]: [Vec3; 3]) -> Option<f32> {
    let u = b.xz() - a.xz();
    let v = c.xz() - a.xz();
    let det = u.perp_dot(v);
    if det.abs() < 0.0001 {
        return None;
    }
    let rel = point - a.xz();
    let s = rel.perp_dot(v) / det;
    let t = u.perp_dot(rel) / det;
    (s >= 0.0 && t >= 0.0 && s + t <= 1.0).then_some(a.y + s * (b.y - a.y) + t * (c.y - a.y))
}

fn distribute(
    grass: &Grass,
    triangles: &[[Vec3; 3]],
    support: Option<&dyn MeshCollider>,
) -> Result<Vec<Tile>, String> {
    let size = grass.tile_size;
    let bounds = grass.bounds.map(|bounds| bounds.map(Vec2::from_array));
    let mut table = BTreeMap::<(i32, i32), Vec<usize>>::new();
    let mut references = 0;
    for (i, &[a, b, c]) in triangles.iter().enumerate() {
        let mut lo = a.min(b).min(c).xz();
        let mut hi = a.max(b).max(c).xz();
        if let Some([min, max]) = bounds {
            lo = lo.max(min);
            hi = hi.min(max);
        }
        if !lo.cmplt(hi).all() {
            continue;
        }
        let first = (lo / size).floor().as_ivec2();
        let last = (hi / size).ceil().as_ivec2();
        let span = last.as_i64vec2() - first.as_i64vec2();
        if span.x > 4096 || span.y > 4096 || span.x * span.y > 4096 {
            return Err(
                "grass surface spans too many tiles; narrow bounds or increase tile_size".into(),
            );
        }
        for y in first.y..last.y {
            for x in first.x..last.x {
                table.entry((x, y)).or_default().push(i);
                references += 1;
                if references > MAX_TILE_REFERENCES || table.len() > 4096 {
                    return Err("grass placement exceeds the tile search budget".into());
                }
            }
        }
    }
    let target = (grass.density * size * size)
        .round()
        .clamp(1.0, ROOTS_PER_TILE as f32) as u32;
    let side = (target as f32).sqrt().ceil() as u32;
    let mut tiles = Vec::new();
    for ((x, y), faces) in table {
        let start = Vec2::new(x as f32, y as f32) * size;
        let seed = hash((x as u32).wrapping_mul(0x5bd1e995) ^ (y as u32).rotate_left(16));
        let mut roots = Vec::new();
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for n in 0..side * side {
            let key = hash(seed.wrapping_add(n));
            let cell = Vec2::new((n % side) as f32, (n / side) as f32);
            let jitter = Vec2::new(unit(key ^ 11), unit(key ^ 37));
            let point = start + (cell + jitter) * (size / side as f32);
            if bounds.is_some_and(|[lo, hi]| !point.cmpge(lo).all() || !point.cmplt(hi).all()) {
                continue;
            }
            let Some(height) = faces
                .iter()
                .filter_map(|&i| ground(point, triangles[i]))
                .reduce(f32::max)
            else {
                continue;
            };
            if support.is_some_and(|collider| {
                collider
                    .sweep_foot_segment(point.x, height + 0.65, point.y, height - 0.05)
                    .is_none_or(|hit| (hit.y - height).abs() > 0.045)
            }) {
                continue;
            }
            let position = Vec3::new(point.x, height + 0.018, point.y);
            min = min.min(position);
            max = max.max(position);
            roots.push(position.extend(0.01 + unit(key ^ 71) * 0.98));
        }
        roots.sort_unstable_by(|a, b| a.w.total_cmp(&b.w));
        roots.truncate(target as usize);
        if !roots.is_empty() {
            tiles.push(Tile { roots, min, max });
            if tiles.len() > MAX_TILES {
                return Err(
                    "grass field exceeds 512 occupied tiles; narrow bounds or increase tile_size"
                        .into(),
                );
            }
        }
    }
    Ok(tiles)
}
