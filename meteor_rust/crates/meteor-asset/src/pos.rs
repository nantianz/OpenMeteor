use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct Pose {
    pub id: u16,

    pub source: u8,
    pub start: u32,
    pub end: u32,
    pub loop_start: u32,
    pub loop_end: u32,

    pub loops: bool,

    pub link: Option<u16>,
    pub note: String,

    pub effect_type: Option<i32>,
    pub effect_id: Option<EffectId>,

    pub attacks: Vec<AttackBlock>,

    pub next_pose: Option<NextPoseBlock>,

    pub blends: Vec<SpeedBlock>,

    pub actions: Vec<SpeedBlock>,

    pub action_entry_frame: u32,

    pub drags: Vec<DragBlock>,
}

#[derive(Debug, Clone)]
pub enum EffectId {
    Int(i32),
    Str(String),
}

#[derive(Debug, Clone)]
pub struct AttackBlock {
    pub start: u32,
    pub end: u32,
    pub attack_type: u32,
    pub check_friend: i32,
    pub defense_value: f32,
    pub defense_move: f32,
    pub target_value: f32,
    pub target_move: f32,
    pub target_pose: Option<u16>,
    pub target_pose_front: Option<u16>,
    pub target_pose_back: Option<u16>,
    pub target_pose_left: Option<u16>,
    pub target_pose_right: Option<u16>,

    pub bones: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct NextPoseBlock {
    pub start: Option<u32>,
    pub end: Option<u32>,
    pub time: f32,
}

#[derive(Debug, Clone)]
pub struct SpeedBlock {
    pub start: Option<u32>,
    pub end: Option<u32>,
    pub speed: f32,
}

#[derive(Debug, Clone)]
pub struct DragBlock {
    pub start: Option<u32>,
    pub end: Option<u32>,
    pub time: f32,
    pub color: [u8; 3],
}

pub fn parse_poses(text: &str) -> Vec<Pose> {
    let mut poses: BTreeMap<u16, Pose> = BTreeMap::new();

    let mut start = 0;
    let bytes = text.as_bytes();
    let mut parts: Vec<&str> = Vec::new();

    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' && i + 1 < bytes.len() && bytes[i + 1..].starts_with(b"Pose ") {
            parts.push(&text[start..i]);
            start = i + 1;
        }
    }

    if start < text.len() {
        parts.push(&text[start..]);
    }

    for part in &parts {
        let head = part.lines().next().unwrap_or("");
        let (id, note) = parse_pose_header(head);
        if id < 0 {
            continue;
        }

        let body = top_level_body(part);
        let Some(body) = body else { continue };

        let (fields, blocks) = parse_body(body);

        let start_frame = int_field(&fields, "start").unwrap_or(0);
        let end_frame = int_field(&fields, "end").unwrap_or(0);
        if end_frame < start_frame {
            continue;
        }

        let has_loop = fields.contains_key("loopstart") && fields.contains_key("loopend");
        let ls = int_field(&fields, "loopstart")
            .unwrap_or(start_frame)
            .clamp(start_frame, end_frame);
        let le = int_field(&fields, "loopend")
            .unwrap_or(end_frame)
            .max(ls)
            .min(end_frame);

        let source = int_field(&fields, "source").unwrap_or(0) as u8;
        let link = int_field(&fields, "link").map(|v| v as u16);

        let attacks: Vec<AttackBlock> = blocks
            .get("Attack")
            .map(|v| v.iter().map(|b| parse_attack_block(b)).collect())
            .unwrap_or_default();

        let next_pose = blocks
            .get("NextPose")
            .and_then(|v| v.first())
            .map(|b| parse_next_pose_block(b));

        let blends: Vec<SpeedBlock> = blocks
            .get("Blend")
            .map(|v| v.iter().map(|b| parse_speed_block(b)).collect())
            .unwrap_or_default();

        let actions: Vec<SpeedBlock> = blocks
            .get("Action")
            .map(|v| v.iter().map(|b| parse_speed_block(b)).collect())
            .unwrap_or_default();

        let drags: Vec<DragBlock> = blocks
            .get("Drag")
            .map(|v| v.iter().filter_map(|b| parse_drag_block(b)).collect())
            .unwrap_or_default();

        let action_entry = compute_action_entry_frame(start_frame, end_frame, &actions);

        let effect_type = fields.get("effecttype").and_then(|v| v.parse::<i32>().ok());
        let effect_id = scalar_field(&fields, "effectid");

        poses.insert(
            id as u16,
            Pose {
                id: id as u16,
                source,
                start: start_frame,
                end: end_frame,
                loop_start: ls,
                loop_end: le,
                loops: has_loop,
                link,
                note: note.to_string(),
                effect_type,
                effect_id,
                attacks,
                next_pose,
                blends,
                actions,
                action_entry_frame: action_entry,
                drags,
            },
        );
    }

    poses.into_values().collect()
}

