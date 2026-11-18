use super::{cloth_obj::ClothObj, config::ClothMesh};
use bevy::prelude::*;
use std::collections::VecDeque;

pub(crate) struct Pinning {
    pub freedom: Vec<f32>,
    pub weights: Vec<Vec<(usize, f32)>>,
}

impl Pinning {
    pub(crate) fn prepare(obj: &ClothObj, source: &ClothMesh) -> Result<Self, String> {
        let groups = std::iter::once(source.pin_vertices.as_slice()).chain(
            source
                .pin_groups
                .iter()
                .map(|group| group.vertices.as_slice()),
        );
        let mut fixed = vec![None; obj.points.len()];
        let mut palette = Vec::new();
        for (group, vertices) in groups.enumerate() {
            let mut seeds = Vec::new();
            for &vertex in vertices {
                let point = obj.pin_point(vertex)?;
                match fixed[point] {
                    None => {
                        fixed[point] = Some(group);
                        seeds.push(point);
                    }
                    Some(old) if old != group => {
                        return Err(format!(
                            "cloth vertex {vertex} belongs to conflicting pin groups"
                        ));
                    }
                    _ => {}
                }
            }
            palette.push(seeds);
        }
        let pins: Vec<_> = source
            .pin_vertices
            .iter()
            .chain(source.pin_groups.iter().flat_map(|group| &group.vertices))
            .copied()
            .collect();
        let freedom = obj.freedom(&pins)?;
        if source.pin_groups.is_empty() {
            return Ok(Self {
                freedom,
                weights: Vec::new(),
            });
        }
        let mut edges = vec![Vec::new(); obj.points.len()];
        for [a, b, c] in &obj.triangles {
            for (&a, &b) in [(a, b), (b, c), (a, c)] {
                edges[a].push(b);
                edges[b].push(a);
            }
        }
        let mut weights = vec![Vec::new(); obj.points.len()];
        for (group, seeds) in palette.into_iter().enumerate() {
            let mut distances = vec![u32::MAX; obj.points.len()];
            for &seed in &seeds {
                distances[seed] = 0;
            }
            let mut queue = VecDeque::from(seeds);
            while let Some(point) = queue.pop_front() {
                for &next in &edges[point] {
                    if distances[next] == u32::MAX {
                        distances[next] = distances[point] + 1;
                        queue.push_back(next);
                    }
                }
            }
            for (point, distance) in distances.into_iter().enumerate() {
                let weight = match fixed[point] {
                    Some(owner) if owner == group => 1.0,
                    None if distance != u32::MAX => 1.0 / (distance as f32).powi(3),
                    _ => continue,
                };
                weights[point].push((group, weight));
            }
        }
        for point in &mut weights {
            let total: f32 = point.iter().map(|(_, weight)| *weight).sum();
            for (_, weight) in point {
                *weight /= total;
            }
        }
        Ok(Self { freedom, weights })
    }
}

struct RenderBinding {
    face: usize,
    particles: [usize; 3],
    weights: Vec3,
    offset: Vec3,
    rest_delta: Vec3,
}

pub(crate) struct DetailMesh {
    bindings: Vec<RenderBinding>,
    pub triangles: Vec<[usize; 3]>,
}

fn triangle_frame([a, b, c]: [Vec3; 3]) -> Option<Mat3> {
    let tangent = (b - a).try_normalize()?;
    let normal = tangent.cross(c - a).try_normalize()?;
    Some(Mat3::from_cols(tangent, normal.cross(tangent), normal))
}

impl RenderBinding {
    fn position(&self, points: &[Vec3], frames: &[Mat3], orientation: Mat3) -> Vec3 {
        let [a, b, c] = self.particles.map(|id| points[id]);
        let anchor = a * self.weights.x + b * self.weights.y + c * self.weights.z;
        let frame = frames[self.face];
        let offset = if frame == Mat3::ZERO {
            orientation * self.rest_delta
        } else {
            frame * self.offset
        };
        anchor + offset
    }
}

impl DetailMesh {
    pub(super) fn prepare(sim: &ClothObj, render: &ClothObj) -> Result<Self, String> {
        sim.simulation_budget()?;
        let faces: Vec<_> = sim
            .triangles
            .iter()
            .map(|&indices| {
                let points = indices.map(|id| sim.points[id]);
                (indices, points, triangle_frame(points).unwrap())
            })
            .collect();
        let bindings = render
            .points
            .iter()
            .map(|&point| {
                let (index, distance, delta, weights) = faces
                    .iter()
                    .enumerate()
                    .map(|(index, face)| {
                        let (closest, weights) = closest_triangle(point, face.1);
                        let delta = point - closest;
                        (index, delta.length_squared(), delta, weights)
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .unwrap();
                if distance > 0.35_f32.powi(2) {
                    return Err(
                        "cloth render mesh is more than 0.35 m from its simulation mesh".into(),
                    );
                }
                Ok(RenderBinding {
                    face: index,
                    particles: faces[index].0,
                    weights,
                    offset: faces[index].2.transpose() * delta,
                    rest_delta: delta,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            bindings,
            triangles: render.triangles.clone(),
        })
    }

    pub(crate) fn deform(
        &self,
        points: &[Vec3],
        faces: &[[usize; 3]],
        orientation: Mat3,
        frames: &mut Vec<Mat3>,
        output: &mut Vec<Vec3>,
    ) {
        let sign = orientation.determinant().signum();
        frames.clear();
        frames.extend(faces.iter().map(|&face| {
            triangle_frame(face.map(|i| points[i])).map_or(Mat3::ZERO, |mut frame| {
                frame.y_axis *= sign;
                frame.z_axis *= sign;
                frame
            })
        }));
        output.clear();
        output.extend(
            self.bindings
                .iter()
                .map(|binding| binding.position(points, frames, orientation)),
        );
    }
}

pub(crate) fn closest_triangle(p: Vec3, [a, b, c]: [Vec3; 3]) -> (Vec3, Vec3) {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0. && d2 <= 0. {
        return (a, Vec3::X);
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0. && d4 <= d3 {
        return (b, Vec3::Y);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0. && d1 >= 0. && d3 <= 0. {
        let v = d1 / (d1 - d3);
        return (a + v * ab, Vec3::new(1. - v, v, 0.));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0. && d5 <= d6 {
        return (c, Vec3::Z);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0. && d2 >= 0. && d6 <= 0. {
        let w = d2 / (d2 - d6);
        return (a + w * ac, Vec3::new(1. - w, 0., w));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0. && d4 - d3 >= 0. && d5 - d6 >= 0. {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + w * (c - b), Vec3::new(0., 1. - w, w));
    }
    let denom = 1. / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (a + ab * v + ac * w, Vec3::new(1. - v - w, v, w))
}
