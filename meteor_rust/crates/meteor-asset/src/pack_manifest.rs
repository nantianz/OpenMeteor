use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct PackDescriptor {
    #[allow(dead_code)]
    pub url: String,
    #[allow(dead_code)]
    pub file_name: String,
    pub size: usize,
    pub hash: String,
    pub requires: Vec<String>,
}

pub fn parse_pack_index(text: &str) -> Result<HashMap<String, PackDescriptor>, String> {
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("packs.json: {error}"))?;
    if root.get("formatVersion").and_then(|value| value.as_u64()) != Some(1) {
        return Err("packs.json: unsupported formatVersion".to_string());
    }
    let packs = root
        .get("packs")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "packs.json: packs must be an array".to_string())?;
    let mut descriptors = HashMap::with_capacity(packs.len());
    for value in packs {
        let id = value
            .get("packId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "packs.json: packId missing".to_string())?;
        let url = value
            .get("url")
            .and_then(|value| value.as_str())
            .ok_or_else(|| format!("packs.json: {id} url missing"))?;
        let file_name = value
            .get("fileName")
            .and_then(|value| value.as_str())
            .or_else(|| url.rsplit('/').next().filter(|value| !value.is_empty()))
            .ok_or_else(|| format!("packs.json: {id} fileName missing"))?;
        if file_name.contains(['/', '\\', ':', '\0'])
            || file_name.starts_with('.')
            || !file_name.ends_with(".mpk")
        {
            return Err(format!("packs.json: unsafe file name {file_name}"));
        }
        let size = value
            .get("size")
            .and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| format!("packs.json: {id} size invalid"))?;
        let hash = value
            .get("hash")
            .and_then(|value| value.as_str())
            .filter(|value| {
                value.len() == 71
                    && value.starts_with("sha256:")
                    && value[7..].chars().all(|digit| digit.is_ascii_hexdigit())
            })
            .ok_or_else(|| format!("packs.json: {id} hash invalid"))?;
        let requires = value
            .get("requires")
            .and_then(|value| value.as_array())
            .ok_or_else(|| format!("packs.json: {id} requires invalid"))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("packs.json: {id} requires non-string"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if descriptors
            .insert(
                id.to_string(),
                PackDescriptor {
                    url: url.to_string(),
                    file_name: file_name.to_string(),
                    size,
                    hash: hash.to_ascii_lowercase(),
                    requires,
                },
            )
            .is_some()
        {
            return Err(format!("packs.json: duplicate packId {id}"));
        }
    }
    for id in descriptors.keys() {
        validate_dependency(id, &descriptors, &mut HashSet::new(), &mut HashSet::new())?;
    }
    Ok(descriptors)
}

fn validate_dependency(
    id: &str,
    descriptors: &HashMap<String, PackDescriptor>,
    active: &mut HashSet<String>,
    done: &mut HashSet<String>,
) -> Result<(), String> {
    if done.contains(id) {
        return Ok(());
    }
    if !active.insert(id.to_string()) {
        return Err(format!("packs.json: dependency cycle at {id}"));
    }
    let descriptor = descriptors
        .get(id)
        .ok_or_else(|| format!("packs.json: required pack {id} missing"))?;
    for dependency in &descriptor.requires {
        validate_dependency(dependency, descriptors, active, done)?;
    }
    active.remove(id);
    done.insert(id.to_string());
    Ok(())
}

fn visit_pack(
    id: &str,
    descriptors: &HashMap<String, PackDescriptor>,
    visited: &mut HashSet<String>,
    ordered: &mut Vec<String>,
) -> Result<(), String> {
    if !visited.insert(id.to_string()) {
        return Ok(());
    }
    let descriptor = descriptors
        .get(id)
        .ok_or_else(|| format!("packs.json: required pack {id} missing"))?;
    for dependency in &descriptor.requires {
        visit_pack(dependency, descriptors, visited, ordered)?;
    }
    ordered.push(id.to_string());
    Ok(())
}

pub fn resolve_pack_order(
    descriptors: &HashMap<String, PackDescriptor>,
    roots: &[String],
) -> Result<Vec<String>, String> {
    let mut visited = HashSet::new();
    let mut ordered = Vec::new();
    for root in roots {
        visit_pack(root, descriptors, &mut visited, &mut ordered)?;
    }
    Ok(ordered)
}

pub fn network_model_pack_roots(characters: &[u8]) -> Vec<String> {
    let mut roots = vec!["common".into(), "textures".into()];
    for id in characters {
        let root = format!("char/p{id}");
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

pub fn match_pack_roots(stage_id: &str) -> Vec<String> {
    let mut roots = vec![
        "common".to_string(),
        "textures".to_string(),
        "sound".to_string(),
        "music".to_string(),
    ];

    roots.extend((0..20).map(|id| format!("char/p{id}")));
    roots.push(format!("stage/{stage_id}"));
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_model_packs_are_roster_scoped_and_deduplicated() {
        assert_eq!(
            network_model_pack_roots(&[0, 1, 0]),
            ["common", "textures", "char/p0", "char/p1"]
        );
    }

    #[test]
    fn dependency_order_is_stable_and_deduplicated() {
        let descriptors = parse_pack_index(
            r#"{"formatVersion":1,"packs":[
              {"packId":"common","url":"/common.mpk","fileName":"common.mpk","size":1,"hash":"sha256:0000000000000000000000000000000000000000000000000000000000000000","requires":[]},
              {"packId":"textures","url":"/textures.mpk","fileName":"textures.mpk","size":2,"hash":"sha256:1111111111111111111111111111111111111111111111111111111111111111","requires":[]},
              {"packId":"stage/sn01","url":"/sn01.mpk","fileName":"sn01.mpk","size":3,"hash":"sha256:2222222222222222222222222222222222222222222222222222222222222222","requires":["common","textures"]}
            ]}"#,
        )
        .unwrap();
        let ordered = resolve_pack_order(
            &descriptors,
            &["stage/sn01".to_string(), "common".to_string()],
        )
        .unwrap();
        assert_eq!(ordered, ["common", "textures", "stage/sn01"]);
        let stage = &descriptors["stage/sn01"];
        assert_eq!(stage.url, "/sn01.mpk");
        assert_eq!(stage.file_name, "sn01.mpk");
        assert_eq!(stage.size, 3);
        assert!(stage.hash.starts_with("sha256:"));
        let roots = match_pack_roots("sn01");
        assert!(roots.contains(&"char/p19".to_string()));
        assert_eq!(roots.last().map(String::as_str), Some("stage/sn01"));
    }
}
