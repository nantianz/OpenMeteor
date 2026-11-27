#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use super::native_text_input::{NativeTextEdit, NativeTextLabel, TextField};
use super::ui_theme as theme;
use crate::app_state::{
    CustomStageGrid, Keymap, LauncherBrandPanel, LauncherNavButton, LauncherScreenMarker,
    LauncherStyledButton, MenuBackButton, MenuMrcButton, MenuNavigateButton, MenuNetStatusText,
    MenuRoot, MenuScreen, OfficialStageGrid, SelectedStageCodeText, SelectedStageNameText,
    Settings, SettingsResetButton, StageButton, StageListTab, StageTabCustomButton,
    StageTabOfficialButton, StartGameButton,
};
use crate::app_state::{LaunchRequest, MenuStockDiscoveryText, MenuStockRefreshButton};
#[cfg(not(target_arch = "wasm32"))]
use crate::app_state::{
    MenuPublicBattleButton, MenuStockJoinButton, MenuStockLoadoutCancel, MenuStockLoadoutConfirm,
    MenuStockLoadoutDialog,
};
use crate::app_state::{
    MenuStockCampButton, MenuStockHostButton, MenuStockItemRuleButton, MenuStockRuleButton,
    MenuStockSelectOption, MenuStockSelectText, MenuStockStageButton, MenuStockStageText,
    StockConfigField, StockItemRule,
};
use crate::plugins::mobile_controls_plugin::MobileLayoutEditorState;
use crate::plugins::network_plugin::room_browser::RoomTarget;
#[cfg(not(target_arch = "wasm32"))]
use crate::plugins::network_plugin::stock_connection_dialog::{
    StockConnectionAction, StockConnectionDialog, StockConnectionDialogRoot, StockConnectionText,
};
use crate::plugins::ui_components::{
    spawn_selection_indicator, UiActionBarBundle, UiPageAction, UiSecondaryHeaderBundle,
    UiSecondaryPageBundle, UiSelectableButtonBundle, UiSelectableKind, UiSelectableStyle,
};
use crate::plugins::ui_components::{
    UiDialogSurfaceBundle, UiModalAction, UiModalId, UiModalOverlayBundle, UiModalRoot,
};
use crate::plugins::ui_components::{
    UiInputBlocker, UiPageId, UiPageRegionBundle, UiPageRoot, UiPageStack, UiPanelBundle,
};
use crate::plugins::ui_scale_plugin::{EffectiveUiScale, SafeAreaInsets};
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::input::touch::{TouchInput, TouchPhase};
use bevy::prelude::*;
use bevy::ui::{ComputedStackIndex, ScrollPosition};
use meteor_scene::stage_registry::official_stages;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
#[cfg(test)]
use std::path::PathBuf;

const LAUNCHER_SCROLL_LINE: f32 = 21.0;
const LAUNCHER_NARROW_W: f32 = 940.0;
const LAUNCHER_NARROW_H: f32 = 560.0;
const LAUNCHER_COMPACT_W: f32 = 720.0;
const LAUNCHER_STACKED_W: f32 = 680.0;
const TOUCH_SCROLL_THRESHOLD: f32 = 8.0;
const MOBILE_MIN_TOUCH_TARGET: f32 = 44.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ResponsiveUiLayout {
    pub mobile: bool,
    pub narrow: bool,
    pub stacked_stage: bool,
    pub left_gutter: f32,
    pub right_gutter: f32,
    pub top_gutter: f32,
    pub bottom_gutter: f32,
}

pub(crate) fn responsive_ui_layout(
    width: f32,
    height: f32,
    native_mobile: bool,
) -> ResponsiveUiLayout {
    responsive_ui_layout_with_safe_area(width, height, native_mobile, SafeAreaInsets::default())
}

pub(crate) fn responsive_ui_layout_with_safe_area(
    width: f32,
    height: f32,
    native_mobile: bool,
    safe_area: SafeAreaInsets,
) -> ResponsiveUiLayout {
    let mobile = native_mobile || width < LAUNCHER_COMPACT_W || height < 430.0;
    let horizontal_gutter: f32 = if native_mobile {
        16.0
    } else if mobile {
        18.0
    } else {
        48.0
    };
    let top_gutter: f32 = if mobile { 8.0 } else { 40.0 };
    let bottom_gutter: f32 = if mobile { 8.0 } else { 18.0 };
    let safe = safe_area.sanitized(width, height);
    let safe_spacing = if native_mobile { 8.0 } else { 0.0 };
    ResponsiveUiLayout {
        mobile,
        narrow: mobile || width < LAUNCHER_NARROW_W || height < LAUNCHER_NARROW_H,

        stacked_stage: width < LAUNCHER_STACKED_W,
        left_gutter: horizontal_gutter.max(safe.left + safe_spacing),
        right_gutter: horizontal_gutter.max(safe.right + safe_spacing),
        top_gutter: top_gutter.max(safe.top + safe_spacing),
        bottom_gutter: bottom_gutter.max(safe.bottom + safe_spacing),
    }
}

#[derive(Component)]
pub(crate) struct LauncherMainCopy;

#[derive(Component)]
pub(crate) struct LauncherMobileHero;

#[derive(Component)]
pub(crate) struct LauncherShell;

#[derive(Component)]
pub(crate) struct LauncherHeader;

#[derive(Component)]
pub(crate) struct LauncherPageHeading;

#[derive(Component)]
pub(crate) struct NetworkBrowserHeading;

#[derive(Component)]
pub(crate) struct HeroNavButton;
#[derive(Component)]
pub(crate) struct HeroNavAccent;

#[derive(Component)]
pub(crate) struct HeroMenuLayout;

#[derive(Component)]
pub(crate) struct LauncherNetworkPrimaryActions;

#[derive(Component)]
pub(crate) struct LauncherNetworkPrimaryButton;

#[derive(Component)]
pub(crate) struct LauncherNetworkSection;

#[derive(Component)]
pub(crate) struct LauncherNetworkRoomList;

#[derive(Component)]
pub(crate) struct LauncherNetworkRoomButton;

#[derive(Component)]
pub(crate) struct LauncherNetworkBrowserLayout;

#[derive(Component)]
pub(crate) struct LauncherNetworkActionRail;

#[derive(Component)]
pub(crate) struct NetworkRailAction;

#[derive(Component)]
pub(crate) struct LauncherNetworkRoomPanel;

#[derive(Component)]
pub(crate) enum LauncherCreateLayout {
    Page,
    Body,
    Maps,
    Settings,
    MapChoice,
    MapPreview,
    MapList,
    Brand,
}

#[derive(Component)]
pub(crate) struct LauncherNavIcon;

#[derive(Component)]
pub(crate) struct LauncherNavLabels;

#[derive(Component)]
pub(crate) struct LauncherStageLayout;

#[derive(Component)]
pub(crate) struct LauncherStageAside;

#[derive(Component)]
pub(crate) struct LauncherMobileStartBar;

#[derive(Component)]
pub(crate) struct LauncherStageAsideStartButton;

#[derive(Component)]
pub(crate) struct RoomMapPickerPage;

#[derive(Component)]
pub(crate) struct LauncherMobileStartButton;

#[derive(Component)]
pub(crate) struct LauncherOuterScrollArea;

#[derive(Component)]
pub(crate) struct LauncherScrollArea;

#[derive(Resource, Debug, Default)]
pub(crate) struct StageSelectionMemory {
    official: Option<String>,
    custom: Option<String>,
}

impl StageSelectionMemory {
    pub(crate) fn remember(&mut self, tab: StageListTab, stage_id: &str) {
        *match tab {
            StageListTab::Official => &mut self.official,
            StageListTab::Custom => &mut self.custom,
        } = Some(stage_id.to_owned());
    }

    fn selected(&self, tab: StageListTab) -> Option<&str> {
        match tab {
            StageListTab::Official => self.official.as_deref(),
            StageListTab::Custom => self.custom.as_deref(),
        }
    }
}

#[derive(Component)]
pub(crate) struct LauncherStageTab(StageListTab);

#[derive(Resource, Default)]
pub(crate) struct LauncherTouchScrollState {
    pointer: Option<u64>,
    target: Option<Entity>,
    button: Option<Entity>,
    start: Vec2,
    previous: Vec2,
    max_distance: f32,
    dragging: bool,
}

pub(crate) fn scrollable_y(mut node: Node) -> impl Bundle {
    node.overflow = Overflow::scroll_y();
    node.min_height = Val::Px(0.0);
    if node.max_height == Val::Auto {
        node.max_height = Val::Percent(100.0);
    }
    (
        node,
        ScrollPosition::default(),
        LauncherScrollArea,
        Interaction::default(),
    )
}

pub const LAUNCHER_CARD: Color = theme::CARD;
pub const LAUNCHER_CARD_ACTIVE: Color = theme::SELECTED;
pub const LAUNCHER_LINE: Color = theme::LINE;
pub const LAUNCHER_RED_BRIGHT: Color = theme::ACCENT_TEXT;

const INK: Color = theme::BACKGROUND;
const PAPER: Color = theme::TEXT;
const RED_BRIGHT: Color = LAUNCHER_RED_BRIGHT;
const CYAN: Color = theme::MUTED;
pub(crate) const PANEL: Color = theme::SURFACE;
const NAV_HOVER: Color = theme::SELECTED;
const STAGE_CARD: Color = LAUNCHER_CARD;
const STAGE_CARD_ACTIVE: Color = LAUNCHER_CARD_ACTIVE;
const ACTION_RED: Color = theme::PRIMARY;

const NETWORK_PAGE_STACK: UiPageStack = UiPageStack(1);
const NETWORK_BROWSER_PAGE: UiPageId = UiPageId(0);
const NETWORK_CREATE_PAGE: UiPageId = UiPageId(1);
#[cfg(not(target_arch = "wasm32"))]
const STOCK_DIRECT_MODAL: UiModalId = UiModalId(1);
const ROOM_ADVANCED_MODAL: UiModalId = UiModalId(4);

#[derive(Component)]
struct RoomAdvancedDialog;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Component)]
pub(super) struct RoomPhysicsSlider;
#[cfg(not(target_arch = "wasm32"))]
#[derive(Component)]
pub(super) struct RoomPhysicsLabel;
#[cfg(not(target_arch = "wasm32"))]
#[derive(Component)]
pub(super) struct RoomPhysicsSection;
#[derive(Resource, Clone)]
pub struct UiFont(pub Handle<Font>);

pub(super) struct UiFontPlugin;
impl Plugin for UiFontPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiFont>();
    }
}

impl FromWorld for UiFont {
    fn from_world(world: &mut World) -> Self {
        Self::load(&mut world.resource_mut::<Assets<Font>>())
    }
}

impl UiFont {
    pub fn load(fonts: &mut Assets<Font>) -> Self {
        Self(fonts.add(Font::from_bytes(crate::resource_io::required(
            "fonts/ui.ttf",
        ))))
    }
}

pub fn setup_launcher(
    mut commands: Commands,
    font: Handle<Font>,
    request: Res<LaunchRequest>,
    settings: Res<Settings>,
    keymap: Res<Keymap>,
    custom: Res<crate::plugins::stage_plugin::CustomStages>,
    screen: Res<MenuScreen>,
    tab: Res<StageListTab>,
) {
    let root = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                min_width: Val::Px(0.0),
                min_height: Val::Px(0.0),
                flex_direction: FlexDirection::Row,
                ..default()
            },
            BackgroundColor(INK),
            MenuRoot,
        ))
        .id();

    spawn_shell(
        &mut commands,
        &font,
        &request,
        &settings,
        &keymap,
        &custom,
        *screen,
        *tab,
        root,
    );

    commands.entity(root).with_children(|overlay| {
        crate::plugins::settings_ui::spawn_settings_select_dialogs(overlay, &font, &settings);
    });

    #[cfg(not(target_arch = "wasm32"))]
    commands.entity(root).with_children(|overlay| {
        spawn_stock_direct_dialog(overlay, &font, &request);
        spawn_stock_loadout_dialog(overlay, &font, &request);
        spawn_stock_connection_dialog(overlay, &font);
    });
    commands.entity(root).with_children(|overlay| {
        spawn_room_advanced_dialog(overlay, &font, &request);
    });
}

fn spawn_shell(
    commands: &mut Commands,
    font: &Handle<Font>,
    request: &LaunchRequest,
    settings: &Settings,
    keymap: &Keymap,
    custom: &crate::plugins::stage_plugin::CustomStages,
    screen: MenuScreen,
    tab: StageListTab,
    root: Entity,
) {
    let shell = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                min_width: Val::Px(0.0),
                min_height: Val::Px(0.0),
                flex_grow: 1.0,
                flex_direction: FlexDirection::Column,
                padding: UiRect {
                    left: Val::Px(48.0),
                    right: Val::Px(48.0),
                    top: Val::Px(40.0),
                    bottom: Val::Px(18.0),
                },
                ..default()
            },
            LauncherShell,
        ))
        .with_children(|shell| {
            shell
                .spawn((
                    Node {
                        width: Val::Percent(100.0),
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::FlexEnd,
                        padding: UiRect {
                            bottom: Val::Px(18.0),
                            ..default()
                        },
                        border: UiRect::bottom(Val::Px(1.0)),
                        ..default()
                    },
                    BorderColor::all(theme::LINE),
                    LauncherHeader,
                ))
                .with_children(|header| {
                    header
                        .spawn(Node {
                            flex_direction: FlexDirection::Column,
                            ..default()
                        })
                        .with_children(|left| {
                            left.spawn((
                                launcher_text("流星再世", 24.0, theme::TEXT, font),
                                Node {
                                    flex_shrink: 0.0,
                                    ..default()
                                },
                                LauncherCreateLayout::Brand,
                            ));
                        });
                });

            shell
                .spawn((
                    Node {
                        width: Val::Percent(100.0),
                        flex_grow: 1.0,
                        min_height: Val::Px(0.0),
                        position_type: PositionType::Relative,

                        overflow: Overflow::scroll_y(),
                        ..default()
                    },
                    ScrollPosition::default(),
                    LauncherOuterScrollArea,
                    LauncherScrollArea,
                    Interaction::default(),
                ))
                .with_children(|screens| {
                    spawn_main_screen(screens, font, screen == MenuScreen::Main);
                    spawn_stages_screen(
                        screens,
                        font,
                        request,
                        custom,
                        tab,
                        screen == MenuScreen::Stages,
                    );
                    spawn_settings_screen(
                        screens,
                        font,
                        settings,
                        keymap,
                        screen == MenuScreen::Settings,
                    );
                    spawn_network_screen(
                        screens,
                        font,
                        request,
                        custom,
                        screen == MenuScreen::Network,
                    );
                    spawn_replay_screen(screens, font, screen == MenuScreen::Replay);
                });

            shell.spawn((
                launcher_text(
                    &super::studio_splash::development_notice(),
                    11.0,
                    theme::MUTED,
                    font,
                ),
                TextLayout::justify(Justify::Center),
                bevy::ui::widget::TextShadow {
                    offset: Vec2::new(1.0, 1.0),
                    color: theme::SCRIM,
                },
                Node {
                    margin: UiRect::top(px(10)),
                    flex_shrink: 0.0,
                    ..default()
                },
            ));
        })
        .id();
    commands.entity(root).add_child(shell);
}

fn spawn_main_screen(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, active: bool) {
    let (visibility, display) = screen_layout(active);
    let screen_bundle = scrollable_y(Node {
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        min_height: Val::Percent(100.0),
        justify_content: JustifyContent::Center,
        position_type: PositionType::Absolute,
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Stretch,
        row_gap: Val::Px(24.0),
        padding: UiRect::vertical(Val::Px(8.0)),
        display,
        ..default()
    });
    parent
        .spawn((
            screen_bundle,
            HeroMenuLayout,
            LauncherScreenMarker(MenuScreen::Main),
            visibility,
        ))
        .with_children(|screen| {
            for (size, compact) in [(60.0, false), (34.0, true)] {
                let mut heading = screen.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::FlexStart,
                    row_gap: px(if compact { 6 } else { 12 }),
                    flex_shrink: 0.0,
                    display: if compact {
                        Display::None
                    } else {
                        Display::Flex
                    },
                    ..default()
                });
                if compact {
                    heading.insert(LauncherMobileHero);
                } else {
                    heading.insert(LauncherMainCopy);
                }
                heading.with_children(|heading| {
                    heading.spawn(launcher_text("流星再世", size, theme::TEXT, font));
                    heading.spawn(theme::heading_rule(px(if compact { 142 } else { 300 })));
                });
            }

            screen
                .spawn((Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    width: Val::Percent(100.0),
                    max_width: Val::Px(320.0),
                    align_self: AlignSelf::FlexStart,
                    ..default()
                },))
                .with_children(|nav| {
                    spawn_hero_nav_button(
                        nav,
                        font,
                        "单人闯关",
                        MenuNavigateButton(MenuScreen::Stages),
                    );
                    spawn_hero_nav_button(
                        nav,
                        font,
                        "网络对战",
                        MenuNavigateButton(MenuScreen::Network),
                    );
                    spawn_hero_nav_button(
                        nav,
                        font,
                        "MRC 回放",
                        MenuNavigateButton(MenuScreen::Replay),
                    );
                    spawn_hero_nav_button(
                        nav,
                        font,
                        "设置",
                        MenuNavigateButton(MenuScreen::Settings),
                    );
                    spawn_hero_nav_button(nav, font, "退出游戏", super::ui_plugin::ExitGameButton);
                });
        });
}

fn spawn_stages_screen(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    _request: &LaunchRequest,
    _custom: &crate::plugins::stage_plugin::CustomStages,
    _tab: StageListTab,
    active: bool,
) {
    super::campaign_ui::spawn(parent, font, active);
}
fn spawn_settings_screen(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    settings: &Settings,
    keymap: &Keymap,
    active: bool,
) {
    let (visibility, display) = screen_layout(active);

    parent
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                display,
                ..default()
            },
            LauncherScreenMarker(MenuScreen::Settings),
            visibility,
        ))
        .with_children(|screen| {
            spawn_full_settings_page(screen, font, settings, keymap, false);
        });
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadoutField {
    Character,
    Weapon,
}

#[derive(Component)]
pub(crate) struct LoadoutArrow {
    field: LoadoutField,
    direction: i8,
}

#[derive(Component, Clone, Copy)]
pub(crate) struct RoomStageStepArrow(pub i8);

const LOADOUT_WEAPONS: [u8; 12] = [6, 5, 8, 7, 9, 4, 1, 2, 3, 10, 11, 12];

