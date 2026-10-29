#![allow(clippy::type_complexity)]

use super::ui_theme as theme;
use crate::app_state::LauncherStyledButton;
use crate::app_state::{
    reset_settings_defaults, save_settings, KeyCaptureButton, KeyCaptureState, KeyField, Keymap,
    Settings, SettingsBoolField, SettingsFormRoot, SettingsRangeDown, SettingsRangeField,
    SettingsRangeUp, SettingsRangeValue, SettingsResetButton, SettingsSelectField,
};
use crate::plugins::launcher_ui::{
    launcher_text, responsive_ui_layout, LAUNCHER_RED_BRIGHT, PANEL,
};
use crate::plugins::mobile_controls_plugin::{
    mobile_layout_editor_available, OpenMobileLayoutEditorButton,
};
use bevy::prelude::*;

const CYAN: Color = theme::MUTED;
const FIELDSET_W: f32 = 220.0;
const KEYS_FIELDSET_W: f32 = 100.0;

#[derive(Component)]
pub(crate) struct SettingsFieldset {
    full_width: bool,
}

#[derive(Component)]
pub(crate) struct SettingsKeyRow;

#[derive(Component)]
pub(crate) struct SettingsStepButton;

#[derive(Component)]
pub(crate) struct HitEffectsOptions;

#[derive(Resource, Default)]
pub(crate) struct SettingsSliderDirty(bool);

pub(crate) fn settings_slider_changes(
    mut changes: MessageReader<crate::plugins::ui_components::slider::UiSliderChanged>,
    drag: Res<crate::plugins::ui_components::slider::UiSliderDrag>,
    mut dirty: ResMut<SettingsSliderDirty>,
    mut sliders: Query<(
        &SettingsRangeField,
        &mut crate::plugins::ui_components::slider::UiSlider,
    )>,
    mut settings: ResMut<Settings>,
    keymap: Res<Keymap>,
) {
    for change in changes.read() {
        if let Ok((field, _)) = sliders.get(change.entity) {
            write_range(&mut settings, *field, change.value);
            dirty.0 = true;
        }
        if change.finished && dirty.0 {
            let _ = save_settings(&settings, &keymap);
            dirty.0 = false;
        }
    }
    if dirty.0 && !drag.is_active() {
        let _ = save_settings(&settings, &keymap);
        dirty.0 = false;
    }

    for (field, mut slider) in &mut sliders {
        slider.value = read_range(&settings, *field);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsCategory {
    Graphics,
    Renderer,
    Display,
    Audio,
    Camera,
    Controls,
}

impl SettingsCategory {
    const ALL: [(Self, &'static str); 6] = [
        (Self::Graphics, "画质"),
        (Self::Renderer, "设备"),
        (Self::Display, "显示"),
        (Self::Audio, "声音"),
        (Self::Camera, "镜头"),
        (Self::Controls, "操作"),
    ];
}

#[derive(Component)]
pub(crate) struct SettingsCategoryButton {
    root: Entity,
    category: SettingsCategory,
}
#[derive(Component)]
pub(crate) struct SettingsCategoryPage {
    root: Entity,
    pub(crate) category: SettingsCategory,
}
#[derive(Component)]
pub(crate) struct SettingsCategoryNav;
#[derive(Component)]
pub(crate) struct SettingsCategoryContent;

pub fn spawn_web_settings_form(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    settings: &Settings,
    keymap: &Keymap,
) {
    let mut form = parent.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            min_height: Val::Px(0.0),
            column_gap: Val::Px(14.0),
            ..default()
        },
        SettingsFormRoot,
    ));
    let root = form.id();
    form.with_children(|body| {
        body.spawn((
            crate::plugins::launcher_ui::scrollable_y(Node {
                width: Val::Px(132.0),
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                ..default()
            }),
            SettingsCategoryNav,
        ))
        .with_children(|nav| {
            for (category, label) in SettingsCategory::ALL {
                if category == SettingsCategory::Renderer && !crate::graphics_settings::AVAILABLE {
                    continue;
                }
                nav.spawn((
                    Button,
                    Node {
                        width: Val::Percent(100.0),
                        min_height: Val::Px(44.0),
                        flex_shrink: 0.0,
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::Center,
                        border: UiRect::left(Val::Px(3.0)),
                        padding: UiRect::horizontal(Val::Px(10.0)),
                        ..default()
                    },
                    BackgroundColor(theme::SURFACE),
                    BorderColor::all(if category == SettingsCategory::Graphics {
                        LAUNCHER_RED_BRIGHT
                    } else {
                        theme::LINE
                    }),
                    LauncherStyledButton,
                    SettingsCategoryButton { root, category },
                ))
                .with_children(|button| {
                    button.spawn(launcher_text(label, 15.0, theme::TEXT, font));
                });
            }
        });
        body.spawn((
            Node {
                width: Val::Px(0.0),
                flex_grow: 1.0,
                min_width: Val::Px(0.0),
                min_height: Val::Px(0.0),
                height: Val::Percent(100.0),
                ..default()
            },
            SettingsCategoryContent,
        ))
        .with_children(|content| {
            for (category, _) in SettingsCategory::ALL {
                if category == SettingsCategory::Renderer && !crate::graphics_settings::AVAILABLE {
                    continue;
                }
                content
                    .spawn((
                        crate::plugins::launcher_ui::scrollable_y(Node {
                            display: if category == SettingsCategory::Graphics {
                                Display::Flex
                            } else {
                                Display::None
                            },
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            flex_direction: FlexDirection::Column,
                            ..default()
                        }),
                        SettingsCategoryPage { root, category },
                    ))
                    .with_children(|page| {
                        page.spawn(Node {
                            width: Val::Percent(100.0),
                            flex_shrink: 0.0,
                            flex_wrap: FlexWrap::Wrap,
                            align_items: AlignItems::FlexStart,
                            column_gap: Val::Px(10.0),
                            row_gap: Val::Px(6.0),
                            ..default()
                        })
                        .with_children(|grid| match category {
                            SettingsCategory::Renderer => {
                                spawn_renderer_fieldset(grid, font, settings)
                            }
                            SettingsCategory::Graphics => {
                                spawn_graphics_fieldset(grid, font, settings)
                            }
                            SettingsCategory::Display => {
                                spawn_display_fieldset(grid, font, settings)
                            }
                            SettingsCategory::Audio => spawn_audio_fieldset(grid, font, settings),
                            SettingsCategory::Camera => spawn_camera_fieldset(grid, font, settings),
                            SettingsCategory::Controls => {
                                grid.spawn(fieldset_bundle(Val::Percent(100.0), true))
                                    .with_children(|fs| {
                                        spawn_select_row(
                                            fs,
                                            font,
                                            "触屏控件",
                                            SettingsSelectField::TouchControls,
                                            touch_controls_label(settings.touch_controls),
                                        );
                                    });
                                if mobile_layout_editor_available() {
                                    spawn_mobile_controls_fieldset(grid, font);
                                } else {
                                    spawn_keybindings_fieldset(grid, font, keymap);
                                }
                            }
                        });
                    });
            }
        });
    });
}

