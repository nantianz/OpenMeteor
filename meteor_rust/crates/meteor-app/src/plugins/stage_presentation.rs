use super::ui_theme as theme;
use crate::app_state::LaunchRequest;
use bevy::{
    prelude::*,
    text::{FontStyle, FontWeight, TextSpan},
};
use pulldown_cmark::{Event, Parser, Tag};

fn read_text(stage: &str, file: &str) -> Option<String> {
    if !is_stage_id(stage) {
        return None;
    }
    let text = crate::resource_io::read_to_string(format!("{stage}/{file}")).ok()?;
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

pub(super) fn is_stage_id(stage: &str) -> bool {
    meteor_scene::stage_registry::normalize_stage_id(stage).eq_ignore_ascii_case(stage)
}

pub(super) fn display_name(stage: &str, fallback: &str) -> String {
    read_text(stage, "name.txt")
        .and_then(|text| text.lines().next().map(|line| line.trim_end().to_owned()))
        .unwrap_or_else(|| fallback.to_owned())
}

#[derive(Component, Default)]
pub(super) struct StageDescription(String);

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Style {
    heading: bool,
    strong: bool,
    emphasis: bool,
    code: bool,
}

fn markdown_runs(source: &str) -> Vec<(String, Style)> {
    let mut runs: Vec<(String, Style)> = Vec::new();
    let mut style = Style::default();
    let (mut strong, mut emphasis) = (0usize, 0usize);
    let mut lists = Vec::<Option<u64>>::new();
    let mut push = |text: &str, style| {
        if let Some((previous, _)) = runs.last_mut().filter(|(_, last)| *last == style) {
            previous.push_str(text);
        } else {
            runs.push((text.to_owned(), style));
        }
    };
    for event in Parser::new(source) {
        match event {
            Event::Start(Tag::Heading(..)) => style.heading = true,
            Event::End(Tag::Heading(..)) => {
                style.heading = false;
                push("\n", style);
            }
            Event::Start(Tag::Strong) => {
                strong += 1;
                style.strong = true;
            }
            Event::End(Tag::Strong) => {
                strong -= 1;
                style.strong = strong > 0;
            }
            Event::Start(Tag::Emphasis) => {
                emphasis += 1;
                style.emphasis = true;
            }
            Event::End(Tag::Emphasis) => {
                emphasis -= 1;
                style.emphasis = emphasis > 0;
            }
            Event::Start(Tag::List(first)) => lists.push(first),
            Event::End(Tag::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                if let Some(Some(number)) = lists.last_mut() {
                    push(&format!("{number}. "), style);
                    *number += 1;
                } else {
                    push("· ", style);
                }
            }
            Event::Start(Tag::CodeBlock(_)) => style.code = true,
            Event::End(Tag::CodeBlock(_)) => style.code = false,
            Event::End(Tag::Item | Tag::Paragraph) | Event::SoftBreak | Event::HardBreak => {
                push("\n", style)
            }
            Event::Text(text) | Event::Html(text) => push(&text, style),
            Event::Code(text) => push(
                &text,
                Style {
                    code: true,
                    ..style
                },
            ),
            _ => {}
        }
    }
    while let Some((text, _)) = runs.last_mut() {
        text.truncate(text.trim_end().len());
        if !text.is_empty() {
            break;
        }
        runs.pop();
    }
    runs
}

pub(super) fn update_descriptions(
    mut commands: Commands,
    request: Res<LaunchRequest>,
    mut descriptions: Query<(Entity, &mut StageDescription, &mut Text, &TextFont)>,
) {
    let stage = &request.stage_id;
    if descriptions
        .iter()
        .all(|(_, rendered, ..)| rendered.0 == *stage)
    {
        return;
    }
    let rich = read_text(stage, "description.md")
        .map(|source| markdown_runs(&source))
        .filter(|runs| !runs.is_empty());
    let plain = rich.is_none().then(|| {
        read_text(stage, "description.txt").unwrap_or_else(|| {
            format!("在 {stage}/ 下添加 description.txt 或 description.md，可填写关卡说明。")
        })
    });
    for (entity, mut rendered, mut text, font) in &mut descriptions {
        if rendered.0 == *stage {
            continue;
        }
        rendered.0.clone_from(stage);
        let mut node = commands.entity(entity);
        node.despawn_children();
        if let Some(plain) = &plain {
            text.0.clone_from(plain);
            continue;
        }
        text.0.clear();
        if let Some(rich) = &rich {
            node.with_children(|spans| {
                for (text, style) in rich {
                    let mut font = font.clone();
                    let color = if style.heading || style.strong {
                        font.weight = FontWeight::BOLD;
                        theme::ACCENT_TEXT
                    } else if style.code || style.emphasis {
                        theme::METAL
                    } else {
                        theme::TEXT
                    };
                    if style.heading {
                        font.font_size = 17.0.into();
                    }
                    if style.emphasis {
                        font.style = FontStyle::Italic;
                    }
                    spans.spawn((TextSpan(text.clone()), font, TextColor(color)));
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_stage_text_supports_markdown_plain_text_and_missing_files() {
        let mut data = Vec::new();
        let entries: Vec<_> = [
            ("stage/sn97.name.txt", "\u{feff} 峡谷\r\n额外一行"),
            (
                "stage/sn97.description.md",
                "# 峡谷\n\n第一行 **加粗** 和 *斜体*\n\n- 路线",
            ),
            ("stage/sn97.description.txt", "有 Markdown 时优先显示富文本"),
            ("stage/sn98.description.txt", "第一行\r\n第二行\r\n"),
        ]
        .into_iter()
        .map(|(name, source)| {
            let entry = serde_json::json!({"name": name, "off": data.len(), "len": source.len()});
            data.extend_from_slice(source.as_bytes());
            entry
        })
        .collect();
        let manifest = serde_json::to_vec(&serde_json::json!({
            "formatVersion": 1, "kind": "test", "packId": "test/stage-presentation", "entries": entries,
        })).unwrap();
        let mut pack = b"MPK1".to_vec();
        pack.extend((manifest.len() as u32).to_le_bytes());
        pack.extend(manifest);
        pack.extend(data);
        meteor_asset::runtime_assets::register_mpk1(&pack).unwrap();

        assert_eq!(display_name("sn97", "SN97"), "峡谷");
        assert_eq!(display_name("sn98", "SN98"), "SN98");
        let mut app = App::new();
        app.insert_resource(LaunchRequest {
            stage_id: "sn97".into(),
            ..default()
        })
        .add_systems(Update, update_descriptions);
        let entity = app
            .world_mut()
            .spawn((Text::default(), StageDescription::default()))
            .id();
        app.update();
        let world = app.world();
        let spans = world.get::<Children>(entity).unwrap();
        let runs: Vec<_> = spans
            .iter()
            .map(|span| {
                (
                    world.get::<TextSpan>(span).unwrap().0.as_str(),
                    world.get::<TextFont>(span).unwrap(),
                )
            })
            .collect();
        assert!(runs
            .iter()
            .any(|(text, font)| *text == "峡谷" && font.weight == FontWeight::BOLD));
        assert!(runs
            .iter()
            .any(|(text, font)| *text == "加粗" && font.weight == FontWeight::BOLD));
        assert!(runs
            .iter()
            .any(|(text, font)| *text == "斜体" && font.style == FontStyle::Italic));
        assert!(runs.iter().any(|(text, _)| text.contains("· 路线")));

        app.world_mut().resource_mut::<LaunchRequest>().stage_id = "sn98".into();
        app.update();
        assert_eq!(
            app.world().get::<Text>(entity).unwrap().0,
            "第一行\r\n第二行"
        );
        assert!(app.world().get::<Children>(entity).is_none());
        app.world_mut().resource_mut::<LaunchRequest>().stage_id = "sn99".into();
        app.update();
        let hint = &app.world().get::<Text>(entity).unwrap().0;
        assert!(
            hint.contains("sn99/")
                && hint.contains("description.txt")
                && hint.contains("description.md")
        );
    }
}
