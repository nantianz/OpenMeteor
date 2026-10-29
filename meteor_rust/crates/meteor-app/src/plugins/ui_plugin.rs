#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use super::ui_theme as theme;
use crate::app_state::{
    assign_key_binding, AppState, BackToMenuButton, CharacterButton, InputBinding,
    KeyCaptureButton, KeyCaptureState, Keymap, LaunchRequest, LauncherStyledButton, LoadingRoot,
    MenuAutoRecordButton, MenuMrcButton, MenuRecordButton, MenuReplayButton, MenuRoomButton,
    MenuRoot, MenuScreen, ModeButton, NetRoomChatInputText, NetRoomChatText, NetRoomLeaveButton,
    NetRoomRoot, NetRoomStartButton, NetRoomTeamSelectButton, PlayingRoot, ResultBackToMenuButton,
    ResultRetryButton, ResultRoot, RoomAddNpcButton, RoomBackButton, RoomMember,
    RoomRemoveNpcButton, RoomRoot, RoomStartButton, RoomState, Settings, SettingsBackButton,
    SettingsRoot, StageButton, StageListTab, StartGameButton, WeaponButton, MAX_ROOM_NPC,
};
use crate::components::fighter::{
    Fighter, GunAimState, Health, KillScore, LockOnController, MatchEnd, Qi,
};
use crate::plugins::launcher_ui::{
    adapt_launcher_layout, adapt_launcher_touch_targets, launcher_touch_scroll, launcher_ui_scroll,
    menu_back_button, menu_navigate_buttons, player_loadout_controls, refresh_stage_tab_visuals,
    reset_menu_screen, responsive_ui_layout, responsive_ui_layout_with_safe_area, scrollable_y,
    setup_launcher, spawn_full_settings_page, stage_tab_buttons, sync_launcher_screen_visibility,
    update_stage_selection_labels, update_stage_start_availability, LauncherTouchScrollState,
    StageSelectionMemory, UiFont, LAUNCHER_CARD, LAUNCHER_CARD_ACTIVE, LAUNCHER_LINE,
    LAUNCHER_RED_BRIGHT,
};
use crate::plugins::mobile_controls_plugin::MobileLayoutEditorState;
use crate::plugins::settings_ui::{
    adapt_settings_layout, refresh_settings_form_labels, settings_bool_buttons,
    settings_range_down_buttons, settings_range_up_buttons, settings_reset_button,
    settings_select_buttons,
};
use crate::plugins::ui_components::UiInputBlocker;
use crate::plugins::ui_scale_plugin::SafeAreaInsets;
use bevy::prelude::*;
#[cfg(test)]
use meteor_network::modes::mode_label;
use meteor_scene::stage_registry::official_stages;

const NORMAL_BUTTON: Color = theme::CARD;
const HOVERED_BUTTON: Color = theme::HOVER;
const PRESSED_BUTTON: Color = theme::SELECTED;

const PAUSE_OVERLAY_Z_INDEX: i32 = 300;

#[allow(dead_code)]
pub(crate) fn character_label(id: u8) -> String {
    let known = match id {
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
        Some(name) => format!("P{id} {name}"),
        None => format!("P{id}"),
    }
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            super::battle_result::BattleResultPlugin,
            super::scoreboard::ScoreboardPlugin,
            theme::CinematicThemePlugin,
            super::campaign_ui::CampaignUiPlugin,
            super::loading_ui::LoadingUiPlugin,
            super::dialogue_portrait::DialoguePortraitPlugin,
        ))
        .configure_sets(
            Update,
            (
                crate::plugins::UiTouchGestureSet,
                crate::plugins::UiTouchActionSet,
            )
                .chain(),
        )
        .init_resource::<DialogueState>()
        .init_resource::<DialoguePresentation>()
        .init_resource::<RadarState>()
        .init_resource::<RadarTerrain>()
        .init_resource::<RadarAssets>()
        .init_resource::<ReplayScrubState>()
        .init_resource::<KeyCaptureState>()
        .init_resource::<LauncherTouchScrollState>()
        .init_resource::<StageSelectionMemory>()
        .init_resource::<MenuScreen>()
        .init_resource::<StageListTab>()
        .add_plugins(super::launcher_ui::UiFontPlugin)
        .add_systems(
            OnEnter(AppState::Menu),
            (reset_menu_screen, setup_menu)
                .chain()
                .after(crate::plugins::stage_plugin::discover_custom_stages),
        )
        .add_systems(OnExit(AppState::Menu), despawn_root::<MenuRoot>)
        .add_systems(
            OnEnter(AppState::Playing),
            setup_playing_hud.run_if(crate::plugins::starts_fresh_playing_session),
        )
        .add_systems(
            OnExit(AppState::Playing),
            despawn_root::<PlayingRoot>.run_if(crate::plugins::ends_playing_session),
        )
        .add_systems(OnEnter(AppState::Paused), setup_pause_menu)
        .add_systems(OnExit(AppState::Paused), despawn_root::<PauseRoot>)
        .add_systems(
            OnTransition {
                exited: AppState::Paused,
                entered: AppState::Menu,
            },
            crate::plugins::cleanup_suspended_playing_session,
        )
        .add_systems(OnEnter(AppState::Result), setup_result_panel)
        .add_systems(OnExit(AppState::Result), despawn_root::<ResultRoot>)
        .add_systems(OnEnter(AppState::Room), setup_room_panel)
        .add_systems(OnExit(AppState::Room), despawn_root::<RoomRoot>)
        .add_systems(OnEnter(AppState::Settings), setup_settings_panel)
        .add_systems(OnExit(AppState::Settings), despawn_root::<SettingsRoot>)
        .add_systems(OnEnter(AppState::Loading), reset_match_state)
        .add_systems(OnEnter(AppState::Loading), reset_radar)
        .add_systems(OnEnter(AppState::Loading), reset_dialogue)
        .add_systems(OnEnter(AppState::Error), setup_error)
        .add_systems(
            Update,
            (
                menu_mode_buttons,
                menu_stage_buttons,
                menu_character_buttons,
                menu_weapon_buttons,
                player_loadout_controls,
                menu_navigate_buttons,
                menu_back_button,
                stage_tab_buttons,
                sync_launcher_screen_visibility,
                refresh_stage_tab_visuals,
                update_stage_selection_labels,
                update_stage_start_availability,
                adapt_launcher_layout,
                start_game_button,
                back_to_menu_button,
                update_button_visuals,
                update_responsive_button_visuals,
                refresh_selection_visuals,
            )
                .chain()
                .in_set(crate::plugins::UiTouchActionSet)
                .run_if(in_state(AppState::Menu)),
        )
        .add_systems(
            Update,
            launcher_touch_scroll.in_set(crate::plugins::UiTouchGestureSet),
        )
        .add_systems(
            Update,
            (
                launcher_ui_scroll,
                adapt_settings_layout,
                crate::plugins::settings_ui::adapt_settings_navigation,
                adapt_launcher_touch_targets,
                adapt_runtime_ui_layout,
            ),
        )
        .add_systems(
            Update,
            (
                update_hud_stats,
                update_mode_hud,
                update_match_result_banner.after(crate::plugins::FighterFrameSet::PoseResolve),
                update_escort_flag_hud,
                update_gun_crosshair,
            ),
        )
        .add_systems(
            Update,
            (
                toggle_pause.after(crate::plugins::FighterFrameSet::Movement),
                pause_resume_button,
                pause_settings_button,
                pause_back_to_menu_button,
                pause_clock,
                result_back_to_menu_button,
                result_retry_button,
                menu_room_button,
                menu_record_button,
                menu_replay_button,
                menu_mrc_button,
                room_add_npc_button,
                room_remove_npc_button,
                room_start_button,
                room_back_button,
                update_room_members,
                replay_pause_button,
                update_replay_hud,
            )
                .in_set(crate::plugins::UiTouchActionSet),
        )
        .add_systems(
            Update,
            exit_game_buttons.in_set(crate::plugins::UiTouchActionSet),
        )
        .add_systems(Update, replay_timeline_scrub)
        .add_systems(Update, cycle_radar_range.before(update_radar))
        .add_systems(
            Update,
            (
                adapt_net_room_layout.after(adapt_runtime_ui_layout),
                update_net_room_cards,
            ),
        )
        .add_systems(
            Update,
            (
                toggle_radar,
                prepare_radar_terrain,
                update_radar,
                update_collapsed_radar,
            )
                .chain()
                .after(crate::plugins::FighterFrameSet::CameraFinalize)
                .run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            Update,
            replay_space_toggle
                .before(crate::plugins::FighterFrameSet::Movement)
                .run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            Update,
            (update_fighter_nameplates, update_fighter_vital_bars)
                .chain()
                .after(crate::plugins::FighterFrameSet::CameraFinalize)
                .run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            Update,
            menu_auto_record_button.in_set(crate::plugins::UiTouchActionSet),
        )
        .init_resource::<crate::plugins::settings_ui::SettingsSliderDirty>()
        .add_systems(
            Update,
            crate::plugins::settings_ui::settings_slider_changes
                .after(settings_range_down_buttons)
                .after(settings_range_up_buttons)
                .after(settings_reset_button)
                .before(refresh_settings_form_labels)
                .in_set(crate::plugins::UiTouchActionSet),
        )
        .add_systems(
            Update,
            (
                crate::plugins::settings_ui::settings_category_buttons
                    .before(key_capture_button_press)
                    .before(key_capture),
                settings_bool_buttons,
                settings_select_buttons,
                settings_range_down_buttons,
                settings_range_up_buttons,
                settings_reset_button,
                refresh_settings_form_labels,
                key_capture_button_press,
                key_capture,
                update_key_capture_buttons,
            )
                .in_set(crate::plugins::UiTouchActionSet)
                .run_if(in_menu_or_settings),
        )
        .add_systems(
            Update,
            settings_back_button
                .in_set(crate::plugins::UiTouchActionSet)
                .run_if(in_state(AppState::Settings)),
        )
        .add_systems(
            Update,
            settings_escape_back
                .after(key_capture)
                .run_if(in_state(AppState::Settings)),
        )
        .add_systems(Update, (update_dialogue_hud, update_gameplay_pause));
    }
}

fn in_menu_or_settings(state: Res<State<AppState>>) -> bool {
    matches!(*state.get(), AppState::Menu | AppState::Settings)
}

fn despawn_root<T: Component>(mut commands: Commands, query: Query<Entity, With<T>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

fn setup_menu(
    mut commands: Commands,
    mut fonts: ResMut<Assets<Font>>,
    ui_font: Option<Res<UiFont>>,
    request: Res<LaunchRequest>,
    settings: Res<Settings>,
    keymap: Res<Keymap>,
    custom: Res<crate::plugins::stage_plugin::CustomStages>,
    screen: Res<MenuScreen>,
    tab: Res<StageListTab>,
) {
    let font = if let Some(ui_font) = ui_font {
        ui_font.0.clone()
    } else {
        let loaded = UiFont::load(&mut fonts);
        let handle = loaded.0.clone();
        commands.insert_resource(loaded);
        handle
    };
    setup_launcher(
        commands, font, request, settings, keymap, custom, screen, tab,
    );
}

#[derive(Component)]
struct ReplayHudText;

#[derive(Component)]
struct DialogueHudText;

#[derive(Component)]
struct GunAimCrosshair;

#[derive(Resource)]
pub(crate) struct RadarState {
    visible: bool,
    range: usize,
    pub(crate) tap_circle: Option<(Vec2, f32)>,
}

impl RadarState {
    fn cycle_range(&mut self) {
        if !self.visible {
            self.visible = true;
            self.range = 0;
        } else if self.range == 2 {
            self.visible = false;
        } else {
            self.range += 1;
        }
    }

    fn world_radius(&self) -> f32 {
        RADAR_WORLD_RADIUS * [1.0, 2.0, 4.0][self.range]
    }

    pub(crate) fn contains_point(&self, point: Vec2) -> bool {
        self.tap_circle
            .is_some_and(|(center, radius)| point.distance_squared(center) <= radius * radius)
    }
}

impl Default for RadarState {
    fn default() -> Self {
        Self {
            visible: true,
            range: 0,
            tap_circle: None,
        }
    }
}

#[derive(Resource, Default)]
struct RadarTerrain {
    bounds: Option<(f32, f32, f32, f32)>,
    image: Option<Handle<Image>>,
    texture_size: Vec2,
}

#[derive(Resource, Default)]
struct RadarAssets {
    plate: Option<Handle<Image>>,
    arrow: Option<Handle<Image>>,
    circle: Option<Handle<Image>>,
    star: Option<Handle<Image>>,
}

#[derive(Component)]
struct RadarRoot;

#[derive(Component)]
struct CollapsedRadarRoot;

#[derive(Component, Debug, Clone, Copy)]
struct RadarMarker {
    fighter: Entity,
}

#[derive(Component)]
struct RadarThreatGlow {
    sector: usize,
    level: f32,
}

#[derive(Component, Debug, Clone, Copy)]
struct FighterNameplate {
    fighter: Entity,
    text: Entity,
}

#[derive(Component)]
struct FighterNameText;

#[derive(Component)]
struct FighterVitalBar {
    fighter: Entity,
    qi: bool,
    fill: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RadarMarkerKind {
    Arrow,
    Circle,
    Star,
}

#[derive(Component)]
struct ReplayPauseButton;

#[derive(Component)]
struct ReplayTimelineTrack;

#[derive(Component)]
struct ReplayTimelineFill;

#[derive(Component)]
struct GameplayHudRoot;

#[derive(Component)]
struct HudDetailText;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum HudVital {
    Health,
    Qi,
}
#[derive(Component)]
struct HudVitalFill(HudVital);
#[derive(Component)]
struct HudMeterTrack;
#[derive(Component)]
struct HudLoadoutRow;
#[derive(Component)]
enum HudLoadoutText {
    Active,
    Spare,
}
#[derive(Component)]
struct DialoguePanelRoot;
#[derive(Component)]
struct DialogueSpeakerText;
#[derive(Component)]
struct HudFixedFont(f32);

#[derive(Resource, Default)]
pub(crate) struct DialoguePresentation {
    source: String,
    speaker: String,
    pages: Vec<String>,
    page: usize,
    page_left: f32,
    page_duration: f32,
    previous_timer: f32,
    capacity: (usize, usize),
}

impl DialoguePresentation {
    pub(crate) fn speaker(&self) -> Option<&str> {
        (!self.source.is_empty()).then_some(self.speaker.as_str())
    }
}

#[derive(Component)]
struct ModeObjectiveText;

#[derive(Component)]
struct MatchResultBanner;

#[derive(Component)]
struct EscortFlagHudIcon;

#[derive(Component)]
struct ResponsiveOverlayRoot;

#[derive(Component)]
struct ResponsiveOverlaySurface;

#[derive(Component)]
struct ResponsiveOverlayTitle(f32);

#[derive(Component)]
pub(crate) struct ResponsiveMenuButton;

#[derive(Component)]
struct ResponsiveButtonPalette(Color);

#[derive(Resource, Default)]
struct ReplayScrubState {
    active: bool,
    resume_mrc: bool,
    resume_player: bool,
}

fn setup_playing_hud(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut radar_assets: ResMut<RadarAssets>,
    ui_font: Res<UiFont>,
    player: Res<crate::plugins::replay_plugin::ReplayPlayer>,
    mrc: Res<crate::plugins::replay_plugin::MrcPlayback>,
) {
    let font = ui_font.0.clone();
    let workdir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let atarget = load_ui_texture_bytes(&workdir, "ATarget.jpg")
        .and_then(|bytes| crate::plugins::pak_textures::image_handle(&mut images, &bytes));
    let flag01 = load_ui_texture_bytes(&workdir, "FLAG01.jpg")
        .and_then(|bytes| crate::plugins::pak_textures::image_handle(&mut images, &bytes));
    radar_assets.plate = load_ui_texture_bytes(&workdir, "plate.jpg")
        .and_then(|bytes| crate::plugins::pak_textures::image_handle(&mut images, &bytes));
    if let Some(handle) = radar_assets.plate.as_ref() {
        if let Some(mut image) = images.get_mut(handle) {
            image.sampler = bevy::image::ImageSampler::nearest();
        }
    }
    radar_assets.arrow = load_ui_colorkey_handle(&workdir, "arrow.bmp", &mut images);
    radar_assets.circle = load_ui_colorkey_handle(&workdir, "circle.bmp", &mut images);
    radar_assets.star = load_ui_colorkey_handle(&workdir, "star.bmp", &mut images);
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(12.0),
                left: Val::Px(12.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                width: Val::Px(224.0),
                padding: UiRect::ZERO,
                border_radius: BorderRadius::all(Val::Px(3.0)),
                ..default()
            },
            PlayingRoot,
            GameplayHudRoot,
        ))
        .id();
    let loadout_row = commands
        .spawn((
            Node {
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                height: Val::Px(20.0),
                border: UiRect::left(Val::Px(2.0)),
                padding: UiRect::left(Val::Px(6.0)),
                ..default()
            },
            BorderColor::all(theme::METAL),
            BackgroundColor(theme::HUD_SURFACE),
            HudLoadoutRow,
        ))
        .id();
    let flag_icon = commands
        .spawn((
            Node {
                width: Val::Px(16.0),
                height: Val::Px(16.0),
                display: Display::None,
                ..default()
            },
            flag01
                .map(ImageNode::new)
                .unwrap_or_else(|| ImageNode::solid_color(Color::srgb(0.95, 0.75, 0.1))),
            Visibility::Hidden,
            EscortFlagHudIcon,
        ))
        .id();
    let loadout = commands
        .spawn((
            Text::new("—"),
            TextFont {
                font: font.clone().into(),
                font_size: 13.0.into(),
                ..default()
            },
            TextColor(theme::METAL),
            bevy::ui::widget::TextShadow {
                offset: Vec2::splat(1.0),
                color: Color::BLACK,
            },
            HudLoadoutText::Active,
            HudFixedFont(13.0),
        ))
        .id();
    commands
        .entity(loadout_row)
        .add_children(&[flag_icon, loadout]);
    let spare = commands
        .spawn((
            Text::new("备 · —"),
            TextFont {
                font: font.clone().into(),
                font_size: 11.0.into(),
                ..default()
            },
            TextColor(theme::MUTED),
            bevy::ui::widget::TextShadow {
                offset: Vec2::splat(1.0),
                color: Color::BLACK,
            },
            HudLoadoutText::Spare,
            HudFixedFont(11.0),
        ))
        .id();
    commands.entity(loadout_row).add_child(spare);

    for (kind, color, height) in [
        (HudVital::Health, theme::HEALTH, 20.0),
        (HudVital::Qi, theme::ANGER, 20.0),
    ] {
        let track = commands
            .spawn((
                Node {
                    height: Val::Px(height),
                    width: Val::Percent(100.0),
                    overflow: Overflow::clip(),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    border_radius: BorderRadius::all(Val::Px(3.0)),
                    ..default()
                },
                BackgroundColor(Color::NONE),
                HudMeterTrack,
            ))
            .id();
        let fill = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(0.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(color),
                HudVitalFill(kind),
            ))
            .id();
        let label = commands
            .spawn((
                Node::default(),
                Text::new("—"),
                TextFont {
                    font: font.clone().into(),
                    font_size: 13.0.into(),
                    ..default()
                },
                TextColor(Color::WHITE),
                bevy::ui::widget::TextShadow {
                    offset: Vec2::splat(1.0),
                    color: Color::BLACK,
                },
                kind,
                HudFixedFont(13.0),
            ))
            .id();
        let rail = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(theme::METAL.with_alpha(0.22)),
            ))
            .id();
        commands.entity(track).add_children(&[rail, fill, label]);
        commands.entity(root).add_child(track);
    }
    commands.entity(root).add_child(loadout_row);
    let objective = commands
        .spawn((
            Node {
                padding: UiRect::horizontal(Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(theme::HUD_SURFACE),
            Text::new(""),
            TextFont {
                font: font.clone().into(),
                font_size: 11.0.into(),
                ..default()
            },
            TextColor(theme::MUTED),
            bevy::ui::widget::TextShadow {
                offset: Vec2::splat(1.0),
                color: Color::BLACK,
            },
            ModeObjectiveText,
            HudFixedFont(11.0),
        ))
        .id();
    commands.entity(root).add_child(objective);

    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Percent(25.0),
            left: Val::Percent(10.0),
            width: Val::Percent(80.0),
            display: Display::None,
            ..default()
        },
        Text::new(""),
        TextFont {
            font: font.clone().into(),
            font_size: 28.0.into(),
            ..default()
        },
        TextLayout::new(Justify::Center, LineBreak::WordOrCharacter),
        TextColor(theme::TEXT),
        bevy::ui::widget::TextShadow {
            offset: Vec2::splat(2.0),
            color: Color::BLACK,
        },
        PlayingRoot,
        MatchResultBanner,
    ));

    let dialogue_root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(260.0),
                top: Val::Px(110.0),
                width: Val::Px(420.0),
                height: Val::Auto,
                padding: UiRect::axes(Val::Px(16.0), Val::Px(12.0)),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(12.0),
                border: UiRect::left(Val::Px(2.0)),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                ..default()
            },
            BackgroundColor(Color::NONE),
            BackgroundGradient(vec![LinearGradient::new(
                LinearGradient::TO_RIGHT,
                vec![
                    ColorStop::new(theme::HUD_SURFACE.with_alpha(0.88), Val::Percent(0.0)),
                    ColorStop::new(theme::HUD_SURFACE.with_alpha(0.64), Val::Percent(65.0)),
                    ColorStop::new(theme::HUD_SURFACE.with_alpha(0.0), Val::Percent(100.0)),
                ],
            )
            .into()]),
            Visibility::Hidden,
            BorderColor::all(theme::ACCENT.with_alpha(0.8)),
            DialoguePanelRoot,
            PlayingRoot,
        ))
        .id();
    let speaker = commands
        .spawn((
            Text::new(""),
            TextFont {
                font: font.clone().into(),
                font_size: 12.0.into(),
                ..default()
            },
            TextColor(theme::METAL),
            DialogueSpeakerText,
            HudFixedFont(12.0),
        ))
        .id();
    let dialogue = commands
        .spawn((
            Text::new(""),
            TextFont {
                font: font.clone().into(),
                font_size: 15.0.into(),
                ..default()
            },
            TextLayout::new(Justify::Left, bevy::text::LineBreak::WordOrCharacter),
            TextColor(Color::srgb(0.95, 0.94, 0.91)),
            DialogueHudText,
            HudFixedFont(15.0),
        ))
        .id();
    let portrait = commands.spawn(super::dialogue_portrait::window_node()).id();
    let copy = commands
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            min_width: Val::Px(0.0),
            flex_grow: 1.0,
            row_gap: Val::Px(6.0),
            ..default()
        })
        .id();
    commands.entity(copy).add_children(&[speaker, dialogue]);
    commands
        .entity(dialogue_root)
        .add_children(&[portrait, copy]);

    commands
        .spawn((
            Button,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(16.0),
                top: Val::Px(12.0),
                width: Val::Px(64.0),
                height: Val::Px(28.0),
                display: if cfg!(target_arch = "wasm32") {
                    Display::None
                } else {
                    Display::Flex
                },
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                column_gap: Val::Px(5.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                ..default()
            },
            BackgroundColor(theme::HUD_SURFACE),
            BorderColor::all(theme::LINE),
            PlayingRoot,
            GameplayPauseButton,
        ))
        .with_children(|button| {
            for _ in 0..2 {
                button.spawn((
                    Node {
                        width: Val::Px(3.0),
                        height: Val::Px(16.0),
                        ..default()
                    },
                    BackgroundColor(theme::TEXT),
                ));
            }
        });

    let crosshair = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                margin: UiRect::new(Val::Px(-16.0), Val::ZERO, Val::Px(-16.0), Val::ZERO),
                width: Val::Px(32.0),
                height: Val::Px(32.0),
                ..default()
            },
            Visibility::Hidden,
            GunAimCrosshair,
            PlayingRoot,
        ))
        .id();
    let pieces = [
        (0.0, 15.0, 10.0, 2.0),
        (22.0, 15.0, 10.0, 2.0),
        (15.0, 0.0, 2.0, 10.0),
        (15.0, 22.0, 2.0, 10.0),
    ];
    for (left, top, width, height) in pieces {
        let piece = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(left),
                    top: Val::Px(top),
                    width: Val::Px(width),
                    height: Val::Px(height),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.96, 0.94, 0.82)),
            ))
            .id();
        commands.entity(crosshair).add_child(piece);
    }
    let dot_node = Node {
        position_type: PositionType::Absolute,
        left: Val::Px(14.0),
        top: Val::Px(14.0),
        width: Val::Px(4.0),
        height: Val::Px(4.0),
        ..default()
    };
    let dot = if let Some(texture) = atarget {
        commands.spawn((dot_node, ImageNode::new(texture))).id()
    } else {
        commands
            .spawn((dot_node, BackgroundColor(Color::srgb(0.96, 0.94, 0.82))))
            .id()
    };
    commands.entity(crosshair).add_child(dot);

    let radar = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(10.0),
                top: Val::Px(10.0),
                width: Val::Px(128.0),
                height: Val::Px(128.0),
                border_radius: BorderRadius::all(Val::Percent(50.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.025, 0.035, 0.72)),
            Visibility::Hidden,
            Outline {
                width: Val::Px(1.0),
                offset: Val::Px(2.0),
                color: theme::METAL.with_alpha(0.6),
            },
            RadarRoot,
            ZIndex(81),
            PlayingRoot,
        ))
        .id();
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(48.0),
                height: Val::Px(48.0),
                border_radius: BorderRadius::all(Val::Percent(50.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(theme::HUD_SURFACE),
            Visibility::Hidden,
            ZIndex(81),
            CollapsedRadarRoot,
            PlayingRoot,
        ))
        .with_children(|button| {
            button.spawn((
                Text::new("雷达"),
                TextFont {
                    font: font.clone().into(),
                    font_size: 13.0.into(),
                    ..default()
                },
                TextColor(theme::TEXT),
                HudFixedFont(13.0),
            ));
        });
    if let Some(plate) = radar_assets.plate.clone() {
        commands.entity(radar).insert(ImageNode {
            color: Color::srgba(0.2, 0.2, 0.2, 0.5),
            image: plate,
            ..default()
        });
    }

    if player.active || mrc.active {
        let replay_text = commands
            .spawn((
                Text::new("回放中"),
                TextFont {
                    font: font.clone().into(),
                    font_size: 16.0.into(),
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.9, 0.6)),
                ReplayHudText,
            ))
            .id();
        commands.entity(root).add_child(replay_text);
        let pause = commands
            .spawn((
                Button,
                Node {
                    width: Val::Px(120.0),
                    min_height: Val::Px(26.0),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                BackgroundColor(Color::srgb(0.2, 0.3, 0.25)),
                Interaction::default(),
                ReplayPauseButton,
            ))
            .id();
        let label = text(
            &mut commands,
            "暂停",
            14.0,
            Color::WHITE,
            Some(font.clone()),
        );
        commands.entity(pause).add_child(label);
        commands.entity(root).add_child(pause);
        if mrc.active {
            let track = commands
                .spawn((
                    Button,
                    Node {
                        width: Val::Px(360.0),
                        height: Val::Px(16.0),
                        border: UiRect::all(Val::Px(1.0)),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    BorderColor::all(Color::srgba(0.75, 0.82, 0.82, 0.45)),
                    BackgroundColor(Color::srgba(0.04, 0.06, 0.08, 0.88)),
                    Interaction::default(),
                    bevy::ui::RelativeCursorPosition::default(),
                    ReplayTimelineTrack,
                ))
                .id();
            let fill = commands
                .spawn((
                    Node {
                        width: Val::Percent(0.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.72, 0.22, 0.16, 0.92)),
                    ReplayTimelineFill,
                ))
                .id();
            commands.entity(track).add_child(fill);
            commands.entity(root).add_child(track);
            let hint = text(
                &mut commands,
                "拖动时间轴跳转 · 空格暂停/继续",
                12.0,
                Color::srgb(0.7, 0.75, 0.76),
                Some(font),
            );
            commands.entity(root).add_child(hint);
        }
    }
}