pub(crate) fn settings_category_buttons(
    buttons: Query<(&Interaction, &SettingsCategoryButton), Changed<Interaction>>,
    mut pages: Query<(&SettingsCategoryPage, &mut Node, &mut ScrollPosition)>,
    mut tabs: Query<(&SettingsCategoryButton, &mut BorderColor)>,
    mut capture: ResMut<KeyCaptureState>,
) {
    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        capture.pending = None;
        capture.arming = false;
        for (page, mut node, mut scroll) in &mut pages {
            if page.root != button.root {
                continue;
            }
            node.display = if page.category == button.category {
                Display::Flex
            } else {
                Display::None
            };
            *scroll = ScrollPosition::default();
        }
        for (tab, mut border) in &mut tabs {
            if tab.root == button.root {
                *border = BorderColor::all(if tab.category == button.category {
                    LAUNCHER_RED_BRIGHT
                } else {
                    theme::LINE
                });
            }
        }
    }
}

pub(crate) fn adapt_settings_navigation(
    windows: Query<&Window>,
    mut navs: Query<&mut Node, (With<SettingsCategoryNav>, Without<SettingsCategoryContent>)>,
    mut buttons: Query<
        &mut Node,
        (
            With<SettingsCategoryButton>,
            Without<SettingsCategoryNav>,
            Without<SettingsCategoryContent>,
        ),
    >,
    mut contents: Query<&mut Node, With<SettingsCategoryContent>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let portrait = window.width() < 600.0 && window.height() > window.width();
    let compact_nav = !portrait && window.height() < 500.0 && crate::graphics_settings::AVAILABLE;
    for mut node in &mut navs {
        node.width = if portrait {
            Val::Percent(100.0)
        } else {
            Val::Px(112.0)
        };
        node.flex_direction = if portrait || compact_nav {
            FlexDirection::Row
        } else {
            FlexDirection::Column
        };
        node.height = if portrait {
            Val::Px(44.0)
        } else {
            Val::Percent(100.0)
        };
        node.column_gap = Val::Px(2.0);
        node.flex_wrap = if compact_nav {
            FlexWrap::Wrap
        } else {
            FlexWrap::NoWrap
        };
        node.align_content = AlignContent::FlexStart;
    }
    for mut node in &mut contents {
        node.width = if portrait {
            Val::Percent(100.0)
        } else {
            Val::Px(0.0)
        };
        node.height = if portrait {
            Val::Px(0.0)
        } else {
            Val::Percent(100.0)
        };
    }
    for mut node in &mut buttons {
        node.width = if portrait {
            Val::Auto
        } else if compact_nav {
            Val::Percent(48.0)
        } else {
            Val::Percent(100.0)
        };
        node.flex_grow = if portrait { 1.0 } else { 0.0 };
    }
}

fn fieldset_bundle(width: Val, full_width: bool) -> impl Bundle {
    (
        Node {
            width,
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(10.0)),
            row_gap: Val::Px(6.0),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(3.0)),
            ..default()
        },
        BackgroundColor(PANEL),
        BorderColor::all(theme::LINE),
        SettingsFieldset { full_width },
    )
}

fn spawn_renderer_fieldset(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    settings: &Settings,
) {
    if crate::graphics_settings::AVAILABLE {
        parent
            .spawn(fieldset_bundle(Val::Px(FIELDSET_W), false))
            .with_children(|fs| {
                fs.spawn(launcher_text(
                    "窗口（立即生效）",
                    18.0,
                    LAUNCHER_RED_BRIGHT,
                    font,
                ));
                spawn_select_row(
                    fs,
                    font,
                    "窗口分辨率",
                    SettingsSelectField::WindowResolution,
                    crate::window_settings::resolution_label(settings.window_resolution),
                );
                spawn_check_row(
                    fs,
                    font,
                    "全屏模式",
                    SettingsBoolField::Fullscreen,
                    settings.fullscreen,
                );
                spawn_check_row(
                    fs,
                    font,
                    "无边框窗口",
                    SettingsBoolField::BorderlessWindow,
                    settings.borderless_window,
                );
                fs.spawn(launcher_text(
                    "全屏使用桌面分辨率；退出后恢复窗口尺寸。无边框仅影响窗口模式。",
                    12.0,
                    theme::MUTED,
                    font,
                ));
            });
        parent
            .spawn(fieldset_bundle(Val::Px(FIELDSET_W), false))
            .with_children(|fs| {
                fs.spawn(launcher_text(
                    "渲染设备（重启后生效）",
                    18.0,
                    LAUNCHER_RED_BRIGHT,
                    font,
                ));
                for (field, title) in [
                    (SettingsSelectField::GraphicsAdapter, "显卡"),
                    (SettingsSelectField::GraphicsBackend, "图形后端"),
                ] {
                    spawn_select_row(
                        fs,
                        font,
                        title,
                        field,
                        &crate::graphics_settings::label(settings, field),
                    );
                }
                fs.spawn(launcher_text(
                    "不兼容的显卡/后端组合将自动选择显卡。",
                    12.0,
                    theme::MUTED,
                    font,
                ));
                fs.spawn((
                    launcher_text("", 12.0, theme::MUTED, font),
                    crate::graphics_settings::CurrentRendererLabel,
                ));
            });
    }
}

