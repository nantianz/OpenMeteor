mod spatial;
use spatial::{Bounds, Tree};

use bevy::ecs::prelude::Resource;
use meteor_asset::des::StageDescription;
use meteor_asset::gmdl::GmdlMesh;
use meteor_core::constants::MODEL_SCALE;
use meteor_physics::collision::{
    FootHit, GroundHit, MeshCollider, Point3, RayHit, BODY_BOX_HALF_XY, BODY_BOX_Z_HIGH,
    BODY_BOX_Z_LOW, WALKABLE_SURFACE_NY,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Resource, Default)]
pub struct StageCollider {
    pub mesh: Option<StageMeshCollider>,
}

const NORMAL_EPS: f32 = 1e-4;

const RAY_EPS: f32 = 1e-6;

const SEAM_RADIUS: f32 = 0.15;
const SEAM_DROP: f32 = 1.2;

#[derive(Debug, Clone, Default)]
pub struct StageMeshCollider {
    pub positions: Vec<[f32; 3]>,

    pub indices: Vec<u32>,

    pub wall_tris: Vec<u32>,

    pub floor_tris: Vec<u32>,

    pub steep_tris: Vec<u32>,

    pub ceiling_tris: Vec<u32>,

    tri_owner: Vec<i32>,
    disabled_owners: BTreeSet<i32>,

    body_normal_preserving_owners: BTreeSet<i32>,

    next_owner: i32,

    owner_vert_ranges: HashMap<i32, (usize, usize)>,

    spatial: Option<Tree>,
    owner_trees: HashMap<i32, Tree>,
}

impl StageMeshCollider {
    pub fn from_gmdl(
        gmdl: &GmdlMesh,
        desc: &StageDescription,
        collision_face_ids: Option<&BTreeSet<u32>>,
        center: [f32; 3],
    ) -> Self {
        let mut meta: BTreeMap<&str, Vec<(&str, bool)>> = BTreeMap::new();
        for obj in desc.scene_objects.iter().rev() {
            meta.entry(obj.name.as_str())
                .or_default()
                .push((obj.custom.as_str(), obj.block_player));
        }

        let mut collider = StageMeshCollider::default();
        let mut vert_base = 0usize;
        let mut face_base = 0u32;

        for obj in &gmdl.objects {
            let (custom, blocking) = meta
                .get_mut(obj.name.as_str())
                .and_then(Vec::pop)
                .unwrap_or(("", true));
            if is_non_solid_custom(custom) {
                face_base += obj.num_faces as u32;
                continue;
            }
            let wall_passable = is_wall_passable(custom, blocking);

            let base = collider.positions.len() as u32;
            for v in obj.positions.chunks_exact(3) {
                collider.positions.push([
                    (v[0] - center[0]) * MODEL_SCALE,
                    (v[2] - center[2]) * MODEL_SCALE,
                    -(v[1] - center[1]) * MODEL_SCALE,
                ]);
            }

            for f in 0..obj.num_faces {
                if let Some(ids) = collision_face_ids {
                    if !ids.contains(&(face_base + f as u32)) {
                        continue;
                    }
                } else {
                    let mat_idx = obj.face_mat.get(f).copied().unwrap_or(-1);
                    if let Some(name) = material_texture(gmdl, mat_idx) {
                        let n = name.to_ascii_lowercase();
                        if n.contains("sky") || n.contains("yin") {
                            continue;
                        }
                    }
                }

                let a = collider.positions[vert_base + obj.indices[f * 3] as usize];
                let b = collider.positions[vert_base + obj.indices[f * 3 + 1] as usize];
                let c = collider.positions[vert_base + obj.indices[f * 3 + 2] as usize];
                let (_, ny, _) = triangle_normal(a, b, c);

                let is_floor = ny >= WALKABLE_SURFACE_NY;
                let is_steep = ny > NORMAL_EPS && ny < WALKABLE_SURFACE_NY;
                let is_ceiling = ny <= -WALKABLE_SURFACE_NY;
                let is_wall = !is_floor && !is_steep && !is_ceiling;

                if (is_wall || is_ceiling) && wall_passable {
                    continue;
                }
                if !is_floor && !blocking && !wall_passable {
                    continue;
                }

                let tri = collider.indices.len() as u32 / 3;
                collider.indices.extend_from_slice(&[
                    base + obj.indices[f * 3],
                    base + obj.indices[f * 3 + 1],
                    base + obj.indices[f * 3 + 2],
                ]);

                collider.tri_owner.push(-1);
                if is_wall {
                    collider.wall_tris.push(tri);
                } else if is_floor {
                    collider.floor_tris.push(tri);
                } else if is_steep {
                    collider.steep_tris.push(tri);
                } else {
                    collider.ceiling_tris.push(tri);
                }
            }
            vert_base += obj.num_verts;
            face_base += obj.num_faces as u32;
        }

        collider.append_boundary_walls();
        collider.rebuild_static_index();
        collider
    }