pub(crate) fn load_ui_texture_bytes(workdir: &std::path::Path, name: &str) -> Option<Vec<u8>> {
    crate::plugins::pak_textures::resolve_runtime_texture(&["uitexture"], name).or_else(|| {
        crate::plugins::audio_plugin::resource_roots(workdir)
            .into_iter()
            .find_map(|root| {
                let bytes = crate::resource_io::read(root.join("UITexture.pak")).ok()?;
                let pak = meteor_asset::pak::PakArchive::from_bytes(&bytes).ok()?;
                crate::plugins::pak_textures::resolve_pak_texture(&[&pak], name)
            })
    })
}

fn load_ui_colorkey_handle(
    workdir: &std::path::Path,
    name: &str,
    images: &mut Assets<Image>,
) -> Option<Handle<Image>> {
    let bytes = load_ui_texture_bytes(workdir, name)?;
    let mut decoded = meteor_asset::texture::decode_image(&bytes).ok()?;
    for pixel in decoded.rgba.chunks_exact_mut(4) {
        let alpha = pixel[0].max(pixel[1]).max(pixel[2]);
        pixel[0] = 255;
        pixel[1] = 255;
        pixel[2] = 255;
        pixel[3] = alpha;
    }
    let handle = crate::plugins::pak_textures::image_from_decoded(images, &decoded);
    if let Some(mut image) = images.get_mut(&handle) {
        image.sampler = bevy::image::ImageSampler::nearest();
    }
    Some(handle)
}

#[derive(Resource, Default)]
pub struct DialogueState {
    pub text: Option<String>,
    pub timer: f32,
}

fn update_gun_crosshair(
    aims: Query<&GunAimState, With<crate::plugins::stage_plugin::LocalFighter>>,
    mut crosshair: Query<&mut Visibility, With<GunAimCrosshair>>,
) {
    let visible = aims.single().is_ok_and(|aim| aim.aiming);
    for mut visibility in &mut crosshair {
        *visibility = if visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

fn reset_radar(mut radar: ResMut<RadarState>, mut terrain: ResMut<RadarTerrain>) {
    *radar = RadarState::default();
    *terrain = RadarTerrain::default();
}

fn prepare_radar_terrain(
    mut commands: Commands,
    collider: Res<crate::plugins::stage_collider::StageCollider>,
    mut terrain: ResMut<RadarTerrain>,
    mut images: ResMut<Assets<Image>>,
    roots: Query<Entity, With<RadarRoot>>,
) {
    if terrain.image.is_some() {
        return;
    }
    let Ok(root) = roots.single() else {
        return;
    };
    let Some(mesh) = collider.mesh.as_ref() else {
        return;
    };
    let Some(bounds) = radar_bounds(&mesh.positions) else {
        return;
    };

    let bounds = (
        bounds.0 - RADAR_WORLD_RADIUS * 4.0,
        bounds.1 + RADAR_WORLD_RADIUS * 4.0,
        bounds.2 - RADAR_WORLD_RADIUS * 4.0,
        bounds.3 + RADAR_WORLD_RADIUS * 4.0,
    );
    let decoded = super::radar_map::terrain_image(mesh, bounds);
    terrain.texture_size = Vec2::new(decoded.width as f32, decoded.height as f32);
    let handle = crate::plugins::pak_textures::image_from_decoded(&mut images, &decoded);
    if let Some(mut image) = images.get_mut(&handle) {
        image.sampler = bevy::image::ImageSampler::linear();
        image.texture_descriptor.format =
            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb;
    }
    commands.entity(root).insert(ImageNode::new(handle.clone()));
    let glow = super::radar_map::threat_glow_image();
    let glow = crate::plugins::pak_textures::image_from_decoded(&mut images, &glow);
    for sector in 0..RADAR_THREAT_SECTORS {
        let child = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                UiTransform::from_rotation(Rot2::radians(
                    sector as f32 * std::f32::consts::TAU / RADAR_THREAT_SECTORS as f32,
                )),
                ImageNode {
                    image: glow.clone(),
                    color: Color::srgba(1.0, 0.08, 0.04, 0.0),
                    ..default()
                },
                RadarThreatGlow { sector, level: 0.0 },
            ))
            .id();
        commands.entity(root).add_child(child);
    }
    terrain.bounds = Some(bounds);
    terrain.image = Some(handle);
}

fn toggle_radar(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keymap: Res<Keymap>,
    performance: Option<Res<crate::plugins::stage_plugin::PlayerPerformance>>,
    mut radar: ResMut<RadarState>,
) {
    if !performance.is_some_and(|performance| performance.input_blocked)
        && keymap.radar.just_pressed(&keys, &mouse)
    {
        radar.cycle_range();
    }
}

fn cycle_radar_range(
    mut touches: MessageReader<bevy::input::touch::TouchInput>,
    mut starts: Local<std::collections::HashMap<u64, Vec2>>,
    state: Option<Res<State<AppState>>>,
    mut radar: ResMut<RadarState>,
    performance: Option<Res<crate::plugins::stage_plugin::PlayerPerformance>>,
    debug: Option<Res<crate::plugins::debug_plugin::DebugConsoleState>>,
) {
    use bevy::input::touch::TouchPhase;
    if state.is_some_and(|s| *s.get() != AppState::Playing)
        || performance.is_some_and(|p| p.input_blocked)
        || debug.is_some_and(|d| d.captures_input())
    {
        starts.clear();
        touches.read().for_each(drop);
        return;
    }
    for event in touches.read() {
        match event.phase {
            TouchPhase::Started => {
                if radar.contains_point(event.position) {
                    starts.insert(event.id, event.position);
                }
            }
            TouchPhase::Moved => {
                if starts
                    .get(&event.id)
                    .is_some_and(|start| start.distance(event.position) > 12.0)
                {
                    starts.remove(&event.id);
                }
            }
            TouchPhase::Ended => {
                if starts
                    .remove(&event.id)
                    .is_some_and(|start| start.distance(event.position) <= 12.0)
                    && radar.contains_point(event.position)
                {
                    radar.cycle_range();
                }
            }
            TouchPhase::Canceled => {
                starts.remove(&event.id);
            }
        }
    }
}

pub(crate) fn radar_screen_geometry(
    window: &Window,
    safe: SafeAreaInsets,
    resolved: Option<&super::mobile_controls_plugin::ResolvedMobileLayout>,
) -> (Vec2, f32) {
    let viewport = Vec2::new(window.width(), window.height());
    let mobile = responsive_ui_layout(viewport.x, viewport.y, cfg!(target_os = "android")).mobile;
    if let Some(geometry) = resolved.and_then(|layout| layout.radar_geometry(viewport)) {
        return geometry;
    }
    let hud = super::combat_hud_layout::CombatHudLayout::new(viewport.x, viewport.y, safe, mobile);
    (
        hud.map_origin + Vec2::splat(hud.map_diameter * 0.5),
        hud.map_diameter,
    )
}

fn update_collapsed_radar(
    windows: Query<&Window>,
    touch_controls: Option<Res<super::mobile_controls_plugin::MobileControlsConfig>>,
    safe: Option<Res<SafeAreaInsets>>,
    resolved: Option<Res<super::mobile_controls_plugin::ResolvedMobileLayout>>,
    scale: Option<Res<crate::plugins::ui_scale_plugin::EffectiveUiScale>>,
    terrain: Res<RadarTerrain>,
    mut radar: ResMut<RadarState>,
    mut buttons: Query<(&mut Node, &mut Visibility), With<CollapsedRadarRoot>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let safe = safe.as_deref().copied().unwrap_or_default();
    let (center, diameter) = radar_screen_geometry(window, safe, resolved.as_deref());
    let mobile =
        responsive_ui_layout(window.width(), window.height(), cfg!(target_os = "android")).mobile;
    let custom = resolved.as_ref().is_some_and(|layout| {
        layout
            .radar_geometry(Vec2::new(window.width(), window.height()))
            .is_some()
    });
    let base = super::combat_hud_layout::CombatHudLayout::new(
        window.width(),
        window.height(),
        safe,
        mobile || custom,
    )
    .map_diameter;
    let button_size = (48.0 * diameter / base).max(48.0);
    let unit = scale.as_ref().map_or(1.0, |scale| scale.0.max(0.05));
    let show_restore_button = touch_controls.is_some_and(|config| config.visible);
    if !radar.visible {
        radar.tap_circle = None;
    }
    for (mut node, mut visibility) in &mut buttons {
        *visibility = if show_restore_button && !radar.visible && terrain.bounds.is_some() {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        node.left = Val::Px((center.x - button_size * 0.5) / unit);
        node.top = Val::Px((center.y - button_size * 0.5) / unit);
        node.width = Val::Px(button_size / unit);
        node.height = Val::Px(button_size / unit);
        if *visibility == Visibility::Visible {
            radar.tap_circle = Some((center, button_size * 0.5));
        }
    }
}

fn radar_bounds(positions: &[[f32; 3]]) -> Option<(f32, f32, f32, f32)> {
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_z = f32::INFINITY;
    let mut max_z = f32::NEG_INFINITY;
    for position in positions {
        min_x = min_x.min(position[0]);
        max_x = max_x.max(position[0]);
        min_z = min_z.min(position[2]);
        max_z = max_z.max(position[2]);
    }
    ((max_x - min_x) > 1e-6 && (max_z - min_z) > 1e-6).then_some((min_x, max_x, min_z, max_z))
}

const RADAR_WORLD_RADIUS: f32 = 300.0 * meteor_core::MODEL_SCALE;
const RADAR_MARKER_HALF: f32 = 8.0;
const RADAR_THREAT_SECTORS: usize = 24;

fn radar_view_rotation(camera_rotation: Quat) -> Rot2 {
    let forward = camera_rotation * Vec3::NEG_Z;
    Rot2::radians(-forward.x.atan2(forward.z))
}

fn radar_is_hostile(local_team: i32, other_team: i32) -> bool {
    use meteor_combat::hit_resolve::team_mask;
    team_mask(local_team) & team_mask(other_team) == 0
}

fn add_radar_threat(strengths: &mut [f32; RADAR_THREAT_SECTORS], offset: Vec3) {
    let angle = offset.x.atan2(offset.z).rem_euclid(std::f32::consts::TAU);
    let sector = angle / std::f32::consts::TAU * RADAR_THREAT_SECTORS as f32;
    let first = sector.floor() as usize % RADAR_THREAT_SECTORS;
    let weight = sector.fract();

    strengths[first] = strengths[first].max(1.0 - weight);
    let second = (first + 1) % RADAR_THREAT_SECTORS;
    strengths[second] = strengths[second].max(weight);
}

fn radar_texture_rect(
    position: Vec3,
    bounds: (f32, f32, f32, f32),
    texture_size: Vec2,
    radius: f32,
) -> Rect {
    let (min_x, max_x, min_z, max_z) = bounds;
    let pixel_scale = (texture_size - Vec2::ONE) / Vec2::new(max_x - min_x, max_z - min_z);
    let center = Vec2::new(position.x - min_x, max_z - position.z) * pixel_scale;
    Rect::from_center_size(center, pixel_scale * (radius * 2.0))
}

fn map_world_to_local_radar(
    position: Vec3,
    origin: Vec3,
    diameter: f32,
    radius: f32,
) -> Option<Vec2> {
    let delta = Vec2::new(position.x - origin.x, origin.z - position.z) * (diameter * 0.5 / radius);

    let limit = (diameter * 0.5 - RADAR_MARKER_HALF * std::f32::consts::SQRT_2 - 2.0).max(0.0);
    (delta.length_squared() <= limit * limit).then_some(Vec2::splat(diameter * 0.5) + delta)
}

fn radar_view_arrow_rotation(map_rotation: Rot2) -> Rot2 {
    map_rotation.inverse()
}

fn radar_marker_style(
    kind: RadarMarkerKind,
    assets: &RadarAssets,
) -> (Option<Handle<Image>>, Color) {
    match kind {
        RadarMarkerKind::Arrow => (assets.arrow.clone(), Color::WHITE),
        RadarMarkerKind::Circle => (assets.circle.clone(), Color::srgb(0.35, 0.9, 0.45)),
        RadarMarkerKind::Star => (assets.star.clone(), Color::srgb(1.0, 0.82, 0.2)),
    }
}

fn set_radar_marker_image(image: &mut ImageNode, kind: RadarMarkerKind, assets: &RadarAssets) {
    let (texture, fallback) = radar_marker_style(kind, assets);
    if let Some(texture) = texture {
        image.image = texture;
        image.color = Color::WHITE;
    } else {
        *image = ImageNode::solid_color(fallback);
    }

    image.flip_y = kind == RadarMarkerKind::Arrow;
}

fn set_radar_contact_image(
    image: &mut ImageNode,
    kind: RadarMarkerKind,
    hostile: bool,
    assets: &RadarAssets,
) {
    if kind == RadarMarkerKind::Arrow {
        set_radar_marker_image(image, kind, assets);
    } else {
        let color = if hostile {
            Color::srgb(1.0, 0.18, 0.13)
        } else {
            Color::srgb(0.2, 1.0, 0.4)
        };
        *image = ImageNode::solid_color(color);
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_radar(
    mut commands: Commands,
    mut radar: ResMut<RadarState>,
    resolved: Option<Res<super::mobile_controls_plugin::ResolvedMobileLayout>>,
    safe_area: Option<Res<SafeAreaInsets>>,
    assets: Res<RadarAssets>,
    terrain: Res<RadarTerrain>,
    net: Res<crate::plugins::network_plugin::NetState>,
    window: Query<&Window>,
    ui_scale: Option<Res<crate::plugins::ui_scale_plugin::EffectiveUiScale>>,
    cameras: Query<&Transform, With<crate::plugins::camera_plugin::GameplayCamera>>,
    time: Option<Res<Time>>,
    mut glows: Query<
        (&mut RadarThreatGlow, &mut ImageNode),
        (Without<RadarRoot>, Without<RadarMarker>),
    >,
    mut roots: Query<
        (
            Entity,
            &mut Node,
            &mut Visibility,
            &mut ImageNode,
            &mut UiTransform,
        ),
        (With<RadarRoot>, Without<RadarMarker>, Without<Fighter>),
    >,
    fighters: Query<
        (
            Entity,
            &Fighter,
            &Transform,
            &Health,
            Option<&crate::plugins::stage_plugin::LocalFighter>,
            Option<&crate::plugins::stage_plugin::MissionNpc>,
            Option<&crate::plugins::network_plugin::RemoteFighter>,
            Option<&crate::components::fighter::EscortFlagCarrier>,
            Option<&Visibility>,
        ),
        (Without<RadarMarker>, Without<RadarRoot>),
    >,
    mut markers: Query<
        (
            Entity,
            &RadarMarker,
            &mut Node,
            &mut UiTransform,
            &mut ImageNode,
        ),
        (With<RadarMarker>, Without<RadarRoot>, Without<Fighter>),
    >,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(6);
    radar.tap_circle = None;
    let Ok((root_entity, mut root_node, mut root_visibility, mut map_image, mut map_rotation)) =
        roots.single_mut()
    else {
        return;
    };
    let Some(bounds) = terrain.bounds else {
        *root_visibility = Visibility::Hidden;
        return;
    };
    if !radar.visible {
        *root_visibility = Visibility::Hidden;
        return;
    }
    *root_visibility = Visibility::Visible;
    let geometry = window.single().ok().map(|window| {
        radar_screen_geometry(
            window,
            safe_area.as_deref().copied().unwrap_or_default(),
            resolved.as_deref(),
        )
    });
    if let Some((center, diameter)) = geometry {
        radar.tap_circle = Some((center, diameter * 0.5));
    }
    let max_size = geometry.map_or(152.0, |(_, diameter)| diameter)
        / ui_scale.as_ref().map_or(1.0, |scale| scale.0.max(0.05));
    let size = Vec2::splat(max_size);
    root_node.width = Val::Px(size.x);
    root_node.height = Val::Px(size.y);
    root_node.border_radius = BorderRadius::all(Val::Percent(50.0));

    if let Ok(camera) = cameras.single() {
        map_rotation.rotation = radar_view_rotation(camera.rotation);
    }

    let Some((local_entity, local_team, local_position)) = fighters.iter().find_map(
        |(entity, _fighter, transform, health, local, _mission, _remote, _carrier, visibility)| {
            (local.is_some()
                && health.current > 0.0
                && !visibility.is_some_and(|visibility| *visibility == Visibility::Hidden))
            .then_some((
                entity,
                crate::plugins::combat_plugin::fighter_combat_team(&net, true, None, None),
                transform.translation,
            ))
        },
    ) else {
        *root_visibility = Visibility::Hidden;
        radar.tap_circle = None;
        return;
    };

    map_image.rect = Some(radar_texture_rect(
        local_position,
        bounds,
        terrain.texture_size,
        radar.world_radius(),
    ));

    let mut threats = [0.0; RADAR_THREAT_SECTORS];
    let mut desired = std::collections::HashMap::new();
    for (entity, _fighter, transform, health, local, mission, remote, carrier, visibility) in
        &fighters
    {
        if health.current <= 0.0
            || visibility.is_some_and(|visibility| *visibility == Visibility::Hidden)
        {
            continue;
        }
        let team = crate::plugins::combat_plugin::fighter_combat_team(
            &net,
            local.is_some(),
            remote,
            mission,
        );
        let hostile = entity != local_entity && radar_is_hostile(local_team, team);
        let kind = if entity == local_entity {
            RadarMarkerKind::Arrow
        } else if mission.is_some_and(|mission| mission.is_vip)
            || remote.is_some_and(|remote| net.radar_star_players.contains(&remote.player_id))
            || carrier.is_some()
        {
            RadarMarkerKind::Star
        } else {
            RadarMarkerKind::Circle
        };
        let Some(point) = map_world_to_local_radar(
            transform.translation,
            local_position,
            max_size,
            radar.world_radius(),
        ) else {
            if hostile {
                add_radar_threat(&mut threats, transform.translation - local_position);
            }
            continue;
        };
        desired.insert(
            entity,
            (
                point,
                radar_view_arrow_rotation(map_rotation.rotation),
                kind,
                hostile,
            ),
        );
    }

    let elapsed = time.as_ref().map_or(0.0, |time| time.elapsed_secs());
    let dt = time
        .as_ref()
        .map_or(1.0 / 60.0, |time| time.delta_secs())
        .min(0.1);
    let pulse = 0.35 + 0.45 * (0.5 + 0.5 * (elapsed * std::f32::consts::TAU * 0.9).sin());
    for (mut glow, mut image) in &mut glows {
        let target = threats[glow.sector];
        let level = &mut glow.level;
        let rate = if target > *level { 18.0 } else { 9.0 };
        *level += (target - *level) * (1.0 - (-rate * dt).exp());
        if target == 0.0 && *level < 0.002 {
            *level = 0.0;
        }
        image.color = Color::srgba(1.0, 0.09, 0.05, *level * pulse);
    }

    for (marker_entity, marker, mut node, mut transform, mut image) in &mut markers {
        let Some((point, heading, kind, hostile)) = desired.remove(&marker.fighter) else {
            commands.entity(marker_entity).despawn();
            continue;
        };
        let side = if kind == RadarMarkerKind::Arrow {
            16.0
        } else {
            8.0
        };
        node.left = Val::Px(point.x - side * 0.5);
        node.top = Val::Px(point.y - side * 0.5);
        node.width = Val::Px(side);
        node.height = Val::Px(side);
        node.border_radius = if kind == RadarMarkerKind::Arrow {
            BorderRadius::default()
        } else {
            BorderRadius::all(Val::Percent(50.0))
        };
        transform.rotation = if kind == RadarMarkerKind::Arrow {
            heading
        } else {
            Rot2::IDENTITY
        };
        set_radar_contact_image(&mut image, kind, hostile, &assets);
    }
    for (fighter, (point, heading, kind, hostile)) in desired {
        let mut image = ImageNode::default();
        set_radar_contact_image(&mut image, kind, hostile, &assets);
        let side = if kind == RadarMarkerKind::Arrow {
            16.0
        } else {
            8.0
        };
        let marker = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(point.x - side * 0.5),
                    top: Val::Px(point.y - side * 0.5),
                    width: Val::Px(side),
                    height: Val::Px(side),
                    border_radius: if kind == RadarMarkerKind::Arrow {
                        BorderRadius::default()
                    } else {
                        BorderRadius::all(Val::Percent(50.0))
                    },
                    ..default()
                },
                UiTransform::from_rotation(if kind == RadarMarkerKind::Arrow {
                    heading
                } else {
                    Rot2::IDENTITY
                }),
                image,
                RadarMarker { fighter },
            ))
            .id();
        commands.entity(root_entity).add_child(marker);
    }
}

fn truncated_role_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty()
        || name.eq_ignore_ascii_case("player")
        || name.eq_ignore_ascii_case("fighter")
    {
        return None;
    }
    Some(name.chars().take(32).collect())
}

fn fighter_display_name(
    fighter: &Fighter,
    mission: Option<&crate::plugins::stage_plugin::MissionNpc>,
    remote: Option<&crate::plugins::network_plugin::RemoteFighter>,
    room: &RoomState,
    net: &crate::plugins::network_plugin::NetState,
) -> Option<String> {
    if let Some(mission) = mission {
        return truncated_role_name(&mission.name);
    }
    if let Some(remote) = remote {
        return net
            .members
            .iter()
            .find(|member| member.player_id == remote.player_id)
            .and_then(|member| truncated_role_name(&member.name))
            .or_else(|| {
                net.remote_actor_names
                    .get(&remote.player_id)
                    .and_then(|name| truncated_role_name(name))
            });
    }
    room.members
        .get(fighter.player_slot as usize)
        .and_then(|member| truncated_role_name(&member.name))
}

fn nameplate_in_range(origin: Vec3, fighter: Vec3, already_visible: bool) -> bool {
    let range = if already_visible { 330.0 } else { 300.0 } * meteor_core::MODEL_SCALE;
    origin.distance_squared(fighter) <= range * range
}

fn spawn_fighter_nameplate(
    commands: &mut Commands,
    name: String,
    point: Vec2,
    unit: f32,
    font: Handle<Font>,
    fighter: Entity,
) -> (Entity, Entity) {
    let text = commands
        .spawn((
            Node::default(),
            Text::new(name),
            TextLayout::new(Justify::Left, bevy::text::LineBreak::NoWrap),
            TextFont {
                font: font.into(),
                font_size: (13.0 / unit).into(),
                ..default()
            },
            TextColor(Color::WHITE),
            FighterNameText,
        ))
        .id();

    let content = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::ZERO,
                top: Val::ZERO,
                ..default()
            },
            UiTransform::from_translation(Val2::percent(-50.0, -100.0)),
        ))
        .id();
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::ZERO,
                top: Val::ZERO,
                width: Val::ZERO,
                height: Val::ZERO,
                ..default()
            },
            UiTransform::from_translation(Val2::px(point.x / unit, point.y / unit)),
            Visibility::Visible,
            FighterNameplate { fighter, text },
            PlayingRoot,
        ))
        .id();

    commands.entity(root).add_child(content);
    commands.entity(content).add_child(text);
    spawn_fighter_vital_bars(commands, content, fighter, unit);
    (root, text)
}

fn vital_fraction(current: f32, max: f32) -> f32 {
    if !current.is_finite() || !max.is_finite() || max <= 0.0 {
        0.0
    } else {
        (current / max).clamp(0.0, 1.0)
    }
}

fn spawn_fighter_vital_bars(commands: &mut Commands, root: Entity, fighter: Entity, unit: f32) {
    for qi in [false, true] {
        let height = if qi { 3.0 } else { 4.0 };
        let track = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(50.0),
                    top: Val::Percent(100.0),
                    width: Val::Px(64.0 / unit),
                    height: Val::Px(height / unit),
                    border_radius: BorderRadius::all(Val::Px(1.0)),
                    overflow: Overflow::clip(),
                    ..default()
                },
                UiTransform::from_translation(Val2::new(
                    Val::Percent(-50.0),
                    Val::Px(if qi { 8.0 / unit } else { 2.0 / unit }),
                )),
                BackgroundColor(Color::srgba(0.015, 0.02, 0.03, 0.85)),
                Visibility::Inherited,
                FighterVitalBar {
                    fighter,
                    qi,
                    fill: false,
                },
            ))
            .id();
        let fill = commands
            .spawn((
                Node {
                    width: Val::Percent(0.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(if qi {
                    Color::srgb(0.15, 0.5, 1.0)
                } else {
                    Color::srgb(0.95, 0.16, 0.19)
                }),
                Visibility::Inherited,
                FighterVitalBar {
                    fighter,
                    qi,
                    fill: true,
                },
            ))
            .id();
        commands.entity(track).add_child(fill);
        commands.entity(root).add_child(track);
    }
}

