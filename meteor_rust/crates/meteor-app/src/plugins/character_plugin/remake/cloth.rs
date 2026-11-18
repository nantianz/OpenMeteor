use crate::plugins::graphics_pack::config::Clothing;
use bevy::prelude::*;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;
pub(super) mod topology;

#[derive(Clone)]
pub(super) struct Edge {
    pub ids: [usize; 2],
    pub rest: f32,
}
#[derive(Clone)]
pub(super) struct Bend {
    pub ids: [usize; 4],
    pub rest: f32,
}
#[derive(Clone, Copy)]
pub(super) struct Capsule {
    pub a: Vec3,
    pub b: Vec3,
    pub radius: f32,
}
#[derive(Default)]
pub(super) struct Simulation {
    pub positions: Vec<Vec3>,
    pub velocities: Vec<Vec3>,
    pub anchors: Vec<Vec3>,
    pub accumulator: f32,
    capsules: Vec<Capsule>,
}
impl Simulation {
    pub fn reset(&mut self, anchors: &[Vec3]) {
        self.positions = anchors.to_vec();
        self.anchors = anchors.to_vec();
        self.velocities = vec![Vec3::ZERO; anchors.len()];
        self.accumulator = 0.0;
        self.capsules.clear();
    }
    pub fn advance(
        &mut self,
        data: &topology::Topology,
        targets: &[Vec3],
        capsules: &[Capsule],
        dt: f32,
        wind: Vec3,
        scale: f32,
        config: &Clothing,
    ) {
        const STEP: f32 = 1.0 / 120.0;
        self.accumulator += dt;
        let steps = ((self.accumulator + 1e-7) / STEP).floor() as usize;
        if steps == 0 {
            return;
        }
        self.accumulator = (self.accumulator - steps as f32 * STEP).max(0.0);
        let from = self.anchors.clone();
        for step in 0..steps.min(24) {
            let t = (step + 1) as f32 / steps as f32;
            let anchors: Vec<_> = from
                .iter()
                .zip(targets)
                .map(|(a, b)| a.lerp(*b, t))
                .collect();
            let contacts: Vec<_> = capsules
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let previous = self.capsules.get(i).unwrap_or(c);
                    Capsule {
                        a: previous.a.lerp(c.a, t),
                        b: previous.b.lerp(c.b, t),
                        radius: c.radius,
                    }
                })
                .collect();
            self.substep(data, &anchors, &contacts, STEP, wind, scale, config);
        }
        self.anchors.copy_from_slice(targets);
        self.capsules = capsules.to_vec();
    }
    fn substep(
        &mut self,
        data: &topology::Topology,
        anchors: &[Vec3],
        capsules: &[Capsule],
        dt: f32,
        wind: Vec3,
        scale: f32,
        config: &Clothing,
    ) {
        let previous = self.positions.clone();
        let inverse_mass: Vec<f32> = data
            .area
            .iter()
            .zip(&data.freedom)
            .map(|(area, free)| {
                if *free == 0.0 {
                    0.0
                } else {
                    1.0 / (area * scale * scale * config.density).max(1e-5)
                }
            })
            .collect();

        let mut forces = vec![Vec3::ZERO; previous.len()];
        for &[a, b, c] in &data.triangles {
            if inverse_mass[a] + inverse_mass[b] + inverse_mass[c] == 0. {
                continue;
            }
            let cross = (previous[b] - previous[a]).cross(previous[c] - previous[a]);
            let area2 = cross.length();
            if area2 < 1e-9 {
                continue;
            }
            let normal = cross / area2;
            let speed = (wind
                - (self.velocities[a] + self.velocities[b] + self.velocities[c]) / 3.0)
                .dot(normal)
                .clamp(-20., 20.);
            let force =
                normal * (0.5 * 1.225 * config.wind_drag * speed * speed.abs() * area2 * 0.5 / 3.0);
            for i in [a, b, c] {
                forces[i] += force;
            }
        }
        for i in 0..previous.len() {
            if inverse_mass[i] == 0.0 {
                self.positions[i] = anchors[i];
                continue;
            }
            let acceleration = Vec3::NEG_Y * (9.81 * config.gravity_scale)
                + (forces[i] * inverse_mass[i]).clamp_length_max(100.);
            self.velocities[i] *= (-config.damping * dt).exp();
            self.positions[i] += self.velocities[i] * dt + acceleration * dt * dt;
        }
        let mut stretch_lambda = vec![0.; data.edges.len()];
        let mut bend_lambda = vec![0.; data.bends.len()];
        for _ in 0..config.iterations {
            for (edge, lambda) in data.edges.iter().zip(&mut stretch_lambda) {
                let [a, b] = edge.ids;
                if inverse_mass[a] + inverse_mass[b] == 0. {
                    continue;
                }
                let delta = self.positions[a] - self.positions[b];
                let length = delta.length();
                if length < 1e-8 {
                    continue;
                }
                let alpha = config.stretch_compliance / (dt * dt);
                let sum = inverse_mass[a] + inverse_mass[b] + alpha;
                if sum <= 0. {
                    continue;
                }
                let dl = (-(length - edge.rest * scale) - alpha * *lambda) / sum;
                *lambda += dl;
                let correction = delta * (dl / length);
                self.positions[a] += correction * inverse_mass[a];
                self.positions[b] -= correction * inverse_mass[b];
            }
            for (bend, lambda) in data.bends.iter().zip(&mut bend_lambda) {
                if bend.ids.iter().all(|i| inverse_mass[*i] == 0.) {
                    continue;
                }
                let p = bend.ids.map(|i| self.positions[i]);
                let Some((angle, grad)) = dihedral(p) else {
                    continue;
                };
                let alpha = config.bend_compliance / (dt * dt);
                let denominator = bend
                    .ids
                    .iter()
                    .zip(grad)
                    .map(|(i, g)| inverse_mass[*i] * g.length_squared())
                    .sum::<f32>()
                    + alpha;
                if denominator < 1e-12 {
                    continue;
                }
                let error = wrap_angle(angle - bend.rest);
                let dl = (-error - alpha * *lambda) / denominator;
                *lambda += dl;
                for (i, g) in bend.ids.iter().zip(grad) {
                    self.positions[*i] += g * (dl * inverse_mass[*i]);
                }
            }
            for i in 0..previous.len() {
                if inverse_mass[i] == 0. {
                    self.positions[i] = anchors[i];
                    continue;
                }

                self.positions[i] = anchors[i]
                    + (self.positions[i] - anchors[i])
                        .clamp_length_max(config.motion_limit * data.freedom[i]);
                for capsule in capsules {
                    let _ = project_capsule(&mut self.positions[i], *capsule, config.thickness);
                }
            }
        }
        if config.self_collision {
            self.collide_self(data, &inverse_mass, config.thickness, scale);
        }
        for i in 0..previous.len() {
            let mut contact = Vec3::ZERO;
            if inverse_mass[i] > 0. {
                for capsule in capsules {
                    if let Some(normal) =
                        project_capsule(&mut self.positions[i], *capsule, config.thickness)
                    {
                        contact += normal;
                    }
                }
            }
            if !self.positions[i].is_finite() {
                self.positions[i] = anchors[i];
            }
            self.velocities[i] = ((self.positions[i] - previous[i]) / dt).clamp_length_max(25.);
            if let Some(normal) = contact.try_normalize() {
                let velocity = self.velocities[i];
                self.velocities[i] -=
                    (velocity - normal * velocity.dot(normal)) * config.collision_friction;
            }
            if inverse_mass[i] == 0. {
                self.velocities[i] = Vec3::ZERO;
            }
        }
    }
    fn collide_self(
        &mut self,
        data: &topology::Topology,
        weights: &[f32],
        thickness: f32,
        scale: f32,
    ) {
        let diameter = thickness * 2.;
        let mut cells = BTreeMap::<[i32; 3], Vec<usize>>::new();
        let cell = |p: Vec3| (p / diameter).floor().as_ivec3().to_array();
        for (i, p) in self.positions.iter().enumerate() {
            cells.entry(cell(*p)).or_default().push(i);
        }
        for i in 0..self.positions.len() {
            let center = cell(self.positions[i]);
            for x in -1..=1 {
                for y in -1..=1 {
                    for z in -1..=1 {
                        let Some(others) =
                            cells.get(&[center[0] + x, center[1] + y, center[2] + z])
                        else {
                            continue;
                        };
                        for &j in others {
                            if j <= i
                                || weights[i] + weights[j] == 0.
                                || data.points[i].distance(data.points[j]) * scale < diameter * 1.5
                            {
                                continue;
                            }
                            let diff = self.positions[i] - self.positions[j];
                            let distance = diff.length();
                            if distance >= diameter {
                                continue;
                            }
                            let normal = if distance > 1e-8 {
                                diff / distance
                            } else {
                                (data.points[i] - data.points[j]).normalize_or_zero()
                            };
                            let delta =
                                normal * ((diameter - distance) / (weights[i] + weights[j]));
                            self.positions[i] += delta * weights[i];
                            self.positions[j] -= delta * weights[j];
                        }
                    }
                }
            }
        }
    }
}
fn wrap_angle(a: f32) -> f32 {
    (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

fn dihedral([a, b, c, d]: [Vec3; 4]) -> Option<(f32, [Vec3; 4])> {
    let e = b - a;
    let l2 = e.length_squared();
    let len = l2.sqrt();
    let n1 = e.cross(c - a);
    let n2 = (d - a).cross(e);
    if l2 < 1e-12 || n1.length_squared() < 1e-14 || n2.length_squared() < 1e-14 {
        return None;
    }
    let u = n1.normalize();
    let v = n2.normalize();
    let angle = (e / len).dot(u.cross(v)).atan2(u.dot(v));
    let gc = -n1 * (len / n1.length_squared());
    let gd = -n2 * (len / n2.length_squared());
    let gb = -gc * ((c - a).dot(e) / l2) - gd * ((d - a).dot(e) / l2);
    Some((angle, [-gb - gc - gd, gb, gc, gd]))
}
fn project_capsule(p: &mut Vec3, c: Capsule, thickness: f32) -> Option<Vec3> {
    let ab = c.b - c.a;
    let t = ((*p - c.a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0., 1.);
    let closest = c.a + ab * t;
    let delta = *p - closest;
    let radius = c.radius + thickness;
    let distance_squared = delta.length_squared();
    if distance_squared <= (radius + 1e-5).powi(2) {
        let normal = if delta.length_squared() > 1e-12 {
            delta.normalize()
        } else {
            ab.normalize_or(Vec3::Y).any_orthonormal_vector()
        };
        if distance_squared < radius * radius {
            *p = closest + normal * radius;
        }
        Some(normal)
    } else {
        None
    }
}