fn spawn_graphics_fieldset(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    settings: &Settings,
) {
    parent
        .spawn(fieldset_bundle(Val::Px(FIELDSET_W), false))
        .with_children(|fs| {
            fs.spawn(launcher_text("画质与性能", 18.0, LAUNCHER_RED_BRIGHT, font));
            spawn_select_row(
                fs,
                font,
                "画面解析",
                SettingsSelectField::RenderScale,
                render_scale_label(settings.render_scale),
            );
            spawn_select_row(
                fs,
                font,
                "贴图品质",
                SettingsSelectField::TextureQuality,
                texture_quality_label(settings.texture_quality),
            );
            spawn_select_row(
                fs,
                font,
                "帧率上限",
                SettingsSelectField::TargetFps,
                target_fps_label(settings.target_fps),
            );
        });
    parent
        .spawn(fieldset_bundle(Val::Px(FIELDSET_W), false))
        .with_children(|fs| {
            fs.spawn(launcher_text(
                "战斗与场景特效",
                18.0,
                LAUNCHER_RED_BRIGHT,
                font,
            ));
            spawn_check_row(
                fs,
                font,
                "武器击打特效",
                SettingsBoolField::HitEffects,
                settings.hit_effects,
            );
            fs.spawn((
                Node {
                    display: if settings.hit_effects {
                        Display::Flex
                    } else {
                        Display::None
                    },
                    padding: UiRect::left(Val::Px(18.0)),
                    ..default()
                },
                HitEffectsOptions,
            ))
            .with_children(|options| {
                spawn_check_row(
                    options,
                    font,
                    "打击卡帧",
                    SettingsBoolField::HitStop,
                    settings.hit_stop,
                );
            });
            spawn_check_row(
                fs,
                font,
                "体积雾",
                SettingsBoolField::VolumetricFog,
                settings.volumetric_fog,
            );
        });
}

fn spawn_display_fieldset(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    settings: &Settings,
) {
    parent
        .spawn(fieldset_bundle(Val::Px(FIELDSET_W), false))
        .with_children(|fs| {
            fs.spawn(launcher_text("画面调整", 18.0, LAUNCHER_RED_BRIGHT, font));
            spawn_range_row(
                fs,
                font,
                "画面亮度",
                SettingsRangeField::Brightness,
                settings.brightness,
            );
            spawn_range_row(
                fs,
                font,
                "画面对比",
                SettingsRangeField::Contrast,
                settings.contrast,
            );
        });
    parent
        .spawn(fieldset_bundle(Val::Px(FIELDSET_W), false))
        .with_children(|fs| {
            fs.spawn(launcher_text("界面与色彩", 18.0, LAUNCHER_RED_BRIGHT, font));
            spawn_range_row(
                fs,
                font,
                "伽马值比",
                SettingsRangeField::Gamma,
                settings.gamma,
            );
            spawn_select_row(
                fs,
                font,
                "界面缩放",
                SettingsSelectField::UiScale,
                ui_scale_label(settings.ui_scale),
            );
        });
}

fn spawn_audio_fieldset(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    settings: &Settings,
) {
    parent
        .spawn(fieldset_bundle(Val::Px(FIELDSET_W), false))
        .with_children(|fs| {
            fs.spawn(launcher_text("声音", 18.0, LAUNCHER_RED_BRIGHT, font));
            spawn_check_row(
                fs,
                font,
                "CD 音乐",
                SettingsBoolField::MusicEnabled,
                settings.music_enabled,
            );
            spawn_range_row(
                fs,
                font,
                "音乐音量",
                SettingsRangeField::MusicVolume,
                settings.music_volume,
            );
            spawn_range_row(
                fs,
                font,
                "音效音量",
                SettingsRangeField::AudioVolume,
                settings.audio_volume,
            );
        });
}

fn spawn_camera_fieldset(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    settings: &Settings,
) {
    parent
        .spawn(fieldset_bundle(Val::Px(FIELDSET_W), false))
        .with_children(|fs| {
            fs.spawn(launcher_text("镜头", 18.0, LAUNCHER_RED_BRIGHT, font));
            spawn_check_row(
                fs,
                font,
                "自动镜头",
                SettingsBoolField::AutoCamera,
                settings.auto_camera,
            );
            spawn_check_row(
                fs,
                font,
                "显示锁定标记",
                SettingsBoolField::LockFlag,
                settings.lock_flag,
            );
            spawn_check_row(
                fs,
                font,
                "显示人名",
                SettingsBoolField::ShowNames,
                settings.show_names,
            );
        });
    parent
        .spawn(fieldset_bundle(Val::Px(FIELDSET_W), false))
        .with_children(|fs| {
            fs.spawn(launcher_text("鼠标", 18.0, LAUNCHER_RED_BRIGHT, font));
            spawn_check_row(
                fs,
                font,
                "鼠标反向",
                SettingsBoolField::ReverseMouse,
                settings.reverse_mouse,
            );
            spawn_check_row(
                fs,
                font,
                "鼠标拖曳",
                SettingsBoolField::DragMouse,
                settings.drag_mouse,
            );
            spawn_range_row(
                fs,
                font,
                "鼠标灵敏",
                SettingsRangeField::MouseSpeed,
                settings.mouse_speed,
            );
        });
}

fn spawn_keybindings_fieldset(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    keymap: &Keymap,
) {
    parent
        .spawn(fieldset_bundle(Val::Percent(KEYS_FIELDSET_W), true))
        .with_children(|fs| {
            fs.spawn(launcher_text("按键设置", 18.0, LAUNCHER_RED_BRIGHT, font));
            fs.spawn(launcher_text(
                "点击动作后按下新键位或鼠标按键，Esc 取消；键位冲突时自动互换。闪避为双击方向键。",
                11.0,
                theme::MUTED,
                font,
            ));
            fs.spawn((Node {
                width: Val::Percent(100.0),
                flex_wrap: FlexWrap::Wrap,
                column_gap: Val::Px(8.0),
                row_gap: Val::Px(4.0),
                ..default()
            },))
                .with_children(|keys_grid| {
                    for field in KeyField::BINDING_ACTIONS {
                        spawn_keybinding_row(keys_grid, font, field, field.get(keymap).label());
                    }
                });
        });
}

