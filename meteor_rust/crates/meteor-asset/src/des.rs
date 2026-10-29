use meteor_core::{Quat, Vec3};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone)]
pub struct StageObject {
    pub name: String,

    pub position: Vec3,

    pub quaternion: Quat,

    pub custom: String,

    pub texture_animation: TextureAnimation,

    pub model: Option<String>,

    pub resource_key: Option<[u32; 2]>,
    pub object_type: Option<String>,
    pub name_attr: Option<String>,
    pub ticket: Option<String>,
    pub effect: Option<String>,
    pub billboard: i32,
    pub visible: bool,

    pub collision_attr: Option<u8>,
    pub block_player: bool,
    pub deform: Option<Deform>,
    pub damage_name: u32,
    pub is_hell: bool,
    pub scale: f32,
    pub tex_scope: Option<String>,

    pub is_dummy: bool,
}

impl StageObject {
    pub fn skip_render(&self) -> bool {
        !self.visible || regex_contains(&self.custom, r"objecttype\s*=\s*camerabox")
    }
}

#[derive(Debug, Clone, Default)]
pub struct TextureAnimation {
    pub enabled: bool,
    pub u: f32,
    pub v: f32,
}

#[derive(Debug, Clone)]
pub struct Deform {
    pub animation: String,
    pub size: f32,
    pub freq: f32,
    pub range: f32,
    pub r#ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StageDescription {
    pub scene_object_count: usize,
    pub dummy_count: usize,
    pub scene_objects: Vec<StageObject>,
    pub dummies: Vec<StageObject>,

    pub block_map: BTreeMap<String, bool>,
}

pub fn parse_stage_description(text: &str) -> StageDescription {
    let mut scene_objects = Vec::new();
    let mut dummies = Vec::new();
    let mut block_map = BTreeMap::new();

    let mut scene_count = None;
    let mut dummy_count = None;
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("SceneObjects") {
            let nums: Vec<usize> = rest
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            if nums.len() >= 2 {
                scene_count = Some(nums[0]);
                dummy_count = Some(nums[1]);
            }
            break;
        }
    }

    let mut pos = 0usize;
    while let Some(open) = find_object_open(text, pos) {
        let name = open.name.clone();
        let body = open.body.clone();
        pos = open.next;

        if let Some(obj) = parse_body(&name, &body) {
            if obj.is_dummy {
                dummies.push(obj);
            } else {
                block_map.insert(name, obj.block_player);
                scene_objects.push(obj);
            }
        }
    }

    StageDescription {
        scene_object_count: scene_count.unwrap_or(scene_objects.len()),
        dummy_count: dummy_count.unwrap_or(dummies.len()),
        scene_objects,
        dummies,
        block_map,
    }
}

pub fn parse_object_positions(text: &str) -> Vec<(String, [f32; 3])> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(open) = find_object_open(text, pos) {
        let name = open.name.clone();
        let body = open.body.clone();
        pos = open.next;
        if let Some(line) = body
            .lines()
            .find(|line| line.trim_start().starts_with("Position"))
        {
            let nums: Vec<f32> = line
                .split_whitespace()
                .filter_map(|part| part.parse().ok())
                .collect();
            if nums.len() >= 3 {
                out.push((name, [nums[0], nums[1], nums[2]]));
            }
        }
    }
    out
}

pub fn parse_object_visibility(text: &str) -> HashMap<String, bool> {
    let mut out = HashMap::new();
    let mut pos = 0usize;
    while let Some(open) = find_object_open(text, pos) {
        pos = open.next;
        out.insert(
            open.name.to_ascii_lowercase(),
            !regex_contains(&open.body, r"visible\s*=\s*0\b"),
        );
    }
    out
}

fn find_object_open(text: &str, from: usize) -> Option<ObjectOpen> {
    let rest = &text[from..];
    let mut search_from = 0usize;
    loop {
        let idx = rest[search_from..].find("Object ")? + search_from;
        let after = &rest[idx + 7..];

        let name_end = after
            .find(|c: char| c == '{' || c.is_whitespace())
            .unwrap_or(after.len());
        let name = after[..name_end].trim().to_string();
        if name.is_empty() {
            search_from = idx + 7;
            continue;
        }

        let brace = after.find('{')?;
        let body_start = brace + 1;
        let body_end = find_matching_brace(&after[body_start..])?;
        let body = after[body_start..body_start + body_end].to_string();

        return Some(ObjectOpen {
            name,
            body,
            next: from + idx + 7 + body_start + body_end + 1,
        });
    }
}