    fn append_boundary_walls(&mut self) {
        if self.positions.is_empty() {
            return;
        }
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for p in &self.positions {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        const MARGIN: f32 = 12.0;
        let (x0, x1) = (min[0] - MARGIN, max[0] + MARGIN);
        let (z0, z1) = (min[2] - MARGIN, max[2] + MARGIN);
        let y0 = min[1] - 4.0;
        let y1 = max[1] + 40.0;

        let walls = [
            ([x0, y0, z0], [x0 + 2.0, y1, z1]),
            ([x1 - 2.0, y0, z0], [x1, y1, z1]),
            ([x0, y0, z0], [x1, y1, z0 + 2.0]),
            ([x0, y0, z1 - 2.0], [x1, y1, z1]),
        ];
        for (a, b) in walls {
            let base = self.positions.len() as u32;
            self.positions.push([a[0], a[1], a[2]]);
            self.positions.push([b[0], a[1], b[2]]);
            self.positions.push([b[0], b[1], b[2]]);
            self.positions.push([a[0], b[1], a[2]]);
            let tri = self.indices.len() as u32 / 3;
            self.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            self.tri_owner.push(-1);
            self.tri_owner.push(-1);
            self.wall_tris.push(tri);
            self.wall_tris.push(tri + 1);
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn append_prop_world_triangles(&mut self, verts: &[[f32; 3]], indices: &[u32]) -> i32 {
        let owner = self.append_world_triangles(verts, indices);
        self.body_normal_preserving_owners.insert(owner);
        owner
    }

    pub fn append_world_triangles(&mut self, verts: &[[f32; 3]], indices: &[u32]) -> i32 {
        if self.spatial.is_none() {
            self.rebuild_static_index();
        }
        let owner = self.next_owner;
        self.next_owner += 1;
        let base = self.positions.len() as u32;
        let vert_start = self.positions.len();
        self.positions.extend_from_slice(verts);
        self.owner_vert_ranges
            .insert(owner, (vert_start, self.positions.len()));
        for idx in indices.chunks_exact(3) {
            let a = verts[idx[0] as usize];
            let b = verts[idx[1] as usize];
            let c = verts[idx[2] as usize];
            let (_, ny, _) = triangle_normal(a, b, c);
            let is_floor = ny >= WALKABLE_SURFACE_NY;
            let is_steep = ny > NORMAL_EPS && ny < WALKABLE_SURFACE_NY;
            let is_ceiling = ny <= -WALKABLE_SURFACE_NY;
            let is_wall = !is_floor && !is_steep && !is_ceiling;

            let tri = self.indices.len() as u32 / 3;
            self.indices
                .extend_from_slice(&[base + idx[0], base + idx[1], base + idx[2]]);
            self.tri_owner.push(owner);
            if is_wall {
                self.wall_tris.push(tri);
            } else if is_floor {
                self.floor_tris.push(tri);
            } else if is_steep {
                self.steep_tris.push(tri);
            } else {
                self.ceiling_tris.push(tri);
            }
        }
        let tree = Tree::build(
            self.classified_triangles()
                .filter(|(tri, _)| self.tri_owner[*tri as usize] == owner),
            &self.positions,
            &self.indices,
        );
        self.owner_trees.insert(owner, tree);
        owner
    }

    pub fn update_owner_vertices(&mut self, owner: i32, verts: &[[f32; 3]]) {
        let _ = self.update_owner_vertices_range(owner, 0, verts);
    }

    pub fn update_owner_vertices_range(
        &mut self,
        owner: i32,
        offset: usize,
        verts: &[[f32; 3]],
    ) -> bool {
        let Some(&(start, end)) = self.owner_vert_ranges.get(&owner) else {
            return false;
        };
        let n = end - start;
        if offset > n || verts.len() > n - offset {
            return false;
        }
        let target = start + offset;
        if self.positions[target..target + verts.len()] == *verts {
            return true;
        }
        self.positions[target..target + verts.len()].copy_from_slice(verts);
        if let Some(tree) = self.owner_trees.get_mut(&owner) {
            tree.refit(&self.positions, &self.indices);
        }
        true
    }

    pub fn set_owner_enabled(&mut self, owner: i32, enabled: bool) {
        if enabled {
            self.disabled_owners.remove(&owner);
        } else {
            self.disabled_owners.insert(owner);
        }
    }

    fn body_box_contact(&self, cx: f32, cy: f32, cz: f32) -> Option<(i32, FootHit)> {
        #[cfg(not(target_arch = "wasm32"))]
        let _perf = super::perf_plugin::Scope::new(2);
        let hx = BODY_BOX_HALF_XY;
        let hz = BODY_BOX_HALF_XY;
        let (y_min, y_max) = (cy + BODY_BOX_Z_LOW, cy + BODY_BOX_Z_HIGH);
        let (x_min, x_max) = (cx - hx, cx + hx);
        let (z_min, z_max) = (cz - hz, cz + hz);
        let mut candidates = self.candidates(
            Bounds::between([x_min, y_min, z_min], [x_max, y_max, z_max]),
            15,
        );

        candidates.sort_unstable();
        for tri in candidates {
            if !self.enabled(tri) {
                continue;
            }
            let (a, b, c) = tri_verts(self, tri);
            if b[1].min(a[1]).min(c[1]) > y_max + 1e-4 || a[1].max(b[1]).max(c[1]) < y_min - 1e-4 {
                continue;
            }
            if box_tri_overlap(
                a, b, c, cx, x_min, x_max, y_min, y_max, z_min, z_max, hx, hz,
            ) {
                let (nx, ny, nz) = triangle_normal(a, b, c);
                return Some((
                    self.tri_owner.get(tri as usize).copied().unwrap_or(-1),
                    FootHit { y: cy, nx, ny, nz },
                ));
            }
        }
        None
    }

    fn enabled(&self, tri: u32) -> bool {
        let owner = self.tri_owner.get(tri as usize).copied();
        match owner {
            Some(o) if o >= 0 => !self.disabled_owners.contains(&o),
            _ => true,
        }
    }

    fn classified_triangles(&self) -> impl Iterator<Item = (u32, u8)> + '_ {
        [
            &self.wall_tris,
            &self.floor_tris,
            &self.steep_tris,
            &self.ceiling_tris,
        ]
        .into_iter()
        .enumerate()
        .flat_map(|(class, tris)| tris.iter().map(move |&tri| (tri, class as u8)))
    }

    fn rebuild_static_index(&mut self) {
        self.spatial = Some(Tree::build(
            self.classified_triangles()
                .filter(|(tri, _)| self.tri_owner.get(*tri as usize).is_none_or(|&o| o < 0)),
            &self.positions,
            &self.indices,
        ));
    }

    fn candidates(&self, bounds: Bounds, mask: u8) -> Vec<u32> {
        let Some(tree) = &self.spatial else {
            return self
                .classified_triangles()
                .filter(|(_, class)| mask & (1 << class) != 0)
                .map(|(tri, _)| tri)
                .collect();
        };
        let mut found = Vec::new();
        tree.query(bounds, mask, &mut found);
        for (owner, tree) in &self.owner_trees {
            if !self.disabled_owners.contains(owner) {
                tree.query(bounds, mask, &mut found);
            }
        }

        found.sort_unstable();
        found.into_iter().map(|(_, tri)| tri).collect()
    }

    fn support_ray_hit(
        &self,
        x: f32,
        z: f32,
        start_y: f32,
        max_drop: f32,
        max_y: f32,
    ) -> Option<GroundHit> {
        let mut best_t = max_drop;
        let mut best = None;
        for tri in self.candidates(
            Bounds::between([x, start_y, z], [x, start_y - max_drop, z]),
            2,
        ) {
            if !self.enabled(tri) {
                continue;
            }
            let (a, b, c) = tri_verts(self, tri);
            let Some(hit_t) = ray_triangle(
                x, start_y, z, 0.0, -1.0, 0.0, a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2],
            ) else {
                continue;
            };

            if hit_t <= 1e-4 || hit_t >= best_t || start_y - hit_t > max_y {
                continue;
            }
            best_t = hit_t;
            let (nx, ny, nz) = triangle_normal(a, b, c);
            best = Some(GroundHit {
                y: start_y - hit_t,
                nx,
                ny,
                nz,
            });
        }
        best
    }

    fn seam_support(&self, x: f32, z: f32, hint_y: f32) -> Option<GroundHit> {
        let mut best: Option<(f32, GroundHit)> = None;
        for tri in self.candidates(
            Bounds::between(
                [x - SEAM_RADIUS, hint_y - SEAM_DROP, z - SEAM_RADIUS],
                [x + SEAM_RADIUS, hint_y + 0.35, z + SEAM_RADIUS],
            ),
            2,
        ) {
            if !self.enabled(tri) {
                continue;
            }
            let (a, b, c) = tri_verts(self, tri);
            let min_y = a[1].min(b[1]).min(c[1]);
            let max_y = a[1].max(b[1]).max(c[1]);
            if min_y > hint_y + 0.35 || max_y < hint_y - SEAM_DROP {
                continue;
            }
            let closest = closest_point_tri_2d(x, z, a, b, c);
            let distance = (x - closest[0]).hypot(z - closest[1]);
            if distance > SEAM_RADIUS {
                continue;
            }
            let Some(y) = interpolate_tri_y(closest[0], closest[1], a, b, c) else {
                continue;
            };
            if y > hint_y + 0.35 || y < hint_y - SEAM_DROP {
                continue;
            }
            if best
                .as_ref()
                .is_some_and(|(best_distance, _)| distance >= *best_distance)
            {
                continue;
            }
            let (nx, ny, nz) = triangle_normal(a, b, c);
            best = Some((distance, GroundHit { y, nx, ny, nz }));
        }
        best.map(|(_, hit)| hit)
    }
}

fn is_non_solid_custom(custom: &str) -> bool {
    regex_contains(custom, r"objecttype\s*=\s*camerabox")
}

fn is_wall_passable(custom: &str, blocking: bool) -> bool {
    if !blocking {
        return true;
    }
    regex_contains(custom, r"blockplayer\s*=\s*no")
}

fn regex_contains(text: &str, pattern: &str) -> bool {
    let compact = pattern.replace(r"\s*", "").replace(r"\b", "");
    let Some((key, val)) = compact.split_once('=') else {
        return text
            .to_ascii_lowercase()
            .contains(&pattern.to_ascii_lowercase());
    };
    custom_has_kv(text, key, val)
}

fn custom_has_kv(text: &str, key: &str, value: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let key = key.to_ascii_lowercase();
    let value = value.to_ascii_lowercase();
    let mut start = 0usize;
    while let Some(rel) = lower[start..].find(key.as_str()) {
        let abs = start + rel;
        if abs > 0 {
            let prev = lower.as_bytes()[abs - 1] as char;
            if prev.is_ascii_alphanumeric() || prev == '_' {
                start = abs + 1;
                continue;
            }
        }
        let after = lower[abs + key.len()..].trim_start();
        let Some(rest) = after.strip_prefix('=') else {
            start = abs + 1;
            continue;
        };
        let rest = rest.trim_start().trim_start_matches('"');
        if rest.starts_with(value.as_str()) {
            let after_val = &rest[value.len()..];
            if after_val.is_empty()
                || after_val.starts_with(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            {
                return true;
            }
        }
        start = abs + 1;
    }
    false
}

fn material_texture(gmdl: &GmdlMesh, mat_idx: i32) -> Option<&str> {
    if mat_idx < 0 || mat_idx as usize >= gmdl.materials.len() {
        return None;
    }
    gmdl.resolve_texture_name(&gmdl.materials[mat_idx as usize])
}

pub fn block_map_from_des(desc: &StageDescription) -> BTreeMap<String, bool> {
    desc.scene_objects
        .iter()
        .map(|obj| (obj.name.clone(), obj.block_player))
        .collect()
}

fn triangle_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> (f32, f32, f32) {
    let (e1x, e1y, e1z) = (b[0] - a[0], b[1] - a[1], b[2] - a[2]);
    let (e2x, e2y, e2z) = (c[0] - a[0], c[1] - a[1], c[2] - a[2]);
    let nx = e1y * e2z - e1z * e2y;
    let ny = e1z * e2x - e1x * e2z;
    let nz = e1x * e2y - e1y * e2x;
    let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-8);
    (nx / len, ny / len, nz / len)
}

fn tri_verts(c: &StageMeshCollider, tri: u32) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let i = tri as usize * 3;
    let a = c.positions[c.indices[i] as usize];
    let b = c.positions[c.indices[i + 1] as usize];
    let d = c.positions[c.indices[i + 2] as usize];
    (a, b, d)
}