fn spawn_mobile_controls_fieldset(parent: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    parent
        .spawn(fieldset_bundle(Val::Percent(KEYS_FIELDSET_W), true))
        .with_children(|fieldset| {
            fieldset.spawn(launcher_text("移动端操作", 18.0, LAUNCHER_RED_BRIGHT, font));
            fieldset.spawn(launcher_text(
                "布局只在本机保存。按住模拟按键拖动；选中后可调整大小。",
                12.0,
                theme::MUTED,
                font,
            ));
            fieldset
                .spawn((
                    Button,
                    Node {
                        width: Val::Px(190.0),
                        min_height: Val::Px(48.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        padding: UiRect::horizontal(Val::Px(12.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        border_radius: BorderRadius::all(Val::Px(3.0)),
                        ..default()
                    },
                    BackgroundColor(theme::SURFACE),
                    BorderColor::all(theme::LINE),
                    Interaction::default(),
                    LauncherStyledButton,
                    OpenMobileLayoutEditorButton,
                ))
                .with_children(|button| {
                    button.spawn(launcher_text("调整按键布局", 14.0, CYAN, font));
                });
        });
}

fn spawn_keybinding_row(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    field: KeyField,
    binding: String,
) {
    parent
        .spawn((
            Button,
            Node {
                width: Val::Px(190.0),
                min_height: Val::Px(28.0),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                padding: UiRect::horizontal(Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                ..default()
            },
            BackgroundColor(theme::SURFACE),
            BorderColor::all(theme::LINE),
            Interaction::default(),
            LauncherStyledButton,
            KeyCaptureButton(field),
            SettingsKeyRow,
        ))
        .with_children(|row| {
            row.spawn(launcher_text(field.label(), 11.0, theme::MUTED, font));
            row.spawn(launcher_text(&binding, 11.0, CYAN, font));
        });
}

fn spawn_check_row(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    field: SettingsBoolField,
    checked: bool,
) {
    use crate::plugins::ui_components::*;
    parent
        .spawn((
            UiSelectableButtonBundle::new(
                Node {
                    width: Val::Percent(100.0),
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    min_height: Val::Px(36.0),
                    padding: UiRect::horizontal(Val::Px(8.0)),
                    border_radius: BorderRadius::all(Val::Px(3.0)),
                    ..default()
                },
                checked,
                UiSelectableKind::Checkbox,
                UiSelectableStyle::default(),
            ),
            LauncherStyledButton,
            field,
        ))
        .with_children(|row| {
            spawn_selection_indicator(row, UiSelectableKind::Checkbox, checked);
            row.spawn((
                launcher_text(label, 12.0, theme::TEXT, font),
                Node {
                    flex_grow: 1.0,
                    ..default()
                },
            ));
            row.spawn((
                launcher_text(bool_label(checked), 12.0, CYAN, font),
                SettingsBoolValue(field),
            ));
        });
}

#[derive(Component, Clone, Copy)]
pub(crate) struct SettingsBoolValue(SettingsBoolField);

fn spawn_select_row(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    field: SettingsSelectField,
    value_label: &str,
) {
    parent
        .spawn((
            Button,
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                min_height: Val::Px(36.0),
                padding: UiRect::horizontal(Val::Px(8.0)),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                ..default()
            },
            BackgroundColor(theme::SURFACE),
            Interaction::default(),
            LauncherStyledButton,
            field,
            crate::plugins::ui_components::UiSelectToggle(select_id(field)),
        ))
        .with_children(|row| {
            row.spawn(launcher_text(label, 12.0, theme::MUTED, font));
            let mut value = row.spawn((
                launcher_text(value_label, 12.0, CYAN, font),
                SettingsSelectValue(field),
            ));
            if matches!(
                field,
                SettingsSelectField::GraphicsAdapter | SettingsSelectField::GraphicsBackend
            ) {
                value.insert(Node {
                    min_width: Val::Px(0.0),
                    max_width: Val::Percent(72.0),
                    ..default()
                });
            }
        });
}

#[derive(Component, Clone, Copy)]
pub(crate) struct SettingsSelectValue(SettingsSelectField);

fn spawn_range_row(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    field: SettingsRangeField,
    value: f32,
) {
    parent
        .spawn((Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            ..default()
        },))
        .with_children(|row| {
            row.spawn((Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                ..default()
            },))
                .with_children(|header| {
                    header.spawn(launcher_text(label, 12.0, theme::MUTED, font));
                    header.spawn((
                        launcher_text(&format_range_value(field, value), 12.0, theme::MUTED, font),
                        SettingsRangeValue(field),
                    ));
                });
            row.spawn((Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                ..default()
            },))
                .with_children(|stepper| {
                    spawn_step_btn(stepper, font, "−", SettingsRangeDown(field));
                    let (min, max, step) = range_bounds(field);
                    crate::plugins::ui_components::slider::spawn_slider(
                        stepper,
                        crate::plugins::ui_components::slider::UiSlider {
                            min,
                            max,
                            step,
                            value,
                        },
                        field,
                    );
                    spawn_step_btn(stepper, font, "+", SettingsRangeUp(field));
                });
        });
}

fn spawn_step_btn<T: Component>(
    parent: &mut ChildSpawnerCommands,
    _font: &Handle<Font>,
    label: &str,
    marker: T,
) {
    parent
        .spawn((
            Button,
            Node {
                width: Val::Px(44.0),
                height: Val::Px(44.0),
                flex_shrink: 0.0,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Percent(50.0)),
                ..default()
            },
            BackgroundColor(theme::SURFACE),
            BorderColor::all(theme::LINE),
            Interaction::default(),
            LauncherStyledButton,
            marker,
            SettingsStepButton,
            theme::ActionStyle::SECONDARY,
        ))
        .with_children(|btn| {
            btn.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(14.0),
                    height: Val::Px(2.0),
                    left: Val::Percent(50.0),
                    top: Val::Percent(50.0),
                    margin: UiRect::new(Val::Px(-7.0), Val::ZERO, Val::Px(-1.0), Val::ZERO),
                    ..default()
                },
                BackgroundColor(CYAN),
            ));
            if label == "+" {
                btn.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Px(2.0),
                        height: Val::Px(14.0),
                        left: Val::Percent(50.0),
                        top: Val::Percent(50.0),
                        margin: UiRect::new(Val::Px(-1.0), Val::ZERO, Val::Px(-7.0), Val::ZERO),
                        ..default()
                    },
                    BackgroundColor(CYAN),
                ));
            }
        });
}