fn update_fighter_vital_bars(
    ui_scale: Res<crate::plugins::ui_scale_plugin::EffectiveUiScale>,
    fighters: Query<(
        &Health,
        Option<&Qi>,
        Option<&crate::plugins::network_plugin::RemoteFighter>,
        Option<&crate::plugins::network_plugin::RemoteVitalsStatus>,
    )>,
    mut bars: Query<(
        &FighterVitalBar,
        &mut Node,
        &mut UiTransform,
        &mut Visibility,
    )>,
) {
    let unit = ui_scale.0.max(0.05);
    for (bar, mut node, mut transform, mut visibility) in &mut bars {
        let value = fighters
            .get(bar.fighter)
            .ok()
            .and_then(|(health, qi, remote, status)| {
                if bar.qi {
                    if remote.is_some() && !status.is_some_and(|status| status.anger_known) {
                        None
                    } else {
                        qi.map(|qi| vital_fraction(qi.current, qi.max))
                    }
                } else if status.is_some_and(|status| !status.health_known) {
                    None
                } else {
                    Some(vital_fraction(health.current, health.max))
                }
            });
        visibility.set_if_neq(if value.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        if bar.fill {
            let width = Val::Percent(value.unwrap_or(0.0) * 100.0);
            if node.width != width {
                node.width = width;
            }
        } else {
            let width = Val::Px(64.0 / unit);
            let height = Val::Px(if bar.qi { 3.0 / unit } else { 4.0 / unit });
            if node.width != width || node.height != height {
                node.width = width;
                node.height = height;
            }
            transform.set_if_neq(UiTransform::from_translation(Val2::new(
                Val::Percent(-50.0),
                Val::Px(if bar.qi { 8.0 / unit } else { 2.0 / unit }),
            )));
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_fighter_nameplates(
    mut commands: Commands,
    settings: Res<Settings>,
    ui_font: Res<UiFont>,
    ui_scale: Res<crate::plugins::ui_scale_plugin::EffectiveUiScale>,
    room: Res<RoomState>,
    net: Res<crate::plugins::network_plugin::NetState>,
    transforms: bevy::transform::helper::TransformHelper,
    cameras: Query<(Entity, &Camera), With<crate::plugins::camera_plugin::GameplayCamera>>,
    fighters: Query<
        (
            Entity,
            &Fighter,
            &Transform,
            &Health,
            Option<&crate::plugins::stage_plugin::LocalFighter>,
            Option<&crate::plugins::stage_plugin::MissionNpc>,
            Option<&crate::plugins::network_plugin::RemoteFighter>,
            Option<&Visibility>,
        ),
        (Without<FighterNameplate>, Without<RadarRoot>),
    >,
    models: Query<(Entity, &crate::plugins::character_plugin::FighterModel)>,
    bones: Query<(Entity, &crate::plugins::character_plugin::FighterBone)>,
    mut names: Query<(&mut Text, &mut TextFont), With<FighterNameText>>,
    mut labels: Query<
        (Entity, &FighterNameplate, &mut UiTransform, &mut Visibility),
        (With<FighterNameplate>, Without<Fighter>, Without<RadarRoot>),
    >,
) {
    let Ok((camera_entity, camera)) = cameras.single() else {
        return;
    };

    let Ok(camera_transform) = transforms.compute_global_transform(camera_entity) else {
        return;
    };
    let Some(viewport) = camera.logical_viewport_size() else {
        return;
    };
    let model_fighters: std::collections::HashMap<Entity, Entity> = models
        .iter()
        .map(|(entity, model)| (entity, model.fighter))
        .collect();
    let head_positions: std::collections::HashMap<Entity, Vec3> = bones
        .iter()
        .filter(|(_, bone)| bone.name.eq_ignore_ascii_case("bau_Head"))
        .filter_map(|(entity, bone)| {
            let fighter = *model_fighters.get(&bone.model)?;
            let transform = transforms.compute_global_transform(entity).ok()?;
            Some((fighter, transform.translation() + Vec3::Y * 0.3))
        })
        .collect();

    let local_position = fighters
        .iter()
        .find_map(|(_, _, transform, _, local, _, _, _)| {
            local.is_some().then_some(transform.translation)
        });
    let visible_fighters: std::collections::HashSet<Entity> = labels
        .iter()
        .map(|(_, label, _, _)| label.fighter)
        .collect();
    let mut desired = std::collections::HashMap::new();
    if settings.show_names {
        for (entity, fighter, transform, health, local, mission, remote, visibility) in &fighters {
            if local.is_some()
                || health.current <= 0.0
                || visibility.is_some_and(|visibility| *visibility == Visibility::Hidden)
            {
                continue;
            }
            let Some(origin) = local_position else {
                continue;
            };
            if !nameplate_in_range(
                origin,
                transform.translation,
                visible_fighters.contains(&entity),
            ) {
                continue;
            }
            let Some(name) = fighter_display_name(fighter, mission, remote, &room, &net) else {
                continue;
            };
            let anchor = head_positions
                .get(&entity)
                .copied()
                .unwrap_or(transform.translation + Vec3::Y * 2.4);
            let Ok(point) = camera.world_to_viewport(&camera_transform, anchor) else {
                continue;
            };
            if point.x < 0.0 || point.y < 0.0 || point.x > viewport.x || point.y > viewport.y {
                continue;
            }

            desired.insert(entity, (name, point - Vec2::Y * 12.0));
        }
    }

    let unit = ui_scale.0.max(0.05);
    for (label_entity, label, mut transform, mut visibility) in &mut labels {
        let Some((name, point)) = desired.remove(&label.fighter) else {
            commands.entity(label_entity).despawn();
            continue;
        };
        if let Ok((mut text, mut font)) = names.get_mut(label.text) {
            if text.0 != name {
                text.0 = name;
            }
            let size = (13.0 / unit).into();
            if font.font_size != size {
                font.font_size = size;
            }
        }
        transform.set_if_neq(UiTransform::from_translation(Val2::px(
            point.x / unit,
            point.y / unit,
        )));
        visibility.set_if_neq(Visibility::Visible);
    }
    for (fighter, (name, point)) in desired {
        spawn_fighter_nameplate(&mut commands, name, point, unit, ui_font.0.clone(), fighter);
    }
}

fn hud_fill_ratio(current: f32, maximum: f32) -> f32 {
    if !current.is_finite() || !maximum.is_finite() || maximum <= 0.0 {
        0.0
    } else {
        (current / maximum).clamp(0.0, 1.0)
    }
}

pub(crate) fn hud_weapon_name(id: u8) -> &'static str {
    match id {
        0 => "空手",
        1 => "飞镖",
        2 => "飞轮",
        3 => "火铳",
        4 => "双刺",
        5 => "匕首",
        6 => "轻剑",
        7 => "长枪",
        8 => "大刀",
        9 => "重锤",
        10 => "乾坤刀",
        11 => "指虎",
        12 => "忍刀",
        _ => "未知武器",
    }
}

fn update_hud_stats(
    mut labels: Query<(&HudVital, &mut Text), Without<HudLoadoutText>>,
    mut loadouts: Query<(&HudLoadoutText, &mut Text), Without<HudVital>>,
    mut fills: Query<(&HudVitalFill, &mut Node)>,
    fighters: Query<(&Fighter, &Health, &Qi), With<crate::plugins::stage_plugin::LocalFighter>>,
) {
    let local = fighters.single().ok();
    for (kind, mut text) in &mut labels {
        let value = local.map_or_else(
            || "—".into(),
            |(_, health, qi)| match kind {
                HudVital::Health => {
                    format!("{:.0} / {:.0}", health.current.max(0.0), health.max)
                }
                HudVital::Qi => format!("{:.0} / {:.0}", qi.current.max(0.0), qi.max),
            },
        );
        if text.0 != value {
            text.0 = value;
        }
    }
    for (fill, mut node) in &mut fills {
        let ratio = local.map_or(0.0, |(_, health, qi)| match fill.0 {
            HudVital::Health => hud_fill_ratio(health.current, health.max),
            HudVital::Qi => hud_fill_ratio(qi.current, qi.max),
        });
        let width = Val::Percent(ratio * 100.0);
        if node.width != width {
            node.width = width;
        }
    }
    for (slot, mut text) in &mut loadouts {
        let value = local.map_or_else(
            || match slot {
                HudLoadoutText::Active => "—".into(),
                HudLoadoutText::Spare => "备 · —".into(),
            },
            |(fighter, _, _)| match slot {
                HudLoadoutText::Active => hud_weapon_name(fighter.weapon).into(),
                HudLoadoutText::Spare => {
                    let spare = fighter.weapon_slots[1 - usize::from(fighter.weapon_slot.min(1))];
                    format!(
                        "备 · {}",
                        if spare == 0 {
                            "—"
                        } else {
                            hud_weapon_name(spare)
                        }
                    )
                }
            },
        );
        if text.0 != value {
            text.0 = value;
        }
    }
}

fn mode_hud_text(
    request: &LaunchRequest,
    net: &crate::plugins::network_plugin::NetState,
    score: &KillScore,
    end: &MatchEnd,
    carrying_flag: bool,
) -> String {
    if !meteor_network::modes::is_original_mode(&request.mode) {
        return String::new();
    }
    let mut status = Vec::new();
    if net.status == crate::plugins::network_plugin::NetStatus::InRoom {
        status.push(if end.settled {
            "已结束".to_string()
        } else if net.round_active {
            "进行中".to_string()
        } else {
            "待开局".to_string()
        });
    }
    match request.mode.as_str() {
        "leader" => status.push(format!("击杀 {}", score.player_kills)),
        "escort" if carrying_flag => {
            status.push("你正携带镖物".to_string());
        }
        "escort" => {
            if let Some(flag) = net.escort_flag.as_ref() {
                let flag_status = match flag.state.as_str() {
                    "carried" => "镖物已被携带".to_string(),
                    "dropped" => flag.return_in.map_or_else(
                        || "镖物已掉落".to_string(),
                        |seconds| format!("镖物掉落 · {:.1} 秒后归位", seconds.max(0.0)),
                    ),
                    _ => "镖物位于归位点".to_string(),
                };
                status.push(flag_status);
            }
        }
        "assassin" if !net.player_id.is_empty() && net.vip_team(&net.player_id).is_some() => {
            status.push("你是本回合 VIP".to_string());
        }
        _ => {}
    }
    if net.status == crate::plugins::network_plugin::NetStatus::InRoom {
        let [meteor_wins, butterfly_wins] = net.stock_team_wins;
        if meteor_wins != 0 || butterfly_wins != 0 {
            status.push(format!("流星 {meteor_wins}:{butterfly_wins} 蝴蝶"));
        }
    }
    if end.settled && net.status != crate::plugins::network_plugin::NetStatus::InRoom {
        status.push("已结束".to_string());
    }
    status.join("  ·  ")
}

fn update_match_result_banner(
    end: Res<MatchEnd>,
    state: Res<State<AppState>>,
    request: Res<LaunchRequest>,
    mut banners: Query<(&mut Text, &mut Node), With<MatchResultBanner>>,
) {
    let visible = end.settled && *state.get() == AppState::Playing && request.mode != "replay";
    for (mut text, mut node) in &mut banners {
        node.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if visible {
            let remaining = (super::combat_plugin::MISSION_SETTLE_SECONDS - end.settle_elapsed)
                .max(0.0)
                .ceil() as u32;
            let value = format!("{}\n{remaining} 秒后结算", result_title(end.result));
            if text.0 != value {
                text.0 = value;
            }
        }
    }
}

fn update_mode_hud(
    request: Res<LaunchRequest>,
    net: Res<crate::plugins::network_plugin::NetState>,
    score: Res<KillScore>,
    end: Res<MatchEnd>,
    carrying: Query<
        (),
        (
            With<crate::plugins::stage_plugin::LocalFighter>,
            Or<(
                With<crate::plugins::stage_plugin::EscortCarrying>,
                With<crate::components::fighter::EscortFlagCarrier>,
            )>,
        ),
    >,
    mut query: Query<(&mut Text, &mut Node), With<ModeObjectiveText>>,
    window: Query<&Window>,
    safe: Option<Res<SafeAreaInsets>>,
) {
    let Ok((mut text, mut node)) = query.single_mut() else {
        return;
    };
    let mut value = mode_hud_text(&request, &net, &score, &end, !carrying.is_empty())
        .split("  ·  ")
        .filter(|part| *part != "你正携带镖物")
        .collect::<Vec<_>>()
        .join(" · ");
    let (width, height) = window
        .single()
        .map_or((1280.0, 720.0), |w| (w.width(), w.height()));
    let mobile = responsive_ui_layout(width, height, cfg!(target_os = "android")).mobile;
    let layout = super::combat_hud_layout::CombatHudLayout::new(
        width,
        height,
        safe.as_deref().copied().unwrap_or_default(),
        mobile,
    );
    let limit = ((layout.status_width - 18.0) / 11.0).floor() as usize;
    if value.chars().count() > limit {
        value = value
            .chars()
            .take(limit.saturating_sub(1))
            .chain(['…'])
            .collect();
    }
    node.display = if value.is_empty() {
        Display::None
    } else {
        Display::Flex
    };
    if text.0 != value {
        text.0 = value;
    }
}

fn update_escort_flag_hud(
    carrying: Query<
        (),
        (
            With<crate::plugins::stage_plugin::LocalFighter>,
            Or<(
                With<crate::plugins::stage_plugin::EscortCarrying>,
                With<crate::components::fighter::EscortFlagCarrier>,
            )>,
        ),
    >,
    mut icons: Query<(&mut Visibility, &mut Node), With<EscortFlagHudIcon>>,
) {
    let visible = !carrying.is_empty();
    for (mut visibility, mut node) in &mut icons {
        node.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        *visibility = if visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

#[derive(Component)]
struct PauseRoot;

#[derive(Component)]
struct PauseResumeButton;

#[derive(Component)]
struct PauseSettingsButton;

#[derive(Component)]
struct PauseBackToMenuButton;

#[derive(Component)]
pub(crate) struct ExitGameButton;

#[derive(Component)]
struct ExitConfirmation;

#[derive(Component)]
struct ExitConfirmationChoice(bool);

fn exit_game_buttons(
    mut commands: Commands,
    buttons: Query<&Interaction, (Changed<Interaction>, With<ExitGameButton>)>,
    choices: Query<(&Interaction, &ExitConfirmationChoice), Changed<Interaction>>,
    confirmation: Query<Entity, With<ExitConfirmation>>,
    pause: Query<Entity, With<PauseRoot>>,
    state: Res<State<AppState>>,
    screen: Res<MenuScreen>,
    font: Res<UiFont>,
    mut exit: MessageWriter<AppExit>,
) {
    if !confirmation.is_empty() {
        for (interaction, choice) in &choices {
            if *interaction == Interaction::Pressed {
                if choice.0 {
                    exit.write(AppExit::Success);
                }
                for entity in &confirmation {
                    commands.entity(entity).despawn();
                }
                break;
            }
        }
        return;
    }
    if !buttons
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        return;
    }
    if *state.get() == AppState::Menu && *screen == MenuScreen::Main {
        exit.write(AppExit::Success);
    } else if *state.get() == AppState::Paused {
        let Ok(parent) = pause.single() else {
            return;
        };
        let overlay = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    padding: UiRect::all(Val::Px(16.0)),
                    ..default()
                },
                BackgroundColor(theme::SCRIM),
                bevy::ui::FocusPolicy::Block,
                UiInputBlocker,
                GlobalZIndex(PAUSE_OVERLAY_Z_INDEX + 1),
                ExitConfirmation,
            ))
            .id();
        commands.entity(parent).add_child(overlay);
        let surface = overlay_surface(&mut commands);
        commands.entity(overlay).add_child(surface);
        let title = text(
            &mut commands,
            "确认退出游戏？",
            26.0,
            theme::TEXT,
            Some(font.0.clone()),
        );
        commands.entity(surface).add_child(title);
        for (label, confirm) in [("取消", false), ("确认退出", true)] {
            let button = menu_button(
                &mut commands,
                label,
                240.0,
                theme::CARD,
                Some(font.0.clone()),
            );
            commands
                .entity(button)
                .insert(ExitConfirmationChoice(confirm));
            commands.entity(surface).add_child(button);
        }
    }
}

#[derive(Resource, Debug, Clone, Copy)]
struct SettingsReturnState(AppState);

fn toggle_pause(
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<State<AppState>>,
    capture: Res<KeyCaptureState>,
    debug_console: Option<Res<crate::plugins::debug_plugin::DebugConsoleState>>,
    mut next_state: ResMut<NextState<AppState>>,
    loading: Option<Res<super::loading_ui::LoadingPresentation>>,
    confirmation: Query<Entity, With<ExitConfirmation>>,
    mut commands: Commands,
) {
    if loading.is_some_and(|loading| loading.active) {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        if capture.pending.is_some()
            || debug_console.is_some_and(|console| console.captures_input())
        {
            return;
        }
        if !confirmation.is_empty() {
            for entity in &confirmation {
                commands.entity(entity).despawn();
            }
            return;
        }
        match state.get() {
            AppState::Playing => next_state.set(AppState::Paused),
            AppState::Paused => next_state.set(AppState::Playing),
            _ => {}
        }
    }
}

fn pause_clock(
    state: Res<State<AppState>>,
    opened_from: Option<Res<SettingsReturnState>>,
    loading: Option<Res<super::loading_ui::LoadingPresentation>>,
    mut time: ResMut<Time<Virtual>>,
) {
    let keep_paused_in_settings =
        *state.get() == AppState::Settings && opened_from.is_some_and(|r| r.0 == AppState::Paused);
    let paused = *state.get() == AppState::Paused
        || keep_paused_in_settings
        || loading.is_some_and(|loading| loading.active);
    if paused != time.is_paused() {
        if paused {
            time.pause();
        } else {
            time.unpause();
        }
    }
}

fn setup_pause_menu(mut commands: Commands, ui_font: Res<UiFont>) {
    let font = Some(ui_font.0.clone());
    let overlay = commands
        .spawn((
            scrollable_y(Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(24.0)),
                ..default()
            }),
            BackgroundColor(theme::SCRIM),
            bevy::ui::FocusPolicy::Block,
            UiInputBlocker,
            PauseRoot,
            ZIndex(PAUSE_OVERLAY_Z_INDEX),
            ResponsiveOverlayRoot,
        ))
        .id();
    let surface = overlay_surface(&mut commands);
    commands.entity(overlay).add_child(surface);
    let title = text(&mut commands, "暂停", 34.0, theme::TEXT, font.clone());
    commands.entity(title).insert(ResponsiveOverlayTitle(34.0));
    commands.entity(surface).add_child(title);
    let resume = menu_button(
        &mut commands,
        "继续游戏 (Esc)",
        240.0,
        theme::PRIMARY,
        font.clone(),
    );
    commands.entity(resume).insert(PauseResumeButton);
    commands.entity(surface).add_child(resume);
    let settings_btn = menu_button(&mut commands, "设置", 240.0, theme::CARD, font.clone());
    commands.entity(settings_btn).insert(PauseSettingsButton);
    commands.entity(surface).add_child(settings_btn);
    let back = menu_button(
        &mut commands,
        "返回主菜单",
        240.0,
        theme::CARD,
        font.clone(),
    );
    commands.entity(back).insert(PauseBackToMenuButton);
    commands.entity(surface).add_child(back);
    let exit = menu_button(&mut commands, "退出游戏", 240.0, theme::CARD, font);
    commands.entity(exit).insert(ExitGameButton);
    commands.entity(surface).add_child(exit);
}

fn pause_resume_button(
    query: Query<&Interaction, (Changed<Interaction>, With<PauseResumeButton>)>,
    confirmation: Query<Entity, With<ExitConfirmation>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if !confirmation.is_empty() {
        return;
    }
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        next_state.set(AppState::Playing);
    }
}

fn pause_settings_button(
    query: Query<&Interaction, (Changed<Interaction>, With<PauseSettingsButton>)>,
    mut commands: Commands,
    confirmation: Query<Entity, With<ExitConfirmation>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if !confirmation.is_empty() {
        return;
    }
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        commands.insert_resource(SettingsReturnState(AppState::Paused));
        next_state.set(AppState::Settings);
    }
}

fn pause_back_to_menu_button(
    query: Query<&Interaction, (Changed<Interaction>, With<PauseBackToMenuButton>)>,
    confirmation: Query<Entity, With<ExitConfirmation>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if !confirmation.is_empty() {
        return;
    }
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        next_state.set(AppState::Menu);
    }
}

#[derive(Component)]
struct RoomMembersText;

fn setup_settings_panel(
    mut commands: Commands,
    ui_font: Res<UiFont>,
    settings: Res<Settings>,
    keymap: Res<Keymap>,
) {
    let font = ui_font.0.clone();
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(16.0)),
                row_gap: Val::Px(12.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.039, 0.051, 0.059, 0.55)),
            bevy::ui::FocusPolicy::Block,
            UiInputBlocker,
            SettingsRoot,
            ZIndex(PAUSE_OVERLAY_Z_INDEX),
            ResponsiveOverlayRoot,
        ))
        .with_children(|root| {
            spawn_full_settings_page(root, &font, &settings, &keymap, true);
            crate::plugins::settings_ui::spawn_settings_select_dialogs(root, &font, &settings);
        });
}

fn settings_escape_back(
    keys: Res<ButtonInput<KeyCode>>,
    editor: Res<MobileLayoutEditorState>,
    select: Option<Res<crate::plugins::ui_components::UiSelectState>>,
    mut capture: ResMut<KeyCaptureState>,
    return_to: Option<Res<SettingsReturnState>>,
    mut commands: Commands,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if editor.is_open() || select.is_some_and(|select| select.open || select.blocked_this_frame) {
        return;
    }
    if capture.esc_consumed {
        capture.esc_consumed = false;
        return;
    }
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    let target = return_to.map(|r| r.0).unwrap_or(AppState::Paused);
    commands.remove_resource::<SettingsReturnState>();
    next_state.set(target);
}

fn settings_back_button(
    query: Query<&Interaction, (Changed<Interaction>, With<SettingsBackButton>)>,
    mut commands: Commands,
    return_to: Option<Res<SettingsReturnState>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        let target = return_to.map(|r| r.0).unwrap_or(AppState::Paused);
        commands.remove_resource::<SettingsReturnState>();
        next_state.set(target);
    }
}

fn menu_record_button(
    query: Query<&Interaction, (Changed<Interaction>, With<MenuRecordButton>)>,
    mut recorder: ResMut<crate::plugins::replay_plugin::ReplayRecorder>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        recorder.active = !recorder.active;
        info!("recording {}", if recorder.active { "ON" } else { "OFF" });
    }
}

fn menu_auto_record_button(
    query: Query<&Interaction, (Changed<Interaction>, With<MenuAutoRecordButton>)>,
    mut recorder: ResMut<crate::plugins::replay_plugin::ReplayRecorder>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        recorder.auto = !recorder.auto;
        info!("auto record {}", if recorder.auto { "ON" } else { "OFF" });
    }
}

fn menu_replay_button(
    query: Query<&Interaction, (Changed<Interaction>, With<MenuReplayButton>)>,
    mut player: ResMut<crate::plugins::replay_plugin::ReplayPlayer>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        let dir = std::path::Path::new(crate::plugins::replay_plugin::REPLAY_DIR);
        match crate::plugins::replay_plugin::load_latest_replay(dir) {
            Some((path, events)) => {
                player.active = true;
                player.events = events;
                player.elapsed = 0.0;
                player.index = 0;
                player.file_name = path.clone();
                info!("replay loaded: {path}");
                next_state.set(AppState::Loading);
            }
            None => warn!("no replay files found in replays/"),
        }
    }
}

fn menu_mrc_button(
    query: Query<&Interaction, (Changed<Interaction>, With<MenuMrcButton>)>,
    mut playback: ResMut<crate::plugins::replay_plugin::MrcPlayback>,
    mut request: ResMut<LaunchRequest>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        let dir = std::path::Path::new(crate::plugins::replay_plugin::REPLAY_DIR);
        let first = std::fs::read_dir(&dir).ok().and_then(|entries| {
            let mut paths: Vec<_> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("mrc"))
                })
                .collect();
            paths.sort();
            paths.into_iter().next()
        });
        match first {
            Some(path) => match playback.load(&path) {
                Ok(()) => {
                    request.mode = "replay".into();
                    request.spawn_room_npcs = false;
                    if let Some(stage_id) = playback.stage_id() {
                        let stage = meteor_scene::stage_registry::get_stage(&stage_id, &[]);
                        request.stage_id = stage.id;
                        request.stage_name = stage.name;
                    }
                    if let Some((weapon, _)) = playback.local_weapon() {
                        request.weapon = weapon;
                    }
                    info!("mrc loaded: {}", path.display());
                    next_state.set(AppState::Loading);
                }
                Err(error) => warn!("mrc load failed: {error}"),
            },
            None => warn!("no mrc recordings in replays/"),
        }
    }
}

fn reset_dialogue(mut dialogue: ResMut<DialogueState>, mut view: ResMut<DialoguePresentation>) {
    *dialogue = DialogueState::default();
    *view = DialoguePresentation::default();
}

fn split_dialogue(content: &str) -> (String, String) {
    if let Some((speaker, body)) = content.split_once('：').or_else(|| content.split_once(':')) {
        if !speaker.trim().is_empty() && speaker.chars().count() <= 20 {
            return (speaker.trim().to_string(), body.trim().to_string());
        }
    }
    ("对话".into(), content.trim().into())
}

fn dialogue_pages(body: &str, columns: usize, lines: usize) -> Vec<String> {
    let columns = columns.max(1);
    let lines = lines.max(1);
    let mut pages = Vec::new();
    let mut page = String::new();
    let mut column = 0;
    let mut line = 0;
    for ch in body.chars() {
        if ch == '\r' {
            continue;
        }
        if ch == '\n' || column == columns {
            line += 1;
            column = 0;
            if line == lines {
                pages.push(std::mem::take(&mut page));
                line = 0;
            } else {
                page.push('\n');
            }
            if ch == '\n' {
                continue;
            }
        }
        page.push(ch);
        column += 1;
    }
    if !page.is_empty() {
        pages.push(page);
    }
    if pages.is_empty() {
        pages.push(String::new());
    }
    pages
}

#[derive(Component)]
struct GameplayPauseButton;

fn update_gameplay_pause(
    buttons: Query<&Interaction, (Changed<Interaction>, With<GameplayPauseButton>)>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
    window: Query<&Window>,
    safe_area: Option<Res<SafeAreaInsets>>,
    scale: Option<Res<crate::plugins::ui_scale_plugin::EffectiveUiScale>>,
    mut nodes: Query<&mut Node, With<GameplayPauseButton>>,
) {
    let unit = scale.as_ref().map_or(1.0, |s| s.0.max(0.05));
    let mobile = window.single().is_ok_and(|w| {
        responsive_ui_layout(w.width(), w.height(), cfg!(target_os = "android")).mobile
    });
    let (width, height) = window
        .single()
        .map_or((1280.0, 720.0), |w| (w.width(), w.height()));
    let hud = super::combat_hud_layout::CombatHudLayout::new(
        width,
        height,
        safe_area.as_deref().copied().unwrap_or_default(),
        mobile,
    );
    for mut node in &mut nodes {
        node.display = if mobile || cfg!(target_arch = "wasm32") {
            Display::None
        } else {
            Display::Flex
        };
        node.width = Val::Px(hud.button_size.x / unit);
        node.height = Val::Px(hud.button_size.y / unit);
        node.right = Val::Px((width - hud.pause_center.x - hud.button_size.x * 0.5) / unit);
        node.top = Val::Px((hud.pause_center.y - hud.button_size.y * 0.5) / unit);
    }
    if *state.get() == AppState::Playing && buttons.iter().any(|i| *i == Interaction::Pressed) {
        next.set(AppState::Paused);
    }
}

fn update_dialogue_hud(
    time: Res<Time>,
    mut dialogue: ResMut<DialogueState>,
    mut view: ResMut<DialoguePresentation>,
    window: Query<&Window>,
    safe_area: Option<Res<SafeAreaInsets>>,
    mut texts: Query<&mut Text, (With<DialogueHudText>, Without<DialogueSpeakerText>)>,
    mut speakers: Query<&mut Text, (With<DialogueSpeakerText>, Without<DialogueHudText>)>,
    mut panels: Query<&mut Visibility, With<DialoguePanelRoot>>,
) {
    let Some(content) = dialogue.text.clone().filter(|text| !text.trim().is_empty()) else {
        for mut panel in &mut panels {
            *panel = Visibility::Hidden;
        }
        *view = DialoguePresentation::default();
        return;
    };
    let (width, height) = window
        .single()
        .map_or((1280.0, 720.0), |w| (w.width(), w.height()));
    let mobile = responsive_ui_layout(width, height, cfg!(target_os = "android")).mobile;
    let layout = super::combat_hud_layout::CombatHudLayout::new(
        width,
        height,
        safe_area.as_deref().copied().unwrap_or_default(),
        mobile,
    );
    let capacity = (
        ((layout.dialogue_size.x - 124.0) / 15.0).floor().max(1.0) as usize,
        2,
    );
    if view.source != content
        || view.capacity != capacity
        || dialogue.timer > view.previous_timer + 0.001
    {
        let (speaker, body) = split_dialogue(&content);
        let pages = dialogue_pages(&body, capacity.0, capacity.1);

        let page_duration =
            (body.chars().count() as f32 / pages.len() as f32 / 10.0).clamp(2.5, 6.0);
        dialogue.timer = dialogue.timer.max(page_duration * pages.len() as f32);
        *view = DialoguePresentation {
            source: content,
            speaker,
            pages,
            page: 0,
            page_left: page_duration,
            page_duration,
            previous_timer: dialogue.timer,
            capacity,
        };
    }
    let dt = time.delta_secs();
    dialogue.timer -= dt;
    view.page_left -= dt;
    while view.page_left <= 0.0 && view.page + 1 < view.pages.len() {
        view.page += 1;
        view.page_left += view.page_duration;
    }
    view.previous_timer = dialogue.timer;
    if dialogue.timer <= 0.0 {
        dialogue.text = None;
        for mut panel in &mut panels {
            *panel = Visibility::Hidden;
        }
        return;
    }
    for mut text in &mut texts {
        if text.0 != view.pages[view.page] {
            text.0.clone_from(&view.pages[view.page]);
        }
    }
    for mut speaker in &mut speakers {
        let label = if view.pages.len() > 1 {
            format!(
                "{}  ·  {}/{}",
                view.speaker,
                view.page + 1,
                view.pages.len()
            )
        } else {
            view.speaker.clone()
        };
        if speaker.0 != label {
            speaker.0 = label;
        }
    }
    for mut panel in &mut panels {
        *panel = Visibility::Visible;
    }
}

