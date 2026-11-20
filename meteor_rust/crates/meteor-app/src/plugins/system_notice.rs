use bevy::prelude::*;
use std::collections::VecDeque;

use crate::app_state::{AppState, LaunchRequest, PlayingRoot, RoomState};
use crate::plugins::launcher_ui::UiFont;
use crate::plugins::network_plugin::{NetState, NetStatus};
use crate::plugins::ui_scale_plugin::SafeAreaInsets;
use crate::plugins::ui_theme as theme;

const NOTICE_SECONDS: f32 = 3.5;
const MAX_PENDING: usize = 24;
const MAX_VISIBLE_LINES: usize = 3;

pub mod mgstring {
    pub const ENTER: &str = "进入战场";
    pub const LEAVE: &str = "离开战场";
    pub const DEFEAT: &str = "击败 ";
    pub const DEATH: &str = "死亡";
    pub const FLAG_TAKEN: &str = "夺得镖物";
    pub const FLAG_LOST: &str = "失落镖物";
    pub const FLAG_RETURNED: &str = "镖物归位";
    pub const NEW_ROUND: &str = "新回合开始计时";
}

#[derive(Resource, Debug, Default)]
pub struct SystemNoticeFeed {
    pending: VecDeque<String>,

    visible: VecDeque<(String, f32)>,
}

impl SystemNoticeFeed {
    pub fn push(&mut self, text: impl Into<String>) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }
        if self.pending.back().is_some_and(|last| last == &text)
            || self.visible.back().is_some_and(|(last, _)| last == &text)
        {
            return;
        }
        self.pending.push_back(text);
        while self.pending.len() > MAX_PENDING {
            self.pending.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.visible.clear();
    }

    fn tick(&mut self, dt: f32) {
        for (_, remaining) in &mut self.visible {
            *remaining -= dt;
        }
        while self
            .visible
            .front()
            .is_some_and(|(_, remaining)| *remaining <= 0.0)
        {
            self.visible.pop_front();
        }
        while self.visible.len() < MAX_VISIBLE_LINES {
            let Some(next) = self.pending.pop_front() else {
                break;
            };
            self.visible.push_back((next, NOTICE_SECONDS));
        }
    }

    fn display_text(&self) -> String {
        self.visible
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn format_enter(name: &str) -> String {
    format!("{name} {}", mgstring::ENTER)
}

pub fn format_leave(name: &str) -> String {
    format!("{name} {}", mgstring::LEAVE)
}

pub fn format_defeat(attacker: &str, target: &str) -> String {
    format!("{attacker} {}{target}", mgstring::DEFEAT)
}

pub fn format_death(target: &str) -> String {
    format!("{target} {}", mgstring::DEATH)
}

pub fn format_kill_or_death(attacker: Option<&str>, target: &str) -> String {
    match attacker {
        Some(attacker) if !attacker.is_empty() && attacker != target => {
            format_defeat(attacker, target)
        }
        _ => format_death(target),
    }
}

pub fn format_flag_taken(name: &str) -> String {
    format!("{name} {}", mgstring::FLAG_TAKEN)
}

pub fn format_flag_lost(name: &str) -> String {
    format!("{name} {}", mgstring::FLAG_LOST)
}

pub fn format_flag_returned() -> String {
    mgstring::FLAG_RETURNED.to_string()
}

pub fn format_new_round(round: Option<u32>) -> String {
    match round {
        Some(round) if round > 0 => format!("第 {round} 回合 · {}", mgstring::NEW_ROUND),
        _ => mgstring::NEW_ROUND.to_string(),
    }
}

pub fn resolve_actor_name(
    net: &NetState,
    request: &LaunchRequest,
    room: &RoomState,
    player_id: &str,
) -> String {
    if !player_id.is_empty() && player_id == net.player_id {
        if !request.player_name.trim().is_empty() {
            return request.player_name.clone();
        }
    }
    if let Some(name) = net.remote_actor_names.get(player_id) {
        if !name.trim().is_empty() {
            return name.clone();
        }
    }
    if let Some(member) = net.members.iter().find(|m| m.player_id == player_id) {
        if !member.name.trim().is_empty() {
            return member.name.clone();
        }
    }
    if let Some(member) = room.members.iter().find(|m| m.name == player_id) {
        if !member.name.trim().is_empty() {
            return member.name.clone();
        }
    }
    if let Some(rest) = player_id.strip_prefix("stock-") {
        return format!("原版玩家 {rest}");
    }
    if player_id.is_empty() {
        "未知".into()
    } else {
        player_id.to_string()
    }
}

pub fn resolve_local_combatant_name(
    request: &LaunchRequest,
    net: &NetState,
    room: &RoomState,
    local: bool,
    remote_player_id: Option<&str>,
    mission_name: Option<&str>,
    player_slot: u8,
) -> String {
    if let Some(name) = mission_name.filter(|name| !name.trim().is_empty()) {
        return name.to_string();
    }
    if local {
        if net.status != NetStatus::InRoom {
            if let Some(name) = net.offline_script_player_name.as_ref() {
                return name.clone();
            }
        }
        return if request.player_name.trim().is_empty() {
            "玩家".into()
        } else {
            request.player_name.clone()
        };
    }
    if let Some(player_id) = remote_player_id {
        return resolve_actor_name(net, request, room, player_id);
    }
    room.members
        .get(player_slot as usize)
        .map(|member| member.name.clone())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("角色{}", player_slot))
}