pub(crate) fn adapt_settings_layout(
    window: Query<&Window>,
    mut roots: Query<&mut Node, With<SettingsFormRoot>>,
    mut fieldsets: Query<(&SettingsFieldset, &mut Node), Without<SettingsFormRoot>>,
    mut key_rows: Query<
        &mut Node,
        (
            With<SettingsKeyRow>,
            Without<SettingsFormRoot>,
            Without<SettingsFieldset>,
        ),
    >,
    mut step_buttons: Query<
        &mut Node,
        (
            With<SettingsStepButton>,
            Without<SettingsFormRoot>,
            Without<SettingsFieldset>,
            Without<SettingsKeyRow>,
        ),
    >,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let layout = responsive_ui_layout(window.width(), window.height(), cfg!(target_os = "android"));

    for mut root in &mut roots {
        root.flex_direction = if window.width() < 600.0 && window.height() > window.width() {
            FlexDirection::Column
        } else {
            FlexDirection::Row
        };
        root.column_gap = Val::Px(if layout.mobile { 8.0 } else { 14.0 });
        root.row_gap = Val::Px(6.0);
    }
    for (fieldset, mut node) in &mut fieldsets {
        node.width = if fieldset.full_width || (layout.mobile && layout.stacked_stage) {
            Val::Percent(100.0)
        } else {
            Val::Percent(48.0)
        };
        node.padding = UiRect::all(Val::Px(if layout.mobile { 8.0 } else { 10.0 }));
        node.row_gap = Val::Px(if layout.mobile { 4.0 } else { 6.0 });
    }
    for mut node in &mut key_rows {
        node.width = if layout.mobile {
            Val::Percent(100.0)
        } else {
            Val::Px(190.0)
        };
        node.min_height = Val::Px(if layout.mobile { 44.0 } else { 28.0 });
    }
    for mut node in &mut step_buttons {
        let size = 44.0;
        node.width = Val::Px(size);
        node.height = Val::Px(size);
    }
}

fn bool_label(checked: bool) -> &'static str {
    if checked {
        "开"
    } else {
        "关"
    }
}

fn render_scale_label(scale: f32) -> &'static str {
    if (scale - 0.65).abs() < 0.01 {
        "流畅"
    } else if (scale - 0.8).abs() < 0.01 {
        "平衡"
    } else {
        "清晰"
    }
}

fn texture_quality_label(q: u8) -> &'static str {
    match q {
        2 => "低",
        1 => "中",
        _ => "好",
    }
}

fn target_fps_label(fps: u32) -> &'static str {
    match fps {
        30 => "30 FPS",
        60 => "60 FPS",
        120 => "120 FPS",
        _ => "不限",
    }
}

fn ui_scale_label(scale: f32) -> &'static str {
    if scale <= 0.0 {
        "自动"
    } else if (scale - 0.7).abs() < 0.01 {
        "70%"
    } else if (scale - 0.85).abs() < 0.01 {
        "85%"
    } else if (scale - 1.2).abs() < 0.01 {
        "120%"
    } else {
        "100%"
    }
}

fn touch_controls_label(value: i8) -> &'static str {
    match value {
        0 => "关",
        1 => "开",
        _ => "自动",
    }
}

fn format_range_value(field: SettingsRangeField, value: f32) -> String {
    match field {
        SettingsRangeField::MusicVolume | SettingsRangeField::AudioVolume => {
            format!("{:.0}", value.round())
        }
        SettingsRangeField::MouseSpeed => format!("{}", value.round() as i32),
        _ => format!("{}", value.round() as i32),
    }
}

fn range_bounds(field: SettingsRangeField) -> (f32, f32, f32) {
    match field {
        SettingsRangeField::Brightness => (45.0, 145.0, 1.0),
        SettingsRangeField::Contrast => (55.0, 145.0, 1.0),
        SettingsRangeField::Gamma => (0.0, 100.0, 1.0),
        SettingsRangeField::MusicVolume | SettingsRangeField::AudioVolume => (0.0, 100.0, 1.0),
        SettingsRangeField::MouseSpeed => {
            (Settings::MOUSE_SPEED_MIN, Settings::MOUSE_SPEED_MAX, 1.0)
        }
    }
}

fn read_range(settings: &Settings, field: SettingsRangeField) -> f32 {
    match field {
        SettingsRangeField::Brightness => settings.brightness,
        SettingsRangeField::Contrast => settings.contrast,
        SettingsRangeField::Gamma => settings.gamma,
        SettingsRangeField::MusicVolume => settings.music_volume,
        SettingsRangeField::AudioVolume => settings.audio_volume,
        SettingsRangeField::MouseSpeed => settings.mouse_speed,
    }
}

fn write_range(settings: &mut Settings, field: SettingsRangeField, value: f32) {
    let (min, max, _) = range_bounds(field);
    let v = value.clamp(min, max);
    match field {
        SettingsRangeField::Brightness => settings.brightness = v,
        SettingsRangeField::Contrast => settings.contrast = v,
        SettingsRangeField::Gamma => settings.gamma = v,
        SettingsRangeField::MusicVolume => settings.music_volume = v,
        SettingsRangeField::AudioVolume => settings.audio_volume = v,
        SettingsRangeField::MouseSpeed => settings.mouse_speed = v,
    }
}

fn read_bool(settings: &Settings, field: SettingsBoolField) -> bool {
    match field {
        SettingsBoolField::Fullscreen => settings.fullscreen,
        SettingsBoolField::BorderlessWindow => settings.borderless_window,
        SettingsBoolField::HitEffects => settings.hit_effects,
        SettingsBoolField::HitStop => settings.hit_stop,
        SettingsBoolField::VolumetricFog => settings.volumetric_fog,
        SettingsBoolField::MusicEnabled => settings.music_enabled,
        SettingsBoolField::AutoCamera => settings.auto_camera,
        SettingsBoolField::LockFlag => settings.lock_flag,
        SettingsBoolField::ShowNames => settings.show_names,
        SettingsBoolField::ReverseMouse => settings.reverse_mouse,
        SettingsBoolField::DragMouse => settings.drag_mouse,
    }
}

fn write_bool(settings: &mut Settings, field: SettingsBoolField, value: bool) {
    match field {
        SettingsBoolField::Fullscreen => settings.fullscreen = value,
        SettingsBoolField::BorderlessWindow => settings.borderless_window = value,
        SettingsBoolField::HitEffects => settings.hit_effects = value,
        SettingsBoolField::HitStop => settings.hit_stop = value,
        SettingsBoolField::VolumetricFog => settings.volumetric_fog = value,
        SettingsBoolField::MusicEnabled => settings.music_enabled = value,
        SettingsBoolField::AutoCamera => settings.auto_camera = value,
        SettingsBoolField::LockFlag => settings.lock_flag = value,
        SettingsBoolField::ShowNames => settings.show_names = value,
        SettingsBoolField::ReverseMouse => settings.reverse_mouse = value,
        SettingsBoolField::DragMouse => settings.drag_mouse = value,
    }
}

