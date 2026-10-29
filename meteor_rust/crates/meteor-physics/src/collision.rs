pub const WALKABLE_SURFACE_NY: f32 = 0.42;

pub const BODY_BOX_HALF_XY: f32 = 0.96;

pub const BODY_BOX_Z_LOW: f32 = 1.2;

pub const BODY_BOX_Z_HIGH: f32 = 3.84;

pub const FOOT_PROBE_DOWN: f32 = 0.24;

pub const FOOT_PROBE_UP: f32 = 1.2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Point3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    pub min: Point3,
    pub max: Point3,
}

#[derive(Debug, Clone, Copy)]
pub struct GroundHit {
    pub y: f32,
    pub nx: f32,
    pub ny: f32,
    pub nz: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct FootHit {
    pub y: f32,
    pub nx: f32,
    pub ny: f32,
    pub nz: f32,
}

pub fn descent_has_support(hit: Option<FootHit>, blocked: bool) -> bool {
    hit.map_or(blocked, |hit| hit.ny >= WALKABLE_SURFACE_NY)
}

pub fn descent_slide_velocity(hit: Option<FootHit>, vertical_speed: f32, dt: f32) -> Point3 {
    let Some(hit) = hit.filter(|hit| hit.ny < WALKABLE_SURFACE_NY) else {
        return Point3::ZERO;
    };
    let k = 10.0 * meteor_core::constants::MODEL_SCALE
        - 2.0 * (vertical_speed + crate::jump::GRAVITY * dt.max(0.0)) * hit.ny;
    Point3::new(k * hit.nx, 0.0, k * hit.nz)
}

pub trait MeshCollider {
    fn sweep_foot_segment(&self, x: f32, y_top: f32, z: f32, y_bottom: f32) -> Option<FootHit>;

    fn box_blocked(&self, cx: f32, cy: f32, cz: f32) -> bool;

    fn body_contact_surface(&self, _cx: f32, _cy: f32, _cz: f32) -> Option<FootHit> {
        None
    }

    fn raycast_hit(&self, origin: Point3, direction: Point3, max_dist: f32) -> Option<RayHit>;

    fn segment_hit(&self, start: Point3, end: Point3) -> Option<RayHit> {
        let d = Point3::new(end.x - start.x, end.y - start.y, end.z - start.z);
        let length = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
        self.raycast_hit(start, d, length)
    }