fn interpolate_tri_y(x: f32, z: f32, a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<f32> {
    let denom = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
    if denom.abs() < 1e-8 {
        return None;
    }
    let wa = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / denom;
    let wb = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / denom;
    let wc = 1.0 - wa - wb;
    if wa < -1e-5 || wb < -1e-5 || wc < -1e-5 {
        return None;
    }
    Some(wa * a[1] + wb * b[1] + wc * c[1])
}

fn closest_point_segment_2d(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    if len2 < 1e-12 {
        return a;
    }
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0);
    [a[0] + ab[0] * t, a[1] + ab[1] * t]
}

fn closest_point_tri_2d(x: f32, z: f32, a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 2] {
    if interpolate_tri_y(x, z, a, b, c).is_some() {
        return [x, z];
    }
    let p = [x, z];
    let mut best = closest_point_segment_2d(p, [a[0], a[2]], [b[0], b[2]]);
    let mut best_d2 = (best[0] - x).powi(2) + (best[1] - z).powi(2);
    for candidate in [
        closest_point_segment_2d(p, [b[0], b[2]], [c[0], c[2]]),
        closest_point_segment_2d(p, [c[0], c[2]], [a[0], a[2]]),
    ] {
        let d2 = (candidate[0] - x).powi(2) + (candidate[1] - z).powi(2);
        if d2 < best_d2 {
            best = candidate;
            best_d2 = d2;
        }
    }
    best
}