fn loadout_label(field: LoadoutField, request: &LaunchRequest) -> String {
    let label = match field {
        LoadoutField::Character => character_label(request.character_id),
        LoadoutField::Weapon => weapon_label(request.weapon),
    };
    label
        .split_once(' ')
        .map_or_else(|| label.clone(), |(_, name)| name.to_owned())
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_player_loadout(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    request: &LaunchRequest,
) {
    for (field, title) in [
        (LoadoutField::Character, "出战人物"),
        (LoadoutField::Weapon, "主手兵器"),
    ] {
        parent
            .spawn((Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            },))
            .with_children(|group| {
                group.spawn(launcher_text(title, 12.0, theme::METAL, font));
                group
                    .spawn(Node {
                        width: Val::Percent(100.0),
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(6.0),
                        ..default()
                    })
                    .with_children(|row| {
                        spawn_loadout_arrow(row, field, -1);
                        row.spawn((
                            Node {
                                flex_grow: 1.0,
                                flex_basis: Val::Px(0.0),
                                min_width: Val::Px(0.0),
                                min_height: Val::Px(44.0),
                                justify_content: JustifyContent::Center,
                                align_items: AlignItems::Center,
                                padding: UiRect::horizontal(Val::Px(6.0)),
                                border: UiRect::all(Val::Px(1.0)),
                                border_radius: BorderRadius::ZERO,
                                overflow: Overflow::clip(),
                                ..default()
                            },
                            BackgroundColor(STAGE_CARD_ACTIVE),
                            BorderColor::all(theme::ACCENT.with_alpha(0.55)),
                        ))
                        .with_children(|value| {
                            value.spawn((
                                launcher_text(&loadout_label(field, request), 15.0, PAPER, font),
                                field,
                            ));
                        });
                        spawn_loadout_arrow(row, field, 1);
                    });
            });
    }
}

fn apply_loadout_step(request: &mut LaunchRequest, field: LoadoutField, direction: i8) {
    match field {
        LoadoutField::Character => {
            request.character_id = (i32::from(request.character_id) + i32::from(direction))
                .rem_euclid(CHARACTER_NAMES.len() as i32) as u8;
        }
        LoadoutField::Weapon => {
            let current = LOADOUT_WEAPONS.iter().position(|id| *id == request.weapon);
            let next = current.map_or(
                if direction < 0 {
                    LOADOUT_WEAPONS.len() - 1
                } else {
                    0
                },
                |index| {
                    (index as i32 + i32::from(direction)).rem_euclid(LOADOUT_WEAPONS.len() as i32)
                        as usize
                },
            );
            request.weapon = LOADOUT_WEAPONS[next];
        }
    }
}

pub(crate) fn player_loadout_controls(
    buttons: Query<(&Interaction, &LoadoutArrow), (Changed<Interaction>, With<Button>)>,
    stage_arrows: Query<(&Interaction, &RoomStageStepArrow), (Changed<Interaction>, With<Button>)>,
    stage_choices: Query<&MenuStockStageButton>,
    mut request: ResMut<LaunchRequest>,
    mut labels: Query<(&LoadoutField, &mut Text)>,
) {
    for (interaction, arrow) in &buttons {
        if *interaction == Interaction::Pressed {
            apply_loadout_step(&mut request, arrow.field, arrow.direction);
        }
    }
    for (interaction, step) in &stage_arrows {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let mut allowed: Vec<(&str, &str)> = stage_choices
            .iter()
            .filter(|s| {
                super::network_plugin::stock_stage_allowed_for_rule(request.stock_rule, &s.id)
            })
            .map(|s| (s.id.as_str(), s.name.as_str()))
            .collect();
        allowed.sort_by_key(|(id, _)| *id);
        if !allowed.is_empty() {
            let current = allowed
                .iter()
                .position(|(id, _)| *id == request.stage_id)
                .unwrap_or(0);
            let next =
                (current as isize + step.0 as isize).rem_euclid(allowed.len() as isize) as usize;
            request.stage_id = allowed[next].0.to_owned();
            request.stage_name = allowed[next].1.to_owned();
        }
    }
    for (field, mut text) in &mut labels {
        let label = loadout_label(*field, &request);
        if text.0 != label {
            text.0 = label;
        }
    }
}

fn spawn_network_screen(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    request: &LaunchRequest,
    custom: &crate::plugins::stage_plugin::CustomStages,
    active: bool,
) {
    #[cfg(target_arch = "wasm32")]
    let _ = (request, custom);
    let (visibility, display) = screen_layout(active);
    parent
        .spawn((
            scrollable_y(Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(14.0),
                padding: UiRect::vertical(Val::Px(8.0)),
                display,
                ..default()
            }),
            LauncherScreenMarker(MenuScreen::Network),
            LauncherOuterScrollArea,
            visibility,
        ))
        .with_children(|screen| {
            screen
                .spawn((
                    UiPageRegionBundle::new(
                        Node {
                            height: Val::Px(72.0),
                            flex_direction: FlexDirection::Row,
                            flex_wrap: FlexWrap::Wrap,
                            flex_shrink: 0.0,
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(24.0),
                            row_gap: Val::Px(8.0),
                            ..default()
                        },
                        UiPageRoot::new(NETWORK_PAGE_STACK, NETWORK_BROWSER_PAGE, Display::Flex),
                    ),
                    LauncherPageHeading,
                    NetworkBrowserHeading,
                ))
                .with_children(|heading| {
                    spawn_back_button(heading, font);
                    heading
                        .spawn(Node {
                            flex_direction: FlexDirection::Column,
                            ..default()
                        })
                        .with_children(|titles| {
                            titles.spawn(launcher_text("网络对战", 9.0, RED_BRIGHT, font));
                            titles.spawn(launcher_text("联机房间", 26.0, PAPER, font));
                        });
                    heading
                        .spawn((
                            Button,
                            LauncherStyledButton,
                            Node {
                                width: Val::Px(300.0),
                                max_width: Val::Percent(100.0),
                                min_height: Val::Px(38.0),
                                flex_shrink: 0.0,
                                margin: UiRect::left(Val::Auto),
                                padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                                border: UiRect::bottom(Val::Px(1.0)),
                                align_items: AlignItems::Center,
                                column_gap: Val::Px(12.0),
                                ..default()
                            },
                            BackgroundColor(theme::CARD.with_alpha(0.45)),
                            BorderColor::all(LAUNCHER_LINE),
                            NativeTextEdit {
                                field: TextField::PlayerName,
                                value: request.player_name.clone(),
                            },
                        ))
                        .with_children(|field| {
                            field.spawn((
                                launcher_text(
                                    &format!("玩家名称：{}", request.player_name),
                                    12.0,
                                    PAPER,
                                    font,
                                ),
                                NativeTextLabel(TextField::PlayerName),
                                TextLayout::no_wrap(),
                                Node {
                                    flex_grow: 1.0,
                                    min_width: Val::Px(0.0),
                                    overflow: Overflow::clip(),
                                    ..default()
                                },
                            ));
                            field.spawn((
                                launcher_text("修改", 11.0, theme::MUTED, font),
                                Node {
                                    flex_shrink: 0.0,
                                    ..default()
                                },
                            ));
                        });
                });

            {
                screen
                    .spawn(UiPageRegionBundle::new(
                        Node {
                            width: Val::Percent(100.0),
                            min_height: Val::Px(0.0),
                            flex_grow: 1.0,
                            flex_direction: FlexDirection::Column,
                            ..default()
                        },
                        UiPageRoot::new(NETWORK_PAGE_STACK, NETWORK_BROWSER_PAGE, Display::Flex),
                    ))
                    .with_children(|browser_page| {
                        browser_page
                            .spawn((
                                Node {
                                    width: Val::Percent(100.0),
                                    min_height: Val::Px(0.0),
                                    flex_grow: 1.0,
                                    flex_direction: FlexDirection::Row,
                                    column_gap: Val::Px(10.0),
                                    row_gap: Val::Px(10.0),
                                    ..default()
                                },
                                LauncherNetworkBrowserLayout,
                            ))
                            .with_children(|layout| {
                                layout
                                    .spawn((
                                        Node {
                                            width: Val::Px(148.0),
                                            min_width: Val::Px(124.0),
                                            flex_shrink: 0.0,
                                            flex_direction: FlexDirection::Column,
                                            row_gap: Val::Px(8.0),
                                            ..default()
                                        },
                                        LauncherNetworkActionRail,
                                    ))
                                    .with_children(|actions| {
                                        spawn_compact_network_button(
                                            actions,
                                            font,
                                            "刷新房间",
                                            false,
                                            (MenuStockRefreshButton, NetworkRailAction),
                                        );
                                        spawn_compact_network_button(
                                            actions,
                                            font,
                                            "创建房间",
                                            true,
                                            (
                                                UiPageAction::show(
                                                    NETWORK_PAGE_STACK,
                                                    NETWORK_CREATE_PAGE,
                                                ),
                                                NetworkRailAction,
                                            ),
                                        );
                                        #[cfg(not(target_arch = "wasm32"))]
                                        spawn_compact_network_button(
                                            actions,
                                            font,
                                            "IP 直连",
                                            false,
                                            (
                                                UiModalAction::show(STOCK_DIRECT_MODAL),
                                                NetworkRailAction,
                                            ),
                                        );
                                    });

                                layout
                                    .spawn((
                                        UiPanelBundle::new(
                                            Node {
                                                min_width: Val::Px(0.0),
                                                min_height: Val::Px(176.0),
                                                flex_grow: 1.0,
                                                flex_direction: FlexDirection::Column,
                                                row_gap: Val::Px(8.0),
                                                padding: UiRect::all(Val::Px(10.0)),
                                                border: UiRect::ZERO,
                                                ..default()
                                            },
                                            theme::SURFACE,
                                            LAUNCHER_LINE,
                                        ),
                                        LauncherNetworkSection,
                                        LauncherNetworkRoomPanel,
                                    ))
                                    .with_children(|panel| {
                                        panel
                                            .spawn(Node {
                                                width: Val::Percent(100.0),
                                                flex_direction: FlexDirection::Row,
                                                flex_wrap: FlexWrap::Wrap,
                                                justify_content: JustifyContent::SpaceBetween,
                                                align_items: AlignItems::Center,
                                                column_gap: Val::Px(8.0),
                                                row_gap: Val::Px(3.0),
                                                ..default()
                                            })
                                            .with_children(|header| {
                                                header.spawn(launcher_text(
                                                    "房间列表",
                                                    15.0,
                                                    PAPER,
                                                    font,
                                                ));
                                                header.spawn((
                                                    launcher_text(
                                                        "正在刷新房间…",
                                                        11.0,
                                                        CYAN,
                                                        font,
                                                    ),
                                                    MenuStockDiscoveryText,
                                                ));
                                            });

                                        panel.spawn((
                                            scrollable_y(Node {
                                                width: Val::Percent(100.0),
                                                min_height: Val::Px(0.0),
                                                flex_grow: 1.0,
                                                flex_direction: FlexDirection::Column,
                                                row_gap: Val::Px(6.0),
                                                padding: UiRect::all(Val::Px(4.0)),
                                                border: UiRect::ZERO,
                                                ..default()
                                            }),
                                            BackgroundColor(Color::NONE),
                                            BorderColor::all(Color::NONE),
                                            LauncherNetworkRoomList,
                                        ));

                                        panel.spawn((
                                            launcher_text(
                                                "正在获取局域网和 Web 房间…",
                                                11.0,
                                                theme::MUTED,
                                                font,
                                            ),
                                            Node {
                                                width: Val::Percent(100.0),
                                                flex_shrink: 0.0,
                                                ..default()
                                            },
                                            MenuNetStatusText,
                                        ));
                                    });
                            });
                    });

                screen
                    .spawn((
                        UiSecondaryPageBundle::full_page(
                            UiPageRoot::new(NETWORK_PAGE_STACK, NETWORK_CREATE_PAGE, Display::Flex),
                            Display::None,
                        ),
                        LauncherCreateLayout::Page,
                        RoomMapPickerPage,
                    ))
                    .with_children(|create_page| {
                        create_page
                            .spawn(UiSecondaryHeaderBundle::default())
                            .with_children(|heading| {
                                spawn_stock_page_button(
                                    heading,
                                    font,
                                    "← 返回",
                                    UiPageAction::show(NETWORK_PAGE_STACK, NETWORK_BROWSER_PAGE),
                                );
                                heading.spawn(launcher_text("创建房间", 20.0, PAPER, font));
                            });

                        create_page
                            .spawn((
                                scrollable_y(Node {
                                    width: Val::Percent(100.0),
                                    flex_grow: 1.0,
                                    flex_basis: Val::Px(0.0),
                                    column_gap: Val::Px(12.0),
                                    row_gap: Val::Px(12.0),
                                    align_items: AlignItems::FlexStart,
                                    ..default()
                                }),
                                LauncherCreateLayout::Body,
                            ))
                            .with_children(|body| {
                                body.spawn((
                                    stock_form_section_bundle(),
                                    LauncherCreateLayout::Maps,
                                ))
                                .with_children(|section| {
                                    section.spawn(launcher_text("对战地图", 15.0, PAPER, font));
                                    spawn_room_map_picker(section, font, request, custom);
                                });

                                body.spawn((
                                    Node {
                                        flex_direction: FlexDirection::Column,
                                        row_gap: Val::Px(12.0),
                                        min_width: Val::Px(0.0),
                                        flex_shrink: 0.0,
                                        ..default()
                                    },
                                    LauncherCreateLayout::Settings,
                                ))
                                .with_children(|settings| {
                                    settings.spawn(stock_form_section_bundle()).with_children(
                                        |section| {
                                            spawn_native_text_field(
                                                section,
                                                font,
                                                TextField::RoomName,
                                                &request.room_name,
                                            );
                                            section.spawn(launcher_text(
                                                "对战模式",
                                                13.0,
                                                CYAN,
                                                font,
                                            ));
                                            section
                                                .spawn(UiActionBarBundle::wrapped())
                                                .with_children(|rules| {
                                                    for (rule, label) in [
                                                        (1, "盟主"),
                                                        (2, "劫镖"),
                                                        (3, "护城"),
                                                        (4, "暗杀"),
                                                        (5, "死斗"),
                                                    ] {
                                                        spawn_stock_selectable_button(
                                                            rules,
                                                            font,
                                                            label,
                                                            request.stock_rule == rule,
                                                            UiSelectableKind::Radio,
                                                            MenuStockRuleButton(rule),
                                                        );
                                                    }
                                                });

                                            section.spawn(launcher_text("阵营", 13.0, CYAN, font));
                                            section
                                                .spawn(UiActionBarBundle::wrapped())
                                                .with_children(|camps| {
                                                    for (camp, label) in [
                                                        (0, "无阵营"),
                                                        (1, "流星队"),
                                                        (2, "蝴蝶队"),
                                                    ] {
                                                        spawn_stock_selectable_button(
                                                            camps,
                                                            font,
                                                            label,
                                                            request.stock_camp == camp,
                                                            UiSelectableKind::Radio,
                                                            MenuStockCampButton(camp),
                                                        );
                                                    }
                                                });
                                        },
                                    );
                                });
                            });
                        create_page
                            .spawn((
                                Node {
                                    width: Val::Percent(100.0),
                                    min_height: Val::Px(56.0),
                                    flex_shrink: 0.0,
                                    align_items: AlignItems::Center,
                                    justify_content: JustifyContent::SpaceBetween,
                                    flex_wrap: FlexWrap::Wrap,
                                    column_gap: Val::Px(12.0),
                                    row_gap: Val::Px(6.0),
                                    padding: UiRect::top(Val::Px(8.0)),
                                    border: UiRect::top(Val::Px(1.0)),
                                    ..default()
                                },
                                BorderColor::all(theme::LINE),
                            ))
                            .with_children(|actions| {
                                actions.spawn((
                                    launcher_text("", 11.0, CYAN, font),
                                    Node {
                                        flex_grow: 1.0,
                                        flex_basis: Val::Px(0.0),
                                        min_width: Val::Px(0.0),
                                        ..default()
                                    },
                                    MenuNetStatusText,
                                ));
                                spawn_stock_page_button(
                                    actions,
                                    font,
                                    "高级设置",
                                    UiModalAction::show(ROOM_ADVANCED_MODAL),
                                );
                                spawn_action_button(actions, font, "创建房间", MenuStockHostButton);
                            });
                    });
            }
        });
}

fn spawn_stock_page_button<M: Bundle>(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    marker: M,
) {
    parent
        .spawn((
            Button,
            Node {
                width: Val::Px(88.0),
                min_height: Val::Px(44.0),
                flex_shrink: 0.0,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::ZERO,
                ..default()
            },
            BackgroundColor(theme::CARD),
            BorderColor::all(LAUNCHER_LINE),
            Interaction::default(),
            LauncherStyledButton,
            theme::ActionStyle::SECONDARY,
            theme::CompactAction,
            marker,
        ))
        .with_children(|button| {
            button.spawn(launcher_text(label, 13.0, PAPER, font));
        });
}

fn stock_form_section_bundle() -> UiPanelBundle {
    UiPanelBundle::new(
        Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            padding: UiRect::all(Val::Px(12.0)),
            border: UiRect::all(Val::Px(1.0)),
            flex_shrink: 0.0,
            min_width: Val::Px(0.0),
            ..default()
        },
        theme::SURFACE,
        theme::LINE,
    )
}

fn stock_selectable_style() -> UiSelectableStyle {
    UiSelectableStyle::default()
}

fn spawn_stock_selectable_button<M: Bundle>(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    selected: bool,
    kind: UiSelectableKind,
    marker: M,
) {
    spawn_stock_selectable_with_node(
        parent,
        font,
        label,
        selected,
        kind,
        Node {
            min_width: Val::Px(108.0),
            height: Val::Px(44.0),
            min_height: Val::Px(44.0),
            max_width: Val::Px(180.0),
            flex_grow: 0.0,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            column_gap: Val::Px(7.0),
            padding: UiRect::horizontal(Val::Px(10.0)),
            border: UiRect::all(Val::Px(2.0)),
            ..default()
        },
        (marker, theme::CompactAction),
    );
}

#[allow(clippy::too_many_arguments)]
fn spawn_stock_selectable_with_node<M: Bundle>(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    selected: bool,
    kind: UiSelectableKind,
    node: Node,
    marker: M,
) {
    parent
        .spawn((
            UiSelectableButtonBundle::new(node, selected, kind, stock_selectable_style()),
            LauncherStyledButton,
            marker,
        ))
        .with_children(|button| {
            spawn_selection_indicator(button, kind, selected);
            button.spawn(launcher_text(label, 13.0, PAPER, font));
        });
}

