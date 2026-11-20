use bevy::prelude::*;

use crate::app_state::{AppState, LaunchRequest, PlayingRoot};
use crate::components::fighter::{DeathRespawn, MatchEnd};
use crate::plugins::launcher_ui::UiFont;
use crate::plugins::network_plugin::NetState;
use crate::plugins::stage_plugin::{LocalFighter, StageMissionRuntime};
use crate::plugins::ui_plugin::radar_screen_geometry;
use crate::plugins::ui_scale_plugin::SafeAreaInsets;
use crate::plugins::ui_theme as theme;

const ROUND_FONT_SIZE: f32 = 14.0;
const DEATH_FONT_SIZE: f32 = 28.0;

#[derive(Resource, Debug, Default)]
struct DeathCountdownTrack {
    remaining: Option<f32>,
}

#[derive(Component)]
struct RoundCountdownRoot;

#[derive(Component)]
struct RoundCountdownHud;

#[derive(Component)]
struct DeathCountdownHud;

pub struct CountdownHudPlugin;

impl Plugin for CountdownHudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DeathCountdownTrack>()
            .add_systems(
                OnEnter(AppState::Playing),
                setup_countdown_hud.run_if(crate::plugins::starts_fresh_playing_session),
            )
            .add_systems(OnEnter(AppState::Loading), reset_death_countdown_track)
            .add_systems(
                Update,
                (
                    update_round_countdown,
                    update_death_countdown,
                    adapt_countdown_layout,
                )
                    .chain()
                    .run_if(in_state(AppState::Playing).or_else(in_state(AppState::Paused))),
            );
    }
}

fn reset_death_countdown_track(mut track: ResMut<DeathCountdownTrack>) {
    *track = DeathCountdownTrack::default();
}

fn setup_countdown_hud(mut commands: Commands, ui_font: Res<UiFont>) {
    let font = ui_font.0.clone();

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Px(152.0),
                display: Display::None,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            RoundCountdownRoot,
            PlayingRoot,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_shrink: 0.0,
                    ..default()
                },
                Text::new(""),
                TextFont {
                    font: font.clone().into(),
                    font_size: ROUND_FONT_SIZE.into(),
                    ..default()
                },
                TextLayout::linebreak(bevy::text::LineBreak::NoWrap),
                TextColor(theme::TEXT),
                bevy::ui::widget::TextShadow {
                    offset: Vec2::splat(1.0),
                    color: Color::BLACK,
                },
                RoundCountdownHud,
            ));
        });
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Percent(50.0),
            left: Val::Percent(50.0),
            width: Val::Px(420.0),
            margin: UiRect::left(Val::Px(-210.0)),
            display: Display::None,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        Text::new(""),
        TextFont {
            font: font.into(),
            font_size: DEATH_FONT_SIZE.into(),
            ..default()
        },
        TextLayout::new(
            bevy::text::Justify::Center,
            bevy::text::LineBreak::WordOrCharacter,
        ),
        TextColor(Color::srgb(1.0, 0.88, 0.55)),
        bevy::ui::widget::TextShadow {
            offset: Vec2::splat(2.0),
            color: Color::BLACK,
        },
        DeathCountdownHud,
        PlayingRoot,
    ));
}

pub(crate) fn resolve_round_limit_seconds(
    request: &LaunchRequest,
    mission: Option<&StageMissionRuntime>,
    end: &MatchEnd,
    net_round_minutes: Option<u32>,
) -> Option<i32> {
    if request.mode == "replay" || end.settled {
        return None;
    }
    let minutes = if request.mode == "single" {
        mission
            .and_then(|runtime| runtime.script())
            .and_then(|script| script.values().get("RoundTime"))
            .map(|value| value.as_f64() as i32)
            .filter(|&m| m > 0)
            .or(Some(meteor_network::modes::DEFAULT_ROUND_TIME_MINUTES))
    } else {
        net_round_minutes
            .filter(|&m| m > 0)
            .or_else(|| {
                (1..=999)
                    .contains(&request.stock_game_time_minutes)
                    .then_some(request.stock_game_time_minutes)
            })
            .map(|m| m as i32)
    }?;
    Some(meteor_network::modes::round_time_limit_seconds(Some(
        minutes,
    )))
}

pub(crate) fn format_round_remaining(remaining_secs: f32) -> String {
    let total = remaining_secs.max(0.0).ceil() as u32;
    format!("{}:{:02}", total / 60, total % 60)
}