#[derive(Component)]
struct SystemNoticeHud;

pub struct SystemNoticePlugin;

impl Plugin for SystemNoticePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SystemNoticeFeed>()
            .add_systems(
                OnEnter(AppState::Playing),
                (
                    setup_system_notice_hud.run_if(crate::plugins::starts_fresh_playing_session),
                    announce_local_enter.run_if(crate::plugins::starts_fresh_playing_session),
                )
                    .chain(),
            )
            .add_systems(OnEnter(AppState::Loading), reset_system_notices)
            .add_systems(
                Update,
                (
                    drain_network_notices,
                    tick_system_notices,
                    adapt_system_notice_layout,
                )
                    .chain()
                    .run_if(in_state(AppState::Playing).or_else(in_state(AppState::Paused))),
            );
    }
}

fn reset_system_notices(mut feed: ResMut<SystemNoticeFeed>) {
    feed.clear();
}

fn announce_local_enter(
    mut feed: ResMut<SystemNoticeFeed>,
    request: Res<LaunchRequest>,
    net: Res<NetState>,
) {
    if request.mode == "replay" || net.status == NetStatus::InRoom {
        return;
    }
    let name = if request.player_name.trim().is_empty() {
        "玩家".into()
    } else {
        request.player_name.clone()
    };
    feed.push(format_enter(&name));
}

fn setup_system_notice_hud(mut commands: Commands, ui_font: Res<UiFont>) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(72.0),
            left: Val::Percent(50.0),
            width: Val::Px(520.0),
            margin: UiRect::left(Val::Px(-260.0)),
            display: Display::None,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
            ..default()
        },
        Text::new(""),
        TextFont {
            font: ui_font.0.clone().into(),
            font_size: 16.0.into(),
            ..default()
        },
        TextLayout::new(
            bevy::text::Justify::Center,
            bevy::text::LineBreak::WordOrCharacter,
        ),
        TextColor(Color::srgb(1.0, 0.92, 0.55)),
        bevy::ui::widget::TextShadow {
            offset: Vec2::splat(1.0),
            color: Color::BLACK,
        },
        BackgroundColor(theme::HUD_SURFACE.with_alpha(0.55)),
        SystemNoticeHud,
        PlayingRoot,
    ));
}

fn drain_network_notices(mut net: ResMut<NetState>, mut feed: ResMut<SystemNoticeFeed>) {
    if net.pending_system_notices.is_empty() {
        return;
    }
    for text in net.pending_system_notices.drain(..) {
        feed.push(text);
    }
}