fn setup_room_panel(mut commands: Commands, ui_font: Res<UiFont>, room: Res<RoomState>) {
    let font = Some(ui_font.0.clone());
    let overlay = commands
        .spawn((
            scrollable_y(Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(24.0)),
                ..default()
            }),
            BackgroundColor(theme::SCRIM),
            bevy::ui::FocusPolicy::Block,
            UiInputBlocker,
            RoomRoot,
            ResponsiveOverlayRoot,
        ))
        .id();
    let surface = overlay_surface(&mut commands);
    commands.entity(overlay).add_child(surface);
    let title = text(&mut commands, "对战房间", 34.0, theme::TEXT, font.clone());
    commands.entity(title).insert(ResponsiveOverlayTitle(34.0));
    commands.entity(surface).add_child(title);
    let members = text(
        &mut commands,
        &room_members_text(&room),
        18.0,
        Color::WHITE,
        font.clone(),
    );
    commands.entity(members).insert(RoomMembersText);
    commands.entity(surface).add_child(members);
    let add = menu_button(
        &mut commands,
        "添加对手 (NPC)",
        220.0,
        Color::srgb(0.25, 0.35, 0.45),
        font.clone(),
    );
    commands.entity(add).insert(RoomAddNpcButton);
    commands.entity(surface).add_child(add);
    let remove = menu_button(&mut commands, "移除对手", 220.0, theme::CARD, font.clone());
    commands.entity(remove).insert(RoomRemoveNpcButton);
    commands.entity(surface).add_child(remove);
    let start = menu_button(
        &mut commands,
        "开始对战",
        240.0,
        theme::PRIMARY,
        font.clone(),
    );
    commands.entity(start).insert(RoomStartButton);
    commands.entity(surface).add_child(start);
    let back = menu_button(&mut commands, "返回主菜单", 220.0, theme::CARD, font);
    commands.entity(back).insert(RoomBackButton);
    commands.entity(surface).add_child(back);
}

#[derive(Component)]
struct NetRoomSurface;
#[derive(Component)]
struct NetRoomColumns;
#[derive(Component)]
struct NetRoomPanel;
#[derive(Component)]
struct NetRoomRosterRow(usize);
#[derive(Component)]
struct NetRoomRosterText {
    slot: usize,
    detail: bool,
}
#[derive(Component)]
struct NetRoomReadyDot(usize);
#[derive(Component)]
struct NetRoomRosterList;
#[derive(Component)]
struct NetRoomDetailsText;

#[derive(Component)]
struct NetRoomFooter;
#[derive(Component)]
struct NetRoomHeader;
#[derive(Component)]
struct NetRoomRosterHeading;

fn net_room_action(
    commands: &mut Commands,
    label: &str,
    width: f32,
    primary: bool,
    font: Option<Handle<Font>>,
) -> Entity {
    let button = commands
        .spawn((
            Button,
            Node {
                width: Val::Px(width),
                height: Val::Px(44.0),
                min_height: Val::Px(44.0),
                flex_shrink: 0.0,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(10.0)),
                ..default()
            },
            BackgroundColor(Color::NONE),
            BorderColor::all(theme::LINE),
            theme::ActionStyle { primary },
            theme::CompactAction,
        ))
        .id();
    if !label.is_empty() {
        let label = text(commands, label, 14.0, theme::TEXT, font);
        commands.entity(button).add_child(label);
    }
    button
}

pub(crate) fn setup_net_room_panel(mut commands: Commands, ui_font: Res<UiFont>) {
    use crate::plugins::network_plugin::room_lobby::{
        LobbyHint, ProfileArrow, ProfileText, ReadyButton,
    };
    use crate::plugins::ui_components::{
        UiSelectableButtonBundle, UiSelectableKind, UiSelectableStyle,
    };
    let font = Some(ui_font.0.clone());
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                padding: UiRect::all(Val::Px(24.0)),
                ..default()
            },
            BackgroundColor(theme::BACKGROUND),
            bevy::ui::FocusPolicy::Block,
            UiInputBlocker,
            NetRoomRoot,
        ))
        .id();
    let surface = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                max_width: Val::Px(1280.0),
                height: Val::Percent(100.0),
                min_height: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(16.0),
                ..default()
            },
            NetRoomSurface,
        ))
        .id();
    commands.entity(root).add_child(surface);
    let header = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_shrink: 0.0,
                align_items: AlignItems::Center,
                column_gap: Val::Px(24.0),
                ..default()
            },
            NetRoomHeader,
        ))
        .id();
    let leave = net_room_action(&mut commands, "离开房间", 112.0, false, font.clone());
    commands.entity(leave).insert(NetRoomLeaveButton);
    let titles = commands
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(4.0),
            min_width: Val::Px(0.0),
            ..default()
        })
        .id();
    let title = text(&mut commands, "房间整备", 26.0, theme::TEXT, font.clone());
    let info = text(
        &mut commands,
        "正在同步房间信息",
        12.0,
        theme::MUTED,
        font.clone(),
    );
    commands.entity(info).insert(NetRoomDetailsText);
    commands.entity(titles).add_children(&[title, info]);
    commands.entity(header).add_children(&[leave, titles]);
    commands.entity(surface).add_child(header);
    let columns = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                min_height: Val::Px(0.0),
                flex_grow: 1.0,
                flex_basis: Val::Px(0.0),
                column_gap: Val::Px(24.0),
                row_gap: Val::Px(16.0),
                align_items: AlignItems::Stretch,
                ..default()
            },
            NetRoomColumns,
        ))
        .id();
    commands.entity(surface).add_child(columns);
    let roster_panel = commands
        .spawn(Node {
            min_width: Val::Px(0.0),
            min_height: Val::Px(0.0),
            flex_grow: 1.0,
            flex_basis: Val::Px(0.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(10.0),
            ..default()
        })
        .id();
    let heading = text(
        &mut commands,
        "房间成员 · 16 席",
        12.0,
        theme::METAL,
        font.clone(),
    );
    commands.entity(heading).insert(NetRoomRosterHeading);
    let members = commands
        .spawn((
            scrollable_y(Node {
                width: Val::Percent(100.0),
                min_height: Val::Px(0.0),
                flex_grow: 1.0,
                flex_basis: Val::Px(0.0),
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                align_content: AlignContent::FlexStart,
                column_gap: Val::Percent(2.0),
                row_gap: Val::Px(6.0),
                ..default()
            }),
            NetRoomRosterList,
        ))
        .id();
    commands
        .entity(roster_panel)
        .add_children(&[heading, members]);
    commands.entity(columns).add_child(roster_panel);
    for slot in 0..16 {
        let row = commands
            .spawn((
                Node {
                    width: Val::Percent(49.0),
                    min_height: Val::Px(64.0),
                    flex_shrink: 0.0,
                    padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
                    column_gap: Val::Px(8.0),
                    align_items: AlignItems::Center,
                    border: UiRect::bottom(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(theme::CARD.with_alpha(0.6)),
                BorderColor::all(theme::LINE),
                NetRoomRosterRow(slot),
            ))
            .id();
        let number = text(
            &mut commands,
            &format!("{:02}", slot + 1),
            12.0,
            theme::MUTED,
            font.clone(),
        );
        let labels = commands
            .spawn(Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                flex_grow: 1.0,
                flex_basis: Val::Px(0.0),
                min_width: Val::Px(0.0),
                ..default()
            })
            .id();
        for detail in [false, true] {
            let label = text(
                &mut commands,
                "",
                if detail { 11.0 } else { 14.0 },
                if detail { theme::MUTED } else { theme::TEXT },
                font.clone(),
            );
            commands
                .entity(label)
                .insert(NetRoomRosterText { slot, detail });
            commands.entity(labels).add_child(label);
        }
        let dot = commands
            .spawn((
                Node {
                    width: Val::Px(5.0),
                    height: Val::Px(5.0),
                    flex_shrink: 0.0,
                    border_radius: BorderRadius::all(Val::Percent(50.0)),
                    ..default()
                },
                BackgroundColor(theme::MUTED),
                NetRoomReadyDot(slot),
            ))
            .id();
        commands.entity(row).add_children(&[number, labels, dot]);
        commands.entity(members).add_child(row);
    }
    let details = commands
        .spawn((
            scrollable_y(Node {
                width: Val::Px(280.0),
                min_height: Val::Px(0.0),
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(12.0)),
                border: UiRect::top(Val::Px(1.0)),
                ..default()
            }),
            BackgroundColor(theme::SURFACE.with_alpha(0.72)),
            BorderColor::all(theme::LINE),
            NetRoomPanel,
        ))
        .id();
    commands.entity(columns).add_child(details);
    let heading = text(&mut commands, "我的整备", 12.0, theme::METAL, font.clone());
    commands.entity(details).add_child(heading);
    for character in [true, false] {
        let row = commands
            .spawn(Node {
                width: Val::Percent(100.0),
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                flex_shrink: 0.0,
                ..default()
            })
            .id();
        let label = text(&mut commands, "", 13.0, theme::TEXT, font.clone());
        commands.entity(label).insert((
            ProfileText(character),
            Node {
                flex_grow: 1.0,
                flex_basis: Val::Px(0.0),
                min_width: Val::Px(0.0),
                ..default()
            },
        ));
        let mut arrows = Vec::new();
        for direction in [-1, 1] {
            let arrow = net_room_action(&mut commands, "", 40.0, false, font.clone());
            commands
                .entity(arrow)
                .insert(ProfileArrow {
                    character,
                    direction,
                })
                .with_children(|p| {
                    crate::plugins::ui_components::spawn_loadout_arrow_icon(p, direction);
                });
            arrows.push(arrow);
        }
        commands
            .entity(row)
            .add_children(&[arrows[0], label, arrows[1]]);
        commands.entity(details).add_child(row);
    }
    let teams = commands
        .spawn(Node {
            width: Val::Percent(100.0),
            column_gap: Val::Percent(4.0),
            flex_shrink: 0.0,
            ..default()
        })
        .id();
    for (team, label) in [(1, "流星"), (2, "蝴蝶")] {
        let button = commands
            .spawn((
                UiSelectableButtonBundle::new(
                    Node {
                        width: Val::Percent(48.0),
                        min_height: Val::Px(40.0),
                        padding: UiRect::horizontal(Val::Px(8.0)),
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::Center,
                        ..default()
                    },
                    false,
                    UiSelectableKind::Radio,
                    UiSelectableStyle::default(),
                ),
                NetRoomTeamSelectButton { team },
            ))
            .id();
        let label = text(&mut commands, label, 12.0, theme::TEXT, font.clone());
        commands.entity(button).add_child(label);
        commands.entity(teams).add_child(button);
    }
    commands.entity(details).add_child(teams);
    let chat_heading = text(&mut commands, "房间消息", 11.0, theme::METAL, font.clone());
    commands.entity(chat_heading).insert(Node {
        margin: UiRect::top(Val::Px(8.0)),
        ..default()
    });
    let chat = text(
        &mut commands,
        "暂无聊天消息",
        11.0,
        theme::MUTED,
        font.clone(),
    );
    commands.entity(chat).insert(NetRoomChatText);
    let input = text(
        &mut commands,
        "点击输入消息",
        11.0,
        theme::MUTED,
        font.clone(),
    );
    commands.entity(input).insert((
        NetRoomChatInputText,
        Button,
        Node {
            min_height: Val::Px(44.0),
            align_items: AlignItems::Center,
            ..default()
        },
        super::native_text_input::NativeTextEdit {
            field: super::native_text_input::TextField::Chat,
            value: String::new(),
        },
    ));
    commands
        .entity(details)
        .add_children(&[chat_heading, chat, input]);
    let actions = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_shrink: 0.0,
                min_height: Val::Px(52.0),
                align_items: AlignItems::Center,
                column_gap: Val::Px(16.0),
                padding: UiRect::top(Val::Px(8.0)),
                border: UiRect::top(Val::Px(1.0)),
                ..default()
            },
            BorderColor::all(theme::LINE),
            NetRoomFooter,
        ))
        .id();
    let hint = text(
        &mut commands,
        "选择整备后准备",
        12.0,
        theme::METAL,
        font.clone(),
    );
    commands.entity(hint).insert((
        LobbyHint,
        Node {
            flex_grow: 1.0,
            flex_basis: Val::Px(0.0),
            min_width: Val::Px(0.0),
            ..default()
        },
    ));
    let ready = net_room_action(&mut commands, "准备", 144.0, true, font.clone());
    commands.entity(ready).insert(ReadyButton);
    let start = net_room_action(&mut commands, "开始游戏", 144.0, true, font);
    commands.entity(start).insert(NetRoomStartButton);
    commands.entity(actions).add_children(&[hint, ready, start]);
    commands.entity(surface).add_child(actions);
}

fn adapt_net_room_layout(
    windows: Query<&Window>,
    safe_area: Option<Res<crate::plugins::ui_scale_plugin::SafeAreaInsets>>,
    mut nodes: ParamSet<(
        Query<&mut Node, With<NetRoomSurface>>,
        Query<&mut Node, With<NetRoomColumns>>,
        Query<&mut Node, With<NetRoomPanel>>,
        Query<&mut Node, With<NetRoomRosterList>>,
        Query<&mut Node, With<NetRoomRoot>>,
        Query<&mut Node, With<NetRoomRosterRow>>,
    )>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let stacked = window.width() < 600.0;
    let short = window.height() < 500.0;
    let layout = crate::plugins::launcher_ui::responsive_ui_layout_with_safe_area(
        window.width(),
        window.height(),
        cfg!(target_os = "android"),
        safe_area.as_deref().copied().unwrap_or_default(),
    );
    for mut node in &mut nodes.p0() {
        node.row_gap = Val::Px(if short { 10.0 } else { 16.0 });
        node.padding = UiRect::new(
            Val::Px(layout.left_gutter.max(18.0)),
            Val::Px(layout.right_gutter.max(18.0)),
            Val::Px(layout.top_gutter.max(if short { 10.0 } else { 24.0 })),
            Val::Px(layout.bottom_gutter.max(if short { 10.0 } else { 24.0 })),
        );
    }
    for mut node in &mut nodes.p1() {
        node.flex_direction = if stacked {
            FlexDirection::Column
        } else {
            FlexDirection::Row
        };
        node.column_gap = Val::Px(if short { 16.0 } else { 24.0 });
    }
    for mut node in &mut nodes.p2() {
        node.width = if stacked {
            Val::Percent(100.0)
        } else {
            Val::Px(if short { 260.0 } else { 300.0 })
        };
        node.height = if stacked { Val::Px(260.0) } else { Val::Auto };
        node.row_gap = Val::Px(if short { 8.0 } else { 12.0 });
        node.padding = UiRect::all(Val::Px(if short { 8.0 } else { 12.0 }));
    }
    for mut node in &mut nodes.p3() {
        node.max_height = Val::Auto;
    }
    for mut node in &mut nodes.p4() {
        node.padding = UiRect::ZERO;
    }
    for mut node in &mut nodes.p5() {
        node.width = Val::Percent(if window.width() >= 1100.0 { 23.5 } else { 49.0 });
    }
}

fn update_net_room_cards(
    net: Option<Res<crate::plugins::network_plugin::NetState>>,
    mut rows: Query<
        (
            &NetRoomRosterRow,
            &mut Node,
            &mut BackgroundColor,
            &mut BorderColor,
        ),
        Without<NetRoomReadyDot>,
    >,
    mut labels: Query<(&NetRoomRosterText, &mut Text), Without<NetRoomDetailsText>>,
    mut details: Query<&mut Text, (With<NetRoomDetailsText>, Without<NetRoomRosterText>)>,
    mut dots: Query<
        (&NetRoomReadyDot, &mut BackgroundColor, &mut Visibility),
        Without<NetRoomRosterRow>,
    >,
) {
    if rows.is_empty() {
        return;
    }
    let Some(net) = net else {
        return;
    };
    for (row, mut node, mut background, mut border) in &mut rows {
        node.display = if row.0 < 16 {
            Display::Flex
        } else {
            Display::None
        };
        let member = net.members.get(row.0);
        let local = member.is_some_and(|m| m.player_id == net.player_id);
        background.0 = if local {
            theme::SELECTED.with_alpha(0.5)
        } else if member.is_some() {
            theme::CARD.with_alpha(0.65)
        } else {
            Color::NONE
        };
        node.border.left = Val::Px(if local { 2.0 } else { 0.0 });
        border.left = if local {
            theme::ACCENT_TEXT
        } else {
            Color::NONE
        };
        border.bottom = theme::LINE.with_alpha(if member.is_some() { 0.28 } else { 0.12 });
    }
    for (label, mut text) in &mut labels {
        let value = net.members.get(label.slot).map_or_else(
            || {
                if label.detail {
                    if label.slot < net.room_capacity() {
                        ""
                    } else {
                        "房主限制席位"
                    }
                    .into()
                } else {
                    if label.slot < net.room_capacity() {
                        "空闲位置"
                    } else {
                        "已关闭"
                    }
                    .into()
                }
            },
            |member| {
                if label.detail {
                    format!(
                        "{} · {}\n{} · {}",
                        crate::plugins::launcher_ui::character_name(member.character_id),
                        hud_weapon_name(member.weapon),
                        match member.team {
                            Some(1) => "流星",
                            Some(2) => "蝴蝶",
                            _ => "无阵营",
                        },
                        if !member.connected {
                            "已断线"
                        } else if net.room_owner() == Some(member.player_id.as_str()) {
                            "房主"
                        } else if member.ready {
                            "已准备"
                        } else {
                            "未准备"
                        }
                    )
                } else {
                    format!(
                        "{}{}{}",
                        member.name,
                        if net.room_owner() == Some(member.player_id.as_str()) {
                            "（房主）"
                        } else {
                            ""
                        },
                        if member.player_id == net.player_id {
                            " · 你"
                        } else {
                            ""
                        }
                    )
                }
            },
        );
        if text.0 != value {
            text.0 = value;
        }
    }
    for (dot, mut bg, mut visibility) in &mut dots {
        *visibility = if net.members.get(dot.0).is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        bg.0 = if net.members.get(dot.0).is_some_and(|member| {
            member.connected
                && (member.ready || net.room_owner() == Some(member.player_id.as_str()))
        }) {
            Color::srgb(0.36, 0.73, 0.48)
        } else {
            theme::MUTED
        };
    }
    let stage = official_stages()
        .into_iter()
        .find(|stage| stage.id == net.stage)
        .map(|stage| stage.name)
        .unwrap_or_else(|| net.stage.clone());
    let info = format!(
        "{}  /  {}  ·  {} / {} 人  ·  友军伤害{}",
        if stage.is_empty() {
            "地图待确认"
        } else {
            &stage
        },
        meteor_network::modes::mode_label(&net.mode),
        net.members.len(),
        net.room_capacity(),
        if net.friendly_fire {
            "开启"
        } else {
            "关闭"
        }
    );
    for mut text in &mut details {
        if text.0 != info {
            text.0.clone_from(&info);
        }
    }
}

fn room_members_text(room: &RoomState) -> String {
    let mut lines = Vec::with_capacity(room.members.len() + 1);
    lines.push("成员".to_string());
    for (slot, member) in room.members.iter().enumerate() {
        let kind = if member.is_npc { "NPC" } else { "玩家" };
        lines.push(format!(
            "  [{}] {} · 角色 P{} · 武器 {} ({kind})",
            slot, member.name, member.character_id, member.weapon
        ));
    }
    lines.join("\n")
}

fn update_room_members(room: Res<RoomState>, mut query: Query<&mut Text, With<RoomMembersText>>) {
    for mut text in &mut query {
        text.0 = room_members_text(&room);
    }
}

fn replay_pause_button(
    query: Query<&Interaction, (Changed<Interaction>, With<ReplayPauseButton>)>,
    mut player: ResMut<crate::plugins::replay_plugin::ReplayPlayer>,
    mut mrc: ResMut<crate::plugins::replay_plugin::MrcPlayback>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        if mrc.active {
            mrc.paused = !mrc.paused;
        } else if player.active {
            player.paused = !player.paused;
        }
    }
}

fn replay_timeline_scrub(
    tracks: Query<(&Interaction, &bevy::ui::RelativeCursorPosition), With<ReplayTimelineTrack>>,
    mut scrub: ResMut<ReplayScrubState>,
    mut player: ResMut<crate::plugins::replay_plugin::ReplayPlayer>,
    mut mrc: ResMut<crate::plugins::replay_plugin::MrcPlayback>,
    mut seek: ResMut<crate::plugins::replay_plugin::ReplaySeekRequest>,
) {
    let Some((interaction, cursor)) = tracks.iter().next() else {
        scrub.active = false;
        return;
    };
    if !mrc.active {
        scrub.active = false;
        return;
    }
    let target = cursor
        .normalized
        .map(|position| position.x.clamp(0.0, 1.0) * mrc.duration());
    if *interaction == Interaction::Pressed {
        if !scrub.active {
            scrub.active = true;
            scrub.resume_mrc = !mrc.paused;
            scrub.resume_player = player.active && !player.paused;
            mrc.paused = true;
            if player.active {
                player.paused = true;
            }
        }
        if let Some(target) = target {
            seek.0 = Some(target);
        }
    } else if scrub.active {
        if let Some(target) = target {
            seek.0 = Some(target);
        }
        scrub.active = false;
        if scrub.resume_mrc {
            mrc.paused = false;
        }
        if scrub.resume_player {
            player.paused = false;
        }
    }
}

fn replay_space_toggle(
    mut events: MessageReader<bevy::input::keyboard::KeyboardInput>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    console: Option<Res<crate::plugins::debug_plugin::DebugConsoleState>>,
    scrub: Res<ReplayScrubState>,
    mut player: ResMut<crate::plugins::replay_plugin::ReplayPlayer>,
    mut mrc: ResMut<crate::plugins::replay_plugin::MrcPlayback>,
) {
    if console.is_some_and(|console| console.open) {
        events.clear();
        return;
    }
    if scrub.active || (!player.active && !mrc.active) {
        return;
    }
    let toggle = events.read().any(|event| {
        event.key_code == KeyCode::Space
            && event.state == bevy::input::ButtonState::Pressed
            && !event.repeat
    });
    if !toggle {
        return;
    }

    keys.reset(KeyCode::Space);
    if mrc.active {
        mrc.paused = !mrc.paused;
    } else {
        player.paused = !player.paused;
    }
}

fn update_replay_hud(
    player: Res<crate::plugins::replay_plugin::ReplayPlayer>,
    mrc: Res<crate::plugins::replay_plugin::MrcPlayback>,
    mut texts: Query<&mut Text, With<ReplayHudText>>,
    mut buttons: Query<(&Interaction, &mut BackgroundColor), With<ReplayPauseButton>>,
    children: Query<&Children, With<ReplayPauseButton>>,
    mut button_texts: Query<&mut Text, Without<ReplayHudText>>,
    mut fills: Query<&mut Node, With<ReplayTimelineFill>>,
) {
    let (elapsed, duration, paused) = if mrc.active {
        (mrc.elapsed, mrc.duration(), mrc.paused)
    } else {
        (player.elapsed, player.duration(), player.paused)
    };
    for mut text in &mut texts {
        text.0 = format!(
            "回放 {:.1}s / {:.1}s{}",
            elapsed,
            duration,
            if paused { "（暂停）" } else { "" }
        );
    }
    for (_, mut background) in &mut buttons {
        background.0 = if paused {
            Color::srgb(0.4, 0.3, 0.2)
        } else {
            Color::srgb(0.2, 0.3, 0.25)
        };
    }
    for child_entities in &children {
        for child in child_entities {
            if let Ok(mut text) = button_texts.get_mut(*child) {
                text.0 = if paused { "继续" } else { "暂停" }.to_string();
            }
        }
    }
    let progress = if duration > 0.0 {
        (elapsed / duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    for mut node in &mut fills {
        node.width = Val::Percent(progress * 100.0);
    }
}

fn menu_room_button(
    query: Query<&Interaction, (Changed<Interaction>, With<MenuRoomButton>)>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        next_state.set(AppState::Room);
    }
}

fn room_add_npc_button(
    query: Query<&Interaction, (Changed<Interaction>, With<RoomAddNpcButton>)>,
    mut room: ResMut<RoomState>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        let npc_count = room.members.iter().filter(|m| m.is_npc).count();
        if npc_count < MAX_ROOM_NPC {
            room.members.push(RoomMember {
                name: format!("对手{}", npc_count + 1),
                character_id: 1,
                weapon: 6,
                is_npc: true,
            });
        }
    }
}

fn room_remove_npc_button(
    query: Query<&Interaction, (Changed<Interaction>, With<RoomRemoveNpcButton>)>,
    mut room: ResMut<RoomState>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        if let Some(index) = room.members.iter().rposition(|m| m.is_npc) {
            room.members.remove(index);
        }
    }
}

fn room_start_button(
    query: Query<&Interaction, (Changed<Interaction>, With<RoomStartButton>)>,
    mut request: ResMut<LaunchRequest>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        request.spawn_room_npcs = true;
        next_state.set(AppState::Loading);
    }
}

fn room_back_button(
    query: Query<&Interaction, (Changed<Interaction>, With<RoomBackButton>)>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        next_state.set(AppState::Menu);
    }
}

type KeyCaptureButtonQuery<'w, 's> = Query<
    'w,
    's,
    (&'static Interaction, &'static KeyCaptureButton),
    (Changed<Interaction>, With<Button>),
>;

fn key_capture_button_press(
    query: KeyCaptureButtonQuery<'_, '_>,
    mut capture: ResMut<KeyCaptureState>,
) {
    for (interaction, button) in &query {
        if *interaction == Interaction::Pressed {
            if capture.pending == Some(button.0) {
                capture.clear();
            } else {
                capture.begin(button.0);
            }
        }
    }
}

fn key_capture(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    settings: Res<Settings>,
    mut capture: ResMut<KeyCaptureState>,
    mut keymap: ResMut<Keymap>,
) {
    let Some(field) = capture.pending else {
        return;
    };
    if capture.arming {
        if mouse.get_pressed().next().is_some() || keys.get_pressed().next().is_some() {
            return;
        }
        capture.arming = false;
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        capture.cancel_by_esc();
        return;
    }
    if let Some(button) = mouse.get_just_pressed().next() {
        assign_key_binding(&mut keymap, field, InputBinding::Mouse(*button));
        capture.clear();
        crate::app_state::save_settings(&settings, &keymap);
        info!("key bound: {} → {:?}", field.label(), button);
        return;
    }
    if let Some(key) = keys.get_just_pressed().next() {
        assign_key_binding(&mut keymap, field, InputBinding::Key(*key));
        capture.clear();
        crate::app_state::save_settings(&settings, &keymap);
        info!("key bound: {} → {:?}", field.label(), key);
    }
}

