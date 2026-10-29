use super::ui_theme as theme;
use crate::app_state::{AppState, InputBinding, Keymap, LauncherStyledButton, Settings};
use crate::plugins::input_plugin::{LocalCommandInput, SyntheticAction, SyntheticInputStream};
use crate::plugins::launcher_ui::UiFont;
use crate::plugins::ui_scale_plugin::{EffectiveUiScale, SafeAreaInsets};
use bevy::input::touch::{TouchInput, TouchPhase};
use bevy::prelude::*;
use bevy::ui::ComputedStackIndex;
use meteor_input::command_parser::Dir;
use std::collections::{HashMap, HashSet};

const STICK_THRESHOLD: f32 = 0.28;
const COMMAND_PRESS_THRESHOLD: f32 = 0.28;
const COMMAND_RELEASE_THRESHOLD: f32 = 0.18;
const COMMAND_AXIS_SWITCH_RATIO: f32 = 1.3;
const DODGE_TAP_WINDOW: f64 = 0.25;

const MIN_TOUCH_TARGET: f32 = 48.0;

const FLOATING_STICK_ZONE_X: f32 = 0.45;
const LAYOUT_MIN: f32 = 5.0;
const LAYOUT_MAX: f32 = 95.0;
const SCALE_MIN: f32 = 0.65;
const SCALE_MAX: f32 = 1.5;
const EDITOR_TOUCH_SLOP: f32 = 8.0;
#[cfg(target_arch = "wasm32")]
const MOBILE_LAYOUT_STORAGE_KEY: &str = "meteor.mobile-controls.layout.v1";

#[cfg(target_os = "android")]
const MOBILE_LAYOUT_FILE: &str = "mobile_controls_layout.json";

#[derive(Debug, Clone, Copy, PartialEq)]
struct MobileLayoutItem {
    x: f32,
    y: f32,
    scale: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum MobileLayoutKey {
    Stick,
    Attack,
    Jump,
    Guard,
    Crouch,
    SwitchWeapon,
    Fake,
    UnlockCamera,
    QiBurst,
    Cure,
    Radar,
    DropWeapon,
    DropItem,
}

impl MobileLayoutKey {
    const ALL: [Self; 13] = [
        Self::Stick,
        Self::Attack,
        Self::Jump,
        Self::Guard,
        Self::Crouch,
        Self::SwitchWeapon,
        Self::Fake,
        Self::UnlockCamera,
        Self::QiBurst,
        Self::Cure,
        Self::Radar,
        Self::DropWeapon,
        Self::DropItem,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Stick => "stick",
            Self::Attack => "attack",
            Self::Jump => "jump",
            Self::Guard => "guard",
            Self::Crouch => "crouch",
            Self::SwitchWeapon => "switch_weapon",
            Self::Fake => "fake",
            Self::UnlockCamera => "unlock_camera",
            Self::QiBurst => "qi_burst",
            Self::Cure => "cure",
            Self::Radar => "radar",
            Self::DropWeapon => "drop_weapon",
            Self::DropItem => "drop_item",
        }
    }

    fn fallback(self) -> MobileLayoutItem {
        let (x, y) = match self {
            Self::Stick => (12.0, 80.0),
            Self::Attack => (94.0, 80.0),
            Self::Jump => (84.0, 86.0),
            Self::Guard => (84.0, 66.0),
            Self::Crouch => (74.0, 86.0),
            Self::SwitchWeapon => (74.0, 66.0),
            Self::Fake => (74.0, 46.0),
            Self::UnlockCamera => (84.0, 46.0),
            Self::QiBurst => (94.0, 56.0),
            Self::Cure => (62.0, 94.0),
            Self::Radar => (90.0, 14.0),
            Self::DropWeapon => (50.0, 94.0),
            Self::DropItem => (38.0, 94.0),
        };
        MobileLayoutItem { x, y, scale: 1.0 }
    }
}

#[derive(Resource, Debug, Clone, PartialEq)]
struct MobileControlLayout(HashMap<MobileLayoutKey, MobileLayoutItem>);

impl Default for MobileControlLayout {
    fn default() -> Self {
        let text = mobile_layout_storage_text();
        Self(normalize_mobile_layout(text.as_deref()))
    }
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn mobile_layout_storage_text() -> Option<String> {
    None
}

#[cfg(target_os = "android")]
fn mobile_layout_storage_text() -> Option<String> {
    std::fs::read_to_string(MOBILE_LAYOUT_FILE).ok()
}

#[cfg(target_arch = "wasm32")]
fn mobile_layout_storage_text() -> Option<String> {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(MOBILE_LAYOUT_STORAGE_KEY).ok().flatten())
}

fn normalize_mobile_layout(text: Option<&str>) -> HashMap<MobileLayoutKey, MobileLayoutItem> {
    let parsed = text
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    MobileLayoutKey::ALL
        .into_iter()
        .map(|key| {
            let fallback = key.fallback();
            let item = parsed.get(key.name()).and_then(|value| value.as_object());
            let number = |name: &str, fallback: f32| {
                item.and_then(|item| item.get(name))
                    .and_then(|value| value.as_f64())
                    .filter(|value| value.is_finite())
                    .map(|value| value as f32)
                    .unwrap_or(fallback)
            };
            (
                key,
                MobileLayoutItem {
                    x: number("x", fallback.x).clamp(LAYOUT_MIN, LAYOUT_MAX),
                    y: number("y", fallback.y).clamp(LAYOUT_MIN, LAYOUT_MAX),
                    scale: number("scale", fallback.scale).clamp(SCALE_MIN, SCALE_MAX),
                },
            )
        })
        .collect()
}

fn default_mobile_layout() -> HashMap<MobileLayoutKey, MobileLayoutItem> {
    MobileLayoutKey::ALL
        .into_iter()
        .map(|key| (key, key.fallback()))
        .collect()
}

fn normalize_mobile_layout_items(
    source: &HashMap<MobileLayoutKey, MobileLayoutItem>,
) -> HashMap<MobileLayoutKey, MobileLayoutItem> {
    MobileLayoutKey::ALL
        .into_iter()
        .map(|key| {
            let fallback = key.fallback();
            let source = source.get(&key).copied().unwrap_or(fallback);
            let finite_or = |value: f32, fallback: f32| {
                if value.is_finite() {
                    value
                } else {
                    fallback
                }
            };
            (
                key,
                MobileLayoutItem {
                    x: finite_or(source.x, fallback.x).clamp(LAYOUT_MIN, LAYOUT_MAX),
                    y: finite_or(source.y, fallback.y).clamp(LAYOUT_MIN, LAYOUT_MAX),
                    scale: finite_or(source.scale, fallback.scale).clamp(SCALE_MIN, SCALE_MAX),
                },
            )
        })
        .collect()
}

#[cfg(any(target_arch = "wasm32", target_os = "android", test))]
fn mobile_layout_json(layout: &HashMap<MobileLayoutKey, MobileLayoutItem>) -> String {
    let normalized = normalize_mobile_layout_items(layout);
    let mut object = serde_json::Map::new();
    for key in MobileLayoutKey::ALL {
        let item = normalized[&key];
        object.insert(
            key.name().to_owned(),
            serde_json::json!({
                "x": item.x,
                "y": item.y,
                "scale": item.scale,
            }),
        );
    }
    serde_json::Value::Object(object).to_string()
}

#[cfg(target_os = "android")]
fn persist_mobile_layout(layout: &HashMap<MobileLayoutKey, MobileLayoutItem>) -> bool {
    std::fs::write(MOBILE_LAYOUT_FILE, mobile_layout_json(layout)).is_ok()
}

#[cfg(target_arch = "wasm32")]
fn persist_mobile_layout(layout: &HashMap<MobileLayoutKey, MobileLayoutItem>) -> bool {
    let text = mobile_layout_json(layout);
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .is_some_and(|storage| storage.set_item(MOBILE_LAYOUT_STORAGE_KEY, &text).is_ok())
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn persist_mobile_layout(_layout: &HashMap<MobileLayoutKey, MobileLayoutItem>) -> bool {
    true
}

pub(crate) fn mobile_layout_editor_available() -> bool {
    detect_touch_controls()
}

#[derive(Component)]
pub(crate) struct OpenMobileLayoutEditorButton;

#[derive(Component)]
struct MobileLayoutEditorRoot;

#[derive(Component)]
struct MobileLayoutEditorPreviewArea;

#[derive(Component)]
struct MobileLayoutEditorPreview;

#[derive(Component, Clone, Copy)]
struct MobileLayoutEditorItem(MobileLayoutKey);

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum MobileLayoutEditorAction {
    SizeDown,
    SizeUp,
    Reset,
    Cancel,
    Save,
}

#[derive(Component)]
struct MobileLayoutEditorSelectionText;

#[derive(Resource, Debug)]
pub(crate) struct MobileLayoutEditorState {
    open: bool,
    draft: HashMap<MobileLayoutKey, MobileLayoutItem>,
    selected: MobileLayoutKey,
    pointer: Option<u64>,
    drag_item: Option<MobileLayoutKey>,
    pending_button: Option<Entity>,
    start: Vec2,
    dragging: bool,
}

impl Default for MobileLayoutEditorState {
    fn default() -> Self {
        Self {
            open: false,
            draft: default_mobile_layout(),
            selected: MobileLayoutKey::Attack,
            pointer: None,
            drag_item: None,
            pending_button: None,
            start: Vec2::ZERO,
            dragging: false,
        }
    }
}

impl MobileLayoutEditorState {
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    fn clear_pointer(&mut self) {
        self.pointer = None;
        self.drag_item = None;
        self.pending_button = None;
        self.dragging = false;
    }
}

#[derive(Resource, Debug)]
pub struct MobileControlsConfig {
    pub visible: bool,
}

impl MobileControlsConfig {
    pub fn resolve(settings: &Settings) -> Self {
        let visible = match settings.touch_controls {
            0 => false,
            1 => true,
            _ => detect_touch_controls(),
        };
        Self { visible }
    }
}

impl Default for MobileControlsConfig {
    fn default() -> Self {
        Self {
            visible: detect_touch_controls(),
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
fn detect_touch_controls() -> bool {
    false
}

#[cfg(target_os = "android")]
fn detect_touch_controls() -> bool {
    true
}

#[cfg(target_arch = "wasm32")]
fn detect_touch_controls() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    if let Ok(search) = window.location().search() {
        for pair in search.trim_start_matches('?').split('&') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            if key == "mobileControls" {
                return value == "1";
            }
        }
    }
    let touch_points = window.navigator().max_touch_points();
    let coarse = window
        .match_media("(pointer: coarse)")
        .ok()
        .flatten()
        .is_some_and(|query| query.matches());
    let touch_property = js_sys::Reflect::has(
        window.as_ref(),
        &wasm_bindgen::JsValue::from_str("ontouchstart"),
    )
    .unwrap_or(false);
    touch_points > 0 && (coarse || touch_property)
}

fn mobile_layout_editor_open(state: Res<MobileLayoutEditorState>) -> bool {
    state.open
}

fn editor_px(value: f32, ui_scale: f32) -> f32 {
    value / ui_scale.max(0.05)
}

fn editor_text(
    value: impl Into<String>,
    size: f32,
    color: Color,
    font: &Handle<Font>,
    ui_scale: f32,
) -> impl Bundle {
    (
        Text::new(value),
        TextFont {
            font: font.clone().into(),
            font_size: editor_px(size, ui_scale).into(),
            ..default()
        },
        TextColor(color),
    )
}

fn mobile_layout_item_label(key: MobileLayoutKey) -> &'static str {
    match key {
        MobileLayoutKey::Stick => "方向",
        MobileLayoutKey::Attack => "攻",
        MobileLayoutKey::Jump => "跃",
        MobileLayoutKey::Guard => "防",
        MobileLayoutKey::Crouch => "蹲",
        MobileLayoutKey::SwitchWeapon => "换",
        MobileLayoutKey::Fake => "假死",
        MobileLayoutKey::UnlockCamera => "解锁",
        MobileLayoutKey::QiBurst => "爆",
        MobileLayoutKey::Cure => "救人",
        MobileLayoutKey::Radar => "雷达",
        MobileLayoutKey::DropWeapon => "丢武器",
        MobileLayoutKey::DropItem => "丢道具",
    }
}

fn mobile_layout_control_kind(key: MobileLayoutKey) -> MobileControlKind {
    if key == MobileLayoutKey::Stick {
        MobileControlKind::Stick
    } else if key == MobileLayoutKey::Radar {
        MobileControlKind::Radar
    } else {
        let (action, _, _, _) = ACTION_CONTROLS
            .iter()
            .find(|(_, layout_key, _, _)| *layout_key == key)
            .expect("editable action has a runtime control");
        MobileControlKind::Action(*action)
    }
}

fn mobile_layout_preview_background(key: MobileLayoutKey) -> Color {
    match key {
        MobileLayoutKey::Stick => Color::srgba(0.16, 0.15, 0.10, 0.97),
        MobileLayoutKey::Attack => Color::srgba(0.41, 0.12, 0.11, 0.94),
        _ => Color::srgba(0.13, 0.14, 0.14, 0.94),
    }
}

fn spawn_mobile_layout_editor_action(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    action: MobileLayoutEditorAction,
    primary: bool,
    ui_scale: f32,
) {
    let target = editor_px(48.0, ui_scale);
    parent
        .spawn((
            Button,
            Node {
                min_width: Val::Px(editor_px(if primary { 104.0 } else { 80.0 }, ui_scale)),
                min_height: Val::Px(target),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(editor_px(12.0, ui_scale))),
                border: UiRect::all(Val::Px(editor_px(1.0, ui_scale))),
                ..default()
            },
            BackgroundColor(if primary {
                theme::PRIMARY
            } else {
                Color::srgba(0.08, 0.09, 0.09, 0.98)
            }),
            BorderColor::all(if primary {
                theme::ACCENT_TEXT
            } else {
                theme::LINE
            }),
            Interaction::default(),
            LauncherStyledButton,
            action,
        ))
        .with_children(|button| {
            button.spawn(editor_text(
                label,
                14.0,
                Color::srgb(0.97, 0.93, 0.85),
                font,
                ui_scale,
            ));
        });
}

