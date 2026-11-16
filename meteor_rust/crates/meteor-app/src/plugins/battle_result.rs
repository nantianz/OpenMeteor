use bevy::prelude::*;

use super::network_plugin::NetState;
use crate::app_state::{AppState, LaunchRequest};
use crate::components::fighter::KillScore;

#[derive(Debug, Clone, Default)]
pub struct NetworkBattleResult {
    pub round: u32,
    pub terminal: bool,
    pub winner_teams: Vec<i32>,
    pub winner_player_id: Option<String>,
    pub reason: String,
    pub elapsed: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreRow {
    pub id: String,
    pub name: String,
    pub team: i32,
    pub kills: u32,
    pub deaths: u32,
}

#[derive(Resource, Default)]
pub struct OfflineScores {
    pub roster: std::collections::BTreeMap<String, ScoreRow>,
    totals: std::collections::BTreeMap<String, (u32, u32)>,
}

pub fn offline_actor_id(local: bool, mission_id: Option<i64>, slot: u8) -> String {
    if local {
        "local".into()
    } else if let Some(id) = mission_id {
        format!("npc:{id}")
    } else {
        format!("slot:{slot}")
    }
}

pub(crate) fn offline_player_team(request: &LaunchRequest) -> i32 {
    if request.mode == "single" && !request.spawn_room_npcs {
        1
    } else {
        request.stock_camp as i32
    }
}

impl OfflineScores {
    pub fn record_death(&mut self, target: String, attacker: Option<String>) {
        let total = self.totals.entry(target.clone()).or_default();
        total.1 = total.1.saturating_add(1);
        if let Some(attacker) = attacker.filter(|id| id != &target) {
            let total = self.totals.entry(attacker).or_default();
            total.0 = total.0.saturating_add(1);
        }
    }

