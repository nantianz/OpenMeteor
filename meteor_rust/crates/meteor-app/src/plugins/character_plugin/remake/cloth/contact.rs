use super::Capsule;
use crate::plugins::graphics_pack::cloth_binding::closest_triangle;
use bevy::prelude::*;

pub(in super::super) struct CapsuleSurface {
    a: Vec3,
    b: Vec3,
    axis: Vec3,
    radius: f32,
    min: Vec3,
    max: Vec3,
}

struct Contact {
    weights: Vec3,
    axis: Vec3,
    offset: Vec3,
    distance_squared: f32,
}

impl CapsuleSurface {
    pub(in super::super) fn new(capsule: Capsule, margin: f32) -> Self {
        let radius = capsule.radius + margin;
        Self {
            a: capsule.a,
            b: capsule.b,
            axis: capsule.b - capsule.a,
            radius,
            min: capsule.a.min(capsule.b) - Vec3::splat(radius),
            max: capsule.a.max(capsule.b) + Vec3::splat(radius),
        }
    }

    fn closest(&self, [a, b, c]: [Vec3; 3]) -> Option<Contact> {
        if a.min(b).min(c).cmpgt(self.max).any() || a.max(b).max(c).cmplt(self.min).any() {
            return None;
        }
        let normal = (b - a).cross(c - a);
        let normal_len_sq = normal.length_squared();
        let da = normal.dot(self.a - a);
        let db = normal.dot(self.b - a);
        let radius_sq = self.radius * self.radius;
        if da * db > 0. && da.abs().min(db.abs()).powi(2) >= radius_sq * normal_len_sq {
            return None;
        }
        let tri = [a, b, c];
        if da * db < 0. {
            let plane = self.a + self.axis * (da / (da - db));
            let (closest, weights) = closest_triangle(plane, tri);
            if closest.distance_squared(plane) < 1e-10 {
                return Some(Contact {
                    weights,
                    axis: plane,
                    offset: Vec3::ZERO,
                    distance_squared: 0.,
                });
            }
        }

        let mut best = Contact {
            weights: Vec3::ZERO,
            axis: self.a,
            offset: Vec3::ZERO,
            distance_squared: radius_sq,
        };
        let mut consider = |axis: Vec3, point: Vec3, weights: Vec3| {
            let distance_squared = point.distance_squared(axis);
            if distance_squared < best.distance_squared {
                best = Contact {
                    weights,
                    axis,
                    offset: point - axis,
                    distance_squared,
                };
            }
        };
        if normal_len_sq > 1e-12 {
            for end in [self.a, self.b] {
                let (point, weights) = closest_triangle(end, tri);
                consider(end, point, weights);
            }
        }
        for (u, v, first, second) in [
            (a, b, Vec3::X, Vec3::Y),
            (b, c, Vec3::Y, Vec3::Z),
            (c, a, Vec3::Z, Vec3::X),
        ] {
            let (s, t) = segment_parameters(self.a, self.axis, u, v - u);
            consider(self.a + self.axis * s, u.lerp(v, t), first.lerp(second, t));
        }
        (best.distance_squared < radius_sq).then_some(best)
    }

    fn outside_contact(&self, tri: [Vec3; 3], normal: Vec3, recovery: f32) -> Option<(Vec3, f32)> {
        let extent = Vec3::splat(recovery);
        let [a, b, c] = tri;
        if a.min(b).min(c).cmpgt(self.max + extent).any()
            || a.max(b).max(c).cmplt(self.min - extent).any()
        {
            return None;
        }

        let outward = |point: Vec3| (point - a).dot(normal);
        let ends = [outward(self.a), outward(self.b)];
        let tri_limit = outward(b).min(outward(c)).min(0.);
        let tri_outer = outward(b).max(outward(c)).max(0.);
        if ends[0].max(ends[1]) + self.radius <= tri_limit
            || ends[0].min(ends[1]) > recovery + tri_outer
        {
            return None;
        }

        let flatten = |p: Vec3| p - normal * outward(p);
        let flat_a = flatten(self.a);
        let flat_b = flatten(self.b);
        let projected = Self::new(
            Capsule {
                a: flat_a,
                b: flat_b,
                radius: self.radius,
            },
            0.,
        );
        let contact = projected.closest(tri.map(flatten))?;
        let len = projected.axis.length_squared();
        let t = if len > 1e-10 {
            ((contact.axis - flat_a).dot(projected.axis) / len).clamp(0., 1.)
        } else {
            f32::from(ends[1] > ends[0])
        };
        let point = a * contact.weights.x + b * contact.weights.y + c * contact.weights.z;
        let axis = self.a + self.axis * t;
        let height = (axis - point).dot(normal);
        let skin = (self.radius * self.radius - contact.distance_squared)
            .max(0.)
            .sqrt();
        let depth = height + skin;
        (depth > 0. && height <= recovery).then_some((contact.weights, depth))
    }

    #[cfg(test)]
    pub(in super::super) fn depth(&self, triangles: [Vec3; 3]) -> f32 {
        self.closest(triangles)
            .map_or(0., |contact| self.radius - contact.distance_squared.sqrt())
    }
}