fn select_id(field: SettingsSelectField) -> crate::plugins::ui_components::UiSelectId {
    crate::plugins::ui_components::UiSelectId(100 + field as u16)
}

fn select_options(field: SettingsSelectField) -> Vec<(f32, String)> {
    if matches!(
        field,
        SettingsSelectField::GraphicsAdapter | SettingsSelectField::GraphicsBackend
    ) {
        return crate::graphics_settings::options(field);
    }
    let options: &[(f32, &str)] = match field {
        SettingsSelectField::WindowResolution => {
            return crate::window_settings::resolution_options()
        }
        SettingsSelectField::GraphicsAdapter | SettingsSelectField::GraphicsBackend => {
            unreachable!()
        }
        SettingsSelectField::RenderScale => &[(0.65, "流畅"), (0.8, "平衡"), (1.0, "清晰")],
        SettingsSelectField::TextureQuality => &[(2.0, "低"), (1.0, "中"), (0.0, "好")],
        SettingsSelectField::TargetFps => &[
            (0.0, "不限"),
            (30.0, "30 FPS"),
            (60.0, "60 FPS"),
            (120.0, "120 FPS"),
        ],
        SettingsSelectField::UiScale => &[
            (0.0, "自动"),
            (0.7, "70%"),
            (0.85, "85%"),
            (1.0, "100%"),
            (1.2, "120%"),
        ],
        SettingsSelectField::TouchControls => &[(-1.0, "自动"), (1.0, "开"), (0.0, "关")],
    };
    options
        .iter()
        .map(|(value, label)| (*value, (*label).into()))
        .collect()
}

fn read_select(settings: &Settings, field: SettingsSelectField) -> f32 {
    match field {
        SettingsSelectField::WindowResolution => settings.window_resolution as f32,
        SettingsSelectField::GraphicsAdapter | SettingsSelectField::GraphicsBackend => {
            crate::graphics_settings::selected(settings, field)
        }
        SettingsSelectField::RenderScale => settings.render_scale,
        SettingsSelectField::TextureQuality => settings.texture_quality as f32,
        SettingsSelectField::TargetFps => settings.target_fps as f32,
        SettingsSelectField::UiScale => settings.ui_scale,
        SettingsSelectField::TouchControls => settings.touch_controls as f32,
    }
}

#[derive(Component)]
pub(crate) struct SettingsSelectOption {
    field: SettingsSelectField,
    value: f32,
}

#[derive(Component)]
pub(crate) struct SettingsOptionLabel {
    field: SettingsSelectField,
    value: f32,
    name: String,
}

fn graphics_field(field: SettingsSelectField) -> bool {
    matches!(
        field,
        SettingsSelectField::GraphicsAdapter | SettingsSelectField::GraphicsBackend
    )
}

fn option_status(
    settings: &Settings,
    field: SettingsSelectField,
    value: f32,
    name: &str,
    adapter: Option<&bevy::render::renderer::RenderAdapterInfo>,
) -> (String, bool) {
    let selected = (read_select(settings, field) - value).abs() < 0.01;
    let reason = if graphics_field(field) {
        crate::graphics_settings::unavailable_reason(settings, field, value)
    } else {
        None
    };
    let mut marks = Vec::new();
    if selected {
        marks.push("已选择");
    }
    if let Some(adapter) = adapter {
        let backend = crate::graphics_settings::backend_label(adapter.backend);
        if (field == SettingsSelectField::GraphicsAdapter && name == adapter.name)
            || (field == SettingsSelectField::GraphicsBackend && name == backend)
        {
            marks.push("当前使用");
        }
    }
    marks.push(if reason.is_some() {
        "不可选"
    } else {
        "可选"
    });
    let mut label = format!("{}\n{}", name, marks.join(" · "));
    if let Some(reason) = reason {
        label.push_str(&format!("：{reason}"));
    }
    (label, reason.is_some())
}

pub(crate) fn spawn_settings_select_dialogs(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    settings: &Settings,
) {
    use crate::plugins::ui_components::*;
    for (field, title) in [
        (
            SettingsSelectField::WindowResolution,
            "窗口分辨率（立即生效）",
        ),
        (SettingsSelectField::GraphicsAdapter, "显卡（重启后生效）"),
        (
            SettingsSelectField::GraphicsBackend,
            "图形后端（重启后生效）",
        ),
        (SettingsSelectField::RenderScale, "画面解析"),
        (SettingsSelectField::TextureQuality, "贴图品质"),
        (SettingsSelectField::TargetFps, "帧率上限"),
        (SettingsSelectField::UiScale, "界面缩放"),
        (SettingsSelectField::TouchControls, "触屏控件"),
    ] {
        spawn_select_dialog(parent, font, select_id(field), title, |menu| {
            if graphics_field(field) {
                menu.spawn((
                    launcher_text("正在读取当前渲染设备…", 13.0, theme::MUTED, font),
                    crate::graphics_settings::CurrentRendererLabel,
                ));
                menu.spawn(launcher_text(
                    "已选择的配置将在重启后生效。",
                    12.0,
                    theme::MUTED,
                    font,
                ));
            }
            let mut choices = select_options(field);
            if graphics_field(field) && read_select(settings, field) == -1.0 {
                choices.push((-1.0, crate::graphics_settings::label(settings, field)));
            }
            for (value, label) in choices {
                let selected = (value - read_select(settings, field)).abs() < 0.01;
                let (caption, disabled) = option_status(settings, field, value, &label, None);
                let style = UiSelectableStyle::default();
                let mut button = menu.spawn((
                    UiSelectableButtonBundle::new(
                        Node {
                            width: Val::Percent(100.0),
                            min_height: Val::Px(44.0),
                            flex_shrink: 0.0,
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(10.0),
                            padding: UiRect::horizontal(Val::Px(12.0)),
                            border: UiRect::all(Val::Px(1.0)),
                            ..default()
                        },
                        selected,
                        UiSelectableKind::Radio,
                        style,
                    ),
                    LauncherStyledButton,
                    UiSelectOption(select_id(field)),
                    SettingsSelectOption { field, value },
                ));
                if disabled {
                    button.insert(UiSelectDisabled);
                }
                button.with_children(|option| {
                    spawn_selection_indicator(option, UiSelectableKind::Radio, selected);
                    option.spawn((
                        launcher_text(
                            &caption,
                            14.0,
                            if disabled { theme::MUTED } else { theme::TEXT },
                            font,
                        ),
                        Node {
                            min_width: Val::Px(0.0),
                            flex_grow: 1.0,
                            flex_basis: Val::Px(0.0),
                            ..default()
                        },
                        SettingsOptionLabel {
                            field,
                            value,
                            name: label,
                        },
                    ));
                });
            }
        });
    }
}

