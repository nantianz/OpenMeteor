use crate::app_state::{
    AppState, Keymap, LaunchRequest, LauncherStyledButton, RoomState, Settings,
};
use crate::components::fighter::{
    AiIntent, CharacterVariantChanged, CombatState, DeathRespawn, EntryBlend, Fighter,
    FighterHitReaction, Health, LockOnController, MotionState, PoseRuntime, Qi, SmState,
    TargetHitClock, VisualRootPin, WeaponForm, WeaponSwitchActive, WeaponVariantChanged,
};
use crate::plugins::camera_plugin::{
    CameraMode, CameraRecentreRequest, GameplayCamera, ThirdPersonState,
};
use crate::plugins::character_plugin::CharacterAssetsSet;
use crate::plugins::combat_plugin::{HitGeometryQueries, NpcFighter};
use crate::plugins::input_plugin::{LocalJumpInput, SyntheticAction, SyntheticInputStream};
use crate::plugins::launcher_ui::LauncherScrollArea;
use crate::plugins::movement_plugin::{
    directional_input, ordinary_locomotion_allowed, FighterAir, FighterAttackMotion, FighterDodge,
    FighterWall,
};
use crate::plugins::stage_collider::StageCollider;
use crate::plugins::stage_plugin::{LocalFighter, MissionNpc, NpcCombatProfile, NpcPerformance};
use crate::plugins::ui_scale_plugin::SafeAreaInsets;
use bevy::gizmos as bevy_gizmos;
use bevy::gizmos::{
    config::{GizmoConfig, GizmoConfigGroup},
    AppGizmoBuilder,
};
use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use meteor_ai::behavior::BehaviorMode;
use meteor_combat::hitbox::Obb;
use meteor_physics::collision::{GroundHit, MeshCollider, Point3};
use meteor_physics::jump::AirMotionMode;
use meteor_physics::wall_run;

const TELEPORT_LIFT: f32 = 2.0;

const GROUND_FALLBACK: f32 = -64.0;

const DUMMY_SPAWN_DISTANCE: f32 = 2.2;

#[derive(Resource, Default)]
struct DebugSpawnedNpcs(Vec<Entity>);

#[derive(Resource, Default)]
struct DebugFpsSample {
    ema_dt: f32,
}

#[derive(Resource)]
struct DebugStatsVisibleFlag(bool);

impl Default for DebugStatsVisibleFlag {
    fn default() -> Self {
        Self(false)
    }
}

#[derive(Resource, Debug, Clone)]
struct DebugPanelState {
    open: bool,
    tab: DebugPanelTab,
    character: u8,
    weapon: u8,
    npc_model: u8,
    npc_weapon: u8,
    npc_team: u8,
    npc_behavior: DebugNpcBehavior,
}

impl Default for DebugPanelState {
    fn default() -> Self {
        Self {
            open: false,
            tab: DebugPanelTab::Stats,
            character: 0,
            weapon: 6,
            npc_model: 0,
            npc_weapon: 6,
            npc_team: 0,
            npc_behavior: DebugNpcBehavior::Follow,
        }
    }
}

#[derive(Resource, Debug, Clone, Default)]
struct DebugPanelStatus(String);

#[derive(Resource, Debug, Clone, Default)]
struct DebugHitboxVisible(bool);

#[derive(Default, Reflect, GizmoConfigGroup)]
struct HitboxGizmos;

#[derive(Resource, Debug, Clone)]
pub(crate) struct DebugMotionLog {
    enabled: bool,
    elapsed: f32,
}

impl Default for DebugMotionLog {
    fn default() -> Self {
        Self {
            enabled: cfg!(all(target_os = "android", debug_assertions)),
            elapsed: 0.0,
        }
    }
}

impl DebugMotionLog {
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DebugAirDiscreteState {
    airborne: bool,
    motion_mode: AirMotionMode,
    fighter_state: meteor_core::FighterState,
    pose_id: u16,
    cling_active: bool,
    cling_pose: u16,
    kick_pose: u16,
    contact_seen: bool,
    cling_armed: bool,
    ground_contact: bool,
    ledge_fall: bool,
    pending_ledge_fall: bool,
    launch_pose_id: u16,
    launch_lifted: bool,
}

#[derive(Resource, Default)]
struct DebugAirTrace {
    previous: Option<DebugAirDiscreteState>,
    sequence: u64,
    sample_elapsed: f32,
}

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct DebugSpawnedNpc;
use std::collections::VecDeque;

const MAX_LOG_LINES: usize = 220;

const MAX_HISTORY: usize = 60;

const STATS_INTERVAL_SECS: f32 = 0.25;

const VISIBLE_LOG_LINES: usize = 14;

const ORIGIN_COMMANDS: &[&str] = &[
    "cheat",
    "kill",
    "pose",
    "play",
    "weapon",
    "switch",
    "dropw",
    "crouch",
    "pause",
    "speed",
    "moveto",
    "position",
    "adddummy",
    "adddummy2",
    "freecam",
    "dcam",
    "camera",
    "msense",
    "minverse",
    "loadch",
    "shows",
    "serverinfo",
    "record",
    "replay",
    "replays",
    "quit",
];

const WEB_COMMANDS: &[&str] = &[
    "help",
    "clear",
    "hp",
    "anger",
    "heal",
    "fps",
    "hitbox",
    "motion",
    "npc",
    "removenpc",
];

pub struct DebugPlugin;

impl Plugin for DebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugSpawnedNpcs>()
            .init_resource::<DebugFpsSample>()
            .init_resource::<DebugStatsVisibleFlag>()
            .init_resource::<DebugConsoleState>()
            .init_resource::<DebugConsoleBuffer>()
            .init_resource::<DebugConsoleLog>()
            .init_resource::<DebugSimControl>()
            .init_resource::<PendingDebugCommands>()
            .init_resource::<DebugPanelState>()
            .init_resource::<DebugPanelStatus>()
            .init_resource::<DebugHitboxVisible>()
            .insert_gizmo_config(
                HitboxGizmos,
                GizmoConfig {
                    depth_bias: -1.0,
                    ..default()
                },
            )
            .init_resource::<DebugMotionLog>()
            .init_resource::<DebugAirTrace>()
            .insert_resource(CommandNameList {
                origin: ORIGIN_COMMANDS.into(),
                web: WEB_COMMANDS.into(),
            })
            .add_systems(
                OnEnter(AppState::Playing),
                setup_debug_ui.run_if(crate::plugins::starts_fresh_playing_session),
            )
            .add_systems(
                OnExit(AppState::Playing),
                despawn_debug_ui.run_if(crate::plugins::ends_playing_session),
            )
            .add_systems(
                Update,
                (
                    debug_console_toggle,
                    debug_panel_buttons,
                    debug_console_typing,
                    debug_exec_queue,
                    update_console_text,
                    refresh_debug_panel,
                    debug_panel_button_visuals,
                    update_debug_stats,
                    adapt_debug_stats_layout,
                    adapt_debug_panel_layout,
                )
                    .chain()
                    .after(super::native_text_input::NativeTextInputSet)
                    .after(crate::plugins::UiTouchGestureSet)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                Update,
                (debug_motion_log, debug_air_state_log)
                    .after(crate::plugins::FighterFrameSet::PoseResolve)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                PostUpdate,
                draw_debug_hitboxes
                    .after(bevy::transform::TransformSystems::Propagate)
                    .run_if(in_state(AppState::Playing)),
            );
    }
}

#[derive(Resource, Default)]
pub struct DebugConsoleState {
    pub open: bool,

    pub panel_open: bool,
}

impl DebugConsoleState {
    pub fn captures_input(&self) -> bool {
        self.open || self.panel_open
    }
}

#[derive(Component, Debug, Clone, Default)]
pub struct CheatInvincible;

#[derive(Resource, Debug, Clone)]
pub struct DebugSimControl {
    pub paused: bool,

    pub time_scale: f32,
}

impl Default for DebugSimControl {
    fn default() -> Self {
        Self {
            paused: false,
            time_scale: 1.0,
        }
    }
}

#[derive(Resource, Default)]
struct DebugConsoleBuffer {
    input: String,
    history: Vec<String>,

    history_idx: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsoleTone {
    Info,
    Ok,
    Warn,
    Err,
    Title,
    Cmd,
    Muted,
}

impl ConsoleTone {
    fn prefix(self) -> &'static str {
        match self {
            Self::Info => "",
            Self::Ok => "[ok] ",
            Self::Warn => "[warn] ",
            Self::Err => "[err] ",
            Self::Title => "── ",
            Self::Cmd => "",
            Self::Muted => "",
        }
    }
}

#[derive(Resource)]
struct DebugConsoleLog {
    lines: VecDeque<(ConsoleTone, String)>,
    revision: u64,
}

impl DebugConsoleLog {
    fn print(&mut self, text: impl Into<String>, tone: ConsoleTone) {
        self.lines.push_back((tone, text.into()));
        while self.lines.len() > MAX_LOG_LINES {
            self.lines.pop_front();
        }
        self.revision += 1;
    }

    fn visible_text(&self) -> String {
        let start = self.lines.len().saturating_sub(VISIBLE_LOG_LINES);
        let mut out = String::new();
        for (tone, line) in self.lines.iter().skip(start) {
            let prefix = tone.prefix();
            if prefix.is_empty() {
                out.push_str(line);
            } else {
                out.push_str(prefix);
                out.push_str(line);
            }
            out.push('\n');
        }
        if out.is_empty() {
            " ".to_string()
        } else {
            out
        }
    }
}

#[derive(Resource, Default)]
struct PendingDebugCommands(Vec<String>);

impl Default for DebugConsoleLog {
    fn default() -> Self {
        let mut log = Self {
            lines: VecDeque::new(),
            revision: 0,
        };
        log.print(
            "流星再世 · 暗令台 — 输入 help 查看全部命令",
            ConsoleTone::Title,
        );
        log.print(
            "命令名与原版 Meteor.exe 隐藏控制台一致：cheat / speed / adddummy / moveto / freecam …",
            ConsoleTone::Muted,
        );
        log
    }
}

#[derive(Resource)]
struct CommandNameList {
    origin: Vec<&'static str>,
    web: Vec<&'static str>,
}

#[derive(Component)]
struct DebugUiRoot;

#[derive(Component)]
struct DebugStatsText;

#[derive(Component)]
struct DebugPanelRuntimeText;

#[derive(Component)]
struct DebugConsoleRoot;

#[derive(Component)]
struct DebugConsoleLogText;

#[derive(Component)]
struct DebugConsolePromptText;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DebugPanelTab {
    Stats,
    Loadout,
    Options,
    Npc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DebugNpcBehavior {
    Follow,
    Dummy,
    Wait,
    Patrol,
}

impl DebugNpcBehavior {
    fn label(self) -> &'static str {
        match self {
            Self::Follow => "追击 follow",
            Self::Dummy => "固定木桩 dummy",
            Self::Wait => "待命 wait",
            Self::Patrol => "巡逻 patrol",
        }
    }

    fn cycle(self, delta: i8) -> Self {
        let values = [Self::Follow, Self::Dummy, Self::Wait, Self::Patrol];
        let index = values.iter().position(|value| *value == self).unwrap_or(0) as i8;
        values[(index + delta).rem_euclid(values.len() as i8) as usize]
    }
}

#[derive(Component)]
struct DebugPanelRoot;

#[derive(Component)]
pub(crate) struct DebugPanelToggle;

#[derive(Component, Debug, Clone, Copy)]
struct DebugPanelFont(f32);

#[derive(Component, Debug, Clone, Copy)]
struct DebugPanelTabRoot(DebugPanelTab);

