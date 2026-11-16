use super::{
    local_player_id, local_result_bits, result_heading, score_rows, OfflineScores, ScoreRow,
};
use crate::plugins::{launcher_ui::UiFont, network_plugin::NetState, ui_theme as theme};
use crate::{app_state::LaunchRequest, components::fighter::KillScore};
use bevy::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Presentation {
    pub bits: i32,
    pub heading: String,
    pub context: String,
    pub summary: String,
    pub team_wins: Option<[u32; 2]>,
    pub rows: Vec<ScoreRow>,
    pub local_id: String,
    pub elapsed_ms: Option<u64>,
    pub best_time: Option<Option<u64>>,
    pub new_best: bool,
    pub save_failed: bool,
}

impl Presentation {
    pub(super) fn network(net: &NetState, request: &LaunchRequest, score: &KillScore) -> Self {
        let result = net.battle_result.as_ref().expect("network settlement");
        let mode = if net.mode.is_empty() {
            &request.mode
        } else {
            &net.mode
        };
        let stage = if net.stage.is_empty() {
            &request.stage_id
        } else {
            &net.stage
        };
        let stage = meteor_scene::stage_registry::get_stage(stage, &[]);
        Self {
            bits: local_result_bits(net, request, result),
            heading: result_heading(net, result),
            context: format!(
                "{}  /  {}  /  第 {} 回合",
                stage.name,
                meteor_network::modes::mode_label(mode),
                result.round.max(1)
            ),
            summary: if meteor_network::modes::is_team_mode(mode) {
                format!(
                    "流星  {:02}    :    {:02}  蝴蝶",
                    net.stock_team_wins[0], net.stock_team_wins[1]
                )
            } else {
                "个人战绩 · 累计击杀排行".into()
            },
            team_wins: meteor_network::modes::is_team_mode(mode).then_some(net.stock_team_wins),
            rows: score_rows(net, request, score),
            local_id: local_player_id(net),
            elapsed_ms: None,
            best_time: None,
            new_best: false,
            save_failed: false,
        }
    }

    pub(super) fn offline(
        request: &LaunchRequest,
        score: &KillScore,
        scores: &OfflineScores,
        bits: i32,
    ) -> Self {
        Self {
            bits,
            heading: "本场战绩已记录".into(),
            context: format!(
                "{}  /  {}",
                request.stage_name,
                meteor_network::modes::mode_label(&request.mode)
            ),
            summary: "对局战绩".into(),
            team_wins: None,
            rows: scores.rows(request, score),
            local_id: "local".into(),
            elapsed_ms: None,
            best_time: None,
            new_best: false,
            save_failed: false,
        }
    }
}

pub(super) fn outcome(bits: i32) -> (&'static str, Color) {
    if bits & 4 != 0 {
        ("胜利", theme::TEXT)
    } else if bits & 2 != 0 {
        ("惜败", theme::ACCENT_TEXT)
    } else if bits & 1 != 0 {
        ("平局", theme::TEXT)
    } else {
        ("战局结束", theme::MUTED)
    }
}