fn spawn_mobile_layout_editor(
    commands: &mut Commands,
    font: &Handle<Font>,
    state: &MobileLayoutEditorState,
    ui_scale: f32,
) {
    let unit = ui_scale.max(0.05);
    let horizontal_padding = editor_px(
        if cfg!(target_os = "android") {
            28.0
        } else {
            20.0
        },
        unit,
    );
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            BackgroundColor(Color::BLACK),
            bevy::ui::FocusPolicy::Block,
            ZIndex(500),
            MobileLayoutEditorRoot,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Percent(100.0),
                    min_height: Val::Px(editor_px(72.0, unit)),
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(editor_px(20.0, unit)),
                    padding: UiRect::axes(
                        Val::Px(horizontal_padding),
                        Val::Px(editor_px(12.0, unit)),
                    ),
                    border: UiRect::bottom(Val::Px(editor_px(1.0, unit))),
                    ..default()
                },
                BorderColor::all(Color::srgba(0.90, 0.72, 0.37, 0.28)),
            ))
            .with_children(|header| {
                header
                    .spawn((Node {
                        min_width: Val::Px(0.0),
                        flex_direction: FlexDirection::Column,
                        ..default()
                    },))
                    .with_children(|title| {
                        title.spawn(editor_text(
                            "移动端操作",
                            12.0,
                            Color::srgb(0.90, 0.72, 0.37),
                            font,
                            unit,
                        ));
                        title.spawn(editor_text(
                            "调整按键布局",
                            24.0,
                            Color::srgb(0.94, 0.91, 0.84),
                            font,
                            unit,
                        ));
                    });
                header.spawn((
                    editor_text(
                        "预览按游戏画面等比缩小；拖动按键，选中后调整大小。",
                        12.0,
                        Color::srgba(0.94, 0.91, 0.84, 0.62),
                        font,
                        unit,
                    ),
                    Node {
                        max_width: Val::Px(editor_px(310.0, unit)),
                        ..default()
                    },
                ));
            });

            root.spawn((
                Node {
                    width: Val::Percent(100.0),
                    flex_grow: 1.0,
                    min_height: Val::Px(0.0),
                    position_type: PositionType::Relative,
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.08, 0.08, 0.08)),
                MobileLayoutEditorPreviewArea,
            ))
            .with_children(|area| {
                area.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Px(0.0),
                        height: Val::Px(0.0),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    BackgroundColor(Color::BLACK),
                    MobileLayoutEditorPreview,
                ))
                .with_children(|preview| {
                    for key in std::iter::once(MobileLayoutKey::Stick)
                        .chain(ACTION_CONTROLS.iter().map(|(_, key, _, _)| *key))
                        .chain(std::iter::once(MobileLayoutKey::Radar))
                    {
                        let item = state.draft[&key];
                        preview
                            .spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    width: Val::Px(0.0),
                                    height: Val::Px(0.0),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border: UiRect::all(Val::Px(editor_px(1.0, unit))),
                                    border_radius: BorderRadius::all(Val::Percent(50.0)),
                                    ..default()
                                },
                                BackgroundColor(mobile_layout_preview_background(key)),
                                BorderColor::all(if key == state.selected {
                                    Color::srgb(1.0, 0.89, 0.64)
                                } else {
                                    Color::srgba(0.91, 0.87, 0.77, 0.58)
                                }),
                                MobileLayoutEditorItem(key),
                            ))
                            .with_children(|preview_item| {
                                preview_item.spawn(editor_text(
                                    mobile_layout_item_label(key),
                                    (if key == MobileLayoutKey::Attack {
                                        24.0
                                    } else {
                                        16.0
                                    }) * item.scale,
                                    Color::srgb(0.97, 0.94, 0.87),
                                    font,
                                    unit,
                                ));
                            });
                    }
                });
            });

            root.spawn((
                Node {
                    width: Val::Percent(100.0),
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(editor_px(12.0, unit)),
                    row_gap: Val::Px(editor_px(8.0, unit)),
                    padding: UiRect::axes(
                        Val::Px(horizontal_padding),
                        Val::Px(editor_px(12.0, unit)),
                    ),
                    border: UiRect::top(Val::Px(editor_px(1.0, unit))),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.027, 0.027, 0.027)),
                BorderColor::all(Color::srgba(0.90, 0.72, 0.37, 0.28)),
            ))
            .with_children(|footer| {
                footer.spawn((
                    editor_text(
                        format!(
                            "已选「{}」；可缩小或放大",
                            mobile_layout_item_label(state.selected)
                        ),
                        12.0,
                        Color::srgba(0.94, 0.91, 0.84, 0.66),
                        font,
                        unit,
                    ),
                    MobileLayoutEditorSelectionText,
                ));
                footer
                    .spawn((Node {
                        flex_direction: FlexDirection::Row,
                        flex_wrap: FlexWrap::Wrap,
                        justify_content: JustifyContent::FlexEnd,
                        column_gap: Val::Px(editor_px(8.0, unit)),
                        row_gap: Val::Px(editor_px(8.0, unit)),
                        ..default()
                    },))
                    .with_children(|actions| {
                        spawn_mobile_layout_editor_action(
                            actions,
                            font,
                            "缩小",
                            MobileLayoutEditorAction::SizeDown,
                            false,
                            unit,
                        );
                        spawn_mobile_layout_editor_action(
                            actions,
                            font,
                            "放大",
                            MobileLayoutEditorAction::SizeUp,
                            false,
                            unit,
                        );
                        spawn_mobile_layout_editor_action(
                            actions,
                            font,
                            "恢复默认",
                            MobileLayoutEditorAction::Reset,
                            false,
                            unit,
                        );
                        spawn_mobile_layout_editor_action(
                            actions,
                            font,
                            "取消",
                            MobileLayoutEditorAction::Cancel,
                            false,
                            unit,
                        );
                        spawn_mobile_layout_editor_action(
                            actions,
                            font,
                            "保存布局",
                            MobileLayoutEditorAction::Save,
                            true,
                            unit,
                        );
                    });
            });
        });
}

fn open_mobile_layout_editor(
    query: Query<&Interaction, (Changed<Interaction>, With<OpenMobileLayoutEditorButton>)>,
    mut commands: Commands,
    font: Res<UiFont>,
    ui_scale: Res<EffectiveUiScale>,
    layout: Res<MobileControlLayout>,
    mut state: ResMut<MobileLayoutEditorState>,
    roots: Query<Entity, With<MobileLayoutEditorRoot>>,
) {
    if !query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        return;
    }
    for root in &roots {
        commands.entity(root).despawn();
    }
    state.open = true;
    state.draft = layout.0.clone();
    state.selected = MobileLayoutKey::Attack;
    state.clear_pointer();
    spawn_mobile_layout_editor(&mut commands, &font.0, &state, ui_scale.0);
}

fn close_mobile_layout_editor(
    commands: &mut Commands,
    state: &mut MobileLayoutEditorState,
    roots: &Query<Entity, With<MobileLayoutEditorRoot>>,
) {
    state.open = false;
    state.clear_pointer();
    for root in roots {
        commands.entity(root).despawn();
    }
}

#[allow(clippy::type_complexity)]
fn mobile_layout_editor_actions(
    query: Query<(&Interaction, &MobileLayoutEditorAction), (Changed<Interaction>, With<Button>)>,
    mut commands: Commands,
    mut state: ResMut<MobileLayoutEditorState>,
    mut layout: ResMut<MobileControlLayout>,
    roots: Query<Entity, With<MobileLayoutEditorRoot>>,
) {
    for (interaction, action) in &query {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match action {
            MobileLayoutEditorAction::SizeDown | MobileLayoutEditorAction::SizeUp => {
                let factor = if *action == MobileLayoutEditorAction::SizeDown {
                    0.9
                } else {
                    1.1
                };
                let selected = state.selected;
                if let Some(item) = state.draft.get_mut(&selected) {
                    item.scale = (item.scale * factor).clamp(SCALE_MIN, SCALE_MAX);
                }
            }
            MobileLayoutEditorAction::Reset => {
                state.draft = default_mobile_layout();
            }
            MobileLayoutEditorAction::Cancel => {
                close_mobile_layout_editor(&mut commands, &mut state, &roots);
                break;
            }
            MobileLayoutEditorAction::Save => {
                let normalized = normalize_mobile_layout_items(&state.draft);
                if !persist_mobile_layout(&normalized) {
                    warn!("mobile control layout could not be persisted");
                }
                layout.0 = normalized;
                close_mobile_layout_editor(&mut commands, &mut state, &roots);
                break;
            }
        }
    }
}

fn mobile_layout_editor_hit(
    node: &Node,
    computed: &ComputedNode,
    transform: &UiGlobalTransform,
    visibility: Option<&InheritedVisibility>,
    physical_position: Vec2,
) -> bool {
    node.display != Display::None
        && visibility.is_none_or(|visibility| visibility.get())
        && computed.contains_point(*transform, physical_position)
}

fn update_mobile_layout_drag(
    state: &mut MobileLayoutEditorState,
    preview: (&ComputedNode, &UiGlobalTransform),
    physical_position: Vec2,
) {
    let Some(key) = state.drag_item else {
        return;
    };
    let Some(normalized) = preview.0.normalize_point(*preview.1, physical_position) else {
        return;
    };
    if let Some(item) = state.draft.get_mut(&key) {
        item.x = ((normalized.x + 0.5) * 100.0).clamp(LAYOUT_MIN, LAYOUT_MAX);
        item.y = ((normalized.y + 0.5) * 100.0).clamp(LAYOUT_MIN, LAYOUT_MAX);
    }
}