    fn sample_ground_hit(&self, x: f32, z: f32, fallback: f32, hint_y: f32) -> Option<GroundHit> {
        let start_y = hint_y + 6.0;
        let max_drop = 48.0_f32.max(start_y - (fallback - 24.0));
        self.raycast_hit(
            Point3::new(x, start_y, z),
            Point3::new(0.0, -1.0, 0.0),
            max_drop,
        )
        .filter(|hit| hit.y <= hint_y + 1.5)
        .map(|hit| GroundHit {
            y: hit.y,
            nx: hit.nx,
            ny: hit.ny,
            nz: hit.nz,
        })
    }
}

impl<T: MeshCollider + ?Sized> MeshCollider for &T {
    fn segment_hit(&self, start: Point3, end: Point3) -> Option<RayHit> {
        (**self).segment_hit(start, end)
    }
    fn sweep_foot_segment(&self, x: f32, y_top: f32, z: f32, y_bottom: f32) -> Option<FootHit> {
        (**self).sweep_foot_segment(x, y_top, z, y_bottom)
    }
    fn box_blocked(&self, cx: f32, cy: f32, cz: f32) -> bool {
        (**self).box_blocked(cx, cy, cz)
    }
    fn body_contact_surface(&self, cx: f32, cy: f32, cz: f32) -> Option<FootHit> {
        (**self).body_contact_surface(cx, cy, cz)
    }
    fn raycast_hit(&self, origin: Point3, direction: Point3, max_dist: f32) -> Option<RayHit> {
        (**self).raycast_hit(origin, direction, max_dist)
    }
    fn sample_ground_hit(&self, x: f32, z: f32, fallback: f32, hint_y: f32) -> Option<GroundHit> {
        (**self).sample_ground_hit(x, z, fallback, hint_y)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RayHit {
    pub t: f32,
    pub y: f32,
    pub nx: f32,
    pub ny: f32,
    pub nz: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct WallMove {
    pub position: Point3,
    pub velocity: Point3,
    pub flags: u32,
    pub vertical_surface: Option<FootHit>,
    pub vertical_speed_before_contact: f32,
}

#[derive(Debug, Clone)]
pub struct CollisionSystem<M: MeshCollider> {
    pub boxes: Vec<Aabb>,
    pub ground_y: f32,
    pub radius: f32,
    pub height: f32,
    pub mesh: Option<M>,
}

impl<M: MeshCollider> CollisionSystem<M> {
    pub fn resolve_wall_motion(
        &self,
        pos: Point3,
        velocity: Point3,
        probe: Point3,
        dt: f32,
    ) -> WallMove {
        let mut p = [pos.x, pos.y, pos.z];
        let mut v = [velocity.x, velocity.y, velocity.z];
        let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if speed > 72.0 {
            for component in &mut v {
                *component *= 72.0 / speed;
            }
        }
        let mut result = WallMove {
            position: pos,
            velocity,
            flags: 0,
            vertical_surface: None,
            vertical_speed_before_contact: velocity.y,
        };
        if self.blocks_motion_sweep(pos, Point3::new(v[0] * dt, v[1] * dt, v[2] * dt)) {
            result.velocity = Point3::ZERO;
            result.flags = 0x15;
            return result;
        }
        let mut normal = FootHit {
            y: pos.y,
            nx: 0.0,
            ny: 1.0,
            nz: 0.0,
        };
        let mut swept = false;
        for (source_axis, axis) in [0usize, 2, 1].into_iter().enumerate() {
            if v[axis] == 0.0 {
                continue;
            }
            if axis == 1 {
                result.vertical_speed_before_contact = v[axis];
            }
            let mut candidate = p;
            candidate[axis] += v[axis] * dt;
            let q = Point3::new(
                candidate[0] + probe.x,
                candidate[1] + probe.y,
                candidate[2] + probe.z,
            );
            let end = Point3::new(candidate[0], candidate[1], candidate[2]);
            let mut flags = 0;
            if let Some(hit) = self.segment_hit(q, end) {
                let length = (probe.x * probe.x + probe.y * probe.y + probe.z * probe.z).sqrt();
                if length > 0.0 {
                    candidate[0] -= probe.x * hit.t / length;
                    candidate[1] -= probe.y * hit.t / length;
                    candidate[2] -= probe.z * hit.t / length;
                    normal = FootHit {
                        y: candidate[1],
                        nx: hit.nx,
                        ny: hit.ny,
                        nz: hit.nz,
                    };
                    flags |= 2;
                    swept = true;
                }
            }
            if self.blocked(candidate[0], candidate[1], candidate[2]) {
                flags |= 1;
                v[axis] = 0.0;
                if let Some(hit) = self
                    .mesh
                    .as_ref()
                    .and_then(|m| m.body_contact_surface(candidate[0], candidate[1], candidate[2]))
                {
                    normal = hit;
                }
            } else {
                p = candidate;
            }
            result.flags |= flags << (source_axis * 2);
            if axis == 1 && flags != 0 {
                result.vertical_surface = Some(normal);
            }
        }

        let length = (probe.x * probe.x + probe.y * probe.y + probe.z * probe.z).sqrt();
        if swept && length > 0.0 {
            let n = [probe.x / length, probe.y / length, probe.z / length];
            let dot = v[0] * n[0] + v[1] * n[1] + v[2] * n[2];
            if dot > 0.0 {
                for i in 0..3 {
                    v[i] -= n[i] * dot;
                }
            }
        }
        result.position = Point3::new(p[0], p[1], p[2]);
        result.velocity = Point3::new(v[0], v[1], v[2]);
        result
    }

    pub fn blocks_motion_sweep(&self, pos: Point3, delta: Point3) -> bool {
        let length = (delta.x * delta.x + delta.y * delta.y + delta.z * delta.z).sqrt();
        if length <= 6.0 * meteor_core::MODEL_SCALE {
            return false;
        }
        [[0.48, 1.2, -0.48], [-0.48, 2.16, 0.48]]
            .into_iter()
            .any(|offset| {
                let start = Point3::new(pos.x + offset[0], pos.y + offset[1], pos.z + offset[2]);
                let end = Point3::new(start.x + delta.x, start.y + delta.y, start.z + delta.z);
                self.segment_hit(start, end).is_some()
            })
    }

    pub fn segment_hit(&self, start: Point3, end: Point3) -> Option<RayHit> {
        let mut best = self
            .mesh
            .as_ref()
            .and_then(|mesh| mesh.segment_hit(start, end));
        let d = Point3::new(end.x - start.x, end.y - start.y, end.z - start.z);
        let length = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
        if length <= 1e-8 {
            return None;
        }
        let direction = Point3::new(d.x / length, d.y / length, d.z / length);
        for box_ in &self.boxes {
            if let Some(hit) = segment_box_hit(start, direction, box_, length) {
                if best.is_none_or(|old| hit.t < old.t) {
                    best = Some(hit);
                }
            }
        }
        best
    }
    pub fn new(radius: f32, height: f32) -> Self {
        Self {
            boxes: Vec::new(),
            ground_y: 0.0,
            radius,
            height,
            mesh: None,
        }
    }

    pub fn set_mesh_collider(&mut self, mesh: M) {
        self.mesh = Some(mesh);
    }

    pub fn add_box(&mut self, min: [f32; 3], max: [f32; 3]) {
        self.boxes.push(Aabb {
            min: Point3::new(min[0], min[1], min[2]),
            max: Point3::new(max[0], max[1], max[2]),
        });
    }

    pub fn set_ground(&mut self, y: f32) {
        self.ground_y = y;
    }

    pub fn sample_ground_y(&self, _x: f32, _z: f32) -> f32 {
        if let Some(mesh) = &self.mesh {
            let _ = mesh;
        }
        self.ground_y
    }

    pub fn sample_ground_hit(&self, x: f32, z: f32, hint_y: f32) -> Option<GroundHit> {
        if let Some(mesh) = &self.mesh {
            return mesh.sample_ground_hit(x, z, self.ground_y, hint_y);
        }
        Some(GroundHit {
            y: self.ground_y,
            nx: 0.0,
            ny: 1.0,
            nz: 0.0,
        })
    }

    pub fn can_occupy(&self, pos: Point3) -> bool {
        !self.blocked(pos.x, pos.y, pos.z)
    }

    fn blocked(&self, cx: f32, cy: f32, cz: f32) -> bool {
        for box_ in &self.boxes {
            let y_min = cy + BODY_BOX_Z_LOW;
            let y_max = cy + BODY_BOX_Z_HIGH;
            if y_max < box_.min.y || y_min > box_.max.y {
                continue;
            }
            let x_min = cx - BODY_BOX_HALF_XY;
            let x_max = cx + BODY_BOX_HALF_XY;
            let z_min = cz - BODY_BOX_HALF_XY;
            let z_max = cz + BODY_BOX_HALF_XY;
            if x_max < box_.min.x || x_min > box_.max.x {
                continue;
            }
            if z_max < box_.min.z || z_min > box_.max.z {
                continue;
            }
            return true;
        }
        if let Some(mesh) = &self.mesh {
            return mesh.box_blocked(cx, cy, cz);
        }
        false
    }

    pub fn resolve_xz(&self, pos: Point3, dx: f32, dz: f32, no_probe: bool) -> (f32, f32, f32) {
        let (x, z, y, _) = self.resolve_xz_with_surface(pos, dx, dz, no_probe);
        (x, z, y)
    }

    pub fn resolve_xz_with_surface(
        &self,
        pos: Point3,
        dx: f32,
        dz: f32,
        no_probe: bool,
    ) -> (f32, f32, f32, Option<FootHit>) {
        let mut surface = None;

        let dist = (dx * dx + dz * dz).sqrt();
        let max_step = self.radius * 0.5;
        let steps = if dist > max_step {
            (dist / max_step).ceil() as usize
        } else {
            1
        };
        let sx = dx / steps as f32;
        let sz = dz / steps as f32;

        let mut x = pos.x;
        let mut z = pos.z;
        let mut y = pos.y;

        for _ in 0..steps {
            if sx != 0.0 {
                let x_new = x + sx;
                let y_after = if !no_probe {
                    self.foot_y_at(x_new, y, z, &mut surface)
                } else {
                    y
                };
                if !self.blocked(x_new, y_after, z) {
                    x = x_new;
                    y = y_after;
                }
            }

            if sz != 0.0 {
                let z_new = z + sz;
                let y_after = if !no_probe {
                    self.foot_y_at(x, y, z_new, &mut surface)
                } else {
                    y
                };
                if !self.blocked(x, y_after, z_new) {
                    z = z_new;
                    y = y_after;
                }
            }
        }

        (x - pos.x, z - pos.z, y - pos.y, surface)
    }

    fn foot_y_at(&self, x: f32, y: f32, z: f32, surface: &mut Option<FootHit>) -> f32 {
        if let Some(mesh) = &self.mesh {
            if let Some(foot) =
                mesh.sweep_foot_segment(x, y + FOOT_PROBE_UP, z, y - FOOT_PROBE_DOWN)
            {
                *surface = Some(foot);
                return foot.y;
            }
        }
        y
    }

    pub fn resolve_ascent_with_surface(
        &self,
        pos: Point3,
        delta_y: f32,
    ) -> (f32, Option<FootHit>, bool) {
        if delta_y > 0.0 && delta_y.is_finite() {
            if let Some(mesh) = &self.mesh {
                let candidate = pos.y + delta_y;
                if let Some(foot) = mesh.sweep_foot_segment(
                    pos.x,
                    candidate + FOOT_PROBE_UP,
                    pos.z,
                    candidate - FOOT_PROBE_DOWN,
                ) {
                    let blocked = self.blocked(pos.x, foot.y, pos.z);
                    return (
                        if blocked { 0.0 } else { foot.y - pos.y },
                        Some(foot),
                        blocked,
                    );
                }
            }
        }
        let (dy, hit, blocked) = self.resolve_y(pos, delta_y);
        (
            dy,
            hit.map(|hit| FootHit {
                y: hit.y,
                nx: hit.nx,
                ny: hit.ny,
                nz: hit.nz,
            }),
            blocked,
        )
    }

    pub fn resolve_y(&self, pos: Point3, delta_y: f32) -> (f32, Option<RayHit>, bool) {
        let requested = if delta_y.is_finite() { delta_y } else { 0.0 };
        if requested <= 0.0 {
            return (requested, None, false);
        }

        let head_y = pos.y + self.height;
        let mut accepted = requested;
        let mut best_hit = None;

        if let Some(mesh) = &self.mesh {
            if let Some(hit) = mesh.raycast_hit(
                Point3::new(pos.x, head_y, pos.z),
                Point3::new(0.0, 1.0, 0.0),
                requested,
            ) {
                accepted = accepted.min((hit.y - head_y).max(0.0));
                best_hit = Some(hit);
            }
        }

        for box_ in &self.boxes {
            if !circle_overlaps_box_xz(pos.x, pos.z, self.radius, box_) {
                continue;
            }
            let underside = box_.min.y;
            if underside < head_y - 1e-6 || underside > head_y + requested + 1e-6 {
                continue;
            }
            let room = (underside - head_y).max(0.0);
            if room <= accepted {
                accepted = room;
                best_hit = Some(RayHit {
                    t: room,
                    y: underside,
                    nx: 0.0,
                    ny: -1.0,
                    nz: 0.0,
                });
            }
        }

        let mut blocked = false;
        if accepted > 0.0 && !self.blocked(pos.x, pos.y, pos.z) {
            let steps = ((accepted / 1.0).ceil() as usize).max(1);
            let step = accepted / steps as f32;
            let mut clear = 0.0;
            for i in 1..=steps {
                let cand = pos.y + step * i as f32;
                if self.blocked(pos.x, cand, pos.z) {
                    blocked = true;
                    break;
                }
                clear = step * i as f32;
            }
            if blocked {
                accepted = clear;
            }
        }

        (accepted, best_hit, blocked)
    }

    pub fn resolve_descent(&self, pos: Point3, delta_y: f32) -> (f32, Option<FootHit>, bool) {
        self.resolve_descent_with_surface(pos, delta_y, None)
    }

    pub fn resolve_descent_with_surface(
        &self,
        pos: Point3,
        delta_y: f32,
        planar_surface: Option<FootHit>,
    ) -> (f32, Option<FootHit>, bool) {
        let requested = if delta_y.is_finite() { delta_y } else { 0.0 };
        if requested >= 0.0 {
            return (requested, None, false);
        }
        if self.mesh.is_none() {
            return (requested, None, false);
        }

        let mesh = self.mesh.as_ref().unwrap();
        let mut y = pos.y;
        let target = pos.y + requested;
        let steps = ((y - target) / 1.0).ceil().max(1.0) as usize;
        let step = (y - target) / steps as f32;

        for _ in 0..steps {
            let cand = y - step;
            let foot =
                mesh.sweep_foot_segment(pos.x, cand + FOOT_PROBE_UP, pos.z, cand - FOOT_PROBE_DOWN);
            if let Some(foot) = foot {
                if !self.blocked(pos.x, foot.y, pos.z) {
                    return (foot.y - pos.y, Some(foot), false);
                }

                return (y - pos.y, Some(foot), true);
            }
            if self.blocked(pos.x, cand, pos.z) {
                return (
                    y - pos.y,
                    planar_surface.or_else(|| mesh.body_contact_surface(pos.x, cand, pos.z)),
                    true,
                );
            }
            y = cand;
        }
        (y - pos.y, None, false)
    }

    pub fn circle_overlaps_box_xz(&self, x: f32, z: f32, box_: &Aabb) -> bool {
        circle_overlaps_box_xz(x, z, self.radius, box_)
    }

    pub fn raycast_hit(&self, origin: Point3, direction: Point3, max_dist: f32) -> Option<RayHit> {
        let mut best: Option<RayHit> = None;
        if let Some(mesh) = &self.mesh {
            if let Some(hit) = mesh.raycast_hit(origin, direction, max_dist) {
                best = Some(hit);
            }
        }
        for box_ in &self.boxes {
            if let Some(hit) = ray_box_hit(origin, direction, box_, max_dist) {
                if best.is_none() || hit.t < best.unwrap().t {
                    best = Some(hit);
                }
            }
        }
        best
    }
}

fn segment_box_hit(start: Point3, direction: Point3, box_: &Aabb, length: f32) -> Option<RayHit> {
    let p = [start.x, start.y, start.z];
    let d = [direction.x, direction.y, direction.z];
    let lo = [box_.min.x, box_.min.y, box_.min.z];
    let hi = [box_.max.x, box_.max.y, box_.max.z];
    let mut best: Option<RayHit> = None;
    for axis in 0..3 {
        if d[axis].abs() < 1e-8 {
            continue;
        }
        for (plane, sign) in [(lo[axis], -1.0), (hi[axis], 1.0)] {
            let t = (plane - p[axis]) / d[axis];
            if t < 0.0 || t > length || best.is_some_and(|h| t >= h.t) {
                continue;
            }
            if (0..3).any(|i| i != axis && (p[i] + d[i] * t < lo[i] || p[i] + d[i] * t > hi[i])) {
                continue;
            }
            let mut n = [0.0; 3];
            n[axis] = sign;
            best = Some(RayHit {
                t,
                y: start.y + direction.y * t,
                nx: n[0],
                ny: n[1],
                nz: n[2],
            });
        }
    }
    best
}

pub fn circle_overlaps_box_xz(x: f32, z: f32, radius: f32, box_: &Aabb) -> bool {
    let closest_x = box_.min.x.max(x.min(box_.max.x));
    let closest_z = box_.min.z.max(z.min(box_.max.z));
    let dx = x - closest_x;
    let dz = z - closest_z;
    dx * dx + dz * dz <= radius * radius
}

pub fn ray_box_hit(
    origin: Point3,
    direction: Point3,
    box_: &Aabb,
    max_dist: f32,
) -> Option<RayHit> {
    let ox = origin.x;
    let oy = origin.y;
    let oz = origin.z;
    let dx = direction.x;
    let dy = direction.y;
    let dz = direction.z;
    let mut tmin = 0.0f32;
    let mut tmax = max_dist;
    let mut near_normal = (0.0f32, 0.0f32, 0.0f32);

    if let Some((t1, t2, n1)) = slab_interval(
        box_.min.x,
        box_.max.x,
        ox,
        dx,
        (-1.0, 0.0, 0.0),
        (1.0, 0.0, 0.0),
    ) {
        if t1 > tmin {
            tmin = t1;
            near_normal = n1;
        }
        tmax = tmax.min(t2);
        if tmin > tmax {
            return None;
        }
    } else {
        return None;
    }

    if let Some((t1, t2, n1)) = slab_interval(
        box_.min.y,
        box_.max.y,
        oy,
        dy,
        (0.0, -1.0, 0.0),
        (0.0, 1.0, 0.0),
    ) {
        if t1 > tmin {
            tmin = t1;
            near_normal = n1;
        }
        tmax = tmax.min(t2);
        if tmin > tmax {
            return None;
        }
    } else {
        return None;
    }

    if let Some((t1, t2, n1)) = slab_interval(
        box_.min.z,
        box_.max.z,
        oz,
        dz,
        (0.0, 0.0, -1.0),
        (0.0, 0.0, 1.0),
    ) {
        if t1 > tmin {
            tmin = t1;
            near_normal = n1;
        }
        tmax = tmax.min(t2);
        if tmin > tmax {
            return None;
        }
    } else {
        return None;
    }

    let t = if tmin >= 0.0 { tmin } else { tmax };
    if t < 0.0 || t > max_dist {
        return None;
    }
    Some(RayHit {
        t,
        y: oy + dy * t,
        nx: near_normal.0,
        ny: near_normal.1,
        nz: near_normal.2,
    })
}

fn slab_interval(
    bmin: f32,
    bmax: f32,
    o: f32,
    d: f32,
    min_normal: (f32, f32, f32),
    max_normal: (f32, f32, f32),
) -> Option<(f32, f32, (f32, f32, f32))> {
    if d.abs() < 1e-8 {
        if o < bmin || o > bmax {
            return None;
        }
        return Some((f32::NEG_INFINITY, f32::INFINITY, (0.0, 0.0, 0.0)));
    }
    let mut t1 = (bmin - o) / d;
    let mut t2 = (bmax - o) / d;
    let mut n1 = min_normal;
    if t1 > t2 {
        std::mem::swap(&mut t1, &mut t2);
        n1 = max_normal;
    }
    Some((t1, t2, n1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn borrowed_collider_delegates_every_override_without_clone() {
        struct Overrides;
        impl MeshCollider for Overrides {
            fn sweep_foot_segment(&self, _: f32, _: f32, _: f32, _: f32) -> Option<FootHit> {
                Some(FootHit {
                    y: 1.0,
                    nx: 0.0,
                    ny: 1.0,
                    nz: 0.0,
                })
            }
            fn box_blocked(&self, _: f32, _: f32, _: f32) -> bool {
                true
            }
            fn body_contact_surface(&self, _: f32, _: f32, _: f32) -> Option<FootHit> {
                Some(FootHit {
                    y: 2.0,
                    nx: 1.0,
                    ny: 0.0,
                    nz: 0.0,
                })
            }
            fn raycast_hit(&self, _: Point3, _: Point3, _: f32) -> Option<RayHit> {
                Some(RayHit {
                    t: 3.0,
                    y: 3.0,
                    nx: 0.0,
                    ny: 1.0,
                    nz: 0.0,
                })
            }
            fn sample_ground_hit(&self, _: f32, _: f32, _: f32, _: f32) -> Option<GroundHit> {
                Some(GroundHit {
                    y: 4.0,
                    nx: 0.0,
                    ny: 1.0,
                    nz: 0.0,
                })
            }
        }
        let mesh = Overrides;
        let borrowed: &dyn MeshCollider = &mesh;
        assert_eq!(
            MeshCollider::sweep_foot_segment(&borrowed, 0.0, 0.0, 0.0, 0.0)
                .unwrap()
                .y,
            1.0
        );
        assert!(MeshCollider::box_blocked(&borrowed, 0.0, 0.0, 0.0));
        assert_eq!(
            MeshCollider::body_contact_surface(&borrowed, 0.0, 0.0, 0.0)
                .unwrap()
                .y,
            2.0
        );
        assert_eq!(
            MeshCollider::raycast_hit(&borrowed, Point3::ZERO, Point3::ZERO, 0.0)
                .unwrap()
                .t,
            3.0
        );
        assert_eq!(
            MeshCollider::sample_ground_hit(&borrowed, 0.0, 0.0, 0.0, 0.0)
                .unwrap()
                .y,
            4.0
        );
        let mut system = CollisionSystem::new(1.0, 4.0);
        system.set_mesh_collider(borrowed);
        assert_eq!(system.sample_ground_hit(0.0, 0.0, 0.0).unwrap().y, 4.0);
    }

    #[test]
    fn missing_mesh_support_is_not_the_no_mesh_test_plane() {
        let mut system = CollisionSystem::<NullMesh>::new(0.55, 1.8);
        system.set_ground(-3.0);
        assert_eq!(system.sample_ground_hit(0.0, 0.0, 0.0).unwrap().y, -3.0);
        system.set_mesh_collider(NullMesh);
        assert!(system.sample_ground_hit(0.0, 0.0, 0.0).is_none());
    }

    struct NullMesh;
    impl MeshCollider for NullMesh {
        fn sweep_foot_segment(&self, _x: f32, _yt: f32, _z: f32, _yb: f32) -> Option<FootHit> {
            None
        }
        fn box_blocked(&self, _cx: f32, _cy: f32, _cz: f32) -> bool {
            false
        }
        fn raycast_hit(
            &self,
            _origin: Point3,
            _direction: Point3,
            _max_dist: f32,
        ) -> Option<RayHit> {
            None
        }
    }

    struct FlatMesh;
    impl MeshCollider for FlatMesh {
        fn sweep_foot_segment(&self, _x: f32, yt: f32, _z: f32, yb: f32) -> Option<FootHit> {
            if yt >= 0.0 && yb <= 0.0 {
                Some(FootHit {
                    y: 0.0,
                    nx: 0.0,
                    ny: 1.0,
                    nz: 0.0,
                })
            } else {
                None
            }
        }
        fn box_blocked(&self, _cx: f32, _cy: f32, _cz: f32) -> bool {
            false
        }
        fn raycast_hit(
            &self,
            _origin: Point3,
            _direction: Point3,
            _max_dist: f32,
        ) -> Option<RayHit> {
            None
        }
    }

    #[test]
    fn blocked_body_box() {
        let mut sys: CollisionSystem<NullMesh> = CollisionSystem::new(0.55, 1.8);
        sys.add_box([-2.0, 0.0, -2.0], [2.0, 5.0, 2.0]);

        assert!(sys.blocked(0.0, 2.0, 0.0));

        assert!(!sys.blocked(10.0, 2.0, 10.0));

        sys.boxes.clear();
        sys.add_box([-2.0, 0.0, -2.0], [2.0, 0.5, 2.0]);
        assert!(!sys.blocked(0.0, 0.5, 0.0));

        sys.boxes.clear();
        sys.add_box([-2.0, 0.0, -2.0], [2.0, 5.0, 2.0]);
        assert!(sys.blocked(0.0, 0.5, 0.0));
    }

    #[test]
    fn resolve_xz_rejects_blocked_axis() {
        let mut sys: CollisionSystem<NullMesh> = CollisionSystem::new(0.55, 1.8);

        sys.add_box([1.5, 0.0, -10.0], [2.5, 5.0, 10.0]);
        let pos = Point3::new(0.0, 2.0, 0.0);

        let (dx, _, _) = sys.resolve_xz(pos, 2.0, 0.0, false);
        assert!(dx <= 0.6);

        let (_, dz, _) = sys.resolve_xz(pos, 0.0, 2.0, false);
        assert!((dz - 2.0).abs() < 1e-5);
    }

    #[test]
    fn ascending_ordinary_probe_reports_floor_contact_before_wall_entry() {
        let mut sys = CollisionSystem::new(0.55, 1.8);
        sys.set_mesh_collider(FlatMesh);
        let (dy, hit, blocked) = sys.resolve_ascent_with_surface(Point3::ZERO, 0.2);
        assert_eq!(dy, 0.0);
        assert_eq!(hit.unwrap().ny, 1.0);
        assert!(!blocked);
        let (dy, hit, blocked) = sys.resolve_ascent_with_surface(Point3::ZERO, 0.3);
        assert_eq!(dy, 0.3);
        assert!(hit.is_none());
        assert!(!blocked);
        sys.add_box([-2.0, 1.0, -2.0], [2.0, 5.0, 2.0]);
        let (dy, hit, blocked) = sys.resolve_ascent_with_surface(Point3::ZERO, 0.2);
        assert_eq!(dy, 0.0);
        assert!(hit.is_some(), "body rejection must retain the swept normal");
        assert!(blocked);
    }

    #[test]
    fn foot_probe_snaps_to_ground() {
        let mut sys: CollisionSystem<FlatMesh> = CollisionSystem::new(0.55, 1.8);
        sys.set_mesh_collider(FlatMesh);

        let (_, _, dy) = sys.resolve_xz(Point3::new(0.0, 0.2, 0.0), 0.0, 1.0, false);
        assert!((dy - -0.2).abs() < 1e-4);
    }

    #[test]
    fn no_probe_keeps_y() {
        let sys: CollisionSystem<FlatMesh> = CollisionSystem::new(0.55, 1.8);

        let (_, _, dy) = sys.resolve_xz(Point3::new(0.0, 1.0, 0.0), 0.0, 1.0, true);
        assert_eq!(dy, 0.0);
    }

    #[test]
    fn resolve_descent_lands() {
        let mut sys: CollisionSystem<FlatMesh> = CollisionSystem::new(0.55, 1.8);
        sys.set_mesh_collider(FlatMesh);
        let pos = Point3::new(0.0, 1.0, 0.0);

        let (dy, hit, blocked) = sys.resolve_descent(pos, -2.0);
        assert!(!blocked);
        assert!(hit.is_some());
        assert!((dy - -1.0).abs() < 1e-4);
    }

    #[test]
    fn descent_support_uses_signed_normal_even_when_body_is_blocked() {
        for ny in [
            -1.0,
            -std::f32::consts::FRAC_1_SQRT_2,
            -0.235885,
            0.0,
            0.419,
            0.42,
            1.0,
        ] {
            let hit = FootHit {
                y: 0.0,
                nx: 0.0,
                ny,
                nz: 1.0,
            };
            for blocked in [false, true] {
                assert_eq!(descent_has_support(Some(hit), blocked), ny >= 0.42);
            }
        }
        assert!(
            descent_has_support(None, true),
            "bare eave rejection retains support"
        );
        assert!(!descent_has_support(None, false));
    }

    #[test]
    fn descent_body_rejection_preserves_swept_surface() {
        let mut sys = CollisionSystem::new(0.55, 1.8);
        sys.set_mesh_collider(FlatMesh);
        sys.add_box([-2.0, 1.2, -2.0], [2.0, 5.0, 2.0]);
        let (dy, hit, blocked) = sys.resolve_descent(Point3::new(0.0, 1.0, 0.0), -2.0);
        assert!(blocked);
        assert_eq!(dy, 0.0, "position is still rejected");
        assert_eq!(hit.unwrap().ny, 1.0, "bit1 survives bit0 rejection");
    }

    #[test]
    fn body_only_descent_keeps_this_solves_planar_surface() {
        let mut sys: CollisionSystem<NullMesh> = CollisionSystem::new(0.55, 1.8);
        sys.set_mesh_collider(NullMesh);
        sys.add_box([-2.0, 1.2, -2.0], [2.0, 5.0, 2.0]);
        let normal = FootHit {
            y: 0.0,
            nx: -0.5,
            ny: -std::f32::consts::FRAC_1_SQRT_2,
            nz: -0.5,
        };
        let (_, hit, blocked) = sys.resolve_descent_with_surface(Point3::ZERO, -0.1, Some(normal));
        assert!(blocked);
        assert_eq!(hit.unwrap().ny, normal.ny);
        assert!(!descent_has_support(hit, blocked));
        let (_, next, _) = sys.resolve_descent(Point3::ZERO, -0.1);
        assert!(
            next.is_none(),
            "normal must not leak to another solve or actor"
        );
    }

    #[test]
    fn descent_slide_matches_exe_signed_external_velocity() {
        let normal = FootHit {
            y: 0.0,
            nx: -0.5,
            ny: -std::f32::consts::FRAC_1_SQRT_2,
            nz: -0.5,
        };
        let slide = descent_slide_velocity(Some(normal), -12.0, 1.0 / 30.0);
        assert!(slide.x > 10.0 && slide.z == slide.x);
        assert_eq!(slide.y, 0.0);
        assert_eq!(
            descent_slide_velocity(None, -12.0, 1.0 / 30.0),
            Point3::ZERO
        );
        assert_eq!(
            descent_slide_velocity(Some(FootHit { ny: 1.0, ..normal }), -12.0, 1.0 / 30.0),
            Point3::ZERO
        );
    }

    #[test]
    fn resolve_y_ceiling_caps() {
        let mut sys: CollisionSystem<NullMesh> = CollisionSystem::new(0.55, 1.8);

        sys.add_box([-10.0, 3.0, -10.0], [10.0, 5.0, 10.0]);
        let pos = Point3::new(0.0, 0.5, 0.0);
        let (dy, hit, blocked) = sys.resolve_y(pos, 2.0);
        assert!(hit.is_some());

        assert!((dy - 0.7).abs() < 0.01);
        assert!(!blocked);
    }

    struct CeilingMesh;
    impl MeshCollider for CeilingMesh {
        fn sweep_foot_segment(&self, _x: f32, _yt: f32, _z: f32, _yb: f32) -> Option<FootHit> {
            None
        }
        fn box_blocked(&self, _cx: f32, _cy: f32, _cz: f32) -> bool {
            false
        }
        fn raycast_hit(&self, origin: Point3, direction: Point3, max_dist: f32) -> Option<RayHit> {
            assert!((direction.y - 1.0).abs() < 1e-5, "ray must point up");
            assert_eq!(origin.x, 0.0);
            assert_eq!(origin.z, 0.0);
            let hit_y = 4.0;
            let t = hit_y - origin.y;
            (t >= 0.0 && t <= max_dist).then(|| RayHit {
                t,
                y: hit_y,
                nx: 0.0,
                ny: -1.0,
                nz: 0.0,
            })
        }
    }

    #[test]
    fn resolve_y_mesh_ceiling_coordinates() {
        let mut sys: CollisionSystem<CeilingMesh> = CollisionSystem::new(0.55, 1.8);
        sys.set_mesh_collider(CeilingMesh);
        let pos = Point3::new(0.0, 1.0, 0.0);
        let (dy, hit, _) = sys.resolve_y(pos, 3.0);

        assert!(hit.is_some());
        assert!((dy - 1.2).abs() < 1e-4);
    }

    #[test]
    fn ray_box_slab() {
        let box_ = Aabb {
            min: Point3::new(-1.0, -1.0, -1.0),
            max: Point3::new(1.0, 1.0, 1.0),
        };

        let hit = ray_box_hit(
            Point3::new(-5.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            &box_,
            20.0,
        )
        .unwrap();
        assert!((hit.t - 4.0).abs() < 1e-5);
        assert!((hit.nx - -1.0).abs() < 1e-5);

        assert!(ray_box_hit(
            Point3::new(-5.0, 5.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            &box_,
            20.0,
        )
        .is_none());
    }

    #[test]
    fn circle_box_overlap() {
        let box_ = Aabb {
            min: Point3::new(0.0, 0.0, 0.0),
            max: Point3::new(2.0, 2.0, 2.0),
        };

        assert!(circle_overlaps_box_xz(1.0, 1.0, 0.55, &box_));

        assert!(!circle_overlaps_box_xz(10.0, 10.0, 0.55, &box_));

        assert!(circle_overlaps_box_xz(2.4, 1.0, 0.55, &box_));
    }

    #[test]
    fn constants_match_stage() {
        assert_eq!(WALKABLE_SURFACE_NY, 0.42);
        assert_eq!(BODY_BOX_HALF_XY, 0.96);
        assert_eq!(BODY_BOX_Z_LOW, 1.2);
        assert_eq!(BODY_BOX_Z_HIGH, 3.84);
        assert_eq!(FOOT_PROBE_DOWN, 0.24);
        assert_eq!(FOOT_PROBE_UP, 1.2);
    }
}