fn spawn_room_map_picker(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    request: &LaunchRequest,
    custom: &crate::plugins::stage_plugin::CustomStages,
) {
    use super::room_map_preview::{MapImage, MapPreviewHint};

    parent
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(228.0),
                flex_shrink: 0.0,
                overflow: Overflow::clip(),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::FlexEnd,
                ..default()
            },
            theme::OrnateFrame,
            BackgroundColor(theme::BACKGROUND),
            BorderColor::all(theme::METAL.with_alpha(0.50)),
            LauncherCreateLayout::MapPreview,
        ))
        .with_children(|frame| {
            frame.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    top: Val::ZERO,
                    left: Val::ZERO,
                    ..default()
                },
                ImageNode {
                    color: Color::NONE,
                    ..default()
                },
                MapImage,
            ));
            frame
                .spawn((
                    Node {
                        width: Val::Percent(100.0),
                        padding: UiRect::axes(Val::Px(14.0), Val::Px(8.0)),
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::SpaceBetween,
                        column_gap: Val::Px(10.0),
                        border: UiRect::top(Val::Px(1.0)),
                        ..default()
                    },
                    BackgroundColor(theme::SCRIM.with_alpha(0.92)),
                    BorderColor::all(theme::LINE),
                ))
                .with_children(|caption| {
                    caption
                        .spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(2.0),
                            min_width: Val::Px(0.0),
                            flex_grow: 1.0,
                            ..default()
                        })
                        .with_children(|titles| {
                            titles.spawn((
                                launcher_text("", 18.0, theme::TEXT, font),
                                MenuStockStageText,
                            ));
                            titles.spawn((
                                launcher_text("", 11.0, theme::METAL, font),
                                MapPreviewHint,
                            ));
                        });
                    caption
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(6.0),
                            flex_shrink: 0.0,
                            ..default()
                        })
                        .with_children(|nav| {
                            for (step, label) in [(-1, "← 上一图"), (1, "下一图 →")] {
                                nav.spawn((
                                    Button,
                                    LauncherStyledButton,
                                    RoomStageStepArrow(step),
                                    theme::ActionStyle::SECONDARY,
                                    Node {
                                        height: Val::Px(34.0),
                                        min_height: Val::Px(34.0),
                                        padding: UiRect::horizontal(Val::Px(10.0)),
                                        align_items: AlignItems::Center,
                                        justify_content: JustifyContent::Center,
                                        border: UiRect::all(Val::Px(1.0)),
                                        ..default()
                                    },
                                    BackgroundColor(theme::CARD),
                                    BorderColor::all(theme::LINE),
                                ))
                                .with_children(|b| {
                                    b.spawn(launcher_text(label, 12.0, theme::ACCENT_TEXT, font));
                                });
                            }
                        });
                });
        });
    parent
        .spawn(scrollable_y(Node {
            width: Val::Percent(100.0),
            max_height: Val::Px(112.0),
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            ..default()
        }))
        .with_children(|description| {
            description.spawn((
                launcher_text("", 13.0, theme::TEXT, font),
                super::stage_presentation::StageDescription::default(),
            ));
        });
    parent
        .spawn((
            scrollable_y(Node {
                width: Val::Percent(100.0),
                max_height: Val::Px(320.0),
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                align_content: AlignContent::FlexStart,
                column_gap: Val::Percent(2.0),
                row_gap: Val::Px(8.0),
                flex_shrink: 0.0,
                ..default()
            }),
            LauncherCreateLayout::MapList,
        ))
        .with_children(|stages| {
            for stage in available_room_stages(custom) {
                spawn_stock_stage_choice(
                    stages,
                    font,
                    &stage.id,
                    &stage.name,
                    request.stock_rule,
                    request.stage_id == stage.id,
                );
            }
        });
}

fn spawn_stock_stage_choice(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    stage_id: &str,
    stage_name: &str,
    rule: i32,
    selected: bool,
) {
    use super::room_map_preview::StageThumbnail;
    let visible = super::network_plugin::stock_stage_allowed_for_rule(rule, stage_id);
    let badge = stage_id
        .strip_prefix("sn")
        .or_else(|| stage_id.strip_prefix("SN"))
        .map_or_else(|| stage_id.to_uppercase(), |num| format!("卷 {num}"));
    let card_style = UiSelectableStyle::new(
        theme::SURFACE.with_alpha(0.92),
        theme::SELECTED.with_alpha(0.96),
        theme::LINE,
        theme::ACCENT_TEXT,
    );
    parent
        .spawn((
            UiSelectableButtonBundle::new(
                Node {
                    width: Val::Percent(32.0),
                    min_width: Val::Px(132.0),
                    height: Val::Px(68.0),
                    min_height: Val::Px(68.0),
                    display: if visible {
                        Display::Flex
                    } else {
                        Display::None
                    },
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::FlexEnd,
                    overflow: Overflow::clip(),
                    border: UiRect::all(Val::Px(1.5)),
                    ..default()
                },
                selected && visible,
                UiSelectableKind::Radio,
                card_style,
            ),
            LauncherStyledButton,
            MenuStockStageButton {
                id: stage_id.to_string(),
                name: stage_name.to_string(),
            },
            LauncherCreateLayout::MapChoice,
        ))
        .with_children(|card| {
            card.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::ZERO,
                    top: Val::ZERO,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                ImageNode {
                    color: Color::NONE,
                    ..default()
                },
                StageThumbnail(stage_id.to_string()),
            ));

            card.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(6.0),
                    top: Val::Px(5.0),
                    padding: UiRect::axes(Val::Px(5.0), Val::Px(1.5)),
                    border: UiRect::left(Val::Px(2.0)),
                    ..default()
                },
                BackgroundColor(theme::SCRIM),
                BorderColor::all(theme::ACCENT),
            ))
            .with_children(|tag| {
                tag.spawn(launcher_text(&badge, 10.0, theme::METAL, font));
            });

            card.spawn((
                Node {
                    width: Val::Percent(100.0),
                    padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::SpaceBetween,
                    border: UiRect::top(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(theme::SCRIM.with_alpha(0.88)),
                BorderColor::all(theme::LINE),
            ))
            .with_children(|footer| {
                footer.spawn(launcher_text(stage_name, 14.0, PAPER, font));
            });
        });
}

pub(crate) fn available_room_stages(
    custom: &crate::plugins::stage_plugin::CustomStages,
) -> Vec<meteor_scene::stage_registry::StageInfo> {
    let candidates = official_stages()
        .into_iter()
        .chain(custom.stages.iter().cloned());
    let manifest = meteor_asset::runtime_assets::manifest().or_else(|| {
        #[cfg(target_os = "android")]
        {
            crate::android_assets::authoring_manifest()
        }
        #[cfg(not(target_os = "android"))]
        {
            None
        }
    });
    if let Some(manifest) = manifest {
        let Ok(index) = meteor_asset::pack_manifest::parse_pack_index(&manifest) else {
            return Vec::new();
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&manifest) else {
            return Vec::new();
        };
        return candidates
            .filter(|stage| {
                let Some(pack) = index.get(&format!("stage/{}", stage.id)) else {
                    return false;
                };
                if !stage_has_required_entries(&json, &stage.id) {
                    return false;
                }
                #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
                {
                    return meteor_asset::pack_repository::default_pack_directory()
                        .ok()
                        .is_some_and(|dir| dir.join(&pack.file_name).is_file());
                }
                #[cfg(any(target_arch = "wasm32", target_os = "android"))]
                {
                    let _ = pack;
                    true
                }
            })
            .map(with_stage_display_name)
            .collect();
    }
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    if let Some(ids) = meteor_asset::runtime_assets::original_stage_ids() {
        return ids
            .iter()
            .map(|id| meteor_scene::stage_registry::get_stage(id, &custom.stages))
            .map(with_stage_display_name)
            .collect();
    }
    #[cfg(test)]
    {
        candidates.map(with_stage_display_name).collect()
    }
    #[cfg(not(test))]
    {
        Vec::new()
    }
}
fn with_stage_display_name(
    mut stage: meteor_scene::stage_registry::StageInfo,
) -> meteor_scene::stage_registry::StageInfo {
    stage.name = super::stage_presentation::display_name(&stage.id, &stage.name);
    stage
}

fn stage_has_required_entries(manifest: &serde_json::Value, id: &str) -> bool {
    let pack_id = format!("stage/{id}");
    manifest["packs"].as_array().is_some_and(|packs| {
        packs.iter().any(|item| {
            item["packId"].as_str() == Some(pack_id.as_str())
                && item["entries"].as_array().is_some_and(|entries| {
                    meteor_asset::runtime_assets::ROOM_STAGE_EXTENSIONS
                        .iter()
                        .all(|ext| {
                            let expected = format!("stage/{id}.{ext}");
                            entries
                                .iter()
                                .any(|entry| entry.as_str() == Some(expected.as_str()))
                        })
                })
        })
    })
}