fn segment_parameters(a: Vec3, axis: Vec3, b: Vec3, edge: Vec3) -> (f32, f32) {
    let diff = a - b;
    let aa = axis.length_squared();
    let ee = edge.length_squared();
    let tol = 1e-10;
    if aa <= tol {
        return (0., (edge.dot(diff) / ee.max(tol)).clamp(0., 1.));
    }
    let ac = axis.dot(diff);
    if ee <= tol {
        return ((-ac / aa).clamp(0., 1.), 0.);
    }
    let ae = axis.dot(edge);
    let ec = edge.dot(diff);
    let determinant = aa * ee - ae * ae;
    let mut s = if determinant > tol {
        ((ae * ec - ac * ee) / determinant).clamp(0., 1.)
    } else {
        0.
    };
    let raw_t = (ae * s + ec) / ee;
    let t = raw_t.clamp(0., 1.);
    if t != raw_t {
        s = ((ae * t - ac) / aa).clamp(0., 1.);
    }
    (s, t)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn project_surface(
    positions: &mut [Vec3],
    previous: &[Vec3],
    targets: &[Vec3],
    triangles: &[[usize; 3]],
    inverse_mass: &[f32],
    capsules: &[Capsule],
    thickness: f32,
    recovery: f32,
) {
    let bounds = capsules
        .iter()
        .map(|&capsule| CapsuleSurface::new(capsule, thickness))
        .collect::<Vec<_>>();
    for &ids in triangles {
        let inverse = Vec3::from_array(ids.map(|i| inverse_mass[i]));
        if inverse == Vec3::ZERO {
            continue;
        }
        let rest_normal = if recovery > 0. {
            let [a, b, c] = ids.map(|i| targets[i]);
            (b - a).cross(c - a).try_normalize()
        } else {
            None
        };
        for capsule in &bounds {
            let tri = ids.map(|i| positions[i]);
            let solution = rest_normal
                .and_then(|normal| {
                    let (weights, depth) = capsule.outside_contact(tri, normal, recovery)?;
                    Some((weights, depth, normal))
                })
                .or_else(|| {
                    let contact = capsule.closest(tri)?;
                    let distance = contact.distance_squared.sqrt();
                    let normal = if distance > 0.004 {
                        contact.offset / distance
                    } else {
                        let [p, q, r] = ids.map(|i| previous[i]);
                        let old =
                            p * contact.weights.x + q * contact.weights.y + r * contact.weights.z;
                        (old - contact.axis).try_normalize().unwrap_or_else(|| {
                            (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalize_or_zero()
                        })
                    };
                    Some((contact.weights, capsule.radius - distance, normal))
                });
            let Some((weights, depth, normal)) = solution else {
                continue;
            };
            let response = inverse * weights;
            let denominator = weights.dot(response);
            if denominator <= 1e-10 {
                continue;
            }
            let strength = depth / denominator;
            for (i, response) in ids.into_iter().zip(response.to_array()) {
                positions[i] += normal * (strength * response).min(0.16);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_capsule_that_has_already_crossed_the_surface() {
        let triangle = [
            Vec3::new(-0.4, -0.4, 0.),
            Vec3::new(0.4, -0.4, 0.),
            Vec3::Y * 0.6,
        ];
        let capsule = CapsuleSurface::new(
            Capsule {
                a: Vec3::new(0., -0.1, 0.18),
                b: Vec3::new(0., 0.1, 0.18),
                radius: 0.1,
            },
            0.01,
        );
        assert_eq!(capsule.depth(triangle), 0.);
        let (_, distance) = capsule.outside_contact(triangle, Vec3::Z, 0.27).unwrap();
        assert!((distance - 0.29).abs() < 1e-5);
        assert!(capsule.outside_contact(triangle, Vec3::Z, 0.04).is_none());
    }
    #[test]
    fn face_contact_clears_between_vertices_with_a_fixed_border() {
        let mut triangle = [
            Vec3::new(-0.3, -0.3, 0.0),
            Vec3::new(0.3, -0.3, 0.),
            Vec3::Y * 0.7,
        ];
        let targets = triangle;
        let previous = triangle.map(|p| p + 0.16 * Vec3::Z);
        let capsule = Capsule {
            a: Vec3::new(0., 0.2, -0.10),
            b: Vec3::new(0., 0.2, 0.02),
            radius: 0.11,
        };
        let surface = CapsuleSurface::new(capsule, 0.01);
        assert!(triangle
            .iter()
            .all(|point| point.distance(capsule.b) > capsule.radius));
        let indices = [[0, 1, 2]];
        assert!(surface.depth(triangle) > 0.115);
        for _ in 0..6 {
            project_surface(
                &mut triangle,
                &previous,
                &targets,
                &indices,
                &[0., 0., 1.],
                &[capsule],
                0.01,
                0.,
            );
        }
        assert_eq!(
            &triangle[..2],
            &previous[..2]
                .iter()
                .map(|p| *p - 0.16 * Vec3::Z)
                .collect::<Vec<_>>()
        );
        assert!(surface.depth(triangle) < 0.02, "{:?}", triangle);
    }
}
