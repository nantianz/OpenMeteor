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

pub fn score_rows(net: &NetState, request: &LaunchRequest, score: &KillScore) -> Vec<ScoreRow> {
    let mut ids = std::collections::BTreeSet::new();
    ids.extend(net.members.iter().map(|m| m.player_id.clone()));
    ids.extend(net.remote_actor_names.keys().cloned());
    ids.extend(net.remote_kills.keys().cloned());
    ids.extend(net.stock_deaths.keys().cloned());
    ids.extend(net.network_npc_ids.iter().cloned());
    if !net.player_id.is_empty() {
        ids.insert(net.player_id.clone());
    }
    let mut rows = ids
        .into_iter()
        .map(|id| {
            let member = net.members.iter().find(|m| m.player_id == id);
            let local = id == net.player_id;
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
                        request.player_name.clone()
                    } else {
                        id.clone()
                    }
                });
            let team = member
                .and_then(|m| m.team)
                .map(|v| v as i32)
                .or_else(|| net.remote_actor_teams.get(&id).copied())
                .unwrap_or(if local { request.stock_camp as i32 } else { 0 });
            let kills = net.remote_kills.get(&id).copied().unwrap_or_else(|| {
                if local {
                    score.player_kills.saturating_add(score.npc_kills)
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
    let mut lines = vec!["排名    姓名 / 阵营                 击杀    被打败".to_owned()];
    for (rank, row) in score_rows(net, request, score).iter().enumerate() {
        let camp = match row.team {
            1 => "流星",
            2 => "蝴蝶",
            _ => "个人",
        };
        let me = if row.id == net.player_id {
            "（你）"
        } else {
            ""
        };
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

pub struct BattleResultPlugin;

#[derive(Component)]
struct RoundOverlay;
#[derive(Component)]
struct RoundHeading;
#[derive(Component)]
struct RoundScores;
#[derive(Component)]
struct RoundFooter;
#[derive(Component)]
struct ResultEmblem;
#[derive(Resource, Default)]
struct ResultArt([Option<Handle<Image>>; 3]);

impl Plugin for BattleResultPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ResultArt>()
            .add_systems(Startup, load_result_art)
            .add_systems(
                Update,
                update_round_overlay.after(super::fighter_frame::SerialFighterSet),
            );
    }
}

fn load_result_art(mut art: ResMut<ResultArt>, mut images: ResMut<Assets<Image>>) {
    for (i, name) in ["sheng.bmp", "fu.bmp", "he.bmp"].iter().enumerate() {
        let bytes = crate::resource_io::required(&format!("image/{name}"));
        let Ok(mut decoded) = meteor_asset::texture::decode_image(&bytes) else {
            continue;
        };

        for pixel in decoded.rgba.chunks_exact_mut(4) {
            pixel[3] = pixel[0].max(pixel[1]).max(pixel[2]);
        }
        art.0[i] = Some(super::pak_textures::image_from_decoded(
            &mut images,
            &decoded,
        ));
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_round_overlay(
    mut commands: Commands,
    state: Res<State<AppState>>,
    time: Res<Time<bevy::time::Real>>,
    mut net: ResMut<NetState>,
    request: Res<LaunchRequest>,
    score: Res<KillScore>,
    font: Res<super::launcher_ui::UiFont>,
    art: Res<ResultArt>,
    roots: Query<Entity, With<RoundOverlay>>,
    mut labels: Query<(
        &mut Text,
        Option<&RoundHeading>,
        Option<&RoundScores>,
        Option<&RoundFooter>,
    )>,
    mut emblems: Query<(&mut Node, &mut ImageNode), With<ResultEmblem>>,
) {
    let visible =
        matches!(*state.get(), AppState::Playing | AppState::Paused) && net.battle_result.is_some();
    if !visible {
        for root in &roots {
            commands.entity(root).despawn();
        }
        return;
    }
    let result = net.battle_result.as_mut().unwrap();
    result.elapsed += time.delta_secs();
    let result = result.clone();
    let bits = local_result_bits(&net, &request, &result);
    let title = format!(
        "{} · 第 {} 回合\n{}",
        if result.terminal {
            "战局结算"
        } else {
            "回合结算"
        },
        result.round.max(1),
        result_heading(&net, &result)
    );
    let scores = score_text(&net, &request, &score);
    let remaining = (10.0 - result.elapsed).max(0.0).ceil() as u32;
    let footer = format!(
        "流星 {} : {} 蝴蝶\n{}",
        net.stock_team_wins[0],
        net.stock_team_wins[1],
        if result.terminal {
            "战局已结束".to_owned()
        } else if remaining > 0 {
            format!("{remaining} 秒后开始下一回合")
        } else {
            "等待房主开始下一回合".to_owned()
        }
    );
    if roots.is_empty() {
        let root = commands
            .spawn((
                super::launcher_ui::scrollable_y(Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(8.0),
                    top: Val::Percent(4.0),
                    width: Val::Percent(84.0),
                    max_height: Val::Percent(88.0),
                    padding: UiRect::all(Val::Px(20.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(10.0),
                    ..default()
                }),
                BackgroundColor(Color::srgba(0.025, 0.03, 0.045, 0.88)),
                GlobalZIndex(290),
                super::ui_components::UiInputBlocker,
                RoundOverlay,
            ))
            .id();
        let image_index = if bits == 4 {
            0
        } else if bits == 2 {
            1
        } else {
            2
        };
        if let Some(handle) = &art.0[image_index] {
            let emblem = commands
                .spawn((
                    Node {
                        width: Val::Px(150.0),
                        height: Val::Px(100.0),
                        align_self: AlignSelf::Center,
                        ..default()
                    },
                    ImageNode::new(handle.clone()),
                    ResultEmblem,
                ))
                .id();
            commands.entity(root).add_child(emblem);
        }
        for (index, content) in [title, scores, footer].into_iter().enumerate() {
            let mut label = commands.spawn((
                Text::new(content),
                TextFont {
                    font: font.0.clone().into(),
                    font_size: (if index == 0 { 26.0 } else { 18.0 }).into(),
                    ..default()
                },
                TextColor(if index == 0 {
                    Color::srgb(1.0, 0.83, 0.45)
                } else {
                    Color::WHITE
                }),
            ));
            match index {
                0 => {
                    label.insert(RoundHeading);
                }
                1 => {
                    label.insert(RoundScores);
                }
                _ => {
                    label.insert(RoundFooter);
                }
            }
            let child = label.id();
            commands.entity(root).add_child(child);
        }
    } else {
        for (mut text, heading, rows, foot) in &mut labels {
            let value = if heading.is_some() {
                &title
            } else if rows.is_some() {
                &scores
            } else if foot.is_some() {
                &footer
            } else {
                continue;
            };
            if text.0 != *value {
                text.0.clone_from(value);
            }
        }
    }

    let progress = (result.elapsed / 0.55).clamp(0.0, 1.0);
    let scale = 1.0 + 0.3 * (1.0 - progress).powi(3);
    let image_index = if bits == 4 {
        0
    } else if bits == 2 {
        1
    } else {
        2
    };
    for (mut node, mut image) in &mut emblems {
        node.width = Val::Px(150.0 * scale);
        node.height = Val::Px(100.0 * scale);
        image.color = Color::srgba(1.0, 1.0, 1.0, progress);
        if let Some(handle) = &art.0[image_index] {
            image.image = handle.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meteor_network::protocol::RoomMemberWire;

    #[cfg(feature = "original-assets")]
    #[test]
    fn original_result_images_are_available_without_external_resource_directories() {
        let mut app = App::new();
        app.init_resource::<ResultArt>()
            .init_resource::<Assets<Image>>()
            .add_systems(Update, load_result_art);
        app.update();
        let images = app.world().resource::<Assets<Image>>();
        for handle in &app.world().resource::<ResultArt>().0 {
            let image = images
                .get(handle.as_ref().expect("original result bitmap"))
                .unwrap();
            assert_eq!(image.size(), UVec2::new(300, 200));
        }
    }

    fn room() -> NetState {
        let mut net = NetState::default();
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
            .init_resource::<ResultArt>()
            .insert_resource(super::super::launcher_ui::UiFont(Handle::default()))
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