struct ObjectOpen {
    name: String,
    body: String,
    next: usize,
}

fn find_matching_brace(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth < 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_body(name: &str, body: &str) -> Option<StageObject> {
    let pos = extract_vec3(body, "Position:")?;

    let quat = extract_vec4(body, "Quaternion:")
        .map(|q| Quat {
            x: q[1],
            y: q[2],
            z: q[3],
            w: q[0],
        })
        .unwrap_or(Quat::IDENTITY);

    let custom = extract_custom(body).unwrap_or_default();

    let fields = parse_custom_fields(&custom);

    Some(StageObject {
        name: name.to_string(),
        position: Vec3::new(pos[0], pos[1], pos[2]),
        quaternion: quat,
        custom: custom.clone(),
        texture_animation: parse_texture_animation(body),
        model: fields.model,
        resource_key: fields.resource_key,
        object_type: fields.object_type,
        name_attr: fields.name_attr,
        ticket: fields.ticket,
        effect: fields.effect,
        billboard: fields.billboard,
        visible: fields.visible,
        collision_attr: fields.collision_attr,
        block_player: fields.block_player,
        deform: fields.deform,
        damage_name: fields.damage_name,
        is_hell: fields.is_hell,
        scale: fields.scale,
        tex_scope: fields.tex_scope,
        is_dummy: name.starts_with("D_"),
    })
}

#[derive(Debug, Clone, Default)]
struct CustomFields {
    model: Option<String>,
    resource_key: Option<[u32; 2]>,
    object_type: Option<String>,
    name_attr: Option<String>,
    ticket: Option<String>,
    effect: Option<String>,
    billboard: i32,
    visible: bool,
    collision_attr: Option<u8>,
    block_player: bool,
    deform: Option<Deform>,
    damage_name: u32,
    is_hell: bool,
    scale: f32,
    tex_scope: Option<String>,
}

fn parse_custom_fields(custom: &str) -> CustomFields {
    let model = regex_get(custom, r#"model\s*=\s*"([^"]+)""#).map(String::from);
    let resource_key =
        regex_get(custom, r#"resource\s*=\s*"?([0-9]+\s*,\s*[0-9]+)"?"#).and_then(|value| {
            let (category, id) = value.split_once(',')?;
            Some([
                category.trim().parse().ok()?,
                id.split_whitespace().next()?.parse().ok()?,
            ])
        });
    let object_type = regex_get(custom, r"ObjectType\s*=\s*(\S+)").map(String::from);
    let name_attr = regex_get(custom, r#"name\s*=\s*"([^"]+)""#)
        .map(String::from)
        .or_else(|| regex_get(custom, r"name\s*=\s*(\S+)").map(String::from));
    let ticket = regex_get(custom, r#"ticket\s*=\s*"([^"]+)""#)
        .map(String::from)
        .or_else(|| regex_get(custom, r"ticket\s*=\s*([^\s}]+)").map(String::from));
    let effect = regex_get(custom, r#"effect\s*=\s*"([^"]+)""#).map(String::from);
    let billboard = regex_get(custom, r"billboard\s*=\s*(\d+)")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let visible = !regex_contains(custom, r"visible\s*=\s*0\b");
    let collision_attr = if regex_contains(custom, r"collision\s*=\s*0\b") {
        Some(0)
    } else if regex_contains(custom, r"collision\s*=\s*1\b") {
        Some(1)
    } else {
        None
    };
    let block_player = !regex_contains(custom, r"blockplayer\s*=\s*no");
    let anim = regex_get(custom, r"animation\s*=\s*(\S+)").map(String::from);
    let deform = anim
        .as_deref()
        .filter(|a| a.to_lowercase().contains("sin"))
        .map(|a| Deform {
            animation: a.to_string(),
            size: regex_get(custom, r"deformsize\s*=\s*([-\d.]+)")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            freq: regex_get(custom, r"deformfreq\s*=\s*([-\d.]+)")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            range: regex_get(custom, r"deformrange\s*=\s*([-\d.]+)")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            r#ref: regex_get(custom, r#"deformref\s*=\s*"([^"]+)""#)
                .map(String::from)
                .or_else(|| regex_get(custom, r"deformref\s*=\s*(\S+)").map(String::from)),
        });
    let damage_name = name_attr
        .as_deref()
        .and_then(|n| {
            let lower = n.to_lowercase();
            lower.strip_prefix("damage").and_then(|d| d.parse().ok())
        })
        .unwrap_or(0);
    let is_hell = name_attr
        .as_deref()
        .map(|n| n.eq_ignore_ascii_case("hell"))
        .unwrap_or(false);
    let scale = regex_get(custom, r"scale\s*=\s*([\d.]+)")
        .and_then(|s| s.parse::<f32>().ok())
        .map(|v| if v == 0.0 { 1.0 } else { v })
        .unwrap_or(1.0);
    let tex_scope = regex_get(custom, r#"texscope\s*=\s*"?(sn\d{2})"? "#).map(|s| s.to_lowercase());

    CustomFields {
        model,
        resource_key,
        object_type,
        name_attr,
        ticket,
        effect,
        billboard,
        visible,
        collision_attr,
        block_player,
        deform,
        damage_name,
        is_hell,
        scale,
        tex_scope,
    }
}

fn parse_texture_animation(body: &str) -> TextureAnimation {
    let nums: Vec<f32> = body
        .lines()
        .find_map(|l| {
            let l = l.trim();
            let lower = l.to_lowercase();
            if let Some(rest) = lower.strip_prefix("textureanimation:") {
                rest.split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect::<Vec<f32>>()
                    .into_some()
            } else {
                None
            }
        })
        .unwrap_or_default();

    if nums.len() >= 3 {
        TextureAnimation {
            enabled: nums[0] != 0.0,
            u: nums[1],
            v: nums[2],
        }
    } else {
        TextureAnimation::default()
    }
}

fn extract_vec3(body: &str, key: &str) -> Option<[f32; 3]> {
    let nums = extract_numbers_after(body, key)?;
    if nums.len() >= 3 {
        Some([nums[0], nums[1], nums[2]])
    } else {
        None
    }
}

fn extract_vec4(body: &str, key: &str) -> Option<[f32; 4]> {
    let nums = extract_numbers_after(body, key)?;
    if nums.len() >= 4 {
        Some([nums[0], nums[1], nums[2], nums[3]])
    } else {
        None
    }
}

fn extract_numbers_after(body: &str, key: &str) -> Option<Vec<f32>> {
    let lower = body.to_lowercase();
    let key_lower = key.to_lowercase();
    let idx = lower.find(&key_lower)?;
    let rest = &body[idx + key_lower.len()..];
    let vals: Vec<f32> = rest
        .chars()
        .take_while(|c| !c.is_ascii_alphabetic() && *c != '{' && *c != '}' && *c != ':')
        .collect::<String>()
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    if vals.is_empty() {
        None
    } else {
        Some(vals)
    }
}

fn extract_custom(body: &str) -> Option<String> {
    let lower = body.to_lowercase();
    let idx = lower.find("custom:")?;
    let rest = &body[idx + 7..];
    let brace = rest.find('{')?;
    let inner = &rest[brace + 1..];
    let end = inner.find('}')?;
    Some(inner[..end].to_string())
}

fn regex_get<'a>(text: &'a str, pattern: &str) -> Option<&'a str> {
    let eq = pattern.find('\\').unwrap_or(pattern.len());
    let key_part = &pattern[..eq];
    let key = key_part.trim_end_matches('\\');
    let lower = text.to_ascii_lowercase();
    let key_lower = key.to_lowercase();
    let value = lower.match_indices(&key_lower).find_map(|(idx, _)| {
        if idx > 0 {
            let prev = text.as_bytes()[idx - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' {
                return None;
            }
        }
        text[idx + key_lower.len()..]
            .trim_start()
            .strip_prefix('=')
            .map(str::trim_start)
    })?;

    let value = if let Some(quoted) = value.strip_prefix('"') {
        quoted.split('"').next()?
    } else if key.eq_ignore_ascii_case("resource") {
        let (category, rest) = value.split_once(',')?;
        category.trim().parse::<u32>().ok()?;
        let rest = rest.trim_start();
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        &value[..value.len() - rest.len() + digits]
    } else {
        value
            .split(|c: char| c.is_whitespace() || c == '}')
            .next()?
    };
    let value = value.trim().trim_end_matches(',');
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn regex_contains(text: &str, pattern: &str) -> bool {
    let compact = pattern.replace(r"\s*", "").replace(r"\b", "");
    let Some((key, val)) = compact.split_once('=') else {
        return text.to_lowercase().contains(&pattern.to_lowercase());
    };
    custom_has_kv(text, key, val)
}

fn custom_has_kv(text: &str, key: &str, value: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let key = key.to_ascii_lowercase();
    let value = value.to_ascii_lowercase();
    let mut start = 0usize;
    while let Some(rel) = lower[start..].find(key.as_str()) {
        let abs = start + rel;

        if abs > 0 {
            let prev = lower.as_bytes()[abs - 1] as char;
            if prev.is_ascii_alphanumeric() || prev == '_' {
                start = abs + 1;
                continue;
            }
        }
        let after = lower[abs + key.len()..].trim_start();
        let Some(rest) = after.strip_prefix('=') else {
            start = abs + 1;
            continue;
        };
        let rest = rest.trim_start().trim_start_matches('"');
        if rest.starts_with(value.as_str()) {
            let after_val = &rest[value.len()..];
            if after_val.is_empty()
                || after_val.starts_with(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            {
                return true;
            }
        }
        start = abs + 1;
    }
    false
}

trait IntoSome {
    fn into_some(self) -> Option<Self>
    where
        Self: Sized;
}

impl<T> IntoSome for Vec<T> {
    fn into_some(self) -> Option<Self> {
        if self.is_empty() {
            None
        } else {
            Some(self)
        }
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_sn05_deformation_parameters_are_not_swallowed_by_following_fields() {
        let text = include_str!("../../../../origin_res/sn05/sn05.des");
        let desc = parse_stage_description(text);
        let animated: Vec<_> = desc
            .scene_objects
            .iter()
            .filter_map(|o| o.deform.as_ref())
            .collect();
        assert_eq!(animated.len(), 7);
        for deform in animated {
            assert_eq!(deform.animation, "sin");
            assert_eq!(deform.size, 0.5);
            assert_eq!(deform.freq, 0.25);
            assert_eq!(deform.range, 0.5);
            assert_eq!(deform.r#ref, None);
        }
    }

    #[test]
    fn explicit_resource_identity_is_independent_of_model_name() {
        let fields = parse_custom_fields("model=\"RelicMesh\" resource=4,2 ticket=2,6");
        assert_eq!(fields.model.as_deref(), Some("RelicMesh"));
        assert_eq!(fields.resource_key, Some([4, 2]));
        assert_eq!(fields.ticket.as_deref(), Some("2,6"));
        assert_eq!(parse_custom_fields("model=\"IPItem02\"").resource_key, None);
        assert_eq!(
            parse_custom_fields("resource=4, 2 ticket=2,6").resource_key,
            Some([4, 2])
        );
    }

    #[test]
    fn custom_bare_values_stop_at_field_boundaries() {
        let fields = parse_custom_fields(
            "animation=sin deformsize=0.5 deformfreq=0.25 deformrange=0.5 deformref=xz\n\
             billboard=1 scale=2 texscope=sn05 name=Damage2 ticket=2,6 model=\"a model\"",
        );
        let deform = fields.deform.unwrap();
        assert_eq!(deform.animation, "sin");
        assert_eq!((deform.size, deform.freq, deform.range), (0.5, 0.25, 0.5));
        assert_eq!(deform.r#ref.as_deref(), Some("xz"));
        assert_eq!(fields.billboard, 1);
        assert_eq!(fields.scale, 2.0);
        assert_eq!(fields.tex_scope.as_deref(), Some("sn05"));
        assert_eq!(fields.name_attr.as_deref(), Some("Damage2"));
        assert_eq!(fields.ticket.as_deref(), Some("2,6"));
        assert_eq!(fields.model.as_deref(), Some("a model"));
        assert!(parse_custom_fields("notdeformsize=9 deformsize_extra=8")
            .deform
            .is_none());
        assert_eq!(
            regex_get("notname=bad name_extra=bad name=good", r"name\s*=\s*(\S+)"),
            Some("good")
        );
    }

    #[test]
    fn test_parse_basic() {
        let des = r#"# GModel Description File V1.0
SceneObjects 2 DummeyObjects 1

Object di01
{
  Position: 100 200 300
  Quaternion: -1 0 0 0
  Custom:
  {
    blockplayer=no
  }
}
Object di02 { Position: 0 0 0 Quaternion: 1 0 0 0 }
Object D_St01
{
  Position: 50 60 70
  Quaternion: 1 0 0 0
  Custom:
  {
    model="sn01st02"
  }
}
"#;
        let desc = parse_stage_description(des);
        assert_eq!(desc.scene_object_count, 2);
        assert_eq!(desc.dummy_count, 1);
        assert_eq!(desc.scene_objects.len(), 2);
        assert_eq!(desc.dummies.len(), 1);

        assert_eq!(desc.block_map.get("di01"), Some(&false));
        assert_eq!(desc.block_map.get("di02"), Some(&true));

        assert_eq!(desc.dummies[0].model.as_deref(), Some("sn01st02"));

        let p = &desc.scene_objects[0].position;
        assert_eq!(p.x, 100.0);
        assert_eq!(p.y, 200.0);
        assert_eq!(p.z, 300.0);

        let q = desc.scene_objects[0].quaternion;
        assert!((q.w - -1.0).abs() < 0.001);
    }

    #[test]
    fn test_no_custom_defaults() {
        let des = "SceneObjects 1 DummeyObjects 0\nObject di01 { Position: 1 2 3 }\n";
        let desc = parse_stage_description(des);
        assert_eq!(desc.scene_objects.len(), 1);
        let obj = &desc.scene_objects[0];
        assert!(obj.block_player);
        assert!(obj.visible);
        assert_eq!(obj.scale, 1.0);
        assert!(!obj.is_dummy);
    }

    #[test]
    fn collision_attr_distinguishes_zero_and_one() {
        let des = r#"SceneObjects 0 DummeyObjects 2
Object D_a { Position: 0 0 0 Quaternion: 1 0 0 0 Custom: { collision=1 } }
Object D_b { Position: 0 0 0 Quaternion: 1 0 0 0 Custom: { collision=0 } }
"#;
        let desc = parse_stage_description(des);
        assert_eq!(desc.dummies[0].collision_attr, Some(1));
        assert_eq!(desc.dummies[1].collision_attr, Some(0));
    }

    #[test]
    fn blockplayer_no_only() {
        let des = r#"SceneObjects 2 DummeyObjects 0
Object a { Position: 0 0 0 Custom: { blockplayer=no } }
Object b { Position: 0 0 0 Custom: { blockplayer=yes } }
"#;
        let desc = parse_stage_description(des);
        assert!(!desc.scene_objects[0].block_player);

        assert!(desc.scene_objects[1].block_player);
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod endpoint_tests {
    use super::*;

    #[test]
    fn parse_object_positions_extracts_blocks() {
        let text = r#"# GModel Description File V1.0
SceneObjects 2 DummeyObjects 2
Object wpnLS01
{
  Position:  0.000 -0.000  0.000
  Quaternion: -1.000 0.000 0.000 0.000
}
Object d_wpnRS
{
  Position:  0.000 -1.696  0.000
  Quaternion: -1.000 0.000 0.000 0.000
}
"#;
        let positions = parse_object_positions(text);
        assert_eq!(positions.len(), 2);
        let rs = positions
            .iter()
            .find(|(name, _)| name == "d_wpnRS")
            .expect("d_wpnRS");
        assert!((rs.1[1] + 1.696).abs() < 1e-4, "y = -1.696");
    }

    #[test]
    fn origin_res_sword_des_carries_wpn_endpoints() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let path = root.join("cmodel/w0_0.des");
        if !path.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let text = std::fs::read(&path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap();
        let positions = parse_object_positions(&text);
        let wpn: Vec<&str> = positions
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| name.to_lowercase().starts_with("d_wpn"))
            .collect();
        assert!(wpn.contains(&"d_wpnRS"), "sword has d_wpnRS (got {wpn:?})");
        assert!(wpn.contains(&"d_wpnRE"), "sword has d_wpnRE");
    }
}
