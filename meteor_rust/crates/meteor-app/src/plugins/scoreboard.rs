use super::{
    battle_result::{score_rows, ScoreRow},
    launcher_ui::UiFont,
    network_plugin::{NetState, NetStatus},
};
use crate::app_state::{AppState, LaunchRequest};
use crate::components::fighter::KillScore;
use bevy::prelude::*;

#[derive(Default)]
pub(crate) struct Latency {
    pub clock: f64,
    pub pending: Option<f64>,
    pub sample: Option<(f64, f64)>,
    pub last_sent: f64,
    pub received: std::collections::HashMap<String, f64>,
}

impl Latency {
    pub fn receive(&mut self, token: f64) -> bool {
        if self.pending != Some(token) {
            return false;
        }
        self.pending = None;
        self.sample = Some(((self.clock - token).max(0.0) * 1000.0, self.clock));
        true
    }
    pub fn value(&self) -> Option<f64> {
        self.sample
            .filter(|(_, at)| self.clock - at < 10.0)
            .map(|(ms, _)| ms)
    }
}

fn ping(net: &NetState, id: &str) -> String {
    let value = if id == net.player_id {
        net.latency.value()
    } else {
        net.latency
            .received
            .get(id)
            .filter(|at| net.latency.clock - **at < 10.0)
            .and_then(|_| net.room_latency.get(id))
            .and_then(|v| {
                if net.is_stock_udp() {
                    v.host_rtt
                } else {
                    v.server_rtt
                }
            })
    };
    value
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map_or_else(|| "—".into(), |v| format!("{v:.0} ms"))
}

fn groups(mode: &str, rows: Vec<ScoreRow>) -> Vec<(&'static str, Vec<ScoreRow>)> {
    if mode == "leader" {
        return vec![("盟主 · 个人战绩", rows)];
    }
    let mut groups = vec![
        ("流星", Vec::new()),
        ("蝴蝶", Vec::new()),
        ("无阵营", Vec::new()),
    ];
    for row in rows {
        let index = match row.team {
            1 => 0,
            2 => 1,
            _ => 2,
        };
        groups[index].1.push(row);
    }
    if groups[2].1.is_empty() {
        groups.pop();
    }
    groups
}

#[derive(Component)]
struct Board;
#[derive(Component)]
struct PingHud;

