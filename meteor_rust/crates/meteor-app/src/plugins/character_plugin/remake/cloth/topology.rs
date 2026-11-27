use super::*;
use crate::plugins::graphics_pack::cloth_binding::closest_triangle;
use meteor_asset::skc::SkinMesh;
use std::collections::BTreeSet;

pub(in super::super) struct Topology {
    pub points: Vec<Vec3>,
    pub skin_vertices: Vec<usize>,
    pub freedom: Vec<f32>,
    pub triangles: Vec<[usize; 3]>,
    pub area: Vec<f32>,
    pub edges: Vec<Edge>,
    pub bends: Vec<Bend>,
    pub bindings: Vec<Binding>,
    pub normal_vertices: Vec<usize>,
    pub normal_triangles: Vec<[usize; 3]>,
}
pub(in super::super) struct Binding {
    pub vertex: usize,
    pub particles: [usize; 3],
    pub weights: Vec3,
    pub influence: f32,
}
fn point(s: &SkinMesh, i: usize) -> Vec3 {
    Vec3::from_slice(&s.positions[i * 3..i * 3 + 3])
}
fn mask(s: &SkinMesh, i: usize) -> f32 {
    (0..4)
        .filter(|j| matches!(s.skin_indices[i*4+j],30..=33|36..=37))
        .map(|j| s.skin_weights[i * 4 + j])
        .sum()
}
impl Topology {
    pub fn from_triangles(
        points: Vec<Vec3>,
        freedom: Vec<f32>,
        triangles: Vec<[usize; 3]>,
    ) -> Self {
        let mut topology = Self {
            area: vec![0.; points.len()],
            points,
            freedom,
            triangles,
            skin_vertices: Vec::new(),
            edges: Vec::new(),
            bends: Vec::new(),
            bindings: Vec::new(),
            normal_vertices: Vec::new(),
            normal_triangles: Vec::new(),
        };
        topology.constraints();
        topology
    }
    pub fn prepare(low: &SkinMesh, render: &SkinMesh) -> Result<Self, String> {
        let mut weld = BTreeMap::new();
        let mut points = Vec::new();
        let mut representatives = Vec::new();
        let mut masks = Vec::new();
        let mut vertex_map = Vec::new();
        for i in 0..low.vertex_count {
            let p = point(low, i);
            let key = p.to_array().map(f32::to_bits);
            let id = *weld.entry(key).or_insert_with(|| {
                points.push(p);
                representatives.push(i);
                masks.push(mask(low, i));
                points.len() - 1
            });
            vertex_map.push(id);
        }
        let mut selected = BTreeSet::new();
        let mut triangles = Vec::new();
        for face in low.indices.chunks_exact(3) {
            let ids = [
                vertex_map[face[0] as usize],
                vertex_map[face[1] as usize],
                vertex_map[face[2] as usize],
            ];
            if ids.iter().any(|i| masks[*i] > 0.01)
                && (points[ids[1]] - points[ids[0]])
                    .cross(points[ids[2]] - points[ids[0]])
                    .length_squared()
                    > 1e-12
            {
                selected.extend(ids);
                triangles.push(ids);
            }
        }
        if selected.is_empty() || selected.len() > 1536 {
            return Err(format!("cloth proxy budget: {} particles", selected.len()));
        }
        let remap: BTreeMap<_, _> = selected
            .iter()
            .enumerate()
            .map(|(new, old)| (*old, new))
            .collect();
        let mut result = Self {
            points: selected.iter().map(|i| points[*i]).collect(),
            skin_vertices: selected.iter().map(|i| representatives[*i]).collect(),
            freedom: selected
                .iter()
                .map(|i| {
                    if masks[*i] <= 0.08 {
                        0.
                    } else {
                        ((masks[*i] - 0.08) / 0.92).sqrt()
                    }
                })
                .collect(),
            triangles: triangles.iter().map(|ids| ids.map(|i| remap[&i])).collect(),
            area: vec![0.; selected.len()],
            edges: Vec::new(),
            bends: Vec::new(),
            bindings: Vec::new(),
            normal_vertices: Vec::new(),
            normal_triangles: Vec::new(),
        };
        result.constraints();

        let mut visited = vec![false; result.points.len()];
        let mut adjacency = vec![Vec::new(); result.points.len()];
        for e in &result.edges {
            adjacency[e.ids[0]].push(e.ids[1]);
            adjacency[e.ids[1]].push(e.ids[0]);
        }
        for seed in 0..visited.len() {
            if visited[seed] {
                continue;
            }
            let mut stack = vec![seed];
            let mut component = Vec::new();
            visited[seed] = true;
            while let Some(i) = stack.pop() {
                component.push(i);
                for &j in &adjacency[i] {
                    if !visited[j] {
                        visited[j] = true;
                        stack.push(j);
                    }
                }
            }
            if !component.iter().any(|i| result.freedom[*i] == 0.) {
                for i in component {
                    result.freedom[i] = 0.;
                }
            }
        }
        for i in 0..render.vertex_count {
            if mask(render, i) <= 0.08 {
                continue;
            }
            let p = point(render, i);
            let mut best = (f32::MAX, [0; 3], Vec3::ZERO);
            for &ids in &result.triangles {
                let (near, weights) = closest_triangle(p, ids.map(|j| result.points[j]));
                let distance = near.distance_squared(p);
                if distance < best.0 {
                    best = (distance, ids, weights);
                }
            }

            if best.0 > 4. {
                return Err(format!("cloth binding too far from proxy: vertex {i}"));
            }
            let freedom = ((mask(render, i) - 0.08) / 0.92).sqrt();
            let proxy_freedom = (0..3)
                .map(|j| result.freedom[best.1[j]] * best.2[j])
                .sum::<f32>();
            let influence = (freedom / proxy_freedom.max(1e-6)).min(1.);
            result.bindings.push(Binding {
                vertex: i,
                particles: best.1,
                weights: best.2,
                influence,
            });
        }
        let affected: BTreeSet<_> = result.bindings.iter().map(|b| b.vertex).collect();
        let mut normals = affected.clone();
        for face in render.indices.chunks_exact(3) {
            if face.iter().any(|i| affected.contains(&(*i as usize))) {
                normals.extend(face.iter().map(|i| *i as usize));
            }
        }
        result.normal_vertices = normals.iter().copied().collect();
        result.normal_triangles = render
            .indices
            .chunks_exact(3)
            .filter(|f| f.iter().any(|i| normals.contains(&(*i as usize))))
            .map(|f| [f[0] as usize, f[1] as usize, f[2] as usize])
            .collect();
        Ok(result)
    }
    pub fn constraints(&mut self) {
        let mut edges: BTreeMap<[usize; 2], Vec<usize>> = BTreeMap::new();
        for &[a, b, c] in &self.triangles {
            let area = (self.points[b] - self.points[a])
                .cross(self.points[c] - self.points[a])
                .length()
                / 6.;
            for i in [a, b, c] {
                self.area[i] += area;
            }
            for (a, b, opposite) in [(a, b, c), (b, c, a), (c, a, b)] {
                edges
                    .entry([a.min(b), a.max(b)])
                    .or_default()
                    .push(opposite);
            }
        }
        for (ids, opposites) in edges {
            self.edges.push(Edge {
                ids,
                rest: self.points[ids[0]].distance(self.points[ids[1]]),
            });
            if opposites.len() == 2 {
                let ids = [ids[0], ids[1], opposites[0], opposites[1]];
                if let Some((rest, _)) = dihedral(ids.map(|i| self.points[i])) {
                    self.bends.push(Bend { ids, rest });
                }
            }
        }
    }
}