pub(super) fn label(
    commands: &mut Commands,
    parent: Entity,
    font: &UiFont,
    value: impl Into<String>,
    size: f32,
    color: Color,
) -> Entity {
    let id = commands
        .spawn((
            Text::new(value),
            TextFont {
                font: font.0.clone().into(),
                font_size: size.into(),
                ..default()
            },
            TextColor(color),
        ))
        .id();
    commands.entity(parent).add_child(id);
    id
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettlementLayout {
    Desktop,
    Landscape,
    Portrait,
}

impl SettlementLayout {
    pub(crate) fn for_viewport(width: f32, height: f32) -> Self {
        if width < height {
            Self::Portrait
        } else if width < 1000.0 || height < 540.0 {
            Self::Landscape
        } else {
            Self::Desktop
        }
    }
}

fn node(commands: &mut Commands, parent: Entity, node: Node) -> Entity {
    let entity = commands.spawn(node).id();
    commands.entity(parent).add_child(entity);
    entity
}

pub(super) fn spawn_surface(
    commands: &mut Commands,
    root: Entity,
    font: &UiFont,
    data: &Presentation,
    layout: SettlementLayout,
) -> Entity {
    let compact = layout != SettlementLayout::Desktop;
    let portrait = layout == SettlementLayout::Portrait;
    let veil = node(
        commands,
        root,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
    );
    commands
        .entity(veil)
        .insert(BackgroundGradient(vec![LinearGradient::new(
            LinearGradient::TO_RIGHT,
            vec![
                ColorStop::auto(theme::BACKGROUND.with_alpha(0.66)),
                ColorStop::auto(theme::BACKGROUND.with_alpha(0.16)),
            ],
        )
        .into()]));
    let panel = node(
        commands,
        root,
        Node {
            width: Val::Percent(90.0),
            max_width: Val::Px(1180.0),
            height: Val::Percent(88.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(if compact { 12.0 } else { 24.0 }),
            min_height: Val::Px(0.0),
            ..default()
        },
    );
    let masthead = node(
        commands,
        panel,
        Node {
            width: Val::Percent(100.0),
            flex_shrink: 0.0,
            flex_wrap: FlexWrap::Wrap,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            column_gap: Val::Px(12.0),
            row_gap: Val::Px(4.0),
            ..default()
        },
    );
    let section = label(commands, masthead, font, "战 局 结 算", 12.0, theme::METAL);
    commands.entity(section).insert(Node {
        border: UiRect::left(Val::Px(2.0)),
        padding: UiRect::left(Val::Px(10.0)),
        ..default()
    });
    commands
        .entity(section)
        .insert(BorderColor::all(theme::ACCENT_TEXT));
    label(commands, masthead, font, &data.context, 11.0, theme::MUTED);
    let body = node(
        commands,
        panel,
        Node {
            width: Val::Percent(100.0),
            flex_grow: 1.0,
            flex_basis: Val::Px(0.0),
            min_height: Val::Px(0.0),
            flex_direction: if portrait {
                FlexDirection::Column
            } else {
                FlexDirection::Row
            },
            column_gap: Val::Px(if compact { 24.0 } else { 56.0 }),
            row_gap: Val::Px(20.0),
            ..default()
        },
    );
    let hero = node(
        commands,
        body,
        Node {
            width: if portrait {
                Val::Percent(100.0)
            } else {
                Val::Percent(35.0)
            },
            flex_shrink: 0.0,
            min_width: Val::Px(0.0),
            flex_direction: FlexDirection::Column,
            justify_content: if portrait {
                JustifyContent::Start
            } else {
                JustifyContent::Center
            },
            row_gap: Val::Px(if compact { 12.0 } else { 28.0 }),
            ..default()
        },
    );
    spawn_hero(commands, hero, font, data, layout);
    let ledger = node(
        commands,
        body,
        Node {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_basis: Val::Px(0.0),
            min_width: Val::Px(0.0),
            min_height: Val::Px(0.0),
            padding: UiRect::all(Val::Px(if compact { 12.0 } else { 20.0 })),
            border: UiRect::top(Val::Px(1.0)),
            ..default()
        },
    );
    commands.entity(ledger).insert((
        BackgroundColor(theme::BACKGROUND.with_alpha(0.86)),
        BorderColor::all(theme::METAL.with_alpha(0.45)),
    ));
    let header = node(
        commands,
        ledger,
        Node {
            width: Val::Percent(100.0),
            flex_shrink: 0.0,
            justify_content: JustifyContent::SpaceBetween,
            padding: UiRect::bottom(Val::Px(12.0)),
            ..default()
        },
    );
    label(commands, header, font, "本场战绩", 16.0, theme::TEXT);
    label(
        commands,
        header,
        font,
        format!("{:02} 人参战", data.rows.len()),
        11.0,
        theme::MUTED,
    );
    table_row(
        commands,
        ledger,
        font,
        ["#", "参战者", "阵营", "击杀", "阵亡"],
        theme::MUTED,
        None,
    );
    let roster = commands
        .spawn((
            super::super::launcher_ui::scrollable_y(Node {
                width: Val::Percent(100.0),
                flex_grow: 1.0,
                flex_basis: Val::Px(0.0),
                min_height: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                ..default()
            }),
            Name::new("Settlement roster"),
        ))
        .id();
    commands.entity(ledger).add_child(roster);
    for (rank, row) in data.rows.iter().enumerate() {
        let local = row.id == data.local_id;
        let name = if local {
            format!("{} · 你", row.name)
        } else {
            row.name.clone()
        };
        let camp = match row.team {
            1 => "流星",
            2 => "蝴蝶",
            _ => "个人",
        };
        let line = table_row(
            commands,
            roster,
            font,
            [
                &format!("{:02}", rank + 1),
                &name,
                camp,
                &row.kills.to_string(),
                &row.deaths.to_string(),
            ],
            theme::TEXT,
            None,
        );
        if local {
            commands.entity(line).insert((
                theme::hero_gradient(true, false),
                BorderColor {
                    left: theme::ACCENT_TEXT,
                    bottom: theme::LINE.with_alpha(0.12),
                    ..default()
                },
            ));
        }
    }
    let note = label(
        commands,
        ledger,
        font,
        "按累计击杀排序  ·  滚动查看全部",
        10.0,
        theme::MUTED,
    );
    commands.entity(note).insert(Node {
        margin: UiRect::top(Val::Px(10.0)),
        flex_shrink: 0.0,
        ..default()
    });
    panel
}

fn spawn_hero(
    commands: &mut Commands,
    hero: Entity,
    font: &UiFont,
    data: &Presentation,
    layout: SettlementLayout,
) {
    let compact = layout != SettlementLayout::Desktop;
    let portrait = layout == SettlementLayout::Portrait;
    let (title, color) = outcome(data.bits);
    let outcome_group = node(
        commands,
        hero,
        Node {
            flex_direction: if portrait {
                FlexDirection::Row
            } else {
                FlexDirection::Column
            },
            align_items: if portrait {
                AlignItems::Center
            } else {
                AlignItems::Start
            },
            column_gap: Val::Px(18.0),
            row_gap: Val::Px(8.0),
            ..default()
        },
    );
    let title = label(
        commands,
        outcome_group,
        font,
        title,
        if portrait {
            48.0
        } else if compact {
            60.0
        } else {
            92.0
        },
        color,
    );
    commands.entity(title).insert((
        TextLayout::linebreak(LineBreak::NoWrap),
        Node {
            flex_shrink: 0.0,
            ..default()
        },
    ));
    let winner = label(
        commands,
        outcome_group,
        font,
        &data.heading,
        if compact { 15.0 } else { 22.0 },
        theme::TEXT,
    );
    commands.entity(winner).insert(Node {
        width: if portrait {
            Val::Px(0.0)
        } else {
            Val::Percent(100.0)
        },
        flex_grow: if portrait { 1.0 } else { 0.0 },
        min_width: Val::Px(0.0),
        ..default()
    });
    if let Some(wins) = data.team_wins {
        let scores = node(
            commands,
            hero,
            Node {
                column_gap: Val::Px(if compact { 18.0 } else { 24.0 }),
                align_items: AlignItems::Center,
                ..default()
            },
        );
        for (i, camp) in ["流星", "蝴蝶"].into_iter().enumerate() {
            if i == 1 {
                label(commands, scores, font, "—", 18.0, theme::MUTED);
            }
            let team = node(
                commands,
                scores,
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(2.0),
                    ..default()
                },
            );
            label(
                commands,
                team,
                font,
                format!("{:02}", wins[i]),
                if compact { 26.0 } else { 44.0 },
                theme::TEXT,
            );
            label(commands, team, font, camp, 11.0, theme::MUTED);
        }
    } else {
        label(commands, hero, font, &data.summary, 12.0, theme::METAL);
    }
    let times = node(
        commands,
        hero,
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(5.0),
            flex_shrink: 0.0,
            ..default()
        },
    );
    label(
        commands,
        times,
        font,
        format!("本局用时  {}", super::timing::format_time(data.elapsed_ms)),
        if compact { 15.0 } else { 20.0 },
        theme::TEXT,
    );
    if let Some(best) = data.best_time {
        label(
            commands,
            times,
            font,
            format!(
                "历史最佳  {}",
                best.map_or_else(
                    || "暂无通关纪录".into(),
                    |ms| super::timing::format_time(Some(ms))
                )
            ),
            12.0,
            theme::METAL,
        );
        if data.new_best {
            label(commands, times, font, "新纪录", 12.0, theme::ACCENT_TEXT);
        }
        if data.save_failed {
            label(commands, times, font, "纪录暂未保存", 11.0, theme::MUTED);
        }
    }
    let personal = node(
        commands,
        hero,
        Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            padding: UiRect::top(Val::Px(if compact { 10.0 } else { 20.0 })),
            border: UiRect::top(Val::Px(1.0)),
            row_gap: Val::Px(if compact { 8.0 } else { 14.0 }),
            ..default()
        },
    );
    commands
        .entity(personal)
        .insert(BorderColor::all(theme::METAL.with_alpha(0.25)));
    label(commands, personal, font, "你的战绩", 11.0, theme::METAL);
    let metrics = node(
        commands,
        personal,
        Node {
            width: Val::Percent(100.0),
            ..default()
        },
    );
    let me = data
        .rows
        .iter()
        .enumerate()
        .find(|(_, r)| r.id == data.local_id);
    for (heading, value) in [
        (
            "排名",
            me.map_or_else(|| "—".into(), |(rank, _)| format!("#{:02}", rank + 1)),
        ),
        (
            "击杀",
            me.map_or_else(|| "—".into(), |(_, row)| format!("{:02}", row.kills)),
        ),
        (
            "阵亡",
            me.map_or_else(|| "—".into(), |(_, row)| format!("{:02}", row.deaths)),
        ),
    ] {
        let metric = node(
            commands,
            metrics,
            Node {
                width: Val::Percent(33.33),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                ..default()
            },
        );
        label(
            commands,
            metric,
            font,
            value,
            if compact { 24.0 } else { 34.0 },
            theme::TEXT,
        );
        label(commands, metric, font, heading, 10.0, theme::MUTED);
    }
}

