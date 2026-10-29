use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct ActData {
    pub groups: HashMap<u16, u16>,

    pub group_members: HashMap<u16, Vec<u16>>,

    pub plain_followup: HashMap<u16, u16>,

    pub edges: HashMap<u16, Vec<u16>>,
}

fn parse_pose_id_list(raw: &str) -> Vec<u16> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut current = String::new();
    let mut ids = Vec::new();
    for ch in raw.chars() {
        if ch.is_ascii_digit() || ch == '-' {
            current.push(ch);
        } else if !current.is_empty() {
            ids.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        ids.push(current);
    }
    for part in ids {
        let (start, end) = match part.split_once('-') {
            Some((a, b)) => (a.parse::<u16>().unwrap_or(0), b.parse::<u16>().unwrap_or(0)),
            None => (part.parse::<u16>().unwrap_or(0), 0),
        };
        if end == 0 {
            if !seen.contains(&start) {
                seen.insert(start);
                out.push(start);
            }
        } else {
            let (lo, hi) = (start.min(end), start.max(end));
            for id in lo..=hi {
                if seen.insert(id) {
                    out.push(id);
                }
            }
        }
    }
    out
}

pub fn parse_character_act(text: &str) -> ActData {
    let mut edges: HashMap<u16, Vec<u16>> = HashMap::new();
    let mut groups: HashMap<u16, u16> = HashMap::new();
    let mut group_members: HashMap<u16, Vec<u16>> = HashMap::new();
    let mut group_id = 0u16;

    let source = text
        .lines()
        .map(|line| line.split('#').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    for declaration in source.split_inclusive(')') {
        let Some(body) = declaration.trim().strip_suffix(')') else {
            continue;
        };
        let Some((name, members)) = body.split_once('(') else {
            continue;
        };
        if members.contains('(') {
            continue;
        }
        let name = name.trim();
        let pose_ids = parse_pose_id_list(members);
        if name.eq_ignore_ascii_case("group") {
            for pose_id in &pose_ids {
                groups.insert(*pose_id, group_id);
            }
            group_members.insert(group_id, pose_ids);
            group_id = group_id.saturating_add(1);
            continue;
        }
        let Ok(from) = name.parse::<u16>() else {
            continue;
        };
        edges.insert(from, pose_ids);
    }

    let mut plain_followup = HashMap::new();
    for members in group_members.values() {
        for pair in members.windows(2) {
            plain_followup.insert(pair[0], pair[1]);
        }
    }

    ActData {
        groups,
        group_members,
        plain_followup,
        edges,
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# comment
group(262-264)
group(266)

262(263,266,267)
263(264)
"#;

    #[test]
    fn parses_groups_and_edges() {
        let act = parse_character_act(SAMPLE);

        assert_eq!(act.groups.get(&262), Some(&0));
        assert_eq!(act.groups.get(&264), Some(&0));
        assert_eq!(act.groups.get(&266), Some(&1));
        assert_eq!(act.group_members.get(&0), Some(&vec![262, 263, 264]));
        assert_eq!(act.plain_followup.get(&262), Some(&263));
        assert_eq!(act.plain_followup.get(&263), Some(&264));
        assert!(!act.plain_followup.contains_key(&264));

        assert_eq!(act.edges.get(&262).unwrap(), &vec![263, 266, 267]);
        assert_eq!(act.edges.get(&263).unwrap(), &vec![264]);
    }

    #[test]
    fn group_declaration_does_not_generate_edges() {
        let act = parse_character_act("group(262-264)");
        assert_eq!(act.groups.len(), 3);
        assert!(act.edges.is_empty());
    }

    #[test]
    fn range_in_edges_expands() {
        let act = parse_character_act("300(301-303,305)");
        assert_eq!(act.edges.get(&300).unwrap(), &vec![301, 302, 303, 305]);
    }

    #[test]
    fn empty_input() {
        let act = parse_character_act("");
        assert!(act.edges.is_empty());
        assert!(act.groups.is_empty());
    }

    #[test]
    fn declarations_follow_parentheses_across_lines_and_comments() {
        let act = parse_character_act(
            "# ignored(999)\r\nGROUP (\r\n262-264) 412(262, # ) 999\r\n\
             266-267,262\r\n268)413(279)\r\n414( # unfinished\r\n280",
        );
        assert_eq!(act.group_members.get(&0), Some(&vec![262, 263, 264]));
        assert_eq!(act.edges.get(&412), Some(&vec![262, 266, 267, 268]));
        assert_eq!(act.edges.get(&413), Some(&vec![279]));
        assert!(!act.edges.contains_key(&414));
    }

    #[test]
    fn origin_res_character_act() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../origin_res/Common/character.act");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("origin_res missing, skipping");
            return;
        };

        let text = String::from_utf8_lossy(&bytes);
        let act = parse_character_act(&text);

        assert_eq!(act.edges.len(), 191);
        assert_eq!(act.groups.len(), 227);
        assert_eq!(act.group_members.len(), 160);
        assert_eq!(act.plain_followup.len(), 67);

        assert_eq!(act.groups.get(&262), act.groups.get(&264));
        assert_eq!(act.edges.get(&262), Some(&vec![263, 266, 267]));
        for pose in [412, 413] {
            let next = &act.edges[&pose];
            assert_eq!(next.len(), 118);
            assert_eq!(&next[..8], &[203, 224, 230, 233, 237, 238, 239, 240]);
            assert_eq!(&next[110..], &[560, 561, 562, 572, 565, 566, 567, 568]);
            for target in [248, 262, 279, 421, 430] {
                assert!(next.contains(&target), "{pose} must retain {target}");
            }
        }
        assert!(act.edges[&412].contains(&417));
        assert!(!act.edges[&412].contains(&418));
        assert!(act.edges[&413].contains(&418));
        assert!(!act.edges[&413].contains(&417));
    }
}