#[allow(clippy::too_many_arguments)]
fn ray_triangle(
    ox: f32,
    oy: f32,
    oz: f32,
    dx: f32,
    dy: f32,
    dz: f32,
    ax: f32,
    ay: f32,
    az: f32,
    bx: f32,
    by: f32,
    bz: f32,
    cx: f32,
    cy: f32,
    cz: f32,
) -> Option<f32> {
    let (e1x, e1y, e1z) = (bx - ax, by - ay, bz - az);
    let (e2x, e2y, e2z) = (cx - ax, cy - ay, cz - az);
    let px = dy * e2z - dz * e2y;
    let py = dz * e2x - dx * e2z;
    let pz = dx * e2y - dy * e2x;
    let det = e1x * px + e1y * py + e1z * pz;
    if det > -RAY_EPS && det < RAY_EPS {
        return None;
    }
    let inv_det = 1.0 / det;
    let (tx, ty, tz) = (ox - ax, oy - ay, oz - az);
    let u = (tx * px + ty * py + tz * pz) * inv_det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qx = ty * e1z - tz * e1y;
    let qy = tz * e1x - tx * e1z;
    let qz = tx * e1y - ty * e1x;
    let v = (dx * qx + dy * qy + dz * qz) * inv_det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = (e2x * qx + e2y * qy + e2z * qz) * inv_det;
    (t >= 0.0).then_some(t)
}

#[allow(clippy::too_many_arguments)]
fn box_tri_overlap(
    v0: [f32; 3],
    v1: [f32; 3],
    v2: [f32; 3],
    cx: f32,
    x_min: f32,
    x_max: f32,
    y_min: f32,
    y_max: f32,
    z_min: f32,
    z_max: f32,
    hx: f32,
    hz: f32,
) -> bool {
    let (f0x, f0y, f0z) = (v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]);
    let (f1x, f1y, f1z) = (v2[0] - v1[0], v2[1] - v1[1], v2[2] - v1[2]);
    let (f2x, f2y, f2z) = (v0[0] - v2[0], v0[1] - v2[1], v0[2] - v2[2]);

    let (nx, ny, nz) = (
        f0y * f1z - f0z * f1y,
        f0z * f1x - f0x * f1z,
        f0x * f1y - f0y * f1x,
    );
    let t0 = v0[0] * nx + v0[1] * ny + v0[2] * nz;
    let t1 = v1[0] * nx + v1[1] * ny + v1[2] * nz;
    let t2 = v2[0] * nx + v2[1] * ny + v2[2] * nz;
    let t_min_n = t0.min(t1).min(t2);
    let t_max_n = t0.max(t1).max(t2);
    let b_extent_n = nx.abs() * hx + ny.abs() * (y_max - y_min) * 0.5 + nz.abs() * hz;
    let b_center_n = cx * nx + (y_min + y_max) * 0.5 * ny + (z_min + z_max) * 0.5 * nz;
    if (b_center_n - (t_min_n + t_max_n) * 0.5).abs()
        > (t_max_n - t_min_n) * 0.5 + b_extent_n + 1e-5
    {
        return false;
    }

    let t_min_x = v0[0].min(v1[0]).min(v2[0]);
    let t_max_x = v0[0].max(v1[0]).max(v2[0]);
    if t_max_x < x_min - 1e-5 || t_min_x > x_max + 1e-5 {
        return false;
    }
    let t_min_y = v0[1].min(v1[1]).min(v2[1]);
    let t_max_y = v0[1].max(v1[1]).max(v2[1]);
    if t_max_y < y_min - 1e-5 || t_min_y > y_max + 1e-5 {
        return false;
    }
    let t_min_z = v0[2].min(v1[2]).min(v2[2]);
    let t_max_z = v0[2].max(v1[2]).max(v2[2]);
    if t_max_z < z_min - 1e-5 || t_min_z > z_max + 1e-5 {
        return false;
    }

    let edges = [[f0x, f0y, f0z], [f1x, f1y, f1z], [f2x, f2y, f2z]];
    let box_half = [hx, (y_max - y_min) * 0.5, hz];
    let box_center = [cx, (y_min + y_max) * 0.5, (z_min + z_max) * 0.5];
    let verts = [v0, v1, v2];
    for f in edges {
        for i in 0..3 {
            let (ax, ay, az) = match i {
                0 => (0.0, -f[2], f[1]),
                1 => (f[2], 0.0, -f[0]),
                _ => (-f[1], f[0], 0.0),
            };
            let axis_len = (ax * ax + ay * ay + az * az).sqrt();
            if axis_len < 1e-8 {
                continue;
            }
            let p0 = verts[0][0] * ax + verts[0][1] * ay + verts[0][2] * az;
            let p1 = verts[1][0] * ax + verts[1][1] * ay + verts[1][2] * az;
            let p2 = verts[2][0] * ax + verts[2][1] * ay + verts[2][2] * az;
            let t_min_p = p0.min(p1).min(p2);
            let t_max_p = p0.max(p1).max(p2);
            let b_center_p = box_center[0] * ax + box_center[1] * ay + box_center[2] * az;
            let b_extent_p =
                ax.abs() * box_half[0] + ay.abs() * box_half[1] + az.abs() * box_half[2];
            if (b_center_p - (t_min_p + t_max_p) * 0.5).abs()
                > (t_max_p - t_min_p) * 0.5 + b_extent_p + 1e-5
            {
                return false;
            }
        }
    }
    true
}

impl MeshCollider for StageMeshCollider {
    fn sample_ground_hit(&self, x: f32, z: f32, fallback: f32, hint_y: f32) -> Option<GroundHit> {
        #[cfg(not(target_arch = "wasm32"))]
        let _perf = super::perf_plugin::Scope::new(0);
        let start_y = hint_y + 6.0;
        let max_drop = 48.0_f32.max(start_y - (fallback - 24.0));
        let hit = self.support_ray_hit(x, z, start_y, max_drop, hint_y + 1.5);
        if hit.as_ref().is_some_and(|hit| hit.y >= hint_y - SEAM_DROP) {
            return hit;
        }
        self.seam_support(x, z, hint_y).or(hit)
    }