fn table_row(
    commands: &mut Commands,
    parent: Entity,
    font: &UiFont,
    cells: [&str; 5],
    color: Color,
    background: Option<Color>,
) -> Entity {
    let row = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(36.0),
                flex_shrink: 0.0,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(8.0)),
                border: UiRect {
                    left: Val::Px(2.0),
                    bottom: Val::Px(1.0),
                    ..default()
                },
                ..default()
            },
            BackgroundColor(background.unwrap_or(Color::NONE)),
            BorderColor {
                bottom: theme::LINE.with_alpha(0.12),
                ..BorderColor::all(Color::NONE)
            },
        ))
        .id();
    commands.entity(parent).add_child(row);
    for (i, value) in cells.into_iter().enumerate() {
        let cell = commands
            .spawn(Node {
                width: Val::Px(match i {
                    0 => 28.0,
                    2 => 42.0,
                    3 | 4 => 38.0,
                    _ => 0.0,
                }),
                min_width: Val::Px(0.0),
                flex_grow: if i == 1 { 1.0 } else { 0.0 },
                flex_shrink: 0.0,
                overflow: Overflow::clip(),
                justify_content: if i >= 2 {
                    JustifyContent::End
                } else {
                    JustifyContent::Start
                },
                padding: UiRect::right(Val::Px(if i == 1 { 8.0 } else { 0.0 })),
                ..default()
            })
            .id();
        commands.entity(row).add_child(cell);
        let text = label(commands, cell, font, value, 12.0, color);
        commands
            .entity(text)
            .insert(TextLayout::linebreak(LineBreak::NoWrap));
    }
    row
}

pub(super) fn overlay_node() -> Node {
    Node {
        position_type: PositionType::Absolute,
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        align_items: AlignItems::Center,
        justify_content: JustifyContent::Center,
        ..default()
    }
}