pub(crate) fn format_death_remaining(remaining_secs: f32) -> String {
    let n = remaining_secs.max(0.0).ceil() as u32;
    format!("{n} 秒后重返战场")
}

fn update_round_countdown(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    request: Res<LaunchRequest>,
    end: Res<MatchEnd>,
    mut net: ResMut<NetState>,
    mission: Option<Res<StageMissionRuntime>>,
    mut roots: Query<&mut Node, With<RoundCountdownRoot>>,
    mut texts: Query<(&mut Text, &mut TextColor), With<RoundCountdownHud>>,
) {
    let in_room = net.status == crate::plugins::network_plugin::NetStatus::InRoom;

    if in_room && !net.round_active {
        for mut node in &mut roots {
            node.display = Display::None;
        }
        for (mut text, _) in &mut texts {
            if !text.0.is_empty() {
                text.0.clear();
            }
        }
        return;
    }

    let limit = resolve_round_limit_seconds(
        request.as_ref(),
        mission.as_deref(),
        end.as_ref(),
        net.round_time_minutes,
    );

    if in_room && net.round_remaining_seconds.is_none() {
        if let Some(limit_secs) = limit {
            net.round_remaining_seconds = Some((limit_secs as f32 - end.elapsed).max(0.0));
            if net.round_time_minutes.is_none() {
                net.round_time_minutes = Some((limit_secs as u32).div_ceil(60).max(1));
            }
        }
    }

    let remaining = if let Some(tracked) = net.round_remaining_seconds {
        let dt = if request.mode == "replay" {
            0.0
        } else {
            crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs())
        };
        let next = (tracked - dt.max(0.0)).max(0.0);
        net.round_remaining_seconds = Some(next);
        Some(next)
    } else {
        limit.map(|limit_secs| (limit_secs as f32 - end.elapsed).max(0.0))
    };

    let value = remaining.map(format_round_remaining);
    for mut node in &mut roots {
        node.display = if value.is_some() {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (mut text, mut color) in &mut texts {
        match &value {
            Some(v) => {
                let urgent = remaining.is_some_and(|secs| secs <= 10.0);
                color.0 = if urgent {
                    Color::srgb(1.0, 0.35, 0.28)
                } else {
                    theme::TEXT
                };
                if text.0 != *v {
                    text.0 = v.clone();
                }
            }
            None => {
                if !text.0.is_empty() {
                    text.0.clear();
                }
            }
        }
    }
}

fn update_death_countdown(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    request: Res<LaunchRequest>,
    end: Res<MatchEnd>,
    net: Res<NetState>,
    mission: Option<Res<StageMissionRuntime>>,
    mut track: ResMut<DeathCountdownTrack>,
    local: Query<&DeathRespawn, With<LocalFighter>>,
    mut texts: Query<(&mut Text, &mut Node), With<DeathCountdownHud>>,
) {
    if request.mode == "replay" || end.settled {
        track.remaining = None;
        for (mut text, mut node) in &mut texts {
            node.display = Display::None;
            if !text.0.is_empty() {
                text.0.clear();
            }
        }
        return;
    }

    let rule = if request.mode == "single" {
        Some(mission.as_deref().map_or(
            meteor_network::modes::DEFAULT_LEVEL_RULE,
            StageMissionRuntime::rule,
        ))
    } else if (1..=5).contains(&request.stock_rule) {
        Some(request.stock_rule)
    } else {
        meteor_network::modes::mode_rule_id(&request.mode)
    };
    let allow_respawn = meteor_network::modes::can_respawn(rule);

    let local_death = local.iter().next();
    let active = local_death.is_some_and(|death| death.active);
    if !active || !allow_respawn {
        track.remaining = None;
        for (mut text, mut node) in &mut texts {
            node.display = Display::None;
            if !text.0.is_empty() {
                text.0.clear();
            }
        }
        return;
    }

    let death = local_death.expect("active checked");

    let seed = if death.timer > f32::EPSILON {
        death.timer
    } else {
        meteor_combat::death::RESPAWN_DELAY_SECONDS
    };
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let remaining = match track.remaining {
        Some(prev)
            if net.status == crate::plugins::network_plugin::NetStatus::InRoom
                || death.timer <= f32::EPSILON =>
        {
            (prev - dt.max(0.0)).max(0.0)
        }
        Some(_) => death.timer.max(0.0),
        None => seed,
    };
    track.remaining = Some(remaining);

    let value = format_death_remaining(remaining);
    for (mut text, mut node) in &mut texts {
        node.display = Display::Flex;
        if text.0 != value {
            text.0 = value.clone();
        }
    }
}

fn adapt_countdown_layout(
    window: Query<&Window>,
    safe_area: Option<Res<SafeAreaInsets>>,
    ui_scale: Res<crate::plugins::ui_scale_plugin::EffectiveUiScale>,
    resolved: Option<Res<crate::plugins::mobile_controls_plugin::ResolvedMobileLayout>>,
    mut round: Query<&mut Node, (With<RoundCountdownRoot>, Without<DeathCountdownHud>)>,
    mut fonts: Query<&mut TextFont, With<RoundCountdownHud>>,
    mut death: Query<&mut Node, (With<DeathCountdownHud>, Without<RoundCountdownRoot>)>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let safe = safe_area
        .as_deref()
        .copied()
        .unwrap_or_default()
        .sanitized(window.width(), window.height());
    let unit = ui_scale.0.max(0.05);

    let (map_center, map_diameter) = radar_screen_geometry(window, safe, resolved.as_deref());
    let round_w = map_diameter / unit;
    let round_left = (map_center.x - map_diameter * 0.5) / unit;

    let round_top = (map_center.y + map_diameter * 0.5 + 5.0) / unit;

    for mut node in &mut round {
        node.position_type = PositionType::Absolute;
        node.top = Val::Px(round_top);
        node.left = Val::Px(round_left);
        node.width = Val::Px(round_w);
        node.height = Val::Auto;
        node.right = Val::Auto;
        node.bottom = Val::Auto;
        node.margin = UiRect::ZERO;
        node.padding = UiRect::ZERO;
        node.justify_content = JustifyContent::Center;
        node.align_items = AlignItems::Center;
    }
    for mut font in &mut fonts {
        let size = (ROUND_FONT_SIZE / unit).into();
        if font.font_size != size {
            font.font_size = size;
        }
    }

    let content_h = (window.height() - safe.top - safe.bottom).max(1.0);
    let center_y = safe.top + content_h * 0.5;
    let death_w = 420.0_f32;
    let death_left = safe.left + (window.width() - safe.left - safe.right - death_w) * 0.5;
    for mut node in &mut death {
        node.top = Val::Px((center_y - 14.0) / unit);
        node.left = Val::Px(death_left / unit);
        node.width = Val::Px(death_w / unit);
        node.margin = UiRect::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_and_death_formatters() {
        assert_eq!(format_round_remaining(125.1), "2:06");
        assert_eq!(format_round_remaining(9.2), "0:10");
        assert_eq!(format_round_remaining(0.0), "0:00");
        assert_eq!(format_death_remaining(2.1), "3 秒后重返战场");
        assert_eq!(format_death_remaining(0.01), "1 秒后重返战场");
        assert_eq!(format_death_remaining(0.0), "0 秒后重返战场");
    }

    #[test]
    fn stock_default_999_minutes_still_shows_countdown() {
        let request = LaunchRequest {
            mode: "leader".into(),
            stock_game_time_minutes: 999,
            ..Default::default()
        };
        assert_eq!(
            resolve_round_limit_seconds(&request, None, &MatchEnd::default(), None),
            Some(999 * 60)
        );
    }

    #[test]
    fn stock_minutes_become_seconds() {
        let request = LaunchRequest {
            mode: "leader".into(),
            stock_game_time_minutes: 10,
            ..Default::default()
        };
        assert_eq!(
            resolve_round_limit_seconds(&request, None, &MatchEnd::default(), None),
            Some(600)
        );
    }

    #[test]
    fn settled_match_hides_round_countdown() {
        let request = LaunchRequest {
            mode: "leader".into(),
            stock_game_time_minutes: 10,
            ..Default::default()
        };
        let end = MatchEnd {
            settled: true,
            ..Default::default()
        };
        assert!(resolve_round_limit_seconds(&request, None, &end, None).is_none());
    }

    #[test]
    fn net_round_minutes_override_local_request() {
        let request = LaunchRequest {
            mode: "leader".into(),
            stock_game_time_minutes: 999,
            ..Default::default()
        };
        assert_eq!(
            resolve_round_limit_seconds(&request, None, &MatchEnd::default(), Some(15)),
            Some(15 * 60)
        );
    }
}