pub struct ScoreboardPlugin;
impl Plugin for ScoreboardPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, update.after(super::fighter_frame::SerialFighterSet));
    }
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    state: Res<State<AppState>>,
    keys: Res<ButtonInput<KeyCode>>,
    net: Res<NetState>,
    request: Res<LaunchRequest>,
    score: Res<KillScore>,
    font: Res<UiFont>,
    boards: Query<Entity, With<Board>>,
    mut hud: Query<(Entity, &mut Text), With<PingHud>>,
    mut previous: Local<String>,
) {
    let playing = *state.get() == AppState::Playing && net.status == NetStatus::InRoom;
    let label = format!(
        "{} Ping {}",
        if net.is_stock_udp() {
            "房主"
        } else {
            "服务器"
        },
        ping(&net, &net.player_id)
    );
    if !playing {
        for (entity, _) in &hud {
            commands.entity(entity).despawn();
        }
    } else if let Ok((_, mut text)) = hud.single_mut() {
        if text.0 != label {
            text.0.clone_from(&label);
        }
    } else {
        commands.spawn((
            PingHud,
            Text::new(&label),
            TextFont {
                font: font.0.clone().into(),
                font_size: 11.0.into(),
                ..default()
            },
            TextColor(super::ui_theme::MUTED),
            GlobalZIndex(85),
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(18.0),
                bottom: Val::Px(12.0),
                ..default()
            },
        ));
    }
    if !playing || !keys.pressed(KeyCode::Tab) {
        for entity in &boards {
            commands.entity(entity).despawn();
        }
        previous.clear();
        return;
    }
    let groups = groups(&net.mode, score_rows(&net, &request, &score));
    let pings: Vec<_> = groups
        .iter()
        .flat_map(|(_, rows)| rows.iter().map(|row| ping(&net, &row.id)))
        .collect();
    let signature = format!("{label}{groups:?}{pings:?}");
    if *previous == signature && !boards.is_empty() {
        return;
    }
    *previous = signature;
    for entity in &boards {
        commands.entity(entity).despawn();
    }
    commands
        .spawn((
            Board,
            GlobalZIndex(295),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Percent(14.0),
                width: Val::Percent(100.0),
                max_height: Val::Percent(76.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|root| {
            spawn_panel(root, &font, &net, &label, groups);
        });
}

fn label_font(font: &UiFont, size: f32) -> TextFont {
    TextFont {
        font: font.0.clone().into(),
        font_size: size.into(),
        ..default()
    }
}

fn spawn_panel(
    root: &mut ChildSpawnerCommands,
    font: &UiFont,
    net: &NetState,
    label: &str,
    groups: Vec<(&str, Vec<ScoreRow>)>,
) {
    use super::ui_theme as theme;
    let solo = groups.len() == 1;
    root.spawn((
        super::launcher_ui::scrollable_y(Node {
            width: Val::Percent(90.0),
            max_width: Val::Px(if solo { 480.0 } else { 900.0 }),
            max_height: Val::Vh(76.0),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(14.0)),
            border: UiRect::all(Val::Px(1.0)),
            row_gap: Val::Px(12.0),
            border_radius: BorderRadius::all(Val::Px(6.0)),
            ..default()
        }),
        BackgroundColor(Color::srgba(0.035, 0.041, 0.052, 0.96)),
        BorderColor::all(theme::LINE),
    ))
    .with_children(|panel| {
        panel
            .spawn(Node {
                width: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                column_gap: Val::Px(12.0),
                ..default()
            })
            .with_children(|header| {
                header.spawn((
                    Text::new("对局战绩"),
                    label_font(font, 14.0),
                    TextColor(theme::TEXT),
                ));
                header.spawn((
                    Text::new(label),
                    label_font(font, 11.0),
                    TextColor(theme::MUTED),
                ));
            });
        panel
            .spawn(Node {
                width: Val::Percent(100.0),
                flex_wrap: FlexWrap::Wrap,
                column_gap: Val::Px(12.0),
                row_gap: Val::Px(12.0),
                ..default()
            })
            .with_children(|tables| {
                for (title, rows) in groups {
                    let accent = match title {
                        "流星" => theme::ACCENT_TEXT,
                        "蝴蝶" => Color::srgb(0.48, 0.69, 0.79),
                        _ => theme::METAL,
                    };
                    tables
                        .spawn((
                            Node {
                                flex_direction: FlexDirection::Column,
                                flex_grow: 1.0,
                                flex_basis: Val::Px(280.0),
                                min_width: Val::Px(0.0),
                                border: UiRect::all(Val::Px(1.0)),
                                ..default()
                            },
                            BorderColor::all(theme::LINE),
                        ))
                        .with_children(|table| {
                            table
                                .spawn((
                                    Node {
                                        width: Val::Percent(100.0),
                                        height: Val::Px(30.0),
                                        padding: UiRect::horizontal(Val::Px(10.0)),
                                        align_items: AlignItems::Center,
                                        justify_content: JustifyContent::SpaceBetween,
                                        border: UiRect {
                                            top: Val::Px(2.0),
                                            ..default()
                                        },
                                        ..default()
                                    },
                                    BorderColor::all(accent),
                                    BackgroundColor(theme::HUD_SURFACE),
                                ))
                                .with_children(|heading| {
                                    heading.spawn((
                                        Text::new(title),
                                        label_font(font, 12.0),
                                        TextColor(accent),
                                    ));
                                    heading.spawn((
                                        Text::new(format!("{} 人", rows.len())),
                                        label_font(font, 10.0),
                                        TextColor(theme::MUTED),
                                    ));
                                });
                            spawn_table_row(
                                table,
                                font,
                                [
                                    "#".into(),
                                    "玩家".into(),
                                    "击杀".into(),
                                    "死亡".into(),
                                    "Ping".into(),
                                ],
                                true,
                                false,
                                false,
                                accent,
                            );
                            if rows.is_empty() {
                                table.spawn((
                                    Text::new("暂无玩家"),
                                    label_font(font, 11.0),
                                    TextColor(theme::MUTED),
                                    Node {
                                        padding: UiRect::all(Val::Px(12.0)),
                                        ..default()
                                    },
                                ));
                            }
                            for (rank, row) in rows.into_iter().enumerate() {
                                let local = row.id == net.player_id;
                                let name = if local {
                                    format!("{} · 你", row.name)
                                } else {
                                    row.name.clone()
                                };
                                spawn_table_row(
                                    table,
                                    font,
                                    [
                                        (rank + 1).to_string(),
                                        name,
                                        row.kills.to_string(),
                                        row.deaths.to_string(),
                                        ping(net, &row.id),
                                    ],
                                    false,
                                    local,
                                    rank % 2 == 1,
                                    accent,
                                );
                            }
                        });
                }
            });
        panel.spawn((
            Text::new("TAB  松开收起"),
            label_font(font, 10.0),
            TextColor(theme::MUTED),
            Node {
                align_self: AlignSelf::End,
                ..default()
            },
        ));
    });
}