fn spawn_room_advanced_dialog(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    request: &LaunchRequest,
) {
    parent.spawn((
        UiModalOverlayBundle::new(
            UiModalRoot::new(ROOM_ADVANCED_MODAL, Display::Flex),
            Display::None, theme::SCRIM, 305,
        ),
        RoomAdvancedDialog,
    )).with_children(|overlay| {
        overlay.spawn(UiDialogSurfaceBundle::new(752.0, PANEL, theme::ACCENT))
            .with_children(|dialog| {
                dialog.spawn(UiSecondaryHeaderBundle::default()).with_children(|header| {
                    header.spawn((launcher_text("高级设置", 19.0, PAPER, font),
                        Node { flex_grow: 1.0, ..default() }));
                    spawn_stock_page_button(header, font, "完成", UiModalAction::hide(ROOM_ADVANCED_MODAL));
                });
                dialog.spawn(scrollable_y(Node {
                    width: Val::Percent(100.0),
                    min_height: Val::Px(0.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(10.0),
                    ..default()
                })).with_children(|settings| {
                    #[cfg(not(target_arch = "wasm32"))]
                    if !super::network_plugin::default_web_address().is_empty() {
                        settings.spawn(stock_form_section_bundle()).with_children(|network| {
                            spawn_stock_selectable_button(
                                network, font, "公网对战", request.public_battle,
                                UiSelectableKind::Checkbox, MenuPublicBattleButton,
                            );
                        });
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    spawn_room_physics_slider(settings, font, request);
                    settings.spawn(stock_form_section_bundle()).with_children(|timing| {
                        for (field, label, options) in [
                            (StockConfigField::GameNum, "局数", &[(1, "1 局"), (3, "3 局"), (5, "5 局"), (10, "10 局"), (999, "999 局"), (9_999, "无限局")][..]),
                            (StockConfigField::GameTime, "单场时限", &[(5, "5 分钟"), (10, "10 分钟"), (15, "15 分钟"), (30, "30 分钟"), (60, "60 分钟"), (999, "不限时")][..]),
                            (StockConfigField::LifeAdjust, "生命调整", &[(50, "50%"), (100, "100%"), (150, "150%"), (200, "200%"), (300, "300%"), (500, "500%")][..]),
                        ] {
                            let current = match field {
                                StockConfigField::GameNum => request.stock_game_num,
                                StockConfigField::GameTime => request.stock_game_time_minutes,
                                StockConfigField::LifeAdjust => request.stock_life_adjust_percent,
                            };
                            timing.spawn(UiActionBarBundle::wrapped()).with_children(|title| {
                                title.spawn(launcher_text(label, 13.0, PAPER, font));
                                title.spawn((launcher_text(
                                    &super::network_plugin::stock_config_value_label(field, request), 12.0, theme::MUTED, font,
                                ), MenuStockSelectText(field)));
                            });
                            timing.spawn(UiActionBarBundle::wrapped()).with_children(|choices| {
                                for &(value, label) in options {
                                    spawn_stock_selectable_button(choices, font, label,
                                        value == current, UiSelectableKind::Radio, MenuStockSelectOption { field, value });
                                }
                            });
                        }
                    });
                    settings.spawn(stock_form_section_bundle()).with_children(|section| {
                        section.spawn(launcher_text("场景道具", 13.0, PAPER, font));
                        section.spawn(UiActionBarBundle::wrapped()).with_children(|choices| {
                            for (rule, label, enabled) in [
                                (StockItemRule::Hide, "隐藏密宝", request.stock_hide_item),
                                (StockItemRule::Health, "恢复道具", request.stock_health_item),
                                (StockItemRule::Update, "提升道具", request.stock_update_item),
                                (StockItemRule::Special, "特殊道具", request.stock_special_item),
                            ] {
                                spawn_stock_selectable_button(choices, font, label, enabled,
                                    UiSelectableKind::Checkbox, MenuStockItemRuleButton(rule));
                            }
                        });
                        spawn_stock_selectable_button(section, font, "再平衡", request.balance_rules,
                            UiSelectableKind::Checkbox, crate::app_state::MenuBalanceRulesButton);
                    });
                });
            });
    });
}
#[cfg(not(target_arch = "wasm32"))]
fn spawn_room_physics_slider(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    request: &LaunchRequest,
) {
    use super::ui_components::slider::{spawn_slider, UiSlider};
    parent
        .spawn((
            Node {
                display: if request.public_battle {
                    Display::None
                } else {
                    Display::Flex
                },
                width: Val::Percent(100.0),
                align_items: AlignItems::Center,
                column_gap: Val::Px(12.0),
                ..default()
            },
            RoomPhysicsSection,
        ))
        .with_children(|row| {
            row.spawn(launcher_text("局域网判定频率", 13.0, PAPER, font));
            spawn_slider(
                row,
                UiSlider {
                    min: meteor_core::PHYSICS_HZ_MIN as f32,
                    max: meteor_core::PHYSICS_HZ_MAX as f32,
                    step: 1.0,
                    value: request.stock_physics_hz as f32,
                },
                RoomPhysicsSlider,
            );
            row.spawn((
                launcher_text(
                    &format!("{} Hz", request.stock_physics_hz),
                    13.0,
                    theme::ACCENT_TEXT,
                    font,
                ),
                Node {
                    width: Val::Px(54.0),
                    flex_shrink: 0.0,
                    ..default()
                },
                RoomPhysicsLabel,
            ));
        });
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn room_physics_slider_changes(
    mut changes: MessageReader<super::ui_components::slider::UiSliderChanged>,
    sliders: Query<(), With<RoomPhysicsSlider>>,
    mut labels: Query<&mut Text, With<RoomPhysicsLabel>>,
    mut sections: Query<&mut Node, With<RoomPhysicsSection>>,
    mut request: ResMut<LaunchRequest>,
) {
    for change in changes.read() {
        if !request.public_battle && sliders.contains(change.entity) {
            request.stock_physics_hz = (change.value.round() as u32)
                .clamp(meteor_core::PHYSICS_HZ_MIN, meteor_core::PHYSICS_HZ_MAX);
        }
    }
    if request.is_changed() {
        for mut node in &mut sections {
            node.display = if request.public_battle {
                Display::None
            } else {
                Display::Flex
            };
        }
        let value = format!("{} Hz", request.stock_physics_hz);
        for mut text in &mut labels {
            if text.0 != value {
                text.0.clone_from(&value);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_stock_loadout_dialog(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    request: &LaunchRequest,
) {
    parent
        .spawn((
            UiModalOverlayBundle::new(
                UiModalRoot::new(UiModalId(2), Display::Flex),
                Display::None,
                theme::SCRIM,
                310,
            ),
            MenuStockLoadoutDialog,
        ))
        .with_children(|overlay| {
            overlay
                .spawn((
                    UiDialogSurfaceBundle::new(880.0, PANEL, theme::ACCENT),
                    ScrollPosition::default(),
                    LauncherScrollArea,
                    Interaction::default(),
                ))
                .with_children(|dialog| {
                    dialog
                        .spawn(Node {
                            width: Val::Percent(100.0),
                            justify_content: JustifyContent::SpaceBetween,
                            align_items: AlignItems::Center,
                            padding: UiRect::bottom(Val::Px(6.0)),
                            border: UiRect::bottom(Val::Px(1.0)),
                            ..default()
                        })
                        .insert(BorderColor::all(theme::LINE))
                        .with_children(|header| {
                            header.spawn(launcher_text("出 战 整 备", 22.0, PAPER, font));
                            header.spawn((
                                launcher_text("正在读取房间模式…", 13.0, theme::METAL, font),
                                crate::plugins::network_plugin::StockLoadoutModeText,
                            ));
                        });

                    dialog
                        .spawn(Node {
                            width: Val::Percent(100.0),
                            flex_direction: FlexDirection::Row,
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: Val::Px(18.0),
                            row_gap: Val::Px(14.0),
                            align_items: AlignItems::Stretch,
                            ..default()
                        })
                        .with_children(|columns| {

                            columns
                                .spawn((
                                    theme::OrnateFrame,
                                    Node {
                                        width: Val::Px(340.0),
                                        max_width: Val::Percent(100.0),
                                        min_height: Val::Px(300.0),
                                        flex_grow: 1.0,
                                        flex_direction: FlexDirection::Column,
                                        justify_content: JustifyContent::FlexEnd,
                                        padding: UiRect::all(Val::Px(12.0)),
                                        border: UiRect::all(Val::Px(1.0)),
                                        overflow: Overflow::clip(),
                                        ..default()
                                    },
                                    BackgroundColor(theme::SURFACE.with_alpha(0.88)),
                                    BorderColor::all(theme::METAL.with_alpha(0.55)),
                                ))
                                .with_children(|stage| {
                                    stage.spawn((
                                        super::character_plugin::preview::CharacterPreviewViewport,
                                        Node {
                                            position_type: PositionType::Absolute,
                                            left: Val::ZERO,
                                            right: Val::ZERO,
                                            top: Val::ZERO,
                                            bottom: Val::ZERO,
                                            width: Val::Percent(100.0),
                                            height: Val::Percent(100.0),
                                            ..default()
                                        },
                                        ZIndex(0),
                                    ));
                                });

                            columns
                                .spawn(Node {
                                    width: Val::Px(460.0),
                                    max_width: Val::Percent(100.0),
                                    flex_grow: 1.0,
                                    flex_direction: FlexDirection::Column,
                                    justify_content: JustifyContent::SpaceBetween,
                                    row_gap: Val::Px(12.0),
                                    ..default()
                                })
                                .with_children(|controls| {
                                    controls
                                        .spawn((
                                            Button,
                                            Node {
                                                width: Val::Percent(100.0),
                                                min_height: Val::Px(42.0),
                                                padding: UiRect::axes(
                                                    Val::Px(12.0),
                                                    Val::Px(8.0),
                                                ),
                                                border: UiRect::all(Val::Px(1.0)),
                                                align_items: AlignItems::Center,
                                                ..default()
                                            },
                                            BackgroundColor(theme::SURFACE),
                                            BorderColor::all(theme::LINE),
                                            NativeTextEdit {
                                                field: TextField::StockPassword,
                                                value: String::new(),
                                            },
                                            crate::plugins::network_plugin::StockPasswordSection,
                                        ))
                                        .with_children(|field| {
                                            field.spawn((
                                                launcher_text(
                                                    "房间密码（无密码留空）：点击输入",
                                                    13.0,
                                                    PAPER,
                                                    font,
                                                ),
                                                NativeTextLabel(TextField::StockPassword),
                                            ));
                                        });

                                    spawn_player_loadout(controls, font, request);

                                    controls
                                        .spawn((
                                            UiActionBarBundle::wrapped(),
                                            crate::plugins::network_plugin::StockLoadoutCampSection,
                                        ))
                                        .with_children(|camps| {
                                            for (camp, label) in
                                                [(1, "流星队"), (2, "蝴蝶队")]
                                            {
                                                spawn_stock_selectable_button(
                                                    camps,
                                                    font,
                                                    label,
                                                    request.stock_camp == camp,
                                                    UiSelectableKind::Radio,
                                                    crate::plugins::network_plugin::StockLoadoutCamp(
                                                        camp,
                                                    ),
                                                );
                                            }
                                        });

                                    controls
                                        .spawn(UiActionBarBundle::single_row())
                                        .with_children(|actions| {
                                            spawn_compact_network_button(
                                                actions,
                                                font,
                                                "取消",
                                                false,
                                                MenuStockLoadoutCancel,
                                            );
                                            spawn_compact_network_button(
                                                actions,
                                                font,
                                                "开始",
                                                true,
                                                MenuStockLoadoutConfirm,
                                            );
                                        });
                                });
                        });
                });
        });
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_stock_connection_dialog(parent: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    parent
        .spawn((
            UiModalOverlayBundle::new(
                UiModalRoot::new(UiModalId(3), Display::Flex),
                Display::None,
                theme::SCRIM,
                320,
            ),
            StockConnectionDialogRoot,
        ))
        .with_children(|overlay| {
            overlay
                .spawn((
                    UiDialogSurfaceBundle::new(480.0, PANEL, RED_BRIGHT),
                    ScrollPosition::default(),
                    LauncherScrollArea,
                    Interaction::default(),
                ))
                .with_children(|dialog| {
                    dialog.spawn((
                        launcher_text("局域网连接", 18.0, PAPER, font),
                        StockConnectionText::Title,
                    ));
                    dialog.spawn((
                        launcher_text("正在联系房主…", 14.0, PAPER, font),
                        StockConnectionText::Detail,
                    ));
                    dialog
                        .spawn(UiActionBarBundle::single_row())
                        .with_children(|actions| {
                            spawn_compact_network_button(
                                actions,
                                font,
                                "重试连接",
                                false,
                                StockConnectionAction::Retry,
                            );
                            actions
                                .spawn((
                                    Button,
                                    Node {
                                        min_width: Val::Px(96.0),
                                        min_height: Val::Px(44.0),
                                        flex_grow: 1.0,
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        padding: UiRect::horizontal(Val::Px(10.0)),
                                        border: UiRect::all(Val::Px(1.0)),
                                        border_radius: BorderRadius::ZERO,
                                        ..default()
                                    },
                                    BackgroundColor(STAGE_CARD),
                                    BorderColor::all(LAUNCHER_LINE),
                                    LauncherStyledButton,
                                    StockConnectionAction::Cancel,
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        launcher_text("取消连接", 14.0, PAPER, font),
                                        StockConnectionText::Cancel,
                                    ));
                                });
                        });
                });
        });
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_stock_direct_dialog(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    request: &LaunchRequest,
) {
    parent
        .spawn(UiModalOverlayBundle::new(
            UiModalRoot::new(STOCK_DIRECT_MODAL, Display::Flex),
            Display::None,
            theme::SCRIM,
            300,
        ))
        .with_children(|overlay| {
            overlay
                .spawn((
                    UiDialogSurfaceBundle::new(560.0, PANEL, theme::ACCENT),
                    ScrollPosition::default(),
                    LauncherScrollArea,
                    Interaction::default(),
                ))
                .with_children(|dialog| {
                    dialog.spawn(launcher_text("直接连接", 18.0, PAPER, font));
                    dialog.spawn(launcher_text(
                        "输入房主 IP:端口；仅在自动搜索不到房间或跨网段时使用。",
                        11.0,
                        theme::MUTED,
                        font,
                    ));
                    spawn_native_text_field(
                        dialog,
                        font,
                        TextField::Address,
                        &request.stock_server_address,
                    );
                    dialog
                        .spawn(UiActionBarBundle::single_row())
                        .with_children(|actions| {
                            spawn_compact_network_button(
                                actions,
                                font,
                                "取消",
                                false,
                                UiModalAction::hide(STOCK_DIRECT_MODAL),
                            );
                            spawn_compact_network_button(
                                actions,
                                font,
                                "加入",
                                false,
                                (MenuStockJoinButton, UiModalAction::hide(STOCK_DIRECT_MODAL)),
                            );
                        });
                });
        });
}

fn spawn_compact_network_button<M: Bundle>(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    primary: bool,
    marker: M,
) {
    parent
        .spawn((
            Button,
            Node {
                min_width: Val::Px(64.0),
                min_height: Val::Px(44.0),
                flex_grow: 0.0,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(10.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::ZERO,
                ..default()
            },
            BackgroundColor(STAGE_CARD),
            BorderColor::all(LAUNCHER_LINE),
            Interaction::default(),
            LauncherStyledButton,
            theme::ActionStyle { primary },
            theme::CompactAction,
            marker,
        ))
        .with_children(|button| {
            button.spawn(launcher_text(label, 14.0, PAPER, font));
        });
}

pub(crate) fn spawn_network_room_button(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    _label: &str,
    target: RoomTarget,
) {
    let (source, title, detail, count) = target.presentation();
    parent
        .spawn((
            Button,
            Node {
                width: Val::Percent(100.0),
                min_height: Val::Px(64.0),
                flex_shrink: 0.0,
                align_items: AlignItems::Center,
                column_gap: Val::Px(14.0),
                padding: UiRect::axes(Val::Px(12.0), Val::Px(8.0)),
                border: UiRect::bottom(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(Color::NONE),
            BorderColor::all(theme::LINE),
            Interaction::default(),
            LauncherStyledButton,
            theme::ActionStyle::SECONDARY,
            LauncherNetworkRoomButton,
            target,
        ))
        .with_children(|row| {
            row.spawn((
                Node {
                    width: Val::Px(56.0),
                    flex_shrink: 0.0,
                    min_height: Val::Px(28.0),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::ZERO,
                    ..default()
                },
                BorderColor::all(theme::METAL.with_alpha(0.45)),
                BackgroundColor(theme::SURFACE.with_alpha(0.78)),
            ))
            .with_children(|badge| {
                badge.spawn(launcher_text(source, 11.0, theme::ACCENT_TEXT, font));
            });
            row.spawn(Node {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_basis: Val::Px(0.0),
                min_width: Val::Px(0.0),
                row_gap: Val::Px(5.0),
                ..default()
            })
            .with_children(|labels| {
                labels.spawn(launcher_text(&title, 16.0, theme::TEXT, font));
                labels.spawn(launcher_text(&detail, 11.0, theme::MUTED, font));
            });
            row.spawn(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexEnd,
                row_gap: Val::Px(3.0),
                min_width: Val::Px(64.0),
                flex_shrink: 0.0,
                ..default()
            })
            .with_children(|right| {
                right.spawn(launcher_text(&count, 13.0, theme::TEXT, font));
                right.spawn(launcher_text("加入 →", 11.0, theme::METAL, font));
            });
        });
}

pub(crate) fn spawn_full_settings_page(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    settings: &Settings,
    keymap: &Keymap,
    in_game: bool,
) {
    use crate::plugins::settings_ui as page;

    parent
        .spawn((
            Node {
                width: Val::Percent(100.0),
                max_width: Val::Px(1056.0),
                flex_grow: 1.0,
                min_height: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::new(Val::Px(28.0), Val::Px(28.0), Val::Px(14.0), Val::Px(10.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundGradient(vec![LinearGradient::new(
                LinearGradient::TO_RIGHT,
                vec![
                    ColorStop::new(theme::SURFACE, Val::Percent(0.0)),
                    ColorStop::new(theme::SURFACE.with_alpha(0.86), Val::Percent(55.0)),
                    ColorStop::new(theme::SURFACE.with_alpha(0.78), Val::Percent(100.0)),
                ],
            )
            .into()]),
            BorderColor::all(theme::LINE),
            LauncherSettingsPanel,
        ))
        .with_children(|panel| {
            panel
                .spawn((page::settings_header_bundle(), LauncherPageHeading))
                .with_children(|heading| {
                    if in_game {
                        spawn_settings_header_back(heading, font);
                    } else {
                        spawn_back_button(heading, font);
                    }
                    page::spawn_settings_title(heading, font);
                    heading.spawn(Node {
                        flex_grow: 1.0,
                        ..default()
                    });
                    if in_game {
                        spawn_settings_header_close(heading, font);
                    } else {
                        spawn_settings_close_button(heading, font);
                    }
                });
            page::spawn_settings_rule(panel);
            panel
                .spawn(Node {
                    width: Val::Percent(100.0),
                    flex_grow: 1.0,
                    min_height: Val::Px(0.0),
                    flex_basis: Val::Px(0.0),
                    justify_content: JustifyContent::Center,
                    overflow: Overflow::clip(),
                    ..default()
                })
                .with_children(|form| {
                    page::spawn_web_settings_form(form, font, settings, keymap);
                });
            page::spawn_settings_footer(panel, font, |actions| {
                spawn_settings_secondary_button(actions, font, "恢复默认设置");
                if in_game {
                    spawn_settings_back_button(actions, font, "确认");
                } else {
                    spawn_settings_confirm_button(actions, font);
                }
            });
        });
}

#[derive(Component)]
pub(crate) struct LauncherSettingsPanel;

pub(crate) fn adapt_settings_panel(
    windows: Query<&Window>,
    mut panels: Query<&mut Node, With<LauncherSettingsPanel>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let layout = responsive_ui_layout(window.width(), window.height(), cfg!(target_os = "android"));
    let (h, top) = if layout.mobile {
        (10.0, 6.0)
    } else {
        (28.0, 14.0)
    };
    for mut node in &mut panels {
        node.padding = UiRect::new(Val::Px(h), Val::Px(h), Val::Px(top), Val::Px(top.min(10.0)));
    }
}

fn spawn_settings_header_back(parent: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    parent
        .spawn((
            Button,
            Node {
                min_height: Val::Px(36.0),
                padding: UiRect {
                    left: Val::Px(28.0),
                    right: Val::Px(16.0),
                    ..default()
                },
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::ZERO,
                ..default()
            },
            BackgroundColor(theme::SURFACE),
            BorderColor::all(theme::LINE),
            Interaction::default(),
            LauncherStyledButton,
            theme::ActionStyle::SECONDARY,
            crate::app_state::SettingsBackButton,
        ))
        .with_children(|btn| {
            btn.spawn(launcher_text("返回", 14.0, CYAN, font));
        });
}

fn spawn_settings_header_close(parent: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    parent
        .spawn((
            Button,
            Node {
                width: Val::Px(34.0),
                height: Val::Px(34.0),
                min_width: Val::Px(34.0),
                min_height: Val::Px(34.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::ZERO,
                ..default()
            },
            BackgroundColor(theme::SURFACE),
            BorderColor::all(theme::LINE),
            Interaction::default(),
            LauncherStyledButton,
            theme::ActionStyle::SECONDARY,
            crate::app_state::SettingsBackButton,
        ))
        .with_children(|btn| {
            btn.spawn(launcher_text("×", 22.0, theme::MUTED, font));
        });
}

#[allow(dead_code)]
fn spawn_settings_fieldset(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    legend: &str,
    rows: &[(&str, &str)],
) {
    parent
        .spawn((
            Node {
                width: Val::Px(220.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(18.0)),
                row_gap: Val::Px(14.0),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(PANEL),
            BorderColor::all(theme::LINE),
        ))
        .with_children(|fieldset| {
            fieldset.spawn(launcher_text(legend, 18.0, RED_BRIGHT, font));
            for (label, value) in rows {
                fieldset
                    .spawn((Node {
                        width: Val::Percent(100.0),
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(10.0),
                        ..default()
                    },))
                    .with_children(|row| {
                        row.spawn(launcher_text(label, 12.0, theme::MUTED, font));
                        row.spawn(launcher_text(value, 12.0, theme::MUTED, font));
                    });
            }
        });
}

fn spawn_settings_close_button(parent: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    parent
        .spawn((
            Button,
            Node {
                width: Val::Px(34.0),
                height: Val::Px(34.0),
                min_width: Val::Px(34.0),
                min_height: Val::Px(34.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::ZERO,
                ..default()
            },
            BackgroundColor(theme::SURFACE),
            BorderColor::all(theme::LINE),
            Interaction::default(),
            LauncherStyledButton,
            theme::ActionStyle::SECONDARY,
            MenuBackButton,
        ))
        .with_children(|btn| {
            btn.spawn(launcher_text("×", 22.0, theme::MUTED, font));
        });
}

pub(crate) fn spawn_settings_secondary_button(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
) {
    parent
        .spawn((
            Button,
            Node {
                min_width: Val::Px(132.0),
                min_height: Val::Px(36.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(16.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::ZERO,
                ..default()
            },
            BackgroundColor(theme::SURFACE),
            BorderColor::all(theme::LINE),
            Interaction::default(),
            LauncherStyledButton,
            theme::ActionStyle::SECONDARY,
            SettingsResetButton,
        ))
        .with_children(|btn| {
            btn.spawn(launcher_text(label, 13.0, CYAN, font));
        });
}

pub(crate) fn spawn_settings_back_button(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
) {
    parent
        .spawn((
            Button,
            Node {
                min_width: Val::Px(96.0),
                min_height: Val::Px(36.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(16.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::ZERO,
                ..default()
            },
            BackgroundColor(theme::PRIMARY),
            BorderColor::all(theme::ACCENT),
            Interaction::default(),
            LauncherStyledButton,
            theme::ActionStyle::SECONDARY,
            crate::app_state::SettingsBackButton,
        ))
        .with_children(|btn| {
            btn.spawn(launcher_text(label, 14.0, theme::TEXT, font));
        });
}

fn spawn_settings_confirm_button(parent: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    parent
        .spawn((
            Button,
            Node {
                min_width: Val::Px(96.0),
                min_height: Val::Px(36.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(16.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::ZERO,
                ..default()
            },
            BackgroundColor(ACTION_RED),
            BorderColor::all(theme::ACCENT),
            Interaction::default(),
            LauncherStyledButton,
            theme::ActionStyle::SECONDARY,
            MenuNavigateButton(MenuScreen::Main),
        ))
        .with_children(|btn| {
            btn.spawn(launcher_text("确认", 14.0, theme::TEXT, font));
        });
}

fn spawn_replay_screen(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, active: bool) {
    let (visibility, display) = screen_layout(active);
    parent
        .spawn((
            scrollable_y(Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(16.0),
                display,
                ..default()
            }),
            LauncherScreenMarker(MenuScreen::Replay),
            visibility,
        ))
        .with_children(|screen| {
            screen
                .spawn((
                    Node {
                        height: Val::Px(72.0),
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(24.0),
                        ..default()
                    },
                    LauncherPageHeading,
                ))
                .with_children(|heading| {
                    spawn_back_button(heading, font);
                    heading
                        .spawn(Node {
                            flex_direction: FlexDirection::Column,
                            ..default()
                        })
                        .with_children(|titles| {
                            titles.spawn(launcher_text("战况纪录", 9.0, RED_BRIGHT, font));
                            titles.spawn(launcher_text("MRC 录像回放", 26.0, PAPER, font));
                        });
                });
            screen.spawn(launcher_text(
                "选择本机的 .mrc 录像文件开始回放",
                13.0,
                theme::MUTED,
                font,
            ));
            screen.spawn(launcher_text(
                "键盘 ←/→ 快退/快进 5 秒，空格暂停/继续",
                13.0,
                theme::MUTED,
                font,
            ));
            screen.spawn((
                launcher_text("", 13.0, theme::MUTED, font),
                super::mrc_file_picker::MrcFileStatus,
            ));
            spawn_hero_nav_button(
                screen,
                font,
                "选择 MRC 文件",
                (LauncherNavButton, MenuMrcButton),
            );
        });
}

fn spawn_hero_nav_button<M: Bundle>(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    marker: M,
) {
    parent
        .spawn((
            Button,
            Node {
                width: Val::Percent(100.0),
                max_width: Val::Px(320.0),
                height: Val::Px(48.0),
                min_height: Val::Px(44.0),
                max_height: Val::Px(48.0),
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(22.0)),
                border: UiRect::bottom(Val::Px(1.0)),
                border_radius: BorderRadius::ZERO,
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(Color::NONE),
            theme::hero_gradient(false, false),
            BorderColor::all(theme::LINE),
            LauncherStyledButton,
            HeroNavButton,
            marker,
        ))
        .with_children(|button| {
            button.spawn(launcher_text(label, 20.0, theme::TEXT, font));
            button.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Px(2.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(Color::NONE),
                HeroNavAccent,
            ));
        });
}

pub(crate) fn hero_nav_feedback(
    mut buttons: Query<
        (
            &Interaction,
            &Children,
            &mut BackgroundGradient,
            &mut BorderColor,
            &mut Node,
        ),
        With<HeroNavButton>,
    >,
    mut accents: Query<&mut BackgroundColor, With<HeroNavAccent>>,
    mut layouts: Query<&mut Node, (With<HeroMenuLayout>, Without<HeroNavButton>)>,
    windows: Query<&Window>,
    scale: Option<Res<EffectiveUiScale>>,
) {
    let unit = scale.as_ref().map_or(1.0, |s| s.0.max(0.05));
    let mobile = windows.single().is_ok_and(|w| {
        responsive_ui_layout(w.width(), w.height(), cfg!(target_os = "android")).mobile
    });
    for mut node in &mut layouts {
        node.justify_content = if mobile {
            JustifyContent::Center
        } else {
            JustifyContent::FlexStart
        };
        node.padding.top = if mobile { px(8) } else { Val::Vh(7.0) };
        node.padding.left = if mobile {
            Val::Px(2.0 / unit)
        } else {
            Val::Percent(1.5)
        };
        node.row_gap = Val::Px(if mobile { 12.0 / unit } else { 32.0 });
    }
    for (state, children, mut gradient, mut border, mut node) in &mut buttons {
        let selected = *state != Interaction::None;
        let pressed = *state == Interaction::Pressed;
        *gradient = theme::hero_gradient(selected, pressed);
        border.set_all(if selected {
            theme::ACCENT.with_alpha(0.72)
        } else {
            theme::LINE
        });
        node.height = Val::Px(if mobile { 48.0 / unit } else { 48.0 });
        node.min_height = node.height;
        node.max_height = node.height;
        for child in children.iter() {
            if let Ok(mut accent) = accents.get_mut(child) {
                accent.0 = if selected {
                    theme::CRIMSON_BRIGHT
                } else {
                    Color::NONE
                };
            }
        }
    }
}

fn spawn_back_button(parent: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    parent
        .spawn((
            Button,
            Node {
                min_height: Val::Px(36.0),
                padding: UiRect {
                    left: Val::Px(28.0),
                    right: Val::Px(16.0),
                    ..default()
                },
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::ZERO,
                ..default()
            },
            BackgroundColor(theme::SURFACE),
            BorderColor::all(theme::LINE),
            Interaction::default(),
            LauncherStyledButton,
            theme::ActionStyle::SECONDARY,
            MenuBackButton,
        ))
        .with_children(|btn| {
            btn.spawn(launcher_text("返回", 14.0, CYAN, font));
        });
}

fn spawn_action_button<M: Bundle>(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    marker: M,
) {
    parent
        .spawn((
            Button,
            Node {
                width: Val::Px(140.0),
                max_width: Val::Percent(100.0),
                min_height: Val::Px(44.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::ZERO,
                ..default()
            },
            BackgroundColor(ACTION_RED),
            BorderColor::all(theme::ACCENT),
            Interaction::default(),
            LauncherStyledButton,
            theme::ActionStyle::PRIMARY,
            theme::CompactAction,
            marker,
        ))
        .with_children(|btn| {
            btn.spawn(launcher_text(label, 16.0, theme::TEXT, font));
        });
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_loadout_arrow(parent: &mut ChildSpawnerCommands, field: LoadoutField, direction: i8) {
    parent
        .spawn((
            Button,
            Node {
                width: Val::Px(44.0),
                min_width: Val::Px(44.0),
                height: Val::Px(44.0),
                flex_shrink: 0.0,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::ZERO,
                ..default()
            },
            BackgroundColor(STAGE_CARD),
            BorderColor::all(LAUNCHER_LINE),
            LauncherStyledButton,
            theme::ActionStyle::SECONDARY,
            LoadoutArrow { field, direction },
        ))
        .with_children(|button| {
            crate::plugins::ui_components::spawn_loadout_arrow_icon(button, direction);
        });
}

pub(crate) fn launcher_text(
    value: &str,
    size: f32,
    color: Color,
    font: &Handle<Font>,
) -> impl Bundle {
    let size = launcher_font_size(size, cfg!(target_os = "android"));
    (
        Text::new(value),
        TextFont {
            font: font.clone().into(),
            font_size: size.into(),
            ..default()
        },
        TextColor(color),
    )
}

fn launcher_font_size(size: f32, native_mobile: bool) -> f32 {
    if !native_mobile {
        return size;
    }

    (size * 0.86).max(10.0)
}

fn launcher_touch_target_layout_px(ui_scale: f32) -> f32 {
    MOBILE_MIN_TOUCH_TARGET / ui_scale.max(0.05)
}

fn screen_layout(active: bool) -> (Visibility, Display) {
    if active {
        (Visibility::Visible, Display::Flex)
    } else {
        (Visibility::Hidden, Display::None)
    }
}

fn grid_layout(visible: bool) -> (Visibility, Display) {
    if visible {
        (Visibility::Visible, Display::Flex)
    } else {
        (Visibility::Hidden, Display::None)
    }
}

pub(crate) fn character_name(id: u8) -> &'static str {
    CHARACTER_NAMES.get(id as usize).copied().unwrap_or("侠客")
}

pub(crate) const CHARACTER_NAMES: [&str; 20] = [
    "孟星魂",
    "冷燕",
    "铁胡子",
    "夜猫子",
    "破空",
    "冯浩",
    "孙剑",
    "萧安",
    "子舆",
    "王强",
    "无名",
    "叶翔",
    "小何",
    "凤凰",
    "石群",
    "屠城",
    "范旋",
    "高寄萍",
    "孙玉伯",
    "律香川",
];

fn character_label(id: u8) -> String {
    match CHARACTER_NAMES.get(usize::from(id)) {
        Some(name) => format!("{id} {name}"),
        None => format!("{id}"),
    }
}

fn weapon_label(id: u8) -> String {
    let (item_id, name) = match id {
        0 => (0, "空手(不能攻击)"),
        1 => (1, "飞镖"),
        2 => (2, "飞轮"),
        3 => (3, "火铳"),
        4 => (4, "双刺"),
        5 => (5, "匕首"),
        6 => (6, "剑"),
        7 => (7, "长枪"),
        8 => (8, "大刀"),
        9 => (9, "大锤"),
        10 => (47, "乾坤刀"),
        11 => (51, "指虎"),
        12 => (55, "忍刀"),
        _ => return format!("W{id}"),
    };
    format!("{item_id} {name}")
}

pub fn menu_navigate_buttons(
    query: Query<(&Interaction, &MenuNavigateButton), (Changed<Interaction>, With<Button>)>,
    mut screen: ResMut<MenuScreen>,
    mut capture: ResMut<crate::app_state::KeyCaptureState>,
) {
    for (interaction, target) in &query {
        if *interaction == Interaction::Pressed {
            if target.0 != MenuScreen::Settings {
                capture.clear();
            }
            *screen = target.0;
        }
    }
}

pub fn menu_back_button(
    query: Query<&Interaction, (Changed<Interaction>, With<MenuBackButton>)>,
    mut screen: ResMut<MenuScreen>,
    mut capture: ResMut<crate::app_state::KeyCaptureState>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        capture.clear();
        *screen = MenuScreen::Main;
    }
}

pub(crate) fn stage_tab_buttons(
    official: Query<
        &Interaction,
        (
            Changed<Interaction>,
            With<StageTabOfficialButton>,
            With<Button>,
        ),
    >,
    custom: Query<
        &Interaction,
        (
            Changed<Interaction>,
            With<StageTabCustomButton>,
            With<Button>,
        ),
    >,
    mut tab: ResMut<StageListTab>,
    custom_stages: Res<crate::plugins::stage_plugin::CustomStages>,
    mut request: ResMut<LaunchRequest>,
    mut memory: ResMut<StageSelectionMemory>,
) {
    let official_pressed = official
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed);
    let custom_pressed = custom
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed);
    let next_tab = if custom_pressed {
        Some(StageListTab::Custom)
    } else if official_pressed {
        Some(StageListTab::Official)
    } else {
        None
    };
    let Some(next_tab) = next_tab else {
        return;
    };

    if stage_name_for_tab(*tab, &request.stage_id, &custom_stages).is_some() {
        memory.remember(*tab, &request.stage_id);
    }
    *tab = next_tab;
    select_stage_for_tab(next_tab, &custom_stages, &mut request, &mut memory);
}

fn stage_name_for_tab(
    tab: StageListTab,
    stage_id: &str,
    custom: &crate::plugins::stage_plugin::CustomStages,
) -> Option<String> {
    match tab {
        StageListTab::Official => official_stages()
            .into_iter()
            .find(|stage| stage.id == stage_id && super::campaign_ui::mission(stage_id).is_some())
            .map(|stage| stage.name),
        StageListTab::Custom => custom
            .stages
            .iter()
            .find(|stage| stage.id == stage_id && super::campaign_ui::mission(stage_id).is_some())
            .map(|stage| stage.name.clone()),
    }
    .map(|name| super::stage_presentation::display_name(stage_id, &name))
}

fn first_stage_for_tab(
    tab: StageListTab,
    custom: &crate::plugins::stage_plugin::CustomStages,
) -> Option<(String, String)> {
    match tab {
        StageListTab::Official => official_stages()
            .into_iter()
            .next()
            .map(|stage| (stage.id, stage.name)),
        StageListTab::Custom => custom
            .stages
            .first()
            .map(|stage| (stage.id.clone(), stage.name.clone())),
    }
}

fn select_stage_for_tab(
    tab: StageListTab,
    custom: &crate::plugins::stage_plugin::CustomStages,
    request: &mut LaunchRequest,
    memory: &mut StageSelectionMemory,
) {
    let remembered = memory.selected(tab).map(str::to_owned);
    let selected = remembered
        .as_deref()
        .and_then(|stage_id| {
            stage_name_for_tab(tab, stage_id, custom).map(|name| (stage_id.to_owned(), name))
        })
        .or_else(|| {
            stage_name_for_tab(tab, &request.stage_id, custom)
                .map(|name| (request.stage_id.clone(), name))
        })
        .or_else(|| first_stage_for_tab(tab, custom));

    if let Some((stage_id, stage_name)) = selected {
        request.stage_name = super::stage_presentation::display_name(&stage_id, &stage_name);
        request.stage_id = stage_id;
        memory.remember(tab, &request.stage_id);
    }
}

pub(crate) fn refresh_stage_tab_visuals(
    tab: Res<StageListTab>,
    mut tabs: Query<
        (
            &LauncherStageTab,
            &Interaction,
            &mut BackgroundColor,
            &mut BorderColor,
            &Children,
        ),
        With<Button>,
    >,
    mut labels: Query<&mut TextColor>,
) {
    for (stage_tab, interaction, mut background, mut border, children) in &mut tabs {
        let active = stage_tab.0 == *tab;
        background.0 = if active {
            STAGE_CARD_ACTIVE
        } else if matches!(*interaction, Interaction::Hovered | Interaction::Pressed) {
            NAV_HOVER
        } else {
            STAGE_CARD
        };
        border.set_all(if active { RED_BRIGHT } else { LAUNCHER_LINE });
        for child in children.iter() {
            if let Ok(mut color) = labels.get_mut(child) {
                color.0 = if active { PAPER } else { CYAN };
            }
        }
    }
}

pub(crate) fn sync_launcher_screen_visibility(
    screen: Res<MenuScreen>,
    tab: Res<StageListTab>,
    mut sets: ParamSet<(
        Query<(&LauncherScreenMarker, &mut Node, &mut Visibility)>,
        Query<(&mut Node, &mut Visibility), With<LauncherBrandPanel>>,
        Query<(&mut Node, &mut Visibility), With<OfficialStageGrid>>,
        Query<(&mut Node, &mut Visibility), With<CustomStageGrid>>,
    )>,
) {
    if !screen.is_changed() && !tab.is_changed() {
        return;
    }
    for (marker, mut node, mut visibility) in &mut sets.p0() {
        let (vis, display) = screen_layout(marker.0 == *screen);
        *visibility = vis;
        node.display = display;
    }
    if let Ok((mut node, mut visibility)) = sets.p1().single_mut() {
        let on_main = *screen == MenuScreen::Main;
        let (vis, display) = screen_layout(on_main);
        *visibility = vis;
        node.display = display;
    }
    if let Ok((mut node, mut visibility)) = sets.p2().single_mut() {
        let (vis, display) = grid_layout(*tab == StageListTab::Official);
        *visibility = vis;
        node.display = display;
    }
    if let Ok((mut node, mut visibility)) = sets.p3().single_mut() {
        let (vis, display) = grid_layout(*tab == StageListTab::Custom);
        *visibility = vis;
        node.display = display;
    }
}

pub fn update_stage_selection_labels(
    request: Res<LaunchRequest>,
    tab: Res<StageListTab>,
    custom: Res<crate::plugins::stage_plugin::CustomStages>,
    mut names: Query<&mut Text, With<SelectedStageNameText>>,
    mut codes: Query<&mut Text, (With<SelectedStageCodeText>, Without<SelectedStageNameText>)>,
) {
    if !request.is_changed() && !tab.is_changed() && !custom.is_changed() {
        return;
    }
    let selected_name = stage_name_for_tab(*tab, &request.stage_id, &custom);
    let (name, code) = match (*tab, selected_name) {
        (StageListTab::Official, Some(name)) => (
            name,
            format!("{} · 原版场景资源", request.stage_id.to_uppercase()),
        ),
        (StageListTab::Custom, Some(name)) => (
            name,
            format!("{} · 自制地图包", request.stage_id.to_uppercase()),
        ),
        (StageListTab::Custom, None) => {
            ("暂无自制地图".to_owned(), "请先导入可用的地图包".to_owned())
        }
        (StageListTab::Official, None) => {
            (request.stage_name.clone(), request.stage_id.to_uppercase())
        }
    };
    for mut text in &mut names {
        text.0 = name.clone();
    }
    for mut text in &mut codes {
        text.0 = code.clone();
    }
}

pub fn update_stage_start_availability(
    tab: Res<StageListTab>,
    custom: Res<crate::plugins::stage_plugin::CustomStages>,
    mut buttons: Query<&mut Visibility, With<StartGameButton>>,
) {
    if !tab.is_changed() && !custom.is_changed() {
        return;
    }
    let available = *tab == StageListTab::Official || !custom.stages.is_empty();
    for mut visibility in &mut buttons {
        *visibility = if available {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

pub(crate) fn reset_menu_screen(
    mut screen: ResMut<MenuScreen>,
    mut tab: ResMut<StageListTab>,
    custom: Res<crate::plugins::stage_plugin::CustomStages>,
    mut request: ResMut<LaunchRequest>,
    mut memory: ResMut<StageSelectionMemory>,
) {
    *screen = MenuScreen::Main;
    let current_tab = if custom
        .stages
        .iter()
        .any(|stage| stage.id == request.stage_id)
    {
        StageListTab::Custom
    } else {
        StageListTab::Official
    };
    memory.remember(current_tab, &request.stage_id);
    *tab = StageListTab::Official;
    select_stage_for_tab(StageListTab::Official, &custom, &mut request, &mut memory);
}

pub(crate) fn sync_create_room_outer_scroll(
    screen: Res<MenuScreen>,
    pages: Query<&Node, With<RoomMapPickerPage>>,
    mut outer: Query<
        (&mut Node, &mut ScrollPosition),
        (With<LauncherOuterScrollArea>, Without<RoomMapPickerPage>),
    >,
) {
    let fixed =
        *screen == MenuScreen::Network && pages.iter().any(|page| page.display != Display::None);
    let overflow = if fixed {
        Overflow::clip()
    } else {
        Overflow::scroll_y()
    };
    for (mut node, mut scroll) in &mut outer {
        if node.overflow != overflow {
            node.overflow = overflow;
        }
        if fixed && scroll.0 != Vec2::ZERO {
            scroll.0 = Vec2::ZERO;
        }
    }
}
pub(crate) fn launcher_ui_scroll(
    mut mouse_wheel_events: MessageReader<MouseWheel>,
    windows: Query<&Window>,
    select: Option<Res<crate::plugins::ui_components::UiSelectState>>,
    #[cfg(not(target_arch = "wasm32"))] connection: Option<Res<StockConnectionDialog>>,
    mut areas: Query<
        (
            Entity,
            &Node,
            &ComputedNode,
            &UiGlobalTransform,
            Option<&InheritedVisibility>,
            Option<&CalculatedClip>,
            Option<&ComputedStackIndex>,
            &mut ScrollPosition,
            Option<&crate::plugins::ui_components::UiSelectScrollArea>,
        ),
        With<LauncherScrollArea>,
    >,
) {
    let select_open = select.is_some_and(|select| select.open || select.blocked_this_frame);
    #[cfg(not(target_arch = "wasm32"))]
    if connection.is_some_and(|connection| connection.is_open()) {
        mouse_wheel_events.read().for_each(drop);
        return;
    }
    for event in mouse_wheel_events.read() {
        let dy = match event.unit {
            MouseScrollUnit::Line => event.y * LAUNCHER_SCROLL_LINE,
            MouseScrollUnit::Pixel => event.y,
        };
        if dy.abs() < f32::EPSILON {
            continue;
        }
        let Some(position) = windows.single().ok().and_then(|window| {
            window
                .cursor_position()
                .map(|pos| pos * window.scale_factor())
        }) else {
            continue;
        };
        let mut best: Option<(Entity, u32, f32, f32)> = None;
        for (entity, node, computed, transform, inherited, clip, stack, scroll, select_area) in
            &mut areas
        {
            if node.overflow.y != OverflowAxis::Scroll || (select_open && select_area.is_none()) {
                continue;
            }
            let extent =
                (computed.content_size().y - computed.size().y) * computed.inverse_scale_factor;
            let next = (scroll.y - dy).clamp(0.0, extent.max(0.0));
            if (next - scroll.y).abs() < 0.01 {
                continue;
            }
            if ui_node_contains(node, computed, transform, inherited, clip, position) {
                let size = computed.size();
                let area = size.x * size.y;
                let stack = stack.map_or(0, |s| s.0);
                if best.is_none_or(|(_, old_stack, old_area, _)| {
                    stack > old_stack || (stack == old_stack && area < old_area)
                }) {
                    best = Some((entity, stack, area, next));
                }
            }
        }
        if let Some((entity, _, _, next)) = best {
            if let Ok((_, _, _, _, _, _, _, mut scroll, _)) = areas.get_mut(entity) {
                scroll.y = next;
            }
        }
    }
}

fn ui_node_contains(
    node: &Node,
    computed: &ComputedNode,
    transform: &UiGlobalTransform,
    inherited_visibility: Option<&InheritedVisibility>,
    clip: Option<&CalculatedClip>,
    physical_position: Vec2,
) -> bool {
    node.display != Display::None
        && inherited_visibility.is_none_or(|visibility| visibility.get())
        && clip.is_none_or(|clip| clip.clip.contains(physical_position))
        && computed.contains_point(*transform, physical_position)
}

pub(crate) fn launcher_touch_scroll(
    mut events: MessageReader<TouchInput>,
    mut state: ResMut<LauncherTouchScrollState>,
    editor: Option<Res<MobileLayoutEditorState>>,
    slider: Option<Res<crate::plugins::ui_components::slider::UiSliderDrag>>,
    windows: Query<&Window>,
    blockers: Query<
        (
            &Node,
            &ComputedNode,
            &UiGlobalTransform,
            Option<&InheritedVisibility>,
            Option<&CalculatedClip>,
            Option<&ComputedStackIndex>,
        ),
        With<UiInputBlocker>,
    >,
    mut areas: Query<
        (
            Entity,
            &Node,
            &ComputedNode,
            &UiGlobalTransform,
            Option<&InheritedVisibility>,
            Option<&CalculatedClip>,
            Option<&ComputedStackIndex>,
            &mut ScrollPosition,
        ),
        With<LauncherScrollArea>,
    >,
    mut buttons: Query<
        (
            Entity,
            &Node,
            &ComputedNode,
            &UiGlobalTransform,
            Option<&InheritedVisibility>,
            Option<&CalculatedClip>,
            Option<&ComputedStackIndex>,
            &mut Interaction,
        ),
        (
            With<Button>,
            Or<(
                With<LauncherStyledButton>,
                With<crate::plugins::ui_plugin::ResponsiveMenuButton>,
                With<crate::plugins::pst_ui::PstPanelButton>,
            )>,
        ),
    >,
) {
    if editor.is_some_and(|editor| editor.is_open()) {
        events.read().for_each(drop);
        *state = LauncherTouchScrollState::default();
        return;
    }
    for event in events.read() {
        if slider
            .as_ref()
            .is_some_and(|slider| slider.consumes_touch(event.id))
        {
            continue;
        }
        match event.phase {
            TouchPhase::Started if state.pointer.is_none() => {
                let Ok(window) = windows.get(event.window) else {
                    continue;
                };

                let physical_position = event.position * window.scale_factor();
                let modal_floor = blockers
                    .iter()
                    .filter_map(|(node, computed, transform, inherited, clip, stack)| {
                        ui_node_contains(
                            node,
                            computed,
                            transform,
                            inherited,
                            clip,
                            physical_position,
                        )
                        .then_some(stack.map_or(0, |stack| stack.0))
                    })
                    .max();
                let mut best: Option<(Entity, u32, f32)> = None;
                for (entity, node, computed, transform, inherited, clip, stack, _) in &mut areas {
                    if node.overflow.y != OverflowAxis::Scroll
                        || !ui_node_contains(
                            node,
                            computed,
                            transform,
                            inherited,
                            clip,
                            physical_position,
                        )
                    {
                        continue;
                    }
                    let size = computed.size();
                    let area = size.x * size.y;
                    let stack = stack.map_or(0, |stack| stack.0);
                    if modal_floor.is_some_and(|floor| stack < floor) {
                        continue;
                    }
                    if best.is_none_or(|(_, best_stack, best_area)| {
                        stack > best_stack || (stack == best_stack && area < best_area)
                    }) {
                        best = Some((entity, stack, area));
                    }
                }
                state.pointer = Some(event.id);
                state.target = best.map(|(entity, _, _)| entity);
                state.button = buttons
                    .iter_mut()
                    .filter_map(
                        |(entity, node, computed, transform, inherited, clip, stack, _)| {
                            ui_node_contains(
                                node,
                                computed,
                                transform,
                                inherited,
                                clip,
                                physical_position,
                            )
                            .then(|| {
                                let stack = stack.map_or(0, |stack| stack.0);
                                (entity, stack, computed.size().x * computed.size().y)
                            })
                            .filter(|(_, stack, _)| modal_floor.is_none_or(|floor| *stack >= floor))
                        },
                    )
                    .max_by(|(_, left_stack, left_area), (_, right_stack, right_area)| {
                        left_stack
                            .cmp(right_stack)
                            .then_with(|| right_area.total_cmp(left_area))
                    })
                    .map(|(entity, _, _)| entity);
                state.start = event.position;
                state.previous = event.position;
                state.max_distance = 0.0;
                state.dragging = false;
            }
            TouchPhase::Moved if state.pointer == Some(event.id) => {
                state.max_distance = state.max_distance.max(event.position.distance(state.start));
                if !state.dragging && state.max_distance >= TOUCH_SCROLL_THRESHOLD {
                    state.dragging = true;
                }
                if state.dragging {
                    if let Some(target) = state.target {
                        if let Ok((_, _, _, _, _, _, _, mut scroll)) = areas.get_mut(target) {
                            scroll.0.y =
                                (scroll.0.y + state.previous.y - event.position.y).max(0.0);
                        }
                    }
                }
                state.previous = event.position;
            }
            TouchPhase::Ended | TouchPhase::Canceled if state.pointer == Some(event.id) => {
                state.max_distance = state.max_distance.max(event.position.distance(state.start));
                let dragging = state.dragging || state.max_distance >= TOUCH_SCROLL_THRESHOLD;
                if dragging {
                    if let Some(target) = state.target {
                        if let Ok((_, _, _, _, _, _, _, mut scroll)) = areas.get_mut(target) {
                            scroll.0.y =
                                (scroll.0.y + state.previous.y - event.position.y).max(0.0);
                        }
                    }
                }
                let physical_position = windows
                    .get(event.window)
                    .ok()
                    .map(|window| event.position * window.scale_factor());
                let tap_button = (event.phase == TouchPhase::Ended && !dragging)
                    .then_some(state.button)
                    .flatten()
                    .filter(|entity| {
                        physical_position.is_some_and(|physical_position| {
                            buttons.get_mut(*entity).is_ok_and(
                                |(_, node, computed, transform, inherited, clip, _, _)| {
                                    ui_node_contains(
                                        node,
                                        computed,
                                        transform,
                                        inherited,
                                        clip,
                                        physical_position,
                                    )
                                },
                            )
                        })
                    });
                *state = LauncherTouchScrollState::default();
                if let Some(entity) = tap_button {
                    if let Ok((_, _, _, _, _, _, _, mut interaction)) = buttons.get_mut(entity) {
                        *interaction = Interaction::Pressed;
                    }
                }
            }
            _ => {}
        }
    }

    if state.pointer.is_some() {
        for (_, _, _, _, _, _, _, mut interaction) in &mut buttons {
            if *interaction == Interaction::Pressed {
                *interaction = Interaction::None;
            }
        }
    }
}

pub(crate) fn adapt_launcher_layout(
    window: Query<&Window>,
    screen: Res<MenuScreen>,
    safe_area: Option<Res<SafeAreaInsets>>,
    ui_scale: Option<Res<EffectiveUiScale>>,
    mut sets: ParamSet<(
        Query<
            (
                &mut Node,
                Option<&LauncherBrandPanel>,
                Option<&LauncherMainCopy>,
                Option<&LauncherStageLayout>,
                Option<&LauncherStageAside>,
                Option<&LauncherMobileStartBar>,
                Option<&LauncherStageAsideStartButton>,
                Option<&LauncherShell>,
                Option<&LauncherHeader>,
                Option<&LauncherPageHeading>,
                Option<&LauncherMobileHero>,
                Option<&NetworkBrowserHeading>,
            ),
            Or<(
                With<LauncherBrandPanel>,
                With<LauncherMainCopy>,
                With<LauncherStageLayout>,
                With<LauncherStageAside>,
                With<LauncherMobileStartBar>,
                With<LauncherStageAsideStartButton>,
                With<LauncherShell>,
                With<LauncherHeader>,
                With<LauncherPageHeading>,
                With<LauncherMobileHero>,
            )>,
        >,
        Query<(&mut Node, Option<&LauncherNetworkPrimaryButton>), With<LauncherNavButton>>,
        Query<&mut Node, With<LauncherNavIcon>>,
        Query<&mut Node, With<LauncherNavLabels>>,
        Query<&mut Node, With<StageButton>>,
        Query<&mut Node, With<LauncherStageTab>>,
        Query<
            (
                &mut Node,
                Option<&LauncherNetworkPrimaryActions>,
                Option<&LauncherNetworkSection>,
                Option<&LauncherNetworkRoomList>,
                Option<&LauncherMobileStartButton>,
                Option<&LauncherNetworkRoomButton>,
                Option<&LauncherNetworkBrowserLayout>,
                Option<&LauncherNetworkActionRail>,
                Option<&LauncherNetworkRoomPanel>,
            ),
            Or<(
                With<LauncherNetworkPrimaryActions>,
                With<LauncherNetworkSection>,
                With<LauncherNetworkRoomList>,
                With<LauncherMobileStartButton>,
                With<LauncherNetworkRoomButton>,
                With<LauncherNetworkBrowserLayout>,
                With<LauncherNetworkActionRail>,
                With<LauncherNetworkRoomPanel>,
            )>,
        >,
        Query<(&mut Node, &LauncherCreateLayout)>,
    )>,
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

    let unit = ui_scale.as_ref().map_or(1.0, |scale| scale.0.max(0.05));
    let viewport_w = window.width() / unit;
    let viewport_h = window.height() / unit;
    let full_bleed_screen = matches!(
        *screen,
        MenuScreen::Main | MenuScreen::Stages | MenuScreen::Network
    );
    let shell_w = if full_bleed_screen {
        viewport_w
    } else {
        viewport_w.min(1360.0)
    };
    let content_width = (shell_w - layout.left_gutter - layout.right_gutter).max(320.0);
    let stacked_create = content_width < 1000.0;
    let settings_width = (content_width * 0.34).clamp(400.0, 560.0);
    let map_width = if stacked_create {
        content_width
    } else {
        (content_width - settings_width - 14.0).max(400.0)
    };
    for (mut node, part) in &mut sets.p7() {
        match part {
            LauncherCreateLayout::Page => {
                node.min_height = Val::Px(0.0);
                node.flex_grow = 1.0;
                node.flex_basis = Val::Px(0.0);
                node.overflow = Overflow::clip();
                node.row_gap = Val::Px(10.0);
            }
            LauncherCreateLayout::Body => {
                node.flex_direction = if stacked_create {
                    FlexDirection::Column
                } else {
                    FlexDirection::Row
                };
            }
            LauncherCreateLayout::Maps => {
                node.width = if stacked_create {
                    Val::Percent(100.0)
                } else {
                    Val::Px(map_width)
                };
                node.flex_shrink = 0.0;
            }
            LauncherCreateLayout::Settings => {
                node.width = if stacked_create {
                    Val::Percent(100.0)
                } else {
                    Val::Px(settings_width)
                };
            }
            LauncherCreateLayout::MapPreview => {
                let budget = (viewport_h - 240.0).max(244.0);
                node.height = Val::Px((budget * 0.44).clamp(140.0, 300.0));
            }
            LauncherCreateLayout::MapList => {
                let budget = (viewport_h - 240.0).max(244.0);
                node.max_height = Val::Px((budget * 0.52).clamp(160.0, 440.0));
            }
            LauncherCreateLayout::MapChoice => {
                node.width = if layout.mobile {
                    Val::Percent(48.5)
                } else if map_width >= 1040.0 {
                    Val::Percent(23.5)
                } else {
                    Val::Percent(32.0)
                };
                node.min_width = Val::Px(120.0);
                node.max_width = Val::Auto;
                node.height = Val::Px(if layout.mobile { 58.0 / unit } else { 72.0 });
                node.min_height = node.height;
                node.border = UiRect::all(Val::Px(1.5));
            }
            LauncherCreateLayout::Brand => {
                node.width = Val::Px(if *screen == MenuScreen::Network {
                    128.0
                } else {
                    240.0
                });
            }
        }
    }

    for (
        mut node,
        brand,
        copy,
        stage,
        aside,
        mobile_start,
        aside_start,
        shell,
        header,
        heading,
        mobile_hero,
        network_heading,
    ) in &mut sets.p0()
    {
        if brand.is_some() {
            node.display = if layout.mobile || *screen != MenuScreen::Main {
                Display::None
            } else {
                Display::Flex
            };
            node.width = if layout.narrow {
                Val::Px(180.0)
            } else {
                Val::Px(280.0)
            };
            node.padding = UiRect::all(Val::Px(if layout.narrow { 28.0 } else { 48.0 }));
        }
        if copy.is_some() {
            node.display = if layout.narrow {
                Display::None
            } else {
                Display::Flex
            };
        }
        if stage.is_some() {
            node.flex_direction = if layout.stacked_stage {
                FlexDirection::Column
            } else {
                FlexDirection::Row
            };
            node.column_gap = Val::Px(if layout.mobile { 10.0 } else { 16.0 });
            node.row_gap = Val::Px(if layout.mobile { 10.0 } else { 16.0 });
        }
        if aside.is_some() {
            node.width = if layout.stacked_stage {
                Val::Percent(100.0)
            } else if layout.mobile {
                Val::Px(210.0)
            } else {
                Val::Px(280.0)
            };
            node.min_width = if layout.stacked_stage {
                Val::Px(0.0)
            } else {
                Val::Px(196.0)
            };
            node.padding = UiRect::all(Val::Px(if layout.mobile { 12.0 } else { 26.0 }));
        }
        if mobile_start.is_some() {
            node.display = if layout.mobile {
                Display::Flex
            } else {
                Display::None
            };
            node.min_height = Val::Px(if layout.mobile { 52.0 } else { 60.0 });
            node.height = Val::Px(if layout.mobile { 52.0 } else { 60.0 });
        }
        if aside_start.is_some() {
            node.display = if layout.mobile {
                Display::None
            } else {
                Display::Flex
            };
        }
        if shell.is_some() {
            node.max_width = if full_bleed_screen {
                Val::Auto
            } else {
                Val::Px(1360.0)
            };
            node.padding = UiRect::new(
                Val::Px(layout.left_gutter),
                Val::Px(layout.right_gutter),
                Val::Px(if *screen == MenuScreen::Network {
                    layout
                        .top_gutter
                        .min(20.0)
                        .max(safe_area.as_deref().map_or(0.0, |safe| safe.top))
                } else {
                    layout.top_gutter
                }),
                Val::Px(layout.bottom_gutter),
            );
        }
        if header.is_some() {
            node.display = if layout.mobile
                || matches!(
                    *screen,
                    MenuScreen::Main | MenuScreen::Stages | MenuScreen::Settings
                ) {
                Display::None
            } else {
                Display::Flex
            };
            node.padding.bottom = Val::Px(if *screen == MenuScreen::Network {
                8.0
            } else {
                18.0
            });
        }
        if heading.is_some() {
            node.height = Val::Px(if layout.mobile { 46.0 } else { 72.0 });
            node.column_gap = Val::Px(if layout.mobile { 10.0 } else { 24.0 });
            if network_heading.is_some() {
                node.min_height = node.height;
                node.height = Val::Auto;
            }
        }
        if mobile_hero.is_some() {
            node.display = if layout.narrow {
                Display::Flex
            } else {
                Display::None
            };
        }
    }

    let horizontal_primary_actions = layout.mobile && !layout.stacked_stage;
    for (mut node, primary) in &mut sets.p1() {
        node.height = Val::Px(if layout.mobile { 48.0 } else { 44.0 });
        node.min_height = node.height;
        node.max_height = node.height;
        node.flex_shrink = 0.0;
        node.column_gap = Val::Px(if layout.mobile { 10.0 } else { 16.0 });
        node.padding = UiRect::new(
            Val::Px(if layout.mobile { 8.0 } else { 10.0 }),
            Val::Px(if layout.mobile { 12.0 } else { 20.0 }),
            Val::Px(0.0),
            Val::Px(0.0),
        );
        if primary.is_some() && horizontal_primary_actions {
            node.width = Val::Percent(49.0);
            node.flex_grow = 1.0;
        } else {
            node.width = Val::Percent(100.0);
            node.flex_grow = 0.0;
        }
    }
    for mut node in &mut sets.p2() {
        let size = if layout.mobile { 34.0 } else { 42.0 };
        node.width = Val::Px(size);
        node.height = Val::Px(size);
    }
    for mut node in &mut sets.p3() {
        node.row_gap = Val::Px(if layout.mobile { 1.0 } else { 4.0 });
    }
    for mut node in &mut sets.p4() {
        node.width = if layout.mobile && layout.stacked_stage {
            Val::Percent(48.0)
        } else {
            Val::Percent(31.0)
        };
        node.min_height = Val::Px(if layout.mobile { 70.0 } else { 92.0 });
        node.padding = UiRect::all(Val::Px(if layout.mobile { 8.0 } else { 12.0 }));
    }
    for mut node in &mut sets.p5() {
        node.min_width = Val::Px(if layout.mobile { 104.0 } else { 138.0 });
        node.height = Val::Px(if layout.mobile { 44.0 } else { 38.0 });
        node.flex_grow = if layout.mobile { 1.0 } else { 0.0 };
    }
    for (
        mut node,
        primary_actions,
        section,
        room_list,
        mobile_start,
        room_button,
        browser_layout,
        action_rail,
        room_panel,
    ) in &mut sets.p6()
    {
        if primary_actions.is_some() {
            node.flex_direction = if horizontal_primary_actions {
                FlexDirection::Row
            } else {
                FlexDirection::Column
            };
        }
        if section.is_some() {
            node.padding = UiRect::all(Val::Px(if layout.mobile { 8.0 } else { 10.0 }));
            node.row_gap = Val::Px(if layout.mobile { 6.0 } else { 8.0 });
        }
        if room_list.is_some() {
            node.row_gap = Val::Px(if layout.mobile { 4.0 } else { 8.0 });
        }
        if mobile_start.is_some() {
            node.width = Val::Px(if layout.stacked_stage {
                128.0
            } else if layout.mobile {
                142.0
            } else {
                180.0
            });
            node.min_height = Val::Px(if layout.mobile { 44.0 } else { 48.0 });
        }
        if room_button.is_some() {
            node.min_height = Val::Px(if layout.mobile { 62.0 } else { 68.0 });
            node.padding = UiRect::horizontal(Val::Px(if layout.mobile { 10.0 } else { 16.0 }));
        }
        if browser_layout.is_some() {
            node.flex_direction = if layout.stacked_stage {
                FlexDirection::Column
            } else {
                FlexDirection::Row
            };
        }
        if action_rail.is_some() {
            node.width = if layout.stacked_stage {
                Val::Percent(100.0)
            } else {
                Val::Px(if layout.mobile { 124.0 } else { 148.0 })
            };
            node.min_width = if layout.stacked_stage {
                Val::Px(0.0)
            } else {
                Val::Px(120.0)
            };
            node.flex_direction = if layout.stacked_stage {
                FlexDirection::Row
            } else {
                FlexDirection::Column
            };
            node.flex_wrap = if layout.stacked_stage {
                FlexWrap::Wrap
            } else {
                FlexWrap::NoWrap
            };
        }
        if room_panel.is_some() {
            node.min_height = Val::Px(if layout.mobile { 160.0 } else { 220.0 });
        }
    }
}

pub(crate) fn adapt_launcher_touch_targets(
    window: Query<&Window>,
    ui_scale: Res<EffectiveUiScale>,
    mut buttons: Query<
        &mut Node,
        (
            With<LauncherStyledButton>,
            Without<super::debug_plugin::DebugPanelToggle>,
        ),
    >,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let layout = responsive_ui_layout(window.width(), window.height(), cfg!(target_os = "android"));
    if !layout.mobile || !cfg!(target_os = "android") {
        return;
    }
    let target = launcher_touch_target_layout_px(ui_scale.0);
    for mut node in &mut buttons {
        if !matches!(node.min_height, Val::Px(value) if value >= target) {
            node.min_height = Val::Px(target);
        }
        if matches!(node.width, Val::Px(value) if value < target) {
            node.width = Val::Px(target);
            node.min_width = Val::Px(target);
        }
        if matches!(node.height, Val::Px(value) if value < target) {
            node.height = Val::Px(target);
        }
    }
}

#[cfg(any(target_os = "windows", test))]
fn font_priority(name: &str) -> u32 {
    let lower = name.to_lowercase();
    const PREFERRED: [&str; 8] = [
        "msyh", "simsun", "simhei", "simkai", "deng", "stzhongs", "msjh", "mingliu",
    ];
    PREFERRED
        .iter()
        .position(|preferred| lower.contains(preferred))
        .map(|index| index as u32)
        .unwrap_or(100)
}

#[cfg(target_os = "windows")]
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
#[cfg(test)]
fn system_font_candidates() -> Vec<PathBuf> {
    let windows = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let fonts = windows.join("Fonts");
    let mut candidates = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&fonts) {
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| {
                let lower = name.to_lowercase();
                lower.ends_with(".ttf") || lower.ends_with(".ttc")
            })
            .collect();
        names.sort_by_key(|name| font_priority(name));
        candidates.extend(names.into_iter().map(|name| fonts.join(name)));
    }
    candidates.extend(
        [
            "msyh.ttc",
            "msyh.ttf",
            "simsun.ttc",
            "simsun.ttf",
            "Deng.ttf",
        ]
        .into_iter()
        .map(|name| fonts.join(name)),
    );
    candidates
}

#[cfg(all(
    not(target_os = "windows"),
    not(any(target_arch = "wasm32", target_os = "android"))
))]
#[cfg(test)]
fn system_font_candidates() -> Vec<PathBuf> {
    [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
#[cfg(test)]
fn ui_font_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let rel = PathBuf::from("assets/fonts/ui.ttf");

    candidates.push(rel.clone());
    candidates.push(PathBuf::from("meteor_rust").join(&rel));

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest.join("../../assets/fonts/ui.ttf"));
    candidates.push(manifest.join("../../../meteor_rust/assets/fonts/ui.ttf"));
    candidates.push(manifest.join("assets/fonts/ui.ttf"));

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(&rel));
            candidates.push(dir.join("fonts/ui.ttf"));
        }
    }

    candidates.extend(system_font_candidates());
    candidates
}

#[cfg(test)]
mod font_tests {
    use super::*;
    use bevy::window::WindowResolution;

    #[test]
    fn room_catalog_requires_stage_entries_and_accepts_future_ids() {
        let manifest = serde_json::json!({"packs": [
            {"packId": "stage/sn23", "entries": ["stage/sn23.gmb", "stage/sn23.des", "stage/sn23.cob", "stage/sn23.pst"]},
            {"packId": "stage/sn24", "entries": ["stage/sn24.gmb", "stage/sn24.des"]}
        ]});
        assert!(stage_has_required_entries(&manifest, "sn23"));
        assert!(!stage_has_required_entries(&manifest, "sn24"));
        assert!(!stage_has_required_entries(&manifest, "sn01"));
    }

    #[test]
    fn main_and_secondary_navigation_only_highlight_the_pointer_target() {
        let mut app = App::new();
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Node::default()).with_children(|parent| {
                let font = Handle::default();
                spawn_main_screen(parent, &font, true);
                spawn_replay_screen(parent, &font, true);
            });
        })
        .add_systems(Update, hero_nav_feedback);
        app.update();
        let buttons: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<HeroNavButton>>()
            .iter(app.world())
            .collect();
        assert!(
            !buttons.is_empty(),
            "both menu levels must expose navigation controls"
        );
        for target in &buttons {
            for state in [
                Interaction::None,
                Interaction::Hovered,
                Interaction::Pressed,
                Interaction::None,
            ] {
                *app.world_mut().get_mut::<Interaction>(*target).unwrap() = state;
                app.update();
                for button in &buttons {
                    let active = button == target && state != Interaction::None;
                    assert_eq!(
                        app.world()
                            .get::<BackgroundGradient>(*button)
                            .unwrap()
                            .0
                            .is_empty(),
                        !active
                    );
                    for child in app.world().get::<Children>(*button).unwrap().iter() {
                        if app.world().get::<HeroNavAccent>(child).is_some() {
                            assert_eq!(
                                app.world().get::<BackgroundColor>(child).unwrap().0,
                                if active {
                                    theme::CRIMSON_BRIGHT
                                } else {
                                    Color::NONE
                                }
                            );
                        }
                        if app.world().get::<Text>(child).is_some() {
                            assert_eq!(app.world().get::<TextColor>(child).unwrap().0, theme::TEXT);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn font_priority_orders_chinese_fonts() {
        assert!(font_priority("msyh.ttc") < font_priority("simsun.ttc"));
        assert!(font_priority("simsun.ttc") < font_priority("arial.ttf"));
        assert!(font_priority("Deng.ttf") < font_priority("arial.ttf"));

        assert_eq!(font_priority("arial.ttf"), font_priority("times.ttf"));
        assert_eq!(font_priority(""), 100);
    }

    #[test]
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    fn repository_ui_font_candidate_is_present_and_parseable() {
        let candidate = ui_font_candidates()
            .into_iter()
            .find(|path| path.is_file())
            .expect("repository or system UI font candidate");
        let bytes = std::fs::read(&candidate).expect("read UI font candidate");
        assert!(bytes.len() > 1_000_000, "CJK font unexpectedly small");
        let _ = Font::from_bytes(bytes);
    }

    #[test]
    fn android_short_landscape_keeps_stage_sidebar_visible() {
        let layout = responsive_ui_layout(872.0, 393.0, true);
        assert!(layout.mobile);
        assert!(layout.narrow);
        assert!(!layout.stacked_stage);
        assert_eq!(layout.left_gutter, 16.0);
        assert_eq!(layout.right_gutter, 16.0);
    }

    fn spawn_launcher_for_structure_test(
        commands: Commands,
        request: Res<LaunchRequest>,
        settings: Res<Settings>,
        keymap: Res<Keymap>,
        custom: Res<crate::plugins::stage_plugin::CustomStages>,
        screen: Res<MenuScreen>,
        tab: Res<StageListTab>,
    ) {
        setup_launcher(
            commands,
            Handle::default(),
            request,
            settings,
            keymap,
            custom,
            screen,
            tab,
        );
    }

    #[test]
    fn network_create_opens_shared_form_instead_of_submitting_defaults() {
        let mut app = App::new();
        app.insert_resource(LaunchRequest {
            public_battle: true,
            ..Default::default()
        })
        .insert_resource(Settings::default())
        .insert_resource(Keymap::default())
        .insert_resource(crate::plugins::stage_plugin::CustomStages::default())
        .insert_resource(MenuScreen::Network)
        .insert_resource(StageListTab::Official)
        .add_systems(Update, spawn_launcher_for_structure_test);
        app.update();
        let world = app.world_mut();
        assert_eq!(
            world
                .query::<&crate::app_state::MenuNetButton>()
                .iter(world)
                .count(),
            0,
            "browser-list Create must navigate to the form, never send a request"
        );
        assert_eq!(world.query::<&MenuStockHostButton>().iter(world).count(), 1);
        assert_eq!(world.query::<&MenuStockRuleButton>().iter(world).count(), 5);
        assert!(world.query::<&MenuStockStageButton>().iter(world).count() >= 22);
        assert_eq!(
            world
                .query::<&MenuStockItemRuleButton>()
                .iter(world)
                .count(),
            4
        );
        assert_eq!(world.query::<&MenuStockSelectText>().iter(world).count(), 3);
        assert_eq!(
            world.query::<&MenuStockSelectOption>().iter(world).count(),
            18
        );
        let advanced = world
            .query_filtered::<(Entity, &Node), With<RoomAdvancedDialog>>()
            .single(world)
            .unwrap();
        assert_eq!(advanced.1.display, Display::None);
        let advanced = advanced.0;
        let settings: Vec<_> = world
            .query_filtered::<Entity, Or<(
                With<MenuStockSelectOption>,
                With<MenuStockItemRuleButton>,
                With<crate::app_state::MenuBalanceRulesButton>,
                With<crate::app_state::MenuPublicBattleButton>,
            )>>()
            .iter(world)
            .collect();
        for mut entity in settings {
            while entity != advanced {
                entity = world
                    .get::<ChildOf>(entity)
                    .expect("advanced setting belongs to the modal")
                    .parent();
            }
        }
        let pages: Vec<_> = world.query::<(&UiPageRoot, &Node)>().iter(world).collect();
        assert!(pages.iter().any(|(_, node)| node.display == Display::None));
        let public_switches = world
            .query::<&crate::app_state::MenuPublicBattleButton>()
            .iter(world)
            .count();
        let expected = cfg!(not(target_arch = "wasm32"))
            && !super::super::network_plugin::default_web_address().is_empty();
        assert_eq!(public_switches, usize::from(expected));
        #[cfg(not(target_arch = "wasm32"))]
        {
            use crate::plugins::ui_components::slider::{UiSlider, UiSliderChanged};
            use bevy::ecs::system::RunSystemOnce;
            let (slider, bounds) = world
                .query_filtered::<(Entity, &UiSlider), With<RoomPhysicsSlider>>()
                .single(world)
                .unwrap();
            assert_eq!(
                (bounds.min, bounds.max, bounds.step, bounds.value),
                (30.0, 200.0, 1.0, 120.0)
            );
            let section = world.get::<ChildOf>(slider).unwrap().parent();
            assert_eq!(world.get::<Node>(section).unwrap().display, Display::None);
            app.add_message::<UiSliderChanged>();
            let world = app.world_mut();
            world.resource_mut::<LaunchRequest>().public_battle = false;
            world.write_message(UiSliderChanged {
                entity: slider,
                value: 173.0,
                finished: true,
            });
            world.run_system_once(room_physics_slider_changes).unwrap();
            assert_eq!(world.resource::<LaunchRequest>().stock_physics_hz, 173);
            assert_eq!(world.get::<Node>(section).unwrap().display, Display::Flex);
            assert_eq!(
                world
                    .query_filtered::<&Text, With<RoomPhysicsLabel>>()
                    .single(world)
                    .unwrap()
                    .0,
                "173 Hz"
            );
        }
    }

    #[test]
    fn campaign_tree_has_nine_pins_one_start_and_no_disabled_entries_or_loadout() {
        let mut app = App::new();
        app.insert_resource(LaunchRequest::default())
            .insert_resource(Settings::default())
            .insert_resource(Keymap::default())
            .insert_resource(crate::plugins::stage_plugin::CustomStages::default())
            .insert_resource(MenuScreen::Stages)
            .insert_resource(StageListTab::Official)
            .add_systems(Update, spawn_launcher_for_structure_test);
        app.update();

        let bars = app
            .world_mut()
            .query_filtered::<Entity, With<super::super::campaign_ui::CampaignPin>>()
            .iter(app.world())
            .count();
        let buttons = app
            .world_mut()
            .query_filtered::<Entity, With<StartGameButton>>()
            .iter(app.world())
            .count();
        assert_eq!(bars, 9, "only authored missions appear on the map");
        assert_eq!(
            buttons, 1,
            "one start action serves both desktop and compact layouts"
        );
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<StageTabCustomButton>>()
                .iter(app.world())
                .count(),
            0
        );
        assert!(app
            .world_mut()
            .query::<&MenuNavigateButton>()
            .iter(app.world())
            .any(|b| b.0 == MenuScreen::Replay));
        assert_eq!(
            app.world_mut()
                .query::<&MenuMrcButton>()
                .iter(app.world())
                .count(),
            1
        );
        assert_eq!(
            app.world_mut()
                .query::<&crate::app_state::MenuRecordButton>()
                .iter(app.world())
                .count(),
            0
        );
        assert_eq!(
            app.world_mut()
                .query::<&crate::app_state::MenuAutoRecordButton>()
                .iter(app.world())
                .count(),
            0
        );
        assert_eq!(
            app.world_mut()
                .query::<&crate::app_state::MenuReplayButton>()
                .iter(app.world())
                .count(),
            0
        );
        let arrows: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<LoadoutArrow>>()
            .iter(app.world())
            .collect();
        for mut entity in arrows {
            while let Some(parent) = app.world().get::<ChildOf>(entity) {
                entity = parent.parent();
                if let Some(screen) = app.world().get::<LauncherScreenMarker>(entity) {
                    assert_ne!(
                        screen.0,
                        MenuScreen::Stages,
                        "solo has no character/weapon selectors"
                    );
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn lan_player_loadout_is_only_inside_the_hidden_entry_dialog() {
        let mut app = App::new();
        app.insert_resource(LaunchRequest::default())
            .insert_resource(Settings::default())
            .insert_resource(Keymap::default())
            .insert_resource(crate::plugins::stage_plugin::CustomStages::default())
            .insert_resource(MenuScreen::Network)
            .insert_resource(StageListTab::Official)
            .add_systems(Update, spawn_launcher_for_structure_test);
        app.update();

        let mut dialogs = app.world_mut().query::<(&UiModalRoot, &Node, &ChildOf)>();
        assert_eq!(dialogs.iter(app.world()).count(), 4);
        let mut selects = app
            .world_mut()
            .query_filtered::<(&Node, &ChildOf), With<crate::plugins::ui_components::UiSelectMenu>>(
            );
        assert!(
            selects.iter(app.world()).next().is_some(),
            "the launcher must expose loadout selectors"
        );
        for (node, parent) in selects.iter(app.world()) {
            assert_eq!(node.position_type, PositionType::Absolute);
            assert_eq!(node.display, Display::None);
            assert!(
                app.world().get::<MenuRoot>(parent.parent()).is_some(),
                "select dialogs must not be children of a form field"
            );
        }
        for (_, node, parent) in dialogs.iter(app.world()) {
            assert_eq!(node.display, Display::None);
            assert_eq!(node.position_type, PositionType::Absolute);
            assert_eq!(node.justify_content, JustifyContent::Center);
            assert_eq!(node.align_items, AlignItems::Center);
            let root = app.world().entity(parent.parent());
            assert!(
                root.contains::<MenuRoot>(),
                "modals must bypass all scrolling ancestors"
            );
            let root_node = root.get::<Node>().unwrap();
            assert_eq!(root_node.width, Val::Percent(100.0));
            assert_eq!(root_node.height, Val::Percent(100.0));
        }
        let connection = app.world_mut()
            .query_filtered::<(&UiInputBlocker, &ZIndex, &bevy::ui::FocusPolicy), With<StockConnectionDialogRoot>>()
            .single(app.world()).unwrap();
        assert_eq!(*connection.1, ZIndex(320));
        assert_eq!(*connection.2, bevy::ui::FocusPolicy::Block);
        assert_eq!(
            app.world_mut()
                .query::<&StockConnectionAction>()
                .iter(app.world())
                .count(),
            2
        );
        let mut choices = app.world_mut().query::<(Entity, &LoadoutArrow)>();
        let mut characters = Vec::new();
        let mut weapons = Vec::new();
        for (entity, arrow) in choices.iter(app.world()) {
            let mut ancestor = entity;
            while let Some(parent) = app.world().get::<ChildOf>(ancestor) {
                ancestor = parent.parent();
                assert!(!app
                    .world()
                    .get::<LauncherScreenMarker>(ancestor)
                    .is_some_and(|marker| marker.0 == MenuScreen::Network));
                if app
                    .world()
                    .get::<MenuStockLoadoutDialog>(ancestor)
                    .is_some()
                {
                    match arrow.field {
                        LoadoutField::Character => characters.push(arrow.direction),
                        LoadoutField::Weapon => weapons.push(arrow.direction),
                    }
                    break;
                }
            }
        }
        characters.sort_unstable();
        weapons.sort_unstable();
        assert_eq!(characters, [-1, 1]);
        assert_eq!(weapons, [-1, 1]);
        let world = app.world_mut();
        let mut viewports = world.query_filtered::<Entity,
            With<super::super::character_plugin::preview::CharacterPreviewViewport>>();
        let mut dialog_previews = 0;
        for preview in viewports.iter(world) {
            let ancestors = std::iter::successors(Some(preview), |entity| {
                world.get::<ChildOf>(*entity).map(ChildOf::parent)
            });
            for entity in ancestors {
                assert!(!matches!(
                    world.get::<LauncherCreateLayout>(entity),
                    Some(LauncherCreateLayout::Page)
                ));
                if world.get::<MenuStockLoadoutDialog>(entity).is_some() {
                    dialog_previews += 1;
                }
            }
        }
        assert_eq!(dialog_previews, 1);
    }

    #[test]
    fn loadout_arrows_cycle_every_choice_and_refresh_shared_entry_labels() {
        let mut app = App::new();
        app.insert_resource(LaunchRequest {
            character_id: 0,
            weapon: 6,
            ..default()
        })
        .add_systems(Update, player_loadout_controls);
        let arrow = app
            .world_mut()
            .spawn((
                Button,
                Interaction::None,
                LoadoutArrow {
                    field: LoadoutField::Character,
                    direction: 1,
                },
            ))
            .id();
        for _ in 0..2 {
            app.world_mut()
                .spawn((Text::default(), LoadoutField::Character));
        }
        app.world_mut()
            .spawn((Text::default(), LoadoutField::Weapon));
        app.update();
        for id in (1..20).chain(std::iter::once(0)) {
            *app.world_mut().get_mut::<Interaction>(arrow).unwrap() = Interaction::Pressed;
            app.update();
            assert_eq!(app.world().resource::<LaunchRequest>().character_id, id);
            for (field, text) in app
                .world_mut()
                .query::<(&LoadoutField, &Text)>()
                .iter(app.world())
            {
                if *field == LoadoutField::Character {
                    assert_eq!(text.0, CHARACTER_NAMES[id as usize]);
                }
            }
        }
        app.world_mut()
            .get_mut::<LoadoutArrow>(arrow)
            .unwrap()
            .direction = -1;
        *app.world_mut().get_mut::<Interaction>(arrow).unwrap() = Interaction::Pressed;
        app.update();
        assert_eq!(app.world().resource::<LaunchRequest>().character_id, 19);
        *app.world_mut().get_mut::<LoadoutArrow>(arrow).unwrap() = LoadoutArrow {
            field: LoadoutField::Weapon,
            direction: 1,
        };
        for id in LOADOUT_WEAPONS
            .into_iter()
            .skip(1)
            .chain(std::iter::once(6))
        {
            *app.world_mut().get_mut::<Interaction>(arrow).unwrap() = Interaction::Pressed;
            app.update();
            assert_eq!(app.world().resource::<LaunchRequest>().weapon, id);
        }
        app.world_mut()
            .get_mut::<LoadoutArrow>(arrow)
            .unwrap()
            .direction = -1;
        *app.world_mut().get_mut::<Interaction>(arrow).unwrap() = Interaction::Pressed;
        app.update();
        assert_eq!(app.world().resource::<LaunchRequest>().weapon, 12);
        let text = app
            .world_mut()
            .query::<(&LoadoutField, &Text)>()
            .iter(app.world())
            .find(|(field, _)| **field == LoadoutField::Weapon)
            .unwrap()
            .1;
        assert_eq!(text.0, "忍刀");
    }

    #[test]
    fn android_launcher_fonts_keep_compact_readable_hierarchy() {
        assert_eq!(launcher_font_size(9.0, true), 10.0);
        assert!((launcher_font_size(13.0, true) - 11.18).abs() < 1e-5);
        assert!((launcher_font_size(16.0, true) - 13.76).abs() < 1e-5);
        assert_eq!(launcher_font_size(9.0, false), 9.0);
        assert_eq!(launcher_touch_target_layout_px(1.0), 44.0);
        assert!((launcher_touch_target_layout_px(0.7) * 0.7 - 44.0).abs() < 1e-5);
    }

    #[test]
    fn android_stage_layout_reserves_fixed_start_bar_and_hides_global_header() {
        let mut app = App::new();
        app.insert_resource(MenuScreen::Stages)
            .add_systems(Update, adapt_launcher_layout);
        app.world_mut().spawn(Window {
            resolution: (872_u32, 393_u32).into(),
            ..default()
        });
        let header = app
            .world_mut()
            .spawn((Node::default(), LauncherHeader))
            .id();
        let bar = app
            .world_mut()
            .spawn((Node::default(), LauncherMobileStartBar))
            .id();
        let aside_start = app
            .world_mut()
            .spawn((
                Node::default(),
                LauncherStageAsideStartButton,
                StartGameButton,
            ))
            .id();
        let layout = app
            .world_mut()
            .spawn((Node::default(), LauncherStageLayout))
            .id();
        app.update();

        assert_eq!(
            app.world().get::<Node>(header).unwrap().display,
            Display::None
        );
        assert_eq!(app.world().get::<Node>(bar).unwrap().display, Display::Flex);
        assert_eq!(
            app.world().get::<Node>(aside_start).unwrap().display,
            Display::None,
            "mobile layout must expose only the fixed bottom start action"
        );
        assert_eq!(
            app.world().get::<Node>(layout).unwrap().flex_direction,
            FlexDirection::Row
        );
    }

    #[test]
    fn network_browser_uses_an_action_rail_and_stacks_it_in_portrait() {
        let mut app = App::new();
        app.insert_resource(MenuScreen::Network)
            .add_systems(Update, adapt_launcher_layout);
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: (872_u32, 393_u32).into(),
                ..default()
            })
            .id();
        let layout = app
            .world_mut()
            .spawn((Node::default(), LauncherNetworkBrowserLayout))
            .id();
        let rail = app
            .world_mut()
            .spawn((Node::default(), LauncherNetworkActionRail))
            .id();
        let panel = app
            .world_mut()
            .spawn((Node::default(), LauncherNetworkRoomPanel))
            .id();

        app.update();
        assert_eq!(
            app.world().get::<Node>(layout).unwrap().flex_direction,
            FlexDirection::Row
        );
        assert_eq!(
            app.world().get::<Node>(rail).unwrap().flex_direction,
            FlexDirection::Column
        );
        assert_eq!(app.world().get::<Node>(rail).unwrap().width, Val::Px(124.0));
        assert_eq!(
            app.world().get::<Node>(panel).unwrap().min_height,
            Val::Px(160.0)
        );

        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution = (412_u32, 915_u32).into();
        app.update();
        assert_eq!(
            app.world().get::<Node>(layout).unwrap().flex_direction,
            FlexDirection::Column
        );
        assert_eq!(
            app.world().get::<Node>(rail).unwrap().flex_direction,
            FlexDirection::Row
        );
        assert_eq!(
            app.world().get::<Node>(rail).unwrap().width,
            Val::Percent(100.0)
        );
    }

    #[test]
    fn active_touch_suppresses_pointer_down_until_release() {
        let mut app = App::new();
        app.add_message::<TouchInput>()
            .insert_resource(LauncherTouchScrollState {
                pointer: Some(7),
                ..default()
            })
            .add_systems(Update, launcher_touch_scroll);
        let button = app
            .world_mut()
            .spawn((
                Button,
                LauncherStyledButton,
                Interaction::Pressed,
                Node::default(),
                ComputedNode::default(),
                UiGlobalTransform::default(),
            ))
            .id();
        app.update();
        assert_eq!(
            *app.world().get::<Interaction>(button).unwrap(),
            Interaction::None
        );
    }

    #[test]
    fn stationary_touch_synthesizes_click_only_on_release() {
        let mut app = App::new();
        app.add_message::<TouchInput>()
            .init_resource::<LauncherTouchScrollState>()
            .add_systems(Update, launcher_touch_scroll);
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: WindowResolution::new(800, 400).with_scale_factor_override(2.0),
                ..default()
            })
            .id();
        let button = app
            .world_mut()
            .spawn((
                Button,
                LauncherStyledButton,
                Interaction::Pressed,
                Node::default(),
                ComputedNode {
                    size: Vec2::new(100.0, 50.0),
                    ..default()
                },
                UiGlobalTransform::from_xy(300.0, 100.0),
                InheritedVisibility::VISIBLE,
            ))
            .id();
        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Started,
            position: Vec2::new(150.0, 50.0),
            window,
            force: None,
            id: 7,
        });
        app.update();
        assert_eq!(
            *app.world().get::<Interaction>(button).unwrap(),
            Interaction::None
        );

        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Ended,
            position: Vec2::new(150.0, 50.0),
            window,
            force: None,
            id: 7,
        });
        app.update();
        assert_eq!(
            *app.world().get::<Interaction>(button).unwrap(),
            Interaction::Pressed
        );
    }

    #[test]
    fn overlapping_touch_activates_topmost_visible_button_only() {
        let mut app = App::new();
        app.add_message::<TouchInput>()
            .init_resource::<LauncherTouchScrollState>()
            .add_systems(Update, launcher_touch_scroll);
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: WindowResolution::new(800, 400),
                ..default()
            })
            .id();
        let lower = app
            .world_mut()
            .spawn((
                Button,
                LauncherStyledButton,
                Interaction::None,
                Node::default(),
                ComputedNode {
                    size: Vec2::new(80.0, 40.0),
                    ..default()
                },
                UiGlobalTransform::from_xy(200.0, 100.0),
                InheritedVisibility::VISIBLE,
                ComputedStackIndex(2),
            ))
            .id();
        let upper = app
            .world_mut()
            .spawn((
                Button,
                LauncherStyledButton,
                Interaction::None,
                Node::default(),
                ComputedNode {
                    size: Vec2::new(140.0, 80.0),
                    ..default()
                },
                UiGlobalTransform::from_xy(200.0, 100.0),
                InheritedVisibility::VISIBLE,
                ComputedStackIndex(9),
            ))
            .id();
        let clipped = app
            .world_mut()
            .spawn((
                Button,
                LauncherStyledButton,
                Interaction::None,
                Node::default(),
                ComputedNode {
                    size: Vec2::new(160.0, 100.0),
                    ..default()
                },
                UiGlobalTransform::from_xy(200.0, 100.0),
                InheritedVisibility::VISIBLE,
                ComputedStackIndex(20),
                CalculatedClip {
                    clip: Rect::from_corners(Vec2::ZERO, Vec2::splat(40.0)),
                },
            ))
            .id();

        for phase in [TouchPhase::Started, TouchPhase::Ended] {
            app.world_mut().write_message(TouchInput {
                phase,
                position: Vec2::new(200.0, 100.0),
                window,
                force: None,
                id: 17,
            });
            app.update();
        }

        assert_eq!(
            *app.world().get::<Interaction>(upper).unwrap(),
            Interaction::Pressed
        );
        assert_eq!(
            *app.world().get::<Interaction>(lower).unwrap(),
            Interaction::None
        );
        assert_eq!(
            *app.world().get::<Interaction>(clipped).unwrap(),
            Interaction::None
        );
    }

    #[test]
    fn touch_move_beyond_slop_cancels_pending_click() {
        let mut app = App::new();
        app.add_message::<TouchInput>()
            .init_resource::<LauncherTouchScrollState>()
            .add_systems(Update, launcher_touch_scroll);
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: WindowResolution::new(800, 400).with_scale_factor_override(2.0),
                ..default()
            })
            .id();
        let button = app
            .world_mut()
            .spawn((
                Button,
                LauncherStyledButton,
                Interaction::Pressed,
                Node::default(),
                ComputedNode {
                    size: Vec2::new(100.0, 80.0),
                    ..default()
                },
                UiGlobalTransform::from_xy(300.0, 100.0),
                InheritedVisibility::VISIBLE,
            ))
            .id();
        for (phase, position) in [
            (TouchPhase::Started, Vec2::new(150.0, 50.0)),
            (TouchPhase::Moved, Vec2::new(150.0, 30.0)),
            (TouchPhase::Ended, Vec2::new(150.0, 30.0)),
        ] {
            app.world_mut().write_message(TouchInput {
                phase,
                position,
                window,
                force: None,
                id: 9,
            });
            app.update();
        }
        assert_eq!(
            *app.world().get::<Interaction>(button).unwrap(),
            Interaction::None
        );
    }

    #[test]
    fn touch_end_displacement_without_move_event_still_cancels_click() {
        let mut app = App::new();
        app.add_message::<TouchInput>()
            .init_resource::<LauncherTouchScrollState>()
            .add_systems(Update, launcher_touch_scroll);
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: WindowResolution::new(800, 400).with_scale_factor_override(2.0),
                ..default()
            })
            .id();
        let button = app
            .world_mut()
            .spawn((
                Button,
                LauncherStyledButton,
                Interaction::None,
                Node::default(),
                ComputedNode {
                    size: Vec2::new(240.0, 160.0),
                    ..default()
                },
                UiGlobalTransform::from_xy(300.0, 100.0),
                InheritedVisibility::VISIBLE,
            ))
            .id();

        for (phase, position) in [
            (TouchPhase::Started, Vec2::new(150.0, 50.0)),
            (TouchPhase::Ended, Vec2::new(150.0, 30.0)),
        ] {
            app.world_mut().write_message(TouchInput {
                phase,
                position,
                window,
                force: None,
                id: 19,
            });
            app.update();
        }

        assert_eq!(
            *app.world().get::<Interaction>(button).unwrap(),
            Interaction::None
        );
    }

    #[test]
    fn visible_modal_scrim_blocks_buttons_behind_it() {
        let mut app = App::new();
        app.add_message::<TouchInput>()
            .init_resource::<LauncherTouchScrollState>()
            .add_systems(Update, launcher_touch_scroll);
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: WindowResolution::new(800, 400),
                ..default()
            })
            .id();
        let underlying = app
            .world_mut()
            .spawn((
                Button,
                LauncherStyledButton,
                Interaction::None,
                Node::default(),
                ComputedNode {
                    size: Vec2::new(180.0, 120.0),
                    ..default()
                },
                UiGlobalTransform::from_xy(200.0, 100.0),
                InheritedVisibility::VISIBLE,
                ComputedStackIndex(2),
            ))
            .id();
        app.world_mut().spawn((
            Node::default(),
            ComputedNode {
                size: Vec2::new(800.0, 400.0),
                ..default()
            },
            UiGlobalTransform::from_xy(400.0, 200.0),
            InheritedVisibility::VISIBLE,
            ComputedStackIndex(10),
            UiInputBlocker,
        ));

        for phase in [TouchPhase::Started, TouchPhase::Ended] {
            app.world_mut().write_message(TouchInput {
                phase,
                position: Vec2::new(200.0, 100.0),
                window,
                force: None,
                id: 23,
            });
            app.update();
        }

        assert_eq!(
            *app.world().get::<Interaction>(underlying).unwrap(),
            Interaction::None
        );
    }

    #[test]
    fn hidpi_touch_drag_scrolls_the_smallest_visible_ui_area() {
        let mut app = App::new();
        app.add_message::<TouchInput>()
            .init_resource::<LauncherTouchScrollState>()
            .add_systems(Update, launcher_touch_scroll);
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: WindowResolution::new(800, 400).with_scale_factor_override(2.0),
                ..default()
            })
            .id();
        let area = app
            .world_mut()
            .spawn((
                Node {
                    overflow: Overflow::scroll_y(),
                    ..default()
                },
                ComputedNode {
                    size: Vec2::new(200.0, 160.0),
                    ..default()
                },
                UiGlobalTransform::from_xy(300.0, 100.0),
                InheritedVisibility::VISIBLE,
                ScrollPosition::default(),
                LauncherScrollArea,
            ))
            .id();

        for (phase, position) in [
            (TouchPhase::Started, Vec2::new(150.0, 50.0)),
            (TouchPhase::Moved, Vec2::new(150.0, 30.0)),
        ] {
            app.world_mut().write_message(TouchInput {
                phase,
                position,
                window,
                force: None,
                id: 11,
            });
            app.update();
        }

        assert_eq!(app.world().get::<ScrollPosition>(area).unwrap().0.y, 20.0);
    }

    #[test]
    fn switching_stage_tabs_refreshes_visuals_and_restores_each_selection() {
        use meteor_scene::stage_registry::StageInfo;

        let mut app = App::new();
        app.insert_resource(StageListTab::Official)
            .insert_resource(LaunchRequest::default())
            .insert_resource(StageSelectionMemory::default())
            .insert_resource(crate::plugins::stage_plugin::CustomStages {
                stages: vec![StageInfo {
                    id: "custom01".to_owned(),
                    number: 1,
                    name: "自制试炼场".to_owned(),
                    description: String::new(),
                    is_custom: true,
                }],
                packs: Vec::new(),
            })
            .add_systems(
                Update,
                (stage_tab_buttons, refresh_stage_tab_visuals).chain(),
            );

        let official_label = app.world_mut().spawn(TextColor(CYAN)).id();
        let official = app
            .world_mut()
            .spawn((
                Button,
                Interaction::None,
                LauncherStageTab(StageListTab::Official),
                StageTabOfficialButton,
                BackgroundColor(STAGE_CARD),
                BorderColor::all(LAUNCHER_LINE),
            ))
            .add_child(official_label)
            .id();
        let custom_label = app.world_mut().spawn(TextColor(CYAN)).id();
        let custom = app
            .world_mut()
            .spawn((
                Button,
                Interaction::Pressed,
                LauncherStageTab(StageListTab::Custom),
                StageTabCustomButton,
                BackgroundColor(STAGE_CARD),
                BorderColor::all(LAUNCHER_LINE),
            ))
            .add_child(custom_label)
            .id();

        app.update();
        assert_eq!(
            *app.world().resource::<StageListTab>(),
            StageListTab::Custom
        );
        assert_eq!(app.world().resource::<LaunchRequest>().stage_id, "custom01");
        assert_eq!(
            app.world().get::<BackgroundColor>(custom).unwrap().0,
            STAGE_CARD_ACTIVE
        );
        assert_eq!(app.world().get::<TextColor>(custom_label).unwrap().0, PAPER);
        assert_eq!(
            app.world().get::<BackgroundColor>(official).unwrap().0,
            STAGE_CARD
        );

        *app.world_mut().get_mut::<Interaction>(custom).unwrap() = Interaction::None;
        *app.world_mut().get_mut::<Interaction>(official).unwrap() = Interaction::Pressed;
        app.update();
        assert_eq!(
            *app.world().resource::<StageListTab>(),
            StageListTab::Official
        );
        assert_eq!(app.world().resource::<LaunchRequest>().stage_id, "sn01");
        assert_eq!(
            app.world().get::<BackgroundColor>(official).unwrap().0,
            STAGE_CARD_ACTIVE
        );
        assert_eq!(
            app.world().get::<TextColor>(official_label).unwrap().0,
            PAPER
        );
    }

    #[test]
    fn narrow_portrait_stacks_stage_content() {
        let layout = responsive_ui_layout(412.0, 915.0, true);
        assert!(layout.mobile);
        assert!(layout.stacked_stage);
    }

    #[test]
    fn desktop_retains_spacious_layout() {
        let layout = responsive_ui_layout(1280.0, 720.0, false);
        assert!(!layout.mobile);
        assert!(!layout.narrow);
        assert!(!layout.stacked_stage);
        assert_eq!(layout.left_gutter, 48.0);
        assert_eq!(layout.right_gutter, 48.0);
    }

    #[test]
    fn android_landscape_uses_asymmetric_display_cutout_insets() {
        let layout = responsive_ui_layout_with_safe_area(
            872.0,
            393.0,
            true,
            SafeAreaInsets {
                left: 34.0,
                top: 0.0,
                right: 6.0,
                bottom: 0.0,
            },
        );
        assert_eq!(layout.left_gutter, 42.0);
        assert_eq!(layout.right_gutter, 16.0);
        assert_eq!(layout.top_gutter, 8.0);
        assert_eq!(layout.bottom_gutter, 8.0);
    }
}

fn spawn_native_text_field(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    field: TextField,
    value: &str,
) {
    parent
        .spawn((
            Button,
            Node {
                width: Val::Percent(100.0),
                min_height: Val::Px(44.0),
                flex_shrink: 0.0,
                align_items: AlignItems::Center,
                padding: UiRect::all(Val::Px(10.0)),
                margin: UiRect::bottom(Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(STAGE_CARD),
            BorderColor::all(LAUNCHER_LINE),
            NativeTextEdit {
                field,
                value: value.into(),
            },
        ))
        .with_children(|row| {
            row.spawn((
                launcher_text(&format!("{}：{}", field.title(), value), 13.0, PAPER, font),
                NativeTextLabel(field),
            ));
        });
}