fn parse_pose_header(line: &str) -> (i32, &str) {
    let line = line.trim();
    if !line.starts_with("Pose ") {
        return (-1, "");
    }
    let rest = &line[5..];
    let space = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let id: i32 = rest[..space].parse().unwrap_or(-1);
    let note = rest[space..].trim();
    (id, note)
}

fn top_level_body(part: &str) -> Option<&str> {
    let brace = part.find('{')?;
    let mut depth = 0;
    for (i, c) in part[brace..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&part[brace + 1..brace + i]);
                }
            }
            _ => {}
        }
    }
    None
}

type Fields = BTreeMap<String, String>;
type Blocks = BTreeMap<String, Vec<String>>;

const BLOCK_NAMES: &[&str] = &["Attack", "NextPose", "Blend", "Action", "Drag"];

fn parse_body(body: &str) -> (Fields, Blocks) {
    let mut fields = Fields::new();
    let mut blocks = Blocks::new();
    let mut block_name: Option<String> = None;
    let mut block_lines: Vec<String> = Vec::new();
    let mut depth: i32 = 0;
    let mut pending: Option<String> = None;

    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        if depth == 0 {
            if let Some(ref pn) = pending {
                if line == "{" || line.starts_with('{') {
                    let opens = line.matches('{').count() as i32;
                    let closes = line.matches('}').count() as i32;
                    depth = opens - closes;
                    block_name = Some(pn.clone());
                    if depth <= 0 {
                        push_block(&mut blocks, &block_name, "");
                        block_name = None;
                    }
                    pending = None;
                    continue;
                }
                pending = None;
            }

            if let Some(name) = starts_with_block(line) {
                if let Some(rest) = line.strip_prefix(name) {
                    let rest = rest.trim();
                    if rest == "{" || rest.starts_with('{') {
                        let opens = line.matches('{').count() as i32;
                        let closes = line.matches('}').count() as i32;
                        depth = opens - closes;
                        block_name = Some(name.to_string());
                        if depth <= 0 {
                            push_block(&mut blocks, &block_name, "");
                            block_name = None;
                        }
                        continue;
                    }
                }

                if is_standalone_block(line) {
                    pending = Some(name.to_string());
                    continue;
                }
            }

            let fm = parse_top_field(line);
            if let Some((k, v)) = fm {
                fields.entry(k).or_insert(v);
            }
        } else {
            let opens = line.matches('{').count() as i32;
            let closes = line.matches('}').count() as i32;
            depth += opens - closes;
            if depth <= 0 {
                let before = line.rsplit_once('}').map(|(b, _)| b).unwrap_or(line);
                if !before.trim().is_empty() {
                    block_lines.push(before.to_string());
                }
                push_block(&mut blocks, &block_name, &block_lines.join("\n"));
                block_name = None;
                block_lines.clear();
            } else {
                block_lines.push(line.to_string());
            }
        }
    }

    (fields, blocks)
}

fn starts_with_block(line: &str) -> Option<&str> {
    BLOCK_NAMES
        .iter()
        .find(|&&n| {
            line.starts_with(n)
                && (line.len() == n.len() || line.as_bytes()[n.len()].is_ascii_whitespace())
        })
        .copied()
}

fn is_standalone_block(line: &str) -> bool {
    BLOCK_NAMES.contains(&line)
}

fn push_block(blocks: &mut Blocks, name: &Option<String>, content: &str) {
    if let Some(ref n) = name {
        blocks
            .entry(n.clone())
            .or_default()
            .push(content.to_string());
    }
}

fn parse_top_field(line: &str) -> Option<(String, String)> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let key = parts.next()?.to_lowercase();
    let val = parts.next()?.trim().to_string();
    if key.is_empty() || val.is_empty() {
        None
    } else {
        Some((key, val))
    }
}