fn update_key_capture_buttons(
    keymap: Res<Keymap>,
    capture: Res<KeyCaptureState>,
    buttons: Query<(Entity, &KeyCaptureButton, &Children)>,
    mut texts: Query<&mut Text>,
) {
    for (_, button, children) in &buttons {
        for child in children {
            let Ok(mut text) = texts.get_mut(*child) else {
                continue;
            };

            let listening = capture.pending == Some(button.0);
            let binding = button.0.get(&keymap).label();
            if children.len() >= 2 {
                continue;
            }
            text.0 = if listening {
                format!("{}: 按下新键… (Esc 取消)", button.0.label())
            } else {
                format!("{}: {}", button.0.label(), binding)
            };
        }
    }

    for (_, button, children) in &buttons {
        if children.len() < 2 {
            continue;
        }
        let listening = capture.pending == Some(button.0);
        if let Ok(mut text) = texts.get_mut(children[1]) {
            text.0 = if listening {
                "按下新键…".into()
            } else {
                button.0.get(&keymap).label()
            };
        }
    }
}

fn result_title(bits: i32) -> &'static str {
    if bits & 4 != 0 {
        "胜利"
    } else if bits & 2 != 0 {
        "失败"
    } else if bits & 1 != 0 {
        "平局"
    } else {
        "战局结束"
    }
}

fn result_reason(bits: i32) -> &'static str {
    if bits & 4 != 0 {
        "敌方已全部击败"
    } else if bits & 2 != 0 {
        "我方已无人生还"
    } else if bits & 1 != 0 {
        "任务时限已到"
    } else {
        "战局结束"
    }
}

fn format_elapsed(seconds: f32) -> String {
    let total = seconds.max(0.0) as u32;
    format!("{}:{:02}", total / 60, total % 60)
}

fn setup_result_panel(
    mut commands: Commands,
    ui_font: Res<UiFont>,
    result: Res<MatchEnd>,
    score: Res<KillScore>,
    net: Res<crate::plugins::network_plugin::NetState>,
    request: Res<LaunchRequest>,
) {
    let font = Some(ui_font.0.clone());
    let overlay = commands
        .spawn((
            scrollable_y(Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(14.0),
                padding: UiRect::all(Val::Px(24.0)),
                ..default()
            }),
            BackgroundColor(theme::SCRIM),
            bevy::ui::FocusPolicy::Block,
            UiInputBlocker,
            ResultRoot,
            ResponsiveOverlayRoot,
        ))
        .id();
    let surface = overlay_surface(&mut commands);
    commands.entity(overlay).add_child(surface);
    let title_color = if result.result & 4 != 0 {
        Color::srgb(0.5, 0.9, 0.5)
    } else if result.result & 2 != 0 {
        Color::srgb(1.0, 0.4, 0.35)
    } else {
        theme::TEXT
    };
    let title = text(
        &mut commands,
        result_title(result.result),
        42.0,
        title_color,
        font.clone(),
    );
    commands.entity(title).insert(ResponsiveOverlayTitle(42.0));
    commands.entity(surface).add_child(title);
    let reason = text(
        &mut commands,
        &net.battle_result
            .as_ref()
            .map(|result| super::battle_result::result_heading(&net, result))
            .unwrap_or_else(|| result_reason(result.result).to_owned()),
        20.0,
        Color::srgb(0.8, 0.8, 0.9),
        font.clone(),
    );
    commands.entity(surface).add_child(reason);
    let mut stats_text = format!(
        "击杀  玩家 {} · NPC {}    用时 {}",
        score.player_kills,
        score.npc_kills,
        format_elapsed(result.elapsed)
    );

    if net.battle_result.is_some() {
        stats_text = super::battle_result::score_text(&net, &request, &score);
    }
    let stats = text(&mut commands, &stats_text, 18.0, Color::WHITE, font.clone());
    commands.entity(surface).add_child(stats);
    if net.battle_result.is_none() {
        let retry = menu_button(
            &mut commands,
            "再来一局",
            240.0,
            theme::PRIMARY,
            font.clone(),
        );
        commands.entity(retry).insert(ResultRetryButton);
        commands.entity(surface).add_child(retry);
    }
    let back = menu_button(&mut commands, "返回主菜单", 240.0, theme::CARD, font);
    commands.entity(back).insert(ResultBackToMenuButton);
    commands.entity(surface).add_child(back);
}

fn result_retry_button(
    query: Query<&Interaction, (Changed<Interaction>, With<ResultRetryButton>)>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        next_state.set(AppState::Loading);
    }
}

fn result_back_to_menu_button(
    query: Query<&Interaction, (Changed<Interaction>, With<ResultBackToMenuButton>)>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        next_state.set(AppState::Menu);
    }
}

fn reset_match_state(
    mut score: ResMut<KillScore>,
    mut end: ResMut<MatchEnd>,
    mut lock: ResMut<LockOnController>,
    mut buffs: ResMut<crate::plugins::item_plugin::ItemBuffs>,
) {
    *score = KillScore::default();
    *end = MatchEnd::default();
    lock.system.clear();
    *buffs = crate::plugins::item_plugin::ItemBuffs::default();
}

fn setup_error(
    mut commands: Commands,
    ui_font: Res<UiFont>,
    loaded: Res<crate::app_state::LoadedStageInfo>,
) {
    let font = ui_font.0.clone();
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(20.0),
                left: Val::Px(20.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(10.0),
                ..default()
            },
            LoadingRoot,
        ))
        .id();
    let error = commands
        .spawn((
            Text::new(loaded.error.clone().unwrap_or_else(|| "加载失败".into())),
            TextFont {
                font: font.clone().into(),
                font_size: 24.0.into(),
                ..default()
            },
            TextColor(Color::srgb(1.0, 0.3, 0.3)),
        ))
        .id();
    let hint = commands
        .spawn((
            Text::new("关闭窗口后检查 meteor_rust/assets/origin_res 路径"),
            TextFont {
                font: font.into(),
                font_size: 16.0.into(),
                ..default()
            },
            TextColor(Color::WHITE),
        ))
        .id();
    commands.entity(root).add_child(error).add_child(hint);
}

type ModeButtonQuery<'w, 's> = Query<
    'w,
    's,
    (&'static Interaction, &'static ModeButton),
    (Changed<Interaction>, With<Button>),
>;
type StageButtonQuery<'w, 's> = Query<
    'w,
    's,
    (&'static Interaction, &'static StageButton),
    (Changed<Interaction>, With<Button>),
>;
type CharacterButtonQuery<'w, 's> = Query<
    'w,
    's,
    (&'static Interaction, &'static CharacterButton),
    (Changed<Interaction>, With<Button>),
>;
type WeaponButtonQuery<'w, 's> = Query<
    'w,
    's,
    (&'static Interaction, &'static WeaponButton),
    (Changed<Interaction>, With<Button>),
>;
type ModeVisualQuery<'w, 's> = Query<'w, 's, (&'static ModeButton, &'static mut BackgroundColor)>;
type StageVisualQuery<'w, 's> = Query<'w, 's, (&'static StageButton, &'static mut BackgroundColor)>;
type CharacterVisualQuery<'w, 's> =
    Query<'w, 's, (&'static CharacterButton, &'static mut BackgroundColor)>;
type WeaponVisualQuery<'w, 's> =
    Query<'w, 's, (&'static WeaponButton, &'static mut BackgroundColor)>;

fn menu_mode_buttons(query: ModeButtonQuery<'_, '_>, mut request: ResMut<LaunchRequest>) {
    for (interaction, button) in &query {
        if *interaction == Interaction::Pressed {
            request.mode = button.0.clone();
        }
    }
}

fn menu_stage_buttons(
    query: StageButtonQuery<'_, '_>,
    tab: Res<StageListTab>,
    custom: Res<crate::plugins::stage_plugin::CustomStages>,
    mut memory: ResMut<StageSelectionMemory>,
    mut request: ResMut<LaunchRequest>,
) {
    for (interaction, button) in &query {
        if *interaction == Interaction::Pressed {
            request.stage_id = button.0.clone();
            request.stage_name = match *tab {
                StageListTab::Official => official_stages()
                    .into_iter()
                    .find(|stage| stage.id == button.0)
                    .map(|stage| stage.name),
                StageListTab::Custom => custom
                    .stages
                    .iter()
                    .find(|stage| stage.id == button.0)
                    .map(|stage| stage.name.clone()),
            }
            .unwrap_or_else(|| button.0.clone());
            memory.remember(*tab, &request.stage_id);
        }
    }
}

fn menu_character_buttons(query: CharacterButtonQuery<'_, '_>, mut request: ResMut<LaunchRequest>) {
    for (interaction, button) in &query {
        if *interaction == Interaction::Pressed {
            request.character_id = button.0;
        }
    }
}

fn menu_weapon_buttons(query: WeaponButtonQuery<'_, '_>, mut request: ResMut<LaunchRequest>) {
    for (interaction, button) in &query {
        if *interaction == Interaction::Pressed {
            request.weapon = button.0;
        }
    }
}

fn update_button_visuals(
    mut query: Query<
        (&Interaction, &mut BackgroundColor),
        (
            With<Button>,
            Without<LauncherStyledButton>,
            Without<ResponsiveMenuButton>,
        ),
    >,
) {
    for (interaction, mut background) in &mut query {
        background.0 = match *interaction {
            Interaction::Pressed => PRESSED_BUTTON,
            Interaction::Hovered => HOVERED_BUTTON,
            Interaction::None => NORMAL_BUTTON,
        };
    }
}

fn update_responsive_button_visuals(
    mut query: Query<
        (
            &Interaction,
            &ResponsiveButtonPalette,
            &mut BackgroundColor,
            Option<&mut BackgroundGradient>,
        ),
        (Changed<Interaction>, With<ResponsiveMenuButton>),
    >,
) {
    for (interaction, palette, mut background, gradient) in &mut query {
        background.0 = match *interaction {
            Interaction::Pressed => {
                if palette.0 == theme::PRIMARY {
                    theme::PRIMARY_PRESSED
                } else {
                    PRESSED_BUTTON
                }
            }
            Interaction::Hovered => {
                if palette.0 == theme::PRIMARY {
                    theme::PRIMARY_HOVER
                } else {
                    HOVERED_BUTTON
                }
            }
            Interaction::None => palette.0,
        };
        if let Some(mut gradient) = gradient {
            *gradient = theme::action_gradient(background.0, palette.0 == theme::PRIMARY);
        }
    }
}

pub(crate) fn refresh_selection_visuals(
    request: Res<LaunchRequest>,
    mut buttons: ParamSet<(
        ModeVisualQuery<'_, '_>,
        StageVisualQuery<'_, '_>,
        CharacterVisualQuery<'_, '_>,
        WeaponVisualQuery<'_, '_>,
    )>,
    mut stage_borders: Query<
        (&StageButton, &mut BorderColor),
        (With<LauncherStyledButton>, Without<ModeButton>),
    >,
) {
    for (button, mut background) in &mut buttons.p0() {
        background.0 = if button.0 == request.mode {
            LAUNCHER_CARD_ACTIVE
        } else {
            LAUNCHER_CARD
        };
    }
    for (button, mut background) in &mut buttons.p1() {
        background.0 = if button.0 == request.stage_id {
            LAUNCHER_CARD_ACTIVE
        } else {
            LAUNCHER_CARD
        };
    }
    for (button, mut border) in &mut stage_borders {
        border.set_all(if button.0 == request.stage_id {
            LAUNCHER_RED_BRIGHT
        } else {
            LAUNCHER_LINE
        });
    }
    for (button, mut background) in &mut buttons.p2() {
        background.0 = if button.0 == request.character_id {
            LAUNCHER_CARD_ACTIVE
        } else {
            LAUNCHER_CARD
        };
    }
    for (button, mut background) in &mut buttons.p3() {
        background.0 = if button.0 == request.weapon {
            LAUNCHER_CARD_ACTIVE
        } else {
            LAUNCHER_CARD
        };
    }
}

fn start_game_button(
    query: Query<&Interaction, (Changed<Interaction>, With<StartGameButton>)>,
    tab: Res<StageListTab>,
    custom: Res<crate::plugins::stage_plugin::CustomStages>,
    mut request: ResMut<LaunchRequest>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        let valid_stage = match *tab {
            StageListTab::Official => official_stages()
                .into_iter()
                .any(|stage| stage.id == request.stage_id),
            StageListTab::Custom => custom
                .stages
                .iter()
                .any(|stage| stage.id == request.stage_id),
        };
        if !valid_stage || !super::campaign_ui::prepare_launch(&mut request) {
            warn!("start game ignored: no valid stage selected for {tab:?}");
            return;
        }
        request.mode = "single".into();
        request.spawn_room_npcs = false;
        info!(
            "start game requested: stage={} character=p{} weapon={}",
            request.stage_id, request.character_id, request.weapon
        );
        next_state.set(AppState::Loading);
    }
}

fn back_to_menu_button(
    query: Query<&Interaction, (Changed<Interaction>, With<BackToMenuButton>)>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        next_state.set(AppState::Menu);
    }
}

fn text(
    commands: &mut Commands,
    value: &str,
    size: f32,
    color: Color,
    font: Option<Handle<Font>>,
) -> Entity {
    let mut text_font = TextFont {
        font_size: size.into(),
        ..default()
    };
    if let Some(font) = font {
        text_font.font = font.into();
    }
    commands
        .spawn((Text::new(value), text_font, TextColor(color)))
        .id()
}

fn adapt_runtime_ui_layout(
    window: Query<&Window>,
    mut previous: Local<Option<(Vec2, f32, SafeAreaInsets, Vec2, f32)>>,
    resolved: Option<Res<super::mobile_controls_plugin::ResolvedMobileLayout>>,
    ui_scale: Res<crate::plugins::ui_scale_plugin::EffectiveUiScale>,
    safe_area: Option<Res<SafeAreaInsets>>,
    mut nodes: ParamSet<(
        Query<&mut Node, With<GameplayHudRoot>>,
        Query<&mut Node, With<RadarRoot>>,
        Query<&mut Node, With<ReplayTimelineTrack>>,
        Query<&mut Node, With<ResponsiveOverlayRoot>>,
        Query<&mut Node, With<ResponsiveMenuButton>>,
        Query<&mut Node, With<ResponsiveOverlaySurface>>,
        Query<&mut Node, With<DialoguePanelRoot>>,
        Query<
            (
                &mut Node,
                Option<&HudMeterTrack>,
                Option<&HudVital>,
                Has<HudLoadoutRow>,
                Has<EscortFlagHudIcon>,
            ),
            Or<(
                With<HudMeterTrack>,
                With<HudVital>,
                With<HudLoadoutRow>,
                With<EscortFlagHudIcon>,
            )>,
        >,
    )>,
    mut fixed_fonts: Query<
        (&HudFixedFont, &mut TextFont),
        (Without<HudDetailText>, Without<ResponsiveOverlayTitle>),
    >,
    mut detail_fonts: Query<&mut TextFont, With<HudDetailText>>,
    mut overlay_title_fonts: Query<
        (&ResponsiveOverlayTitle, &mut TextFont),
        Without<HudDetailText>,
    >,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let layout = responsive_ui_layout_with_safe_area(
        window.width(),
        window.height(),
        cfg!(target_os = "android"),
        safe_area.as_deref().copied().unwrap_or_default(),
    );

    let hud = super::combat_hud_layout::CombatHudLayout::new(
        window.width(),
        window.height(),
        safe_area.as_deref().copied().unwrap_or_default(),
        layout.mobile,
    );
    let unit = ui_scale.0.max(0.05);
    let (map_center, map_diameter) = radar_screen_geometry(
        window,
        safe_area.as_deref().copied().unwrap_or_default(),
        resolved.as_deref(),
    );
    let key = (
        Vec2::new(window.width(), window.height()),
        unit,
        safe_area.as_deref().copied().unwrap_or_default(),
        map_center,
        map_diameter,
    );
    let layout_changed = previous.as_ref() != Some(&key);
    *previous = Some(key);
    for mut node in &mut nodes.p0() {
        if !layout_changed && !node.is_added() {
            continue;
        }
        node.left = Val::Px(hud.stats_origin.x / unit);
        node.top = Val::Px(hud.stats_origin.y / unit);
        node.width = Val::Px(hud.status_width / unit);
        node.max_width = Val::Px(hud.status_width / unit);
        node.padding = UiRect::ZERO;
        node.row_gap = Val::Px(3.0 / unit);
    }

    for mut node in &mut nodes.p1() {
        if !layout_changed && !node.is_added() {
            continue;
        }
        node.left = Val::Px((map_center.x - map_diameter * 0.5) / unit);
        node.top = Val::Px((map_center.y - map_diameter * 0.5) / unit);
        node.right = Val::Auto;
        node.bottom = Val::Auto;
    }
    for mut node in &mut nodes.p6() {
        if !layout_changed && !node.is_added() {
            continue;
        }
        node.left = Val::Px(hud.dialogue_origin.x / unit);
        node.top = Val::Px(hud.dialogue_origin.y / unit);
        node.width = Val::Px(hud.dialogue_size.x / unit);
        node.height = Val::Auto;
        node.max_height = Val::Px(hud.dialogue_size.y / unit);
        node.padding = UiRect::axes(Val::Px(16.0 / unit), Val::Px(12.0 / unit));
        node.row_gap = Val::Px(6.0 / unit);
        node.column_gap = Val::Px(12.0 / unit);
        node.border = UiRect::left(Val::Px(2.0 / unit));
    }
    for (mut node, track, label, header, icon) in &mut nodes.p7() {
        if !layout_changed && !node.is_added() {
            continue;
        }
        if track.is_some() {
            node.height = Val::Px(if layout.mobile { 20.0 } else { 22.0 } / unit);
        }
        if label.is_some() {
            node.left = Val::Auto;
            node.top = Val::Auto;
        }
        if header {
            node.height = Val::Px(20.0 / unit);
            node.column_gap = Val::Px(6.0 / unit);
            node.border = UiRect::left(Val::Px(2.0 / unit));
            node.padding = UiRect::left(Val::Px(6.0 / unit));
        }
        if icon {
            node.width = Val::Px(16.0 / unit);
            node.height = Val::Px(16.0 / unit);
        }
    }
    for (size, mut font) in &mut fixed_fonts {
        if !layout_changed && !font.is_added() {
            continue;
        }
        let pixels = if layout.mobile && size.0 == 13.0 {
            12.0
        } else {
            size.0
        };
        let next = (pixels / unit).into();
        if font.font_size != next {
            font.font_size = next;
        }
    }
    for mut node in &mut nodes.p2() {
        if !layout_changed && !node.is_added() {
            continue;
        }
        node.width = Val::Px(if layout.mobile {
            (window.width() * 0.46).clamp(220.0, 360.0)
        } else {
            360.0
        });
        node.height = Val::Px(if layout.mobile { 24.0 } else { 16.0 });
    }
    for mut node in &mut nodes.p3() {
        if !layout_changed && !node.is_added() {
            continue;
        }
        node.padding = UiRect::new(
            Val::Px(if layout.mobile {
                layout.left_gutter
            } else {
                24.0
            }),
            Val::Px(if layout.mobile {
                layout.right_gutter
            } else {
                24.0
            }),
            Val::Px(if layout.mobile {
                layout.top_gutter
            } else {
                24.0
            }),
            Val::Px(if layout.mobile {
                layout.bottom_gutter
            } else {
                24.0
            }),
        );
        node.row_gap = Val::Px(if layout.mobile { 8.0 } else { 12.0 });
        node.justify_content = if layout.mobile && window.height() < 560.0 || ui_scale.0 < 0.999 {
            JustifyContent::FlexStart
        } else {
            JustifyContent::Center
        };
    }
    for mut node in &mut nodes.p4() {
        if !layout_changed && !node.is_added() {
            continue;
        }
        node.height = Val::Px(if layout.mobile { 44.0 / unit } else { 40.0 });
        node.min_height = node.height;
        node.max_height = node.height;
        if layout.mobile {
            node.width = Val::Px((window.width() * 0.46).clamp(168.0, 220.0));
            node.padding = UiRect::horizontal(Val::Px(10.0));
        }
    }
    for mut node in &mut nodes.p5() {
        if !layout_changed && !node.is_added() {
            continue;
        }
        node.width = if layout.mobile {
            Val::Percent(100.0)
        } else {
            Val::Px(420.0)
        };
        node.max_width = Val::Px(if layout.mobile { 560.0 } else { 420.0 });
        node.padding = UiRect::all(Val::Px(if layout.mobile { 14.0 } else { 24.0 }));
        node.row_gap = Val::Px(if layout.mobile { 8.0 } else { 12.0 });
    }
    for mut font in &mut detail_fonts {
        if !layout_changed && !font.is_added() {
            continue;
        }
        font.font_size = if layout.mobile { 14.0 } else { 16.0 }.into();
    }
    for (title, mut font) in &mut overlay_title_fonts {
        if !layout_changed && !font.is_added() {
            continue;
        }
        font.font_size = if layout.mobile {
            if title.0 >= 40.0 {
                28.0
            } else {
                24.0
            }
        } else {
            title.0
        }
        .into();
    }
}

fn overlay_surface(commands: &mut Commands) -> Entity {
    commands
        .spawn((
            Node {
                width: Val::Px(420.0),
                max_width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(24.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(8.0)),
                ..default()
            },
            BackgroundColor(theme::SURFACE),
            BorderColor::all(theme::LINE),
            ResponsiveOverlaySurface,
        ))
        .id()
}

