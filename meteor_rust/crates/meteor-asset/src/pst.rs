#[derive(Debug, Clone, PartialEq)]
pub struct SceneItemSet {
    pub item: String,

    pub key: String,

    pub value: String,
}

#[derive(Debug, Clone, Default)]
pub struct SceneScript {
    pub sets: Vec<SceneItemSet>,

    pub includes: Vec<String>,
}

fn literal(raw: &str) -> String {
    let trimmed = raw.trim();
    let trimmed = trimmed.trim_end_matches(';').trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn parse_scene_script(text: &str) -> SceneScript {
    let mut script = SceneScript::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#include") {
            let include = literal(rest);
            if !include.is_empty() {
                script.includes.push(include);
            }
            continue;
        }

        let Some(body) = trimmed
            .strip_prefix("SetSceneItem")
            .and_then(|r| r.trim().strip_prefix('('))
            .map(|r| r.trim().trim_end_matches(';').trim())
            .and_then(|r| r.strip_suffix(')'))
        else {
            continue;
        };

        let args: Vec<&str> = split_top_level_commas(body);
        if args.len() < 2 {
            continue;
        }
        script.sets.push(SceneItemSet {
            item: literal(args[0]),
            key: literal(args[1]),

            value: if args.len() > 3 {
                literal(args[3])
            } else if args.len() > 2 {
                literal(args[2])
            } else {
                String::new()
            },
        });
    }
    script
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_quote = false;
    let mut quote_char = '\0';
    for (i, c) in s.char_indices() {
        if in_quote {
            if c == quote_char {
                in_quote = false;
            }
        } else if c == '"' || c == '\'' {
            in_quote = true;
            quote_char = c;
        } else if c == ',' {
            parts.push(&s[start..i]);
            start = i + 1;
        }
    }
    parts.push(&s[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_set_scene_item_calls() {
        let text = r#"
#include "..\PScriptLib\box.pst"
Scene_OnLoad()
{
    SetSceneItem(name, "name", "machine");
    SetSceneItem(name, "attribute", "damagevalue", 50);
    SetSceneItem("D_Box1", "visible", 0);
}
"#;
        let script = parse_scene_script(text);
        assert_eq!(script.includes.len(), 1);
        assert_eq!(script.sets.len(), 3);
        assert_eq!(script.sets[0].item, "name");
        assert_eq!(script.sets[0].key, "name");
        assert_eq!(script.sets[0].value, "machine");
        assert_eq!(script.sets[1].key, "attribute");
        assert_eq!(script.sets[1].value, "50");
        assert_eq!(script.sets[2].item, "D_Box1");
        assert_eq!(script.sets[2].key, "visible");
        assert_eq!(script.sets[2].value, "0");
    }

    #[test]
    fn comma_in_quoted_value_stays_intact() {
        let text = r#"SetSceneItem("D_X", "name", "damage, heavy");"#;
        let script = parse_scene_script(text);
        assert_eq!(script.sets.len(), 1);
        assert_eq!(script.sets[0].value, "damage, heavy");
    }

    #[test]
    fn expression_values_kept_raw() {
        let text = r#"SetSceneItem(name, "hp", g_iStoneMaxHP);"#;
        let script = parse_scene_script(text);
        assert_eq!(script.sets[0].item, "name");
        assert_eq!(script.sets[0].value, "g_iStoneMaxHP");
    }
}
