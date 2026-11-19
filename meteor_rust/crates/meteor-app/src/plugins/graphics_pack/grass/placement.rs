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
            let Some(target) = grass
                .surfaces
                .iter()
                .position(|target| target.matches(&object.name, texture))
            else {
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

fn ground(point: Vec2, [a, b, c]: [Vec3; 3]) -> Option<(f32, Vec2)> {
    let u = b.xz() - a.xz();
    let v = c.xz() - a.xz();
    let det = u.perp_dot(v);
    if det.abs() < 0.0001 {
        return None;
    }
    let rel = point - a.xz();
    let s = rel.perp_dot(v) / det;
    let t = u.perp_dot(rel) / det;
    (s >= 0.0 && t >= 0.0 && s + t <= 1.0).then(|| {
        let normal = (b - a).cross(c - a);
        (
            a.y + s * (b.y - a.y) + t * (c.y - a.y),
            -normal.xz() / normal.y,
        )
    })
}

fn vegetation(grass: &Grass, point: Vec2, seed: u32) -> u32 {
    let patch = point / grass.patch_size;
    let phase = patch - patch.floor();
    let border = phase * phase * (Vec2::splat(3.0) - phase * 2.0);
    let pick = Vec2::new(unit(seed ^ 31), unit(seed ^ 117));
    let influences = IVec2::new(i32::from(pick.x < border.x), i32::from(pick.y < border.y));
    let cell = patch.floor().as_ivec2() + influences;
    let patch_seed = (cell.x as u32).wrapping_mul(0x9e3779b9) ^ (cell.y as u32).rotate_left(13);
    let species = grass.species;
    let roll = if unit(seed ^ 593) < 0.12 {
        unit(seed ^ 721)
    } else {
        unit(patch_seed)
    } * (species.grass + species.plumes + species.broadleaf);
    if roll < species.grass {
        0
    } else if roll < species.grass + species.plumes {
        1
    } else {
        2
    }
}

fn root_style(species: u32, slope: Vec2, seed: u32) -> f32 {
    let quantize = |v: f32| ((v.clamp(-1.52, 1.52) / 1.52 * 0.5 + 0.5) * 127.0).round() as u32;
    let random = (hash(seed) >> 24) & 255;
    (1 + (random | (species << 8) | (quantize(slope.x) << 10) | (quantize(slope.y) << 17))) as f32
}

fn density_noise(point: Vec2) -> f32 {
    let cell = point.floor();
    let frac = point - cell;
    let weight = frac * frac * (Vec2::splat(3.0) - frac * 2.0);
    let cell = cell.as_ivec2();
    let sample = |dx, dz| {
        let x = (cell.x + dx) as u32;
        let z = (cell.y + dz) as u32;
        unit(x.wrapping_mul(0x9e3779b9) ^ z.rotate_left(17))
    };
    let row0 = sample(0, 0) + (sample(1, 0) - sample(0, 0)) * weight.x;
    let row1 = sample(0, 1) + (sample(1, 1) - sample(0, 1)) * weight.x;
    row0 + (row1 - row0) * weight.y
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

    let candidates = if grass.patch_variation > 0.0 {
        target * 2
    } else {
        target
    };
    let side = (candidates as f32).sqrt().ceil() as u32;
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
            if grass.patch_variation > 0.0 {
                let patch = point / (grass.patch_size * 0.46);
                let growth = 0.73 * density_noise(patch)
                    + 0.27 * density_noise(patch * 2.37 + Vec2::new(4.0, -2.0));
                let coverage = (0.12 + growth * 0.92).clamp(0.16, 0.94);
                let acceptance = 0.53 + (coverage - 0.53) * grass.patch_variation;
                if unit(key ^ 617) > acceptance {
                    continue;
                }
            }
            if bounds.is_some_and(|[lo, hi]| !point.cmpge(lo).all() || !point.cmplt(hi).all()) {
                continue;
            }
            let Some((height, slope)) = faces
                .iter()
                .filter_map(|&i| ground(point, triangles[i]))
                .max_by(|a, b| a.0.total_cmp(&b.0))
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
            roots.push(position.extend(root_style(vegetation(grass, point, key), slope, key ^ 71)));
        }
        roots.sort_unstable_by_key(|root| (root.w as u32 - 1) & 255);
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