fn menu_button(
    commands: &mut Commands,
    label: &str,
    width: f32,
    color: Color,
    font: Option<Handle<Font>>,
) -> Entity {
    let entity = commands
        .spawn((
            Button,
            Node {
                width: Val::Px(width),
                height: Val::Px(40.0),
                min_height: Val::Px(40.0),
                max_height: Val::Px(40.0),
                flex_shrink: 0.0,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                ..default()
            },
            BackgroundColor(color),
            theme::action_gradient(color, color == theme::PRIMARY),
            BorderColor::all(if color == theme::PRIMARY {
                theme::ACCENT
            } else {
                theme::LINE
            }),
            Interaction::default(),
            ResponsiveMenuButton,
            ResponsiveButtonPalette(color),
        ))
        .id();
    let child = text(commands, label, 16.0, Color::WHITE, font);
    commands.entity(entity).add_child(child);
    entity
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_game_menu_is_direct_and_pause_requires_confirmation() {
        for state in [AppState::Menu, AppState::Paused] {
            for confirm in [false, true] {
                let mut app = App::new();
                app.insert_resource(State::new(state))
                    .init_resource::<MenuScreen>()
                    .insert_resource(UiFont(Handle::default()))
                    .add_message::<AppExit>()
                    .add_systems(Update, exit_game_buttons);
                app.world_mut().spawn(PauseRoot);
                app.world_mut()
                    .spawn((ExitGameButton, Interaction::Pressed));
                app.update();
                let exits = app.world().resource::<Messages<AppExit>>().len();
                if state == AppState::Menu {
                    assert_eq!(exits, 1);
                    assert_eq!(
                        app.world_mut()
                            .query::<&ExitConfirmation>()
                            .iter(app.world())
                            .count(),
                        0
                    );
                } else {
                    assert_eq!(exits, 0);
                    assert_eq!(
                        app.world_mut()
                            .query::<&ExitConfirmation>()
                            .iter(app.world())
                            .count(),
                        1
                    );
                    app.world_mut()
                        .spawn((ExitConfirmationChoice(confirm), Interaction::Pressed));
                    app.update();
                    assert_eq!(
                        app.world().resource::<Messages<AppExit>>().len(),
                        usize::from(confirm)
                    );
                    assert_eq!(
                        app.world_mut()
                            .query::<&ExitConfirmation>()
                            .iter(app.world())
                            .count(),
                        0
                    );
                }
            }
        }
    }

    #[test]
    fn exit_game_escape_cancels_without_resuming() {
        let mut app = App::new();
        app.insert_resource(State::new(AppState::Paused))
            .init_resource::<NextState<AppState>>()
            .init_resource::<KeyCaptureState>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Update, toggle_pause);
        app.world_mut().spawn(ExitConfirmation);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        assert!(matches!(
            app.world().resource::<NextState<AppState>>(),
            NextState::Unchanged
        ));
        assert_eq!(
            app.world_mut()
                .query::<&ExitConfirmation>()
                .iter(app.world())
                .count(),
            0
        );
    }

    #[test]
    fn native_text_console_space_does_not_toggle_replay_or_replay_after_close() {
        use crate::plugins::debug_plugin::DebugConsoleState;
        use crate::plugins::replay_plugin::{MrcPlayback, ReplayPlayer};
        use bevy::input::keyboard::{Key, KeyboardInput};
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ReplayScrubState>()
            .init_resource::<MrcPlayback>()
            .insert_resource(ReplayPlayer {
                active: true,
                ..default()
            })
            .insert_resource(DebugConsoleState {
                open: true,
                ..default()
            })
            .add_message::<KeyboardInput>()
            .add_systems(Update, replay_space_toggle);
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Space,
            logical_key: Key::Space,
            state: bevy::input::ButtonState::Pressed,
            text: Some(" ".into()),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(!app.world().resource::<ReplayPlayer>().paused);
        app.world_mut().resource_mut::<DebugConsoleState>().open = false;
        app.update();
        assert!(!app.world().resource::<ReplayPlayer>().paused);
    }

    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    #[test]
    #[ignore = "requires a local graphics adapter; writes a review image"]
    fn cinematic_frontend_gpu_capture() {
        use bevy::app::ScheduleRunnerPlugin;
        use bevy::camera::RenderTarget;
        use bevy::render::render_resource::TextureFormat;
        use bevy::render::view::screenshot::{save_to_disk, Screenshot, ScreenshotCaptured};
        #[derive(Resource)]
        struct CaptureTarget(Handle<Image>);
        fn prepare(
            mut commands: Commands,
            mut images: ResMut<Assets<Image>>,
            mut fonts: ResMut<Assets<Font>>,
        ) {
            let (width, height) =
                if std::env::var("METEOR_UI_CAPTURE").is_ok_and(|v| v.ends_with("portrait")) {
                    (412, 915)
                } else if std::env::var("METEOR_UI_CAPTURE")
                    .as_deref()
                    .is_ok_and(|v| v.ends_with("mobile"))
                {
                    (872, 393)
                } else {
                    (1280, 720)
                };
            let target = images.add(Image::new_target_texture(
                width,
                height,
                TextureFormat::Rgba8UnormSrgb,
                None,
            ));
            commands.spawn((
                Camera2d,
                RenderTarget::Image(target.clone().into()),
                IsDefaultUiCamera,
            ));
            commands.spawn(Window {
                resolution: (width, height).into(),
                ..default()
            });
            commands.insert_resource(CaptureTarget(target));
            let bytes = std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/ui.ttf"),
            )
            .unwrap();
            commands.insert_resource(UiFont(fonts.add(Font::from_bytes(bytes))));
        }
        fn capture(mut commands: Commands, target: Res<CaptureTarget>, mut frame: Local<u32>) {
            *frame += 1;
            let motion = std::env::var("METEOR_UI_CAPTURE").as_deref() == Ok("hud-motion");
            if (!motion && *frame == 45)
                || (motion && (30..=90).contains(&*frame) && *frame % 3 == 0)
            {
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
                    if std::env::var("METEOR_UI_CAPTURE").as_deref() == Ok("hud") {
                        "../../../output/ui-concepts/A-dialogue-live-desktop.png"
                    } else if std::env::var("METEOR_UI_CAPTURE").as_deref() == Ok("hud-mobile") {
                        "../../../output/ui-concepts/A-dialogue-live-mobile.png"
                    } else if std::env::var("METEOR_UI_CAPTURE").as_deref() == Ok("loading") {
                        "../../../output/ui-concepts/A-loading-fog.png"
                    } else if std::env::var("METEOR_UI_CAPTURE").as_deref() == Ok("network") {
                        "../../../output/ui-concepts/A-aligned-native-network.png"
                    } else if std::env::var("METEOR_UI_CAPTURE").as_deref() == Ok("mobile") {
                        "../../../output/ui-concepts/A-calligraphy-mobile.png"
                    } else if std::env::var("METEOR_UI_CAPTURE").as_deref() == Ok("campaign-mobile")
                    {
                        "../../../output/ui-concepts/campaign-map-mobile.png"
                    } else if std::env::var("METEOR_UI_CAPTURE").as_deref() == Ok("campaign") {
                        "../../../output/ui-concepts/campaign-map-desktop.png"
                    } else {
                        "../../../output/ui-concepts/A-calligraphy-menu.png"
                    },
                );
                let path = std::env::var("METEOR_UI_CAPTURE_PATH")
                    .map(std::path::PathBuf::from)
                    .unwrap_or(path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                let path = if motion {
                    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../../output/ui-concepts/dialogue-motion-frames");
                    std::fs::create_dir_all(&dir).unwrap();
                    dir.join(format!("{:03}.png", (*frame - 30) / 3))
                } else {
                    path
                };
                let finish = !motion || *frame == 90;
                commands
                    .spawn(Screenshot::image(target.0.clone()))
                    .observe(save_to_disk(path))
                    .observe(
                        move |_: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
                            if finish {
                                exit.write(AppExit::Success);
                            }
                        },
                    );
            }
            assert!(*frame < 600, "GPU capture timed out");
        }
        fn lobby_mode() -> bool {
            std::env::var("METEOR_UI_CAPTURE").is_ok_and(|v| v.starts_with("lobby"))
        }
        fn lobby_fixture(
            mut net: ResMut<crate::plugins::network_plugin::NetState>,
            mut request: ResMut<LaunchRequest>,
        ) {
            net.status = crate::plugins::network_plugin::NetStatus::InRoom;
            net.transport_connected = true;
            net.player_id = "guest".into();
            net.room_id = "江湖夜雨".into();
            net.stage = "sn01".into();
            net.mode = "deathmatch".into();
            net.lobby.owner_player_id = Some("host".into());
            net.lobby.configured = Some(true);
            net.members = [
                ("host", "江湖故人", 0, 6, 1, false),
                ("guest", "孟星魂", 0, 5, 2, false),
                ("peer", "冷燕", 1, 7, 1, true),
                ("peer2", "无名", 3, 8, 2, false),
            ]
            .into_iter()
            .map(|(id, name, character_id, weapon, team, ready)| {
                meteor_network::protocol::RoomMemberWire {
                    player_id: id.into(),
                    name: name.into(),
                    character_id,
                    weapon,
                    team: Some(team),
                    ready,
                    connected: true,
                }
            })
            .collect();
            request.character_id = 0;
            request.weapon = 5;
        }
        fn hud_mode() -> bool {
            std::env::var("METEOR_UI_CAPTURE").is_ok_and(|v| v.starts_with("hud"))
        }
        fn fog_mode() -> bool {
            std::env::var("METEOR_UI_CAPTURE").as_deref() == Ok("loading")
        }
        fn hud_fixture(
            mut commands: Commands,
            font: Res<UiFont>,
            mut dialogue: ResMut<DialogueState>,
            mut next: ResMut<NextState<AppState>>,
        ) {
            next.set(AppState::Playing);
            commands.spawn((
                Fighter {
                    character_id: 1,
                    player_slot: 0,
                    weapon: 5,
                    weapon_slots: [5, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health {
                    current: 1500.0,
                    max: 1500.0,
                },
                Qi {
                    current: 30.0,
                    max: 100.0,
                },
                Transform::default(),
                crate::plugins::stage_plugin::LocalFighter,
                crate::components::fighter::MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
            ));
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
            let gmb = std::fs::read(root.join("sn01/sn01.gmb")).unwrap();
            let gmdl = meteor_asset::gmdl::decode_gmdl(&gmb).unwrap();
            let des = std::fs::read(root.join("sn01/sn01.des")).unwrap();
            let desc =
                meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(&des));
            commands.insert_resource(crate::plugins::stage_collider::StageCollider {
                mesh: Some(
                    crate::plugins::stage_collider::StageMeshCollider::from_gmdl(
                        &gmdl, &desc, None, [0.0; 3],
                    ),
                ),
            });
            dialogue.text = Some("冷燕：前面有守卫，小心行事。".into());
            dialogue.timer = 30.0;
            commands.spawn((
                Text::new("HUD 布局验收样例"),
                TextFont {
                    font: font.0.clone().into(),
                    font_size: 12.0.into(),
                    ..default()
                },
                TextColor(theme::MUTED),
                Node {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(16.0),
                    left: Val::Px(16.0),
                    ..default()
                },
            ));
        }
        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .set(bevy::window::WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                })
                .disable::<bevy::winit::WinitPlugin>()
                .disable::<bevy::audio::AudioPlugin>(),
        )
        .add_plugins(ScheduleRunnerPlugin::run_loop(
            std::time::Duration::from_millis(16),
        ))
        .init_state::<AppState>()
        .init_resource::<crate::plugins::network_plugin::NetState>()
        .init_resource::<crate::plugins::stage_collider::StageCollider>()
        .init_resource::<RadarAssets>()
        .init_resource::<RadarState>()
        .init_resource::<RadarTerrain>()
        .init_resource::<DialogueState>()
        .init_resource::<DialoguePresentation>()
        .init_resource::<crate::plugins::replay_plugin::ReplayPlayer>()
        .init_resource::<crate::plugins::replay_plugin::MrcPlayback>()
        .init_resource::<LaunchRequest>()
        .init_resource::<Settings>()
        .init_resource::<Keymap>()
        .init_resource::<crate::plugins::stage_plugin::CustomStages>()
        .init_resource::<StageListTab>()
        .insert_resource(
            if std::env::var("METEOR_UI_CAPTURE").is_ok_and(|v| v.starts_with("network")) {
                MenuScreen::Network
            } else if std::env::var("METEOR_UI_CAPTURE").is_ok_and(|v| v.starts_with("campaign")) {
                MenuScreen::Stages
            } else {
                MenuScreen::Main
            },
        )
        .init_resource::<crate::plugins::ui_scale_plugin::EffectiveUiScale>()
        .add_plugins((
            theme::CinematicThemePlugin,
            super::super::campaign_ui::CampaignUiPlugin,
            crate::plugins::ui_components::UiComponentsPlugin,
        ))
        .add_plugins((
            super::super::loading_ui::LoadingUiPlugin,
            super::super::dialogue_portrait::DialoguePortraitPlugin,
            super::super::stage_material::GmdlMaterialPlugin,
        ))
        .add_systems(
            Startup,
            (
                prepare,
                setup_menu.run_if(|| !hud_mode() && !fog_mode() && !lobby_mode()),
                lobby_fixture.run_if(lobby_mode),
                setup_net_room_panel.run_if(lobby_mode),
                setup_playing_hud.run_if(hud_mode),
                hud_fixture.run_if(hud_mode),
                super::super::character_plugin::spawn_live_portrait_fixture.run_if(hud_mode),
                super::super::loading_ui::setup.run_if(fog_mode),
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                adapt_runtime_ui_layout,
                super::super::character_plugin::animate_live_portrait_fixture,
                update_hud_stats,
                update_dialogue_hud,
                update_gameplay_pause,
                prepare_radar_terrain,
                update_radar,
            )
                .chain()
                .run_if(hud_mode),
        )
        .add_systems(
            Update,
            (
                sync_launcher_screen_visibility,
                adapt_launcher_layout,
                capture,
            )
                .chain(),
        );
        if lobby_mode() {
            app.add_systems(
                Update,
                (
                    adapt_runtime_ui_layout,
                    adapt_net_room_layout,
                    update_net_room_cards,
                    crate::plugins::network_plugin::room_lobby::update,
                    crate::plugins::network_plugin::net_update_team_select_buttons,
                )
                    .chain(),
            );
        }
        if std::env::var("METEOR_UI_CAPTURE").is_ok_and(|v| v.starts_with("network")) {
            crate::plugins::network_plugin::room_browser::install_capture_fixture(&mut app);
        }
        assert_eq!(app.run(), AppExit::Success);
    }

    #[test]
    fn live_portrait_tracks_head_and_releases_render_layer_after_dialogue() {
        use bevy::camera::visibility::RenderLayers;
        use bevy::ecs::system::RunSystemOnce;
        use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
        let (mut app, font) = hud_text_layout_app(1280, 720);
        app.add_plugins(bevy::transform::TransformPlugin)
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<crate::plugins::stage_material::GmdlMaterial>>()
            .init_resource::<Assets<SkinnedMeshInverseBindposes>>()
            .insert_resource(UiFont(font))
            .init_resource::<LaunchRequest>()
            .insert_resource(State::new(AppState::Playing))
            .insert_resource(DialogueState {
                text: Some("冷燕：前面有守卫".into()),
                timer: 10.0,
            })
            .insert_resource(DialoguePresentation {
                source: "冷燕：前面有守卫".into(),
                speaker: "冷燕".into(),
                ..default()
            })
            .add_plugins(super::super::dialogue_portrait::DialoguePortraitPlugin);
        let fighter = app
            .world_mut()
            .spawn((
                Fighter {
                    character_id: 1,
                    player_slot: 0,
                    weapon: 5,
                    weapon_slots: [5, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                crate::plugins::stage_plugin::LocalFighter,
                crate::components::fighter::MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                Transform::default(),
            ))
            .id();
        app.world_mut()
            .run_system_once(super::super::character_plugin::spawn_live_portrait_fixture)
            .unwrap();
        let window = app
            .world_mut()
            .spawn(super::super::dialogue_portrait::window_node())
            .id();
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(
            app.world().get::<Node>(window).unwrap().display,
            Display::Flex
        );
        let tagged: Vec<Entity> = app
            .world_mut()
            .query::<(Entity, &RenderLayers)>()
            .iter(app.world())
            .filter(|(_, layers)| layers.iter().any(|l| l == 31) && layers.iter().any(|l| l == 0))
            .map(|(e, _)| e)
            .collect();
        assert!(
            !tagged.is_empty(),
            "speaker remains visible in gameplay and portrait layers"
        );
        let camera = app
            .world_mut()
            .query::<(Entity, &Camera)>()
            .iter(app.world())
            .find(|(_, camera)| camera.order == -1)
            .map(|(e, _)| e)
            .unwrap();
        assert!(app.world().get::<Camera>(camera).unwrap().is_active);
        let old = app.world().get::<Transform>(camera).unwrap().translation;
        app.world_mut()
            .get_mut::<Transform>(fighter)
            .unwrap()
            .translation += Vec3::X * 2.0;
        app.update();
        assert!(
            (app.world().get::<Transform>(camera).unwrap().translation - old - Vec3::X * 2.0)
                .length()
                < 0.001
        );
        app.world_mut().resource_mut::<DialogueState>().text = None;
        app.update();
        assert!(!app.world().get::<Camera>(camera).unwrap().is_active);
        assert_eq!(
            app.world().get::<Node>(window).unwrap().display,
            Display::None
        );
        for entity in tagged {
            let layers = app.world().get::<RenderLayers>(entity).unwrap();
            assert!(layers.iter().any(|l| l == 0));
            assert!(!layers.iter().any(|l| l == 31));
        }
    }

    #[test]
    fn hud_only_displays_local_vitals_even_with_npcs_and_lock_target() {
        let mut app = App::new();
        app.init_resource::<LockOnController>()
            .add_systems(Update, update_hud_stats);
        let text = app.world_mut().spawn((HudVital::Qi, Text::default())).id();
        let loadout = app
            .world_mut()
            .spawn((HudLoadoutText::Active, Text::default()))
            .id();
        let spare = app
            .world_mut()
            .spawn((HudLoadoutText::Spare, Text::default()))
            .id();
        let fill = app
            .world_mut()
            .spawn((HudVitalFill(HudVital::Qi), Node::default()))
            .id();
        let fighter = |slot| Fighter {
            character_id: 1,
            player_slot: slot,
            weapon: 6,
            weapon_slots: [6, 0],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        };
        let npc = app
            .world_mut()
            .spawn((
                fighter(0),
                Health::default(),
                Qi {
                    current: 75.0,
                    max: 100.0,
                },
            ))
            .id();
        app.world_mut().spawn((
            fighter(4),
            crate::plugins::stage_plugin::LocalFighter,
            Health::default(),
            Qi {
                current: 100.0,
                max: 100.0,
            },
        ));
        app.world_mut().spawn((
            fighter(3),
            crate::plugins::network_plugin::RemoteFighter {
                player_id: "stock-3".into(),
            },
            Health::default(),
            Qi::default(),
        ));
        app.world_mut()
            .resource_mut::<LockOnController>()
            .lock_entity = Some(npc);
        app.update();
        let label = &app.world().get::<Text>(text).unwrap().0;
        assert_eq!(label, "100 / 100");
        assert_eq!(
            app.world().get::<Node>(fill).unwrap().width,
            Val::Percent(100.0)
        );
        assert_eq!(app.world().get::<Text>(loadout).unwrap().0, "轻剑");
        assert_eq!(app.world().get::<Text>(spare).unwrap().0, "备 · —");
        assert!(!label.contains("NPC") && !label.contains("远端") && !label.contains("75"));
        app.world_mut().get_mut::<Qi>(npc).unwrap().current = 15.0;
        app.update();
        assert!(!app.world().get::<Text>(text).unwrap().0.contains("15"));
        let local = app
            .world_mut()
            .query_filtered::<Entity, With<crate::plugins::stage_plugin::LocalFighter>>()
            .single(app.world())
            .unwrap();
        {
            let mut fighter = app.world_mut().get_mut::<Fighter>(local).unwrap();
            fighter.weapon_slots = [6, 7];
        }
        app.update();
        assert_eq!(app.world().get::<Text>(spare).unwrap().0, "备 · 长枪");
        {
            let mut fighter = app.world_mut().get_mut::<Fighter>(local).unwrap();
            fighter.weapon = 7;
            fighter.weapon_slot = 1;
        }
        app.world_mut()
            .entity_mut(local)
            .insert(crate::components::fighter::EscortFlagCarrier);
        app.world_mut().get_mut::<Qi>(local).unwrap().current = 50.0;
        app.update();
        assert_eq!(app.world().get::<Text>(loadout).unwrap().0, "长枪");
        assert_eq!(app.world().get::<Text>(spare).unwrap().0, "备 · 轻剑");
        {
            let mut fighter = app.world_mut().get_mut::<Fighter>(local).unwrap();
            assert!(crate::plugins::item_plugin::drop_spare_weapon(&mut fighter).is_some());
        }
        app.update();
        assert_eq!(app.world().get::<Text>(spare).unwrap().0, "备 · —");
        assert_eq!(
            app.world().get::<Node>(fill).unwrap().width,
            Val::Percent(50.0)
        );
        app.world_mut()
            .entity_mut(local)
            .remove::<crate::components::fighter::EscortFlagCarrier>();
        app.update();
        assert_eq!(app.world().get::<Text>(loadout).unwrap().0, "长枪");
        assert_eq!(hud_fill_ratio(10.0, 0.0), 0.0);
        assert_eq!(hud_fill_ratio(f32::NAN, 100.0), 0.0);
        assert_eq!(hud_fill_ratio(200.0, 100.0), 1.0);
    }

    #[test]
    fn dialogue_hud_pages_unicode_and_clears_after_last_page() {
        let body = "甲乙丙丁戊己庚辛壬癸".repeat(12);
        let pages = dialogue_pages(&body, 9, 2);
        assert_eq!(pages.concat().replace('\n', ""), body);
        assert!(pages
            .iter()
            .all(|page| page.lines().count() <= 2
                && page.lines().all(|line| line.chars().count() <= 9)));
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<DialogueState>()
            .init_resource::<DialoguePresentation>()
            .add_systems(Update, update_dialogue_hud);
        let speaker = app
            .world_mut()
            .spawn((DialogueSpeakerText, Text::default()))
            .id();
        app.world_mut().spawn((DialogueHudText, Text::default()));
        let panel = app
            .world_mut()
            .spawn((DialoguePanelRoot, Visibility::Hidden))
            .id();
        {
            let mut state = app.world_mut().resource_mut::<DialogueState>();
            state.text = Some(format!("冷燕：{body}"));
            state.timer = 3.0;
        }
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(panel).unwrap(),
            Visibility::Visible
        );
        assert!(app
            .world()
            .get::<Text>(speaker)
            .unwrap()
            .0
            .starts_with("冷燕"));
        assert!(app.world().resource::<DialoguePresentation>().pages.len() > 1);
        let duration = app.world().resource::<DialoguePresentation>().page_duration;
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(duration + 0.1));
        app.update();
        assert_eq!(app.world().resource::<DialoguePresentation>().page, 1);
        {
            let mut state = app.world_mut().resource_mut::<DialogueState>();
            state.text = Some("孟星魂：走吧".into());
            state.timer = 3.0;
        }
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::ZERO);
        app.update();
        assert_eq!(app.world().resource::<DialoguePresentation>().page, 0);
        assert_eq!(app.world().get::<Text>(speaker).unwrap().0, "孟星魂");
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs(10));
        app.update();
        assert!(app.world().resource::<DialogueState>().text.is_none());
        assert_eq!(
            *app.world().get::<Visibility>(panel).unwrap(),
            Visibility::Hidden
        );
    }

    #[test]
    fn hud_starts_with_fighter_vitals_without_map_or_mode_title() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<RadarAssets>()
            .insert_resource(UiFont(Handle::default()))
            .init_resource::<LaunchRequest>()
            .init_resource::<crate::plugins::replay_plugin::ReplayPlayer>()
            .init_resource::<crate::plugins::replay_plugin::MrcPlayback>()
            .add_systems(Update, setup_playing_hud);
        app.update();
        let world = app.world_mut();
        let root = world
            .query_filtered::<Entity, With<GameplayHudRoot>>()
            .single(world)
            .unwrap();
        let children = world.get::<Children>(root).unwrap();
        let stats = children
            .iter()
            .position(|child| world.get::<HudLoadoutRow>(child).is_some())
            .unwrap();
        assert_eq!(stats, 2, "vital rows precede weapon status");
        assert_eq!(world.query::<&HudVitalFill>().iter(world).count(), 2);
        let dialogue = world
            .query_filtered::<Entity, With<DialoguePanelRoot>>()
            .single(world)
            .unwrap();
        assert!(
            world.get::<ChildOf>(dialogue).is_none(),
            "dialogue must be independent from status"
        );
        assert_eq!(
            *world.get::<Visibility>(dialogue).unwrap(),
            Visibility::Hidden
        );
    }

    #[test]
    fn pause_overlay_uses_a_blocking_card_surface() {
        let mut app = App::new();
        app.insert_resource(UiFont(Handle::default()))
            .add_systems(Update, setup_pause_menu);
        app.update();

        let blockers = app
            .world_mut()
            .query_filtered::<Entity, With<UiInputBlocker>>()
            .iter(app.world())
            .count();
        let surfaces = app
            .world_mut()
            .query_filtered::<Entity, With<ResponsiveOverlaySurface>>()
            .iter(app.world())
            .count();
        let actions = app
            .world_mut()
            .query_filtered::<Entity, With<ResponsiveMenuButton>>()
            .iter(app.world())
            .count();
        assert_eq!(blockers, 1);
        assert_eq!(surfaces, 1);
        assert_eq!(actions, 3);
    }

    #[test]
    fn runtime_layout_cache_handles_late_nodes_resize_scale_and_safe_area() {
        #[derive(Resource, Default)]
        struct Writes(usize, usize);
        let mut app = App::new();
        app.insert_resource(crate::plugins::ui_scale_plugin::EffectiveUiScale(1.0))
            .init_resource::<SafeAreaInsets>()
            .init_resource::<Writes>()
            .add_systems(
                Update,
                (
                    adapt_runtime_ui_layout,
                    |nodes: Query<(), Changed<Node>>,
                     fonts: Query<(), Changed<TextFont>>,
                     mut writes: ResMut<Writes>| {
                        *writes = Writes(nodes.iter().count(), fonts.iter().count());
                    },
                )
                    .chain(),
            );
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: (872_u32, 393_u32).into(),
                ..default()
            })
            .id();
        let hud = app
            .world_mut()
            .spawn((Node::default(), GameplayHudRoot))
            .id();
        let button = app
            .world_mut()
            .spawn((Node::default(), ResponsiveMenuButton))
            .id();
        let text = app
            .world_mut()
            .spawn((TextFont::default(), HudFixedFont(13.0)))
            .id();
        app.update();
        for _ in 0..5 {
            app.update();
            let writes = app.world().resource::<Writes>();
            assert_eq!(
                (writes.0, writes.1),
                (0, 0),
                "steady HUD must not invalidate layout/text"
            );
        }
        let late = app
            .world_mut()
            .spawn((Node::default(), ResponsiveMenuButton))
            .id();
        let late_text = app
            .world_mut()
            .spawn((TextFont::default(), HudFixedFont(13.0)))
            .id();
        app.update();
        assert_eq!(app.world().get::<Node>(late).unwrap().height, Val::Px(44.0));
        assert_eq!(
            app.world().get::<TextFont>(late_text).unwrap().font_size,
            app.world().get::<TextFont>(text).unwrap().font_size
        );
        app.world_mut()
            .resource_mut::<crate::plugins::ui_scale_plugin::EffectiveUiScale>()
            .0 = 0.5;
        app.update();
        assert_eq!(
            app.world().get::<Node>(button).unwrap().height,
            Val::Px(88.0)
        );
        assert_eq!(
            app.world().get::<TextFont>(text).unwrap().font_size,
            24.0.into()
        );
        let old_left = app.world().get::<Node>(hud).unwrap().left;
        app.world_mut().resource_mut::<SafeAreaInsets>().left = 40.0;
        app.update();
        assert_ne!(app.world().get::<Node>(hud).unwrap().left, old_left);
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set(1920.0, 1080.0);
        app.update();
        assert_eq!(
            app.world().get::<Node>(button).unwrap().height,
            Val::Px(40.0)
        );
        assert_eq!(
            app.world().get::<TextFont>(text).unwrap().font_size,
            26.0.into()
        );
        app.update();
        let writes = app.world().resource::<Writes>();
        assert_eq!((writes.0, writes.1), (0, 0));
    }

    #[test]
    fn short_landscape_compacts_overlay_surface_and_actions() {
        let mut app = App::new();
        app.insert_resource(crate::plugins::ui_scale_plugin::EffectiveUiScale(1.0))
            .add_systems(Update, adapt_runtime_ui_layout);
        app.world_mut().spawn(Window {
            resolution: (872_u32, 393_u32).into(),
            ..default()
        });
        let root = app
            .world_mut()
            .spawn((Node::default(), ResponsiveOverlayRoot))
            .id();
        let surface = app
            .world_mut()
            .spawn((Node::default(), ResponsiveOverlaySurface))
            .id();
        let button = app
            .world_mut()
            .spawn((Node::default(), ResponsiveMenuButton))
            .id();
        app.update();

        assert_eq!(
            app.world().get::<Node>(root).unwrap().justify_content,
            JustifyContent::FlexStart
        );
        assert_eq!(
            app.world().get::<Node>(surface).unwrap().width,
            Val::Percent(100.0)
        );
        assert_eq!(
            app.world().get::<Node>(surface).unwrap().padding.left,
            Val::Px(14.0)
        );
        assert_eq!(
            app.world().get::<Node>(button).unwrap().min_height,
            Val::Px(44.0)
        );
    }

    #[test]
    fn responsive_buttons_keep_their_semantic_idle_color() {
        let semantic = theme::PRIMARY;
        let mut app = App::new();
        app.add_systems(Update, update_responsive_button_visuals);
        let button = app
            .world_mut()
            .spawn((
                Button,
                Interaction::None,
                BackgroundColor(Color::BLACK),
                ResponsiveMenuButton,
                ResponsiveButtonPalette(semantic),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<BackgroundColor>(button).unwrap().0,
            semantic
        );

        *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::Hovered;
        app.update();
        assert_eq!(
            app.world().get::<BackgroundColor>(button).unwrap().0,
            theme::PRIMARY_HOVER
        );
    }

    #[test]
    fn result_title_maps_bits() {
        assert_eq!(result_title(4), "胜利");
        assert_eq!(result_title(2), "失败");
        assert_eq!(result_title(1), "平局");
        assert_eq!(result_title(4 | 2), "胜利");
        assert_eq!(result_title(0), "战局结束");
        assert_eq!(result_reason(4), "敌方已全部击败");
        assert_eq!(result_reason(2), "我方已无人生还");
        assert_eq!(result_reason(1), "任务时限已到");
    }

    #[test]
    fn match_result_banner_announces_single_player_result_before_settlement() {
        let mut app = App::new();
        app.init_resource::<MatchEnd>()
            .init_resource::<LaunchRequest>()
            .insert_resource(State::new(AppState::Playing))
            .add_systems(Update, update_match_result_banner);
        let banner = app
            .world_mut()
            .spawn((Text::new(""), Node::default(), MatchResultBanner))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Node>(banner).unwrap().display,
            Display::None
        );
        for (result, title) in [(4, "胜利"), (2, "失败"), (1, "平局")] {
            *app.world_mut().resource_mut::<MatchEnd>() = MatchEnd {
                result,
                settled: true,
                ..default()
            };
            app.update();
            assert_eq!(
                app.world().get::<Node>(banner).unwrap().display,
                Display::Flex
            );
            assert_eq!(
                app.world().get::<Text>(banner).unwrap().0,
                format!("{title}\n10 秒后结算")
            );
        }
        app.world_mut().resource_mut::<MatchEnd>().settle_elapsed = 9.5;
        app.update();
        assert_eq!(
            app.world().get::<Text>(banner).unwrap().0,
            "平局\n1 秒后结算"
        );
        app.world_mut()
            .insert_resource(State::new(AppState::Paused));
        app.update();
        assert_eq!(
            app.world().get::<Node>(banner).unwrap().display,
            Display::None
        );
        app.world_mut()
            .insert_resource(State::new(AppState::Playing));
        app.world_mut().resource_mut::<LaunchRequest>().mode = "replay".into();
        app.update();
        assert_eq!(
            app.world().get::<Node>(banner).unwrap().display,
            Display::None
        );
        app.world_mut().resource_mut::<LaunchRequest>().mode = "single".into();
        *app.world_mut().resource_mut::<MatchEnd>() = MatchEnd::default();
        app.update();
        assert_eq!(
            app.world().get::<Node>(banner).unwrap().display,
            Display::None
        );
    }

    #[test]
    fn format_elapsed_minutes_seconds() {
        assert_eq!(format_elapsed(0.0), "0:00");
        assert_eq!(format_elapsed(59.9), "0:59");
        assert_eq!(format_elapsed(60.0), "1:00");
        assert_eq!(format_elapsed(125.0), "2:05");
    }

    #[test]
    fn escort_hud_reports_authority_state_and_local_carry_priority() {
        let request = LaunchRequest {
            mode: "escort".into(),
            stage_id: "sn03".into(),
            ..default()
        };
        let net = crate::plugins::network_plugin::NetState {
            status: crate::plugins::network_plugin::NetStatus::InRoom,
            round_active: true,
            escort_flag: Some(meteor_network::protocol::EscortFlagSnapshot {
                position: [0.0; 3],
                carrier_id: None,
                state: "dropped".into(),
                return_in: Some(3.4),
            }),
            ..default()
        };
        let dropped = mode_hud_text(
            &request,
            &net,
            &KillScore::default(),
            &MatchEnd::default(),
            false,
        );
        assert!(!dropped.contains(meteor_network::modes::mode_objective(&request.mode)));
        assert!(dropped.contains("进行中"));
        assert!(dropped.contains("3.4 秒后归位"));

        let carrying = mode_hud_text(
            &request,
            &net,
            &KillScore::default(),
            &MatchEnd::default(),
            true,
        );
        assert!(carrying.contains("你正携带镖物"));
        assert!(!carrying.contains("3.4 秒后归位"));
    }

    #[test]
    fn menu_defaults_match_single_player_flow() {
        let request = LaunchRequest::default();
        assert_eq!(request.mode, "single");
        assert_eq!(request.stage_id, "sn01");
        assert_eq!(mode_label(&request.mode), "单人");
    }

    #[test]
    fn radar_view_up_and_threat_bearings_follow_camera_in_all_quadrants() {
        for yaw in [0.0, 0.7, 1.57, 3.14, -1.57] {
            let camera = Quat::from_euler(EulerRot::YXZ, yaw, -0.6, 0.0);
            let forward = camera * Vec3::NEG_Z;
            let screen_forward = Vec2::new(forward.x, -forward.z).normalize();
            assert!((radar_view_rotation(camera) * screen_forward).abs_diff_eq(Vec2::NEG_Y, 0.0001));
        }
        assert!(!radar_is_hostile(1, 1));
        assert!(radar_is_hostile(1, 2));
        assert!(
            radar_is_hostile(0, 0),
            "FFA is hostile even for equal zero teams"
        );
        let mut strengths = [0.0; RADAR_THREAT_SECTORS];
        for _ in 0..100 {
            add_radar_threat(&mut strengths, Vec3::X * 60.0);
        }
        assert_eq!(strengths[RADAR_THREAT_SECTORS / 4], 1.0);
        assert_eq!(strengths.iter().filter(|&&value| value > 0.0).count(), 1);
        for (hostile, expected) in [
            (true, Color::srgb(1.0, 0.18, 0.13)),
            (false, Color::srgb(0.2, 1.0, 0.4)),
        ] {
            let mut image = ImageNode::default();
            set_radar_contact_image(
                &mut image,
                RadarMarkerKind::Circle,
                hostile,
                &RadarAssets::default(),
            );
            assert_eq!(image.color, expected);
        }
    }

    #[test]
    fn radar_original_arrow_tip_is_up_after_ui_texture_correction() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let bytes = load_ui_texture_bytes(&root, "arrow.bmp").unwrap();
        let decoded = meteor_asset::texture::decode_image(&bytes).unwrap();
        let mut image = ImageNode::default();
        set_radar_marker_image(&mut image, RadarMarkerKind::Arrow, &RadarAssets::default());
        let lit = |x: usize, y: usize| {
            let source_y = if image.flip_y {
                decoded.height as usize - 1 - y
            } else {
                y
            };
            let i = (source_y * decoded.width as usize + x) * 4;
            decoded.rgba[i..i + 3].iter().copied().max().unwrap() > 128
        };
        assert!(
            lit(7, 0) && !lit(1, 0),
            "arrow tip must face screen up at yaw zero"
        );
        assert!(lit(1, 15) && !lit(7, 15), "tail must remain below the tip");
    }

    #[test]
    fn folded_radar_keeps_a_safe_clickable_restore_button() {
        let mut app = App::new();
        app.insert_resource(super::super::mobile_controls_plugin::MobileControlsConfig {
            visible: true,
        });
        app.insert_resource(RadarState {
            visible: false,
            range: 2,
            ..default()
        })
        .insert_resource(RadarTerrain {
            bounds: Some((-10.0, 10.0, -10.0, 10.0)),
            ..default()
        })
        .insert_resource(SafeAreaInsets {
            left: 28.0,
            top: 12.0,
            ..default()
        })
        .insert_resource(crate::plugins::ui_scale_plugin::EffectiveUiScale(0.5))
        .add_systems(Update, update_collapsed_radar);
        app.world_mut().spawn(Window {
            resolution: bevy::window::WindowResolution::new(900, 432),
            ..default()
        });
        let button = app
            .world_mut()
            .spawn((CollapsedRadarRoot, Node::default(), Visibility::Hidden))
            .id();
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(button).unwrap(),
            Visibility::Visible
        );
        let node = app.world().get::<Node>(button).unwrap();
        assert_eq!(node.width, Val::Px(96.0));
        let radar = app.world().resource::<RadarState>();
        let (center, radius) = radar.tap_circle.unwrap();
        assert_eq!(radius, 24.0);
        assert!(radar.contains_point(center));
        assert!(!radar.contains_point(center + Vec2::X * 25.0));

        app.world_mut()
            .resource_mut::<super::super::mobile_controls_plugin::MobileControlsConfig>()
            .visible = false;
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(button).unwrap(),
            Visibility::Hidden
        );
        assert!(app.world().resource::<RadarState>().tap_circle.is_none());
        app.world_mut()
            .resource_mut::<super::super::mobile_controls_plugin::MobileControlsConfig>()
            .visible = true;
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(button).unwrap(),
            Visibility::Visible
        );
        app.world_mut().resource_mut::<RadarState>().visible = true;
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(button).unwrap(),
            Visibility::Hidden
        );
    }

    #[test]
    fn radar_shortcut_cycles_ranges_fold_restore_and_respects_rebinding() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<Keymap>()
            .init_resource::<RadarState>()
            .add_systems(Update, toggle_radar);
        for key in [KeyCode::KeyI, KeyCode::KeyR] {
            app.world_mut().resource_mut::<Keymap>().radar = InputBinding::Key(key);
            for (range, visible) in [(1, true), (2, true), (2, false), (0, true)] {
                app.world_mut()
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .press(key);
                app.update();
                let radar = app.world().resource::<RadarState>();
                assert_eq!((radar.range, radar.visible), (range, visible));
                app.world_mut()
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .clear();
                app.update();
                let radar = app.world().resource::<RadarState>();
                assert_eq!((radar.range, radar.visible), (range, visible));
                app.world_mut()
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .reset_all();
            }
        }
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyI);
        app.update();
        assert_eq!(app.world().resource::<RadarState>().range, 0);
        app.insert_resource(crate::plugins::stage_plugin::PlayerPerformance {
            input_blocked: true,
            ..default()
        });
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyR);
        app.update();
        assert_eq!(app.world().resource::<RadarState>().range, 0);
    }

    #[test]
    fn radar_three_ranges_keep_crop_and_contacts_aligned() {
        let origin = Vec3::ZERO;
        let bounds = (-200.0, 200.0, -200.0, 200.0);
        let dimensions = Vec2::splat(1024.0);
        for range in 0..3 {
            let radar = RadarState { range, ..default() };
            let radius = radar.world_radius();
            let rect = radar_texture_rect(origin, bounds, dimensions, radius);
            let target = Vec3::X * (radius * 0.5);
            let marker = map_world_to_local_radar(target, origin, 100.0, radius).unwrap();
            let pixel = radar_texture_rect(target, bounds, dimensions, radius).center();
            assert!(marker.abs_diff_eq((pixel - rect.min) / rect.size() * 100.0, 0.001));
            assert_eq!(marker, Vec2::new(75.0, 50.0));
            assert!(map_world_to_local_radar(Vec3::X * radius, origin, 100.0, radius).is_none());
        }
        let far = Vec3::X * RADAR_WORLD_RADIUS;
        assert!(map_world_to_local_radar(far, origin, 100.0, RADAR_WORLD_RADIUS).is_none());
        assert!(map_world_to_local_radar(far, origin, 100.0, RADAR_WORLD_RADIUS * 2.0).is_some());
    }

    #[test]
    fn radar_taps_cycle_ranges_fold_restore_and_reject_drags_and_cancels() {
        use bevy::input::touch::{TouchInput, TouchPhase};
        let mut app = App::new();
        app.add_message::<TouchInput>()
            .insert_resource(RadarState {
                tap_circle: Some((Vec2::splat(60.0), 50.0)),
                ..default()
            })
            .add_systems(Update, cycle_radar_range);
        let window = app.world_mut().spawn_empty().id();
        for (expected, visible) in [(1, true), (2, true), (2, false), (0, true)] {
            for phase in [TouchPhase::Started, TouchPhase::Ended] {
                app.world_mut().write_message(TouchInput {
                    phase,
                    position: Vec2::splat(60.0),
                    window,
                    force: None,
                    id: 1,
                });
            }
            app.update();
            assert_eq!(app.world().resource::<RadarState>().range, expected);
            assert_eq!(app.world().resource::<RadarState>().visible, visible);
        }
        for (phase, position) in [
            (TouchPhase::Started, Vec2::splat(60.0)),
            (TouchPhase::Moved, Vec2::splat(80.0)),
            (TouchPhase::Ended, Vec2::splat(60.0)),
            (TouchPhase::Started, Vec2::splat(60.0)),
            (TouchPhase::Canceled, Vec2::splat(60.0)),
            (TouchPhase::Started, Vec2::splat(10.0)),
            (TouchPhase::Ended, Vec2::splat(60.0)),
        ] {
            app.world_mut().write_message(TouchInput {
                phase,
                position,
                window,
                force: None,
                id: 1,
            });
        }
        app.update();
        assert_eq!(app.world().resource::<RadarState>().range, 0);
        app.world_mut().resource_mut::<RadarState>().visible = false;
        for phase in [TouchPhase::Started, TouchPhase::Ended] {
            app.world_mut().write_message(TouchInput {
                phase,
                position: Vec2::splat(60.0),
                window,
                force: None,
                id: 1,
            });
        }
        app.update();
        assert_eq!(app.world().resource::<RadarState>().range, 0);
    }

    #[test]
    fn radar_local_crop_and_markers_share_world_coordinates() {
        let origin = Vec3::new(40.0, 3.0, 50.0);
        let bounds = (-100.0, 200.0, -150.0, 150.0);
        let dimensions = Vec2::splat(1024.0);
        let rect = radar_texture_rect(origin, bounds, dimensions, RADAR_WORLD_RADIUS);
        assert_eq!(
            map_world_to_local_radar(origin, origin, 190.0, RADAR_WORLD_RADIUS),
            Some(Vec2::splat(95.0))
        );
        for delta in [Vec3::X * 10.0, Vec3::Z * 10.0] {
            let point = map_world_to_local_radar(origin + delta, origin, 190.0, RADAR_WORLD_RADIUS)
                .unwrap();
            let pixel =
                radar_texture_rect(origin + delta, bounds, dimensions, RADAR_WORLD_RADIUS).center();
            let projected = (pixel - rect.min) / rect.size() * 190.0;
            assert!(point.abs_diff_eq(projected, 0.001));
        }
        assert!(map_world_to_local_radar(
            origin + Vec3::X * RADAR_WORLD_RADIUS,
            origin,
            190.0,
            RADAR_WORLD_RADIUS
        )
        .is_none());
        assert!(map_world_to_local_radar(
            origin + Vec3::new(30.0, 0.0, 30.0),
            origin,
            190.0,
            RADAR_WORLD_RADIUS
        )
        .is_none());
        for yaw in [
            0.0,
            std::f32::consts::FRAC_PI_2,
            std::f32::consts::PI,
            -std::f32::consts::FRAC_PI_2,
        ] {
            let rotation = Quat::from_rotation_y(yaw);
            let map = radar_view_rotation(rotation);
            let arrow = radar_view_arrow_rotation(map);
            assert!((map * arrow * Vec2::NEG_Y).abs_diff_eq(Vec2::NEG_Y, 0.0001));
        }
    }

    #[test]
    fn role_names_filter_defaults_truncate_and_resolve_network_roster() {
        assert_eq!(truncated_role_name("player"), None);
        assert_eq!(truncated_role_name(" Fighter "), None);
        assert_eq!(
            truncated_role_name("123456789012345678901234567890123456"),
            Some("12345678901234567890123456789012".into())
        );
        let fighter = Fighter {
            character_id: 1,
            player_slot: 1,
            weapon: 6,
            weapon_slots: [6, 5],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        };
        let remote = crate::plugins::network_plugin::RemoteFighter {
            player_id: "p2".into(),
        };
        let mut net = crate::plugins::network_plugin::NetState::default();
        net.members.push(meteor_network::protocol::RoomMemberWire {
            connected: true,
            player_id: "p2".into(),
            name: "冷燕".into(),
            character_id: 1,
            weapon: 6,
            team: Some(1),
            ready: true,
        });
        assert_eq!(
            fighter_display_name(&fighter, None, Some(&remote), &RoomState::default(), &net),
            Some("冷燕".into())
        );
    }

    #[test]
    fn nameplate_distance_has_hysteresis_and_counts_height() {
        let at = |distance| Vec3::X * distance * meteor_core::MODEL_SCALE;
        assert!(nameplate_in_range(Vec3::ZERO, at(299.0), false));
        assert!(!nameplate_in_range(Vec3::ZERO, at(301.0), false));
        assert!(nameplate_in_range(Vec3::ZERO, at(320.0), true));
        assert!(!nameplate_in_range(Vec3::ZERO, at(331.0), true));
        assert!(!nameplate_in_range(Vec3::ZERO, Vec3::Y * 40.0, false));
    }

    fn hud_text_layout_app(width: u32, height: u32) -> (App, Handle<Font>) {
        use bevy::app::{HierarchyPropagatePlugin, PropagateSet};
        use bevy::camera::{ComputedCameraValues, RenderTargetInfo, Viewport};
        use bevy::text::TextPlugin;
        use bevy::ui::ui_surface::UiSurface;
        use bevy::ui::update::propagate_ui_target_cameras;
        use bevy::ui::widget::{measure_text_system, text_system};
        use bevy::ui::{ComputedUiRenderTargetInfo, ComputedUiTargetCamera};

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            TextPlugin,
        ))
        .init_asset::<Image>()
        .init_resource::<bevy::ui::UiScale>()
        .init_resource::<UiSurface>()
        .add_plugins(HierarchyPropagatePlugin::<ComputedUiTargetCamera>::new(
            PostUpdate,
        ))
        .add_plugins(HierarchyPropagatePlugin::<ComputedUiRenderTargetInfo>::new(
            PostUpdate,
        ))
        .add_systems(
            PostUpdate,
            (
                propagate_ui_target_cameras,
                measure_text_system
                    .after(bevy::text::detect_text_needs_rerender)
                    .after(bevy::text::load_font_assets_into_font_collection),
                bevy::ui::ui_layout_system,
                text_system,
            )
                .chain(),
        )
        .configure_sets(
            PostUpdate,
            PropagateSet::<ComputedUiTargetCamera>::default()
                .after(propagate_ui_target_cameras)
                .before(measure_text_system),
        )
        .configure_sets(
            PostUpdate,
            PropagateSet::<ComputedUiRenderTargetInfo>::default()
                .after(propagate_ui_target_cameras)
                .before(measure_text_system),
        );
        app.world_mut().spawn((
            Camera2d,
            Camera {
                computed: ComputedCameraValues {
                    target_info: Some(RenderTargetInfo {
                        physical_size: UVec2::new(width, height),
                        scale_factor: 1.0,
                    }),
                    ..default()
                },
                viewport: Some(Viewport {
                    physical_size: UVec2::new(width, height),
                    ..default()
                }),
                ..default()
            },
        ));
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/ui.ttf"),
        )
        .unwrap();
        let font = app
            .world_mut()
            .resource_mut::<Assets<Font>>()
            .add(Font::from_bytes(bytes));
        (app, font)
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn cinematic_frontend_keeps_real_desktop_and_mobile_actions_bounded() {
        for (width, height, screen) in [
            (1920, 1080, MenuScreen::Settings),
            (1280, 720, MenuScreen::Settings),
            (872, 393, MenuScreen::Settings),
            (412, 915, MenuScreen::Settings),
            (1920, 1080, MenuScreen::Main),
            (2560, 1440, MenuScreen::Network),
            (1920, 1080, MenuScreen::Network),
            (1280, 720, MenuScreen::Network),
            (872, 393, MenuScreen::Network),
            (412, 915, MenuScreen::Network),
            (1920, 1080, MenuScreen::Stages),
            (1280, 720, MenuScreen::Stages),
            (872, 393, MenuScreen::Stages),
            (412, 915, MenuScreen::Stages),
        ] {
            let (mut app, font) = hud_text_layout_app(width, height);
            app.insert_resource(UiFont(font))
                .init_resource::<LaunchRequest>()
                .init_resource::<Settings>()
                .init_resource::<Keymap>()
                .init_resource::<crate::plugins::stage_plugin::CustomStages>()
                .init_resource::<StageListTab>()
                .insert_resource(screen)
                .init_resource::<crate::plugins::ui_scale_plugin::EffectiveUiScale>()
                .add_plugins((
                    theme::CinematicThemePlugin,
                    super::super::campaign_ui::CampaignUiPlugin,
                    crate::plugins::ui_components::UiComponentsPlugin,
                ))
                .add_systems(Startup, setup_menu)
                .add_systems(
                    Update,
                    (
                        sync_launcher_screen_visibility,
                        adapt_launcher_layout,
                        adapt_settings_layout,
                        crate::plugins::settings_ui::adapt_settings_navigation,
                    )
                        .chain(),
                );
            app.world_mut().spawn(Window {
                resolution: (width, height).into(),
                ..default()
            });
            for _ in 0..3 {
                app.update();
            }
            let root = app
                .world_mut()
                .query_filtered::<Entity, With<MenuRoot>>()
                .single(app.world())
                .unwrap();
            assert!(
                app.world().get::<ImageNode>(root).is_some(),
                "packaged art must be attached to the actual menu"
            );
            if screen == MenuScreen::Settings {
                let mut tabs = 0;
                for (computed, transform) in app.world_mut().query_filtered::<(&ComputedNode, &UiGlobalTransform), With<crate::plugins::settings_ui::SettingsCategoryButton>>().iter(app.world()) {
                    let rect = Rect::from_center_size(transform.translation, computed.size());
                    assert!(rect.min.x >= 0.0 && rect.min.y >= 0.0 && rect.max.x <= width as f32 && rect.max.y <= height as f32,
                        "settings category clipped at {width}x{height}: {rect:?}");
                    assert!(computed.size().y >= 44.0);
                    tabs += 1;
                }
                assert_eq!(tabs, 6);
                let pages: Vec<_> = app.world_mut().query_filtered::<(&Node, &ComputedNode), With<crate::plugins::settings_ui::SettingsCategoryPage>>()
                    .iter(app.world()).filter(|(node,_)|node.display != Display::None).collect();
                assert_eq!(pages.len(), 1);
                assert!(
                    pages[0].1.size().x > 100.0 && pages[0].1.size().y > 80.0,
                    "category content must retain usable space"
                );
                let category_pages: Vec<_> = app
                    .world_mut()
                    .query::<(Entity, &crate::plugins::settings_ui::SettingsCategoryPage)>()
                    .iter(app.world())
                    .map(|(id, page)| (id, page.category))
                    .collect();
                for (selected, category) in &category_pages {
                    if *category == crate::plugins::settings_ui::SettingsCategory::Controls {
                        continue;
                    }
                    for (id, _) in &category_pages {
                        app.world_mut().get_mut::<Node>(*id).unwrap().display = if id == selected {
                            Display::Flex
                        } else {
                            Display::None
                        };
                    }
                    for _ in 0..3 {
                        app.update();
                    }
                    let computed = app.world().get::<ComputedNode>(*selected).unwrap();
                    assert!(computed.content_size().y <= computed.size().y + 1.0,
                        "common settings should fit at {width}x{height}: content {:?}, viewport {:?}", computed.content_size(), computed.size());
                }
            } else if screen == MenuScreen::Stages {
                let mut count = 0;
                for (pin, computed, transform) in app
                    .world_mut()
                    .query::<(
                        &super::super::campaign_ui::CampaignPin,
                        &ComputedNode,
                        &UiGlobalTransform,
                    )>()
                    .iter(app.world())
                {
                    let rect = Rect::from_center_size(transform.translation, computed.size());
                    assert!(
                        rect.min.x >= 0.0
                            && rect.min.y >= 0.0
                            && rect.max.x <= width as f32
                            && rect.max.y <= height as f32,
                        "{} pin clipped at {width}x{height}: {rect:?}",
                        pin.0
                    );
                    assert!(computed.size().y >= 44.0);
                    count += 1;
                }
                assert_eq!(count, 9);
                let (computed, transform) = app
                    .world_mut()
                    .query_filtered::<(&ComputedNode, &UiGlobalTransform), With<StartGameButton>>()
                    .single(app.world())
                    .unwrap();
                let rect = Rect::from_center_size(transform.translation, computed.size());
                assert!(
                    rect.min.y >= 0.0 && rect.max.y <= height as f32,
                    "start clipped at {width}x{height}: {rect:?}"
                );
                assert!(computed.size().y >= 44.0);
            } else if screen == MenuScreen::Network {
                let button = app
                    .world_mut()
                    .query::<(&Text, &ChildOf)>()
                    .iter(app.world())
                    .find(|(text, _)| text.0 == "创建房间")
                    .map(|(_, parent)| parent.parent())
                    .unwrap();
                *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::Pressed;
                for _ in 0..3 {
                    app.update();
                }
                let mobile = responsive_ui_layout(width as f32, height as f32, false).mobile;
                let mut checked = 0;
                for computed in app
                    .world_mut()
                    .query_filtered::<&ComputedNode, With<theme::CompactAction>>()
                    .iter(app.world())
                {
                    let size = computed.size();
                    if size.x <= 0.0 || size.y <= 0.0 {
                        continue;
                    }
                    checked += 1;
                    assert!(
                        size.x <= if mobile { 241.0 } else { 201.0 },
                        "stretched width at {width}: {size:?}"
                    );
                    assert!(
                        size.y <= if mobile { 45.0 } else { 41.0 },
                        "stretched height at {width}: {size:?}"
                    );
                    if mobile {
                        assert!(size.y >= 43.0);
                    }
                }
                assert!(checked >= 6, "create form actions must really be laid out");
                let (computed, transform) = app.world_mut()
                    .query_filtered::<(&ComputedNode, &UiGlobalTransform), With<crate::app_state::MenuStockHostButton>>()
                    .single(app.world()).unwrap();
                let rect = Rect::from_center_size(transform.translation, computed.size());
                assert!(
                    rect.min.x >= 0.0
                        && rect.min.y >= 0.0
                        && rect.max.x <= width as f32
                        && rect.max.y <= height as f32,
                    "create action must stay on screen at {width}x{height}: {rect:?}"
                );
                for (part, computed) in app
                    .world_mut()
                    .query::<(
                        &super::super::launcher_ui::LauncherCreateLayout,
                        &ComputedNode,
                    )>()
                    .iter(app.world())
                {
                    if matches!(part, super::super::launcher_ui::LauncherCreateLayout::Body) {
                        assert!(
                            computed.size().y > 100.0,
                            "create form needs a usable viewport"
                        );
                        assert!(
                            computed.content_size().x <= computed.size().x + 1.0,
                            "create form overflows horizontally at {width}x{height}"
                        );
                        if width >= 1280 {
                            assert!(computed.content_size().y <= computed.size().y + 1.0,
                                "desktop create form must fit at {width}x{height}: content {:?}, viewport {:?}",
                                computed.content_size(), computed.size());
                        }
                    }
                }
            } else {
                let mut count = 0;
                for (node, children, gradient) in app
                    .world_mut()
                    .query_filtered::<(&ComputedNode, &Children, &BackgroundGradient), With<super::super::launcher_ui::HeroNavButton>>()
                    .iter(app.world())
                {
                    if node.size().x <= 0.0 || node.size().y <= 0.0 {
                        continue;
                    }
                    count += 1;
                    assert!(node.size().x > 0.0 && node.size().x <= 321.0);
                    assert!((node.size().y - 48.0).abs() < 1.0);
                    assert!(gradient.0.is_empty(), "idle menus must have no background highlight");
                    let labels = children.iter().filter(|child| app.world().get::<Text>(*child).is_some()).count();
                    assert_eq!(labels, 1, "hero navigation has one text line, no numbered tile or subtitle");
                }
                assert_eq!(count, 3);
            }
        }
    }

    #[test]
    fn cinematic_room_cards_reflect_roster_and_stack_on_narrow_screens() {
        for (width, height) in [(1920, 1080), (872, 393), (640, 360), (412, 915)] {
            let (mut app, font) = hud_text_layout_app(width, height);
            app.insert_resource(UiFont(font))
                .init_resource::<crate::plugins::network_plugin::NetState>()
                .init_resource::<LaunchRequest>()
                .init_resource::<crate::plugins::ui_scale_plugin::EffectiveUiScale>()
                .add_systems(Startup, setup_net_room_panel)
                .add_systems(
                    Update,
                    (
                        adapt_runtime_ui_layout,
                        adapt_net_room_layout,
                        update_net_room_cards,
                        crate::plugins::network_plugin::room_lobby::update,
                        theme::compact_actions,
                    )
                        .chain(),
                );
            app.world_mut().spawn(Window {
                resolution: (width, height).into(),
                ..default()
            });
            {
                let mut net = app
                    .world_mut()
                    .resource_mut::<crate::plugins::network_plugin::NetState>();
                net.player_id = "host".into();
                net.stage = "sn01".into();
                net.mode = "assassin".into();
                net.members.push(meteor_network::protocol::RoomMemberWire {
                    connected: true,
                    player_id: "host".into(),
                    name: "房主".into(),
                    character_id: 1,
                    weapon: 5,
                    team: Some(1),
                    ready: true,
                });
            }
            for _ in 0..3 {
                app.update();
            }
            let world = app.world_mut();
            let rows = world
                .query::<(&NetRoomRosterRow, &Node)>()
                .iter(world)
                .filter(|(_, n)| n.display != Display::None)
                .count();
            assert_eq!(rows, 16);
            for (node, transform) in world
                .query_filtered::<(&ComputedNode, &UiGlobalTransform), With<NetRoomFooter>>()
                .iter(world)
            {
                assert!(
                    transform.translation.y + node.size().y * 0.5 <= height as f32 + 1.0,
                    "ready/start footer must stay inside {width}x{height}"
                );
            }

            for (node, transform) in world
                .query_filtered::<(&ComputedNode, &UiGlobalTransform), With<NetRoomSurface>>()
                .iter(world)
            {
                assert!(
                    transform.translation.y - node.size().y * 0.5 >= -1.0,
                    "room top must remain reachable at {width}x{height}"
                );
            }
            for node in world.query_filtered::<&Node, With<crate::plugins::network_plugin::room_lobby::ProfileArrow>>().iter(world) {
                assert_eq!(node.width, Val::Px(40.0), "profile arrows must not expand into full menu buttons");
            }

            assert!(world
                .query::<(&NetRoomRosterText, &Text)>()
                .iter(world)
                .any(|(role, text)| role.slot == 0
                    && role.detail
                    && text.0.contains("房主")
                    && text.0.contains("匕首")));
            let layout = world
                .query_filtered::<&Node, With<NetRoomColumns>>()
                .single(world)
                .unwrap();
            assert_eq!(
                layout.flex_direction,
                if width < 600 {
                    FlexDirection::Column
                } else {
                    FlexDirection::Row
                }
            );
            for computed in world
                .query_filtered::<&ComputedNode, With<theme::CompactAction>>()
                .iter(world)
            {
                assert!(computed.size().x <= 241.0 && computed.size().y <= 45.0);
            }
        }
    }

    #[test]
    fn hud_status_and_dialogue_fit_real_text_layout_at_mobile_scales() {
        for (width, height) in [(640, 360), (899, 432), (393, 852), (1280, 720)] {
            let (mut app, font) = hud_text_layout_app(width, height);
            app.init_resource::<RadarAssets>()
                .insert_resource(UiFont(font))
                .init_resource::<crate::plugins::replay_plugin::ReplayPlayer>()
                .init_resource::<crate::plugins::replay_plugin::MrcPlayback>()
                .init_resource::<crate::plugins::ui_scale_plugin::EffectiveUiScale>()
                .init_resource::<DialogueState>()
                .init_resource::<DialoguePresentation>()
                .add_systems(Startup, setup_playing_hud)
                .add_systems(
                    Update,
                    (adapt_runtime_ui_layout, update_dialogue_hud).chain(),
                );
            app.world_mut().spawn(Window {
                resolution: (width, height).into(),
                ..default()
            });
            for unit in [0.55, 1.0, 1.5] {
                app.world_mut().resource_mut::<bevy::ui::UiScale>().0 = unit;
                app.world_mut()
                    .resource_mut::<crate::plugins::ui_scale_plugin::EffectiveUiScale>()
                    .0 = unit;
                {
                    let mut dialogue = app.world_mut().resource_mut::<DialogueState>();
                    dialogue.text = Some(format!(
                        "冷燕：{}",
                        "你快去前面看看，千万小心守卫。".repeat(8)
                    ));
                    dialogue.timer = 3.0;
                }
                for _ in 0..3 {
                    app.update();
                }
                for mut text in app
                    .world_mut()
                    .query_filtered::<&mut Text, With<HudVital>>()
                    .iter_mut(app.world_mut())
                {
                    text.0 = "2000 / 2000".into();
                }
                for (slot, mut text) in app
                    .world_mut()
                    .query::<(&HudLoadoutText, &mut Text)>()
                    .iter_mut(app.world_mut())
                {
                    text.0 = match slot {
                        HudLoadoutText::Active => "乾坤刀".into(),
                        HudLoadoutText::Spare => "备 · 乾坤刀".into(),
                    };
                }
                app.update();
                let world = app.world_mut();
                let labels: Vec<_> = world
                    .query::<(Entity, &HudVital, &ChildOf)>()
                    .iter(world)
                    .map(|(entity, _, parent)| (entity, parent.parent()))
                    .collect();
                let status = world
                    .query_filtered::<Entity, With<GameplayHudRoot>>()
                    .single(world)
                    .unwrap();
                let panel = world
                    .query_filtered::<Entity, With<DialoguePanelRoot>>()
                    .single(world)
                    .unwrap();
                let weapons: Vec<_> = world
                    .query_filtered::<Entity, With<HudLoadoutText>>()
                    .iter(world)
                    .collect();
                let bounds = |entity| {
                    let node = world.get::<ComputedNode>(entity).unwrap();
                    Rect::from_center_size(
                        world.get::<UiGlobalTransform>(entity).unwrap().translation,
                        node.size(),
                    )
                };
                assert!(world
                    .get::<BackgroundColor>(status)
                    .is_none_or(|color| color.0 == Color::NONE));
                for (label, track) in labels {
                    let label_rect = bounds(label);
                    let track_rect = bounds(track);
                    assert!(label_rect.center().abs_diff_eq(track_rect.center(), 1.0));
                    assert!(
                        label_rect.min.x >= track_rect.min.x - 1.0
                            && label_rect.max.x <= track_rect.max.x + 1.0
                    );
                    assert!(
                        label_rect.min.y >= track_rect.min.y - 1.0
                            && label_rect.max.y <= track_rect.max.y + 1.0
                    );
                }
                let status_rect = bounds(status);
                for entity in weapons {
                    let rect = Rect::from_center_size(
                        world.get::<UiGlobalTransform>(entity).unwrap().translation,
                        world.get::<ComputedNode>(entity).unwrap().size(),
                    );
                    assert!(rect.min.x >= status_rect.min.x - 1.0);
                    assert!(rect.max.x <= status_rect.max.x + 1.0);
                    assert!(
                        rect.height() <= 20.0 + 1.0,
                        "weapon text must stay on one line"
                    );
                }
                let panel_rect = bounds(panel);
                assert!(
                    status_rect.max.y <= panel_rect.min.y || status_rect.max.x <= panel_rect.min.x,
                    "status overlaps dialogue {width}x{height} scale={unit}: {status_rect:?} {panel_rect:?}"
                );
                assert!(
                    panel_rect.min.x >= -1.0
                        && panel_rect.max.x <= width as f32 + 1.0
                        && panel_rect.max.y <= height as f32 + 1.0
                );
                let body = world
                    .query_filtered::<Entity, With<DialogueHudText>>()
                    .single(world)
                    .unwrap();
                let layout = world.get::<bevy::text::TextLayoutInfo>(body).unwrap();
                assert!(!layout.glyphs.is_empty());
                let body_rect = Rect::from_center_size(
                    world.get::<UiGlobalTransform>(body).unwrap().translation,
                    world.get::<ComputedNode>(body).unwrap().size(),
                );
                assert!(
                    body_rect.max.y <= panel_rect.max.y + 1.0
                        && body_rect.max.x <= panel_rect.max.x + 1.0,
                    "subtitle overflow {width}x{height} scale={unit}: {body_rect:?} {panel_rect:?}"
                );
            }
        }
    }

    #[test]
    fn moving_crowd_nameplates_reuse_text_layout_and_follow_projection() {
        #[derive(Resource, Default)]
        struct Writes(usize, usize, usize);
        let (mut app, font) = hud_text_layout_app(1280, 720);
        app.insert_resource(Settings {
            show_names: true,
            ..default()
        })
        .insert_resource(UiFont(font))
        .init_resource::<RoomState>()
        .init_resource::<crate::plugins::network_plugin::NetState>()
        .init_resource::<crate::plugins::ui_scale_plugin::EffectiveUiScale>()
        .init_resource::<Writes>()
        .add_systems(
            Update,
            (update_fighter_nameplates, update_fighter_vital_bars).chain(),
        )
        .add_systems(
            PostUpdate,
            (|nodes: Query<(), Changed<Node>>,
              fonts: Query<(), Changed<TextFont>>,
              layouts: Query<(), Changed<bevy::text::TextLayoutInfo>>,
              mut writes: ResMut<Writes>| {
                *writes = Writes(
                    nodes.iter().count(),
                    fonts.iter().count(),
                    layouts.iter().count(),
                );
            })
            .after(bevy::ui::widget::text_system),
        );
        let camera = app
            .world_mut()
            .query_filtered::<Entity, With<Camera>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .get_mut::<Camera>(camera)
            .unwrap()
            .computed
            .clip_from_view = Mat4::perspective_rh(60_f32.to_radians(), 1280.0 / 720.0, 0.1, 100.0);
        app.world_mut().entity_mut(camera).insert((
            crate::plugins::camera_plugin::GameplayCamera,
            Transform::from_xyz(0.0, 4.0, 20.0).looking_at(Vec3::Y * 2.0, Vec3::Y),
        ));
        let mut actors = Vec::new();
        for slot in 0..24 {
            let actor = app
                .world_mut()
                .spawn((
                    Fighter {
                        character_id: 1,
                        player_slot: slot,
                        weapon: 6,
                        weapon_slots: [6, 0],
                        weapon_slot: 0,
                        weapon_slot_variants: [0, 0],
                        weapon_variant: 0,
                    },
                    Health::default(),
                    Qi::default(),
                    Transform::from_xyz(slot as f32 * 0.1 - 1.2, 0.0, 0.0),
                    GlobalTransform::default(),
                ))
                .id();
            if slot == 0 {
                app.world_mut()
                    .entity_mut(actor)
                    .insert(crate::plugins::stage_plugin::LocalFighter);
            } else {
                let player_id = format!("npc-{slot}");
                app.world_mut()
                    .resource_mut::<crate::plugins::network_plugin::NetState>()
                    .remote_actor_names
                    .insert(player_id.clone(), format!("冷燕{slot}"));
                app.world_mut()
                    .entity_mut(actor)
                    .insert(crate::plugins::network_plugin::RemoteFighter { player_id });
                actors.push(actor);
            }
        }
        for _ in 0..4 {
            app.update();
        }
        let labels: Vec<_> = app
            .world_mut()
            .query::<&FighterNameplate>()
            .iter(app.world())
            .map(|label| label.text)
            .collect();
        assert_eq!(labels.len(), 23);
        for _ in 0..30 {
            let before = app
                .world()
                .get::<UiGlobalTransform>(labels[0])
                .unwrap()
                .translation;
            for actor in &actors {
                app.world_mut()
                    .get_mut::<Transform>(*actor)
                    .unwrap()
                    .translation
                    .x += 0.02;
            }
            app.update();
            let writes = app.world().resource::<Writes>();
            assert_eq!(
                (writes.0, writes.1, writes.2),
                (0, 0, 0),
                "moving 23 unchanged names must not reshape glyphs or dirty layout nodes"
            );
            let after = app
                .world()
                .get::<UiGlobalTransform>(labels[0])
                .unwrap()
                .translation;
            assert!(
                after.x > before.x,
                "cached names must still follow the current camera projection"
            );
        }

        app.world_mut()
            .resource_mut::<crate::plugins::network_plugin::NetState>()
            .remote_actor_names
            .insert("npc-1".into(), "新名字铁胡子".into());
        app.update();
        assert_eq!(app.world().resource::<Writes>().2, 1);
        assert!(labels
            .iter()
            .any(|entity| app.world().get::<Text>(*entity).unwrap().0 == "新名字铁胡子"));
    }

    #[test]
    fn nameplate_actual_text_layout_is_centered_at_all_scales() {
        use bevy::ecs::system::RunSystemOnce;
        use bevy::text::TextLayoutInfo;
        let (mut app, font) = hud_text_layout_app(1280, 720);
        for unit in [0.5, 0.55, 1.0, 1.5] {
            app.world_mut().resource_mut::<bevy::ui::UiScale>().0 = unit;
            for (name, head) in [
                ("冷燕", Vec2::new(640.0, 360.0)),
                ("铁胡子", Vec2::new(3.0, 5.0)),
                (
                    "一二三四五六七八九十甲乙丙丁戊己庚辛壬癸",
                    Vec2::new(1278.0, 718.0),
                ),
            ] {
                let font = font.clone();
                let (entity, text_entity) = app
                    .world_mut()
                    .run_system_once(move |mut commands: Commands| {
                        spawn_fighter_nameplate(
                            &mut commands,
                            name.into(),
                            head,
                            unit,
                            font.clone(),
                            Entity::PLACEHOLDER,
                        )
                    })
                    .unwrap();
                for _ in 0..3 {
                    app.update();
                }
                let node = app.world().get::<ComputedNode>(text_entity).unwrap();
                let layout = app.world().get::<TextLayoutInfo>(text_entity).unwrap();
                assert!(!layout.glyphs.is_empty(), "real font must be shaped");
                let transform = app
                    .world()
                    .get::<UiGlobalTransform>(text_entity)
                    .unwrap()
                    .affine();

                let origin = node.content_box().min;
                let physical_text_size = layout.size * layout.scale_factor;
                let center_bottom = transform.transform_point2(
                    origin + Vec2::new(physical_text_size.x * 0.5, physical_text_size.y),
                );
                assert!(
                    center_bottom.abs_diff_eq(head, 1.5),
                    "{name} scale={unit}: {center_bottom:?} != {head:?}, node={:?} text={:?}",
                    node.size(),
                    layout.size
                );
                app.world_mut().despawn(entity);
            }
        }
    }

    #[test]
    fn nameplate_vital_bars_follow_health_qi_and_parent_lifecycle() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.init_resource::<crate::plugins::ui_scale_plugin::EffectiveUiScale>()
            .add_systems(Update, update_fighter_vital_bars);
        let fighter = app
            .world_mut()
            .spawn((
                Health {
                    current: 25.0,
                    max: 100.0,
                },
                Qi {
                    current: 75.0,
                    max: 100.0,
                },
            ))
            .id();
        let root = app.world_mut().spawn(Node::default()).id();
        app.world_mut()
            .run_system_once(move |mut commands: Commands| {
                spawn_fighter_vital_bars(&mut commands, root, fighter, 1.0)
            })
            .unwrap();
        app.update();
        for (bar, node, color) in app
            .world_mut()
            .query::<(&FighterVitalBar, &Node, &BackgroundColor)>()
            .iter(app.world())
        {
            if bar.fill {
                assert_eq!(node.width, Val::Percent(if bar.qi { 75.0 } else { 25.0 }));
                assert_eq!(
                    color.0,
                    if bar.qi {
                        Color::srgb(0.15, 0.5, 1.0)
                    } else {
                        Color::srgb(0.95, 0.16, 0.19)
                    }
                );
            }
        }
        app.world_mut().get_mut::<Health>(fighter).unwrap().current = 0.0;
        app.world_mut()
            .entity_mut(fighter)
            .insert(crate::plugins::network_plugin::RemoteFighter {
                player_id: "remote".into(),
            });
        app.update();
        for (bar, node, visibility) in app
            .world_mut()
            .query::<(&FighterVitalBar, &Node, &Visibility)>()
            .iter(app.world())
        {
            if bar.qi {
                assert_eq!(*visibility, Visibility::Hidden);
            } else if bar.fill {
                assert_eq!(node.width, Val::Percent(0.0));
            }
        }
        app.world_mut().entity_mut(fighter).insert(
            crate::plugins::network_plugin::RemoteVitalsStatus {
                health_known: false,
                anger_known: false,
            },
        );
        app.update();
        for (_, visibility) in app
            .world_mut()
            .query::<(&FighterVitalBar, &Visibility)>()
            .iter(app.world())
        {
            assert_eq!(*visibility, Visibility::Hidden);
        }
        app.world_mut().entity_mut(fighter).insert(
            crate::plugins::network_plugin::RemoteVitalsStatus {
                health_known: true,
                anger_known: true,
            },
        );
        app.world_mut().get_mut::<Health>(fighter).unwrap().current = 50.0;
        app.update();
        for (bar, node, visibility) in app
            .world_mut()
            .query::<(&FighterVitalBar, &Node, &Visibility)>()
            .iter(app.world())
        {
            assert_eq!(*visibility, Visibility::Inherited);
            if bar.fill {
                assert_eq!(node.width, Val::Percent(if bar.qi { 75.0 } else { 50.0 }));
            }
        }
        app.world_mut().despawn(root);
        assert_eq!(
            app.world_mut()
                .query::<&FighterVitalBar>()
                .iter(app.world())
                .count(),
            0
        );
        assert_eq!(vital_fraction(200.0, 100.0), 1.0);
        assert_eq!(vital_fraction(f32::NAN, 100.0), 0.0);
        assert_eq!(vital_fraction(10.0, 0.0), 0.0);
    }

    #[test]
    fn production_nameplate_queries_are_disjoint() {
        let mut app = App::new();
        app.insert_resource(Settings::default())
            .insert_resource(RoomState::default())
            .init_resource::<crate::plugins::network_plugin::NetState>()
            .init_resource::<crate::plugins::ui_scale_plugin::EffectiveUiScale>()
            .insert_resource(UiFont(Handle::default()))
            .add_systems(Update, update_fighter_nameplates);
        app.world_mut().spawn((
            Camera::default(),
            Transform::default(),
            GlobalTransform::default(),
            crate::plugins::camera_plugin::GameplayCamera,
        ));
        app.update();
    }

    #[test]
    fn radar_new_stage_resets_hidden_state_and_cached_terrain() {
        let mut app = App::new();
        app.insert_resource(RadarState {
            visible: false,
            ..default()
        })
        .insert_resource(RadarTerrain {
            bounds: Some((0.0, 1.0, 0.0, 1.0)),
            image: Some(Handle::default()),
            texture_size: Vec2::splat(1024.0),
        })
        .add_systems(Update, reset_radar);
        app.update();
        assert!(app.world().resource::<RadarState>().visible);
        assert!(app.world().resource::<RadarTerrain>().image.is_none());
        assert!(app.world().resource::<RadarTerrain>().bounds.is_none());
    }

    #[test]
    fn production_radar_builds_local_marker_from_live_stage_collider() {
        let mut app = App::new();
        let mut stage_mesh = crate::plugins::stage_collider::StageMeshCollider::default();
        stage_mesh.positions = vec![[-5.0, 0.0, -5.0], [5.0, 0.0, 5.0]];
        app.init_resource::<RadarState>()
            .init_resource::<RadarTerrain>()
            .init_resource::<Assets<Image>>()
            .init_resource::<RadarAssets>()
            .init_resource::<crate::plugins::network_plugin::NetState>()
            .insert_resource(crate::plugins::stage_collider::StageCollider {
                mesh: Some(stage_mesh),
            })
            .add_systems(Update, (prepare_radar_terrain, update_radar).chain());
        app.world_mut()
            .spawn((RadarRoot, Node::default(), Visibility::Hidden));
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
                Health::default(),
                Transform::default(),
                Visibility::Visible,
                crate::plugins::stage_plugin::LocalFighter,
            ))
            .id();
        app.update();
        let first_image = app
            .world()
            .resource::<RadarTerrain>()
            .image
            .clone()
            .unwrap();
        app.update();
        assert_eq!(
            app.world().resource::<RadarTerrain>().image.as_ref(),
            Some(&first_image)
        );
        assert_eq!(app.world().resource::<Assets<Image>>().len(), 2);
        let markers: Vec<RadarMarker> = app
            .world_mut()
            .query::<&RadarMarker>()
            .iter(app.world())
            .copied()
            .collect();
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].fighter, fighter);
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<RadarRoot>>()
            .single(app.world())
            .unwrap();
        assert_eq!(
            app.world().get::<Node>(root).unwrap().border_radius,
            BorderRadius::all(Val::Percent(50.0))
        );
        let old_crop = app.world().get::<ImageNode>(root).unwrap().rect.unwrap();
        app.world_mut()
            .get_mut::<Transform>(fighter)
            .unwrap()
            .translation
            .x += 3.0;
        app.update();
        let new_crop = app.world().get::<ImageNode>(root).unwrap().rect.unwrap();
        assert!(new_crop.center().x > old_crop.center().x);
        assert!(new_crop.size().abs_diff_eq(old_crop.size(), 0.001));
        let marker_node = app
            .world_mut()
            .query_filtered::<&Node, With<RadarMarker>>()
            .single(app.world())
            .unwrap();
        assert_eq!(marker_node.left, Val::Px(76.0 - 8.0));
        assert_eq!(marker_node.top, Val::Px(76.0 - 8.0));
        app.world_mut()
            .get_mut::<Transform>(fighter)
            .unwrap()
            .rotation = Quat::from_rotation_y(1.0);
        app.update();
        let heading = app
            .world_mut()
            .query_filtered::<&UiTransform, With<RadarMarker>>()
            .single(app.world())
            .unwrap()
            .rotation
            .as_radians();
        assert!(
            heading.abs() < 1e-5,
            "body rotation must not turn the view arrow"
        );
        app.world_mut().resource_mut::<RadarState>().visible = false;
        app.update();
        assert_eq!(
            *app.world_mut()
                .query_filtered::<&Visibility, With<RadarRoot>>()
                .single(app.world())
                .unwrap(),
            Visibility::Hidden
        );
        app.world_mut().resource_mut::<RadarState>().visible = true;
        app.update();
        assert_eq!(
            app.world_mut()
                .query_filtered::<&Visibility, With<RadarRoot>>()
                .single(app.world())
                .expect("single radar root"),
            &Visibility::Visible
        );

        let camera = app
            .world_mut()
            .spawn((
                crate::plugins::camera_plugin::GameplayCamera,
                Transform::default(),
            ))
            .id();
        for camera_yaw in [0.0, 0.7, 1.57, 3.14, -1.57] {
            for body_yaw in [0.0, 1.0, -2.0] {
                app.world_mut()
                    .get_mut::<Transform>(camera)
                    .unwrap()
                    .rotation = Quat::from_euler(EulerRot::YXZ, camera_yaw, -0.5, 0.0);
                app.world_mut()
                    .get_mut::<Transform>(fighter)
                    .unwrap()
                    .rotation = Quat::from_rotation_y(body_yaw);
                app.update();
                let map = app.world().get::<UiTransform>(root).unwrap().rotation;
                let arrow = app
                    .world_mut()
                    .query_filtered::<&UiTransform, With<RadarMarker>>()
                    .single(app.world())
                    .unwrap()
                    .rotation;
                assert!(
                    (map * arrow * Vec2::NEG_Y).abs_diff_eq(Vec2::NEG_Y, 0.0001),
                    "view={camera_yaw}, body={body_yaw}: centre arrow must point up"
                );
            }
        }
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .rotation = Quat::IDENTITY;

        {
            let mut net = app
                .world_mut()
                .resource_mut::<crate::plugins::network_plugin::NetState>();
            net.status = crate::plugins::network_plugin::NetStatus::InRoom;
            net.player_id = "self".into();
            for (id, team) in [("self", 1), ("friend", 1), ("enemy", 2), ("far", 2)] {
                net.remote_actor_teams.insert(id.into(), team);
            }
        }
        let prototype = app.world().get::<Fighter>(fighter).unwrap().clone();
        let mut far = Entity::PLACEHOLDER;
        for (id, x) in [("friend", 10.0), ("enemy", 15.0), ("far", 80.0)] {
            let entity = app
                .world_mut()
                .spawn((
                    prototype.clone(),
                    Health::default(),
                    Transform::from_xyz(x, 0.0, 0.0),
                    Visibility::Visible,
                    crate::plugins::network_plugin::RemoteFighter {
                        player_id: id.into(),
                    },
                ))
                .id();
            if id == "far" {
                far = entity;
            }
        }
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&RadarMarker>()
                .iter(app.world())
                .count(),
            3
        );
        assert!(app
            .world_mut()
            .query::<&RadarThreatGlow>()
            .iter(app.world())
            .any(|glow| glow.level > 0.0));
        let rotation = app.world().get::<UiTransform>(root).unwrap().rotation;
        assert!((rotation * Vec2::Y).abs_diff_eq(Vec2::NEG_Y, 0.0001));
        app.world_mut().get_mut::<Health>(far).unwrap().current = 0.0;
        for _ in 0..60 {
            app.update();
        }
        assert!(app
            .world_mut()
            .query::<&RadarThreatGlow>()
            .iter(app.world())
            .all(|glow| glow.level == 0.0));
        let size = app.world().get::<Node>(root).unwrap().width;
        let crop = app
            .world()
            .get::<ImageNode>(root)
            .unwrap()
            .rect
            .unwrap()
            .size();
        for (range, factor) in [(1, 2.0), (2, 4.0), (0, 1.0)] {
            app.world_mut().resource_mut::<RadarState>().range = range;
            app.update();
            assert_eq!(app.world().get::<Node>(root).unwrap().width, size);
            assert!(app
                .world()
                .get::<ImageNode>(root)
                .unwrap()
                .rect
                .unwrap()
                .size()
                .abs_diff_eq(crop * factor, 0.001));
        }
    }

    #[test]
    fn production_replay_scrub_pauses_seeks_and_restores_play_state() {
        let mut app = App::new();
        app.init_resource::<ReplayScrubState>()
            .init_resource::<crate::plugins::replay_plugin::ReplayPlayer>()
            .init_resource::<crate::plugins::replay_plugin::MrcPlayback>()
            .init_resource::<crate::plugins::replay_plugin::ReplaySeekRequest>()
            .add_systems(Update, replay_timeline_scrub);
        let mut doc = meteor_asset::mrc::MrcDocument {
            magic: meteor_asset::mrc::MRC_MAGIC,
            header_size: meteor_asset::mrc::MRC_HEADER_SIZE,
            mark0: 0,
            mark1: 0,
            tail: 0,
            state: [0; meteor_asset::mrc::MRC_STATE_SIZE],
            meta: [0; 8],
            queue_a: Vec::new(),
            queue_b: Vec::new(),
            queue_c: Vec::new(),
            queue_d: Vec::new(),
            queue_e: Vec::new(),
        };
        doc.queue_d.push(meteor_asset::mrc::MrcQueueDEntry {
            a: 0,
            b: 0,
            c: 0,
            time: 10.0,
        });
        {
            let mut mrc = app
                .world_mut()
                .resource_mut::<crate::plugins::replay_plugin::MrcPlayback>();
            mrc.active = true;
            mrc.doc = Some(doc);
        }
        let track = app
            .world_mut()
            .spawn((
                ReplayTimelineTrack,
                Interaction::Pressed,
                bevy::ui::RelativeCursorPosition {
                    normalized: Some(Vec2::new(0.5, 0.5)),
                    ..default()
                },
            ))
            .id();
        app.update();
        assert!(
            app.world()
                .resource::<crate::plugins::replay_plugin::MrcPlayback>()
                .paused
        );
        assert_eq!(
            app.world()
                .resource::<crate::plugins::replay_plugin::ReplaySeekRequest>()
                .0,
            Some(5.0)
        );
        assert!(app.world().resource::<ReplayScrubState>().active);

        *app.world_mut().get_mut::<Interaction>(track).unwrap() = Interaction::Hovered;
        app.world_mut()
            .get_mut::<bevy::ui::RelativeCursorPosition>(track)
            .unwrap()
            .normalized = Some(Vec2::new(0.7, 0.5));
        app.update();
        assert!(
            !app.world()
                .resource::<crate::plugins::replay_plugin::MrcPlayback>()
                .paused
        );
        assert_eq!(
            app.world()
                .resource::<crate::plugins::replay_plugin::ReplaySeekRequest>()
                .0,
            Some(7.0)
        );
        assert!(!app.world().resource::<ReplayScrubState>().active);
    }

    #[test]
    fn original_firearm_crosshair_resolves_from_ui_texture_pack() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let bytes = load_ui_texture_bytes(&root, "ATarget.jpg").expect("UITexture.pak/ATarget.jpg");
        let image = meteor_asset::texture::decode_image(&bytes).expect("decode ATarget.jpg");
        assert_eq!((image.width, image.height), (16, 16));
        let arrow = load_ui_texture_bytes(&root, "arrow.bmp").expect("UITexture.pak/arrow.bmp");
        let arrow = meteor_asset::texture::decode_image(&arrow).expect("decode arrow.bmp");
        assert!(arrow.width > 0 && arrow.height > 0);
        let flag = load_ui_texture_bytes(&root, "FLAG01.jpg").expect("UITexture.pak/FLAG01.jpg");
        let flag = meteor_asset::texture::decode_image(&flag).expect("decode FLAG01.jpg");
        assert_eq!((flag.width, flag.height), (32, 32));
    }
}