#[allow(clippy::type_complexity)]
fn mobile_layout_editor_touch(
    mut events: MessageReader<TouchInput>,
    windows: Query<&Window>,
    mut state: ResMut<MobileLayoutEditorState>,
    preview: Query<(&ComputedNode, &UiGlobalTransform), With<MobileLayoutEditorPreview>>,
    items: Query<(
        &MobileLayoutEditorItem,
        &Node,
        &ComputedNode,
        &UiGlobalTransform,
        Option<&InheritedVisibility>,
        &ComputedStackIndex,
    )>,
    mut buttons: Query<
        (
            Entity,
            &Node,
            &ComputedNode,
            &UiGlobalTransform,
            Option<&InheritedVisibility>,
            &ComputedStackIndex,
            &mut Interaction,
        ),
        (With<Button>, With<MobileLayoutEditorAction>),
    >,
) {
    if !state.open {
        events.read().for_each(drop);
        state.clear_pointer();
        return;
    }
    let Ok(preview) = preview.single() else {
        events.read().for_each(drop);
        return;
    };
    for event in events.read() {
        match event.phase {
            TouchPhase::Started if state.pointer.is_none() => {
                let Ok(window) = windows.get(event.window) else {
                    continue;
                };
                let physical_position = event.position * window.scale_factor();
                let hit_item = items
                    .iter()
                    .filter(|(_, node, computed, transform, visibility, _)| {
                        mobile_layout_editor_hit(
                            node,
                            computed,
                            transform,
                            *visibility,
                            physical_position,
                        )
                    })
                    .max_by_key(|(_, _, _, _, _, stack)| stack.0)
                    .map(|(item, _, _, _, _, _)| item.0);
                let hit_button = buttons
                    .iter_mut()
                    .filter(|(_, node, computed, transform, visibility, _, _)| {
                        mobile_layout_editor_hit(
                            node,
                            computed,
                            transform,
                            *visibility,
                            physical_position,
                        )
                    })
                    .max_by_key(|(_, _, _, _, _, stack, _)| stack.0)
                    .map(|(entity, _, _, _, _, _, _)| entity);

                state.pointer = Some(event.id);
                state.start = event.position;
                state.dragging = false;
                state.drag_item = hit_item;
                state.pending_button = hit_item.is_none().then_some(hit_button).flatten();
                if let Some(key) = hit_item {
                    state.selected = key;
                    update_mobile_layout_drag(&mut state, preview, physical_position);
                }
            }
            TouchPhase::Moved if state.pointer == Some(event.id) => {
                if event.position.distance(state.start) >= EDITOR_TOUCH_SLOP {
                    state.dragging = true;
                }
                if state.drag_item.is_some() {
                    let Ok(window) = windows.get(event.window) else {
                        continue;
                    };
                    update_mobile_layout_drag(
                        &mut state,
                        preview,
                        event.position * window.scale_factor(),
                    );
                }
            }
            TouchPhase::Ended | TouchPhase::Canceled if state.pointer == Some(event.id) => {
                let pending = (event.phase == TouchPhase::Ended && !state.dragging)
                    .then_some(state.pending_button)
                    .flatten();
                let physical_position = windows
                    .get(event.window)
                    .ok()
                    .map(|window| event.position * window.scale_factor());
                let tap = pending.filter(|entity| {
                    physical_position.is_some_and(|physical_position| {
                        buttons.get_mut(*entity).is_ok_and(
                            |(_, node, computed, transform, visibility, _, _)| {
                                mobile_layout_editor_hit(
                                    node,
                                    computed,
                                    transform,
                                    visibility,
                                    physical_position,
                                )
                            },
                        )
                    })
                });
                state.clear_pointer();
                if let Some(entity) = tap {
                    if let Ok((_, _, _, _, _, _, mut interaction)) = buttons.get_mut(entity) {
                        *interaction = Interaction::Pressed;
                    }
                }
            }
            _ => {}
        }
    }

    if state.pointer.is_some() {
        for (_, _, _, _, _, _, mut interaction) in &mut buttons {
            if *interaction == Interaction::Pressed {
                *interaction = Interaction::None;
            }
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn refresh_mobile_layout_editor(
    state: Res<MobileLayoutEditorState>,
    ui_scale: Res<EffectiveUiScale>,
    windows: Query<&Window>,
    safe_area: Option<Res<SafeAreaInsets>>,
    areas: Query<&ComputedNode, With<MobileLayoutEditorPreviewArea>>,
    mut previews: Query<
        &mut Node,
        (
            With<MobileLayoutEditorPreview>,
            Without<MobileLayoutEditorItem>,
        ),
    >,
    mut items: Query<(
        &MobileLayoutEditorItem,
        &mut Node,
        &mut BorderColor,
        &Children,
    )>,
    mut fonts: Query<&mut TextFont>,
    mut selection: Query<&mut Text, With<MobileLayoutEditorSelectionText>>,
) {
    let (Ok(window), Ok(area), Ok(mut preview)) =
        (windows.single(), areas.single(), previews.single_mut())
    else {
        return;
    };
    let viewport = Vec2::new(window.width().max(1.0), window.height().max(1.0));
    let available = area.size() / window.scale_factor();
    if available.min_element() <= 0.0 {
        return;
    }

    let ratio = (available / viewport).min_element().min(1.0);
    let size = viewport * ratio;
    let unit = ui_scale.0.max(0.05);
    preview.left = Val::Px(editor_px((available.x - size.x) * 0.5, unit));
    preview.top = Val::Px(editor_px((available.y - size.y) * 0.5, unit));
    preview.width = Val::Px(editor_px(size.x, unit));
    preview.height = Val::Px(editor_px(size.y, unit));
    let resolved = resolve_mobile_layout_with_safe_area(
        &MobileControlLayout(state.draft.clone()),
        viewport.x,
        viewport.y,
        safe_area.as_deref().copied().unwrap_or_default(),
    );
    let factor = mobile_control_size_factor(viewport.x, viewport.y);
    for (marker, mut node, mut border, children) in &mut items {
        let item = state.draft[&marker.0];
        let region = resolved
            .region(mobile_layout_control_kind(marker.0))
            .expect("editable control has a resolved region");
        let geometry = control_node(region.center * ratio, region.visual_size * ratio, unit);
        node.left = geometry.left;
        node.top = geometry.top;
        node.width = geometry.width;
        node.height = geometry.height;
        node.margin = geometry.margin;
        border.set_all(if marker.0 == state.selected {
            Color::srgb(1.0, 0.89, 0.64)
        } else {
            Color::srgba(0.91, 0.87, 0.77, 0.58)
        });
        for child in children.iter() {
            if let Ok(mut font) = fonts.get_mut(child) {
                font.font_size = editor_px(
                    action_base_font(mobile_layout_item_label(marker.0))
                        * item.scale
                        * factor
                        * ratio,
                    unit,
                )
                .into();
            }
        }
    }
    for mut text in &mut selection {
        text.0 = format!(
            "已选「{}」；可缩小或放大",
            mobile_layout_item_label(state.selected)
        );
    }
}

fn mobile_layout_editor_escape(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    mut state: ResMut<MobileLayoutEditorState>,
    roots: Query<Entity, With<MobileLayoutEditorRoot>>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        close_mobile_layout_editor(&mut commands, &mut state, &roots);
    }
}

fn close_mobile_layout_editor_outside_settings(
    app_state: Res<State<AppState>>,
    mut commands: Commands,
    mut editor: ResMut<MobileLayoutEditorState>,
    roots: Query<Entity, With<MobileLayoutEditorRoot>>,
) {
    if !matches!(app_state.get(), AppState::Menu | AppState::Settings) {
        close_mobile_layout_editor(&mut commands, &mut editor, &roots);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum MobileDirection {
    Forward,
    Backward,
    Left,
    Right,
}

impl MobileDirection {
    const ALL: [Self; 4] = [Self::Forward, Self::Backward, Self::Left, Self::Right];

    fn index(self) -> usize {
        match self {
            Self::Forward => 0,
            Self::Backward => 1,
            Self::Left => 2,
            Self::Right => 3,
        }
    }

    fn command_direction(self) -> Dir {
        match self {
            Self::Forward => Dir::Up,
            Self::Backward => Dir::Down,
            Self::Left => Dir::Left,
            Self::Right => Dir::Right,
        }
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum MobileAction {
    Pause,
    Attack,
    Jump,
    Guard,
    Crouch,
    SwitchWeapon,
    Fake,
    UnlockCamera,
    QiBurst,
    Cure,
    DropWeapon,
    DropItem,
}

impl MobileAction {
    const ALL: [Self; 12] = [
        Self::Pause,
        Self::Attack,
        Self::Jump,
        Self::Guard,
        Self::Crouch,
        Self::SwitchWeapon,
        Self::Fake,
        Self::UnlockCamera,
        Self::QiBurst,
        Self::Cure,
        Self::DropWeapon,
        Self::DropItem,
    ];

    fn synthetic_action(self) -> Option<SyntheticAction> {
        Some(match self {
            Self::Pause => return None,
            Self::Attack => SyntheticAction::Attack,
            Self::Jump => SyntheticAction::Jump,
            Self::Guard => SyntheticAction::Guard,
            Self::Crouch => SyntheticAction::Crouch,
            Self::SwitchWeapon => SyntheticAction::WeaponSwitch,
            Self::Fake => SyntheticAction::Fake,
            Self::UnlockCamera => SyntheticAction::LockToggle,
            Self::QiBurst => SyntheticAction::QiBurst,
            Self::Cure => SyntheticAction::Cure,

            Self::DropWeapon => SyntheticAction::DropWeapon,
            Self::DropItem => SyntheticAction::DropItem,
        })
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
enum MobileControlKind {
    Stick,
    Radar,
    Action(MobileAction),
}

#[derive(Debug, Clone, Copy)]
struct MobileHitRegion {
    kind: MobileControlKind,
    center: Vec2,
    visual_size: Vec2,

    size: Vec2,
}

impl MobileHitRegion {
    fn contains(self, position: Vec2) -> bool {
        let half = self.size * 0.5;
        match self.kind {
            MobileControlKind::Stick | MobileControlKind::Action(MobileAction::Pause) => {
                let delta = (position - self.center).abs();
                delta.x <= half.x && delta.y <= half.y
            }
            MobileControlKind::Radar | MobileControlKind::Action(_) => {
                let radius = half.min_element();
                position.distance_squared(self.center) <= radius * radius
            }
        }
    }

    fn contains_visual_box(self, position: Vec2) -> bool {
        let half = self.visual_size * 0.5;
        let delta = (position - self.center).abs();
        delta.x <= half.x && delta.y <= half.y
    }
}

#[derive(Resource, Debug, Default)]
pub(crate) struct ResolvedMobileLayout {
    viewport: Vec2,
    safe_area: SafeAreaInsets,
    regions: Vec<MobileHitRegion>,
    stick_center: Vec2,
    stick_max_distance: f32,
}

impl ResolvedMobileLayout {
    pub(crate) fn radar_geometry(&self, viewport: Vec2) -> Option<(Vec2, f32)> {
        if !self.viewport.abs_diff_eq(viewport, 0.01) {
            return None;
        }
        self.region(MobileControlKind::Radar)
            .map(|region| (region.center, region.visual_size.x))
    }

    fn region(&self, kind: MobileControlKind) -> Option<MobileHitRegion> {
        self.regions
            .iter()
            .find(|region| region.kind == kind)
            .copied()
    }

    fn control_at(&self, position: Vec2) -> Option<MobileControlKind> {
        self.regions
            .iter()
            .rev()
            .find(|region| region.kind != MobileControlKind::Radar && region.contains(position))
            .map(|region| region.kind)
    }
}

#[derive(Component)]
struct MobileStickNode;

#[derive(Component)]
struct MobileCommandIndicator(MobileDirection);

#[derive(Component, Clone, Copy)]
struct MobileBaseSize {
    font: f32,
    layout_key: Option<MobileLayoutKey>,
}

#[derive(Component)]
struct MobileStickKnob {
    half_size: f32,
}

#[derive(Component)]
struct MobileControlsRoot;

#[derive(Debug, Clone, Copy)]
enum MobilePointer {
    Stick {
        center: Vec2,
        max_distance: f32,
        floating: bool,
    },
    Action(Option<MobileAction>),
    Look {
        last: Vec2,
    },

    Ignored,
}

#[derive(Resource, Default)]
struct MobileTouchState {
    pointers: HashMap<u64, MobilePointer>,
    stick_pointer: Option<u64>,
    stick_directions: HashSet<MobileDirection>,
    command_direction: Option<MobileDirection>,
    action_counts: HashMap<MobileAction, u8>,
    stick_visual: Vec2,

    floating_stick_center: Option<Vec2>,
    last_tap: [f64; 4],
    tap_armed: [bool; 4],
}

#[derive(Resource, Default, Debug)]
pub(crate) struct MobileLookDelta(pub Vec2);

pub struct MobileControlsPlugin;

impl Plugin for MobileControlsPlugin {
    fn build(&self, app: &mut App) {
        let config = MobileControlsConfig::resolve(app.world().resource::<Settings>());
        app.insert_resource(config)
            .init_resource::<MobileControlLayout>()
            .init_resource::<MobileLayoutEditorState>()
            .init_resource::<ResolvedMobileLayout>()
            .init_resource::<MobileTouchState>()
            .init_resource::<MobileLookDelta>()
            .init_resource::<SyntheticInputStream>()
            .add_systems(
                OnEnter(AppState::Playing),
                (
                    setup_mobile_controls.run_if(mobile_controls_visible),
                    show_mobile_controls,
                )
                    .chain(),
            )
            .add_systems(
                OnExit(AppState::Playing),
                (
                    release_all_mobile_inputs,
                    hide_mobile_controls,
                    despawn_mobile_controls.run_if(crate::plugins::ends_playing_session),
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (update_resolved_mobile_layout, mobile_touch_input)
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .before(crate::plugins::input_plugin::capture_local_command_input)
                    .run_if(super::loading_ui::input_ready)
                    .run_if(in_state(AppState::Playing))
                    .run_if(mobile_controls_visible),
            )
            .add_systems(
                Update,
                (
                    adapt_mobile_control_sizes,
                    update_mobile_stick_visual,
                    update_mobile_action_visuals,
                )
                    .chain()
                    .run_if(in_state(AppState::Playing))
                    .run_if(mobile_controls_visible),
            )
            .add_systems(
                Update,
                mobile_layout_editor_touch.in_set(crate::plugins::UiTouchGestureSet),
            )
            .add_systems(
                Update,
                open_mobile_layout_editor.in_set(crate::plugins::UiTouchActionSet),
            )
            .add_systems(
                Update,
                (
                    mobile_layout_editor_actions,
                    mobile_layout_editor_escape,
                    refresh_mobile_layout_editor,
                    close_mobile_layout_editor_outside_settings,
                )
                    .chain()
                    .in_set(crate::plugins::UiTouchActionSet)
                    .run_if(mobile_layout_editor_open),
            )
            .add_systems(Update, sync_touch_controls_visibility);
    }
}

fn mobile_controls_visible(config: Res<MobileControlsConfig>) -> bool {
    config.visible
}

fn mobile_control_size_factor(width: f32, height: f32) -> f32 {
    (width.min(height) / 720.0).clamp(0.7, 1.2)
}

fn layout_px(target_logical: f32, ui_scale: f32) -> f32 {
    target_logical / ui_scale.max(0.05)
}

#[cfg(test)]
fn clamped_anchor(
    item: MobileLayoutItem,
    half_logical: f32,
    width: f32,
    height: f32,
) -> (f32, f32) {
    let clamp_axis = |v: f32, half: f32, total: f32| {
        let margin = if total > 0.0 {
            (100.0 * half / total).min(45.0)
        } else {
            5.0
        };
        v.clamp(margin, 100.0 - margin)
    };
    (
        clamp_axis(item.x, half_logical, width),
        clamp_axis(item.y, half_logical, height),
    )
}

fn target_size(base: f32, layout_scale: f32, size_factor: f32, min: f32) -> f32 {
    (base * layout_scale * size_factor).max(min)
}

#[cfg(test)]
fn resolve_mobile_layout(
    layout: &MobileControlLayout,
    width: f32,
    height: f32,
) -> ResolvedMobileLayout {
    resolve_mobile_layout_with_safe_area(layout, width, height, SafeAreaInsets::default())
}

fn resolve_mobile_layout_with_safe_area(
    layout: &MobileControlLayout,
    width: f32,
    height: f32,
    safe_area: SafeAreaInsets,
) -> ResolvedMobileLayout {
    let viewport = Vec2::new(width.max(1.0), height.max(1.0));
    let safe = safe_area.sanitized(viewport.x, viewport.y);
    let factor = mobile_control_size_factor(viewport.x, viewport.y);
    let mut regions = Vec::with_capacity(ACTION_CONTROLS.len() + 3);
    let safe_center = |item: MobileLayoutItem, half_size: f32| {
        let raw = Vec2::new(item.x / 100.0 * viewport.x, item.y / 100.0 * viewport.y);
        let min = Vec2::new(safe.left + half_size, safe.top + half_size);
        let max = Vec2::new(
            (viewport.x - safe.right - half_size).max(min.x),
            (viewport.y - safe.bottom - half_size).max(min.y),
        );
        raw.clamp(min, max)
    };

    let stick_item = layout.0[&MobileLayoutKey::Stick];
    let stick_size = target_size(
        STICK_BASE_SIZE,
        stick_item.scale,
        factor,
        MIN_TOUCH_TARGET * 2.0,
    );
    let stick_center = safe_center(stick_item, stick_size * 0.5);
    regions.push(MobileHitRegion {
        kind: MobileControlKind::Stick,
        center: stick_center,
        visual_size: Vec2::splat(stick_size),
        size: Vec2::splat(stick_size),
    });

    let hud = super::combat_hud_layout::CombatHudLayout::new(viewport.x, viewport.y, safe, true);
    regions.push(MobileHitRegion {
        kind: MobileControlKind::Action(MobileAction::Pause),
        center: hud.pause_center,
        visual_size: hud.button_size,
        size: hud.button_size,
    });

    for (action, layout_key, _, base_size) in ACTION_CONTROLS {
        let item = layout.0[&layout_key];
        let visual_size = target_size(base_size, item.scale, factor, MIN_TOUCH_TARGET);
        regions.push(MobileHitRegion {
            kind: MobileControlKind::Action(action),
            center: safe_center(item, visual_size * 0.5),
            visual_size: Vec2::splat(visual_size),
            size: Vec2::splat(visual_size + ACTION_HIT_SLOP * 2.0),
        });
    }

    let radar = layout.0[&MobileLayoutKey::Radar];
    let diameter = (hud.map_diameter * radar.scale).max(MIN_TOUCH_TARGET);
    regions.push(MobileHitRegion {
        kind: MobileControlKind::Radar,
        center: safe_center(radar, diameter * 0.5),
        visual_size: Vec2::splat(diameter),
        size: Vec2::splat(diameter),
    });

    ResolvedMobileLayout {
        viewport,
        safe_area: safe,
        regions,
        stick_center,
        stick_max_distance: stick_size * 0.32,
    }
}

fn update_resolved_mobile_layout(
    windows: Query<Ref<Window>>,
    layout: Res<MobileControlLayout>,
    safe_area: Option<Res<SafeAreaInsets>>,
    mut resolved: ResMut<ResolvedMobileLayout>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let viewport = Vec2::new(window.width().max(1.0), window.height().max(1.0));
    let safe_resource = safe_area.as_deref().copied().unwrap_or_default();
    let safe = safe_resource.sanitized(viewport.x, viewport.y);
    if !layout.is_changed()
        && !safe_area
            .as_ref()
            .is_some_and(|safe_area| safe_area.is_changed())
        && resolved.viewport == viewport
        && resolved.safe_area == safe
        && !resolved.regions.is_empty()
    {
        return;
    }
    *resolved = resolve_mobile_layout_with_safe_area(&layout, viewport.x, viewport.y, safe);
}

fn control_node(center: Vec2, size: Vec2, ui_scale: f32) -> Node {
    let unit = ui_scale.max(0.05);
    let layout_size = size / unit;
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(center.x / unit),
        top: Val::Px(center.y / unit),
        width: Val::Px(layout_size.x),
        height: Val::Px(layout_size.y),
        margin: UiRect::new(
            Val::Px(-layout_size.x * 0.5),
            Val::ZERO,
            Val::Px(-layout_size.y * 0.5),
            Val::ZERO,
        ),
        border: UiRect::all(Val::Px(2.0)),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    }
}

const ACTION_CONTROLS: [(MobileAction, MobileLayoutKey, &str, f32); 11] = [
    (
        MobileAction::SwitchWeapon,
        MobileLayoutKey::SwitchWeapon,
        "换",
        58.0,
    ),
    (
        MobileAction::DropWeapon,
        MobileLayoutKey::DropWeapon,
        "丢武器",
        56.0,
    ),
    (
        MobileAction::DropItem,
        MobileLayoutKey::DropItem,
        "丢道具",
        56.0,
    ),
    (MobileAction::Fake, MobileLayoutKey::Fake, "假死", 58.0),
    (
        MobileAction::UnlockCamera,
        MobileLayoutKey::UnlockCamera,
        "解锁",
        60.0,
    ),
    (MobileAction::QiBurst, MobileLayoutKey::QiBurst, "爆", 62.0),
    (MobileAction::Cure, MobileLayoutKey::Cure, "救人", 56.0),
    (MobileAction::Crouch, MobileLayoutKey::Crouch, "蹲", 58.0),
    (MobileAction::Guard, MobileLayoutKey::Guard, "防", 64.0),
    (MobileAction::Jump, MobileLayoutKey::Jump, "跃", 68.0),
    (MobileAction::Attack, MobileLayoutKey::Attack, "攻", 86.0),
];

const STICK_BASE_SIZE: f32 = 150.0;
const KNOB_BASE_SIZE: f32 = 54.0;
const PAUSE_BASE_FONT: f32 = 13.0;
const ACTION_HIT_SLOP: f32 = 6.0;

fn action_base_font(label: &str) -> f32 {
    if label.chars().count() > 1 {
        15.0
    } else {
        22.0
    }
}

fn setup_mobile_controls(
    mut commands: Commands,
    font: Res<UiFont>,
    layout: Res<MobileControlLayout>,
    windows: Query<&Window>,
    ui_scale: Res<EffectiveUiScale>,
    safe_area: Option<Res<SafeAreaInsets>>,
    roots: Query<Entity, With<MobileControlsRoot>>,
) {
    if !roots.is_empty() {
        return;
    }
    let (width, height) = windows
        .single()
        .map(|window| (window.width(), window.height()))
        .unwrap_or((1280.0, 720.0));
    info!(
        target: "meteor::mobile",
        "controls event=spawn viewport=({width:.1},{height:.1})"
    );
    spawn_mobile_controls(
        &mut commands,
        &font.0,
        &layout,
        width,
        height,
        ui_scale.0,
        safe_area.as_deref().copied().unwrap_or_default(),
    );
}

fn spawn_mobile_controls(
    commands: &mut Commands,
    font: &Handle<Font>,
    layout: &MobileControlLayout,
    width: f32,
    height: f32,
    ui_scale: f32,
    safe_area: SafeAreaInsets,
) {
    let factor = mobile_control_size_factor(width, height);
    let resolved = resolve_mobile_layout_with_safe_area(layout, width, height, safe_area);
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            bevy::ui::FocusPolicy::Pass,
            Visibility::Visible,
            ZIndex(80),
            MobileControlsRoot,
        ))
        .id();

    let stick_layout = layout.0[&MobileLayoutKey::Stick];
    let stick_region = resolved
        .region(MobileControlKind::Stick)
        .expect("resolved stick region");
    let stick = commands
        .spawn((
            Node {
                border_radius: BorderRadius::all(Val::Percent(50.0)),
                ..control_node(stick_region.center, stick_region.visual_size, ui_scale)
            },
            BorderColor::all(Color::srgba(0.72, 0.80, 0.82, 0.50)),
            BackgroundColor(Color::srgba(0.04, 0.06, 0.08, 0.46)),
            MobileControlKind::Stick,
            MobileStickNode,
            MobileBaseSize {
                font: 0.0,
                layout_key: Some(MobileLayoutKey::Stick),
            },
        ))
        .id();
    let knob_target = KNOB_BASE_SIZE * stick_layout.scale * factor;
    let knob_px = layout_px(knob_target, ui_scale);
    let knob = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                width: Val::Px(knob_px),
                height: Val::Px(knob_px),
                margin: UiRect::new(
                    Val::Px(-knob_px * 0.5),
                    Val::ZERO,
                    Val::Px(-knob_px * 0.5),
                    Val::ZERO,
                ),
                border_radius: BorderRadius::all(Val::Percent(50.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.72, 0.82, 0.84, 0.64)),
            MobileStickKnob {
                half_size: knob_px * 0.5,
            },
        ))
        .id();
    commands.entity(stick).add_child(knob);
    for (direction, left, top, width, height) in [
        (MobileDirection::Forward, 43.0, 5.0, 14.0, 6.0),
        (MobileDirection::Backward, 43.0, 89.0, 14.0, 6.0),
        (MobileDirection::Left, 5.0, 43.0, 6.0, 14.0),
        (MobileDirection::Right, 89.0, 43.0, 6.0, 14.0),
    ] {
        let indicator = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(left),
                    top: Val::Percent(top),
                    width: Val::Percent(width),
                    height: Val::Percent(height),
                    border_radius: BorderRadius::all(Val::Percent(50.0)),
                    ..default()
                },
                BackgroundColor(Color::NONE),
                MobileCommandIndicator(direction),
            ))
            .id();
        commands.entity(stick).add_child(indicator);
    }
    commands.entity(root).add_child(stick);

    let pause_region = resolved
        .region(MobileControlKind::Action(MobileAction::Pause))
        .expect("resolved pause region");
    let pause = commands
        .spawn((
            Node {
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..control_node(pause_region.center, pause_region.visual_size, ui_scale)
            },
            BorderColor::all(theme::LINE),
            BackgroundColor(theme::HUD_SURFACE),
            MobileControlKind::Action(MobileAction::Pause),
            MobileAction::Pause,
            MobileBaseSize {
                font: PAUSE_BASE_FONT,
                layout_key: None,
            },
        ))
        .id();
    let pause_text = commands
        .spawn((
            Text::new("Ⅱ"),
            TextFont {
                font: font.clone().into(),
                font_size: layout_px(PAUSE_BASE_FONT * factor, ui_scale).into(),
                ..default()
            },
            TextColor(Color::WHITE),
        ))
        .id();
    commands.entity(pause).add_child(pause_text);
    commands.entity(root).add_child(pause);

    for (action, layout_key, label, _) in ACTION_CONTROLS {
        let item = layout.0[&layout_key];
        let region = resolved
            .region(MobileControlKind::Action(action))
            .expect("resolved action region");
        let button = commands
            .spawn((
                Node {
                    border_radius: BorderRadius::all(Val::Percent(50.0)),
                    ..control_node(region.center, region.visual_size, ui_scale)
                },
                BorderColor::all(theme::LINE),
                BackgroundColor(theme::HUD_SURFACE),
                MobileControlKind::Action(action),
                action,
                MobileBaseSize {
                    font: action_base_font(label),
                    layout_key: Some(layout_key),
                },
            ))
            .id();
        let text = commands
            .spawn((
                Text::new(label),
                TextFont {
                    font: font.clone().into(),
                    font_size: layout_px(action_base_font(label) * item.scale * factor, ui_scale)
                        .into(),
                    ..default()
                },
                TextColor(theme::TEXT),
            ))
            .id();
        commands.entity(button).add_child(text);
        commands.entity(root).add_child(button);
    }
}

#[allow(clippy::too_many_arguments)]
fn adapt_mobile_control_sizes(
    resolved: Res<ResolvedMobileLayout>,
    ui_scale: Res<EffectiveUiScale>,
    layout: Res<MobileControlLayout>,
    mut controls: Query<(&MobileBaseSize, &MobileControlKind, &mut Node)>,
    mut knobs: Query<(&mut MobileStickKnob, &mut Node), Without<MobileControlKind>>,
    mut texts: Query<(&ChildOf, &mut TextFont)>,
) {
    if resolved.regions.is_empty() {
        return;
    }
    if !resolved.is_changed() && !ui_scale.is_changed() && !layout.is_changed() {
        return;
    }
    let unit = ui_scale.0.max(0.05);
    let factor = mobile_control_size_factor(resolved.viewport.x, resolved.viewport.y);
    for (_, kind, mut node) in &mut controls {
        let Some(region) = resolved.region(*kind) else {
            continue;
        };
        let px_w = region.visual_size.x / unit;
        let px_h = region.visual_size.y / unit;
        node.width = Val::Px(px_w);
        node.height = Val::Px(px_h);
        node.margin = UiRect::new(
            Val::Px(-px_w * 0.5),
            Val::ZERO,
            Val::Px(-px_h * 0.5),
            Val::ZERO,
        );
        match kind {
            MobileControlKind::Stick => {}
            MobileControlKind::Radar | MobileControlKind::Action(_) => {
                node.left = Val::Px(region.center.x / unit);
                node.top = Val::Px(region.center.y / unit);
            }
        }
    }
    for (mut knob, mut node) in &mut knobs {
        let scale = layout.0[&MobileLayoutKey::Stick].scale;
        let px = layout_px(KNOB_BASE_SIZE * scale * factor, ui_scale.0);
        node.width = Val::Px(px);
        node.height = Val::Px(px);
        knob.half_size = px * 0.5;
    }
    for (parent, mut text_font) in &mut texts {
        let Ok((base, _, _)) = controls.get(parent.parent()) else {
            continue;
        };
        if base.font > 0.0 {
            let scale = base
                .layout_key
                .map(|key| layout.0[&key].scale)
                .unwrap_or(1.0);
            text_font.font_size = layout_px(base.font * scale * factor, ui_scale.0).into();
        }
    }
}

fn binding_for_direction(direction: MobileDirection, keymap: &Keymap) -> InputBinding {
    match direction {
        MobileDirection::Forward => keymap.move_up,
        MobileDirection::Backward => keymap.move_down,
        MobileDirection::Left => keymap.move_left,
        MobileDirection::Right => keymap.move_right,
    }
}

fn binding_for_action(action: MobileAction, keymap: &Keymap) -> InputBinding {
    match action {
        MobileAction::Pause => InputBinding::Key(KeyCode::Escape),
        MobileAction::Attack => keymap.attack,
        MobileAction::Jump => keymap.jump,
        MobileAction::Guard => keymap.guard,
        MobileAction::Crouch => keymap.crouch,
        MobileAction::SwitchWeapon => keymap.weapon_switch,
        MobileAction::Fake => keymap.fake,
        MobileAction::UnlockCamera => keymap.lock_toggle,
        MobileAction::QiBurst => keymap.qi_burst,
        MobileAction::Cure => keymap.cure,
        MobileAction::DropWeapon => keymap.drop_weapon,
        MobileAction::DropItem => keymap.drop_item,
    }
}

fn set_binding(
    keys: &mut ButtonInput<KeyCode>,
    mouse: &mut ButtonInput<MouseButton>,
    binding: InputBinding,
    pressed: bool,
) {
    match (binding, pressed) {
        (InputBinding::Key(key), true) => keys.press(key),
        (InputBinding::Key(key), false) => keys.release(key),
        (InputBinding::Mouse(button), true) => mouse.press(button),
        (InputBinding::Mouse(button), false) => mouse.release(button),
    }
}

fn cancel_binding(
    keys: &mut ButtonInput<KeyCode>,
    mouse: &mut ButtonInput<MouseButton>,
    binding: InputBinding,
) {
    match binding {
        InputBinding::Key(key) => keys.reset(key),
        InputBinding::Mouse(button) => mouse.reset(button),
    }
}

fn stick_directions(delta: Vec2, max_distance: f32) -> HashSet<MobileDirection> {
    let distance = max_distance.max(1.0);
    let normalized = delta / distance;
    let mut directions = HashSet::new();
    if normalized.y < -STICK_THRESHOLD {
        directions.insert(MobileDirection::Forward);
    }
    if normalized.y > STICK_THRESHOLD {
        directions.insert(MobileDirection::Backward);
    }
    if normalized.x < -STICK_THRESHOLD {
        directions.insert(MobileDirection::Left);
    }
    if normalized.x > STICK_THRESHOLD {
        directions.insert(MobileDirection::Right);
    }
    directions
}

fn stick_command_direction(
    delta: Vec2,
    max_distance: f32,
    previous: Option<MobileDirection>,
) -> Option<MobileDirection> {
    let normalized = delta / max_distance.max(1.0);
    let threshold = if previous.is_some() {
        COMMAND_RELEASE_THRESHOLD
    } else {
        COMMAND_PRESS_THRESHOLD
    };
    if normalized.length_squared() <= threshold * threshold {
        return None;
    }
    let x = normalized.x.abs();
    let y = normalized.y.abs();
    let vertical = match previous {
        Some(MobileDirection::Forward | MobileDirection::Backward) => {
            x <= y * COMMAND_AXIS_SWITCH_RATIO
        }
        Some(MobileDirection::Left | MobileDirection::Right) => y > x * COMMAND_AXIS_SWITCH_RATIO,
        None => y >= x,
    };
    Some(if vertical {
        if normalized.y < 0.0 {
            MobileDirection::Forward
        } else {
            MobileDirection::Backward
        }
    } else if normalized.x < 0.0 {
        MobileDirection::Left
    } else {
        MobileDirection::Right
    })
}

#[allow(clippy::too_many_arguments)]
fn set_stick_input(
    delta: Vec2,
    max_distance: f32,
    now: f64,
    state: &mut MobileTouchState,
    keymap: &Keymap,
    keys: &mut ButtonInput<KeyCode>,
    mouse: &mut ButtonInput<MouseButton>,
    synthetic: &mut SyntheticInputStream,
) {
    let next = stick_directions(delta, max_distance);
    for direction in MobileDirection::ALL {
        let was_held = state.stick_directions.contains(&direction);
        let is_held = next.contains(&direction);
        if was_held == is_held {
            continue;
        }
        let binding = binding_for_direction(direction, keymap);
        set_binding(keys, mouse, binding, is_held);
        synthetic.push_mobile_movement(binding, is_held);
    }
    state.stick_directions = next;

    let next_command = stick_command_direction(delta, max_distance, state.command_direction);
    if next_command == state.command_direction {
        return;
    }

    if let Some(previous) = state.command_direction {
        synthetic.push_mobile_direction(binding_for_direction(previous, keymap), false);
        state.tap_armed[previous.index()] = false;
    }
    if let Some(direction) = next_command {
        synthetic.push_mobile_direction(binding_for_direction(direction, keymap), true);
        let index = direction.index();
        if !state.tap_armed[index] {
            state.tap_armed[index] = true;
            let previous = state.last_tap[index];
            if previous > 0.0 && now - previous < DODGE_TAP_WINDOW {
                let burst = state
                    .action_counts
                    .get(&MobileAction::Guard)
                    .copied()
                    .unwrap_or_default()
                    > 0;
                synthetic.push_dodge(direction.command_direction(), burst);
                state.last_tap[index] = 0.0;
            } else {
                state.last_tap[index] = now;
            }
        }
    }
    state.command_direction = next_command;
}

fn press_action(
    action: MobileAction,
    state: &mut MobileTouchState,
    keymap: &Keymap,
    keys: &mut ButtonInput<KeyCode>,
    mouse: &mut ButtonInput<MouseButton>,
    synthetic: &mut SyntheticInputStream,
) {
    let count = state.action_counts.entry(action).or_default();
    *count = count.saturating_add(1);
    if *count == 1 {
        let binding = binding_for_action(action, keymap);
        set_binding(keys, mouse, binding, true);
        if let Some(action) = action.synthetic_action() {
            synthetic.push_action(binding, action, true, None);
        } else {
            synthetic.push_live_binding(binding, true);
        }
    }
}

fn release_action(
    action: MobileAction,
    canceled: bool,
    state: &mut MobileTouchState,
    keymap: &Keymap,
    keys: &mut ButtonInput<KeyCode>,
    mouse: &mut ButtonInput<MouseButton>,
    synthetic: &mut SyntheticInputStream,
) {
    let Some(count) = state.action_counts.get_mut(&action) else {
        return;
    };
    *count = count.saturating_sub(1);
    if *count > 0 {
        return;
    }
    state.action_counts.remove(&action);
    let binding = binding_for_action(action, keymap);
    if canceled && action == MobileAction::Jump {
        cancel_binding(keys, mouse, binding);
        synthetic.cancel_binding(binding);
    } else {
        set_binding(keys, mouse, binding, false);
        if let Some(action) = action.synthetic_action() {
            synthetic.push_action(binding, action, false, None);
        } else {
            synthetic.push_live_binding(binding, false);
        }
    }
}

fn moved_action_at(
    resolved: &ResolvedMobileLayout,
    previous: Option<MobileAction>,
    position: Vec2,
) -> Option<MobileAction> {
    if let Some(previous) = previous {
        let current = MobileControlKind::Action(previous);
        if resolved
            .region(current)
            .is_some_and(|region| region.contains_visual_box(position))
        {
            return Some(previous);
        }
    }
    match resolved.control_at(position) {
        Some(MobileControlKind::Action(action)) => Some(action),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn mobile_touch_input(
    time: Res<Time<bevy::time::Real>>,
    mut events: MessageReader<TouchInput>,
    resolved: Res<ResolvedMobileLayout>,
    keymap: Res<Keymap>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut state: ResMut<MobileTouchState>,
    mut look: ResMut<MobileLookDelta>,
    mut synthetic: ResMut<SyntheticInputStream>,
    debug_ui: Option<Res<crate::plugins::debug_plugin::DebugConsoleState>>,
    motion_log: Option<Res<crate::plugins::debug_plugin::DebugMotionLog>>,
    radar: Option<Res<super::ui_plugin::RadarState>>,
    mut command: Option<ResMut<LocalCommandInput>>,
) {
    if debug_ui.is_some_and(|debug| debug.captures_input()) {
        events.read().for_each(drop);
        release_all_mobile_inputs_inner(
            &keymap,
            &mut keys,
            &mut mouse,
            &mut state,
            &mut look,
            &mut synthetic,
        );

        synthetic.clear();
        if let Some(command) = command.as_deref_mut() {
            command.0.clear();
        }
        return;
    }
    let now = time.elapsed_secs_f64();
    for event in events.read() {
        let motion_enabled = motion_log
            .as_deref()
            .is_some_and(crate::plugins::debug_plugin::DebugMotionLog::is_enabled);
        let actions_before = motion_enabled.then(|| state.action_counts.clone());
        if event.phase == TouchPhase::Started
            && radar
                .as_ref()
                .is_some_and(|radar| radar.contains_point(event.position))
        {
            state.pointers.insert(event.id, MobilePointer::Ignored);
            continue;
        }
        match event.phase {
            TouchPhase::Started => match resolved.control_at(event.position) {
                Some(MobileControlKind::Stick) if state.stick_pointer.is_none() => {
                    let center = resolved.stick_center;
                    let max_distance = resolved.stick_max_distance.max(1.0);
                    state.stick_pointer = Some(event.id);
                    state.pointers.insert(
                        event.id,
                        MobilePointer::Stick {
                            center,
                            max_distance,
                            floating: false,
                        },
                    );
                    let delta = event.position - center;
                    state.stick_visual = delta.clamp_length_max(max_distance);
                    set_stick_input(
                        delta,
                        max_distance,
                        now,
                        &mut state,
                        &keymap,
                        &mut keys,
                        &mut mouse,
                        &mut synthetic,
                    );
                }
                Some(MobileControlKind::Action(action)) => {
                    press_action(
                        action,
                        &mut state,
                        &keymap,
                        &mut keys,
                        &mut mouse,
                        &mut synthetic,
                    );
                    state
                        .pointers
                        .insert(event.id, MobilePointer::Action(Some(action)));
                }
                _ => {
                    let window_width = resolved.viewport.x;
                    let floating_zone = window_width > 0.0
                        && event.position.x < window_width * FLOATING_STICK_ZONE_X;
                    if floating_zone && state.stick_pointer.is_none() {
                        let max_distance = resolved.stick_max_distance.max(1.0);
                        state.stick_pointer = Some(event.id);
                        state.floating_stick_center = Some(event.position);
                        state.pointers.insert(
                            event.id,
                            MobilePointer::Stick {
                                center: event.position,
                                max_distance,
                                floating: true,
                            },
                        );
                        state.stick_visual = Vec2::ZERO;
                    } else if floating_zone {
                        state.pointers.insert(event.id, MobilePointer::Ignored);
                    } else {
                        state.pointers.insert(
                            event.id,
                            MobilePointer::Look {
                                last: event.position,
                            },
                        );
                    }
                }
            },
            TouchPhase::Moved => {
                let Some(pointer) = state.pointers.get(&event.id).copied() else {
                    continue;
                };
                match pointer {
                    MobilePointer::Stick {
                        center,
                        max_distance,
                        ..
                    } => {
                        let delta = event.position - center;
                        state.stick_visual = delta.clamp_length_max(max_distance);
                        set_stick_input(
                            delta,
                            max_distance,
                            now,
                            &mut state,
                            &keymap,
                            &mut keys,
                            &mut mouse,
                            &mut synthetic,
                        );
                    }
                    MobilePointer::Action(previous) => {
                        let next = moved_action_at(&resolved, previous, event.position);
                        if next != previous {
                            if let Some(previous) = previous {
                                release_action(
                                    previous,
                                    false,
                                    &mut state,
                                    &keymap,
                                    &mut keys,
                                    &mut mouse,
                                    &mut synthetic,
                                );
                            }
                            if let Some(next) = next {
                                press_action(
                                    next,
                                    &mut state,
                                    &keymap,
                                    &mut keys,
                                    &mut mouse,
                                    &mut synthetic,
                                );
                            }
                            state.pointers.insert(event.id, MobilePointer::Action(next));
                        }
                    }
                    MobilePointer::Look { last } => {
                        look.0 += event.position - last;
                        state.pointers.insert(
                            event.id,
                            MobilePointer::Look {
                                last: event.position,
                            },
                        );
                    }
                    MobilePointer::Ignored => {}
                }
            }
            TouchPhase::Ended | TouchPhase::Canceled => {
                let canceled = event.phase == TouchPhase::Canceled;
                let Some(pointer) = state.pointers.remove(&event.id) else {
                    continue;
                };
                match pointer {
                    MobilePointer::Stick { floating, .. } => {
                        state.stick_pointer = None;
                        state.stick_visual = Vec2::ZERO;
                        if floating {
                            state.floating_stick_center = None;
                        }
                        set_stick_input(
                            Vec2::ZERO,
                            1.0,
                            now,
                            &mut state,
                            &keymap,
                            &mut keys,
                            &mut mouse,
                            &mut synthetic,
                        );
                    }
                    MobilePointer::Action(Some(action)) => release_action(
                        action,
                        canceled,
                        &mut state,
                        &keymap,
                        &mut keys,
                        &mut mouse,
                        &mut synthetic,
                    ),
                    MobilePointer::Action(None)
                    | MobilePointer::Look { .. }
                    | MobilePointer::Ignored => {}
                }
            }
        }
        if let Some(actions_before) = actions_before {
            for action in MobileAction::ALL {
                let before = actions_before.get(&action).copied().unwrap_or_default();
                let after = state
                    .action_counts
                    .get(&action)
                    .copied()
                    .unwrap_or_default();
                if before == after {
                    continue;
                }
                if action == MobileAction::Jump {
                    info!(
                        target: "meteor::motion",
                        "air event=mobile-jump action={:?} touch={} phase={:?} pos=({:.1},{:.1}) touches={}->{} stick(F={},B={},L={},R={}) pointers={}",
                        action,
                        event.id,
                        event.phase,
                        event.position.x,
                        event.position.y,
                        before,
                        after,
                        state.stick_directions.contains(&MobileDirection::Forward),
                        state.stick_directions.contains(&MobileDirection::Backward),
                        state.stick_directions.contains(&MobileDirection::Left),
                        state.stick_directions.contains(&MobileDirection::Right),
                        state.pointers.len(),
                    );
                } else {
                    info!(
                        target: "meteor::motion",
                        "mobile event=action action={:?} touch={} phase={:?} pos=({:.1},{:.1}) touches={}->{} pointers={}",
                        action,
                        event.id,
                        event.phase,
                        event.position.x,
                        event.position.y,
                        before,
                        after,
                        state.pointers.len(),
                    );
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn release_all_mobile_inputs_inner(
    keymap: &Keymap,
    keys: &mut ButtonInput<KeyCode>,
    mouse: &mut ButtonInput<MouseButton>,
    state: &mut MobileTouchState,
    look: &mut MobileLookDelta,
    synthetic: &mut SyntheticInputStream,
) {
    let command_direction = state.command_direction;
    for direction in state.stick_directions.clone() {
        set_binding(keys, mouse, binding_for_direction(direction, keymap), false);
    }
    let actions: Vec<MobileAction> = state.action_counts.keys().copied().collect();
    for action in actions {
        state.action_counts.insert(action, 1);
        release_action(
            action,
            action == MobileAction::Jump,
            state,
            keymap,
            keys,
            mouse,
            synthetic,
        );
    }
    *state = MobileTouchState::default();
    look.0 = Vec2::ZERO;
    synthetic.clear();

    if let Some(direction) = command_direction {
        synthetic.push_mobile_direction(binding_for_direction(direction, keymap), false);
    }
}

fn release_all_mobile_inputs(
    keymap: Res<Keymap>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut state: ResMut<MobileTouchState>,
    mut look: ResMut<MobileLookDelta>,
    mut synthetic: ResMut<SyntheticInputStream>,
) {
    release_all_mobile_inputs_inner(
        &keymap,
        &mut keys,
        &mut mouse,
        &mut state,
        &mut look,
        &mut synthetic,
    );
}

fn show_mobile_controls(
    config: Res<MobileControlsConfig>,
    mut roots: Query<&mut Visibility, With<MobileControlsRoot>>,
) {
    let visibility = if config.visible {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    let mut count = 0;
    for mut current in &mut roots {
        *current = visibility;
        count += 1;
    }
    if config.visible || count > 0 {
        info!(
            target: "meteor::mobile",
            "controls event=show visible={} roots={count}",
            config.visible
        );
    }
}

fn hide_mobile_controls(mut roots: Query<&mut Visibility, With<MobileControlsRoot>>) {
    let mut count = 0;
    for mut visibility in &mut roots {
        *visibility = Visibility::Hidden;
        count += 1;
    }
    if count > 0 {
        info!(target: "meteor::mobile", "controls event=hide roots={count}");
    }
}

fn despawn_mobile_controls(mut commands: Commands, roots: Query<Entity, With<MobileControlsRoot>>) {
    let mut count = 0;
    for entity in &roots {
        commands.entity(entity).despawn();
        count += 1;
    }
    if count > 0 {
        info!(
            target: "meteor::mobile",
            "controls event=despawn roots={count}"
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn sync_touch_controls_visibility(
    settings: Res<Settings>,
    mut config: ResMut<MobileControlsConfig>,
    mut commands: Commands,
    roots: Query<Entity, With<MobileControlsRoot>>,
    font: Option<Res<UiFont>>,
    layout: Res<MobileControlLayout>,
    windows: Query<&Window>,
    ui_scale: Res<EffectiveUiScale>,
    safe_area: Option<Res<SafeAreaInsets>>,
    app_state: Res<State<AppState>>,
    keymap: Res<Keymap>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut touch_state: ResMut<MobileTouchState>,
    mut look: ResMut<MobileLookDelta>,
    mut synthetic: ResMut<SyntheticInputStream>,
) {
    if !settings.is_changed() || settings.is_added() {
        return;
    }
    let visible = MobileControlsConfig::resolve(&settings).visible;
    if visible == config.visible {
        return;
    }
    config.visible = visible;
    if visible {
        if *app_state.get() == AppState::Playing && roots.is_empty() {
            if let (Some(font), Ok(window)) = (font, windows.single()) {
                spawn_mobile_controls(
                    &mut commands,
                    &font.0,
                    &layout,
                    window.width(),
                    window.height(),
                    ui_scale.0,
                    safe_area.as_deref().copied().unwrap_or_default(),
                );
            }
        } else if *app_state.get() == AppState::Playing {
            for entity in &roots {
                commands.entity(entity).insert(Visibility::Visible);
            }
        }
    } else {
        for entity in &roots {
            commands.entity(entity).despawn();
        }
        release_all_mobile_inputs_inner(
            &keymap,
            &mut keys,
            &mut mouse,
            &mut touch_state,
            &mut look,
            &mut synthetic,
        );
    }
}

fn update_mobile_stick_visual(
    state: Res<MobileTouchState>,
    resolved: Res<ResolvedMobileLayout>,
    ui_scale: Res<EffectiveUiScale>,
    mut sticks: Query<&mut Node, (With<MobileStickNode>, Without<MobileStickKnob>)>,
    mut knobs: Query<(&MobileStickKnob, &mut Node), Without<MobileStickNode>>,
    mut indicators: Query<(&MobileCommandIndicator, &mut BackgroundColor)>,
) {
    let unit = ui_scale.0.max(0.05);
    for mut node in &mut sticks {
        let center = state.floating_stick_center.unwrap_or(resolved.stick_center);
        node.left = Val::Px(center.x / unit);
        node.top = Val::Px(center.y / unit);
    }
    for (knob, mut node) in &mut knobs {
        let offset = state.stick_visual / unit;
        node.margin.left = Val::Px(-knob.half_size + offset.x);
        node.margin.top = Val::Px(-knob.half_size + offset.y);
    }
    for (indicator, mut color) in &mut indicators {
        color.0 = if state.command_direction == Some(indicator.0) {
            Color::srgba(1.0, 0.78, 0.32, 0.95)
        } else {
            Color::srgba(0.72, 0.82, 0.84, 0.24)
        };
    }
}

fn update_mobile_action_visuals(
    state: Res<MobileTouchState>,
    mut buttons: Query<(&MobileAction, &mut BackgroundColor)>,
) {
    for (action, mut color) in &mut buttons {
        color.0 = if state.action_counts.contains_key(action) {
            theme::PRIMARY_HOVER
        } else {
            theme::HUD_SURFACE
        };
    }
}

#[cfg(test)]
pub(super) fn install_touch_input_for_test(app: &mut App) -> (Vec2, f32, Vec2) {
    let resolved = resolve_mobile_layout(&MobileControlLayout::default(), 1000.0, 500.0);
    let geometry = (
        resolved.stick_center,
        resolved.stick_max_distance,
        resolved
            .region(MobileControlKind::Action(MobileAction::Attack))
            .unwrap()
            .center,
    );
    app.init_resource::<MobileTouchState>()
        .init_resource::<MobileLookDelta>()
        .insert_resource(resolved)
        .add_message::<TouchInput>()
        .add_systems(
            Update,
            mobile_touch_input
                .in_set(crate::plugins::FighterFrameSet::Movement)
                .before(crate::plugins::input_plugin::capture_local_command_input),
        );
    geometry
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::state::app::AppExtStates;
    use bevy::window::WindowResolution;

    fn transition_to(app: &mut App, state: AppState) {
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(state);
        app.update();
    }

    fn mobile_lifecycle_test_app() -> (App, Entity) {
        let settings = Settings {
            touch_controls: 1,
            ..default()
        };
        let mut app = App::new();
        crate::plugins::configure_fighter_frame_sets(&mut app);
        app.add_plugins(bevy::state::app::StatesPlugin)
            .init_state::<AppState>()
            .insert_resource(settings)
            .insert_resource(UiFont(Handle::default()))
            .init_resource::<EffectiveUiScale>()
            .init_resource::<Time<bevy::time::Real>>()
            .init_resource::<Keymap>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_message::<TouchInput>()
            .add_plugins(MobileControlsPlugin)
            .add_systems(
                OnTransition {
                    exited: AppState::Paused,
                    entered: AppState::Menu,
                },
                crate::plugins::cleanup_suspended_playing_session,
            );
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: WindowResolution::new(817, 393),
                ..default()
            })
            .id();
        app.update();
        (app, window)
    }

    fn touch_test_app(resolved: ResolvedMobileLayout) -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<Time<bevy::time::Real>>()
            .init_resource::<Keymap>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<MobileTouchState>()
            .init_resource::<MobileLookDelta>()
            .init_resource::<SyntheticInputStream>()
            .insert_resource(resolved)
            .add_message::<TouchInput>()
            .add_systems(Update, mobile_touch_input);
        let window = app.world_mut().spawn_empty().id();
        (app, window)
    }

    #[test]
    fn mobile_session_button_maps_to_escape() {
        assert_eq!(
            binding_for_action(MobileAction::Pause, &Keymap::default()),
            InputBinding::Key(KeyCode::Escape)
        );
    }

    #[test]
    fn mobile_controls_hide_on_pause_and_rebuild_with_live_actions_after_menu_exit() {
        let (mut app, window) = mobile_lifecycle_test_app();
        transition_to(&mut app, AppState::Playing);
        let first_root = app
            .world_mut()
            .query_filtered::<Entity, With<MobileControlsRoot>>()
            .single(app.world())
            .expect("first mobile control root");
        assert_eq!(
            app.world().get::<Visibility>(first_root),
            Some(&Visibility::Visible)
        );

        let attack = app
            .world()
            .resource::<ResolvedMobileLayout>()
            .region(MobileControlKind::Action(MobileAction::Attack))
            .expect("attack region")
            .center;
        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Started,
            position: attack,
            window,
            force: None,
            id: 41,
        });
        app.update();
        {
            let keymap = app.world().resource::<Keymap>();
            let keys = app.world().resource::<ButtonInput<KeyCode>>();
            let mouse = app.world().resource::<ButtonInput<MouseButton>>();
            assert!(keymap.attack.pressed(keys, mouse));
        }

        transition_to(&mut app, AppState::Paused);
        assert_eq!(
            app.world().get::<Visibility>(first_root),
            Some(&Visibility::Hidden),
            "gameplay controls must not cover the pause menu"
        );
        assert!(app
            .world()
            .resource::<MobileTouchState>()
            .pointers
            .is_empty());
        {
            let keymap = app.world().resource::<Keymap>();
            let keys = app.world().resource::<ButtonInput<KeyCode>>();
            let mouse = app.world().resource::<ButtonInput<MouseButton>>();
            assert!(!keymap.attack.pressed(keys, mouse));
        }

        transition_to(&mut app, AppState::Playing);
        assert_eq!(
            app.world().get::<Visibility>(first_root),
            Some(&Visibility::Visible),
            "resume must reveal the preserved control layer"
        );

        transition_to(&mut app, AppState::Paused);
        transition_to(&mut app, AppState::Menu);
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<MobileControlsRoot>>()
                .iter(app.world())
                .count(),
            0
        );

        transition_to(&mut app, AppState::Loading);
        transition_to(&mut app, AppState::Playing);
        let second_root = app
            .world_mut()
            .query_filtered::<Entity, With<MobileControlsRoot>>()
            .single(app.world())
            .expect("second mobile control root");
        assert_ne!(second_root, first_root);
        assert_eq!(
            app.world().get::<Visibility>(second_root),
            Some(&Visibility::Visible)
        );

        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Started,
            position: attack,
            window,
            force: None,
            id: 42,
        });
        app.update();
        let keymap = app.world().resource::<Keymap>();
        let keys = app.world().resource::<ButtonInput<KeyCode>>();
        let mouse = app.world().resource::<ButtonInput<MouseButton>>();
        assert!(
            keymap.attack.pressed(keys, mouse),
            "fresh-session action regions must still drive gameplay input"
        );
    }

    #[test]
    fn radar_is_editable_without_a_standalone_action_button() {
        assert_eq!(
            mobile_layout_control_kind(MobileLayoutKey::Radar),
            MobileControlKind::Radar
        );
        assert!(ACTION_CONTROLS
            .iter()
            .all(|(_, key, _, _)| *key != MobileLayoutKey::Radar));
        let mut saved = default_mobile_layout();
        saved.insert(
            MobileLayoutKey::Radar,
            MobileLayoutItem {
                x: 30.0,
                y: 30.0,
                scale: 1.5,
            },
        );
        let resolved = resolve_mobile_layout(&MobileControlLayout(saved), 900.0, 432.0);
        let (center, diameter) = resolved.radar_geometry(Vec2::new(900.0, 432.0)).unwrap();
        assert!(center.abs_diff_eq(Vec2::new(270.0, 129.6), 0.001));
        assert!((diameter - 432.0 * 0.22 * 1.5).abs() < 0.001);
    }

    #[test]
    fn mobile_drop_buttons_publish_ordered_discrete_actions() {
        assert_eq!(
            MobileAction::DropWeapon.synthetic_action(),
            Some(SyntheticAction::DropWeapon)
        );
        assert_eq!(
            MobileAction::DropItem.synthetic_action(),
            Some(SyntheticAction::DropItem)
        );
    }

    #[test]
    fn mobile_controls_plugin_editor_schedule_initializes_without_query_conflicts() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .insert_resource(Settings::default())
            .insert_resource(Keymap::default())
            .insert_resource(UiFont(Handle::default()))
            .init_resource::<EffectiveUiScale>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_state::<AppState>()
            .add_message::<TouchInput>()
            .add_plugins(MobileControlsPlugin);
        app.update();
        assert!(!app.world().resource::<MobileLayoutEditorState>().open);
    }

    #[test]
    fn stick_threshold_matches_web_axes_and_diagonals() {
        let directions = stick_directions(Vec2::new(29.0, -29.0), 100.0);
        assert!(directions.contains(&MobileDirection::Forward));
        assert!(directions.contains(&MobileDirection::Right));
        assert_eq!(directions.len(), 2);
        assert!(stick_directions(Vec2::new(28.0, 0.0), 100.0).is_empty());
    }

    #[test]
    fn command_direction_keeps_cardinal_intent_across_diagonal_jitter_and_scale() {
        for radius in [32.0, 48.0, 96.0] {
            for (direction, forward, side) in [
                (MobileDirection::Forward, Vec2::NEG_Y, Vec2::X),
                (MobileDirection::Backward, Vec2::Y, Vec2::NEG_X),
                (MobileDirection::Left, Vec2::NEG_X, Vec2::NEG_Y),
                (MobileDirection::Right, Vec2::X, Vec2::Y),
            ] {
                let mut previous = stick_command_direction(forward * radius, radius, None);
                for lateral in [0.27, 0.29, 0.9, 1.1, 0.98, 1.25] {
                    previous = stick_command_direction(
                        (forward + side * lateral) * radius,
                        radius,
                        previous,
                    );
                    assert_eq!(previous, Some(direction));
                }
                assert_ne!(
                    stick_command_direction((forward + side * 1.4) * radius, radius, previous),
                    Some(direction)
                );
                assert_ne!(
                    stick_command_direction(-forward * radius, radius, previous),
                    Some(direction),
                    "a deliberate opposite input must switch immediately"
                );
            }
        }
    }

    #[test]
    fn command_center_hysteresis_requires_a_real_release_before_repeated_direction() {
        let up = Some(MobileDirection::Forward);
        assert_eq!(
            stick_command_direction(Vec2::new(0.0, -0.27), 1.0, None),
            None
        );
        assert_eq!(
            stick_command_direction(Vec2::new(0.0, -0.29), 1.0, None),
            up
        );
        for radius in [0.27, 0.29, 0.23, 0.19] {
            assert_eq!(
                stick_command_direction(Vec2::new(0.0, -radius), 1.0, up),
                up
            );
        }
        assert_eq!(
            stick_command_direction(Vec2::new(0.0, -0.17), 1.0, up),
            None
        );
        assert_eq!(
            stick_command_direction(Vec2::new(0.0, -0.23), 1.0, None),
            None
        );
        assert_eq!(
            stick_command_direction(Vec2::new(0.0, -0.29), 1.0, None),
            up
        );
    }

    #[test]
    fn floating_stick_cancel_resets_command_and_movement_independently() {
        let resolved = resolve_mobile_layout(&MobileControlLayout::default(), 1000.0, 500.0);
        let origin = Vec2::new(300.0, 150.0);
        assert!(resolved.control_at(origin).is_none());
        let radius = resolved.stick_max_distance;
        let (mut app, window) = touch_test_app(resolved);
        for (phase, position) in [
            (TouchPhase::Started, origin),
            (TouchPhase::Moved, origin + Vec2::new(0.4, -1.0) * radius),
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
        let state = app.world().resource::<MobileTouchState>();
        assert_eq!(state.command_direction, Some(MobileDirection::Forward));
        assert_eq!(state.stick_directions.len(), 2);
        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Canceled,
            position: origin,
            window,
            force: None,
            id: 1,
        });
        app.update();
        let state = app.world().resource::<MobileTouchState>();
        assert_eq!(state.command_direction, None);
        assert!(state.stick_directions.is_empty());
        assert_eq!(state.floating_stick_center, None);
    }

    #[test]
    fn mobile_layout_uses_web_schema_and_clamps_values() {
        let layout = normalize_mobile_layout(Some(
            r#"{"stick":{"x":1,"y":100,"scale":2},"attack":{"x":42,"y":43,"scale":0.8}}"#,
        ));
        assert_eq!(
            layout[&MobileLayoutKey::Stick],
            MobileLayoutItem {
                x: 5.0,
                y: 95.0,
                scale: 1.5,
            }
        );
        assert_eq!(
            layout[&MobileLayoutKey::Attack],
            MobileLayoutItem {
                x: 42.0,
                y: 43.0,
                scale: 0.8,
            }
        );
        assert_eq!(
            layout[&MobileLayoutKey::Jump],
            MobileLayoutKey::Jump.fallback()
        );
    }

    #[test]
    fn default_landscape_controls_leave_the_fighter_view_clear() {
        for (width, height) in [(640.0, 360.0), (899.2, 432.0), (960.0, 540.0)] {
            let layout = resolve_mobile_layout_with_safe_area(
                &MobileControlLayout(default_mobile_layout()),
                width,
                height,
                SafeAreaInsets {
                    left: 28.0,
                    right: 12.0,
                    ..default()
                },
            );
            for (i, region) in layout.regions.iter().enumerate() {
                let min = region.center - region.visual_size * 0.5;
                let max = region.center + region.visual_size * 0.5;
                assert!(
                    max.x <= width * 0.30
                        || min.x >= width * 0.70
                        || max.y <= height * 0.20
                        || min.y >= height * 0.80,
                    "{:?} obscures the central fighter view",
                    region.kind
                );
                for other in &layout.regions[i + 1..] {
                    let delta = (region.center - other.center).abs();
                    let extent = (region.visual_size + other.visual_size) * 0.5;
                    assert!(
                        delta.x >= extent.x || delta.y >= extent.y,
                        "{:?} overlaps {:?} at {width}x{height}",
                        region.kind,
                        other.kind
                    );
                }
            }
        }
    }

    #[test]
    fn mobile_layout_json_round_trips_the_complete_runtime_schema() {
        let mut layout = default_mobile_layout();
        layout.insert(
            MobileLayoutKey::Attack,
            MobileLayoutItem {
                x: 41.5,
                y: 62.25,
                scale: 1.21,
            },
        );
        let json = mobile_layout_json(&layout);
        let parsed = normalize_mobile_layout(Some(&json));
        assert_eq!(parsed, layout);
        let object = serde_json::from_str::<serde_json::Value>(&json)
            .expect("layout json")
            .as_object()
            .expect("layout object")
            .clone();
        assert_eq!(object.len(), MobileLayoutKey::ALL.len());
        assert!(MobileLayoutKey::ALL
            .into_iter()
            .all(|key| object.contains_key(key.name())));
    }

    #[test]
    fn saved_layout_resource_drives_the_live_visual_and_hit_region() {
        let mut layout = MobileControlLayout::default();
        layout.0.insert(
            MobileLayoutKey::Attack,
            MobileLayoutItem {
                x: 40.0,
                y: 30.0,
                scale: 1.2,
            },
        );
        let resolved = resolve_mobile_layout(&layout, 817.0, 393.0);
        let attack = resolved
            .region(MobileControlKind::Action(MobileAction::Attack))
            .expect("custom attack region");
        assert!((attack.center.x - 817.0 * 0.4).abs() < 1e-4);
        assert!((attack.center.y - 393.0 * 0.3).abs() < 1e-4);
        assert!(attack.visual_size.x > MIN_TOUCH_TARGET);
        assert_eq!(
            resolved.control_at(attack.center),
            Some(MobileControlKind::Action(MobileAction::Attack))
        );
    }

    #[test]
    fn overlapping_custom_actions_follow_web_visual_order_and_hold_boundary() {
        let mut layout = MobileControlLayout::default();
        let attack_layout = layout.0[&MobileLayoutKey::Attack];
        layout.0.insert(MobileLayoutKey::Cure, attack_layout);
        let resolved = resolve_mobile_layout(&layout, 817.0, 393.0);
        let attack = resolved
            .region(MobileControlKind::Action(MobileAction::Attack))
            .expect("attack region");
        let cure = resolved
            .region(MobileControlKind::Action(MobileAction::Cure))
            .expect("cure region");
        assert_eq!(attack.center, cure.center);
        assert_eq!(
            resolved.control_at(attack.center),
            Some(MobileControlKind::Action(MobileAction::Attack)),
            "Web live DOM places attack above cure"
        );
        assert_eq!(
            moved_action_at(&resolved, Some(MobileAction::Cure), cure.center),
            Some(MobileAction::Cure),
            "the held button keeps ownership while the finger remains in its real box"
        );

        let attack_only_edge = attack.center + Vec2::new(cure.visual_size.x * 0.5 + 1.0, 0.0);
        assert!(attack.contains_visual_box(attack_only_edge));
        assert!(!cure.contains_visual_box(attack_only_edge));
        assert_eq!(
            moved_action_at(&resolved, Some(MobileAction::Cure), attack_only_edge),
            Some(MobileAction::Attack)
        );
    }

    #[test]
    fn editor_cancel_keeps_saved_layout_and_save_applies_normalized_draft() {
        let saved = MobileControlLayout(default_mobile_layout());
        let mut cancel_state = MobileLayoutEditorState {
            open: true,
            ..default()
        };
        cancel_state
            .draft
            .get_mut(&MobileLayoutKey::Attack)
            .unwrap()
            .x = 37.0;
        let mut cancel_app = App::new();
        cancel_app
            .insert_resource(saved.clone())
            .insert_resource(cancel_state)
            .add_systems(Update, mobile_layout_editor_actions);
        cancel_app.world_mut().spawn((
            Button,
            Interaction::Pressed,
            MobileLayoutEditorAction::Cancel,
        ));
        cancel_app.update();
        assert_eq!(cancel_app.world().resource::<MobileControlLayout>(), &saved);
        assert!(
            !cancel_app
                .world()
                .resource::<MobileLayoutEditorState>()
                .open
        );

        let mut save_state = MobileLayoutEditorState {
            open: true,
            ..default()
        };
        save_state.draft.insert(
            MobileLayoutKey::Attack,
            MobileLayoutItem {
                x: 101.0,
                y: 37.0,
                scale: 2.0,
            },
        );
        let mut save_app = App::new();
        save_app
            .insert_resource(saved)
            .insert_resource(save_state)
            .add_systems(Update, mobile_layout_editor_actions);
        save_app
            .world_mut()
            .spawn((Button, Interaction::Pressed, MobileLayoutEditorAction::Save));
        save_app.update();
        assert_eq!(
            save_app.world().resource::<MobileControlLayout>().0[&MobileLayoutKey::Attack],
            MobileLayoutItem {
                x: 95.0,
                y: 37.0,
                scale: 1.5,
            }
        );
        assert!(!save_app.world().resource::<MobileLayoutEditorState>().open);
    }

    #[test]
    fn opening_editor_builds_the_complete_runtime_preview_from_a_layout_draft() {
        let mut saved = default_mobile_layout();
        saved.get_mut(&MobileLayoutKey::Guard).unwrap().x = 63.0;
        let expected = saved.clone();
        let mut app = App::new();
        app.insert_resource(MobileControlLayout(saved))
            .init_resource::<MobileLayoutEditorState>()
            .init_resource::<EffectiveUiScale>()
            .insert_resource(UiFont(Handle::default()))
            .add_systems(Update, open_mobile_layout_editor);
        app.world_mut()
            .spawn((Button, Interaction::Pressed, OpenMobileLayoutEditorButton));
        app.update();

        let editor = app.world().resource::<MobileLayoutEditorState>();
        assert!(editor.open);
        assert_eq!(editor.draft, expected);
        assert_eq!(editor.selected, MobileLayoutKey::Attack);
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<MobileLayoutEditorRoot>>()
                .iter(app.world())
                .count(),
            1
        );
        assert_eq!(
            app.world_mut()
                .query::<&MobileLayoutEditorItem>()
                .iter(app.world())
                .count(),
            MobileLayoutKey::ALL.len()
        );
        assert_eq!(
            app.world_mut()
                .query::<&MobileLayoutEditorAction>()
                .iter(app.world())
                .count(),
            5
        );
    }

    #[test]
    fn editor_preview_matches_live_nodes_after_save_across_viewports_and_scales() {
        use bevy::ecs::system::RunSystemOnce;

        let px = |value: Val| match value {
            Val::Px(value) => value,
            _ => panic!("expected pixel geometry, got {value:?}"),
        };
        for (width, height, dpi) in [(1634, 786, 2.0), (1280, 720, 1.0), (1179, 2451, 3.0)] {
            for unit in [0.55, 1.0, 1.5] {
                let mut app = App::new();
                let safe = SafeAreaInsets {
                    left: 34.0,
                    top: 8.0,
                    right: 12.0,
                    bottom: 24.0,
                };
                app.insert_resource(MobileControlLayout(default_mobile_layout()))
                    .init_resource::<MobileLayoutEditorState>()
                    .insert_resource(EffectiveUiScale(unit))
                    .insert_resource(safe)
                    .insert_resource(UiFont(Handle::default()))
                    .add_systems(
                        Update,
                        (open_mobile_layout_editor, refresh_mobile_layout_editor).chain(),
                    );
                let window = Window {
                    resolution: WindowResolution::new(width, height)
                        .with_scale_factor_override(dpi),
                    ..default()
                };
                let viewport = Vec2::new(window.width(), window.height());
                app.world_mut().spawn(window);
                app.world_mut()
                    .spawn((Button, Interaction::Pressed, OpenMobileLayoutEditorButton));
                app.update();

                let available = viewport - Vec2::new(0.0, 144.0);
                let ratio = (available / viewport).min_element();
                let area = app
                    .world_mut()
                    .query_filtered::<Entity, With<MobileLayoutEditorPreviewArea>>()
                    .single(app.world())
                    .unwrap();
                app.world_mut().get_mut::<ComputedNode>(area).unwrap().size = available * dpi;
                for (i, key) in MobileLayoutKey::ALL.into_iter().enumerate() {
                    app.world_mut()
                        .resource_mut::<MobileLayoutEditorState>()
                        .draft
                        .insert(
                            key,
                            MobileLayoutItem {
                                x: if i % 2 == 0 { 5.0 } else { 95.0 },
                                y: if i % 3 == 0 { 5.0 } else { 95.0 },
                                scale: if i % 2 == 0 { SCALE_MIN } else { SCALE_MAX },
                            },
                        );
                }
                app.update();
                let preview = app
                    .world_mut()
                    .query_filtered::<&Node, With<MobileLayoutEditorPreview>>()
                    .single(app.world())
                    .unwrap();
                assert!(
                    (px(preview.width) / px(preview.height) - viewport.x / viewport.y).abs() < 1e-5
                );
                assert!(
                    (px(preview.left) * unit - (available.x - viewport.x * ratio) * 0.5).abs()
                        < 1e-4
                );
                let preview_nodes: Vec<_> = app
                    .world_mut()
                    .query::<(&MobileLayoutEditorItem, &Node)>()
                    .iter(app.world())
                    .map(|(key, node)| (key.0, node.clone()))
                    .collect();

                app.world_mut().spawn((
                    Button,
                    Interaction::Pressed,
                    MobileLayoutEditorAction::Save,
                ));
                app.world_mut()
                    .run_system_once(mobile_layout_editor_actions)
                    .unwrap();
                app.world_mut()
                    .run_system_once(setup_mobile_controls)
                    .unwrap();
                let live_nodes: Vec<_> = app
                    .world_mut()
                    .query::<(&MobileControlKind, &Node)>()
                    .iter(app.world())
                    .map(|(kind, node)| (*kind, node.clone()))
                    .collect();
                for (key, preview) in preview_nodes {
                    if key == MobileLayoutKey::Radar {
                        let resolved = resolve_mobile_layout_with_safe_area(
                            app.world().resource::<MobileControlLayout>(),
                            viewport.x,
                            viewport.y,
                            safe,
                        );
                        let window = app
                            .world_mut()
                            .query::<&Window>()
                            .single(app.world())
                            .unwrap();
                        let (center, diameter) = super::super::ui_plugin::radar_screen_geometry(
                            window,
                            safe,
                            Some(&resolved),
                        );
                        assert!((px(preview.left) * unit / ratio - center.x).abs() < 0.001);
                        assert!((px(preview.top) * unit / ratio - center.y).abs() < 0.001);
                        assert!((px(preview.width) * unit / ratio - diameter).abs() < 0.001);
                        continue;
                    }
                    let (_, live) = live_nodes
                        .iter()
                        .find(|(kind, _)| *kind == mobile_layout_control_kind(key))
                        .unwrap();
                    for (a, b) in [
                        (preview.left, live.left),
                        (preview.top, live.top),
                        (preview.width, live.width),
                        (preview.height, live.height),
                        (preview.margin.left, live.margin.left),
                        (preview.margin.top, live.margin.top),
                    ] {
                        assert!(
                            (px(a) / ratio - px(b)).abs() < 0.001,
                            "{key:?}: {a:?} vs {b:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn editor_size_down_and_reset_change_only_the_draft() {
        let saved = MobileControlLayout(default_mobile_layout());
        let state = MobileLayoutEditorState {
            open: true,
            ..default()
        };
        let mut app = App::new();
        app.insert_resource(saved.clone())
            .insert_resource(state)
            .add_systems(Update, mobile_layout_editor_actions);
        let action = app
            .world_mut()
            .spawn((
                Button,
                Interaction::Pressed,
                MobileLayoutEditorAction::SizeDown,
            ))
            .id();
        app.update();
        assert!(
            (app.world().resource::<MobileLayoutEditorState>().draft[&MobileLayoutKey::Attack]
                .scale
                - 0.9)
                .abs()
                < 1e-5
        );
        assert_eq!(app.world().resource::<MobileControlLayout>(), &saved);

        *app.world_mut().get_mut::<Interaction>(action).unwrap() = Interaction::None;
        app.update();
        *app.world_mut()
            .get_mut::<MobileLayoutEditorAction>(action)
            .unwrap() = MobileLayoutEditorAction::Reset;
        *app.world_mut().get_mut::<Interaction>(action).unwrap() = Interaction::Pressed;
        app.update();
        assert_eq!(
            app.world().resource::<MobileLayoutEditorState>().draft,
            default_mobile_layout()
        );
        assert_eq!(app.world().resource::<MobileControlLayout>(), &saved);
    }

    #[test]
    fn editor_escape_closes_as_cancel_without_mutating_layout() {
        let saved = MobileControlLayout(default_mobile_layout());
        let mut state = MobileLayoutEditorState {
            open: true,
            ..default()
        };
        state.draft.get_mut(&MobileLayoutKey::Stick).unwrap().y = 44.0;
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::Escape);
        let mut app = App::new();
        app.insert_resource(saved.clone())
            .insert_resource(state)
            .insert_resource(keys)
            .add_systems(Update, mobile_layout_editor_escape);
        let root = app.world_mut().spawn(MobileLayoutEditorRoot).id();
        app.update();

        assert_eq!(app.world().resource::<MobileControlLayout>(), &saved);
        assert!(!app.world().resource::<MobileLayoutEditorState>().open);
        assert!(app.world().get_entity(root).is_err());
    }

    #[test]
    fn hidpi_editor_drag_maps_letterboxed_preview_to_game_viewport() {
        let state = MobileLayoutEditorState {
            open: true,
            ..default()
        };
        let mut app = App::new();
        app.add_message::<TouchInput>()
            .insert_resource(state)
            .add_systems(Update, mobile_layout_editor_touch);
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: WindowResolution::new(800, 500).with_scale_factor_override(2.0),
                ..default()
            })
            .id();
        app.world_mut().spawn((
            Node::default(),
            ComputedNode {
                size: Vec2::new(480.0, 300.0),
                ..default()
            },
            UiGlobalTransform::from_xy(400.0, 250.0),
            InheritedVisibility::VISIBLE,
            MobileLayoutEditorPreview,
        ));
        app.world_mut().spawn((
            Button,
            Node::default(),
            ComputedNode {
                size: Vec2::splat(72.0),
                ..default()
            },
            UiGlobalTransform::from_xy(496.0, 364.0),
            InheritedVisibility::VISIBLE,
            ComputedStackIndex(10),
            MobileLayoutEditorItem(MobileLayoutKey::Jump),
        ));

        for (phase, position) in [
            (TouchPhase::Started, Vec2::new(248.0, 182.0)),
            (TouchPhase::Moved, Vec2::new(200.0, 87.5)),
        ] {
            app.world_mut().write_message(TouchInput {
                phase,
                position,
                window,
                force: None,
                id: 8,
            });
            app.update();
        }

        let editor = app.world().resource::<MobileLayoutEditorState>();
        assert_eq!(editor.selected, MobileLayoutKey::Jump);
        let item = editor.draft[&MobileLayoutKey::Jump];
        assert!((item.x - 50.0).abs() < 1e-4);
        assert!((item.y - 25.0).abs() < 1e-4);
        let saved = MobileControlLayout(normalize_mobile_layout(Some(&mobile_layout_json(
            &editor.draft,
        ))));
        let resolved = resolve_mobile_layout(&saved, 400.0, 250.0);
        let jump = resolved
            .region(MobileControlKind::Action(MobileAction::Jump))
            .unwrap();
        assert!((jump.center - Vec2::new(200.0, 62.5)).length() < 1e-4);
    }

    #[test]
    fn opening_editor_does_not_replay_touches_collected_while_it_was_closed() {
        let mut app = App::new();
        app.add_message::<TouchInput>()
            .init_resource::<MobileLayoutEditorState>()
            .add_systems(Update, mobile_layout_editor_touch);
        let window = app.world_mut().spawn(Window::default()).id();
        for phase in [TouchPhase::Started, TouchPhase::Ended] {
            app.world_mut().write_message(TouchInput {
                phase,
                position: Vec2::new(100.0, 100.0),
                window,
                force: None,
                id: 12,
            });
        }
        app.update();

        app.world_mut()
            .resource_mut::<MobileLayoutEditorState>()
            .open = true;
        app.world_mut().spawn((
            Node::default(),
            ComputedNode {
                size: Vec2::new(800.0, 300.0),
                ..default()
            },
            UiGlobalTransform::from_xy(400.0, 250.0),
            InheritedVisibility::VISIBLE,
            MobileLayoutEditorPreview,
        ));
        app.update();
        assert!(app
            .world()
            .resource::<MobileLayoutEditorState>()
            .pointer
            .is_none());
    }

    #[test]
    fn editor_touch_button_resizes_only_after_release() {
        let state = MobileLayoutEditorState {
            open: true,
            ..default()
        };
        let mut app = App::new();
        app.add_message::<TouchInput>()
            .insert_resource(state)
            .insert_resource(MobileControlLayout::default())
            .add_systems(
                Update,
                (mobile_layout_editor_touch, mobile_layout_editor_actions).chain(),
            );
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: WindowResolution::new(800, 500).with_scale_factor_override(2.0),
                ..default()
            })
            .id();
        app.world_mut().spawn((
            Node::default(),
            ComputedNode {
                size: Vec2::new(800.0, 300.0),
                ..default()
            },
            UiGlobalTransform::from_xy(400.0, 250.0),
            InheritedVisibility::VISIBLE,
            MobileLayoutEditorPreview,
        ));
        let button = app
            .world_mut()
            .spawn((
                Button,
                Node::default(),
                Interaction::Pressed,
                ComputedNode {
                    size: Vec2::new(160.0, 96.0),
                    ..default()
                },
                UiGlobalTransform::from_xy(400.0, 450.0),
                InheritedVisibility::VISIBLE,
                ComputedStackIndex(20),
                MobileLayoutEditorAction::SizeUp,
            ))
            .id();

        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Started,
            position: Vec2::new(200.0, 225.0),
            window,
            force: None,
            id: 9,
        });
        app.update();
        assert_eq!(
            app.world().resource::<MobileLayoutEditorState>().draft[&MobileLayoutKey::Attack].scale,
            1.0
        );
        assert_eq!(
            *app.world().get::<Interaction>(button).unwrap(),
            Interaction::None
        );

        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Ended,
            position: Vec2::new(200.0, 225.0),
            window,
            force: None,
            id: 9,
        });
        app.update();
        assert!(
            (app.world().resource::<MobileLayoutEditorState>().draft[&MobileLayoutKey::Attack]
                .scale
                - 1.1)
                .abs()
                < 1e-5
        );
    }

    #[test]
    fn direction_double_tap_requests_dodge_and_guard_burst() {
        let keymap = Keymap::default();
        let mut keys = ButtonInput::<KeyCode>::default();
        let mut mouse = ButtonInput::<MouseButton>::default();
        let mut state = MobileTouchState::default();
        let mut synthetic = SyntheticInputStream::default();
        set_stick_input(
            Vec2::NEG_Y,
            1.0,
            1.0,
            &mut state,
            &keymap,
            &mut keys,
            &mut mouse,
            &mut synthetic,
        );
        set_stick_input(
            Vec2::ZERO,
            1.0,
            1.1,
            &mut state,
            &keymap,
            &mut keys,
            &mut mouse,
            &mut synthetic,
        );
        press_action(
            MobileAction::Guard,
            &mut state,
            &keymap,
            &mut keys,
            &mut mouse,
            &mut synthetic,
        );
        set_stick_input(
            Vec2::NEG_Y,
            1.0,
            1.2,
            &mut state,
            &keymap,
            &mut keys,
            &mut mouse,
            &mut synthetic,
        );
        assert!(synthetic.contains_dodge(Dir::Up, true));
        assert!(keymap.move_up.pressed(&keys, &mouse));
    }

    #[test]
    fn canceled_jump_does_not_emit_release_edge() {
        let keymap = Keymap::default();
        let mut keys = ButtonInput::<KeyCode>::default();
        let mut mouse = ButtonInput::<MouseButton>::default();
        let mut state = MobileTouchState::default();
        let mut synthetic = SyntheticInputStream::default();
        press_action(
            MobileAction::Jump,
            &mut state,
            &keymap,
            &mut keys,
            &mut mouse,
            &mut synthetic,
        );
        release_action(
            MobileAction::Jump,
            true,
            &mut state,
            &keymap,
            &mut keys,
            &mut mouse,
            &mut synthetic,
        );
        assert!(!keymap.jump.pressed(&keys, &mouse));
        assert!(!keymap.jump.just_released(&keys, &mouse));
        assert_eq!(synthetic.len(), 2, "press + semantic cancel");
        assert!(synthetic.contains_cancel(keymap.jump));
    }

    #[test]
    fn radar_touch_does_not_start_floating_stick_or_camera_drag() {
        let resolved = resolve_mobile_layout(&MobileControlLayout::default(), 1000.0, 500.0);
        let (mut app, window) = touch_test_app(resolved);
        let mut radar = super::super::ui_plugin::RadarState::default();
        radar.tap_circle = Some((Vec2::splat(60.0), 50.0));
        app.insert_resource(radar);
        for (phase, position) in [
            (TouchPhase::Started, Vec2::splat(60.0)),
            (TouchPhase::Moved, Vec2::new(600.0, 100.0)),
            (TouchPhase::Ended, Vec2::new(600.0, 100.0)),
        ] {
            app.world_mut().write_message(TouchInput {
                phase,
                position,
                window,
                force: None,
                id: 7,
            });
        }
        app.update();
        let state = app.world().resource::<MobileTouchState>();
        assert!(state.stick_pointer.is_none());
        assert!(state.action_counts.is_empty());
        assert!(state.pointers.is_empty());
        assert_eq!(app.world().resource::<MobileLookDelta>().0, Vec2::ZERO);
        assert_eq!(app.world().resource::<SyntheticInputStream>().len(), 0);
    }

    #[test]
    fn production_unclaimed_touch_drag_accumulates_camera_delta_without_attack() {
        let resolved = resolve_mobile_layout(&MobileControlLayout::default(), 1000.0, 500.0);
        let (mut app, window) = touch_test_app(resolved);
        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Started,
            position: Vec2::new(600.0, 100.0),
            window,
            force: None,
            id: 7,
        });
        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Moved,
            position: Vec2::new(614.0, 94.0),
            window,
            force: None,
            id: 7,
        });
        app.update();
        assert_eq!(
            app.world().resource::<MobileLookDelta>().0,
            Vec2::new(14.0, -6.0)
        );
        let keymap = app.world().resource::<Keymap>();
        let keys = app.world().resource::<ButtonInput<KeyCode>>();
        let mouse = app.world().resource::<ButtonInput<MouseButton>>();
        assert!(!keymap.attack.pressed(keys, mouse));
        assert!(!keymap.attack.just_pressed(keys, mouse));
    }

    #[test]
    fn debug_side_panel_consumes_raw_touches_without_replaying_actions() {
        let resolved = resolve_mobile_layout(&MobileControlLayout::default(), 1000.0, 500.0);
        let attack = resolved
            .regions
            .iter()
            .find(|region| region.kind == MobileControlKind::Action(MobileAction::Attack))
            .unwrap()
            .center;
        let (mut app, window) = touch_test_app(resolved);
        app.insert_resource(crate::plugins::debug_plugin::DebugConsoleState {
            open: false,
            panel_open: true,
        });
        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Started,
            position: attack,
            window,
            force: None,
            id: 8,
        });
        app.update();

        assert!(app
            .world()
            .resource::<MobileTouchState>()
            .pointers
            .is_empty());
        assert_eq!(app.world().resource::<SyntheticInputStream>().len(), 0);
        let keymap = app.world().resource::<Keymap>();
        let keys = app.world().resource::<ButtonInput<KeyCode>>();
        let mouse = app.world().resource::<ButtonInput<MouseButton>>();
        assert!(!keymap.attack.pressed(keys, mouse));
    }

    #[test]
    fn resolved_layout_claims_android_short_landscape_controls_in_logical_space() {
        let resolved = resolve_mobile_layout(&MobileControlLayout::default(), 817.0, 393.0);
        let stick = resolved
            .region(MobileControlKind::Stick)
            .expect("stick region");
        let attack = resolved
            .region(MobileControlKind::Action(MobileAction::Attack))
            .expect("attack region");
        assert!(matches!(
            resolved.control_at(stick.center),
            Some(MobileControlKind::Stick)
        ));
        assert!(matches!(
            resolved.control_at(attack.center + Vec2::splat(ACTION_HIT_SLOP * 0.5)),
            Some(MobileControlKind::Action(MobileAction::Attack))
        ));
        assert!(resolved.control_at(Vec2::new(500.0, 100.0)).is_none());
    }

    #[test]
    fn resolved_layout_clamps_every_visible_control_inside_asymmetric_safe_area() {
        let safe = SafeAreaInsets {
            left: 52.0,
            top: 10.0,
            right: 24.0,
            bottom: 18.0,
        };
        let resolved = resolve_mobile_layout_with_safe_area(
            &MobileControlLayout::default(),
            817.0,
            393.0,
            safe,
        );
        for region in &resolved.regions {
            let half = region.visual_size * 0.5;
            assert!(region.center.x - half.x >= safe.left - 1e-4);
            assert!(region.center.y - half.y >= safe.top - 1e-4);
            assert!(region.center.x + half.x <= 817.0 - safe.right + 1e-4);
            assert!(region.center.y + half.y <= 393.0 - safe.bottom + 1e-4);
        }
    }

    #[test]
    fn production_logical_hit_region_presses_action_instead_of_look() {
        let resolved = resolve_mobile_layout(&MobileControlLayout::default(), 817.0, 393.0);
        let attack_center = resolved
            .region(MobileControlKind::Action(MobileAction::Attack))
            .expect("attack region")
            .center;
        let (mut app, window) = touch_test_app(resolved);
        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Started,
            position: attack_center,
            window,
            force: None,
            id: 11,
        });
        app.update();

        let keymap = app.world().resource::<Keymap>();
        let keys = app.world().resource::<ButtonInput<KeyCode>>();
        let mouse = app.world().resource::<ButtonInput<MouseButton>>();
        assert!(keymap.attack.pressed(keys, mouse));
        assert_eq!(app.world().resource::<MobileLookDelta>().0, Vec2::ZERO);
    }

    #[test]
    fn simultaneous_stick_action_and_look_keep_independent_pointer_owners() {
        let resolved = resolve_mobile_layout(&MobileControlLayout::default(), 817.0, 393.0);
        let stick = resolved
            .region(MobileControlKind::Stick)
            .expect("stick region");
        let attack = resolved
            .region(MobileControlKind::Action(MobileAction::Attack))
            .expect("attack region");
        let look_start = Vec2::new(500.0, 100.0);
        assert!(resolved.control_at(look_start).is_none());
        let (mut app, window) = touch_test_app(resolved);

        for event in [
            TouchInput {
                phase: TouchPhase::Started,
                position: stick.center,
                window,
                force: None,
                id: 1,
            },
            TouchInput {
                phase: TouchPhase::Moved,
                position: stick.center + Vec2::new(stick.size.x * 0.2, 0.0),
                window,
                force: None,
                id: 1,
            },
            TouchInput {
                phase: TouchPhase::Started,
                position: attack.center,
                window,
                force: None,
                id: 2,
            },
            TouchInput {
                phase: TouchPhase::Moved,
                position: attack.center + Vec2::new(2.0, -2.0),
                window,
                force: None,
                id: 2,
            },
            TouchInput {
                phase: TouchPhase::Started,
                position: look_start,
                window,
                force: None,
                id: 3,
            },
            TouchInput {
                phase: TouchPhase::Moved,
                position: look_start + Vec2::new(20.0, -10.0),
                window,
                force: None,
                id: 3,
            },
        ] {
            app.world_mut().write_message(event);
        }
        app.update();

        let state = app.world().resource::<MobileTouchState>();
        assert!(matches!(
            state.pointers.get(&1),
            Some(MobilePointer::Stick { .. })
        ));
        assert!(matches!(
            state.pointers.get(&2),
            Some(MobilePointer::Action(Some(MobileAction::Attack)))
        ));
        assert!(matches!(
            state.pointers.get(&3),
            Some(MobilePointer::Look { .. })
        ));
        let keymap = app.world().resource::<Keymap>();
        let keys = app.world().resource::<ButtonInput<KeyCode>>();
        let mouse = app.world().resource::<ButtonInput<MouseButton>>();
        assert!(keymap.move_right.pressed(keys, mouse));
        assert!(keymap.attack.pressed(keys, mouse));
        assert_eq!(
            app.world().resource::<MobileLookDelta>().0,
            Vec2::new(20.0, -10.0)
        );
    }

    #[test]
    fn second_left_pointer_is_ignored_while_stick_is_owned() {
        let resolved = resolve_mobile_layout(&MobileControlLayout::default(), 817.0, 393.0);
        let stick_center = resolved.stick_center;
        let (mut app, window) = touch_test_app(resolved);
        for event in [
            TouchInput {
                phase: TouchPhase::Started,
                position: stick_center,
                window,
                force: None,
                id: 1,
            },
            TouchInput {
                phase: TouchPhase::Started,
                position: Vec2::new(300.0, 100.0),
                window,
                force: None,
                id: 2,
            },
            TouchInput {
                phase: TouchPhase::Moved,
                position: Vec2::new(340.0, 80.0),
                window,
                force: None,
                id: 2,
            },
        ] {
            app.world_mut().write_message(event);
        }
        app.update();

        assert!(matches!(
            app.world().resource::<MobileTouchState>().pointers.get(&2),
            Some(MobilePointer::Ignored)
        ));
        assert_eq!(app.world().resource::<MobileLookDelta>().0, Vec2::ZERO);
    }

    #[test]
    fn control_size_factor_scales_with_short_edge() {
        assert_eq!(mobile_control_size_factor(800.0, 360.0), 0.7);
        assert_eq!(mobile_control_size_factor(1280.0, 720.0), 1.0);
        assert_eq!(mobile_control_size_factor(1920.0, 1080.0), 1.2);
    }

    #[test]
    fn clamped_anchor_keeps_stick_fully_on_small_screen() {
        let item = MobileLayoutItem {
            x: 12.0,
            y: 80.0,
            scale: 1.0,
        };

        let (x, y) = clamped_anchor(item, 52.5, 800.0, 360.0);
        let center_y = y / 100.0 * 360.0;
        assert!(center_y + 52.5 <= 360.0 + 1e-4);
        assert!(center_y - 52.5 >= -1e-4);
        let center_x = x / 100.0 * 800.0;
        assert!(center_x - 52.5 >= -1e-4);

        let (x, y) = clamped_anchor(item, 75.0, 1280.0, 720.0);
        assert_eq!((x, y), (12.0, 80.0));
    }

    #[test]
    fn floating_stick_claims_left_half_blank_touch() {
        let resolved = resolve_mobile_layout(&MobileControlLayout::default(), 1000.0, 500.0);
        let (mut app, window) = touch_test_app(resolved);

        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Started,
            position: Vec2::new(100.0, 250.0),
            window,
            force: None,
            id: 5,
        });
        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Moved,
            position: Vec2::new(140.0, 250.0),
            window,
            force: None,
            id: 5,
        });

        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Started,
            position: Vec2::new(800.0, 250.0),
            window,
            force: None,
            id: 6,
        });
        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Moved,
            position: Vec2::new(840.0, 250.0),
            window,
            force: None,
            id: 6,
        });
        app.update();

        let state = app.world().resource::<MobileTouchState>();
        assert_eq!(state.floating_stick_center, Some(Vec2::new(100.0, 250.0)));
        assert!(matches!(
            state.pointers.get(&5),
            Some(MobilePointer::Stick { floating: true, .. })
        ));
        assert!(matches!(
            state.pointers.get(&6),
            Some(MobilePointer::Look { .. })
        ));

        {
            let keymap = app.world().resource::<Keymap>();
            let keys = app.world().resource::<ButtonInput<KeyCode>>();
            let mouse = app.world().resource::<ButtonInput<MouseButton>>();
            assert!(keymap.move_right.pressed(keys, mouse));
            assert_eq!(
                app.world().resource::<MobileLookDelta>().0,
                Vec2::new(40.0, 0.0)
            );
        }

        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Ended,
            position: Vec2::new(140.0, 250.0),
            window,
            force: None,
            id: 5,
        });
        app.update();
        let state = app.world().resource::<MobileTouchState>();
        assert_eq!(state.floating_stick_center, None);
        assert!(state.stick_pointer.is_none());
        {
            let keymap = app.world().resource::<Keymap>();
            let keys = app.world().resource::<ButtonInput<KeyCode>>();
            let mouse = app.world().resource::<ButtonInput<MouseButton>>();
            assert!(!keymap.move_right.pressed(keys, mouse));
        }
    }
}