#[derive(Component, Debug, Clone, Copy)]
enum DebugPanelAction {
    TogglePanel,
    ClosePanel,
    SelectTab(DebugPanelTab),
    Character(i8),
    Weapon(i8),
    Command(&'static str),
    Speed(i8),
    OpenConsole,
    NpcModel(i8),
    NpcWeapon(i8),
    NpcTeam(i8),
    NpcBehavior(i8),
    AddNpc,
    RemoveNpc(bool),
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
enum DebugPanelTextKind {
    Character,
    Weapon,
    Cheat,
    Hitbox,
    Motion,
    Freecam,
    Shows,
    Speed,
    NpcModel,
    NpcWeapon,
    NpcTeam,
    NpcBehavior,
    NpcHint,
    Status,
}

#[derive(Component, Debug, Clone, Copy)]
struct DebugPanelText(DebugPanelTextKind);

fn adapt_debug_stats_layout(
    window: Query<&Window>,
    safe_area: Option<Res<SafeAreaInsets>>,
    ui_scale: Option<Res<crate::plugins::ui_scale_plugin::EffectiveUiScale>>,
    resolved: Option<Res<super::mobile_controls_plugin::ResolvedMobileLayout>>,
    mut previous: Local<Option<(Vec2, SafeAreaInsets, f32, Vec2, f32)>>,
    mut roots: Query<&mut Node, With<DebugStatsRootNode>>,

    mut fonts: Query<&mut TextFont, (With<DebugStatsText>, Without<DebugPanelRuntimeText>)>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let safe = safe_area.as_deref().copied().unwrap_or_default();
    let unit = ui_scale.as_ref().map_or(1.0, |scale| scale.0.max(0.05));
    let (map_center, map_diameter) =
        super::ui_plugin::radar_screen_geometry(window, safe, resolved.as_deref());
    let key = (
        Vec2::new(window.width(), window.height()),
        safe,
        unit,
        map_center,
        map_diameter,
    );
    let changed = previous.as_ref() != Some(&key);
    *previous = Some(key);
    let layout = crate::plugins::launcher_ui::responsive_ui_layout_with_safe_area(
        window.width(),
        window.height(),
        cfg!(target_os = "android"),
        safe_area.as_deref().copied().unwrap_or_default(),
    );
    let hud = super::combat_hud_layout::CombatHudLayout::new(
        window.width(),
        window.height(),
        safe,
        layout.mobile,
    );
    for mut node in &mut roots {
        if !changed && !node.is_added() {
            continue;
        }
        let top = (map_center.y + map_diameter * 0.5 + 12.0).max(hud.stats_origin.y + 132.0);
        node.left = Val::Px((safe.left + 12.0) / unit);
        node.top = Val::Px(
            if layout.mobile {
                top.max(window.height() * 0.48)
            } else {
                top
            } / unit,
        );
    }
    for mut font in &mut fonts {
        let size = if layout.mobile { 10.0 } else { 13.0 }.into();
        if font.font_size != size {
            font.font_size = size;
        }
    }
}

type DebugPanelRootLayoutQuery<'w, 's> = Query<'w, 's, &'static mut Node, With<DebugPanelRoot>>;
type DebugPanelToggleLayoutQuery<'w, 's> = Query<'w, 's, &'static mut Node, With<DebugPanelToggle>>;
type DebugConsoleLayoutQuery<'w, 's> = Query<'w, 's, &'static mut Node, With<DebugConsoleRoot>>;

fn adapt_debug_panel_layout(
    window: Query<&Window>,
    ui_scale: Option<Res<crate::plugins::ui_scale_plugin::EffectiveUiScale>>,
    safe_area: Option<Res<SafeAreaInsets>>,
    mut previous: Local<Option<(Vec2, SafeAreaInsets, f32)>>,
    mut nodes: ParamSet<(
        DebugPanelRootLayoutQuery<'_, '_>,
        DebugPanelToggleLayoutQuery<'_, '_>,
        DebugConsoleLayoutQuery<'_, '_>,
    )>,
    mut fonts: Query<(&DebugPanelFont, &mut TextFont)>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let unit = ui_scale.as_ref().map_or(1.0, |scale| scale.0.max(0.05));
    let key = (
        Vec2::new(window.width(), window.height()),
        safe_area.as_deref().copied().unwrap_or_default(),
        unit,
    );
    let changed = previous.as_ref() != Some(&key);
    *previous = Some(key);
    let layout = crate::plugins::launcher_ui::responsive_ui_layout_with_safe_area(
        window.width(),
        window.height(),
        cfg!(target_os = "android"),
        safe_area.as_deref().copied().unwrap_or_default(),
    );
    let (base_gutter, base_top, base_width, _) =
        debug_panel_geometry(window.width(), window.height(), layout.mobile);
    let gutter = base_gutter.max(layout.right_gutter);
    let top = base_top.max(layout.top_gutter + if layout.mobile { 46.0 } else { 0.0 });
    let available_width = (window.width() - layout.left_gutter - gutter).max(180.0);
    let width = base_width.min(available_width);
    let max_height = (window.height() - top - layout.bottom_gutter).max(120.0);
    for mut node in &mut nodes.p0() {
        if !changed && !node.is_added() {
            continue;
        }
        node.top = Val::Px(top);
        node.right = Val::Px(gutter);
        node.width = Val::Px(width);
        node.max_height = Val::Px(max_height);
        node.row_gap = Val::Px(if layout.mobile { 5.0 } else { 8.0 });
        node.padding = UiRect::all(Val::Px(if layout.mobile { 7.0 } else { 10.0 }));
    }
    let hud = super::combat_hud_layout::CombatHudLayout::new(
        window.width(),
        window.height(),
        safe_area.as_deref().copied().unwrap_or_default(),
        layout.mobile,
    );
    for mut node in &mut nodes.p1() {
        if !changed && !node.is_added() {
            continue;
        }
        node.top = Val::Px((hud.debug_center.y - hud.button_size.y * 0.5) / unit);
        node.right =
            Val::Px((window.width() - hud.debug_center.x - hud.button_size.x * 0.5) / unit);
        node.width = Val::Px(hud.button_size.x / unit);
        node.height = Val::Px(hud.button_size.y / unit);
        node.min_height = Val::Px(hud.button_size.y / unit);
    }
    for mut node in &mut nodes.p2() {
        if !changed && !node.is_added() {
            continue;
        }
        node.padding = UiRect::new(
            Val::Px(layout.left_gutter),
            Val::Px(layout.right_gutter),
            Val::Px(10.0_f32.max(layout.top_gutter)),
            Val::Px(10.0_f32.max(layout.bottom_gutter)),
        );
    }
    for (base, mut font) in &mut fonts {
        let size = debug_panel_font_size(base.0, layout.mobile).into();
        if font.font_size != size {
            font.font_size = size;
        }
    }
}

fn debug_panel_geometry(width: f32, height: f32, mobile: bool) -> (f32, f32, f32, f32) {
    let gutter = if mobile { 8.0 } else { 12.0 };
    let top = if mobile { 54.0 } else { 64.0 };
    (
        gutter,
        top,
        (width - gutter * 2.0).clamp(260.0, 390.0),
        (height - top - gutter).max(180.0),
    )
}

fn debug_panel_font_size(base: f32, mobile: bool) -> f32 {
    if mobile {
        (base * 0.82).clamp(9.0, 14.0)
    } else {
        base
    }
}

fn setup_debug_ui(mut commands: Commands, ui_font: Res<crate::plugins::launcher_ui::UiFont>) {
    let font = ui_font.0.clone();

    let stats_root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(96.0),
                left: Val::Px(12.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.03, 0.08, 0.65)),
            DebugUiRoot,
            DebugStatsRootNode,
        ))
        .id();
    let stats_text = spawn_text(
        &mut commands,
        "",
        13.0,
        Color::srgb(0.85, 0.88, 0.92),
        font.clone(),
    );
    commands.entity(stats_text).insert(DebugStatsText);
    commands.entity(stats_root).add_child(stats_text);

    let console_root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(0.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(10.0)),
                row_gap: Val::Px(4.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.02, 0.05, 0.92)),
            DebugUiRoot,
            DebugConsoleRoot,
        ))
        .id();
    let header = spawn_text(
        &mut commands,
        "暗令 · 隐藏命令    Tab 补全 · ↑↓ 历史 · Esc 关闭 · ~ 开关",
        14.0,
        Color::srgb(0.95, 0.8, 0.35),
        font.clone(),
    );
    commands.entity(console_root).add_child(header);
    let log = spawn_text(
        &mut commands,
        " ",
        14.0,
        Color::srgb(0.85, 0.88, 0.92),
        font.clone(),
    );
    commands.entity(log).insert(DebugConsoleLogText);
    commands.entity(console_root).add_child(log);
    let prompt = spawn_text(
        &mut commands,
        "» ",
        15.0,
        Color::srgb(0.55, 0.9, 0.55),
        font.clone(),
    );
    commands.entity(prompt).insert((
        DebugConsolePromptText,
        Button,
        Node {
            min_height: Val::Px(44.0),
            ..default()
        },
        super::native_text_input::NativeTextEdit {
            field: super::native_text_input::TextField::Console,
            value: String::new(),
        },
    ));
    commands.entity(console_root).add_child(prompt);

    let toggle = commands
        .spawn((
            Button,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(12.0),

                right: Val::Px(106.0),
                width: Val::Px(64.0),
                height: Val::Px(28.0),
                min_height: Val::Px(28.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(10.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.05, 0.08, 0.88)),
            BorderColor::all(Color::srgba(0.78, 0.82, 0.78, 0.38)),
            Interaction::default(),
            FocusPolicy::Block,
            ZIndex(230),
            DebugUiRoot,
            DebugPanelToggle,
            LauncherStyledButton,
            DebugPanelAction::TogglePanel,
        ))
        .id();
    let toggle_text = spawn_text(&mut commands, "调试", 13.0, Color::WHITE, font.clone());
    commands.entity(toggle).add_child(toggle_text);

    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(64.0),
                right: Val::Px(12.0),
                width: Val::Px(390.0),
                max_width: Val::Percent(94.0),
                max_height: Val::Percent(82.0),
                display: Display::None,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(10.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(14.0)),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            BackgroundColor(Color::srgba(0.025, 0.03, 0.045, 0.96)),
            BorderColor::all(Color::srgba(0.88, 0.3, 0.22, 0.75)),
            ScrollPosition::default(),
            Interaction::default(),
            FocusPolicy::Block,
            ZIndex(220),
            DebugUiRoot,
            DebugPanelRoot,
            LauncherScrollArea,
            crate::plugins::ui_components::UiInputBlocker,
        ))
        .id();
    commands.entity(panel).with_children(|panel| {
        panel
            .spawn(Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|header| {
                spawn_debug_label(header, &font, "调试面板", 18.0, Color::WHITE, None);
                spawn_debug_button_with_node(
                    header,
                    &font,
                    "关闭",
                    DebugPanelAction::ClosePanel,
                    None,
                    debug_compact_button_node(72.0),
                );
            });
        panel
            .spawn(Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                column_gap: Val::Px(6.0),
                row_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|tabs| {
                spawn_debug_button_with_node(
                    tabs,
                    &font,
                    "运行信息",
                    DebugPanelAction::SelectTab(DebugPanelTab::Stats),
                    None,
                    debug_tab_button_node(),
                );
                spawn_debug_button_with_node(
                    tabs,
                    &font,
                    "换武器/模型",
                    DebugPanelAction::SelectTab(DebugPanelTab::Loadout),
                    None,
                    debug_tab_button_node(),
                );
                spawn_debug_button_with_node(
                    tabs,
                    &font,
                    "调试选项",
                    DebugPanelAction::SelectTab(DebugPanelTab::Options),
                    None,
                    debug_tab_button_node(),
                );
                spawn_debug_button_with_node(
                    tabs,
                    &font,
                    "添加 NPC",
                    DebugPanelAction::SelectTab(DebugPanelTab::Npc),
                    None,
                    debug_tab_button_node(),
                );
            });

        panel
            .spawn((
                debug_panel_page_node(Display::Flex),
                DebugPanelTabRoot(DebugPanelTab::Stats),
            ))
            .with_children(|page| {
                spawn_debug_label(
                    page,
                    &font,
                    "实时数据 · 每 250ms 更新",
                    11.0,
                    Color::srgb(0.66, 0.72, 0.74),
                    None,
                );
                page.spawn((
                    Text::new("正在读取角色状态…"),
                    TextFont {
                        font: font.clone().into(),
                        font_size: 11.0.into(),
                        ..default()
                    },
                    TextColor(Color::srgb(0.88, 0.90, 0.92)),
                    TextLayout::new(Justify::Left, LineBreak::NoWrap),
                    Node {
                        width: Val::Percent(100.0),
                        min_width: Val::Px(0.0),
                        ..default()
                    },
                    DebugPanelFont(11.0),
                    DebugStatsText,
                    DebugPanelRuntimeText,
                ));
            });

        panel
            .spawn((
                debug_panel_page_node(Display::None),
                DebugPanelTabRoot(DebugPanelTab::Loadout),
            ))
            .with_children(|page| {
                spawn_debug_label(
                    page,
                    &font,
                    "与 Web 选择器一致，切换后立即应用",
                    12.0,
                    Color::srgb(0.66, 0.72, 0.74),
                    None,
                );
                spawn_debug_value_row(
                    page,
                    &font,
                    "模型",
                    DebugPanelAction::Character(-1),
                    DebugPanelAction::Character(1),
                    DebugPanelTextKind::Character,
                );
                spawn_debug_value_row(
                    page,
                    &font,
                    "兵器",
                    DebugPanelAction::Weapon(-1),
                    DebugPanelAction::Weapon(1),
                    DebugPanelTextKind::Weapon,
                );
            });

        panel
            .spawn((
                debug_panel_page_node(Display::None),
                DebugPanelTabRoot(DebugPanelTab::Options),
            ))
            .with_children(|page| {
                spawn_debug_button(
                    page,
                    &font,
                    "无敌模式",
                    DebugPanelAction::Command("cheat"),
                    Some(DebugPanelTextKind::Cheat),
                );
                spawn_debug_button(
                    page,
                    &font,
                    "碰撞盒可视化",
                    DebugPanelAction::Command("hitbox"),
                    Some(DebugPanelTextKind::Hitbox),
                );
                spawn_debug_button(
                    page,
                    &font,
                    "运动日志",
                    DebugPanelAction::Command("motion"),
                    Some(DebugPanelTextKind::Motion),
                );
                spawn_debug_button(
                    page,
                    &font,
                    "自由镜头（驻留）",
                    DebugPanelAction::Command("freecam"),
                    Some(DebugPanelTextKind::Freecam),
                );
                spawn_debug_button(
                    page,
                    &font,
                    "显示调试运行信息",
                    DebugPanelAction::Command("shows"),
                    Some(DebugPanelTextKind::Shows),
                );
                spawn_debug_value_row(
                    page,
                    &font,
                    "时间缩放",
                    DebugPanelAction::Speed(-1),
                    DebugPanelAction::Speed(1),
                    DebugPanelTextKind::Speed,
                );
                page.spawn(Node {
                    width: Val::Percent(100.0),
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: Val::Px(6.0),
                    row_gap: Val::Px(6.0),
                    ..default()
                })
                .with_children(|row| {
                    spawn_debug_button_with_node(
                        row,
                        &font,
                        "满血满怒",
                        DebugPanelAction::Command("heal"),
                        None,
                        debug_compact_button_node(96.0),
                    );
                    spawn_debug_button_with_node(
                        row,
                        &font,
                        "全灭 NPC",
                        DebugPanelAction::Command("kill all"),
                        None,
                        debug_compact_button_node(96.0),
                    );
                    spawn_debug_button_with_node(
                        row,
                        &font,
                        "回出生点",
                        DebugPanelAction::Command("moveto spawn"),
                        None,
                        debug_compact_button_node(96.0),
                    );
                });
                spawn_debug_button(
                    page,
                    &font,
                    "暗令台 · 命令行 (~)",
                    DebugPanelAction::OpenConsole,
                    None,
                );
            });

        panel
            .spawn((
                debug_panel_page_node(Display::None),
                DebugPanelTabRoot(DebugPanelTab::Npc),
            ))
            .with_children(|page| {
                spawn_debug_value_row(
                    page,
                    &font,
                    "模型",
                    DebugPanelAction::NpcModel(-1),
                    DebugPanelAction::NpcModel(1),
                    DebugPanelTextKind::NpcModel,
                );
                spawn_debug_value_row(
                    page,
                    &font,
                    "兵器",
                    DebugPanelAction::NpcWeapon(-1),
                    DebugPanelAction::NpcWeapon(1),
                    DebugPanelTextKind::NpcWeapon,
                );
                spawn_debug_value_row(
                    page,
                    &font,
                    "阵营",
                    DebugPanelAction::NpcTeam(-1),
                    DebugPanelAction::NpcTeam(1),
                    DebugPanelTextKind::NpcTeam,
                );
                spawn_debug_value_row(
                    page,
                    &font,
                    "行为",
                    DebugPanelAction::NpcBehavior(-1),
                    DebugPanelAction::NpcBehavior(1),
                    DebugPanelTextKind::NpcBehavior,
                );
                page.spawn(Node {
                    width: Val::Percent(100.0),
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: Val::Px(6.0),
                    row_gap: Val::Px(6.0),
                    ..default()
                })
                .with_children(|row| {
                    spawn_debug_button_with_node(
                        row,
                        &font,
                        "添加 NPC",
                        DebugPanelAction::AddNpc,
                        None,
                        debug_compact_button_node(96.0),
                    );
                    spawn_debug_button_with_node(
                        row,
                        &font,
                        "移除",
                        DebugPanelAction::RemoveNpc(false),
                        None,
                        debug_compact_button_node(82.0),
                    );
                    spawn_debug_button_with_node(
                        row,
                        &font,
                        "清空",
                        DebugPanelAction::RemoveNpc(true),
                        None,
                        debug_compact_button_node(82.0),
                    );
                });
                spawn_debug_label(
                    page,
                    &font,
                    "",
                    12.0,
                    Color::srgb(0.72, 0.78, 0.78),
                    Some(DebugPanelTextKind::NpcHint),
                );
            });
        spawn_debug_label(
            panel,
            &font,
            "",
            12.0,
            Color::srgb(0.95, 0.76, 0.4),
            Some(DebugPanelTextKind::Status),
        );
    });
}

fn despawn_debug_ui(
    mut commands: Commands,
    roots: Query<Entity, With<DebugUiRoot>>,
    mut spawned: ResMut<DebugSpawnedNpcs>,
    mut status: ResMut<DebugPanelStatus>,
) {
    for entity in &roots {
        commands.entity(entity).despawn();
    }
    commands.insert_resource(DebugConsoleState {
        open: false,
        panel_open: false,
    });
    commands.insert_resource(DebugPanelState::default());
    spawned.0.clear();
    status.0.clear();
}

fn spawn_text(
    commands: &mut Commands,
    value: &str,
    size: f32,
    color: Color,
    font: Handle<Font>,
) -> Entity {
    commands
        .spawn((
            Text::new(value),
            TextFont {
                font_size: size.into(),
                font: font.into(),
                ..default()
            },
            TextColor(color),
        ))
        .id()
}

fn debug_panel_page_node(display: Display) -> Node {
    Node {
        width: Val::Percent(100.0),
        display,
        flex_direction: FlexDirection::Column,
        row_gap: Val::Px(5.0),
        padding: UiRect::all(Val::Px(6.0)),
        border: UiRect::all(Val::Px(1.0)),
        border_radius: BorderRadius::all(Val::Px(10.0)),
        ..default()
    }
}

fn debug_tab_button_node() -> Node {
    Node {
        min_width: Val::Px(78.0),
        min_height: Val::Px(40.0),
        flex_grow: 1.0,
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        padding: UiRect::horizontal(Val::Px(7.0)),
        border: UiRect::all(Val::Px(1.0)),
        border_radius: BorderRadius::all(Val::Px(7.0)),
        ..default()
    }
}

fn debug_compact_button_node(min_width: f32) -> Node {
    Node {
        min_width: Val::Px(min_width),
        min_height: Val::Px(40.0),
        flex_grow: 1.0,
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        padding: UiRect::horizontal(Val::Px(7.0)),
        border: UiRect::all(Val::Px(1.0)),
        border_radius: BorderRadius::all(Val::Px(7.0)),
        ..default()
    }
}