fn tick_system_notices(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut feed: ResMut<SystemNoticeFeed>,
    mut texts: Query<(&mut Text, &mut Node, &mut BackgroundColor), With<SystemNoticeHud>>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    feed.tick(dt.max(0.0));
    let value = feed.display_text();
    for (mut text, mut node, mut background) in &mut texts {
        node.display = if value.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
        background.0 = if value.is_empty() {
            Color::NONE
        } else {
            theme::HUD_SURFACE.with_alpha(0.55)
        };
        if text.0 != value {
            text.0 = value.clone();
        }
    }
}

fn adapt_system_notice_layout(
    window: Query<&Window>,
    safe_area: Option<Res<SafeAreaInsets>>,
    ui_scale: Res<crate::plugins::ui_scale_plugin::EffectiveUiScale>,
    mut nodes: Query<&mut Node, With<SystemNoticeHud>>,
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
    let width = (window.width() - safe.left - safe.right - 32.0).clamp(
        240.0,
        if cfg!(target_os = "android") {
            420.0
        } else {
            560.0
        },
    );
    let top = safe.top + (window.height() * 0.11).clamp(48.0, 88.0);
    let left = safe.left + (window.width() - safe.left - safe.right - width) * 0.5;
    for mut node in &mut nodes {
        node.top = Val::Px(top / unit);
        node.left = Val::Px(left / unit);
        node.width = Val::Px(width / unit);
        node.margin = UiRect::ZERO;
    }
}

pub fn queue_net_notice(net: &mut NetState, text: impl Into<String>) {
    let text = text.into();
    if text.trim().is_empty() {
        return;
    }
    if net
        .pending_system_notices
        .last()
        .is_some_and(|last| last == &text)
    {
        return;
    }
    net.pending_system_notices.push(text);
    if net.pending_system_notices.len() > MAX_PENDING {
        let excess = net.pending_system_notices.len() - MAX_PENDING;
        net.pending_system_notices.drain(..excess);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mgstring_phrasing_matches_web_and_table() {
        assert_eq!(format_enter("冷燕"), "冷燕 进入战场");
        assert_eq!(format_leave("孟星魂"), "孟星魂 离开战场");
        assert_eq!(format_defeat("甲", "乙"), "甲 击败 乙");
        assert_eq!(format_death("乙"), "乙 死亡");
        assert_eq!(format_kill_or_death(Some("甲"), "乙"), "甲 击败 乙");
        assert_eq!(format_kill_or_death(None, "乙"), "乙 死亡");
        assert_eq!(format_kill_or_death(Some("乙"), "乙"), "乙 死亡");
        assert_eq!(format_flag_taken("甲"), "甲 夺得镖物");
        assert_eq!(format_flag_lost("甲"), "甲 失落镖物");
        assert_eq!(format_flag_returned(), "镖物归位");
        assert_eq!(format_new_round(Some(2)), "第 2 回合 · 新回合开始计时");
        assert_eq!(format_new_round(None), "新回合开始计时");
    }

    #[test]
    fn feed_queues_and_expires_lines() {
        let mut feed = SystemNoticeFeed::default();
        feed.push("a 进入战场");
        feed.push("b 击败 a");
        feed.tick(0.0);
        assert_eq!(feed.display_text(), "a 进入战场\nb 击败 a");
        feed.tick(NOTICE_SECONDS + 0.1);
        assert!(feed.display_text().contains("b 击败 a") || feed.display_text().is_empty());
        feed.push("镖物归位");
        feed.tick(0.0);
        assert!(feed.display_text().contains("镖物归位"));
    }

    #[test]
    fn feed_dedupes_adjacent_duplicates() {
        let mut feed = SystemNoticeFeed::default();
        feed.push("甲 进入战场");
        feed.push("甲 进入战场");
        assert_eq!(feed.pending.len(), 1);
    }
}
