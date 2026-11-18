use bevy::prelude::*;
use std::collections::HashMap;

const MAX_POINTS: usize = 4096;
const MAX_FACES: usize = 8192;

pub(crate) struct ClothObj {
    pub points: Vec<Vec3>,
    pub triangles: Vec<[usize; 3]>,
    pub vertices: Vec<(usize, [f32; 2])>,
    pub indices: Vec<u16>,
    original_points: Vec<Option<usize>>,
    pub has_uvs: bool,
}

fn index(token: &str, length: usize) -> Option<usize> {
    let index: i32 = token.parse().ok()?;
    let index = if index < 0 {
        length as i32 + index
    } else {
        index.checked_sub(1)?
    };
    (0..length as i32)
        .contains(&index)
        .then_some(index as usize)
}

fn numbers<const N: usize>(mut parts: std::str::SplitWhitespace<'_>) -> Option<[f32; N]> {
    let mut values = [0.; N];
    for value in &mut values {
        let number: f32 = parts.next()?.parse().ok()?;
        if !number.is_finite() {
            return None;
        }
        *value = number;
    }
    Some(values)
}

impl ClothObj {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > 1024 * 1024 {
            return Err("cloth OBJ exceeds 1 MiB".into());
        }
        let contents = std::str::from_utf8(bytes).map_err(|_| "cloth OBJ must be UTF-8")?;
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut normals = 0;
        let mut obj = Self {
            points: Vec::new(),
            triangles: Vec::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
            original_points: Vec::new(),
            has_uvs: true,
        };
        let mut welded = HashMap::new();
        let mut render = HashMap::new();
        let mut edges = HashMap::<[usize; 2], u8>::new();
        for (line_id, line) in contents.lines().enumerate() {
            let mut parts = line
                .split('#')
                .next()
                .unwrap_or_default()
                .split_whitespace();
            let invalid = || format!("invalid cloth OBJ at line {}", line_id + 1);
            match parts.next() {
                Some("v") if positions.len() < MAX_POINTS => {
                    let p = Vec3::from_array(numbers(parts).ok_or_else(invalid)?);
                    if p.abs().max_element() > 5.0 {
                        return Err(invalid());
                    }
                    positions.push(p);
                    obj.original_points.push(None);
                }
                Some("vt") if uvs.len() < MAX_FACES * 3 => {
                    let [u, v] = numbers(parts).ok_or_else(invalid)?;
                    if u.abs().max(v.abs()) > 1024.0 {
                        return Err(invalid());
                    }
                    uvs.push([u, 1.0 - v]);
                }
                Some("vn") if normals < MAX_FACES * 3 => {
                    numbers::<3>(parts).ok_or_else(invalid)?;
                    normals += 1;
                }
                Some("f") if obj.triangles.len() < MAX_FACES => {
                    let mut triangle = [0; 3];
                    for point in &mut triangle {
                        let mut corner = parts.next().ok_or_else(invalid)?.split('/');
                        let vertex = index(corner.next().ok_or_else(invalid)?, positions.len())
                            .ok_or_else(invalid)?;
                        let uv = corner
                            .next()
                            .filter(|s| !s.is_empty())
                            .map(|s| index(s, uvs.len()).ok_or_else(invalid))
                            .transpose()?;
                        if let Some(normal) = corner.next().filter(|s| !s.is_empty()) {
                            index(normal, normals).ok_or_else(invalid)?;
                        }
                        if corner.next().is_some() {
                            return Err(invalid());
                        }
                        let position = positions[vertex];
                        let key = position
                            .to_array()
                            .map(|v| if v == 0.0 { 0 } else { v.to_bits() });
                        *point = *welded.entry(key).or_insert_with(|| {
                            obj.points.push(position);
                            obj.points.len() - 1
                        });
                        obj.original_points[vertex] = Some(*point);
                        obj.has_uvs &= uv.is_some();
                        let id = *render.entry((*point, uv)).or_insert_with(|| {
                            obj.vertices
                                .push((*point, uv.map_or([0., 0.], |id| uvs[id])));
                            (obj.vertices.len() - 1) as u16
                        });
                        obj.indices.push(id);
                    }
                    if parts.next().is_some() {
                        return Err(format!(
                            "cloth OBJ faces must be triangulated, line {}",
                            line_id + 1
                        ));
                    }
                    let [a, b, c] = triangle.map(|id| obj.points[id]);
                    if (b - a).cross(c - a).length_squared() <= 1e-12 {
                        return Err(invalid());
                    }
                    let [a, b, c] = triangle;
                    for (a, b) in [(a, b), (a, c), (b, c)] {
                        let count = edges.entry([a.min(b), a.max(b)]).or_default();
                        *count += 1;
                        if *count > 2 {
                            return Err(invalid());
                        }
                    }
                    obj.triangles.push(triangle);
                }
                None | Some("g" | "o" | "s" | "mtllib" | "usemtl") => {}
                _ => return Err(invalid()),
            }
        }
        if obj.triangles.is_empty() {
            return Err("cloth OBJ has no triangles".into());
        }
        Ok(obj)
    }

    pub(crate) fn freedom(&self, pins: &[usize]) -> Result<Vec<f32>, String> {
        self.simulation_budget()?;
        let mut freedom = vec![1.; self.points.len()];
        for &index in pins {
            let point = self.pin_point(index)?;
            freedom[point] = 0.;
        }
        let mut visited: Vec<_> = freedom.iter().map(|v| *v == 0.).collect();
        if visited.iter().all(|v| *v) {
            return Err("cloth OBJ needs movable vertices".into());
        }
        let mut stack: Vec<_> = visited
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.then_some(i))
            .collect();
        let mut neighbors = vec![Vec::new(); self.points.len()];
        for &[a, b, c] in &self.triangles {
            for (a, b) in [(a, b), (a, c), (b, c)] {
                neighbors[a].push(b);
                neighbors[b].push(a);
            }
        }
        while let Some(i) = stack.pop() {
            for &j in &neighbors[i] {
                if !visited[j] {
                    visited[j] = true;
                    stack.push(j);
                }
            }
        }
        if visited.iter().any(|v| !v) {
            return Err("every cloth OBJ piece requires a pinned vertex".into());
        }
        Ok(freedom)
    }

    pub(super) fn simulation_budget(&self) -> Result<(), String> {
        if self.original_points.len() > 384 || self.triangles.len() > 768 {
            return Err("cloth simulation OBJ exceeds 384 positions or 768 triangles".into());
        }
        Ok(())
    }

    pub(super) fn pin_point(&self, index: usize) -> Result<usize, String> {
        index
            .checked_sub(1)
            .and_then(|i| self.original_points.get(i))
            .copied()
            .flatten()
            .ok_or_else(|| format!("invalid cloth OBJ pinned vertex {index}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloth_obj_welds_uv_seams_and_rejects_unanchored_islands() {
        let bytes = b"v 0 0 0\nv 1 0 0\nv 0 -1 0\nv 1 -1 0\nvt 0 0\nvt 1 0\n\
                      f 1/1 2/2 3/1\nf -3/1 -1/2 -2/2\n";
        let mesh = ClothObj::parse(bytes).unwrap();
        assert_eq!(mesh.points.len(), 4);
        assert_eq!(mesh.vertices.len(), 6);
        assert_eq!(mesh.freedom(&[1, 2]).unwrap(), [0., 0., 1., 1.]);
        assert!(mesh.freedom(&[0]).is_err());
        assert!(mesh.freedom(&[1, 2, 3, 4]).is_err());
        let mut disconnected = bytes.to_vec();
        disconnected.extend(b"v 2 0 0\nv 3 0 0\nv 2 -1 0\nf 5 6 7\n");
        assert!(ClothObj::parse(&disconnected)
            .unwrap()
            .freedom(&[1, 2])
            .is_err());
        assert!(ClothObj::parse(b"v 0 0 0\nf 1 1 99\n").is_err());
    }
}