#[allow(clippy::too_many_arguments)]
fn spawn_table_row(
    table: &mut ChildSpawnerCommands,
    font: &UiFont,
    cells: [String; 5],
    header: bool,
    local: bool,
    stripe: bool,
    accent: Color,
) {
    use super::ui_theme as theme;
    let background = if local {
        Color::srgba(0.80, 0.74, 0.60, 0.10)
    } else if header {
        Color::srgba(1.0, 1.0, 1.0, 0.045)
    } else if stripe {
        Color::srgba(1.0, 1.0, 1.0, 0.018)
    } else {
        Color::NONE
    };
    table
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(if header { 24.0 } else { 28.0 }),
                flex_shrink: 0.0,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(8.0)),
                border: UiRect {
                    bottom: Val::Px(1.0),
                    ..default()
                },
                ..default()
            },
            BackgroundColor(background),
            BorderColor::all(Color::srgba(0.62, 0.65, 0.68, 0.10)),
        ))
        .with_children(|line| {
            for (index, text) in cells.into_iter().enumerate() {
                let name = index == 1;
                let width = match index {
                    0 => 24.0,
                    2 | 3 => 38.0,
                    4 => 66.0,
                    _ => 0.0,
                };
                line.spawn((Node {
                    width: Val::Px(width),
                    min_width: Val::Px(0.0),
                    flex_grow: if name { 1.0 } else { 0.0 },
                    flex_shrink: 0.0,
                    overflow: Overflow::clip(),
                    justify_content: if index >= 2 {
                        JustifyContent::End
                    } else {
                        JustifyContent::Start
                    },
                    padding: UiRect::right(Val::Px(if name { 8.0 } else { 0.0 })),
                    ..default()
                },))
                    .with_children(|cell| {
                        cell.spawn((
                            Text::new(text),
                            label_font(font, if header { 10.0 } else { 12.0 }),
                            TextLayout::linebreak(LineBreak::NoWrap),
                            TextColor(if header || index == 0 {
                                theme::MUTED
                            } else if local && name {
                                accent
                            } else {
                                theme::TEXT
                            }),
                        ));
                    });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tab_overlay_releases_and_leaving_cleans_hud() {
        let mut app = App::new();
        app.insert_resource(State::new(AppState::Playing))
            .insert_resource(NetState {
                status: NetStatus::InRoom,
                mode: "leader".into(),
                player_id: "me".into(),
                ..default()
            })
            .init_resource::<LaunchRequest>()
            .init_resource::<KillScore>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(UiFont(Handle::default()))
            .add_systems(Update, update);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Tab);
        app.update();
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<Board>>()
                .iter(app.world())
                .count(),
            1
        );
        app.update();
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<Board>>()
                .iter(app.world())
                .count(),
            1
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::Tab);
        app.update();
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<Board>>()
                .iter(app.world())
                .count(),
            0
        );
        app.insert_resource(State::new(AppState::Menu));
        app.update();
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<PingHud>>()
                .iter(app.world())
                .count(),
            0
        );
    }

    #[test]
    fn ping_uses_server_for_ws_and_host_for_udp_and_expires() {
        let mut net = NetState::default();
        net.latency.received.insert("guest".into(), 0.0);
        net.room_latency.insert(
            "guest".into(),
            meteor_network::protocol::RoomLatencyWire {
                server_rtt: Some(50.0),
                host_rtt: Some(12.0),
                p2p_state: "direct".into(),
            },
        );
        assert_eq!(ping(&net, "guest"), "50 ms");
        #[cfg(not(target_arch = "wasm32"))]
        {
            net.address = "udp://127.0.0.1:7000".into();
            assert_eq!(ping(&net, "guest"), "12 ms");
        }
        net.latency.clock = 10.0;
        assert_eq!(ping(&net, "guest"), "—");
        assert_eq!(ping(&net, "unknown"), "—");
    }

    #[test]
    fn ping_correlates_reply_and_expires() {
        let mut latency = Latency {
            clock: 2.05,
            pending: Some(2.0),
            ..default()
        };
        assert!(!latency.receive(1.0));
        assert!(latency.receive(2.0));
        assert!((latency.value().unwrap() - 50.0).abs() < 0.001);
        assert!(!latency.receive(2.0));
        latency.clock = 12.1;
        assert_eq!(latency.value(), None);
    }
    #[test]
    fn leader_has_one_list_and_teams_keep_score_order() {
        let row = |team| ScoreRow {
            id: format!("{team}"),
            name: "玩家".into(),
            team,
            kills: 2,
            deaths: 1,
        };
        assert_eq!(groups("leader", vec![row(1), row(2)]).len(), 1);
        let teams = groups("deathmatch", vec![row(2), row(1)]);
        assert_eq!(teams.len(), 2);
        assert_eq!(teams[0].1[0].team, 1);
        assert_eq!(teams[1].1[0].team, 2);
    }
}
