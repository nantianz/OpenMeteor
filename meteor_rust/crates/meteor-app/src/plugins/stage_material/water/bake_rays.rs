use bevy::prelude::*;

#[derive(Clone)]
pub(super) struct Triangle {
    pub sky: bool,
    pub points: [Vec3; 3],
    pub uv: [Vec2; 3],
    pub color: [Vec3; 3],
    pub material: usize,
}
struct Node {
    lo: Vec3,
    hi: Vec3,
    children: Option<(usize, usize)>,
    indices: Vec<usize>,
}
pub(super) struct Rays {
    pub triangles: Vec<Triangle>,
    nodes: Vec<Node>,
}
pub(super) struct Hit {
    pub distance: f32,
    pub index: usize,
    pub weights: Vec3,
}
impl Rays {
    pub fn new(triangles: Vec<Triangle>) -> Self {
        let indices = (0..triangles.len()).collect();
        let mut this = Self {
            triangles,
            nodes: Vec::new(),
        };
        if !this.triangles.is_empty() {
            this.build(indices);
        }
        this
    }
    fn build(&mut self, mut indices: Vec<usize>) -> usize {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for &i in &indices {
            for p in self.triangles[i].points {
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            lo,
            hi,
            children: None,
            indices: Vec::new(),
        });
        if indices.len() <= 16 {
            self.nodes[index].indices = indices;
        } else {
            let extent = hi - lo;
            let axis = if extent.x >= extent.y && extent.x >= extent.z {
                0
            } else if extent.y >= extent.z {
                1
            } else {
                2
            };
            indices.sort_by(|&a, &b| {
                let center = |i: usize| {
                    self.triangles[i]
                        .points
                        .iter()
                        .map(|p| p[axis])
                        .sum::<f32>()
                };
                center(a).total_cmp(&center(b)).then(a.cmp(&b))
            });
            let right = indices.split_off(indices.len() / 2);
            let children = (self.build(indices), self.build(right));
            self.nodes[index].children = Some(children);
        }
        index
    }
    pub fn trace(&self, origin: Vec3, direction: Vec3) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let inverse = direction.map(|v| if v.abs() < 1e-12 { 1e12 } else { 1.0 / v });
        let mut hit = None;
        self.visit(0, origin, direction, inverse, &mut hit);
        hit
    }
    fn visit(&self, index: usize, o: Vec3, d: Vec3, inv: Vec3, best: &mut Option<Hit>) {
        let n = &self.nodes[index];
        let a = (n.lo - o) * inv;
        let b = (n.hi - o) * inv;
        if a.min(b).max_element().max(0.0)
            > a.max(b)
                .min_element()
                .min(best.as_ref().map_or(f32::INFINITY, |h| h.distance))
                + 1e-5
        {
            return;
        }
        if let Some((left, right)) = n.children {
            self.visit(left, o, d, inv, best);
            self.visit(right, o, d, inv, best);
            return;
        }
        for &i in &n.indices {
            let [a, b, c] = self.triangles[i].points;
            let e1 = b - a;
            let e2 = c - a;
            let p = d.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-9 {
                continue;
            }
            let tvec = o - a;
            let u = tvec.dot(p) / det;
            let q = tvec.cross(e1);
            let v = d.dot(q) / det;
            let t = e2.dot(q) / det;
            if u >= -1e-7
                && v >= -1e-7
                && u + v <= 1.0000001
                && t > 0.001
                && t < best.as_ref().map_or(f32::INFINITY, |h| h.distance)
            {
                *best = Some(Hit {
                    distance: t,
                    index: i,
                    weights: Vec3::new(1.0 - u - v, u, v),
                });
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nearest_hit_has_barycentrics_and_misses_remain_empty() {
        let tri = |z| Triangle {
            sky: false,
            points: [
                Vec3::new(0., 0., z),
                Vec3::new(2., 0., z),
                Vec3::new(0., 2., z),
            ],
            uv: [Vec2::ZERO; 3],
            color: [Vec3::ONE; 3],
            material: 0,
        };
        let rays = Rays::new(vec![tri(0.), tri(1.)]);
        let hit = rays.trace(Vec3::new(0.5, 0.5, 3.), -Vec3::Z).unwrap();
        assert_eq!(hit.index, 1);
        assert_eq!(hit.distance, 2.);
        assert_eq!(hit.weights, Vec3::new(0.5, 0.25, 0.25));
        assert!(rays.trace(Vec3::new(3., 3., 3.), -Vec3::Z).is_none());
        assert!(Rays::new(vec![]).trace(Vec3::ZERO, Vec3::Z).is_none());
    }
}
