use meteor_core::{Quat, Vec3};

#[derive(Debug, Clone)]
pub struct BoneNode {
    pub name: String,
    pub parent: Option<String>,
    pub is_dummy: bool,
    pub pivot: Vec3,

    pub quat: Quat,
}

#[derive(Debug, Clone)]
pub struct BoneData {
    pub bones: Vec<BoneNode>,
    pub dummies: Vec<BoneNode>,
    pub all: Vec<BoneNode>,
}

pub fn parse_bones(text: &str) -> BoneData {
    let mut nodes: Vec<BoneNode> = Vec::new();

    let mut rest = text;
    while let Some(close) = rest.find('}') {
        let block = &rest[..=close];
        let after = &rest[close + 1..];

        let decl = if let Some(p) = block.find("bone ") {
            Some((false, p + 5))
        } else {
            block.find("Dummey ").map(|p| (true, p + 7))
        };

        if let Some((is_dummy, name_start)) = decl {
            let name_end = block[name_start..]
                .find(|c: char| c == '{' || c.is_whitespace())
                .map(|e| name_start + e)
                .unwrap_or(block.len());
            let name = block[name_start..name_end].trim().to_string();

            let body = if let Some(brace) = block.find('{') {
                &block[brace + 1..close]
            } else {
                ""
            };

            let parent = extract_field(body, "parent").filter(|s| *s != "NULL");
            let pivot = extract_vec3(body, "pivot")
                .map(|v| Vec3::new(v[0], v[1], v[2]))
                .unwrap_or(Vec3::ZERO);
            let quat = extract_vec4(body, "quaternion")
                .map(|q| Quat {
                    x: q[1],
                    y: q[2],
                    z: q[3],
                    w: q[0],
                })
                .unwrap_or(Quat::IDENTITY);

            nodes.push(BoneNode {
                name,
                parent: parent.map(String::from),
                is_dummy,
                pivot,
                quat,
            });
        }

        rest = after;
    }

    let mut bones: Vec<BoneNode> = nodes.iter().filter(|n| !n.is_dummy).cloned().collect();
    let mut dummies: Vec<BoneNode> = nodes.iter().filter(|n| n.is_dummy).cloned().collect();

    for node in &mut nodes {
        if let Some(ref parent) = node.parent {
            if let Some(resolved) = resolve_alias(parent, &bones, &dummies) {
                node.parent = Some(resolved.to_string());
            }
        }
    }
    for node in &mut nodes {
        let p = node.parent.clone();
        if node.is_dummy {
            if let Some(d) = dummies.iter_mut().find(|d| d.name == node.name) {
                d.parent.clone_from(&p);
            }
        } else if let Some(b) = bones.iter_mut().find(|b| b.name == node.name) {
            b.parent.clone_from(&p);
        }
    }

    BoneData {
        bones,
        dummies,
        all: nodes,
    }
}

fn extract_field<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    if let Some(pos) = body.find(key) {
        let rest = &body[pos + key.len()..];
        let v = rest.trim();
        if !v.is_empty() {
            return Some(v.split_whitespace().next().unwrap_or(v));
        }
    }
    None
}

fn extract_vec3(body: &str, key: &str) -> Option<[f32; 3]> {
    let vals: Vec<f32> = extract_numbers(body, key);
    if vals.len() >= 3 {
        Some([vals[0], vals[1], vals[2]])
    } else {
        None
    }
}

fn extract_vec4(body: &str, key: &str) -> Option<[f32; 4]> {
    let vals: Vec<f32> = extract_numbers(body, key);
    if vals.len() >= 4 {
        Some([vals[0], vals[1], vals[2], vals[3]])
    } else {
        None
    }
}

fn extract_numbers(body: &str, key: &str) -> Vec<f32> {
    if let Some(pos) = body.find(key) {
        let rest = &body[pos + key.len()..];
        return rest
            .split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .filter_map(|s| {
                let s = s.trim();
                if s.is_empty() {
                    None
                } else {
                    s.parse().ok()
                }
            })
            .collect();
    }
    vec![]
}

fn resolve_alias<'a>(
    alias: &str,
    bones: &'a [BoneNode],
    dummies: &'a [BoneNode],
) -> Option<&'a str> {
    let alias = alias.trim();
    if let Some(rest) = alias.strip_prefix('b') {
        let idx: usize = rest.parse().ok()?;
        bones.get(idx).map(|b| b.name.as_str())
    } else if let Some(rest) = alias.strip_prefix('d') {
        let idx: usize = rest.parse().ok()?;
        dummies.get(idx).map(|d| d.name.as_str())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic() {
        let bnc = "Bones: 30 Dummey: 6\nbone b {\n  parent NULL\n  pivot 0 0 0\n  quaternion 0 0 0 1\n}\nbone b1 {\n  parent b\n  pivot 0 0 50\n  quaternion 0 0 0 1\n}\n";
        let d = parse_bones(bnc);
        assert_eq!(d.bones.len(), 2);
        assert_eq!(d.bones[0].name, "b");
        assert!(d.bones[0].parent.is_none());
        assert_eq!(d.bones[1].name, "b1");
        assert_eq!(d.bones[1].parent.as_deref(), Some("b"));
    }

    #[test]
    fn test_dummy_single_line() {
        let bnc = "Bones: 30 Dummey: 6\nbone b { parent NULL pivot 0 0 0 quaternion 0 0 0 1 }\nDummey d_wpnL { parent b pivot 10 0 0 quaternion 0 0 0 1 }\nDummey d_wpnR { parent d0 pivot -10 0 0 quaternion 0 0 0.707 0.707 }\n";
        let d = parse_bones(bnc);
        assert_eq!(d.bones.len(), 1);
        assert_eq!(d.dummies.len(), 2);
        assert_eq!(d.dummies[1].name, "d_wpnR");
        assert_eq!(d.dummies[1].parent.as_deref(), Some("d_wpnL"));
    }

    #[test]
    fn test_quat_convert() {
        let bnc = "Bones: 30 Dummey: 6\nbone b { parent NULL pivot 0 0 0 quaternion 0.707 0.0 0.0 0.707 }\n";
        let d = parse_bones(bnc);
        let b = &d.bones[0];
        assert!((b.quat.x - 0.0).abs() < 0.01);
        assert!((b.quat.y - 0.0).abs() < 0.01);
        assert!((b.quat.z - 0.707).abs() < 0.01);
        assert!((b.quat.w - 0.707).abs() < 0.01);
    }
}