fn parse_attack_block(body: &str) -> AttackBlock {
    let mut atk = AttackBlock {
        start: 0,
        end: 0,
        attack_type: 0,
        check_friend: 0,
        defense_value: 0.0,
        defense_move: 0.0,
        target_value: 0.0,
        target_move: 0.0,
        target_pose: None,
        target_pose_front: None,
        target_pose_back: None,
        target_pose_left: None,
        target_pose_right: None,
        bones: Vec::new(),
    };

    let mut in_bone = false;
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        if in_bone && line.starts_with('"') {
            for (index, part) in line.split('"').enumerate() {
                let p = part.trim();
                if index % 2 == 1 && !p.is_empty() {
                    atk.bones.push(p.to_string());
                }
            }
            if !line.ends_with(',') {
                in_bone = false;
            }
            continue;
        }
        in_bone = false;

        let lower = line.to_ascii_lowercase();
        if line.starts_with("BoneName") || lower.starts_with("bone ") || lower.starts_with("bone=")
        {
            in_bone = true;

            for (index, part) in line.split('"').enumerate() {
                let p = part.trim();
                if index % 2 == 1 && !p.is_empty() {
                    atk.bones.push(p.to_string());
                }
            }
            if !line.ends_with(',') {
                in_bone = false;
            }
            continue;
        }

        let m = parse_top_field(line);
        if let Some((key, val)) = m {
            let n: f32 = val.parse().unwrap_or(0.0);
            match key.as_str() {
                "start" => atk.start = n as u32,
                "end" => atk.end = n as u32,
                "attacktype" => atk.attack_type = n as u32,
                "checkfriend" => atk.check_friend = n as i32,
                "defensevalue" => atk.defense_value = n,
                "defensemove" => atk.defense_move = n,
                "targetvalue" => atk.target_value = n,
                "targetmove" => atk.target_move = n,
                "targetpose" => atk.target_pose = Some(n as u16),
                "targetposefront" => atk.target_pose_front = Some(n as u16),
                "targetposeback" => atk.target_pose_back = Some(n as u16),
                "targetposeleft" => atk.target_pose_left = Some(n as u16),
                "targetposeright" => atk.target_pose_right = Some(n as u16),
                _ => {}
            }
        }
    }

    atk
}

fn parse_next_pose_block(body: &str) -> NextPoseBlock {
    let f = simple_fields(body);
    NextPoseBlock {
        start: num_or(&f, "start"),
        end: num_or(&f, "end"),
        time: num_or_f32(&f, "time").unwrap_or(0.0),
    }
}

fn parse_speed_block(body: &str) -> SpeedBlock {
    let f = simple_fields(body);
    SpeedBlock {
        start: num_or(&f, "start"),
        end: num_or(&f, "end"),
        speed: num_or_f32(&f, "speed").unwrap_or(1.0),
    }
}

fn parse_drag_block(body: &str) -> Option<DragBlock> {
    let f = simple_fields(body);
    let start = num_or(&f, "start")?;
    let end = num_or(&f, "end")?;
    if end < start {
        return None;
    }
    let time = num_or_f32(&f, "time").unwrap_or(0.1);
    let color = parse_color(body).unwrap_or([255, 255, 198]);
    Some(DragBlock {
        start: Some(start),
        end: Some(end),
        time,
        color,
    })
}

fn parse_color(body: &str) -> Option<[u8; 3]> {
    for line in body.lines() {
        let l = line.trim().to_lowercase();
        if let Some(rest) = l.strip_prefix("color") {
            let parts: Vec<f32> = rest
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            if parts.len() >= 3 {
                return Some([parts[0] as u8, parts[1] as u8, parts[2] as u8]);
            }
        }
    }
    None
}

fn simple_fields(body: &str) -> Fields {
    let mut map = Fields::new();
    for line in body.lines() {
        let l = line.trim();
        if let Some((k, v)) = parse_top_field(l) {
            map.entry(k).or_insert(v);
        }
    }
    map
}

fn int_field(fields: &Fields, key: &str) -> Option<u32> {
    fields.get(key).and_then(|v| v.parse().ok())
}

fn scalar_field(fields: &Fields, key: &str) -> Option<EffectId> {
    let v = fields.get(key)?;
    v.parse::<i32>()
        .ok()
        .map(EffectId::Int)
        .or_else(|| Some(EffectId::Str(v.clone())))
}

fn num_or(fields: &Fields, key: &str) -> Option<u32> {
    fields.get(key).and_then(|v| v.parse().ok())
}

fn num_or_f32(fields: &Fields, key: &str) -> Option<f32> {
    fields.get(key).and_then(|v| v.parse().ok())
}