fn select_label(settings: &Settings, field: SettingsSelectField) -> String {
    match field {
        SettingsSelectField::WindowResolution => {
            crate::window_settings::resolution_label(settings.window_resolution).into()
        }
        SettingsSelectField::GraphicsAdapter | SettingsSelectField::GraphicsBackend => {
            crate::graphics_settings::label(settings, field)
        }
        SettingsSelectField::RenderScale => render_scale_label(settings.render_scale).into(),
        SettingsSelectField::TextureQuality => {
            texture_quality_label(settings.texture_quality).into()
        }
        SettingsSelectField::TargetFps => target_fps_label(settings.target_fps).into(),
        SettingsSelectField::UiScale => ui_scale_label(settings.ui_scale).into(),
        SettingsSelectField::TouchControls => touch_controls_label(settings.touch_controls).into(),
    }
}

pub fn settings_bool_buttons(
    query: Query<(&Interaction, &SettingsBoolField), (Changed<Interaction>, With<Button>)>,
    keymap: Res<Keymap>,
    mut settings: ResMut<Settings>,
) {
    for (interaction, field) in &query {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if matches!(field, SettingsBoolField::HitStop) && !settings.hit_effects {
            continue;
        }
        let next = !read_bool(&settings, *field);
        write_bool(&mut settings, *field, next);
        let _ = save_settings(&settings, &keymap);
    }
}

pub(crate) fn settings_select_buttons(
    mut commands: Commands,
    query: Query<(&Interaction, &SettingsSelectOption), (Changed<Interaction>, With<Button>)>,
    mut options: Query<(
        Entity,
        &SettingsSelectOption,
        &mut crate::plugins::ui_components::UiSelectionState,
        Option<&crate::plugins::ui_components::UiSelectDisabled>,
    )>,
    mut labels: Query<(&SettingsOptionLabel, &mut Text, &mut TextColor)>,
    adapter: Option<Res<bevy::render::renderer::RenderAdapterInfo>>,
    keymap: Res<Keymap>,
    mut settings: ResMut<Settings>,
) {
    for (interaction, option) in &query {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if graphics_field(option.field)
            && crate::graphics_settings::unavailable_reason(&settings, option.field, option.value)
                .is_some()
        {
            continue;
        }
        match option.field {
            SettingsSelectField::WindowResolution => {
                settings.window_resolution = option.value as u8
            }
            SettingsSelectField::GraphicsAdapter | SettingsSelectField::GraphicsBackend => {
                crate::graphics_settings::select(&mut settings, option.field, option.value)
            }
            SettingsSelectField::RenderScale => settings.render_scale = option.value,
            SettingsSelectField::TextureQuality => settings.texture_quality = option.value as u8,
            SettingsSelectField::TargetFps => settings.target_fps = option.value as u32,
            SettingsSelectField::UiScale => settings.ui_scale = option.value,
            SettingsSelectField::TouchControls => settings.touch_controls = option.value as i8,
        }
        let _ = save_settings(&settings, &keymap);
    }
    for (entity, option, mut state, disabled) in &mut options {
        state.selected = (read_select(&settings, option.field) - option.value).abs() < 0.01;
        if graphics_field(option.field)
            && crate::graphics_settings::unavailable_reason(&settings, option.field, option.value)
                .is_some()
        {
            if disabled.is_none() {
                commands
                    .entity(entity)
                    .insert(crate::plugins::ui_components::UiSelectDisabled);
            }
        } else if disabled.is_some() {
            commands
                .entity(entity)
                .remove::<crate::plugins::ui_components::UiSelectDisabled>();
        }
    }
    for (option, mut text, mut color) in &mut labels {
        let (caption, disabled) = option_status(
            &settings,
            option.field,
            option.value,
            &option.name,
            adapter.as_deref(),
        );
        if text.0 != caption {
            text.0 = caption;
        }
        color.0 = if disabled { theme::MUTED } else { theme::TEXT };
    }
}

pub fn settings_range_down_buttons(
    query: Query<(&Interaction, &SettingsRangeDown), (Changed<Interaction>, With<Button>)>,
    keymap: Res<Keymap>,
    mut settings: ResMut<Settings>,
) {
    for (interaction, btn) in &query {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let field = btn.0;
        let (min, _, step) = range_bounds(field);
        let next = (read_range(&settings, field) - step).max(min);
        write_range(&mut settings, field, next);
        let _ = save_settings(&settings, &keymap);
    }
}

pub fn settings_range_up_buttons(
    query: Query<(&Interaction, &SettingsRangeUp), (Changed<Interaction>, With<Button>)>,
    keymap: Res<Keymap>,
    mut settings: ResMut<Settings>,
) {
    for (interaction, btn) in &query {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let field = btn.0;
        let (_, max, step) = range_bounds(field);
        let next = (read_range(&settings, field) + step).min(max);
        write_range(&mut settings, field, next);
        let _ = save_settings(&settings, &keymap);
    }
}

pub fn settings_reset_button(
    query: Query<&Interaction, (Changed<Interaction>, With<SettingsResetButton>)>,
    mut settings: ResMut<Settings>,
    mut keymap: ResMut<Keymap>,
    mut capture: ResMut<KeyCaptureState>,
) {
    if query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        capture.clear();
        reset_settings_defaults(&mut settings, &mut keymap);
    }
}