    fn sweep_foot_segment(&self, x: f32, y_top: f32, z: f32, y_bottom: f32) -> Option<FootHit> {
        #[cfg(not(target_arch = "wasm32"))]
        let _perf = super::perf_plugin::Scope::new(1);
        let (dx, dy, dz) = (0.0, y_bottom - y_top, 0.0);
        let mut best_t: f32 = 1.0;
        let mut best: Option<FootHit> = None;
        for tri in self.candidates(Bounds::between([x, y_top, z], [x, y_bottom, z]), 15) {
            if !self.enabled(tri) {
                continue;
            }
            let (a, b, c) = tri_verts(self, tri);
            let hit_t = ray_triangle(
                x, y_top, z, dx, dy, dz, a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2],
            );
            let Some(hit_t) = hit_t else { continue };
            if hit_t < 1e-4 || hit_t >= best_t {
                continue;
            }
            best_t = hit_t;
            let hy = y_top + dy * hit_t;
            let (nx, ny, nz) = triangle_normal(a, b, c);
            best = Some(FootHit { y: hy, nx, ny, nz });
        }
        best
    }

    fn box_blocked(&self, cx: f32, cy: f32, cz: f32) -> bool {
        self.body_box_contact(cx, cy, cz).is_some()
    }

    fn body_contact_surface(&self, cx: f32, cy: f32, cz: f32) -> Option<FootHit> {
        self.body_box_contact(cx, cy, cz).and_then(|(owner, hit)| {
            (!self.body_normal_preserving_owners.contains(&owner)).then_some(hit)
        })
    }

    fn raycast_hit(&self, origin: Point3, direction: Point3, max_dist: f32) -> Option<RayHit> {
        #[cfg(not(target_arch = "wasm32"))]
        let _perf = super::perf_plugin::Scope::new(3);
        let (ox, oy, oz) = (origin.x, origin.y, origin.z);
        let (dx, dy, dz) = (direction.x, direction.y, direction.z);
        let len = (dx * dx + dy * dy + dz * dz).sqrt();
        if len < 1e-8 {
            return None;
        }

        let (dx, dy, dz) = (dx / len, dy / len, dz / len);
        let vertical = dx.abs() < 1e-6 && dz.abs() < 1e-6;

        let mut best_t = max_dist;
        let mut best: Option<RayHit> = None;

        let mask = if vertical && dy > 0.0 { 8 } else { 15 };
        for tri in self.candidates(
            Bounds::between(
                [ox, oy, oz],
                [ox + dx * max_dist, oy + dy * max_dist, oz + dz * max_dist],
            ),
            mask,
        ) {
            if !self.enabled(tri) {
                continue;
            }
            let (a, b, c) = tri_verts(self, tri);
            let Some(hit_t) = ray_triangle(
                ox, oy, oz, dx, dy, dz, a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2],
            ) else {
                continue;
            };
            if hit_t <= 1e-4 || hit_t >= best_t {
                continue;
            }
            best_t = hit_t;
            let (nx, ny, nz) = triangle_normal(a, b, c);
            best = Some(RayHit {
                t: hit_t,
                y: oy + dy * hit_t,
                nx,
                ny,
                nz,
            });
        }
        best
    }

    fn segment_hit(&self, start: Point3, end: Point3) -> Option<RayHit> {
        let d = [end.x - start.x, end.y - start.y, end.z - start.z];
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if length < 1e-8 {
            return None;
        }
        let d = [d[0] / length, d[1] / length, d[2] / length];
        let mut best: Option<RayHit> = None;
        for tri in self.candidates(
            Bounds::between([start.x, start.y, start.z], [end.x, end.y, end.z]),
            15,
        ) {
            if !self.enabled(tri) {
                continue;
            }
            let (a, b, c) = tri_verts(self, tri);
            let Some(t) = ray_triangle(
                start.x, start.y, start.z, d[0], d[1], d[2], a[0], a[1], a[2], b[0], b[1], b[2],
                c[0], c[1], c[2],
            ) else {
                continue;
            };

            if t > length || best.is_some_and(|hit| t >= hit.t) {
                continue;
            }
            let (nx, ny, nz) = triangle_normal(a, b, c);
            best = Some(RayHit {
                t,
                y: start.y + d[1] * t,
                nx,
                ny,
                nz,
            });
        }
        best
    }
}