fn compute_action_entry_frame(pose_start: u32, pose_end: u32, actions: &[SpeedBlock]) -> u32 {
    let mut entry = pose_end;
    for a in actions {
        if let Some(s) = a.start {
            if s < entry {
                entry = s;
            }
        }
    }
    if entry == pose_end {
        entry = pose_start;
    }
    entry
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_pose() {
        let pos = "Pose 0\n{\n  source 0\n  Start 0\n  End 100\n}\n";
        let poses = parse_poses(pos);
        assert_eq!(poses.len(), 1);
        assert_eq!(poses[0].id, 0);
        assert_eq!(poses[0].source, 0);
        assert_eq!(poses[0].start, 0);
        assert_eq!(poses[0].end, 100);
    }

    #[test]
    fn test_parse_with_loop() {
        let pos =
            "Pose 1\n{\n  source 1\n  Start 200\n  End 400\n  LoopStart 210\n  LoopEnd 390\n}\n";
        let poses = parse_poses(pos);
        assert_eq!(poses.len(), 1);
        assert!(poses[0].loops);
        assert_eq!(poses[0].loop_start, 210);
        assert_eq!(poses[0].loop_end, 390);
    }

    #[test]
    fn test_parse_with_attack() {
        let pos = "Pose 10\n{\n  source 1\n  Start 0\n  End 50\n  Attack {\n    Start 10\n    End 30\n    AttackType 3\n    TargetValue 100\n    DefenseValue 50\n  }\n}\n";
        let poses = parse_poses(pos);
        assert_eq!(poses.len(), 1);
        assert_eq!(poses[0].attacks.len(), 1);
        let atk = &poses[0].attacks[0];
        assert_eq!(atk.start, 10);
        assert_eq!(atk.end, 30);
        assert_eq!(atk.attack_type, 3);
        assert_eq!(atk.target_value, 100.0);
    }

    #[test]
    fn test_parse_original_bone_assignment_attack() {
        let pos = "Pose 102\n{\n  Start 1927\n  End 1934\n  Attack {\n    bone = \"bau Head\", \"bau Spine1\"\n    Start 1929\n    End 1932\n  }\n}\n";
        let poses = parse_poses(pos);
        let attack = &poses[0].attacks[0];
        assert_eq!(attack.bones, ["bau Head", "bau Spine1"]);
    }

    #[test]
    fn attack_bone_trailing_comma_preserves_following_field() {
        for name in ["bone =", "BoneName"] {
            for bones in [
                "\"weapon\",",
                "\"weapon\",\n    \"bau R Forearm\",\"bau R Hand\",",
                "\"weapon\",\n    \"bau R Forearm\",\"bau R Hand\"",
            ] {
                let body = format!("{name} {bones}\n\nStart 5760\nEnd 5763\nTargetValue 0.4");
                let attack = parse_attack_block(&body);
                assert_eq!((attack.start, attack.end), (5760, 5763), "{body}");
                assert_eq!(attack.target_value, 0.4);
                let expected = if bones.contains("Forearm") {
                    vec!["weapon", "bau R Forearm", "bau R Hand"]
                } else {
                    vec!["weapon"]
                };
                assert_eq!(attack.bones, expected);
            }
        }
    }

    #[test]
    fn original_blade_single_a_attack_window() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/pmodel");
        if !root.exists() {
            return;
        }
        for character in 0..20 {
            let bytes = std::fs::read(root.join(format!("P{character}.pos"))).unwrap();

            let text = String::from_utf8_lossy(&bytes);
            let poses = parse_poses(&text);
            let blade = poses.iter().find(|pose| pose.id == 295).unwrap();
            assert_eq!(blade.attacks.len(), 1, "P{character}");
            assert_eq!(
                (blade.attacks[0].start, blade.attacks[0].end),
                (5760, 5763),
                "P{character}"
            );
        }
    }

    #[test]
    fn test_parse_with_next_pose() {
        let pos = "Pose 5\n{\n  source 1\n  Start 0\n  End 60\n  NextPose {\n    Start 40\n    End 55\n    Time 0.15\n  }\n}\n";
        let poses = parse_poses(pos);
        assert_eq!(poses.len(), 1);
        let np = poses[0].next_pose.as_ref().unwrap();
        assert_eq!(np.start, Some(40));
        assert_eq!(np.end, Some(55));
        assert!((np.time - 0.15).abs() < 0.001);
    }

    #[test]
    fn test_parse_multiple_poses() {
        let pos = "Pose 0\n{\n  Start 0\n  End 50\n}\nPose 1\n{\n  Start 100\n  End 200\n}\n";
        let poses = parse_poses(pos);
        assert_eq!(poses.len(), 2);
    }
}