pub(crate) fn refresh_settings_form_labels(
    settings: Res<Settings>,
    mut hit_options: Query<&mut Node, With<HitEffectsOptions>>,
    mut bools: Query<(&SettingsBoolValue, &mut Text)>,
    mut choices: Query<(
        &SettingsBoolField,
        &mut crate::plugins::ui_components::UiSelectionState,
    )>,
    mut selects: Query<(&SettingsSelectValue, &mut Text), Without<SettingsBoolValue>>,
    mut ranges: Query<
        (&SettingsRangeValue, &mut Text),
        (Without<SettingsBoolValue>, Without<SettingsSelectValue>),
    >,
) {
    if !settings.is_changed() {
        return;
    }
    for mut node in &mut hit_options {
        node.display = if settings.hit_effects {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (field, mut state) in &mut choices {
        state.selected = read_bool(&settings, *field);
    }
    for (field, mut text) in &mut bools {
        text.0 = bool_label(read_bool(&settings, field.0)).into();
    }
    for (field, mut text) in &mut selects {
        text.0 = select_label(&settings, field.0);
    }
    for (field, mut text) in &mut ranges {
        text.0 = format_range_value(field.0, read_range(&settings, field.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_option_captions_explicitly_show_selection_and_availability() {
        let settings = Settings::default();
        let (selected, disabled) = option_status(
            &settings,
            SettingsSelectField::GraphicsAdapter,
            0.0,
            "自动",
            None,
        );
        assert_eq!(selected, "自动\n已选择 · 可选");
        assert!(!disabled);
        let (other, disabled) = option_status(
            &settings,
            SettingsSelectField::TargetFps,
            60.0,
            "60 FPS",
            None,
        );
        assert_eq!(other, "60 FPS\n可选");
        assert!(!disabled);
    }

    #[test]
    fn settings_form_has_six_real_sliders_visible_signs_and_cancelable_selection() {
        use crate::plugins::ui_components::slider::UiSlider;
        use crate::plugins::ui_components::{UiComponentsPlugin, UiSelectMenu, UiSelectToggle};
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.add_plugins(UiComponentsPlugin)
            .init_resource::<Settings>()
            .init_resource::<Keymap>()
            .init_resource::<KeyCaptureState>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(
                Update,
                (
                    settings_category_buttons,
                    settings_select_buttons,
                    refresh_settings_form_labels,
                )
                    .in_set(crate::plugins::UiTouchActionSet),
            );
        app.world_mut()
            .run_system_once(|mut commands: Commands| {
                commands
                    .spawn(Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    })
                    .with_children(|root| {
                        spawn_web_settings_form(
                            root,
                            &Handle::default(),
                            &Settings::default(),
                            &Keymap::default(),
                        );
                        spawn_settings_select_dialogs(
                            root,
                            &Handle::default(),
                            &Settings::default(),
                        );
                    });
            })
            .unwrap();
        assert_eq!(
            app.world_mut()
                .query::<&UiSlider>()
                .iter(app.world())
                .count(),
            6
        );
        for (up, down, children, node) in app
            .world_mut()
            .query::<(
                Option<&SettingsRangeUp>,
                Option<&SettingsRangeDown>,
                &Children,
                &Node,
            )>()
            .iter(app.world())
        {
            if up.is_none() && down.is_none() {
                continue;
            }
            assert_eq!(node.flex_shrink, 0.0);
            assert_eq!(node.width, Val::Px(44.0));
            assert_eq!(children.len(), if up.is_some() { 2 } else { 1 });
            assert!(
                children
                    .iter()
                    .all(|child| app.world().get::<Text>(child).is_none()),
                "signs must not depend on font glyphs"
            );
        }
        for enabled in [false, true, false] {
            app.world_mut().resource_mut::<Settings>().hit_effects = enabled;
            app.update();
            let node = app
                .world_mut()
                .query_filtered::<&Node, With<HitEffectsOptions>>()
                .single(app.world())
                .unwrap();
            assert_eq!(
                node.display,
                if enabled {
                    Display::Flex
                } else {
                    Display::None
                }
            );
        }
        for (category, _) in SettingsCategory::ALL {
            let button = app
                .world_mut()
                .query::<(Entity, &SettingsCategoryButton)>()
                .iter(app.world())
                .find(|(_, tab)| tab.category == category)
                .unwrap()
                .0;
            *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::Pressed;
            app.update();
            let visible: Vec<_> = app
                .world_mut()
                .query::<(&SettingsCategoryPage, &Node)>()
                .iter(app.world())
                .filter(|(_, node)| node.display != Display::None)
                .map(|(page, _)| page.category)
                .collect();
            assert!(
                visible == vec![category],
                "only the selected category occupies layout"
            );
            *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::None;
        }
        let display_tab = app
            .world_mut()
            .query::<(Entity, &SettingsCategoryButton)>()
            .iter(app.world())
            .find(|(_, tab)| tab.category == SettingsCategory::Display)
            .unwrap()
            .0;
        *app.world_mut().get_mut::<Interaction>(display_tab).unwrap() = Interaction::Pressed;
        app.update();
        let toggle = app
            .world_mut()
            .query::<(Entity, &UiSelectToggle)>()
            .iter(app.world())
            .find(|(_, toggle)| toggle.0 == select_id(SettingsSelectField::UiScale))
            .unwrap()
            .0;
        let original = app.world().resource::<Settings>().ui_scale;
        *app.world_mut().get_mut::<Interaction>(toggle).unwrap() = Interaction::Pressed;
        app.update();
        assert_eq!(
            app.world().resource::<Settings>().ui_scale,
            original,
            "opening is not cycling a setting"
        );
        assert_eq!(
            app.world_mut()
                .query_filtered::<&Node, With<UiSelectMenu>>()
                .iter(app.world())
                .filter(|node| node.display != Display::None)
                .count(),
            1
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        assert_eq!(app.world().resource::<Settings>().ui_scale, original);
        assert!(app
            .world_mut()
            .query_filtered::<&Node, With<UiSelectMenu>>()
            .iter(app.world())
            .all(|node| node.display == Display::None));
        assert!(!app
            .world()
            .resource::<ButtonInput<KeyCode>>()
            .just_pressed(KeyCode::Escape));
    }

    #[test]
    fn mobile_settings_use_two_section_columns_in_landscape_and_one_in_portrait() {
        let mut app = App::new();
        app.add_systems(Update, adapt_settings_layout);
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: (872_u32, 393_u32).into(),
                ..default()
            })
            .id();
        app.world_mut().spawn((Node::default(), SettingsFormRoot));
        let regular = app
            .world_mut()
            .spawn((Node::default(), SettingsFieldset { full_width: false }))
            .id();
        let full_width = app
            .world_mut()
            .spawn((Node::default(), SettingsFieldset { full_width: true }))
            .id();

        app.update();
        assert_eq!(
            app.world().get::<Node>(regular).unwrap().width,
            Val::Percent(48.0)
        );
        assert_eq!(
            app.world().get::<Node>(full_width).unwrap().width,
            Val::Percent(100.0)
        );

        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution = (412_u32, 915_u32).into();
        app.update();
        assert_eq!(
            app.world().get::<Node>(regular).unwrap().width,
            Val::Percent(100.0)
        );
    }
}