#[cfg(test)]
pub(crate) fn sn15_test_mesh() -> StageMeshCollider {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let gmdl = meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn15/sn15.gmb")).unwrap())
        .unwrap();
    let desc = meteor_asset::des::parse_stage_description(
        &std::fs::read_to_string(root.join("sn15/sn15.des")).unwrap(),
    );
    let cob =
        meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn15/sn15.cob")).unwrap()).unwrap();
    let wp = std::fs::read_to_string(root.join("Level/sn15.wp")).unwrap();
    StageMeshCollider::from_gmdl(
        &gmdl,
        &desc,
        Some(&cob.face_ids),
        meteor_scene::stage_loader::get_stage_center("sn15", Some(&wp)),
    )
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prop_body_rejection_preserves_support_normal_without_removing_collision() {
        use meteor_physics::collision::{descent_has_support, CollisionSystem};
        let vertices = [
            [0.0, -5.0, -5.0],
            [0.0, 8.0, -5.0],
            [0.0, 8.0, 5.0],
            [0.0, -5.0, 5.0],
        ];
        let indices = [0, 1, 2, 0, 2, 3];
        let mut mesh = StageMeshCollider::default();
        let owner = mesh.append_prop_world_triangles(&vertices, &indices);
        assert!(mesh.box_blocked(0.3, 0.0, 0.0));
        assert!(mesh.body_contact_surface(0.3, 0.0, 0.0).is_none());
        let mut mover = CollisionSystem::new(0.55, 1.8);
        mover.set_mesh_collider(&mesh);
        let pos = Point3::new(0.3, 0.0, 0.0);
        let (dy, normal, blocked) = mover.resolve_descent(pos, -0.1);
        assert_eq!(dy, 0.0);
        assert!(blocked && descent_has_support(normal, blocked));
        let steep = FootHit {
            y: 0.0,
            nx: 0.98,
            ny: 0.2,
            nz: 0.0,
        };
        let (_, normal, blocked) = mover.resolve_descent_with_surface(pos, -0.1, Some(steep));
        assert_eq!(normal.unwrap().ny, steep.ny);
        assert!(blocked && !descent_has_support(normal, blocked));
        mesh.set_owner_enabled(owner, false);
        assert!(!mesh.box_blocked(0.3, 0.0, 0.0));
        mesh.append_world_triangles(&vertices, &indices);
        assert!(mesh.box_blocked(0.3, 0.0, 0.0));
        assert_eq!(mesh.body_contact_surface(0.3, 0.0, 0.0).unwrap().ny, 0.0);
    }

    #[test]
    fn sn15_duplicate_names_do_not_disable_the_reported_wall() {
        let mesh = sn15_test_mesh();

        assert!(mesh.box_blocked(17.9, 0.0, 55.5));
        for (z, direction) in [(54.0, 1.0), (57.5, -1.0)] {
            let hit = mesh
                .raycast_hit(
                    Point3::new(17.9, 2.0, z),
                    Point3::new(0.0, 0.0, direction),
                    4.0,
                )
                .expect("reported wall must exist on both sides");
            assert!((hit.t - (55.854687 - z).abs()).abs() < 0.001);
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn15/sn15.gmb")).unwrap())
                .unwrap();
        let mut desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn15/sn15.des")).unwrap(),
        );
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn15/sn15.cob")).unwrap())
            .unwrap();
        let wp = std::fs::read_to_string(root.join("Level/sn15.wp")).unwrap();

        for (i, (geometry, metadata)) in gmdl
            .objects
            .iter_mut()
            .zip(&mut desc.scene_objects)
            .enumerate()
        {
            assert_eq!(geometry.name, metadata.name);
            geometry.name = format!("object_{i}");
            metadata.name = geometry.name.clone();
        }
        let unique = StageMeshCollider::from_gmdl(
            &gmdl,
            &desc,
            Some(&cob.face_ids),
            meteor_scene::stage_loader::get_stage_center("sn15", Some(&wp)),
        );
        assert_eq!(mesh.indices, unique.indices);
        assert_eq!(mesh.positions, unique.positions);
    }

    #[test]
    fn sn15_wall_endpoint_requires_the_accepted_sweep_height() {
        let mesh = sn15_test_mesh();
        let mut mover = meteor_physics::collision::CollisionSystem::new(0.55, 1.8);
        mover.set_mesh_collider(&mesh);
        let p = Point3::new(2.7278004, 8.0, 46.980072);
        assert!(mover.can_occupy(p));
        let (rx, rz, ry) = mover.resolve_xz(p, 0.0, 0.25, false);
        assert!((ry - 0.33230114).abs() < 1e-4);
        assert!(mover.can_occupy(Point3::new(p.x + rx, p.y + ry, p.z + rz)));
        assert!(!mover.can_occupy(Point3::new(p.x + rx, p.y, p.z + rz)));
    }

    fn assert_same_queries(indexed: &StageMeshCollider, p: Point3, direction: Point3) {
        let mut exhaustive = indexed.clone();
        exhaustive.spatial = None;
        let foot = |h: Option<FootHit>| h.map(|h| (h.y, h.nx, h.ny, h.nz));
        let ground = |h: Option<GroundHit>| h.map(|h| (h.y, h.nx, h.ny, h.nz));
        let ray = |h: Option<RayHit>| h.map(|h| (h.t, h.y, h.nx, h.ny, h.nz));
        assert_eq!(
            foot(indexed.body_contact_surface(p.x, p.y, p.z)),
            foot(exhaustive.body_contact_surface(p.x, p.y, p.z)),
            "body {p:?}"
        );
        assert_eq!(
            foot(indexed.sweep_foot_segment(p.x, p.y + 8.0, p.z, p.y - 12.0)),
            foot(exhaustive.sweep_foot_segment(p.x, p.y + 8.0, p.z, p.y - 12.0)),
            "foot {p:?}"
        );
        assert_eq!(
            ground(indexed.sample_ground_hit(p.x, p.z, -200.0, p.y)),
            ground(exhaustive.sample_ground_hit(p.x, p.z, -200.0, p.y)),
            "ground {p:?}"
        );
        assert_eq!(
            ray(indexed.raycast_hit(p, direction, 300.0)),
            ray(exhaustive.raycast_hit(p, direction, 300.0)),
            "ray {p:?} {direction:?}"
        );
    }

    #[test]
    fn spatial_queries_match_exhaustive_real_maps_and_boundaries() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        for stage in ["sn01", "sn09", "sn13"] {
            let gmdl = meteor_asset::gmdl::decode_gmdl(
                &std::fs::read(root.join(format!("{stage}/{stage}.gmb"))).unwrap(),
            )
            .unwrap();
            let desc = meteor_asset::des::parse_stage_description(
                &std::fs::read_to_string(root.join(format!("{stage}/{stage}.des"))).unwrap(),
            );
            let cob = meteor_asset::cob::parse_cob(
                &std::fs::read(root.join(format!("{stage}/{stage}.cob"))).unwrap(),
            )
            .unwrap();
            let mesh = StageMeshCollider::from_gmdl(
                &gmdl,
                &desc,
                Some(&cob.face_ids),
                meteor_scene::stage_loader::get_stage_center(stage, None),
            );
            let mut candidates = 0;
            for i in 0..128 {
                let tri = (i * 7919 % mesh.triangle_count()) as u32;
                let (a, b, c) = tri_verts(&mesh, tri);
                let p = Point3::new(
                    (a[0] + b[0] + c[0]) / 3.0,
                    (a[1] + b[1] + c[1]) / 3.0 + 0.1,
                    (a[2] + b[2] + c[2]) / 3.0,
                );
                for direction in [
                    Point3::new(0.0, -1.0, 0.0),
                    Point3::new(0.0, 1.0, 0.0),
                    Point3::new(0.7, 0.2, -0.8),
                ] {
                    assert_same_queries(&mesh, p, direction);
                }

                assert_same_queries(
                    &mesh,
                    Point3::new(a[0], a[1], a[2]),
                    Point3::new(-0.5, -0.1, 0.3),
                );
                candidates += mesh
                    .candidates(
                        Bounds::between(
                            [p.x - 1.0, p.y, p.z - 1.0],
                            [p.x + 1.0, p.y + 5.0, p.z + 1.0],
                        ),
                        15,
                    )
                    .len();
            }
            assert!(
                candidates < mesh.triangle_count() * 128 / 4,
                "broad phase must substantially prune {stage}"
            );
            eprintln!(
                "{stage}: {} triangles -> {:.1} mean local-box candidates",
                mesh.triangle_count(),
                candidates as f64 / 128.0
            );
        }
    }

    #[test]
    fn spatial_prop_refit_disable_and_multipart_preserve_results() {
        let mut mesh = StageMeshCollider::default();

        let vertices = [
            [-4.0, 0.0, -4.0],
            [0.0, 0.0, 4.0],
            [4.0, 0.0, -4.0],
            [0.0, 0.0, -4.0],
            [0.0, 8.0, -4.0],
            [0.0, 0.0, 4.0],
        ];
        let owner = mesh.append_world_triangles(&vertices, &[0, 1, 2, 3, 4, 5]);
        mesh.append_world_triangles(&vertices, &[0, 1, 2, 3, 4, 5]);
        for step in 0..6 {
            let moved = vertices[..3]
                .iter()
                .map(|v| [v[0] + step as f32 * 16.0, v[1] + step as f32, v[2]])
                .collect::<Vec<_>>();
            assert!(mesh.update_owner_vertices_range(owner, 0, &moved));
            for enabled in [false, true] {
                mesh.set_owner_enabled(owner, enabled);
                for x in [0.0, 0.001, -0.001, step as f32 * 16.0, -100.0] {
                    assert_same_queries(
                        &mesh,
                        Point3::new(x, 2.0, 0.0),
                        Point3::new(1.0, -0.1, 0.0),
                    );
                }
            }
        }
        assert!(!mesh.update_owner_vertices_range(owner, 6, &vertices));
        assert_same_queries(
            &mesh,
            Point3::new(0.0, 2.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        );
    }

    #[test]
    fn overhead_surface_does_not_hide_the_lower_floor() {
        let mut mesh = StageMeshCollider::default();
        for y in [5.0, -10.0] {
            mesh.append_world_triangles(
                &[[-10.0, y, -10.0], [0.0, y, 10.0], [10.0, y, -10.0]],
                &[0, 1, 2],
            );
        }
        let hit = mesh.sample_ground_hit(0.0, 0.0, 0.0, 0.0).unwrap();
        assert!((hit.y + 10.0).abs() < 1e-5);
        let upper = mesh.sample_ground_hit(0.0, 0.0, 0.0, 5.0).unwrap();
        assert!((upper.y - 5.0).abs() < 1e-5);
    }

    #[test]
    fn steep_faces_block_the_body_but_are_not_standing_ground() {
        let mut mesh = StageMeshCollider::default();
        mesh.append_world_triangles(
            &[
                [-20.0, -10.0, -20.0],
                [0.0, -10.0, 20.0],
                [20.0, -10.0, -20.0],
            ],
            &[0, 1, 2],
        );
        mesh.append_world_triangles(
            &[
                [-5.0, 0.0, 0.0],
                [5.0, 0.0, 0.0],
                [5.0, 20.0, 5.0],
                [-5.0, 20.0, 5.0],
            ],
            &[0, 2, 1, 0, 3, 2],
        );
        let support = mesh.sample_ground_hit(0.0, 2.0, -10.0, 8.0).unwrap();
        assert!((support.y + 10.0).abs() < 1e-5);
        let body = mesh.body_contact_surface(0.0, 7.5, 2.0).unwrap();
        assert!(body.ny > 0.0 && body.ny < WALKABLE_SURFACE_NY);
        assert!(!meteor_physics::collision::descent_has_support(
            Some(body),
            true
        ));
    }

    #[test]
    fn horizontal_wall_probe_includes_downward_bevel_faces() {
        let mut mesh = StageMeshCollider::default();
        mesh.append_world_triangles(
            &[
                [-5.0, 0.0, 2.0],
                [5.0, 0.0, 2.0],
                [5.0, 5.0, -3.0],
                [-5.0, 5.0, -3.0],
            ],
            &[0, 2, 1, 0, 3, 2],
        );
        assert_eq!(mesh.ceiling_tris.len(), 2);
        let hit = mesh
            .raycast_hit(Point3::new(0.0, 1.0, 0.0), Point3::new(0.0, 0.0, 1.0), 1.92)
            .unwrap();
        assert!((hit.t - 1.0).abs() < 1e-5);
        assert!(hit.ny < -0.55);
    }

    #[test]
    fn tri_norm() {
        let (_, ny, _) = triangle_normal([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
        assert!((ny - 1.0).abs() < 1e-4);

        let (nx, ny, nz) = triangle_normal([0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
        assert!(ny.abs() < 1e-4);
        assert!(nx.abs() > 0.5 || nz.abs() > 0.5);
    }

    #[test]
    fn ray_triangle_hits_and_misses() {
        let t = ray_triangle(
            0.2, 2.0, 0.2, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        );
        assert!(t.is_some());

        let t = ray_triangle(
            5.0, 1.0, 5.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        );
        assert!(t.is_none());
    }

    #[test]
    fn box_tri_overlap_simple() {
        let hit = box_tri_overlap(
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            0.5,
            0.2,
            0.8,
            -1.0,
            1.0,
            0.2,
            0.8,
            0.96,
            0.96,
        );
        assert!(hit);

        let miss = box_tri_overlap(
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            0.5,
            0.2,
            0.8,
            10.0,
            12.0,
            0.2,
            0.8,
            0.96,
            0.96,
        );
        assert!(!miss);
    }

    #[test]
    fn sweep_foot_lands_on_floor() {
        let mut c = StageMeshCollider::default();

        c.positions.extend_from_slice(&[
            [-2.0, 0.0, -2.0],
            [-2.0, 0.0, 2.0],
            [2.0, 0.0, 2.0],
            [2.0, 0.0, -2.0],
        ]);
        c.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        for tri in [0u32, 1] {
            c.floor_tris.push(tri);
            c.tri_owner.push(-1);
        }
        let foot = c.sweep_foot_segment(0.0, 2.0, 0.0, -1.0);
        assert!(foot.is_some());
        assert!((foot.unwrap().y - 0.0).abs() < 1e-4);
        assert!((foot.unwrap().ny - 1.0).abs() < 1e-4);
    }

    #[test]
    fn reversed_winding_surface_still_participates_in_foot_and_body_queries() {
        let mut c = StageMeshCollider::default();

        c.positions
            .extend_from_slice(&[[-2.0, 1.5, -2.0], [2.0, 1.5, -2.0], [-2.0, 1.5, 2.0]]);
        c.indices.extend_from_slice(&[0, 1, 2]);
        c.ceiling_tris.push(0);
        c.tri_owner.push(-1);

        let foot = c
            .sweep_foot_segment(0.0, 2.0, 0.0, 1.0)
            .expect("double-sided foot sweep sees reversed winding");
        assert!((foot.y - 1.5).abs() < 1e-5);
        assert!(c.box_blocked(0.0, 0.0, 0.0));
    }

    #[test]
    fn ground_query_bridges_only_hairline_support_seams() {
        let mut c = StageMeshCollider::default();
        c.positions
            .extend_from_slice(&[[-2.0, 0.0, -2.0], [-2.0, 0.0, 2.0], [0.0, 0.0, 0.0]]);
        c.indices.extend_from_slice(&[0, 1, 2]);
        c.floor_tris.push(0);
        c.tri_owner.push(-1);

        let seam = c
            .sample_ground_hit(0.10, 0.0, -20.0, 0.0)
            .expect("0.10-wide seam is narrower than rescue radius");
        assert!((seam.y - 0.0).abs() < 1e-5);
        assert!(
            c.sample_ground_hit(0.20, 0.0, -20.0, 0.0).is_none(),
            "real ledge beyond 0.15 must remain a fall"
        );
    }

    #[test]
    fn ground_query_reaches_lower_decks_beyond_short_foot_probe() {
        let mut c = StageMeshCollider::default();
        c.positions.extend_from_slice(&[
            [-2.0, -10.0, -2.0],
            [-2.0, -10.0, 2.0],
            [2.0, -10.0, 0.0],
        ]);
        c.indices.extend_from_slice(&[0, 1, 2]);
        c.floor_tris.push(0);
        c.tri_owner.push(-1);
        let hit = c
            .sample_ground_hit(0.0, 0.0, -20.0, 0.0)
            .expect("48-unit support ray reaches lower deck");
        assert!((hit.y + 10.0).abs() < 1e-5);
    }

    #[test]
    fn box_blocked_wall() {
        let mut c = StageMeshCollider::default();

        c.positions
            .extend_from_slice(&[[0.0, -2.0, -2.0], [0.0, 2.0, -2.0], [0.0, 2.0, 2.0]]);
        c.indices.extend_from_slice(&[0, 1, 2]);
        c.wall_tris.push(0);
        c.tri_owner.push(-1);

        assert!(c.box_blocked(0.3, 0.0, 0.0));

        assert!(!c.box_blocked(5.0, 0.0, 0.0));
    }

    #[test]
    fn dynamic_prop_owner_cannot_disable_static_map_triangles() {
        let mut c = StageMeshCollider::default();
        c.positions
            .extend_from_slice(&[[0.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]]);
        c.indices.extend_from_slice(&[0, 1, 2]);
        c.wall_tris.push(0);
        c.tri_owner.push(-1);

        let owner = c.append_world_triangles(
            &[[4.0, 0.0, 0.0], [4.0, 2.0, 0.0], [4.0, 0.0, 2.0]],
            &[0, 1, 2],
        );
        let prop_tri = c.triangle_count() as u32 - 1;
        assert_eq!(owner, 0);
        c.set_owner_enabled(owner, false);
        assert!(c.enabled(0), "static map triangle must remain enabled");
        assert!(!c.enabled(prop_tri), "only the dynamic prop is disabled");
    }

    #[test]
    fn multipart_owner_updates_only_the_requested_vertex_range() {
        let mut c = StageMeshCollider::default();
        let owner = c.append_world_triangles(
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [10.0, 0.0, 0.0],
                [11.0, 0.0, 0.0],
                [10.0, 1.0, 0.0],
            ],
            &[0, 1, 2, 3, 4, 5],
        );
        assert!(c.update_owner_vertices_range(
            owner,
            3,
            &[[20.0, 0.0, 0.0], [21.0, 0.0, 0.0], [20.0, 1.0, 0.0],],
        ));
        assert_eq!(c.positions[0], [0.0, 0.0, 0.0]);
        assert_eq!(c.positions[3], [20.0, 0.0, 0.0]);
        assert!(!c.update_owner_vertices_range(owner, 5, &[[0.0; 3]; 2]));
    }

    #[test]
    fn origin_res_builds_sn01_collider() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("sn01/sn01.gmb").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let gmb = std::fs::read(root.join("sn01/sn01.gmb")).expect("sn01.gmb");
        let gmdl = meteor_asset::gmdl::decode_gmdl(&gmb).expect("decode gmb");
        let des_text = std::fs::read_to_string(root.join("sn01/sn01.des")).expect("sn01.des");
        let desc = meteor_asset::des::parse_stage_description(&des_text);
        let cob = std::fs::read(root.join("sn01/sn01.cob")).expect("sn01.cob");
        let grid = meteor_asset::cob::parse_cob(&cob).expect("decode cob");
        let collider = StageMeshCollider::from_gmdl(
            &gmdl,
            &desc,
            Some(&grid.face_ids),
            meteor_scene::stage_loader::get_stage_center("sn01", None),
        );
        assert!(
            collider.triangle_count() > 1000,
            "sn01 has meaningful collision"
        );
        assert!(
            collider.floor_tris.len() > collider.ceiling_tris.len(),
            "floors outnumber ceilings: floor={} ceiling={}",
            collider.floor_tris.len(),
            collider.ceiling_tris.len()
        );
        assert!(!collider.wall_tris.is_empty(), "walls exist");
        assert!(
            collider.tri_owner.iter().all(|owner| *owner == -1),
            "all static sn01 triangles stay outside the dynamic prop owner namespace"
        );

        let foot = collider.sweep_foot_segment(0.0, 3.0, 0.0, -3.0);
        assert!(foot.is_some(), "floor probe hits at center");

        let spawn_foot = collider.sweep_foot_segment(120.29, 24.0, 0.164, 12.0);
        assert!(spawn_foot.is_some(), "spawn xz must have floor");

        assert!(
            !collider.positions.is_empty(),
            "static mesh should retain floor/wall tris"
        );
    }
}
