#[derive(Debug, Clone, Default)]
pub struct SkinMaterial {
    pub texture: Option<String>,
    pub alpha: bool,
    pub two_side: bool,
}

#[derive(Debug, Clone)]
pub struct SkinInfluence {
    pub bone: u32,
    pub weight: f32,
}

#[derive(Debug, Clone)]
pub struct SkinMesh {
    pub positions: Vec<f32>,

    pub uvs: Vec<f32>,

    pub skin_indices: Vec<u32>,

    pub skin_weights: Vec<f32>,

    pub skin_influences: Vec<Vec<SkinInfluence>>,

    pub indices: Vec<u32>,

    pub groups: Vec<SkinGroup>,

    pub materials: Vec<SkinMaterial>,

    pub vertex_count: usize,

    pub triangle_count: usize,

    pub min_z: f32,
}

#[derive(Debug, Clone)]
pub struct SkinGroup {
    pub start: usize,

    pub count: usize,

    pub material_index: usize,
}

pub fn parse_skin(text: &str) -> SkinMesh {
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut skin_indices = Vec::new();
    let mut skin_weights = Vec::new();
    let mut skin_influences: Vec<Vec<SkinInfluence>> = Vec::new();
    let mut materials: Vec<SkinMaterial> = Vec::new();
    let mut face_buckets: Vec<Vec<u32>> = Vec::new();

    let mut current_mat: Option<&mut SkinMaterial> = None;

    for raw in text.lines() {
        let line = raw.trim();

        if line == "Material" {
            materials.push(SkinMaterial::default());

            current_mat = materials.last_mut();
            continue;
        }
        if let Some(ref mut mat) = current_mat {
            if let Some(rest) = line.strip_prefix("Texture") {
                let tex = rest.trim();
                mat.texture = if tex.eq_ignore_ascii_case("NULL") || tex.is_empty() {
                    None
                } else {
                    Some(tex.to_string())
                };
                continue;
            }
            if line.starts_with("Option") {
                mat.alpha = line.to_uppercase().contains("ALPHA");
                continue;
            }
            if line.starts_with("TwoSide") {
                mat.two_side = line.to_uppercase().contains("TRUE");
                continue;
            }
        }

        if line.starts_with('v') {
            let parts: Vec<&str> = line.split_whitespace().collect();

            if parts.len() < 10 {
                continue;
            }

            if let (Ok(x), Ok(y), Ok(z)) = (
                parts[1].parse::<f32>(),
                parts[2].parse::<f32>(),
                parts[3].parse::<f32>(),
            ) {
                positions.push(x);
                positions.push(y);
                positions.push(z);
            } else {
                continue;
            }

            if parts.len() >= 7 && parts[4].eq_ignore_ascii_case("vt") {
                if let (Ok(u), Ok(v)) = (parts[5].parse::<f32>(), parts[6].parse::<f32>()) {
                    uvs.push(u);
                    uvs.push(v);
                } else {
                    uvs.push(0.0);
                    uvs.push(0.0);
                }
            } else {
                uvs.push(0.0);
                uvs.push(0.0);
            }

            let bone_start = parts.iter().position(|p| p.eq_ignore_ascii_case("Bones"));
            let refs = if let Some(bi) = bone_start {
                let count: usize = parts.get(bi + 1).and_then(|c| c.parse().ok()).unwrap_or(0);
                let mut refs = Vec::with_capacity(count);
                for i in 0..count {
                    let idx = bi + 2 + i * 2;
                    if idx + 1 < parts.len() {
                        let bone = parts[idx].parse::<u32>().unwrap_or(0);
                        let weight = parts[idx + 1].parse::<f32>().unwrap_or(0.0);
                        refs.push(SkinInfluence { bone, weight });
                    }
                }

                refs.sort_by(|a, b| {
                    b.weight
                        .partial_cmp(&a.weight)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                refs
            } else {
                Vec::new()
            };

            skin_influences.push(refs.clone());

            let mut idx = [0u32; 4];
            let mut wgt = [0.0f32; 4];
            let sum: f32 = refs.iter().take(4).map(|r| r.weight).sum();
            for (i, r) in refs.iter().take(4).enumerate() {
                idx[i] = r.bone;
                wgt[i] = if sum > 0.0 { r.weight / sum } else { 0.0 };
            }
            skin_indices.extend_from_slice(&idx);
            skin_weights.extend_from_slice(&wgt);
            continue;
        }

        if line.starts_with('f') {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 5 && parts[1] == "3" {
                let mat: usize = parts[2].parse().unwrap_or(0);
                let i0: u32 = parts[3].parse().unwrap_or(0);
                let i1: u32 = parts[4].parse().unwrap_or(0);
                let i2: u32 = parts[5].parse().unwrap_or(0);
                if face_buckets.len() <= mat {
                    face_buckets.resize(mat + 1, Vec::new());
                }
                face_buckets[mat].push(i0);
                face_buckets[mat].push(i1);
                face_buckets[mat].push(i2);
            }
        }
    }

    let mut indices = Vec::new();
    let mut groups = Vec::new();
    for (mat_idx, bucket) in face_buckets.iter().enumerate() {
        if bucket.is_empty() {
            continue;
        }
        groups.push(SkinGroup {
            start: indices.len(),
            count: bucket.len(),
            material_index: mat_idx,
        });
        indices.extend_from_slice(bucket);
    }

    let min_z = positions
        .iter()
        .skip(2)
        .step_by(3)
        .fold(f32::INFINITY, |a, &b| a.min(b));

    let vertex_count = positions.len() / 3;
    let triangle_count = indices.len() / 3;

    SkinMesh {
        positions,
        uvs,
        skin_indices,
        skin_weights,
        skin_influences,
        indices,
        groups,
        materials,
        vertex_count,
        triangle_count,
        min_z,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skin_minimal() {
        let skc = r#"
Material
  Texture   nm001b01.tga
  Option    NORMAL
  TwoSide   FALSE
v 0.0 0.0 0.0 vt 0.0 0.0 Bones 1 0 1.0
v 1.0 0.0 0.0 vt 1.0 0.0 Bones 1 0 1.0
v 0.0 1.0 0.0 vt 0.0 1.0 Bones 1 0 1.0
f 3 0 0 1 2
"#;
        let mesh = parse_skin(skc);
        assert_eq!(mesh.vertex_count, 3);
        assert_eq!(mesh.triangle_count, 1);
        assert_eq!(mesh.materials.len(), 1);
        assert_eq!(mesh.materials[0].texture.as_deref(), Some("nm001b01.tga"));
        assert!(!mesh.materials[0].alpha);
        assert_eq!(mesh.indices, vec![0, 1, 2]);
    }

    #[test]
    fn test_parse_skin_multi_bone() {
        let skc = r#"
Material
  Texture   skin.tga
v 0.0 0.0 0.0 vt 0.5 0.5 Bones 2 11 0.83 2 0.17
f 3 0 0 1 2
v 1.0 0.0 0.0 vt 1.0 0.0 Bones 1 5 1.0
f 3 0 3 1 2
"#;
        let mesh = parse_skin(skc);
        assert_eq!(mesh.vertex_count, 2);

        assert_eq!(mesh.skin_indices[0], 11);
        assert!((mesh.skin_weights[0] - 0.83).abs() < 0.01);

        assert_eq!(mesh.skin_influences[0].len(), 2);
        assert_eq!(mesh.skin_influences[0][0].bone, 11);
    }

    #[test]
    fn test_parse_skin_multi_material() {
        let skc = r#"
Material
  Texture   mat_a.tga
Material
  Texture   mat_b.tga
v 0.0 0.0 0.0 vt 0.0 0.0 Bones 1 0 1.0
v 1.0 0.0 0.0 vt 1.0 0.0 Bones 1 0 1.0
v 0.0 1.0 0.0 vt 0.0 1.0 Bones 1 0 1.0
v 0.0 0.0 1.0 vt 0.0 0.0 Bones 1 0 1.0
f 3 0 0 1 2
f 3 1 2 1 3
"#;
        let mesh = parse_skin(skc);
        assert_eq!(mesh.materials.len(), 2);
        assert_eq!(mesh.groups.len(), 2);
        assert_eq!(mesh.groups[0].material_index, 0);
        assert_eq!(mesh.groups[1].material_index, 1);
    }
}