fn spawn_debug_label(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    value: &str,
    size: f32,
    color: Color,
    kind: Option<DebugPanelTextKind>,
) -> Entity {
    spawn_debug_label_with_node(
        parent,
        font,
        value,
        size,
        color,
        kind,
        Node {
            min_width: Val::Px(0.0),
            ..default()
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn spawn_debug_label_with_node(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    value: &str,
    size: f32,
    color: Color,
    kind: Option<DebugPanelTextKind>,
    node: Node,
) -> Entity {
    let mut entity = parent.spawn((
        Text::new(value),
        TextFont {
            font: font.clone().into(),
            font_size: size.into(),
            ..default()
        },
        TextColor(color),
        TextLayout::new(Justify::Center, LineBreak::WordOrCharacter),
        DebugPanelFont(size),
        node,
    ));
    if let Some(kind) = kind {
        entity.insert(DebugPanelText(kind));
    }
    entity.id()
}

fn spawn_debug_button(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    action: DebugPanelAction,
    text_kind: Option<DebugPanelTextKind>,
) -> Entity {
    spawn_debug_button_with_node(
        parent,
        font,
        label,
        action,
        text_kind,
        Node {
            width: Val::Percent(100.0),
            min_height: Val::Px(40.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            padding: UiRect::horizontal(Val::Px(7.0)),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(7.0)),
            ..default()
        },
    )
}

fn spawn_debug_button_with_node(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    action: DebugPanelAction,
    text_kind: Option<DebugPanelTextKind>,
    node: Node,
) -> Entity {
    let mut button = parent.spawn((
        Button,
        node,
        BackgroundColor(Color::srgb(0.09, 0.11, 0.16)),
        BorderColor::all(Color::srgba(0.72, 0.76, 0.74, 0.3)),
        Interaction::default(),
        FocusPolicy::Block,
        LauncherStyledButton,
        action,
    ));
    button.with_children(|button| {
        spawn_debug_label_with_node(
            button,
            font,
            label,
            12.0,
            Color::WHITE,
            text_kind,
            Node {
                width: Val::Percent(100.0),
                min_width: Val::Px(0.0),
                ..default()
            },
        );
    });
    button.id()
}

fn spawn_debug_value_row(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    title: &str,
    previous: DebugPanelAction,
    next: DebugPanelAction,
    value_kind: DebugPanelTextKind,
) {
    parent
        .spawn(Node {
            width: Val::Percent(100.0),
            min_height: Val::Px(42.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(6.0),
            ..default()
        })
        .with_children(|row| {
            spawn_debug_label_with_node(
                row,
                font,
                title,
                12.0,
                Color::srgb(0.72, 0.78, 0.8),
                None,
                Node {
                    width: Val::Px(52.0),
                    flex_shrink: 0.0,
                    ..default()
                },
            );
            let step_node = || Node {
                width: Val::Px(42.0),
                min_height: Val::Px(36.0),
                flex_shrink: 0.0,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(7.0)),
                ..default()
            };
            spawn_debug_button_with_node(row, font, "‹", previous, None, step_node());
            spawn_debug_label_with_node(
                row,
                font,
                "-",
                12.0,
                Color::WHITE,
                Some(value_kind),
                Node {
                    min_width: Val::Px(0.0),
                    flex_grow: 1.0,
                    ..default()
                },
            );
            spawn_debug_button_with_node(row, font, "›", next, None, step_node());
        });
}

fn debug_console_toggle(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keymap: Res<Keymap>,
    mut state: ResMut<DebugConsoleState>,
    mut panel: ResMut<DebugPanelState>,
) {
    if keymap.debug_panel.just_pressed(&keys, &mouse) {
        panel.open = !panel.open;
        state.panel_open = panel.open;

        if panel.open {
            state.open = false;
        }
    }
    if keys.just_pressed(KeyCode::Escape) && panel.open {
        panel.open = false;
        state.panel_open = false;
    }
}

fn debug_console_typing(
    keys: Res<ButtonInput<KeyCode>>,
    mut console: ResMut<DebugConsoleState>,
    mut buffer: ResMut<DebugConsoleBuffer>,
    mut log: ResMut<DebugConsoleLog>,
    mut queue: ResMut<PendingDebugCommands>,
    names: Res<CommandNameList>,
    mut submitted: MessageReader<super::native_text_input::TextSubmitted>,
) {
    if !console.open {
        submitted.clear();
        return;
    }
    for event in submitted.read() {
        if event.field != super::native_text_input::TextField::Console || event.value.is_empty() {
            continue;
        }
        let line = event.value.clone();
        buffer.input.clear();
        buffer.history_idx = None;
        if buffer.history.last().is_none_or(|last| last != &line) {
            buffer.history.push(line.clone());
        }
        while buffer.history.len() > MAX_HISTORY {
            buffer.history.remove(0);
        }
        log.print(format!("» {line}"), ConsoleTone::Cmd);
        queue.0.push(line);
    }
    for key in keys.get_just_pressed() {
        match key {
            KeyCode::Escape => {
                console.open = false;
                buffer.input.clear();
                buffer.history_idx = None;
                return;
            }
            KeyCode::Backquote | KeyCode::Tab | KeyCode::Enter => {}
            KeyCode::ArrowUp => {
                if buffer.history.is_empty() {
                    continue;
                }
                let next = match buffer.history_idx {
                    None => buffer.history.len() - 1,
                    Some(i) => i.saturating_sub(1),
                };
                buffer.history_idx = Some(next);
                buffer.input = buffer.history[next].clone();
                continue;
            }
            KeyCode::ArrowDown => {
                let Some(idx) = buffer.history_idx else {
                    continue;
                };
                if idx + 1 >= buffer.history.len() {
                    buffer.history_idx = None;
                    buffer.input.clear();
                } else {
                    buffer.history_idx = Some(idx + 1);
                    buffer.input = buffer.history[idx + 1].clone();
                }
                continue;
            }
            _ => {}
        }
        match key {
            KeyCode::Enter => {
                let line = buffer.input.trim().to_string();
                buffer.input.clear();
                buffer.history_idx = None;
                if line.is_empty() {
                    continue;
                }
                if buffer.history.last().is_none_or(|last| last != &line) {
                    buffer.history.push(line.clone());
                }
                while buffer.history.len() > MAX_HISTORY {
                    buffer.history.remove(0);
                }
                log.print(format!("» {line}"), ConsoleTone::Cmd);
                queue.0.push(line);
            }
            KeyCode::Tab => {
                let head = buffer.input.trim_start().to_lowercase();
                if head.is_empty() || head.contains(' ') {
                    continue;
                }
                let matches: Vec<&str> = names
                    .origin
                    .iter()
                    .chain(names.web.iter())
                    .copied()
                    .filter(|name| name.starts_with(head.as_str()))
                    .collect();
                if matches.len() == 1 {
                    buffer.input = format!("{} ", matches[0]);
                } else if matches.len() > 1 {
                    log.print(matches.join("  "), ConsoleTone::Muted);
                }
            }
            _ => {}
        }
    }
}

const DEBUG_NPC_MODELS: &[u8] = &[0, 1, 2, 8, 19];
const DEBUG_NPC_WEAPONS: &[u8] = &[6, 8, 1, 2, 3, 4, 5, 7, 9, 0];
const DEBUG_SPEED_STEPS: &[f32] = &[0.1, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0];

fn cycle_u8(value: u8, values: &[u8], delta: i8) -> u8 {
    let index = values
        .iter()
        .position(|candidate| *candidate == value)
        .unwrap_or(0) as i8;
    values[(index + delta).rem_euclid(values.len() as i8) as usize]
}

#[allow(clippy::too_many_arguments)]
fn debug_panel_buttons(
    pressed: Query<(&Interaction, &DebugPanelAction), Changed<Interaction>>,
    mut panel: ResMut<DebugPanelState>,
    mut status: ResMut<DebugPanelStatus>,
    mut capture: ResMut<DebugConsoleState>,
    sim: Res<DebugSimControl>,
    net: Option<Res<crate::plugins::network_plugin::NetState>>,
    mut queue: ResMut<PendingDebugCommands>,
    collider: Option<Res<StageCollider>>,
    mut local: LocalFighterMutQuery<'_, '_>,
    npcs: NpcInfoQuery<'_, '_>,
    mut spawned: ResMut<DebugSpawnedNpcs>,
    mut panel_scrolls: Query<&mut ScrollPosition, With<DebugPanelRoot>>,
    mut commands: Commands,
) {
    for (interaction, action) in &pressed {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let reset_scroll = matches!(
            *action,
            DebugPanelAction::TogglePanel | DebugPanelAction::SelectTab(_)
        );
        match *action {
            DebugPanelAction::TogglePanel => {
                panel.open = !panel.open;
                capture.panel_open = panel.open;

                if panel.open {
                    capture.open = false;
                }
            }
            DebugPanelAction::ClosePanel => {
                panel.open = false;
                capture.panel_open = false;
            }
            DebugPanelAction::SelectTab(tab) => panel.tab = tab,
            DebugPanelAction::Character(delta) => {
                panel.character = (panel.character as i16 + i16::from(delta)).rem_euclid(20) as u8;
                queue.0.push(format!("loadch {}", panel.character));
                status.0 = format!("正在切换到 {}", char_label(panel.character));
            }
            DebugPanelAction::Weapon(delta) => {
                panel.weapon = (panel.weapon as i16 + i16::from(delta)).rem_euclid(13) as u8;
                queue.0.push(format!("weapon {}", panel.weapon));
                status.0 = format!("正在装备 {}", weapon_label(panel.weapon));
            }
            DebugPanelAction::Command(command) => {
                queue.0.push(command.to_string());
                status.0 = format!("命令：{command}");
            }
            DebugPanelAction::Speed(delta) => {
                let current = DEBUG_SPEED_STEPS
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        (*a - sim.time_scale)
                            .abs()
                            .total_cmp(&(*b - sim.time_scale).abs())
                    })
                    .map(|(index, _)| index)
                    .unwrap_or(4) as i8;
                let next = (current + delta).clamp(0, DEBUG_SPEED_STEPS.len() as i8 - 1) as usize;
                queue.0.push(format!("speed {}", DEBUG_SPEED_STEPS[next]));
            }
            DebugPanelAction::OpenConsole => {
                capture.open = true;
                panel.open = false;
                capture.panel_open = false;
            }
            DebugPanelAction::NpcModel(delta) => {
                panel.npc_model = cycle_u8(panel.npc_model, DEBUG_NPC_MODELS, delta);
            }
            DebugPanelAction::NpcWeapon(delta) => {
                panel.npc_weapon = cycle_u8(panel.npc_weapon, DEBUG_NPC_WEAPONS, delta);
            }
            DebugPanelAction::NpcTeam(delta) => {
                panel.npc_team = (panel.npc_team as i8 + delta).rem_euclid(3) as u8;
            }
            DebugPanelAction::NpcBehavior(delta) => {
                panel.npc_behavior = panel.npc_behavior.cycle(delta);
            }
            DebugPanelAction::AddNpc => {
                spawn_debug_npc(
                    net.as_deref(),
                    panel.npc_model,
                    panel.npc_weapon,
                    panel.npc_team,
                    panel.npc_behavior,
                    &mut local,
                    &npcs,
                    collider.as_deref(),
                    &mut spawned,
                    &mut commands,
                    &mut status.0,
                );
            }
            DebugPanelAction::RemoveNpc(all) => {
                let removed = remove_debug_npcs(all, &mut spawned, &mut commands);
                status.0 = if removed == 0 {
                    if all {
                        "没有调试 NPC"
                    } else {
                        "没有可移除的调试 NPC"
                    }
                    .to_string()
                } else {
                    format!(
                        "已{} {removed} 个调试 NPC",
                        if all { "清空" } else { "移除" }
                    )
                };
            }
        }
        if reset_scroll && panel.open {
            for mut scroll in &mut panel_scrolls {
                scroll.0 = Vec2::ZERO;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn refresh_debug_panel(
    mut panel: ResMut<DebugPanelState>,
    capture: Res<DebugConsoleState>,
    status: Res<DebugPanelStatus>,
    hitbox: Res<DebugHitboxVisible>,
    motion_log: Res<DebugMotionLog>,
    stats: Res<DebugStatsVisibleFlag>,
    sim: Res<DebugSimControl>,
    camera: Res<CameraMode>,
    spawned: Res<DebugSpawnedNpcs>,
    npcs: Query<(), With<NpcFighter>>,
    local: Query<(&Fighter, Option<&CheatInvincible>), With<LocalFighter>>,
    mut roots: Query<&mut Node, With<DebugPanelRoot>>,
    mut pages: Query<(&DebugPanelTabRoot, &mut Node), Without<DebugPanelRoot>>,
    mut texts: Query<(&DebugPanelText, &mut Text)>,
) {
    if let Ok((fighter, _)) = local.single() {
        if panel.character != fighter.character_id || panel.weapon != fighter.weapon {
            panel.character = fighter.character_id;
            panel.weapon = fighter.weapon;
        }
    }
    for mut node in &mut roots {
        let display = if panel.open {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }

    if !panel.open {
        return;
    }
    for (page, mut node) in &mut pages {
        let display = if page.0 == panel.tab {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    let cheat = local
        .single()
        .ok()
        .is_some_and(|(_, cheat)| cheat.is_some());
    for (marker, mut text) in &mut texts {
        let value = match marker.0 {
            DebugPanelTextKind::Character => char_label(panel.character),
            DebugPanelTextKind::Weapon => weapon_label(panel.weapon).to_string(),
            DebugPanelTextKind::Cheat => format!("无敌模式：{}", on_off(cheat)),
            DebugPanelTextKind::Hitbox => {
                format!("碰撞盒可视化：{}", on_off(hitbox.0))
            }
            DebugPanelTextKind::Motion => {
                format!("运动日志：{}", on_off(motion_log.enabled))
            }
            DebugPanelTextKind::Freecam => {
                format!("自由镜头（驻留）：{}", on_off(camera.freecam_hold))
            }
            DebugPanelTextKind::Shows => {
                format!("显示调试运行信息：{}", on_off(stats.0))
            }
            DebugPanelTextKind::Speed => format!("{}×", fmt_f(sim.time_scale)),
            DebugPanelTextKind::NpcModel => char_label(panel.npc_model),
            DebugPanelTextKind::NpcWeapon => weapon_label(panel.npc_weapon).to_string(),
            DebugPanelTextKind::NpcTeam => match panel.npc_team {
                1 => "友方 (1)".into(),
                2 => "中立 (2)".into(),
                _ => "敌方 (0)".into(),
            },
            DebugPanelTextKind::NpcBehavior => panel.npc_behavior.label().into(),
            DebugPanelTextKind::NpcHint => format!(
                "调试 NPC {} · 全部 {} · 生成在玩家前方约 2.2m",
                spawned.0.len(),
                npcs.iter().count()
            ),
            DebugPanelTextKind::Status => {
                if status.0.is_empty() {
                    if capture.open {
                        "暗令台已打开".into()
                    } else {
                        " ".into()
                    }
                } else {
                    status.0.clone()
                }
            }
        };
        if text.0 != value {
            text.0 = value;
        }
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "开"
    } else {
        "关"
    }
}

fn debug_panel_button_visuals(
    panel: Res<DebugPanelState>,
    mut buttons: Query<
        (
            &Interaction,
            &DebugPanelAction,
            &mut BackgroundColor,
            &mut BorderColor,
        ),
        With<DebugPanelAction>,
    >,
) {
    for (interaction, action, mut color, mut border) in &mut buttons {
        let active_tab = matches!(action, DebugPanelAction::SelectTab(tab) if *tab == panel.tab);
        color.0 = match interaction {
            Interaction::Pressed => Color::srgb(0.52, 0.18, 0.12),
            Interaction::Hovered => Color::srgb(0.22, 0.28, 0.34),
            Interaction::None if active_tab => Color::srgba(0.42, 0.29, 0.12, 0.88),
            Interaction::None => Color::srgb(0.09, 0.11, 0.16),
        };
        border.set_all(if active_tab {
            Color::srgba(0.90, 0.72, 0.37, 0.78)
        } else {
            Color::srgba(0.72, 0.76, 0.74, 0.3)
        });
    }
}

type DebugConsoleNodeQuery<'w, 's> = Query<'w, 's, &'static mut Node, With<DebugConsoleRoot>>;
type DebugStatsNodeQuery<'w, 's> = Query<'w, 's, &'static mut Node, With<DebugStatsRootNode>>;
type DebugConsoleLogTextQuery<'w, 's> = Query<'w, 's, &'static mut Text, With<DebugConsoleLogText>>;
type DebugConsolePromptTextQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Text,
        &'static mut super::native_text_input::NativeTextEdit,
    ),
    With<DebugConsolePromptText>,
>;
type DebugStatsTextQuery<'w, 's> = Query<
    'w,
    's,
    (&'static mut Text, Option<&'static DebugPanelRuntimeText>),
    (With<DebugStatsText>, Without<DebugConsoleLogText>),
>;

#[allow(clippy::too_many_arguments)]
fn update_console_text(
    console: Res<DebugConsoleState>,
    log: Res<DebugConsoleLog>,
    buffer: Res<DebugConsoleBuffer>,
    stats_visible: Res<DebugStatsVisibleFlag>,
    mut nodes: ParamSet<(DebugConsoleNodeQuery<'_, '_>, DebugStatsNodeQuery<'_, '_>)>,
    mut texts: ParamSet<(
        DebugConsoleLogTextQuery<'_, '_>,
        DebugConsolePromptTextQuery<'_, '_>,
    )>,
) {
    for mut node in &mut nodes.p0() {
        let display = if console.open {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    for mut node in &mut nodes.p1() {
        let display = if stats_visible.0 {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    if !console.open {
        return;
    }
    for mut text in &mut texts.p0() {
        let value = log.visible_text();
        if text.0 != value {
            text.0 = value;
        }
    }
    for (mut text, mut field) in &mut texts.p1() {
        let value = if buffer.input.is_empty() {
            "» 点击输入命令".into()
        } else {
            format!("» {}", buffer.input)
        };
        if text.0 != value {
            text.0 = value;
        }
        if field.value != buffer.input {
            field.value.clone_from(&buffer.input);
        }
    }
}

#[derive(Component)]
struct DebugStatsRootNode;

type LocalFighterMutQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Fighter,
        &'static mut Health,
        &'static mut Qi,
        &'static mut PoseRuntime,
        &'static mut EntryBlend,
        &'static mut Transform,
        &'static MotionState,
        &'static SmState,
        &'static CombatState,
        Option<&'static CheatInvincible>,
        &'static DeathRespawn,
    ),
    (With<LocalFighter>, Without<GameplayCamera>),
>;
type NpcInfoQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static Health,
        &'static Transform,
        &'static SmState,
        Option<&'static MissionNpc>,
    ),
    (
        Without<LocalFighter>,
        Without<crate::plugins::network_plugin::RemoteFighter>,
    ),
>;

#[derive(bevy::ecs::system::SystemParam)]
struct ExecEnv<'w> {
    queue: ResMut<'w, PendingDebugCommands>,
    log: ResMut<'w, DebugConsoleLog>,
    names: Res<'w, CommandNameList>,
    sim: ResMut<'w, DebugSimControl>,
    stats_visible: ResMut<'w, DebugStatsVisibleFlag>,
    hitbox_visible: ResMut<'w, DebugHitboxVisible>,
    motion_log: ResMut<'w, DebugMotionLog>,
    settings: ResMut<'w, Settings>,
    next_state: ResMut<'w, NextState<AppState>>,
}

#[derive(bevy::ecs::system::SystemParam)]
struct ExecWorld<'w> {
    net: Option<Res<'w, crate::plugins::network_plugin::NetState>>,
    request: Res<'w, LaunchRequest>,
    room: Res<'w, RoomState>,
    keymap: Res<'w, Keymap>,
    waypoints: Option<Res<'w, meteor_scene::stage_loader::WaypointGraph>>,
}

#[derive(bevy::ecs::system::SystemParam)]
struct ExecScene<'w, 's> {
    synthetic: ResMut<'w, SyntheticInputStream>,
    assets: Option<Res<'w, CharacterAssetsSet>>,
    collider: Option<Res<'w, StageCollider>>,
    camera_mode: ResMut<'w, CameraMode>,
    recentre: ResMut<'w, CameraRecentreRequest>,
    cameras: Query<'w, 's, (&'static Transform, &'static ThirdPersonState), With<GameplayCamera>>,
}

#[derive(bevy::ecs::system::SystemParam)]
struct ExecFighters<'w, 's> {
    local: LocalFighterMutQuery<'w, 's>,
    npcs: NpcInfoQuery<'w, 's>,
    spawned: ResMut<'w, DebugSpawnedNpcs>,
    commands: Commands<'w, 's>,
}

fn debug_exec_queue(
    mut env: ExecEnv,
    world: ExecWorld,
    mut scene: ExecScene,
    mut fighters: ExecFighters,
) {
    let lines = std::mem::take(&mut env.queue.0);
    for line in lines {
        let (name, args) = split_command(&line);
        let args = args.as_slice();
        let local_entity = fighters.local.single().ok().map(|(e, ..)| e);
        match name.as_str() {
            "help" => help(args, &env.names, &mut env.log),
            "clear" => env.log.lines.clear(),
            "shows" => env.stats_visible.0 = parse_bool(args.first(), env.stats_visible.0),
            "speed" => match args.first().and_then(|raw| raw.parse::<f32>().ok()) {
                None => env.log.print(
                    format!("当前时间缩放：{}", fmt_f(env.sim.time_scale)),
                    ConsoleTone::Info,
                ),
                Some(v) => {
                    let clamped = v.clamp(0.05, 5.0);
                    env.sim.time_scale = clamped;
                    env.log
                        .print(format!("时间缩放：{}×", fmt_f(clamped)), ConsoleTone::Ok);
                }
            },
            "pause" => {
                env.sim.paused = parse_bool(args.first(), env.sim.paused);
                env.log.print(
                    format!(
                        "模拟：{}",
                        if env.sim.paused {
                            "已暂停"
                        } else {
                            "运行中"
                        }
                    ),
                    ConsoleTone::Ok,
                );
            }
            "fps" => match args.first().and_then(|raw| raw.parse::<u32>().ok()) {
                None => env.log.print(
                    format!(
                        "帧率上限：{}（运行时改动，未写盘）",
                        fps_value_label(env.settings.target_fps)
                    ),
                    ConsoleTone::Info,
                ),
                Some(v) => {
                    env.settings.target_fps = v.clamp(0, 240);
                    env.log.print(
                        format!("帧率上限：{}", fps_value_label(env.settings.target_fps)),
                        ConsoleTone::Ok,
                    );
                }
            },
            "hitbox" => {
                env.hitbox_visible.0 = parse_bool(args.first(), env.hitbox_visible.0);
                env.log.print(
                    format!("碰撞盒调试：{}", on_off(env.hitbox_visible.0)),
                    ConsoleTone::Ok,
                );
            }
            "motion" => {
                env.motion_log.enabled = parse_bool(args.first(), env.motion_log.enabled);
                env.log.print(
                    format!(
                        "运动调试：{}（日志目标 meteor::motion）",
                        on_off(env.motion_log.enabled)
                    ),
                    ConsoleTone::Ok,
                );
            }
            "msense" => match args.first().and_then(|raw| raw.parse::<i32>().ok()) {
                None => env.log.print(
                    format!("当前灵敏度：{}", fmt_f(env.settings.mouse_speed)),
                    ConsoleTone::Info,
                ),
                Some(v) => {
                    env.settings.mouse_speed = (v as f32).clamp(
                        crate::app_state::Settings::MOUSE_SPEED_MIN,
                        crate::app_state::Settings::MOUSE_SPEED_MAX,
                    );
                    env.log.print(
                        format!("鼠标灵敏度：{}", fmt_f(env.settings.mouse_speed)),
                        ConsoleTone::Ok,
                    );
                }
            },
            "minverse" => {
                env.settings.reverse_mouse = parse_bool(args.first(), env.settings.reverse_mouse);
                env.log.print(
                    format!(
                        "鼠标反向：{}",
                        if env.settings.reverse_mouse {
                            "开"
                        } else {
                            "关"
                        }
                    ),
                    ConsoleTone::Ok,
                );
            }
            "cheat" => match (local_entity, args.first()) {
                (None, _) => env.log.print(NO_FIGHTER_HINT.to_string(), ConsoleTone::Err),
                (Some(entity), arg) => {
                    let current = fighters
                        .local
                        .get(entity)
                        .map(|(_, _, _, _, _, _, _, _, _, _, cheat, _)| cheat.is_some())
                        .unwrap_or(false);
                    let next = parse_bool_by_str(arg.map(String::as_str), current);
                    if next {
                        fighters.commands.entity(entity).insert(CheatInvincible);
                    } else {
                        fighters.commands.entity(entity).remove::<CheatInvincible>();
                    }
                    env.log.print(
                        format!("无敌模式：{}", if next { "开" } else { "关" }),
                        ConsoleTone::Ok,
                    );
                }
            },
            "kill" => kill_command(args, local_entity, &mut fighters, &mut env.log),
            "heal" => match fighters.local.single_mut() {
                Ok((_, _, mut health, mut qi, ..)) => {
                    health.current = health.max;
                    qi.current = qi.max;
                    env.log.print(
                        format!(
                            "气血 {}/{} · 怒气 {}/{}",
                            health.current, health.max, qi.current, qi.max
                        ),
                        ConsoleTone::Ok,
                    );
                }
                Err(_) => env.log.print(NO_FIGHTER_HINT.to_string(), ConsoleTone::Err),
            },
            "hp" | "anger" => hp_anger(args, &mut fighters.local, &mut env.log, name == "hp"),
            "position" => match fighters.local.single() {
                Ok((_, _, _, _, _, _, transform, ..)) => {
                    log_position(&transform.translation, &mut env.log)
                }
                Err(_) => env.log.print(NO_FIGHTER_HINT.to_string(), ConsoleTone::Err),
            },
            "moveto" => moveto(
                args,
                &mut fighters.local,
                scene.collider.as_deref(),
                world.waypoints.as_deref(),
                &mut env.log,
            ),
            "pose" | "play" => pose_cmd(
                args,
                local_entity,
                scene.assets.as_deref(),
                &mut fighters.local,
                &mut env.log,
            ),
            "weapon" => weapon_cmd(
                args,
                &mut fighters.local,
                &mut fighters.commands,
                &mut env.log,
            ),
            "switch" => {
                scene.synthetic.push_action(
                    world.keymap.weapon_switch,
                    SyntheticAction::WeaponSwitch,
                    true,
                    None,
                );
                env.log.print(
                    "请求切换兵器(Pose 24/36)[下一帧经 E 键生产链生效]",
                    ConsoleTone::Ok,
                );
            }
            "crouch" => {
                let on = parse_bool(args.first(), false);
                scene
                    .synthetic
                    .push_action(world.keymap.crouch, SyntheticAction::Crouch, on, None);
                env.log.print(
                    format!("蹲伏：{}", if on { "1" } else { "0" }),
                    ConsoleTone::Ok,
                );
            }
            "freecam" => {
                scene.camera_mode.freecam_hold =
                    parse_bool(args.first(), scene.camera_mode.freecam_hold);
                env.log.print(
                    format!(
                        "自由镜头：{}",
                        if scene.camera_mode.freecam_hold {
                            "开(镜头驻留)"
                        } else {
                            "关(恢复跟随)"
                        }
                    ),
                    ConsoleTone::Ok,
                );
            }
            "camera" => {
                scene.recentre.0 = true;
                env.log.print("镜头已回正", ConsoleTone::Ok);
            }
            "dcam" => match scene.cameras.single() {
                Ok((transform, cam)) => {
                    let (yaw, pitch, distance) = cam.debug_snapshot();
                    env.log
                        .print("Switch Extra Camera.".to_string(), ConsoleTone::Title);
                    env.log.print(
                        format!(
                            "pos {:.2}, {:.2}, {:.2}",
                            transform.translation.x,
                            transform.translation.y,
                            transform.translation.z
                        ),
                        ConsoleTone::Info,
                    );
                    env.log.print(
                        format!(
                            "yaw {:.3} pitch {:.3} dist {:.2} freecam={}",
                            yaw,
                            pitch,
                            distance,
                            u8::from(scene.camera_mode.freecam_hold)
                        ),
                        ConsoleTone::Info,
                    );
                }
                Err(_) => env.log.print("镜头未就绪".to_string(), ConsoleTone::Err),
            },
            "serverinfo" => {
                if world.request.mode == "single" || world.room.members.is_empty() {
                    env.log
                        .print("当前为单机战局(未加入房间)", ConsoleTone::Info);
                } else {
                    let robots = world.room.members.iter().filter(|m| m.is_npc).count();
                    env.log.print(
                        format!(
                            "模式:{} · 成员:{} 人（真人 {} · NPC {}）",
                            world.request.mode,
                            world.room.members.len(),
                            world.room.members.len() - robots,
                            robots
                        ),
                        ConsoleTone::Info,
                    );
                }
            }
            "npc" => list_npcs(&fighters.npcs, &fighters.spawned, &mut env.log),
            "removenpc" => {
                let all = matches!(args.first().map(String::as_str), Some("all"));
                let removed = remove_debug_npcs(all, &mut fighters.spawned, &mut fighters.commands);
                env.log.print(
                    if removed > 0 {
                        format!("已移除 {removed} 个调试 NPC")
                    } else {
                        "没有调试 NPC".to_string()
                    },
                    ConsoleTone::Ok,
                );
            }
            "adddummy" | "adddummy2" => add_dummy(
                world.net.as_deref(),
                name == "adddummy",
                args,
                &mut fighters.local,
                &fighters.npcs,
                scene.collider.as_deref(),
                &mut fighters.spawned,
                &mut fighters.commands,
                &mut env.log,
            ),
            "loadch" => character_cmd(
                args,
                &mut fighters.local,
                &mut fighters.commands,
                &mut env.log,
            ),
            "dropw" | "record" | "replay" | "replays" | "moveto-wp" => env.log.print(
                "[未接线] 该命令依赖的宿主子系统尚未迁移（RUST-BEVY-093 后续批次）",
                ConsoleTone::Warn,
            ),
            "quit" => {
                env.next_state.set(AppState::Menu);
                env.log.print("返回主菜单", ConsoleTone::Warn);
            }
            other => {
                if other.is_empty() {
                    continue;
                }
                env.log.print(
                    format!("未知命令：{other}（help 查看全部）"),
                    ConsoleTone::Err,
                );
            }
        }
    }
}

const NO_FIGHTER_HINT: &str = "战局尚未开始 — 请先从菜单进入任务";

fn parse_bool(arg: Option<&String>, current: bool) -> bool {
    parse_bool_by_str(arg.map(String::as_str), current)
}

fn parse_bool_by_str(arg: Option<&str>, current: bool) -> bool {
    match arg {
        None | Some("") => !current,
        Some(raw) => matches!(raw, "1" | "true" | "on"),
    }
}

fn width5(value: f32) -> String {
    format!("{:>5}", value.round() as i64)
}

fn fmt_f(value: f32) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").map(str::to_string).unwrap_or(text)
}

fn fps_value_label(target_fps: u32) -> String {
    if target_fps == 0 {
        "不限".into()
    } else {
        target_fps.to_string()
    }
}

fn split_command(line: &str) -> (String, Vec<String>) {
    let mut parts = line.split_whitespace();
    let name = parts.next().unwrap_or("").to_lowercase();
    (name, parts.map(str::to_string).collect())
}

fn help(args: &[String], names: &CommandNameList, log: &mut DebugConsoleLog) {
    if let Some(target) = args.first() {
        log.print(
            format!("help <{target}> — 用法说明留待各命令接入时补齐"),
            ConsoleTone::Info,
        );
        return;
    }
    log.print("── 原版隐藏命令(Meteor.exe 同名) ──", ConsoleTone::Title);
    log.print(names.origin.join("  "), ConsoleTone::Info);
    log.print("── Web 扩展 ──", ConsoleTone::Title);
    log.print(names.web.join("  "), ConsoleTone::Info);
    log.print(
        "示例：cheat 1 · speed 0.3 · adddummy2 1 8 · moveto 0 0 · pose 253",
        ConsoleTone::Muted,
    );
}

pub(crate) fn weapon_label(id: u8) -> &'static str {
    match id {
        0 => "空手",
        1 => "飛鏢(W1)",
        2 => "飛輪(W2)",
        3 => "火銃(W3)",
        4 => "雙刺(W4)",
        5 => "匕首(W5)",
        6 => "輕劍(W6)",
        7 => "長槍(W7)",
        8 => "大刀(W8)",
        9 => "重錘(W9)",
        10 => "乾坤刀(W10)",
        11 => "指虎(W11)",
        12 => "忍刀(W12)",
        _ => "?",
    }
}

fn hp_anger(
    args: &[String],
    local: &mut LocalFighterMutQuery,
    log: &mut DebugConsoleLog,
    want_hp: bool,
) {
    let Ok((_, _, mut health, mut qi, ..)) = local.single_mut() else {
        log.print(NO_FIGHTER_HINT.to_string(), ConsoleTone::Err);
        return;
    };
    if want_hp {
        if let Some(raw) = args.first().and_then(|v| v.parse::<i64>().ok()) {
            health.current = (raw.max(0) as f32).min(health.max);
        }
        log.print(
            format!("气血 {}/{}", health.current, health.max),
            ConsoleTone::Ok,
        );
    } else {
        if let Some(raw) = args.first().and_then(|v| v.parse::<i64>().ok()) {
            qi.current = (raw.max(0) as f32).min(qi.max);
        }
        log.print(format!("怒气 {}/{}", qi.current, qi.max), ConsoleTone::Ok);
    }
}

fn log_position(pos: &Vec3, log: &mut DebugConsoleLog) {
    log.print(
        format!(": {} {} {}", width5(pos.x), width5(pos.y), width5(pos.z)),
        ConsoleTone::Info,
    );
}

fn moveto(
    args: &[String],
    local: &mut LocalFighterMutQuery,
    collider: Option<&StageCollider>,
    waypoints: Option<&meteor_scene::stage_loader::WaypointGraph>,
    log: &mut DebugConsoleLog,
) {
    let Ok((_, _, _, _, _, _, mut transform, _, _, _, _, death)) = local.single_mut() else {
        log.print(NO_FIGHTER_HINT.to_string(), ConsoleTone::Err);
        return;
    };
    match args.first().map(String::as_str) {
        Some("spawn") => {
            transform.translation = death.spawn_pos;
        }
        Some("wp") => {
            let id = args.get(1).and_then(|value| value.parse::<usize>().ok());
            let Some(position) = id
                .and_then(|id| waypoints.and_then(|graph| graph.world_position(id)))
                .map(Vec3::from_array)
            else {
                log.print("用法：moveto wp <序号>（当前路点不存在）", ConsoleTone::Err);
                return;
            };
            transform.translation = position;
        }
        _ => {
            let x = args.first().and_then(|v| v.parse::<f32>().ok());
            let z = args.get(1).and_then(|v| v.parse::<f32>().ok());
            let (Some(x), Some(z)) = (x, z) else {
                log.print("用法：moveto <x> <z>".to_string(), ConsoleTone::Err);
                return;
            };
            transform.translation.x = x;
            transform.translation.z = z;
            transform.translation.y += TELEPORT_LIFT;

            if let Some(hit) = collider
                .and_then(|stage| stage.mesh.as_ref())
                .and_then(|mesh| {
                    mesh.sample_ground_hit(x, z, GROUND_FALLBACK, transform.translation.y)
                })
            {
                transform.translation.y = hit.y;
            }
        }
    }
    let p = transform.translation;
    log.print(
        format!("已传送至 {}, {}, {}", fmt_f(p.x), fmt_f(p.y), fmt_f(p.z)),
        ConsoleTone::Ok,
    );
}

fn character_cmd(
    args: &[String],
    local: &mut LocalFighterMutQuery,
    commands: &mut Commands,
    log: &mut DebugConsoleLog,
) {
    let raw = args
        .first()
        .map(|value| value.trim_start_matches(['p', 'P']))
        .and_then(|value| value.parse::<u8>().ok());
    let Some(character_id) = raw.filter(|value| *value <= 19) else {
        log.print("用法：loadch <0-19>", ConsoleTone::Err);
        return;
    };
    let Ok((entity, fighter, ..)) = local.single_mut() else {
        log.print(NO_FIGHTER_HINT.to_string(), ConsoleTone::Err);
        return;
    };
    if fighter.character_id == character_id {
        log.print(
            format!("当前已是 {}", char_label(character_id)),
            ConsoleTone::Info,
        );
        return;
    }
    commands.entity(entity).insert(CharacterVariantChanged {
        requested_character_id: character_id,
    });
    log.print(
        format!(
            "已请求切换角色 {}（资源按需加载）",
            char_label(character_id)
        ),
        ConsoleTone::Ok,
    );
}

fn pose_cmd(
    args: &[String],
    local_entity: Option<Entity>,
    assets: Option<&CharacterAssetsSet>,
    local: &mut LocalFighterMutQuery,
    log: &mut DebugConsoleLog,
) {
    let Some(id_raw) = args.first().and_then(|v| v.parse::<u16>().ok()) else {
        log.print("用法：pose <动作编号>".to_string(), ConsoleTone::Err);
        return;
    };
    let (Some(entity), Some(assets)) = (local_entity, assets) else {
        log.print(NO_FIGHTER_HINT.to_string(), ConsoleTone::Err);
        return;
    };
    let Ok((_, fighter, _, _, mut pose, mut blend, _, _, _, _, _, _)) = local.get_mut(entity)
    else {
        log.print(NO_FIGHTER_HINT.to_string(), ConsoleTone::Err);
        return;
    };
    let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
        log.print(
            format!("模型 p{} 未加载", fighter.character_id),
            ConsoleTone::Err,
        );
        return;
    };
    if !character_assets.poses.contains_key(&id_raw) {
        log.print(format!("当前角色没有动作 {id_raw}"), ConsoleTone::Err);
        return;
    }
    character_assets.apply_pose(&mut pose, id_raw, true);
    blend.duration = 0.0;
    log.print(format!("播放动作 {id_raw}"), ConsoleTone::Ok);
}

fn weapon_cmd(
    args: &[String],
    local: &mut LocalFighterMutQuery,
    commands: &mut Commands,
    log: &mut DebugConsoleLog,
) {
    let Ok((entity, mut fighter, ..)) = local.single_mut() else {
        log.print(NO_FIGHTER_HINT.to_string(), ConsoleTone::Err);
        return;
    };
    let Some(raw) = args.first() else {
        let list = (0u8..=12)
            .map(|id| format!("{id}:{}", weapon_label(id)))
            .collect::<Vec<_>>()
            .join(" · ");
        log.print(list, ConsoleTone::Info);
        return;
    };
    let Some(id) = raw.parse::<u8>().ok().filter(|id| *id <= 12) else {
        log.print(
            format!("未知武器：{raw}（用 0-12 编号，无参列出全部）"),
            ConsoleTone::Err,
        );
        return;
    };
    fighter.weapon = id;
    fighter.weapon_slots[0] = id;
    fighter.weapon_slot = 0;
    fighter.weapon_slot_variants = [0, 0];
    fighter.weapon_variant = 0;

    commands.entity(entity).insert(WeaponVariantChanged);
    log.print(
        format!("已装备 {}（武器网格/攻击盒本帧末重建）", weapon_label(id)),
        ConsoleTone::Ok,
    );
}

fn kill_command(
    args: &[String],
    local_entity: Option<Entity>,
    fighters: &mut ExecFighters,
    log: &mut DebugConsoleLog,
) {
    let Some(target) = args.first().cloned() else {
        let Some(entity) = local_entity else {
            log.print(NO_FIGHTER_HINT.to_string(), ConsoleTone::Err);
            return;
        };
        let Ok((_, _, mut health, mut qi, ..)) = fighters.local.get_mut(entity) else {
            return;
        };
        health.current = 0.0;
        qi.current = 0.0;
        fighters.commands.entity(entity).remove::<CheatInvincible>();
        log.print("你已伏剑自尽(BeKill hp0)", ConsoleTone::Warn);
        return;
    };
    if target == "all" || target == "npcs" {
        let count = fighters.npcs.iter().count();
        for (entity, ..) in fighters.npcs.iter() {
            fighters.commands.entity(entity).despawn();
        }
        fighters.spawned.0.clear();
        log.print(format!("已清场：{count} 个 NPC"), ConsoleTone::Ok);
        return;
    }
    let found = fighters
        .npcs
        .iter()
        .find_map(|(entity, _, _, _, _, mission)| {
            mission.filter(|m| m.name == target).map(|_| entity)
        });
    match found {
        Some(entity) => {
            fighters.commands.entity(entity).despawn();
            fighters.spawned.0.retain(|e| *e != entity);
            log.print(format!("已处决：{target}"), ConsoleTone::Ok);
        }
        None => log.print(
            format!("找不到 NPC：{target}(用 npc 命令列出)"),
            ConsoleTone::Err,
        ),
    }
}

fn list_npcs(npcs: &NpcInfoQuery, spawned: &DebugSpawnedNpcs, log: &mut DebugConsoleLog) {
    if npcs.is_empty() {
        log.print("场上没有 NPC", ConsoleTone::Info);
        return;
    }
    for (entity, fighter, health, transform, sm, mission) in npcs.iter() {
        let label = mission.map_or_else(|| char_label(fighter.character_id), |m| m.name.clone());
        let team = mission.map(|m| m.team).unwrap_or(fighter.player_slot);
        let tag = if spawned.0.contains(&entity) {
            " [debug]"
        } else {
            ""
        };
        log.print(
            format!(
                "● {label}({team}) · {:?} · HP {} · ({:.0}, {:.0}){tag}",
                sm.current, health.current, transform.translation.x, transform.translation.z
            ),
            ConsoleTone::Info,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn add_dummy(
    net: Option<&crate::plugins::network_plugin::NetState>,
    passive: bool,
    args: &[String],
    local: &mut LocalFighterMutQuery,
    npcs: &NpcInfoQuery,
    collider: Option<&StageCollider>,
    spawned: &mut DebugSpawnedNpcs,
    commands: &mut Commands,
    log: &mut DebugConsoleLog,
) {
    let model = args.first().and_then(|v| v.parse::<u8>().ok()).unwrap_or(0);
    let weapon = args
        .get(1)
        .and_then(|v| v.parse::<u8>().ok())
        .unwrap_or(6)
        .min(12);
    let mut status = String::new();
    spawn_debug_npc(
        net,
        model,
        weapon,
        0,
        if passive {
            DebugNpcBehavior::Dummy
        } else {
            DebugNpcBehavior::Follow
        },
        local,
        npcs,
        collider,
        spawned,
        commands,
        &mut status,
    );
    log.print(
        status,
        if local.is_empty() {
            ConsoleTone::Err
        } else {
            ConsoleTone::Ok
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn spawn_debug_npc(
    net: Option<&crate::plugins::network_plugin::NetState>,
    model: u8,
    weapon: u8,
    team: u8,
    behavior: DebugNpcBehavior,
    local: &mut LocalFighterMutQuery,
    npcs: &NpcInfoQuery,
    collider: Option<&StageCollider>,
    spawned: &mut DebugSpawnedNpcs,
    commands: &mut Commands,
    status: &mut String,
) {
    if net.is_some_and(|net| {
        net.status == crate::plugins::network_plugin::NetStatus::InRoom && !net.is_stock_host()
    }) {
        *status = "联机 NPC 请由局域网房主添加".into();
        return;
    }
    let Ok((_, _, _, _, _, _, origin_transform, origin_motion, ..)) = local.single() else {
        *status = NO_FIGHTER_HINT.to_string();
        return;
    };
    let slot = npcs
        .iter()
        .map(|(_, f, ..)| f.player_slot)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let yaw = origin_motion.facing;

    let base = Vec3::new(
        origin_transform.translation.x + yaw.sin() * DUMMY_SPAWN_DISTANCE,
        origin_transform.translation.y,
        origin_transform.translation.z + yaw.cos() * DUMMY_SPAWN_DISTANCE,
    );
    let ground_y = collider
        .and_then(|stage| stage.mesh.as_ref())
        .and_then(|mesh| mesh.sample_ground_hit(base.x, base.z, GROUND_FALLBACK, base.y))
        .map_or(base.y, |GroundHit { y, .. }| y);
    let spawn_pos = Vec3::new(base.x, ground_y, base.z);
    let serial = spawned.0.len() + 1;
    let (forced_behavior, behavior_args) = match behavior {
        DebugNpcBehavior::Follow => (
            Some(BehaviorMode::Follow),
            vec![meteor_scene::pst_runtime::PstValue::Num(0.0)],
        ),
        DebugNpcBehavior::Dummy | DebugNpcBehavior::Wait => (Some(BehaviorMode::Wait), Vec::new()),
        DebugNpcBehavior::Patrol => (Some(BehaviorMode::Patrol), Vec::new()),
    };
    let performance = if behavior == DebugNpcBehavior::Dummy {
        NpcPerformance {
            pause_remaining: f32::INFINITY,
            ..default()
        }
    } else {
        NpcPerformance::default()
    };
    let mut entity_commands = commands.spawn((
        (
            NpcFighter,
            DebugSpawnedNpc,
            MissionNpc {
                id: -(serial as i64),
                name: format!("DebugNPC_{serial}"),
                team: team.min(2),
                in_scene_roster: false,
                view: 200.0,
                think: 50.0,
                forced_behavior,
                is_vip: false,
                attack_target_slot: None,
                behavior_args,
                attack_target_remaining: 0,
            },
            performance,
            Fighter {
                character_id: model.min(19),
                player_slot: slot,
                weapon,
                weapon_slots: [weapon, 5],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
        ),
        (
            Health {
                current: 1000.0,
                max: 1000.0,
            },
            Qi::default(),
            CombatState::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: yaw + std::f32::consts::PI,
            },
            PoseRuntime::idle(),
            SmState::default(),
            DeathRespawn {
                spawn_pos,
                ..default()
            },
        ),
        (
            FighterAir::default(),
            FighterDodge::default(),
            AiIntent::default(),
            crate::plugins::ai_plugin::NpcPatrol::default(),
            NpcCombatProfile::web_debug(),
            Transform::from_translation(spawn_pos),
        ),
    ));
    entity_commands.insert(Visibility::default());
    entity_commands.insert(InheritedVisibility::default());
    entity_commands.insert(EntryBlend::default());
    entity_commands.insert(FighterAttackMotion::default());
    entity_commands.insert(VisualRootPin::default());
    entity_commands.insert(TargetHitClock::default());
    let entity = entity_commands.id();
    spawned.0.push(entity);
    *status = format!(
        "已添加 DebugNPC_{serial}（team {}, {}）于 ({:.1}, {:.1})",
        team.min(2),
        behavior.label(),
        spawn_pos.x,
        spawn_pos.z
    );
}

fn remove_debug_npcs(all: bool, spawned: &mut DebugSpawnedNpcs, commands: &mut Commands) -> usize {
    if all {
        let count = spawned.0.len();
        for entity in spawned.0.drain(..) {
            commands.entity(entity).despawn();
        }
        count
    } else if let Some(entity) = spawned.0.pop() {
        commands.entity(entity).despawn();
        1
    } else {
        0
    }
}

fn char_label(character_id: u8) -> String {
    let known = match character_id {
        0 => Some("孟星魂"),
        1 => Some("冷燕"),
        2 => Some("大刀装"),
        3 => Some("夜猫子"),
        8 => Some("守卫"),
        9 => Some("哨兵长"),
        19 => Some("轻剑装"),
        _ => None,
    };
    match known {
        Some(name) => format!("P{character_id} {name}"),
        None => format!("P{character_id}"),
    }
}

type DebugMotionFighterQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static Transform,
        &'static MotionState,
        &'static PoseRuntime,
        &'static SmState,
        Option<&'static AiIntent>,
        Option<&'static MissionNpc>,
    ),
>;

type DebugAirFighterQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static MotionState,
        &'static PoseRuntime,
        &'static SmState,
        &'static FighterAir,
        &'static FighterWall,
        &'static FighterHitReaction,
        Option<&'static WeaponSwitchActive>,
    ),
    With<LocalFighter>,
>;

#[derive(Debug, Clone, Copy, Default)]
struct DebugWallProbeSample {
    hit: bool,
    climbable: bool,
    distance: f32,
    normal: Vec3,
}

fn debug_wall_probe<M: MeshCollider>(
    mesh: &M,
    position: Vec3,
    direction: Vec3,
    max_distance: f32,
) -> DebugWallProbeSample {
    let direction = direction.normalize_or_zero();
    if direction.length_squared() < 1e-12 {
        return DebugWallProbeSample::default();
    }
    let origin = Point3::new(
        position.x,
        position.y + wall_run::WALL_PROBE_HEIGHT,
        position.z,
    );
    let Some(hit) = mesh.raycast_hit(
        origin,
        Point3::new(direction.x, direction.y, direction.z),
        max_distance,
    ) else {
        return DebugWallProbeSample::default();
    };
    DebugWallProbeSample {
        hit: true,
        climbable: wall_run::wall_normal_climbable(wall_run::Vec3::new(hit.nx, hit.ny, hit.nz)),
        distance: hit.t,
        normal: Vec3::new(hit.nx, hit.ny, hit.nz),
    }
}

fn fmt_wall_probe(sample: DebugWallProbeSample) -> String {
    if !sample.hit {
        return "miss".into();
    }
    format!(
        "t={:.3},n=({:.3},{:.3},{:.3}),climb={}",
        sample.distance, sample.normal.x, sample.normal.y, sample.normal.z, sample.climbable,
    )
}

fn debug_air_discrete_state(
    state: &SmState,
    pose: &PoseRuntime,
    air: &FighterAir,
    wall: &FighterWall,
) -> DebugAirDiscreteState {
    DebugAirDiscreteState {
        airborne: air.air.airborne,
        motion_mode: air.air.motion_mode,
        fighter_state: state.current,
        pose_id: pose.pose_id,
        cling_active: wall.cling_active,
        cling_pose: wall.cling_pose,
        kick_pose: wall.kick_pose,
        contact_seen: wall.contact_seen,
        cling_armed: air.air.cling_armed,
        ground_contact: air.ground_contact,
        ledge_fall: air.ledge_fall,
        pending_ledge_fall: air.pending_ledge_fall,
        launch_pose_id: air.air.launch_pose_id,
        launch_lifted: air.air.launch_lifted,
    }
}

fn debug_air_transition_events(
    previous: Option<DebugAirDiscreteState>,
    current: DebugAirDiscreteState,
    jump_pressed: bool,
    jump_released: bool,
    jump_accepted: bool,
) -> Vec<&'static str> {
    let mut events = Vec::new();
    if previous.is_none() {
        events.push("observe-start");
    }
    if jump_pressed {
        events.push("jump-press");
    }
    if jump_released {
        events.push(if jump_accepted {
            "jump-release-accepted"
        } else {
            "jump-release-rejected"
        });
    }
    let Some(previous) = previous else {
        return events;
    };
    if previous.airborne != current.airborne {
        events.push(if current.airborne { "takeoff" } else { "land" });
    }
    if previous.motion_mode != current.motion_mode {
        events.push("air-mode-change");
    }
    if previous.cling_active != current.cling_active {
        events.push(if current.cling_active {
            "wall-cling-enter"
        } else {
            "wall-cling-exit"
        });
    }
    if previous.kick_pose != current.kick_pose && current.kick_pose != 0 {
        events.push("wall-kick-pose");
    }
    if previous.cling_armed != current.cling_armed {
        events.push(if current.cling_armed {
            "wall-cling-armed"
        } else {
            "wall-cling-disarmed"
        });
    }
    if (previous.fighter_state != current.fighter_state || previous.pose_id != current.pose_id)
        && (previous.airborne || current.airborne || previous.cling_active || current.cling_active)
    {
        events.push("air-pose-state-change");
    }
    events
}

#[allow(clippy::too_many_arguments)]
fn debug_air_state_log(
    time: Res<Time<Real>>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    config: Res<DebugMotionLog>,
    mut trace: ResMut<DebugAirTrace>,
    jump: Option<Res<LocalJumpInput>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keymap: Res<Keymap>,
    collider: Option<Res<StageCollider>>,
    fighter: DebugAirFighterQuery<'_, '_>,
) {
    if !config.enabled {
        *trace = DebugAirTrace::default();
        return;
    }
    let Ok((entity, transform, motion, pose, state, air, wall, hit_reaction, weapon_switch)) =
        fighter.single()
    else {
        trace.previous = None;
        trace.sample_elapsed = 0.0;
        return;
    };
    let default_jump = LocalJumpInput::default();
    let jump = jump.as_deref().unwrap_or(&default_jump);
    let current = debug_air_discrete_state(state, pose, air, wall);
    let previous = trace.previous;
    let events = debug_air_transition_events(
        previous,
        current,
        jump.pressed,
        jump.released,
        jump.action_accepted,
    );

    trace.sample_elapsed += time.delta_secs();
    let periodic = air.air.airborne && trace.sample_elapsed >= 0.1;
    if periodic {
        trace.sample_elapsed = 0.0;
    }
    trace.previous = Some(current);
    if events.is_empty() && !periodic {
        return;
    }

    let input = directional_input(&keys, &mouse, &keymap);
    let forward_held = keymap.move_up.pressed(&keys, &mouse);
    let jump_physically_held = keymap.jump.pressed(&keys, &mouse);
    let attacking = matches!(
        state.current,
        meteor_core::FighterState::Attack1
            | meteor_core::FighterState::Attack2
            | meteor_core::FighterState::Attack3
    );
    let ordinary = ordinary_locomotion_allowed(state, weapon_switch.is_some());
    let gate_delay = air.air.air_time >= 0.1;
    let gate_rising = air.air.vertical_speed >= 0.0;
    let gate_height = transform.translation.y > air.ground_y + 0.25;
    let wall_gate = air.air.airborne
        && ordinary
        && !attacking
        && weapon_switch.is_none()
        && !wall.cling_active
        && air.air.cling_armed
        && gate_delay
        && forward_held
        && gate_rising
        && gate_height;

    let facing = Vec3::new(motion.facing.sin(), 0.0, motion.facing.cos());
    let right = Vec3::new(-facing.z, 0.0, facing.x);
    let planar = wall_run::Vec3::new(air.air.planar_x, 0.0, air.air.planar_z);
    let primary =
        wall_run::wall_cling_probe_direction(wall_run::Vec3::new(facing.x, 0.0, facing.z), planar);
    let primary = Vec3::new(primary.x, primary.y, primary.z);
    let side = (input.x.abs() > 0.1).then(|| {
        let sign = input.x.signum();
        Vec3::new(-facing.z * sign, 0.0, facing.x * sign)
    });
    let mesh = collider
        .as_deref()
        .and_then(|collider| collider.mesh.as_ref());
    let mesh_ready = mesh.is_some();
    let cling_front = mesh
        .map(|mesh| {
            debug_wall_probe(
                mesh,
                transform.translation,
                primary,
                wall_run::WALL_CLING_PROBE,
            )
        })
        .unwrap_or_default();
    let cling_side = side
        .and_then(|direction| {
            let mesh = mesh?;
            Some(debug_wall_probe(
                mesh,
                transform.translation,
                direction,
                wall_run::WALL_CLING_PROBE,
            ))
        })
        .unwrap_or_default();
    let kick_front = mesh
        .map(|mesh| {
            debug_wall_probe(
                mesh,
                transform.translation,
                facing,
                wall_run::WALL_KICK_PROBE,
            )
        })
        .unwrap_or_default();
    let kick_left = mesh
        .map(|mesh| {
            debug_wall_probe(
                mesh,
                transform.translation,
                (facing + right).normalize_or_zero(),
                wall_run::WALL_KICK_PROBE,
            )
        })
        .unwrap_or_default();
    let kick_right = mesh
        .map(|mesh| {
            debug_wall_probe(
                mesh,
                transform.translation,
                (facing - right).normalize_or_zero(),
                wall_run::WALL_KICK_PROBE,
            )
        })
        .unwrap_or_default();

    trace.sequence = trace.sequence.wrapping_add(1);
    let event = if events.is_empty() {
        "air-tick".to_string()
    } else {
        events.join("+")
    };
    let sim_dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let wall_dt = frame_clock
        .as_deref()
        .map_or_else(|| time.delta_secs(), |clock| clock.wall_dt);
    info!(
        target: "meteor::motion",
        "air seq={} event={} actor={:?} dt(sim={:.4},wall={:.4}) input=({:.0},{:.0}) forward={} jump(edgeP={},edgeR={},physical={},held={},accepted={},hold={:.3}) state={:?} pose={}:{:.2} moveMode={:?} air(mode={:?},time={:.3},v=({:.3},{:.3},{:.3}),armed={}) pos=({:.3},{:.3},{:.3}) motionV=({:.3},{:.3},{:.3}) ground(y={:.3},contact={},peak={:.3},ledge={},pending={}) wall(active={},pose={},kick={},remain={:.3},seen={},normal=({:.3},{:.3},{:.3}),ext=({:.3},{:.3},{:.3})) wallGate={} gates(ord={},attack={},switch={},delay={},forward={},rising={},height={}) probes(mesh={},clingFront=[{}],clingSide=[{}],kickF=[{}],kickL=[{}],kickR=[{}]) rootPinPose={} rootPin={} reaction={:?}",
        trace.sequence,
        event,
        entity,
        sim_dt,
        wall_dt,
        input.x,
        input.z,
        forward_held,
        jump.pressed,
        jump.released,
        jump_physically_held,
        jump.held,
        jump.action_accepted,
        jump.hold_seconds,
        state.current,
        pose.pose_id,
        pose.frame,
        motion.movement_mode,
        air.air.motion_mode,
        air.air.air_time,
        air.air.planar_x,
        air.air.vertical_speed,
        air.air.planar_z,
        air.air.cling_armed,
        transform.translation.x,
        transform.translation.y,
        transform.translation.z,
        motion.velocity.x,
        motion.velocity.y,
        motion.velocity.z,
        air.ground_y,
        air.ground_contact,
        air.peak_lift,
        air.ledge_fall,
        air.pending_ledge_fall,
        wall.cling_active,
        wall.cling_pose,
        wall.kick_pose,
        wall.remaining,
        wall.contact_seen,
        wall.wall_normal.x,
        wall.wall_normal.y,
        wall.wall_normal.z,
        wall.external_accel.x,
        wall.external_accel.y,
        wall.external_accel.z,
        wall_gate,
        ordinary,
        attacking,
        weapon_switch.is_some(),
        gate_delay,
        forward_held,
        gate_rising,
        gate_height,
        mesh_ready,
        fmt_wall_probe(cling_front),
        fmt_wall_probe(cling_side),
        fmt_wall_probe(kick_front),
        fmt_wall_probe(kick_left),
        fmt_wall_probe(kick_right),
        air.motion_root_pin_pose_id,
        air.motion_root_pin.is_some(),
        hit_reaction.current.as_ref().map(|reaction| reaction.kind),
    );
}

fn debug_motion_log(
    time: Res<Time<Real>>,
    mut config: ResMut<DebugMotionLog>,
    fighters: DebugMotionFighterQuery<'_, '_>,
) {
    if !config.enabled {
        config.elapsed = 0.0;
        return;
    }
    config.elapsed += time.delta_secs();
    if config.elapsed < STATS_INTERVAL_SECS {
        return;
    }
    config.elapsed = 0.0;
    for (entity, fighter, transform, motion, pose, state, intent, mission) in &fighters {
        info!(
            target: "meteor::motion",
            "actor={:?} name={} slot={} pos=({:.3},{:.3},{:.3}) vel=({:.3},{:.3},{:.3}) facing={:.4} mode={:?} state={:?} pose={}:{} enemy={:?} move=({:.3},{:.3}) local=({:.1},{:.1})",
            entity,
            mission.map_or("player/debug", |npc| npc.name.as_str()),
            fighter.player_slot,
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
            motion.velocity.x,
            motion.velocity.y,
            motion.velocity.z,
            motion.facing,
            motion.movement_mode,
            state.current,
            pose.pose_id,
            pose.frame,
            intent.and_then(|intent| intent.enemy_id),
            intent.map_or(0.0, |intent| intent.move_dir.x),
            intent.map_or(0.0, |intent| intent.move_dir.y),
            intent.map_or(0.0, |intent| intent.locomotion_input.x),
            intent.map_or(0.0, |intent| intent.locomotion_input.y),
        );
    }
}

type DebugHitboxFighterQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static PoseRuntime,
        &'static WeaponForm,
    ),
>;

fn visit_debug_hitboxes(
    assets: &CharacterAssetsSet,
    fighters: &DebugHitboxFighterQuery<'_, '_>,
    geometry: &HitGeometryQueries<'_, '_>,
    mut draw: impl FnMut(Obb, bool),
) {
    for (entity, fighter, pose, form) in fighters {
        let Some(character) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        for obb in geometry.hurt_obbs(entity, character) {
            draw(obb, false);
        }
        let Some(pose_data) = character.poses.get(&pose.pose_id) else {
            continue;
        };
        for attack in &pose_data.attacks {
            if pose.frame < attack.start as f32 || pose.frame > attack.end as f32 {
                continue;
            }
            for obb in geometry.attack_obbs(
                entity,
                fighter,
                form,
                pose.pose_id,
                character,
                &attack.bones,
            ) {
                draw(obb, true);
            }
            if attack
                .bones
                .iter()
                .any(|name| name.eq_ignore_ascii_case("effect"))
            {
                for obb in geometry.live_effect_obbs(entity, character) {
                    draw(obb, true);
                }
            }
        }
    }
}

fn debug_obb_transform(obb: &Obb) -> GlobalTransform {
    let vector = |v: meteor_core::Vec3| Vec3::new(v.x, v.y, v.z);
    GlobalTransform::from(Mat4::from_cols(
        (vector(obb.axes[0]) * obb.half_size.x * 2.0).extend(0.0),
        (vector(obb.axes[1]) * obb.half_size.y * 2.0).extend(0.0),
        (vector(obb.axes[2]) * obb.half_size.z * 2.0).extend(0.0),
        vector(obb.center).extend(1.0),
    ))
}

fn draw_debug_hitboxes(
    visible: Res<DebugHitboxVisible>,
    assets: Option<Res<CharacterAssetsSet>>,
    gizmos: Option<Gizmos<HitboxGizmos>>,
    fighters: DebugHitboxFighterQuery<'_, '_>,
    geometry: HitGeometryQueries<'_, '_>,
) {
    if !visible.0 {
        return;
    }
    let (Some(assets), Some(mut gizmos)) = (assets, gizmos) else {
        return;
    };
    visit_debug_hitboxes(&assets, &fighters, &geometry, |obb, attack| {
        let color = if attack {
            Color::srgba(1.0, 0.2, 34.0 / 255.0, 0.95)
        } else {
            Color::srgba(0.2, 1.0, 0.4, 0.9)
        };
        gizmos.cube(debug_obb_transform(&obb), color);
    });
}

#[allow(clippy::too_many_arguments)]
fn update_debug_stats(
    time: Res<Time<Real>>,
    mut fps: ResMut<DebugFpsSample>,
    visible: Res<DebugStatsVisibleFlag>,
    panel: Res<DebugPanelState>,
    display: (
        Res<Settings>,
        Option<Res<super::display_settings_plugin::ActualRenderResolution>>,
        Query<&Window, With<bevy::window::PrimaryWindow>>,
    ),
    sim: Res<DebugSimControl>,
    keymap: Res<Keymap>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    lock: Res<LockOnController>,
    npcs: Query<(), With<NpcFighter>>,
    mut local: LocalFighterMutQuery<'_, '_>,
    npc_names: Query<&MissionNpc>,
    assets: Option<Res<CharacterAssetsSet>>,
    mut throttle: Local<f32>,
    mut texts: DebugStatsTextQuery<'_, '_>,
) {
    let (settings, actual_resolution, windows) = display;
    let raw_dt = time.delta_secs();
    if raw_dt > 0.0 {
        fps.ema_dt = if fps.ema_dt <= 0.0 {
            raw_dt
        } else {
            fps.ema_dt + (raw_dt - fps.ema_dt) * 0.1
        };
    }
    let panel_visible = panel.open && panel.tab == DebugPanelTab::Stats;
    if !visible.0 && !panel_visible {
        *throttle = STATS_INTERVAL_SECS;
        return;
    }
    *throttle += raw_dt;
    if *throttle < STATS_INTERVAL_SECS {
        return;
    }
    *throttle = 0.0;
    let mut resolution = actual_resolution
        .as_deref()
        .map_or_else(|| "3D 实际渲染：等待渲染器".into(), |r| r.label());
    if let Ok(window) = windows.single() {
        resolution.push_str(&format!(
            "\n窗口输出：{}×{}",
            window.physical_width(),
            window.physical_height()
        ));
    }
    let held_keys = [
        ("W", keymap.move_up.pressed(&keys, &mouse)),
        ("A", keymap.move_left.pressed(&keys, &mouse)),
        ("S", keymap.move_down.pressed(&keys, &mouse)),
        ("D", keymap.move_right.pressed(&keys, &mouse)),
    ]
    .into_iter()
    .filter_map(|(label, on)| on.then_some(label))
    .collect::<Vec<_>>()
    .join("");

    let fps_now = if fps.ema_dt > 0.0 {
        1.0 / fps.ema_dt
    } else {
        0.0
    };
    let sim_note = if sim.paused {
        " [paused]".to_string()
    } else if (sim.time_scale - 1.0).abs() > 1e-3 {
        format!(" [speed {}]", fmt_f(sim.time_scale))
    } else {
        String::new()
    };

    let Ok((
        _,
        fighter,
        health,
        qi,
        pose,
        _blend,
        transform,
        motion,
        sm,
        combat_state,
        cheat,
        _death,
    )) = local.single_mut()
    else {
        let body = format!(
            "FPS {:.0} / 目标 {}{sim_note}\n{resolution}\n角色状态尚未就绪\nNPC 数量 {}\n",
            fps_now,
            fps_value_label(settings.target_fps),
            npcs.iter().count()
        );
        for (mut text, panel_copy) in &mut texts {
            if (if panel_copy.is_some() {
                panel_visible
            } else {
                visible.0
            }) && text.0 != body
            {
                text.0.clone_from(&body);
            }
        }
        return;
    };

    let combat_note = if combat_state.state != sm.current {
        format!(" Combat={:?}", combat_state.state)
    } else {
        String::new()
    };
    let anim_poses = assets
        .as_deref()
        .and_then(|set| set.0.get(&format!("p{}", fighter.character_id)))
        .map(|ca| ca.poses.len().to_string())
        .unwrap_or_else(|| "-".into());
    let body = format!(
        "FPS {:.0} / 目标 {}{sim_note}\n{resolution}\n位置 {:7.1} {:7.1} {:7.1}\n速度 {:6.1} {:6.1} {:6.1} 朝向 {:.2}\n状态 {:?}{combat_note} 阶段 {}\n动作 {} 帧 {:.1}/{}/{}{}\n气血 {:.0}/{:.0}{}  怒气 {:.0}/{:.0}\n武器 {}\n按键 {}  NPC {}  动作库 {}\n锁定={}  跟踪={}\n",
        fps_now,
        fps_value_label(settings.target_fps),
        transform.translation.x,
        transform.translation.y,
        transform.translation.z,
        motion.velocity.x,
        motion.velocity.y,
        motion.velocity.z,
        motion.facing,
        sm.current,
        sm.attack_stage,
        pose.pose_id,
        pose.frame,
        pose.end,
        pose.loops,
        if pose.loops { format!(" loop[{}/{}]", pose.loop_start, pose.loop_end) } else { String::new() },
        health.current,
        health.max,
        if cheat.is_some() { "[无敌]" } else { "" },
        qi.current,
        qi.max,
        weapon_label(fighter.weapon),
        if held_keys.is_empty() { "-" } else { &held_keys },
        npcs.iter().count(),
        anim_poses,
        lock_name(lock.lock_entity, lock_track_fallback(false), &npc_names),
        lock_name(lock.track_entity, lock_track_fallback(true), &npc_names),
    );
    for (mut text, panel_copy) in &mut texts {
        if (if panel_copy.is_some() {
            panel_visible
        } else {
            visible.0
        }) && text.0 != body
        {
            text.0.clone_from(&body);
        }
    }
}

fn lock_track_fallback(_track: bool) -> &'static str {
    "本地玩家"
}

fn lock_name(entity: Option<Entity>, fallback: &str, npc_names: &Query<&MissionNpc>) -> String {
    let Some(entity) = entity else {
        return "-".to_string();
    };
    if let Ok(mission) = npc_names.get(entity) {
        return mission.name.clone();
    }
    fallback.to_string()
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_panel_uses_configured_binding_and_closes_console() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<Keymap>()
            .init_resource::<DebugConsoleState>()
            .init_resource::<DebugPanelState>()
            .add_systems(Update, debug_console_toggle);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Backquote);
        app.update();
        assert!(app.world().resource::<DebugPanelState>().open);
        assert!(!app.world().resource::<DebugConsoleState>().open);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut().resource_mut::<Keymap>().debug_panel =
            crate::app_state::InputBinding::Mouse(MouseButton::Middle);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Backquote);
        app.update();
        assert!(app.world().resource::<DebugPanelState>().open);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Middle);
        app.update();
        assert!(!app.world().resource::<DebugPanelState>().open);
        app.world_mut().resource_mut::<DebugConsoleState>().open = true;
        app.update();
        assert!(app.world().resource::<DebugPanelState>().open);
        assert!(!app.world().resource::<DebugConsoleState>().open);
    }

    fn hitbox_fixture() -> (App, Entity) {
        use crate::plugins::character_plugin::{
            load_character_asset_for_test, spawn_fighter_model_for_test, CommonAmb,
        };
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character = load_character_asset_for_test(std::slice::from_ref(&root), "p0").unwrap();
        let common = meteor_asset::amb::parse_amb(
            &std::fs::read(root.join("Common/character.amb")).unwrap(),
        )
        .unwrap();
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
                "p0".into(),
                character,
            )])))
            .insert_resource(CommonAmb(common));
        let fighter = app
            .world_mut()
            .spawn((
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                PoseRuntime::idle(),
                WeaponForm::default(),
                SmState::default(),
                EntryBlend::default(),
                FighterHitReaction::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.7,
                },
                Transform::from_xyz(3.0, 2.0, 5.0),
            ))
            .id();
        spawn_fighter_model_for_test(app.world_mut(), fighter);
        (app, fighter)
    }

    fn sample_debug_boxes(
        app: &mut App,
        fighter: Entity,
        pose_id: u16,
        frame: f32,
    ) -> (Vec<Obb>, Vec<Obb>) {
        use bevy::ecs::system::SystemState;
        let (start, end) = {
            let assets = app.world().resource::<CharacterAssetsSet>();
            let pose = &assets.0["p0"].poses[&pose_id];
            (pose.start, pose.end)
        };
        {
            let mut pose = app.world_mut().get_mut::<PoseRuntime>(fighter).unwrap();
            pose.pose_id = pose_id;
            pose.start = start;
            pose.end = end;
            pose.frame = frame;
        }
        crate::plugins::character_plugin::drive_fighter_animation_for_test(app.world_mut());
        let mut state = SystemState::<(
            Res<CharacterAssetsSet>,
            DebugHitboxFighterQuery,
            HitGeometryQueries,
        )>::new(app.world_mut());
        let (assets, fighters, geometry) = state.get(app.world()).unwrap();
        let (mut hurt, mut attack) = (Vec::new(), Vec::new());
        visit_debug_hitboxes(&assets, &fighters, &geometry, |obb, attacking| {
            if attacking {
                attack.push(obb);
            } else {
                hurt.push(obb);
            }
        });
        (hurt, attack)
    }

    #[test]
    fn debug_hitboxes_follow_real_skin_and_show_authored_kick_only_in_window() {
        let (mut app, fighter) = hitbox_fixture();

        let (hurt, attack) = sample_debug_boxes(&mut app, fighter, 269, 10086.0);
        let expected = {
            let assets = app.world().resource::<CharacterAssetsSet>();
            let character = &assets.0["p0"];
            character
                .skeleton
                .skin_indices
                .iter()
                .enumerate()
                .filter(|(i, node)| {
                    meteor_combat::hitbox::is_hurt_bone_name(&character.skeleton.nodes[**node].name)
                        && character
                            .bone_local_bounds
                            .get(*i)
                            .is_some_and(Option::is_some)
                })
                .count()
        };
        assert!(
            expected > 10,
            "must draw individual body bones, not a movement box"
        );
        assert_eq!(hurt.len(), expected);
        assert_eq!(
            attack.len(),
            5,
            "effect excluded; all five authored body bones visible"
        );
        let (next_hurt, next_attack) = sample_debug_boxes(&mut app, fighter, 269, 10087.5);
        assert!(
            next_attack.is_empty(),
            "Web overlay uses fractional frame <= end"
        );
        assert!(hurt
            .iter()
            .zip(&next_hurt)
            .any(|(a, b)| (a.center - b.center).length() > 0.001));
        let (_, before) = sample_debug_boxes(&mut app, fighter, 269, 10084.9);
        assert!(before.is_empty());
    }

    #[test]
    fn debug_hitboxes_use_live_weapon_boxes_and_filter_hidden_parts() {
        use crate::components::fighter::WeaponCollisionGeometry;
        let (mut app, fighter) = hitbox_fixture();
        let (pose_id, frame) = {
            let assets = app.world().resource::<CharacterAssetsSet>();
            let pose = &assets.0["p0"].poses[&260];
            let attack = pose
                .attacks
                .iter()
                .find(|a| a.bones.iter().any(|b| b == "weapon"))
                .unwrap();
            (pose.id, attack.start as f32)
        };
        let (_, boxes) = sample_debug_boxes(&mut app, fighter, pose_id, frame);
        assert!(
            boxes.len() > 3,
            "real sword adds weapon boxes alongside Pose 260's three arm bones"
        );
        let delta = Vec3::new(11.0, -2.0, 7.0);
        app.world_mut()
            .get_mut::<Transform>(fighter)
            .unwrap()
            .translation += delta;
        let (_, moved) = sample_debug_boxes(&mut app, fighter, pose_id, frame);
        assert_eq!(boxes.len(), moved.len());
        for (a, b) in boxes.iter().zip(&moved) {
            let d = b.center - a.center;
            assert!(
                (Vec3::new(d.x, d.y, d.z) - delta).length() < 1e-4,
                "must follow current hierarchy even before GlobalTransform propagation"
            );
        }
        for (geometry, mut visibility) in app
            .world_mut()
            .query::<(&WeaponCollisionGeometry, &mut Visibility)>()
            .iter_mut(app.world_mut())
        {
            if geometry.fighter == fighter {
                *visibility = Visibility::Hidden;
            }
        }
        let (_, hidden) = sample_debug_boxes(&mut app, fighter, pose_id, frame);
        assert_eq!(
            hidden.len(),
            3,
            "hiding the sword keeps the authored arm attack boxes"
        );
    }

    #[test]
    fn debug_hitbox_render_transform_preserves_obb_corners() {
        use meteor_combat::hitbox::{bone_world_obb, BoneLocalBox};
        let source = Mat4::from_scale_rotation_translation(
            Vec3::new(-0.12, 0.2, 0.3),
            Quat::from_rotation_y(0.7),
            Vec3::new(3.0, 2.0, 5.0),
        );
        let bounds = BoneLocalBox {
            center: meteor_core::Vec3::new(2.0, 3.0, 4.0),
            half_size: meteor_core::Vec3::new(1.0, 2.0, 3.0),
        };
        let obb = bone_world_obb(
            &meteor_core::Mat4 {
                m: source.to_cols_array(),
            },
            &bounds,
        )
        .unwrap();
        let transform = debug_obb_transform(&obb);
        for x in [-1.0, 1.0] {
            for y in [-1.0, 1.0] {
                for z in [-1.0, 1.0] {
                    let actual = transform.transform_point(Vec3::new(x, y, z) * 0.5);
                    let expected =
                        source.transform_point3(Vec3::new(2.0 + x, 3.0 + 2.0 * y, 4.0 + 3.0 * z));
                    assert!((actual - expected).length() < 1e-5);
                }
            }
        }
    }

    fn debug_npc_facing_fixture(yaw: f32) -> (App, Entity, Entity, Entity, Entity) {
        use crate::plugins::character_plugin::{
            load_character_asset_for_test, CharacterAssetsSet, CommonAmb, FighterModel,
        };
        use bevy::ecs::system::SystemState;

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character =
            load_character_asset_for_test(std::slice::from_ref(&root), "p1").expect("p1 assets");
        let common = meteor_asset::amb::parse_amb(
            &std::fs::read(root.join("Common/character.amb")).expect("character.amb"),
        )
        .expect("parse character.amb");
        let mut app = App::new();
        app.add_plugins(bevy::transform::TransformPlugin)
            .init_resource::<Time>()
            .init_resource::<Time<bevy::time::Real>>()
            .insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
                "p1".to_string(),
                character,
            )])))
            .insert_resource(CommonAmb(common));
        let player = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 1,
                    player_slot: 0,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health::default(),
                Qi::default(),
                PoseRuntime::idle(),
                EntryBlend::default(),
                SmState::default(),
                CombatState::default(),
                DeathRespawn::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: yaw,
                },
                Transform::from_xyz(3.0, 0.0, 5.0),
            ))
            .id();
        let mut state =
            SystemState::<(Commands, LocalFighterMutQuery, NpcInfoQuery)>::new(app.world_mut());
        let mut spawned = DebugSpawnedNpcs::default();
        let mut status = String::new();
        {
            let (mut commands, mut local, npcs) =
                state.get_mut(app.world_mut()).expect("debug spawn queries");
            spawn_debug_npc(
                None,
                1,
                6,
                2,
                DebugNpcBehavior::Wait,
                &mut local,
                &npcs,
                None,
                &mut spawned,
                &mut commands,
                &mut status,
            );
        }
        state.apply(app.world_mut());
        let npc = *spawned.0.last().expect("debug NPC spawned");

        let model = app
            .world_mut()
            .spawn((
                FighterModel {
                    fighter: npc,
                    character: "p1".to_string(),
                },
                Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                    .with_scale(Vec3::splat(0.12)),
                ChildOf(npc),
            ))
            .id();

        let marker = app
            .world_mut()
            .spawn((Transform::from_xyz(0.0, -10.0, 0.0), ChildOf(model)))
            .id();
        (app, player, npc, model, marker)
    }

    fn assert_debug_npc_world_facing(
        app: &mut App,
        player: Entity,
        npc: Entity,
        model: Entity,
        marker: Entity,
    ) {
        crate::plugins::character_plugin::drive_fighter_animation_for_test(app.world_mut());
        app.update();
        let world = app.world();
        let expected = (world.get::<Transform>(player).unwrap().translation
            - world.get::<Transform>(npc).unwrap().translation)
            .normalize();
        let model_global = world.get::<GlobalTransform>(model).unwrap();
        let visible_forward = model_global
            .affine()
            .transform_vector3(-Vec3::Y)
            .normalize();
        assert!(
            visible_forward.dot(expected) > 0.9999,
            "model world forward {visible_forward:?} must face enemy {expected:?}"
        );
        let weapon_forward = (world.get::<GlobalTransform>(marker).unwrap().translation()
            - model_global.translation())
        .normalize();
        assert!(
            weapon_forward.dot(expected) > 0.9999,
            "weapon descendant faces enemy"
        );
        assert_eq!(
            world.get::<Transform>(npc).unwrap().rotation,
            Quat::IDENTITY
        );
    }

    #[test]
    fn production_debug_npc_world_facing_applies_spawn_yaw_once() {
        for yaw in [
            0.0,
            0.7,
            std::f32::consts::FRAC_PI_2,
            std::f32::consts::PI,
            -1.2,
        ] {
            let (mut app, player, npc, model, marker) = debug_npc_facing_fixture(yaw);
            assert_debug_npc_world_facing(&mut app, player, npc, model, marker);
        }
    }

    #[test]
    fn production_debug_npc_world_facing_after_script_faceto() {
        use crate::plugins::stage_plugin::NpcPerformCommand;
        use bevy::ecs::system::RunSystemOnce;
        use meteor_scene::pst_runtime::PstValue;

        let (mut app, player, npc, model, marker) = debug_npc_facing_fixture(0.7);

        app.world_mut().get_mut::<Transform>(npc).unwrap().rotation = Quat::IDENTITY;
        let npc_pos = app.world().get::<Transform>(npc).unwrap().translation;
        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = npc_pos + Vec3::new(-3.0, 0.0, 4.0);
        app.world_mut()
            .get_mut::<NpcPerformance>(npc)
            .unwrap()
            .queue
            .extend([
                NpcPerformCommand {
                    action: "faceto".into(),
                    args: vec![PstValue::Num(0.0)],
                },
                NpcPerformCommand {
                    action: "pause".into(),
                    args: vec![PstValue::Num(1.0)],
                },
            ]);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(100));
        for _ in 0..4 {
            app.world_mut()
                .run_system_once(crate::plugins::ai_plugin::drive_npc_ai)
                .expect("production FaceTo at the script cadence");
        }
        assert_debug_npc_world_facing(&mut app, player, npc, model, marker);
    }

    fn debug_air_sample() -> DebugAirDiscreteState {
        DebugAirDiscreteState {
            airborne: false,
            motion_mode: AirMotionMode::Ballistic,
            fighter_state: meteor_core::FighterState::Name,
            pose_id: 0,
            cling_active: false,
            cling_pose: 0,
            kick_pose: 0,
            contact_seen: false,
            cling_armed: false,
            ground_contact: true,
            ledge_fall: false,
            pending_ledge_fall: false,
            launch_pose_id: 0,
            launch_lifted: false,
        }
    }

    #[test]
    fn air_trace_marks_input_takeoff_cling_and_kick_transitions() {
        let ground = debug_air_sample();
        let airborne = DebugAirDiscreteState {
            airborne: true,
            fighter_state: meteor_core::FighterState::Jump,
            pose_id: 153,
            cling_armed: true,
            ground_contact: false,
            launch_pose_id: 153,
            launch_lifted: true,
            ..ground
        };
        assert_eq!(
            debug_air_transition_events(Some(ground), airborne, true, false, false),
            [
                "jump-press",
                "takeoff",
                "wall-cling-armed",
                "air-pose-state-change"
            ]
        );

        let cling = DebugAirDiscreteState {
            motion_mode: AirMotionMode::Wall,
            pose_id: 161,
            cling_active: true,
            cling_pose: 161,
            contact_seen: true,
            ..airborne
        };
        assert_eq!(
            debug_air_transition_events(Some(airborne), cling, false, false, false),
            [
                "air-mode-change",
                "wall-cling-enter",
                "air-pose-state-change"
            ]
        );

        let kick = DebugAirDiscreteState {
            motion_mode: AirMotionMode::Ballistic,
            pose_id: 157,
            cling_active: false,
            kick_pose: 157,
            ..cling
        };
        assert_eq!(
            debug_air_transition_events(Some(cling), kick, false, true, true),
            [
                "jump-release-accepted",
                "air-mode-change",
                "wall-cling-exit",
                "wall-kick-pose",
                "air-pose-state-change"
            ]
        );
    }

    #[test]
    fn closed_debug_ui_does_not_dirty_text_or_layout_and_reopens_with_current_data() {
        use bevy::ecs::system::RunSystemOnce;
        #[derive(bevy::ecs::schedule::ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
        struct Capture;
        fn step(app: &mut App) {
            app.world_mut().run_schedule(Update);
            app.world_mut().run_schedule(Capture);
            app.world_mut().clear_trackers();
        }
        #[derive(Resource, Default)]
        struct Writes(usize, usize, usize);
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_message::<crate::plugins::native_text_input::TextSubmitted>()
            .insert_state(AppState::Playing)
            .insert_resource(LaunchRequest::default())
            .insert_resource(RoomState::default())
            .insert_resource(Settings::default())
            .insert_resource(Keymap::default())
            .insert_resource(crate::plugins::launcher_ui::UiFont(Handle::default()))
            .init_resource::<Time<Real>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<SyntheticInputStream>()
            .init_resource::<CameraMode>()
            .init_resource::<CameraRecentreRequest>()
            .init_resource::<LockOnController>()
            .init_resource::<Writes>()
            .add_plugins(DebugPlugin)
            .add_systems(
                Capture,
                |nodes: Query<(), Changed<Node>>,
                 fonts: Query<(), Changed<TextFont>>,
                 texts: Query<(), Changed<Text>>,
                 mut writes: ResMut<Writes>| {
                    *writes = Writes(
                        nodes.iter().count(),
                        fonts.iter().count(),
                        texts.iter().count(),
                    );
                },
            );
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: (872_u32, 393_u32).into(),
                ..default()
            })
            .id();
        app.world_mut().run_system_once(setup_debug_ui).unwrap();
        step(&mut app);
        assert!(
            app.world_mut()
                .query::<&DebugPanelFont>()
                .iter(app.world())
                .count()
                > 10
        );
        for _ in 0..10 {
            app.world_mut()
                .resource_mut::<Time<Real>>()
                .advance_by(std::time::Duration::from_millis(50));
            step(&mut app);
            let writes = app.world().resource::<Writes>();
            assert_eq!(
                (writes.0, writes.1, writes.2),
                (0, 0, 0),
                "closed debug panel/console must not invalidate layout or shape hidden text"
            );
        }
        app.world_mut()
            .resource_mut::<DebugConsoleLog>()
            .print("hidden-new-log", ConsoleTone::Ok);
        app.world_mut().resource_mut::<DebugConsoleBuffer>().input = "speed 1".into();
        app.world_mut().resource_mut::<DebugPanelStatus>().0 = "hidden-new-status".into();
        step(&mut app);
        assert_eq!(app.world().resource::<Writes>().2, 0);
        app.world_mut().resource_mut::<DebugConsoleState>().open = true;
        step(&mut app);
        let log = app
            .world_mut()
            .query_filtered::<&Text, With<DebugConsoleLogText>>()
            .single(app.world())
            .unwrap();
        assert!(log.0.contains("hidden-new-log"));
        let prompt = app
            .world_mut()
            .query_filtered::<&Text, With<DebugConsolePromptText>>()
            .single(app.world())
            .unwrap();
        assert_eq!(prompt.0, "» speed 1");
        step(&mut app);
        assert_eq!(
            app.world().resource::<Writes>().2,
            0,
            "unchanged visible console is cached too"
        );
        app.world_mut().resource_mut::<DebugConsoleState>().open = false;
        app.world_mut().resource_mut::<DebugPanelState>().open = true;
        step(&mut app);
        let texts: Vec<_> = app
            .world_mut()
            .query::<(&DebugPanelText, &Text)>()
            .iter(app.world())
            .map(|(_, text)| text.0.clone())
            .collect();
        assert!(texts.iter().any(|text| text == "hidden-new-status"));
        let body = app
            .world_mut()
            .query_filtered::<&Text, With<DebugPanelRuntimeText>>()
            .single(app.world())
            .unwrap();
        assert!(
            body.0.contains("FPS"),
            "reopened stats must refresh immediately"
        );
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set(1920.0, 1080.0);
        step(&mut app);
        assert!(
            app.world().resource::<Writes>().1 > 0,
            "resize must still adapt fonts"
        );
        step(&mut app);
        let writes = app.world().resource::<Writes>();
        assert_eq!((writes.0, writes.1, writes.2), (0, 0, 0));
    }

    #[test]
    fn debug_plugin_update_schedule_initializes_without_b0001() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .init_state::<AppState>()
            .add_plugins(DebugPlugin);

        app.world_mut().run_schedule(Update);

        let world = app.world();
        assert!(world.contains_resource::<DebugSpawnedNpcs>());
        assert!(world.contains_resource::<DebugFpsSample>());
        assert!(!world.resource::<DebugStatsVisibleFlag>().0);
    }

    #[test]
    fn debug_plugin_update_runs_in_playing_with_owned_resources() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_message::<crate::plugins::native_text_input::TextSubmitted>()
            .insert_state(AppState::Playing)
            .insert_resource(LaunchRequest::default())
            .insert_resource(RoomState::default())
            .insert_resource(Settings::default())
            .insert_resource(Keymap::default())
            .init_resource::<Time<Real>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<SyntheticInputStream>()
            .init_resource::<CameraMode>()
            .init_resource::<CameraRecentreRequest>()
            .init_resource::<LockOnController>()
            .add_plugins(DebugPlugin);

        app.world_mut().run_schedule(Update);
        app.world_mut().run_schedule(PostUpdate);
        let (config, _) = app
            .world()
            .resource::<bevy_gizmos::config::GizmoConfigStore>()
            .config::<HitboxGizmos>();
        assert_eq!(
            config.depth_bias, -1.0,
            "hitboxes render over the character skin"
        );
    }

    #[test]
    fn debug_panel_builds_runtime_and_three_control_pages_with_visible_labels() {
        let mut app = App::new();
        app.insert_resource(crate::plugins::launcher_ui::UiFont(Handle::default()))
            .add_systems(Update, setup_debug_ui);
        app.update();

        let world = app.world_mut();
        let tabs = world
            .query::<&DebugPanelTabRoot>()
            .iter(world)
            .map(|tab| tab.0)
            .collect::<Vec<_>>();
        assert_eq!(tabs.len(), 4);
        assert!(tabs.contains(&DebugPanelTab::Stats));
        assert!(tabs.contains(&DebugPanelTab::Loadout));
        assert!(tabs.contains(&DebugPanelTab::Options));
        assert!(tabs.contains(&DebugPanelTab::Npc));

        let actions = world
            .query::<&DebugPanelAction>()
            .iter(world)
            .copied()
            .collect::<Vec<_>>();
        assert!(actions
            .iter()
            .any(|action| matches!(action, DebugPanelAction::TogglePanel)));
        assert!(actions
            .iter()
            .any(|action| matches!(action, DebugPanelAction::AddNpc)));
        assert!(actions
            .iter()
            .any(|action| matches!(action, DebugPanelAction::Command("hitbox"))));
        assert!(actions
            .iter()
            .any(|action| matches!(action, DebugPanelAction::OpenConsole)));

        let root = world
            .query_filtered::<Entity, With<DebugPanelRoot>>()
            .single(world)
            .expect("debug panel root");
        assert!(world.entity(root).contains::<ScrollPosition>());
        assert!(world.entity(root).contains::<LauncherScrollArea>());
        assert!(world.entity(root).contains::<Interaction>());
        assert!(world
            .entity(root)
            .contains::<crate::plugins::ui_components::UiInputBlocker>());
        assert_eq!(
            world
                .query_filtered::<Entity, With<DebugPanelRuntimeText>>()
                .iter(world)
                .count(),
            1
        );
        let unstyled_actions = world
            .query_filtered::<Entity, (With<DebugPanelAction>, Without<LauncherStyledButton>)>()
            .iter(world)
            .count();
        assert_eq!(
            unstyled_actions, 0,
            "touch scrolling must own every panel button"
        );

        let action_entities = world
            .query_filtered::<Entity, With<DebugPanelAction>>()
            .iter(world)
            .collect::<Vec<_>>();
        for entity in action_entities {
            let has_visible_label = world.get::<Children>(entity).is_some_and(|children| {
                children.iter().any(|child| {
                    world
                        .get::<Text>(child)
                        .is_some_and(|text| !text.0.is_empty())
                })
            });
            assert!(
                has_visible_label,
                "debug action {entity:?} must own a label"
            );
        }
        assert_eq!(DebugPanelState::default().tab, DebugPanelTab::Stats);
    }

    #[test]
    fn short_landscape_debug_panel_uses_full_safe_height_and_compact_fonts() {
        assert_eq!(
            debug_panel_geometry(872.0, 393.0, true),
            (8.0, 54.0, 390.0, 331.0)
        );
        assert_eq!(debug_panel_font_size(18.0, true), 14.0);
        assert!((debug_panel_font_size(13.0, true) - 10.66).abs() < 1e-5);
        assert_eq!(debug_panel_font_size(10.0, true), 9.0);
        assert_eq!(debug_panel_font_size(13.0, false), 13.0);
    }

    fn ctx_log_lines(line: &str) -> Vec<String> {
        let mut log = DebugConsoleLog::default();
        log.lines.clear();
        execute_simple(&mut log, line);
        log.visible_text().lines().map(str::to_string).collect()
    }

    fn execute_simple(log: &mut DebugConsoleLog, line: &str) {
        let (name, args) = split_command(line);
        match name.as_str() {
            "" => {}
            "help" | "clear" => {}
            other => log.print(
                format!("未知命令：{other}（help 查看全部）"),
                ConsoleTone::Err,
            ),
        }
        let _ = args;
    }

    #[test]
    fn split_command_lowercases_and_collects_args() {
        assert_eq!(
            split_command("MoVeTo 10 -3.5"),
            ("moveto".into(), vec!["10".to_string(), "-3.5".to_string()])
        );
        assert_eq!(split_command("   "), (String::new(), vec![]));
    }

    #[test]
    fn bool_flags_toggle_on_missing_arg() {
        assert!(parse_bool(None, false));
        assert!(!parse_bool(Some(&"0".to_string()), true));
        assert!(parse_bool(Some(&"on".to_string()), false));
    }

    #[test]
    fn visible_text_keeps_tail_window() {
        let mut log = DebugConsoleLog::default();
        log.lines.clear();
        for i in 0..30 {
            log.print(format!("line{i}"), ConsoleTone::Info);
        }
        let text = log.visible_text();
        assert_eq!(text.lines().count(), VISIBLE_LOG_LINES);
        assert!(text.contains(&format!("line{}", 29)));
        assert!(!text.contains("line0"));
    }

    #[test]
    fn original_position_format_pads_to_five() {
        assert_eq!(width5(-1.4), "   -1");
        assert_eq!(width5(123456.4), "123456");
    }

    #[test]
    fn console_line_tone_prefixes_render() {
        let mut log = DebugConsoleLog::default();
        log.lines.clear();
        log.print("你好", ConsoleTone::Err);
        log.print("完成", ConsoleTone::Ok);
        let text = log.visible_text();
        assert!(text.starts_with("[err] 你好\n"));
        assert!(text.ends_with("[ok] 完成\n"));
    }

    #[test]
    fn stub_route_reports_unknown_commands() {
        let lines = ctx_log_lines("nosuchcmd");
        assert_eq!(lines, vec!["[err] 未知命令：nosuchcmd（help 查看全部）"]);
    }
}
