#[derive(Debug, Clone, Default)]
pub struct GmcMesh {
    pub textures: Vec<String>,

    pub shaders: Vec<GmcShader>,

    pub objects: Vec<GmcObject>,
}

#[derive(Debug, Clone, Default)]
pub struct GmcShader {
    pub id: usize,

    pub texture_index: i32,

    pub texture_mode: String,
    pub two_side: bool,
    pub blend_enabled: bool,
    pub blend_source: String,
    pub blend_destination: String,
    pub opacity: f32,
}

#[derive(Debug, Clone, Default)]
pub struct GmcObject {
    pub name: String,

    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub colors: Vec<u8>,
    pub uvs: Vec<f32>,

    pub faces: Vec<GmcFace>,
}

#[derive(Debug, Clone, Copy)]
pub struct GmcFace {
    pub shader: i32,
    pub indices: [u32; 3],
}

pub fn parse_gmc(text: &str) -> GmcMesh {
    let mut mesh = GmcMesh::default();
    let mut in_textures = false;
    let mut current: Option<usize> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if in_textures {
            if line == "{" {
                continue;
            }
            if line == "}" {
                in_textures = false;
                continue;
            }
            mesh.textures.push(line.to_string());
            continue;
        }

        if let Some(rest) = line
            .strip_prefix("Textures")
            .or_else(|| line.strip_prefix("textures"))
        {
            if rest
                .trim()
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
            {
                in_textures = true;
                continue;
            }
        }

        if let Some(rest) = strip_prefix_ci(line, "Object") {
            let Some(name) = rest.split_whitespace().next() else {
                continue;
            };
            let index = mesh.objects.len();
            mesh.objects.push(GmcObject {
                name: name.to_string(),
                ..Default::default()
            });
            current = Some(index);
            continue;
        }
        let Some(obj) = current.and_then(|index| mesh.objects.get_mut(index)) else {
            continue;
        };

        if let Some(rest) = strip_prefix_ci(line, "v") {
            let parts: Vec<&str> = rest.split_whitespace().collect();

            if parts.len() >= 13 {
                let mut iter = parts.iter();
                let get = |iter: &mut std::slice::Iter<&str>| -> f32 {
                    iter.next().and_then(|s| s.parse().ok()).unwrap_or(0.0)
                };
                let mut position = [0.0f32; 3];
                let mut normal = [0.0f32; 3];
                let mut color = [0u8; 4];
                let mut uv = [0.0f32; 2];

                for slot in &mut position {
                    *slot = get(&mut iter);
                }
                let _ = iter.next();
                for slot in &mut normal {
                    *slot = get(&mut iter);
                }
                let _ = iter.next();
                for slot in &mut color {
                    *slot = get(&mut iter) as u8;
                }
                let _ = iter.next();
                for slot in &mut uv {
                    *slot = get(&mut iter);
                }
                obj.positions.extend_from_slice(&position);
                obj.normals.extend_from_slice(&normal);
                obj.colors.extend_from_slice(&color);
                obj.uvs.extend_from_slice(&uv);
            }
            continue;
        }

        if let Some(rest) = strip_prefix_ci(line, "f") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 4 {
                let shader = parts[0].parse::<i32>().unwrap_or(0);
                let indices = [
                    parts[1].parse::<u32>().unwrap_or(0),
                    parts[2].parse::<u32>().unwrap_or(0),
                    parts[3].parse::<u32>().unwrap_or(0),
                ];
                obj.faces.push(GmcFace { shader, indices });
            }
        }
    }

    let mut shader_pos = 0usize;
    while let Some(rel) = text[shader_pos..].find("shader") {
        let start = shader_pos + rel;
        let body_start = text[start..].find('{');
        let Some(body_start) = body_start.map(|i| start + i) else {
            break;
        };
        let body_end = text[body_start..].find('}');
        let Some(body_end) = body_end.map(|i| body_start + i) else {
            break;
        };
        let header = &text[start..body_start];
        let body = &text[body_start + 1..body_end];
        let id: usize = header
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let mut shader = GmcShader {
            id,
            texture_index: -1,
            ..Default::default()
        };
        for line in body.lines() {
            let line = line.trim();
            if let Some(rest) = strip_prefix_ci(line, "Texture") {
                let mut parts = rest.split_whitespace();
                shader.texture_index = parts.next().and_then(|s| s.parse().ok()).unwrap_or(-1);
                shader.texture_mode = parts
                    .next()
                    .map(|s| s.to_uppercase())
                    .unwrap_or_else(|| "NORMAL".into());
            } else if let Some(rest) = strip_prefix_ci(line, "Blend") {
                let mut parts = rest.split_whitespace();
                if let Some(flag) = parts.next() {
                    shader.blend_enabled = flag.eq_ignore_ascii_case("ENABLE");
                    shader.blend_source = parts.next().unwrap_or("").to_lowercase();
                    shader.blend_destination = parts.next().unwrap_or("").to_lowercase();
                }
            } else if let Some(rest) = strip_prefix_ci(line, "Opaque") {
                shader.opacity = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1.0);
            } else if let Some(rest) = strip_prefix_ci(line, "TwoSide") {
                shader.two_side = rest
                    .split_whitespace()
                    .next()
                    .is_some_and(|s| matches!(s, "1" | "2" | "true" | "TRUE"));
            }
        }
        mesh.shaders.push(shader);
        shader_pos = body_end + 1;
    }
    mesh
}

fn strip_prefix_ci<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let head = line.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        Some(&line[prefix.len()..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# weapon model
Textures 2
{
ctexture/wpnLS01.jpg
ctexture/wpnLS02.jpg
}
Object sword
{
v 0.0 0.0 0.0 n 0 1 0 c 255 255 255 255 t 0.0 0.0
v 1.0 0.0 0.0 n 0 1 0 c 255 255 255 255 t 1.0 0.0
v 0.0 1.0 0.0 n 0 1 0 c 255 255 255 255 t 0.0 1.0
f 0 0 1 2
}
shader 0 {
Texture 0 NORMAL
Blend DISABLE 1 0
Opaque 1.0
TwoSide 1
}
"#;

    #[test]
    fn parses_textures_and_object() {
        let mesh = parse_gmc(SAMPLE);
        assert_eq!(mesh.textures.len(), 2);
        assert_eq!(mesh.textures[0], "ctexture/wpnLS01.jpg");
        assert_eq!(mesh.objects.len(), 1);
        assert_eq!(mesh.objects[0].name, "sword");
        assert_eq!(mesh.objects[0].positions.len(), 9);
        assert_eq!(mesh.objects[0].faces.len(), 1);
        assert_eq!(mesh.objects[0].faces[0].indices, [0, 1, 2]);
    }

    #[test]
    fn parses_shader_block() {
        let mesh = parse_gmc(SAMPLE);
        assert_eq!(mesh.shaders.len(), 1);
        let shader = &mesh.shaders[0];
        assert_eq!(shader.id, 0);
        assert_eq!(shader.texture_index, 0);
        assert_eq!(shader.texture_mode, "NORMAL");
        assert!(!shader.blend_enabled);
        assert!(shader.two_side);
        assert!((shader.opacity - 1.0).abs() < 1e-5);
    }

    #[test]
    fn ignores_comments_and_empty() {
        let mesh = parse_gmc("# comment\n\n\n");
        assert!(mesh.objects.is_empty());
        assert!(mesh.textures.is_empty());

        let mesh = parse_gmc("Object\n");
        assert!(mesh.objects.is_empty());
    }
}
