use super::{
    bake_rays::{Rays, Triangle},
    config::{BlendMode, Surface},
};
use bevy::prelude::*;
use meteor_asset::{gmdl::GmdlMesh, texture::DecodedImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const ALGORITHM: &[u8] = b"meteor-water-native-v1-cube-depth128-shore12";
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Map {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
impl Map {
    fn blank(width: usize, height: usize) -> Self {
        Self {
            width: width as u32,
            height: height as u32,
            rgba: vec![0; width * height * 4],
        }
    }
    pub fn valid(&self, width: usize, height: usize) -> bool {
        self.width as usize == width
            && self.height as usize == height
            && self.rgba.len() == width * height * 4
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct BakedSurface {
    pub bounds: [f32; 4],
    pub vertex_alpha: f32,
    pub reflection: Option<Map>,
    pub depth: Option<Map>,
}
pub(super) struct Inputs {
    mesh: GmdlMesh,
    description: meteor_asset::des::StageDescription,
    textures: BTreeMap<usize, DecodedImage>,
    hash: Sha256,
}
impl Inputs {
    pub fn load(stage: &str) -> Result<Self, String> {
        Self::read(
            stage,
            |name| crate::resource_io::read(name).map_err(|e| e.to_string()),
            |name| {
                super::super::super::pak_textures::resolve_runtime_texture_exact(
                    &[stage, "patch", "ctexture", "ptexture"],
                    name,
                )
            },
        )
    }
    fn read(
        stage: &str,
        read: impl Fn(&str) -> Result<Vec<u8>, String>,
        texture: impl Fn(&str) -> Option<Vec<u8>>,
    ) -> Result<Self, String> {
        let gmb = read(&format!("{stage}/{stage}.gmb"))?;
        let des = read(&format!("{stage}/{stage}.des"))?;
        let mesh = meteor_asset::gmdl::decode_gmdl(&gmb).map_err(|e| e.to_string())?;
        let (text, _, _) = encoding_rs::GBK.decode(&des);
        let description = meteor_asset::des::parse_stage_description(&text);
        let mut hash = Sha256::new();
        hash.update(ALGORITHM);
        hash.update(&gmb);
        hash.update(&des);
        let mut textures = BTreeMap::new();
        for (i, mat) in mesh.materials.iter().enumerate() {
            if !mat.state.to_ascii_uppercase().starts_with("DISABLE") || mat.blend != "NORMAL" {
                continue;
            }
            if let Some(name) = mesh.resolve_texture_name(mat) {
                hash.update(name.as_bytes());
                if let Some(bytes) = texture(name) {
                    hash.update((bytes.len() as u64).to_le_bytes());
                    hash.update(&bytes);
                    textures.insert(
                        i,
                        meteor_asset::texture::decode_image(&bytes)
                            .map_err(|e| format!("{name}: {e}"))?,
                    );
                } else {
                    hash.update(b"missing");
                }
            }
        }
        Ok(Self {
            mesh,
            description,
            textures,
            hash,
        })
    }
    pub fn cache_key(&self, s: &Surface) -> String {
        let mut hash = self.hash.clone();

        hash.update(
            serde_json::to_vec(&(
                &s.stage,
                &s.object,
                &s.texture,
                s.kind,
                s.blend_mode,
                s.max_depth,
                s.reflection_face,
            ))
            .unwrap(),
        );
        format!("{:x}", hash.finalize())
    }
    fn triangles(&self, s: &Surface) -> Result<(Rays, Rays, f32), String> {
        let mut water = Vec::new();
        let mut scene = Vec::new();
        let mut alpha = 0u8;
        for object in &self.mesh.objects {
            let Some(des) = self
                .description
                .scene_objects
                .iter()
                .find(|d| d.name == object.name)
            else {
                continue;
            };
            if des.skip_render() || des.billboard != 0 {
                continue;
            }
            for (face, &mat_id) in object.indices.chunks_exact(3).zip(&object.face_mat) {
                let Some(mat) = self.mesh.materials.get(mat_id as usize) else {
                    continue;
                };
                let selected = object.name == s.object
                    && self
                        .mesh
                        .resolve_texture_name(mat)
                        .is_some_and(|t| s.matches(&s.stage, &object.name, t));
                let expected = if s.blend_mode == BlendMode::SrcColorOne {
                    "ENABLE S 1"
                } else {
                    "ENABLE SA 1-SA"
                };
                let is_water = selected
                    && mat.state.eq_ignore_ascii_case(expected)
                    && matches!(mat.blend.as_str(), "NORMAL" | "ALPHACHANNEL")
                    && des.texture_animation.enabled
                    && (des.deform.is_none() || s.kind == 2);
                let is_static = mat.state.to_ascii_uppercase().starts_with("DISABLE")
                    && mat.blend == "NORMAL"
                    && des.deform.is_none()
                    && !des.texture_animation.enabled;
                if !is_water && !is_static {
                    continue;
                }
                if face.iter().any(|&i| i as usize >= object.num_verts) {
                    return Err("GMDL 顶点索引越界".into());
                }
                let triangle = Triangle {
                    points: std::array::from_fn(|i| {
                        Vec3::from_slice(&object.positions[face[i] as usize * 3..][..3])
                    }),
                    uv: std::array::from_fn(|i| {
                        Vec2::from_slice(&object.uvs[face[i] as usize * 2..][..2])
                    }),
                    color: std::array::from_fn(|i| {
                        let c = &object.colors[face[i] as usize * 4..];
                        Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32) / 255.0
                    }),
                    material: mat_id as usize,
                };
                if triangle.points.iter().any(|p| !p.is_finite()) {
                    return Err("水面几何包含非有限坐标".into());
                }
                if is_water {
                    for &i in face {
                        alpha = alpha.max(object.colors[i as usize * 4 + 3]);
                    }
                    water.push(triangle);
                } else {
                    scene.push(triangle);
                }
            }
        }
        if water.is_empty() {
            return Err(format!(
                "{}/{} / {} 未匹配到可增强的滚动液面",
                s.stage, s.object, s.texture
            ));
        }
        Ok((Rays::new(water), Rays::new(scene), alpha as f32 / 255.0))
    }
    pub fn bake(&self, s: &Surface, mut progress: impl FnMut(f32)) -> Result<BakedSurface, String> {
        let (water, scene, vertex_alpha) = self.triangles(s)?;
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for t in &water.triangles {
            for p in t.points {
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        let bounds = [
            lo.x * 0.12,
            -hi.y * 0.12,
            (hi.x - lo.x) * 0.12,
            (hi.y - lo.y) * 0.12,
        ];
        if bounds[2] <= 0.0 || bounds[3] <= 0.0 {
            return Err("液面投影面积为零".into());
        }
        if !s.needs_maps() {
            progress(1.0);
            return Ok(BakedSurface {
                bounds,
                vertex_alpha,
                reflection: None,
                depth: None,
            });
        }
        let candidates: Vec<_> = water
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.points;
                let p = (a + b + c) / 3.0 + Vec3::Z * 2.0;
                let clearance = scene
                    .trace(p, Vec3::Z)
                    .map_or(10000.0, |h| h.distance.min(10000.0));
                (p, clearance, (b - a).cross(c - a).length())
            })
            .collect();
        let clearance = candidates.iter().map(|c| c.1).fold(0.0_f32, f32::max);
        let probe = candidates
            .iter()
            .filter(|c| c.1 >= 100.0_f32.min(clearance * 0.8))
            .max_by(|a, b| a.2.total_cmp(&b.2))
            .unwrap()
            .0;
        let tile = s.reflection_face + 2;
        let mut reflection = Map::blank(tile * 3, tile * 2);
        for face in 0..6 {
            for y in 0..tile {
                for x in 0..tile {
                    let u = ((x as f32 - 0.5) / s.reflection_face as f32) * 2.0 - 1.0;
                    let v = ((y as f32 - 0.5) / s.reflection_face as f32) * 2.0 - 1.0;
                    let world = cube_direction(face, u, v);
                    let direction = Vec3::new(world.x, -world.z, world.y);
                    if let Some(h) = scene.trace(probe, direction) {
                        let t = &scene.triangles[h.index];
                        let uv =
                            t.uv[0] * h.weights.x + t.uv[1] * h.weights.y + t.uv[2] * h.weights.z;
                        let color = t.color[0] * h.weights.x
                            + t.color[1] * h.weights.y
                            + t.color[2] * h.weights.z;
                        let tex = self
                            .textures
                            .get(&t.material)
                            .map_or(Vec3::splat(255.0), |t| sample_texture(t, uv));
                        let pixel = (color * tex).clamp(Vec3::ZERO, Vec3::splat(255.0));
                        let i = ((face / 3 * tile + y) * tile * 3 + face % 3 * tile + x) * 4;
                        reflection.rgba[i..i + 4].copy_from_slice(&[
                            pixel.x as u8,
                            pixel.y as u8,
                            pixel.z as u8,
                            255,
                        ]);
                    }
                }
                if y % 8 == 0 {
                    progress((face as f32 + y as f32 / tile as f32) / 7.0);
                }
            }
            blur_tile(&mut reflection, face, tile);
        }
        let edges = shore_edges(&water);
        let mut depth = Map::blank(128, 128);
        for y in 0..128 {
            for x in 0..128 {
                let mut p = Vec3::new(
                    lo.x + (x as f32 + 0.5) / 128.0 * (hi.x - lo.x),
                    hi.y - (y as f32 + 0.5) / 128.0 * (hi.y - lo.y),
                    hi.z + 10.0,
                );
                let i = (y * 128 + x) * 4;
                if let Some(hit) = water.trace(p, -Vec3::Z) {
                    p.z -= hit.distance + 0.01;
                    depth.rgba[i + 2] = 255;
                    if let Some(bed) = scene.trace(p, -Vec3::Z) {
                        depth.rgba[i] = (bed.distance * 0.12 / s.max_depth)
                            .clamp(0.0, 1.0)
                            .mul_add(255.0, 0.5) as u8;
                        depth.rgba[i + 1] = 255;
                    }
                    let distance = edges
                        .iter()
                        .map(|&(a, b)| {
                            let e = b - a;
                            let t = ((p.xy() - a).dot(e) / e.length_squared().max(1e-12))
                                .clamp(0.0, 1.0);
                            p.xy().distance(a + e * t)
                        })
                        .fold(f32::INFINITY, f32::min);
                    depth.rgba[i + 3] =
                        (distance * 0.12 / 12.0).clamp(0.0, 1.0).mul_add(255.0, 0.5) as u8;
                }
            }
            if y % 8 == 0 {
                progress((6.0 + y as f32 / 128.0) / 7.0);
            }
        }
        progress(1.0);
        Ok(BakedSurface {
            bounds,
            vertex_alpha,
            reflection: Some(reflection),
            depth: Some(depth),
        })
    }
}
fn cube_direction(face: usize, u: f32, v: f32) -> Vec3 {
    match face {
        0 => Vec3::new(1., -v, -u),
        1 => Vec3::new(-1., -v, u),
        2 => Vec3::new(u, 1., v),
        3 => Vec3::new(u, -1., -v),
        4 => Vec3::new(u, -v, 1.),
        _ => Vec3::new(-u, -v, -1.),
    }
}
fn sample_texture(t: &DecodedImage, uv: Vec2) -> Vec3 {
    let p = Vec2::new(
        uv.x.rem_euclid(1.0) * t.width as f32 - 0.5,
        uv.y.rem_euclid(1.0) * t.height as f32 - 0.5,
    );
    let i = p.floor().as_ivec2();
    let f = p - p.floor();
    let at = |x: i32, y: i32| {
        let k = (y.rem_euclid(t.height as i32) as usize * t.width as usize
            + x.rem_euclid(t.width as i32) as usize)
            * 4;
        Vec3::new(t.rgba[k] as f32, t.rgba[k + 1] as f32, t.rgba[k + 2] as f32)
    };
    at(i.x, i.y)
        .lerp(at(i.x + 1, i.y), f.x)
        .lerp(at(i.x, i.y + 1).lerp(at(i.x + 1, i.y + 1), f.x), f.y)
}
fn blur_tile(map: &mut Map, face: usize, tile: usize) {
    let source = map.rgba.clone();
    let weights = [0.0_f32, 1.0, 2.0].map(|v| (-v * v / (2.0 * 0.65 * 0.65)).exp());
    for y in 0..tile {
        for x in 0..tile {
            let mut sum = [0.0; 4];
            let mut total = 0.0;
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let w =
                        weights[dx.unsigned_abs() as usize] * weights[dy.unsigned_abs() as usize];
                    total += w;
                    let xx = (x as i32 + dx).clamp(0, tile as i32 - 1) as usize;
                    let yy = (y as i32 + dy).clamp(0, tile as i32 - 1) as usize;
                    let k =
                        ((face / 3 * tile + yy) * map.width as usize + face % 3 * tile + xx) * 4;
                    for c in 0..4 {
                        sum[c] += source[k + c] as f32 * w;
                    }
                }
            }
            let k = ((face / 3 * tile + y) * map.width as usize + face % 3 * tile + x) * 4;
            for c in 0..4 {
                map.rgba[k + c] = (sum[c] / total).round() as u8;
            }
        }
    }
}
fn shore_edges(water: &Rays) -> Vec<(Vec2, Vec2)> {
    let mut edges = BTreeMap::new();
    for t in &water.triangles {
        for (a, b) in [
            (t.points[0], t.points[1]),
            (t.points[1], t.points[2]),
            (t.points[2], t.points[0]),
        ] {
            let key = |p: Vec3| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
            let (ka, kb) = (key(a), key(b));
            let k = if ka < kb { (ka, kb) } else { (kb, ka) };
            edges
                .entry(k)
                .and_modify(|e: &mut (usize, Vec2, Vec2)| e.0 += 1)
                .or_insert((1, a.xy(), b.xy()));
        }
    }
    edges
        .into_values()
        .filter_map(|(n, a, b)| (n == 1).then_some((a, b)))
        .collect()
}

#[cfg(all(test, feature = "original-assets"))]
mod tests;