    pub fn rows(&self, request: &LaunchRequest, score: &KillScore) -> Vec<ScoreRow> {
        let mut roster = self.roster.clone();
        roster.entry("local".into()).or_insert_with(|| ScoreRow {
            id: "local".into(),
            name: if request.player_name.is_empty() {
                "玩家".into()
            } else {
                request.player_name.clone()
            },
            team: offline_player_team(request),
            kills: 0,
            deaths: 0,
        });
        let mut rows: Vec<_> = roster
            .into_values()
            .map(|mut row| {
                let (kills, deaths) = self.totals.get(&row.id).copied().unwrap_or((
                    if row.id == "local" {
                        score.player_kills
                    } else {
                        0
                    },
                    0,
                ));
                row.kills = kills;
                row.deaths = deaths;
                row
            })
            .collect();
        rows.sort_by(|a, b| {
            b.kills
                .cmp(&a.kills)
                .then(a.deaths.cmp(&b.deaths))
                .then(a.id.cmp(&b.id))
        });
        rows
    }
}

pub fn local_player_id(net: &NetState) -> String {
    if net.player_id.is_empty() {
        "local".into()
    } else {
        net.player_id.clone()
    }
}

pub fn score_rows(net: &NetState, request: &LaunchRequest, score: &KillScore) -> Vec<ScoreRow> {
    let local_id = local_player_id(net);
    let mut ids = std::collections::BTreeSet::new();
    ids.extend(net.members.iter().map(|m| m.player_id.clone()));
    ids.extend(net.remote_actor_names.keys().cloned());
    ids.extend(net.remote_kills.keys().cloned());
    ids.extend(net.stock_deaths.keys().cloned());
    ids.extend(net.network_npc_ids.iter().cloned());

    ids.insert(local_id.clone());
    let mut rows = ids
        .into_iter()
        .map(|id| {
            let member = net.members.iter().find(|m| m.player_id == id);
            let local = id == local_id || (!net.player_id.is_empty() && id == net.player_id);
            let name = net
                .remote_actor_names
                .get(&id)
                .filter(|s| !s.is_empty())
                .cloned()
                .or_else(|| {
                    member
                        .filter(|m| !m.name.is_empty())
                        .map(|m| m.name.clone())
                })
                .unwrap_or_else(|| {
                    if local {
                        if request.player_name.is_empty() {
                            "玩家".into()
                        } else {
                            request.player_name.clone()
                        }
                    } else {
                        id.clone()
                    }
                });
            let team = if local && net.status != super::network_plugin::NetStatus::InRoom {
                offline_player_team(request)
            } else {
                member
                    .and_then(|m| m.team)
                    .map(|v| v as i32)
                    .or_else(|| net.remote_actor_teams.get(&id).copied())
                    .unwrap_or(if local { request.stock_camp as i32 } else { 0 })
            };
            let kills = net.remote_kills.get(&id).copied().unwrap_or_else(|| {
                if local {
                    score.player_kills
                } else {
                    0
                }
            });
            ScoreRow {
                deaths: net.stock_deaths.get(&id).copied().unwrap_or(0),
                id,
                name,
                team,
                kills,
            }
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.kills
            .cmp(&a.kills)
            .then(a.deaths.cmp(&b.deaths))
            .then(a.id.cmp(&b.id))
    });
    rows
}

pub fn local_result_bits(
    net: &NetState,
    request: &LaunchRequest,
    result: &NetworkBattleResult,
) -> i32 {
    if result.reason == "aborted" {
        return 0;
    }
    if let Some(winner) = &result.winner_player_id {
        return if *winner == net.player_id { 4 } else { 2 };
    }
    if result.winner_teams.len() != 1 {
        return 1;
    }
    let team = net
        .members
        .iter()
        .find(|m| m.player_id == net.player_id)
        .and_then(|m| m.team)
        .map(|v| v as i32)
        .or_else(|| net.remote_actor_teams.get(&net.player_id).copied())
        .unwrap_or(request.stock_camp as i32);
    if team == 0 {
        1
    } else if result.winner_teams.contains(&team) {
        4
    } else {
        2
    }
}

pub fn result_heading(net: &NetState, result: &NetworkBattleResult) -> String {
    if result.reason == "aborted" {
        return "战局已中止".into();
    }
    if let Some(id) = &result.winner_player_id {
        let name = net
            .remote_actor_names
            .get(id)
            .map(String::as_str)
            .or_else(|| {
                net.members
                    .iter()
                    .find(|m| &m.player_id == id)
                    .map(|m| m.name.as_str())
            })
            .unwrap_or(id);
        return format!("{name} · 盟主");
    }
    match result.winner_teams.as_slice() {
        [1] => "流星阵营胜利".into(),
        [2] => "蝴蝶阵营胜利".into(),
        [_, _, ..] => "双方得分".into(),
        _ if net.mode == "leader" && result.terminal => "战局结束 · 并列盟主".into(),
        _ if net.mode == "leader" => "时间到 · 本回合结束".into(),
        _ => "平局 · 回合结束".into(),
    }
}

pub fn score_text(net: &NetState, request: &LaunchRequest, score: &KillScore) -> String {
    format_score_rows(&score_rows(net, request, score), &local_player_id(net))
}

pub fn format_score_rows(rows: &[ScoreRow], local_id: &str) -> String {
    let mut lines = vec!["排名    姓名 / 阵营                 击杀    被打败".to_owned()];
    for (rank, row) in rows.iter().enumerate() {
        let camp = match row.team {
            1 => "流星",
            2 => "蝴蝶",
            _ => "个人",
        };
        let me = if row.id == local_id { "（你）" } else { "" };
        lines.push(format!(
            "{}    {}{} / {}        {}        {}",
            rank + 1,
            row.name,
            me,
            camp,
            row.kills,
            row.deaths
        ));
    }
    lines.join("\n")
}

pub(crate) fn gameplay_active(net: Option<Res<NetState>>) -> bool {
    net.is_none_or(|net| net.battle_result.is_none())
}

pub(crate) mod timing;
mod view;
pub(crate) use view::SettlementLayout;

pub struct BattleResultPlugin;

#[derive(Component)]
struct RoundOverlay;
#[derive(Component)]
struct RoundFooter;
#[derive(Component)]
struct RoundProgress;

impl Plugin for BattleResultPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<timing::BestTimes>()
            .init_resource::<timing::SettlementTime>()
            .add_systems(OnEnter(AppState::Loading), timing::reset)
            .add_systems(
                Update,
                timing::capture
                    .in_set(timing::CaptureSettlementTime)
                    .after(super::fighter_frame::SerialFighterSet),
            )
            .add_systems(
                Update,
                update_round_overlay.after(timing::CaptureSettlementTime),
            );
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_final_surface(
    commands: &mut Commands,
    root: Entity,
    font: &super::launcher_ui::UiFont,
    net: &NetState,
    request: &LaunchRequest,
    score: &KillScore,
    offline: &OfflineScores,
    bits: i32,
    elapsed: f32,
    timing: &timing::SettlementTime,
    records: &timing::BestTimes,
    layout: SettlementLayout,
) -> Entity {
    let mut data = if net.battle_result.is_some() {
        view::Presentation::network(net, request, score)
    } else {
        view::Presentation::offline(request, score, offline, bits)
    };
    data.elapsed_ms = timing.elapsed_ms.or_else(|| timing::milliseconds(elapsed));
    if net.battle_result.is_none() && request.mode == "single" && !request.spawn_room_npcs {
        data.best_time = Some(records.best(&request.stage_id));
        data.new_best = timing.new_best;
        data.save_failed = records.save_failed;
    }
    view::spawn_surface(commands, root, font, &data, layout)
}

pub(crate) fn settlement_overlay_node() -> Node {
    view::overlay_node()
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_round_overlay(
    mut commands: Commands,
    state: Res<State<AppState>>,
    time: Res<Time<bevy::time::Real>>,
    mut net: ResMut<NetState>,
    request: Res<LaunchRequest>,
    score: Res<KillScore>,
    timing: Res<timing::SettlementTime>,
    font: Res<super::launcher_ui::UiFont>,
    windows: Query<&Window>,
    scale: Option<Res<UiScale>>,
    roots: Query<Entity, With<RoundOverlay>>,
    mut footers: Query<&mut Text, With<RoundFooter>>,
    mut progress_bars: Query<&mut Node, With<RoundProgress>>,
    mut previous: Local<Option<(view::Presentation, SettlementLayout, bool)>>,
) {
    let visible =
        matches!(*state.get(), AppState::Playing | AppState::Paused) && net.battle_result.is_some();
    if !visible {
        for root in &roots {
            commands.entity(root).despawn();
        }
        *previous = None;
        return;
    }
    let result = net.battle_result.as_mut().unwrap();
    result.elapsed += time.delta_secs();
    let elapsed = result.elapsed;
    let terminal = result.terminal;
    let mut data = view::Presentation::network(&net, &request, &score);
    data.elapsed_ms = timing.elapsed_ms;
    let unit = scale.as_ref().map_or(1.0, |s| s.0).max(0.05);
    let layout = windows.single().map_or(SettlementLayout::Desktop, |w| {
        SettlementLayout::for_viewport(w.width() / unit, w.height() / unit)
    });
    let remaining = (10.0 - elapsed).max(0.0).ceil() as u32;
    let footer = if terminal {
        "本场对战结束 · 即将进入最终战绩".to_owned()
    } else if remaining > 0 {
        format!("下一回合即将开始    {remaining:02} 秒")
    } else {
        "等待房主开始下一回合".to_owned()
    };

    let signature = (data.clone(), layout, terminal);
    if previous.as_ref() != Some(&signature) || roots.is_empty() {
        for root in &roots {
            commands.entity(root).despawn();
        }
        let root = commands
            .spawn((
                view::overlay_node(),
                BackgroundColor(super::ui_theme::BACKGROUND),
                super::ui_theme::CinematicBackdrop,
                GlobalZIndex(290),
                bevy::ui::FocusPolicy::Block,
                super::ui_components::UiInputBlocker,
                RoundOverlay,
            ))
            .id();
        let panel = view::spawn_surface(&mut commands, root, &font, &data, layout);
        let foot = view::label(
            &mut commands,
            panel,
            &font,
            &footer,
            12.0,
            super::ui_theme::MUTED,
        );
        commands.entity(foot).insert((
            RoundFooter,
            Node {
                flex_shrink: 0.0,
                ..default()
            },
        ));
        if !terminal {
            let track = commands
                .spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(2.0),
                        flex_shrink: 0.0,
                        ..default()
                    },
                    BackgroundColor(super::ui_theme::LINE),
                ))
                .id();
            let bar = commands
                .spawn((
                    Node {
                        width: Val::Percent(((10.0 - elapsed) / 10.0).clamp(0.0, 1.0) * 100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    BackgroundColor(super::ui_theme::METAL),
                    RoundProgress,
                ))
                .id();
            commands.entity(track).add_child(bar);
            commands.entity(panel).add_child(track);
        }
        *previous = Some(signature);
    } else {
        for mut text in &mut footers {
            if text.0 != footer {
                text.0.clone_from(&footer);
            }
        }
        for mut bar in &mut progress_bars {
            bar.width = Val::Percent(((10.0 - elapsed) / 10.0).clamp(0.0, 1.0) * 100.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meteor_network::protocol::RoomMemberWire;

    fn room() -> NetState {
        let mut net = NetState::default();
        net.status = super::super::network_plugin::NetStatus::InRoom;
        net.player_id = "b".into();
        net.members = [("a", "甲", 1), ("b", "乙", 2), ("c", "丙", 2)]
            .into_iter()
            .map(|(id, name, team)| RoomMemberWire {
                player_id: id.into(),
                name: name.into(),
                character_id: 0,
                weapon: 0,
                team: Some(team),
                ready: true,
                connected: true,
            })
            .collect();
        net
    }

    #[test]
    fn winner_is_relative_to_local_camp_and_individual_identity() {
        let net = room();
        let request = LaunchRequest::default();
        let mut result = NetworkBattleResult {
            winner_teams: vec![2],
            ..default()
        };
        assert_eq!(local_result_bits(&net, &request, &result), 4);
        result.winner_teams = vec![1];
        assert_eq!(local_result_bits(&net, &request, &result), 2);
        result.winner_teams = vec![1, 2];
        assert_eq!(local_result_bits(&net, &request, &result), 1);
        result.winner_teams.clear();
        assert_eq!(local_result_bits(&net, &request, &result), 1);
        result.winner_player_id = Some("b".into());
        assert_eq!(local_result_bits(&net, &request, &result), 4);
        result.winner_player_id = Some("a".into());
        assert_eq!(local_result_bits(&net, &request, &result), 2);
    }

    #[test]
    fn scoreboard_includes_every_player_and_npc_with_stable_ranking() {
        let mut net = room();
        net.remote_kills = [("a".into(), 4), ("b".into(), 4), ("npc".into(), 2)]
            .into_iter()
            .collect();
        net.stock_deaths = [("a".into(), 3), ("b".into(), 1)].into_iter().collect();
        net.remote_actor_names.insert("npc".into(), "守卫".into());
        let rows = score_rows(&net, &LaunchRequest::default(), &KillScore::default());
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["b", "a", "npc", "c"]
        );
        assert_eq!((rows[0].kills, rows[0].deaths, rows[0].team), (4, 1, 2));
        let text = score_text(&net, &LaunchRequest::default(), &KillScore::default());
        assert!(text.contains("被打败"));
        assert!(text.contains("乙（你） / 蝴蝶"));
        assert!(text.contains("守卫"));
    }

    #[test]
    fn offline_scoreboard_keeps_local_player_row() {
        let rows = score_rows(
            &NetState::default(),
            &LaunchRequest {
                player_name: "剑客".into(),
                stock_camp: 1,
                ..default()
            },
            &KillScore {
                player_kills: 2,
                npc_kills: 1,
            },
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "local");
        assert_eq!(rows[0].name, "剑客");
        assert_eq!(rows[0].team, 1);
        assert_eq!(rows[0].kills, 2);
        let text = score_text(
            &NetState::default(),
            &LaunchRequest {
                player_name: "剑客".into(),
                ..default()
            },
            &KillScore {
                player_kills: 2,
                npc_kills: 0,
            },
        );
        assert!(text.contains("剑客（你）"));
    }

    #[test]
    fn solo_score_row_ignores_disconnected_room_camp() {
        let mut net = room();
        net.status = super::super::network_plugin::NetStatus::Disconnected;
        let rows = score_rows(
            &net,
            &LaunchRequest {
                stock_camp: 2,
                ..default()
            },
            &KillScore::default(),
        );
        assert_eq!(rows.iter().find(|row| row.id == "b").unwrap().team, 1);
    }

    #[test]
    fn offline_totals_are_per_actor_and_survive_roster_refresh() {
        let mut scores = OfflineScores::default();
        let npc = offline_actor_id(false, Some(7), 1);
        scores.roster.insert(
            npc.clone(),
            ScoreRow {
                id: npc.clone(),
                name: "对手".into(),
                team: 2,
                kills: 0,
                deaths: 0,
            },
        );
        scores.record_death("local".into(), Some(npc.clone()));
        scores.record_death("local".into(), None);
        scores.record_death(npc.clone(), Some("local".into()));

        scores.roster.get_mut(&npc).unwrap().name = "对手复活".into();
        let rows = scores.rows(
            &LaunchRequest::default(),
            &KillScore {
                player_kills: 1,
                npc_kills: 1,
            },
        );
        let local = rows.iter().find(|row| row.id == "local").unwrap();
        let opponent = rows.iter().find(|row| row.id == npc).unwrap();
        assert_eq!((local.kills, local.deaths), (1, 2));
        assert_eq!((opponent.kills, opponent.deaths), (1, 1));
        assert!(format_score_rows(&rows, "local").contains("对手复活"));
    }

    #[test]
    fn round_overlay_appears_updates_and_clears_without_leaving_playing() {
        let mut net = room();
        net.battle_result = Some(NetworkBattleResult {
            round: 3,
            winner_teams: vec![2],
            ..default()
        });
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(State::new(AppState::Playing))
            .insert_resource(net)
            .init_resource::<LaunchRequest>()
            .init_resource::<KillScore>()
            .insert_resource(super::super::launcher_ui::UiFont(Handle::default()))
            .init_resource::<timing::SettlementTime>()
            .add_systems(Update, update_round_overlay);
        app.update();
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, With<RoundOverlay>>()
                .iter(world)
                .count(),
            1
        );
        assert!(world
            .query::<&Text>()
            .iter(world)
            .any(|t| t.0.contains("蝴蝶阵营胜利")));
        app.world_mut().resource_mut::<NetState>().battle_result = None;
        app.update();
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, With<RoundOverlay>>()
                .iter(world)
                .count(),
            0
        );
        assert_eq!(
            *world.resource::<State<AppState>>().get(),
            AppState::Playing
        );
    }
}
