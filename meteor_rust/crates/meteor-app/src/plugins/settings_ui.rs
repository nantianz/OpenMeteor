#![allow(clippy::type_complexity)]

use super::ui_theme as theme;
use crate::app_state::LauncherStyledButton;
use crate::app_state::{
    reset_settings_defaults, save_settings, KeyCaptureButton, KeyCaptureState, KeyField, Keymap,
    Settings, SettingsBoolField, SettingsFormRoot, SettingsRangeDown, SettingsRangeField,
    SettingsRangeUp, SettingsRangeValue, SettingsResetButton, SettingsSelectField,
};
use crate::plugins::launcher_ui::{launcher_text, responsive_ui_layout};
use crate::plugins::mobile_controls_plugin::{
    mobile_layout_editor_available, OpenMobileLayoutEditorButton,
};
use bevy::prelude::*;
use bevy::ui::FocusPolicy;

pub(crate) const GOLD: Color = theme::METAL;

const ROW_H: f32 = 46.0;
const TOUCH_ROW_H: f32 = 48.0;
const ARROW_HIT: f32 = 44.0;
const CONTENT_MAX_W: f32 = 960.0;

#[derive(Component)]
pub(crate) struct SettingsKeyRow;

#[derive(Component)]
pub(crate) struct SettingsStepButton;

#[derive(Component)]
pub(crate) struct HitEffectsOptions;

#[derive(Component, Clone)]
pub(crate) struct SettingsRow {
    pub(crate) hint: String,
}

#[derive(Component)]
pub(crate) struct SettingsValueColumn {
    range: bool,
}

#[derive(Component)]
pub(crate) struct SettingsRangeRow;

#[derive(Component, Clone, Copy)]
pub(crate) struct SettingsCycle {
    field: SettingsSelectField,
    step: i8,
}

#[derive(Component, Clone, PartialEq, Eq, Debug)]
pub(crate) enum PackToggle {
    Master,
    Pack(String),
}

#[derive(Component, Clone)]
pub(crate) struct SettingsPackValue(PackToggle);

#[derive(Component)]
pub(crate) struct SettingsPackList;

#[derive(Component)]
pub(crate) struct SettingsHintText;

#[derive(Component)]
pub(crate) struct SettingsRestartNotice;

#[derive(Component)]
pub(crate) struct SettingsKeyHint;

#[derive(Component)]
pub(crate) struct SettingsChrome {
    footer: bool,
}

#[derive(Component)]
pub(crate) struct SettingsTitle;

#[derive(Component)]
pub(crate) struct SettingsScrollTrack {
    root: Entity,
}
#[derive(Component)]
pub(crate) struct SettingsScrollThumb {
    root: Entity,
}

const DEFAULT_HINT: &str = "将指针移到选项上查看说明。";

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SettingsCategory {
    Graphics,
    Enhance,
    Display,
    Renderer,
    Audio,
    Camera,
    Controls,
}

impl SettingsCategory {
    pub(crate) const ALL: [(Self, &'static str); 7] = [
        (Self::Graphics, "画质"),
        (Self::Enhance, "增强"),
        (Self::Display, "显示"),
        (Self::Renderer, "设备"),
        (Self::Audio, "声音"),
        (Self::Camera, "镜头"),
        (Self::Controls, "操作"),
    ];

    fn available(self) -> bool {
        self != Self::Renderer || crate::graphics_settings::AVAILABLE
    }
}

#[derive(Component)]
pub(crate) struct SettingsCategoryButton {
    root: Entity,
    pub(crate) category: SettingsCategory,
    selected: bool,
}
#[derive(Component)]
pub(crate) struct SettingsCategoryLabel;
#[derive(Component)]
pub(crate) struct SettingsCategoryPage {
    root: Entity,
    pub(crate) category: SettingsCategory,
}
#[derive(Component)]
pub(crate) struct SettingsCategoryNav;
#[derive(Component)]
pub(crate) struct SettingsCategoryContent;

pub(crate) struct SettingsUiPlugin;

impl Plugin for SettingsUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                settings_cycle_buttons,
                settings_pack_buttons,
                fill_pack_lists,
                settings_row_feedback,
                settings_nav_feedback,
                settings_hints,
                settings_restart_notice,
                settings_scrollbars,
            )
                .in_set(crate::plugins::UiTouchActionSet),
        );
    }
}

pub fn spawn_web_settings_form(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    settings: &Settings,
    keymap: &Keymap,
    in_game: bool,
) {
    let mut form = parent.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            min_height: Val::Px(0.0),
            column_gap: Val::Px(28.0),
            ..default()
        },
        SettingsFormRoot,
    ));
    let root = form.id();
    form.with_children(|body| {
        body.spawn((
            crate::plugins::launcher_ui::scrollable_y(Node {
                width: Val::Percent(16.0),
                min_width: Val::Px(140.0),
                max_width: Val::Px(220.0),
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                align_content: AlignContent::FlexStart,
                row_gap: Val::Px(2.0),
                padding: UiRect::right(Val::Px(14.0)),
                border: UiRect::right(Val::Px(1.0)),
                ..default()
            }),
            BorderColor::all(theme::LINE.with_alpha(0.16)),
            SettingsCategoryNav,
        ))
        .with_children(|nav| {
            for (category, label) in SettingsCategory::ALL {
                if !category.available() {
                    continue;
                }
                let selected = category == SettingsCategory::Graphics;
                nav.spawn((
                    Button,
                    Node {
                        width: Val::Percent(100.0),
                        min_height: Val::Px(46.0),
                        flex_shrink: 0.0,
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::Center,
                        padding: UiRect::horizontal(Val::Px(12.0)),
                        ..default()
                    },
                    BackgroundColor(Color::NONE),
                    nav_gradient(selected, false),
                    LauncherStyledButton,
                    SettingsCategoryButton {
                        root,
                        category,
                        selected,
                    },
                ))
                .with_children(|button| {
                    button.spawn((
                        launcher_text(
                            label,
                            17.0,
                            if selected {
                                theme::ACCENT_TEXT
                            } else {
                                theme::MUTED
                            },
                            font,
                        ),
                        SettingsCategoryLabel,
                    ));
                });
            }
        });
        body.spawn((
            Node {
                width: Val::Px(0.0),
                flex_grow: 1.0,
                max_width: Val::Px(CONTENT_MAX_W),
                min_width: Val::Px(0.0),
                min_height: Val::Px(0.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(10.0),
                ..default()
            },
            SettingsCategoryContent,
        ))
        .with_children(|content| {
            content
                .spawn(Node {
                    flex_grow: 1.0,
                    flex_basis: Val::Px(0.0),
                    min_width: Val::Px(0.0),
                    height: Val::Percent(100.0),
                    ..default()
                })
                .with_children(|pages| {
                    for (category, _) in SettingsCategory::ALL {
                        if !category.available() {
                            continue;
                        }
                        pages
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
                                    padding: UiRect::bottom(Val::Px(12.0)),
                                    ..default()
                                }),
                                SettingsCategoryPage { root, category },
                            ))
                            .with_children(|page| match category {
                                SettingsCategory::Graphics => {
                                    spawn_graphics_page(page, font, settings, in_game)
                                }
                                SettingsCategory::Enhance => {
                                    spawn_enhance_page(page, font, settings)
                                }
                                SettingsCategory::Renderer => {
                                    spawn_renderer_page(page, font, settings)
                                }
                                SettingsCategory::Display => {
                                    spawn_display_page(page, font, settings)
                                }
                                SettingsCategory::Audio => spawn_audio_page(page, font, settings),
                                SettingsCategory::Camera => spawn_camera_page(page, font, settings),
                                SettingsCategory::Controls => {
                                    spawn_controls_page(page, font, settings, keymap)
                                }
                            });
                    }
                });
            content
                .spawn((
                    Node {
                        width: Val::Px(3.0),
                        height: Val::Percent(100.0),
                        flex_shrink: 0.0,
                        ..default()
                    },
                    BackgroundColor(theme::LINE.with_alpha(0.07)),
                    Visibility::Hidden,
                    SettingsScrollTrack { root },
                ))
                .with_children(|track| {
                    track.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            width: Val::Percent(100.0),
                            top: Val::Percent(0.0),
                            height: Val::Percent(100.0),
                            ..default()
                        },
                        BackgroundColor(GOLD.with_alpha(0.55)),
                        SettingsScrollThumb { root },
                    ));
                });
        });
    });
}

fn nav_gradient(selected: bool, hovered: bool) -> BackgroundGradient {
    let peak = match (selected, hovered) {
        (true, _) => GOLD.with_alpha(0.20),
        (false, true) => GOLD.with_alpha(0.08),
        (false, false) => return BackgroundGradient::default(),
    };
    BackgroundGradient(vec![LinearGradient::new(
        LinearGradient::TO_RIGHT,
        vec![
            ColorStop::new(Color::NONE, Val::Percent(0.0)),
            ColorStop::new(peak, Val::Percent(50.0)),
            ColorStop::new(Color::NONE, Val::Percent(100.0)),
        ],
    )
    .into()])
}

fn row_gradient(active: bool) -> BackgroundGradient {
    if !active {
        return BackgroundGradient::default();
    }
    BackgroundGradient(vec![LinearGradient::new(
        LinearGradient::TO_RIGHT,
        vec![
            ColorStop::new(GOLD.with_alpha(0.16), Val::Percent(0.0)),
            ColorStop::new(GOLD.with_alpha(0.06), Val::Percent(60.0)),
            ColorStop::new(GOLD.with_alpha(0.02), Val::Percent(100.0)),
        ],
    )
    .into()])
}

pub(crate) fn settings_category_buttons(
    mut tabs: Query<(Entity, Ref<Interaction>, &mut SettingsCategoryButton)>,
    mut pages: Query<(&SettingsCategoryPage, &mut Node, &mut ScrollPosition)>,
    mut capture: ResMut<KeyCaptureState>,
) {
    let pressed: Vec<(Entity, Entity, SettingsCategory)> = tabs
        .iter()
        .filter(|(_, interaction, _)| {
            interaction.is_changed() && **interaction == Interaction::Pressed
        })
        .map(|(entity, _, tab)| (entity, tab.root, tab.category))
        .collect();
    for (pressed, root, category) in pressed {
        capture.pending = None;
        capture.arming = false;
        for (page, mut node, mut scroll) in &mut pages {
            if page.root != root {
                continue;
            }
            node.display = if page.category == category {
                Display::Flex
            } else {
                Display::None
            };
            *scroll = ScrollPosition::default();
        }
        for (entity, _, mut tab) in &mut tabs {
            if tab.root == root && tab.selected != (entity == pressed) {
                tab.selected = entity == pressed;
            }
        }
    }
}

fn settings_nav_feedback(
    mut tabs: Query<
        (
            &SettingsCategoryButton,
            &Interaction,
            &Children,
            &mut BackgroundGradient,
        ),
        Or<(Changed<Interaction>, Changed<SettingsCategoryButton>)>,
    >,
    mut labels: Query<(&mut TextColor, &mut TextFont), With<SettingsCategoryLabel>>,
) {
    for (tab, interaction, children, mut gradient) in &mut tabs {
        let hovered = *interaction != Interaction::None;
        *gradient = nav_gradient(tab.selected, hovered);
        for child in children {
            if let Ok((mut color, mut font)) = labels.get_mut(*child) {
                color.0 = if tab.selected {
                    theme::ACCENT_TEXT
                } else if hovered {
                    theme::TEXT
                } else {
                    theme::MUTED
                };
                font.font_size = FontSize::Px(if tab.selected { 18.0 } else { 17.0 });
            }
        }
    }
}

fn settings_row_feedback(
    mut rows: Query<
        (&Interaction, &mut BackgroundGradient, &mut BorderColor),
        (With<SettingsRow>, Changed<Interaction>),
    >,
) {
    for (interaction, mut gradient, mut border) in &mut rows {
        let active = *interaction != Interaction::None;
        *gradient = row_gradient(active);
        *border = if active {
            BorderColor::all(GOLD.with_alpha(0.55))
        } else {
            BorderColor {
                bottom: theme::LINE.with_alpha(0.07),
                ..BorderColor::all(Color::NONE)
            }
        };
    }
}

fn settings_hints(
    rows: Query<(&Interaction, &SettingsRow)>,
    mut texts: Query<&mut Text, With<SettingsHintText>>,
) {
    let hint = rows
        .iter()
        .find(|(interaction, _)| **interaction != Interaction::None)
        .map_or(DEFAULT_HINT, |(_, row)| row.hint.as_str());
    for mut text in &mut texts {
        if text.0 != hint {
            text.0 = hint.to_owned();
        }
    }
}

pub(crate) fn restart_pending(
    settings: &Settings,
    startup: Option<&crate::app_state::StartupSettings>,
    catalog: Option<&crate::plugins::graphics_pack::GraphicsPackCatalog>,
) -> bool {
    catalog.is_some_and(|catalog| catalog.restart_pending(settings))
        || startup.is_some_and(|startup| {
            let start = &startup.0;
            start.graphics_adapter != settings.graphics_adapter
                || start.graphics_backend != settings.graphics_backend
                || start.water_live_reflections != settings.water_live_reflections
                || start.dlss5_neural_rendering != settings.dlss5_neural_rendering
        })
}

fn settings_restart_notice(
    settings: Res<Settings>,
    startup: Option<Res<crate::app_state::StartupSettings>>,
    catalog: Option<Res<crate::plugins::graphics_pack::GraphicsPackCatalog>>,
    mut notices: Query<&mut Node, With<SettingsRestartNotice>>,
) {
    let pending = restart_pending(&settings, startup.as_deref(), catalog.as_deref());
    let display = if pending {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in &mut notices {
        if node.display != display {
            node.display = display;
        }
    }
}

fn settings_scrollbars(
    pages: Query<(&SettingsCategoryPage, &Node, &ComputedNode, &ScrollPosition)>,
    mut tracks: Query<(&SettingsScrollTrack, &mut Visibility)>,
    mut thumbs: Query<(&SettingsScrollThumb, &mut Node), Without<SettingsCategoryPage>>,
) {
    for (track, mut visibility) in &mut tracks {
        let page = pages
            .iter()
            .find(|(page, node, ..)| page.root == track.root && node.display != Display::None);
        let Some((_, _, computed, scroll)) = page else {
            continue;
        };
        let viewport = computed.size().y;
        let content = computed.content_size().y;
        let overflow = content - viewport;
        let wanted = if viewport > 0.0 && overflow > 1.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        visibility.set_if_neq(wanted);
        if wanted == Visibility::Hidden {
            continue;
        }
        let offset = (scroll.y / computed.inverse_scale_factor.max(1e-6)).clamp(0.0, overflow);
        let size = (viewport / content).clamp(0.08, 1.0);
        let top = (offset / overflow) * (1.0 - size);
        for (thumb, mut node) in &mut thumbs {
            if thumb.root == track.root {
                node.top = Val::Percent(top * 100.0);
                node.height = Val::Percent(size * 100.0);
            }
        }
    }
}

fn spawn_graphics_page(
    page: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    settings: &Settings,
    in_game: bool,
) {
    spawn_section(page, font, "画质与性能", None);
    spawn_select_row(
        page,
        font,
        "画面解析",
        SettingsSelectField::RenderScale,
        render_scale_label(settings.render_scale),
        "降低内部渲染分辨率可提升帧率。“清晰”为原生分辨率。",
        false,
    );
    spawn_select_row(
        page,
        font,
        "贴图品质",
        SettingsSelectField::TextureQuality,
        texture_quality_label(settings.texture_quality),
        "降低贴图分辨率以减少显存占用。",
        false,
    );
    spawn_select_row(
        page,
        font,
        "帧率上限",
        SettingsSelectField::TargetFps,
        target_fps_label(settings.target_fps),
        "限制最高帧率以降低功耗与发热。",
        false,
    );
    spawn_check_row(
        page,
        font,
        "高清场景",
        SettingsBoolField::HdScene,
        settings.hd_scene,
        "为支持的关卡使用高清贴图，进入关卡时生效。",
        false,
    );
    spawn_check_row(
        page,
        font,
        "动态阴影",
        SettingsBoolField::DynamicShadows,
        settings.dynamic_shadows,
        "人物与场景投射实时阴影。",
        false,
    );
    #[cfg(all(
        feature = "dlss5",
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc"
    ))]
    spawn_check_row(
        page,
        font,
        "DLSS 5 神经渲染（实验）",
        SettingsBoolField::Dlss5NeuralRendering,
        settings.dlss5_neural_rendering,
        "仅 NVIDIA Vulkan 64 位版本可用。",
        true,
    );

    spawn_section(page, font, "光影与环境", None);
    spawn_check_row(
        page,
        font,
        "环境光与人物明暗",
        SettingsBoolField::EnvironmentLighting,
        settings.environment_lighting,
        "为原版材质加入环境补光和方向明暗。",
        false,
    );
    spawn_check_row(
        page,
        font,
        "体积雾",
        SettingsBoolField::VolumetricFog,
        settings.volumetric_fog,
        "显示增强包中的局部雾气。",
        false,
    );
    spawn_check_row(
        page,
        font,
        "墙面与地面凹凸",
        SettingsBoolField::SceneNormalMaps,
        settings.scene_normal_maps,
        "为墙面与地面加入法线细节。",
        false,
    );
    spawn_check_row(
        page,
        font,
        "水面增强",
        SettingsBoolField::WaterEnhance,
        settings.water_enhance,
        "水面反光、折射，以及人物走过时的涟漪。",
        false,
    );
    if !in_game && !cfg!(any(target_arch = "wasm32", target_os = "android")) {
        spawn_check_row(
            page,
            font,
            "水面实时倒影",
            SettingsBoolField::WaterLiveReflections,
            settings.water_live_reflections,
            "水面实时映出人物和场景，帧耗较高。",
            true,
        );
    }
}

fn spawn_enhance_page(page: &mut ChildSpawnerCommands, font: &Handle<Font>, settings: &Settings) {
    spawn_section(page, font, "图形增强包", Some("重启后生效"));
    page.spawn((
        Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            ..default()
        },
        SettingsPackList,
    ));

    spawn_section(page, font, "场景与特效", None);
    if !cfg!(any(target_arch = "wasm32", target_os = "android")) {
        spawn_check_row(
            page,
            font,
            "天空增强",
            SettingsBoolField::SkyEnhance,
            settings.sky_enhance,
            "体积云与昼夜天空，下一局生效。",
            false,
        );
    }
    spawn_check_row(
        page,
        font,
        "可交互草地",
        SettingsBoolField::GrassEnhance,
        settings.grass_enhance,
        "增强包中的草地会被人物踩倒并回弹。",
        false,
    );
    spawn_check_row(
        page,
        font,
        "刀光与特效增强",
        SettingsBoolField::EffectEnhance,
        settings.effect_enhance,
        "使用增强包中的刀光、火花与闪光。原版判定不变。",
        false,
    );

    spawn_section(page, font, "打击反馈", None);
    spawn_check_row(
        page,
        font,
        "武器击打特效",
        SettingsBoolField::HitEffects,
        settings.hit_effects,
        "命中时显示火花、震屏等反馈。",
        false,
    );
    page.spawn((
        Node {
            display: if settings.hit_effects {
                Display::Flex
            } else {
                Display::None
            },
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            padding: UiRect::left(Val::Px(24.0)),
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
            "命中瞬间短暂停顿，仅单机生效。",
            false,
        );
        spawn_check_row(
            options,
            font,
            "打击震屏",
            SettingsBoolField::HitShake,
            settings.hit_shake,
            "本地玩家参与的命中会让镜头震动。",
            false,
        );
    });
    spawn_check_row(
        page,
        font,
        "受击轻微摆动",
        SettingsBoolField::HitSway,
        settings.hit_sway,
        "受击时身体轻微晃动，不影响判定。",
        false,
    );

    spawn_section(page, font, "人物", None);
    spawn_check_row(
        page,
        font,
        "使用新人物模型",
        SettingsBoolField::NewCharacterModels,
        settings.new_character_models,
        "使用重制的人物外观，战斗骨骼不变。",
        false,
    );
    spawn_check_row(
        page,
        font,
        "角色衣物摆动",
        SettingsBoolField::CharacterMotion,
        settings.character_motion,
        "衣物与头发随动作和风摆动。",
        false,
    );
}

fn spawn_renderer_page(page: &mut ChildSpawnerCommands, font: &Handle<Font>, settings: &Settings) {
    spawn_section(page, font, "窗口", None);
    spawn_select_row(
        page,
        font,
        "窗口分辨率",
        SettingsSelectField::WindowResolution,
        crate::window_settings::resolution_label(settings.window_resolution),
        "窗口模式下的分辨率，立即生效。",
        false,
    );
    spawn_check_row(
        page,
        font,
        "全屏模式",
        SettingsBoolField::Fullscreen,
        settings.fullscreen,
        "全屏使用桌面分辨率，退出后恢复窗口尺寸。",
        false,
    );
    spawn_check_row(
        page,
        font,
        "无边框窗口",
        SettingsBoolField::BorderlessWindow,
        settings.borderless_window,
        "仅影响窗口模式。",
        false,
    );
    spawn_section(page, font, "渲染设备", Some("重启后生效"));
    for (field, title, hint) in [
        (
            SettingsSelectField::GraphicsAdapter,
            "显卡",
            "不兼容的显卡与后端组合会自动改选显卡。",
        ),
        (
            SettingsSelectField::GraphicsBackend,
            "图形后端",
            "切换 Vulkan、DX12、Metal 等图形接口。",
        ),
    ] {
        spawn_select_row(
            page,
            font,
            title,
            field,
            &crate::graphics_settings::label(settings, field),
            hint,
            true,
        );
    }
    page.spawn((
        launcher_text("", 12.0, theme::MUTED, font),
        Node {
            margin: UiRect::new(Val::Px(16.0), Val::ZERO, Val::Px(8.0), Val::ZERO),
            ..default()
        },
        crate::graphics_settings::CurrentRendererLabel,
    ));
}

fn spawn_display_page(page: &mut ChildSpawnerCommands, font: &Handle<Font>, settings: &Settings) {
    spawn_section(page, font, "画面调整", None);
    spawn_range_row(
        page,
        font,
        "画面亮度",
        SettingsRangeField::Brightness,
        settings.brightness,
        "整体画面亮度。",
    );
    spawn_range_row(
        page,
        font,
        "画面对比",
        SettingsRangeField::Contrast,
        settings.contrast,
        "明暗之间的反差。",
    );
    spawn_range_row(
        page,
        font,
        "画面锐化",
        SettingsRangeField::Sharpness,
        settings.sharpness * 100.0,
        "边缘锐化强度，降低画面解析时可调高。",
    );
    spawn_range_row(
        page,
        font,
        "伽马值比",
        SettingsRangeField::Gamma,
        settings.gamma,
        "暗部亮度曲线。",
    );
    spawn_section(page, font, "界面", None);
    spawn_select_row(
        page,
        font,
        "界面缩放",
        SettingsSelectField::UiScale,
        ui_scale_label(settings.ui_scale),
        "“自动”按窗口大小缩放界面，大屏会自动放大。",
        false,
    );
}

fn spawn_audio_page(page: &mut ChildSpawnerCommands, font: &Handle<Font>, settings: &Settings) {
    spawn_section(page, font, "声音", None);
    spawn_check_row(
        page,
        font,
        "CD 音乐",
        SettingsBoolField::MusicEnabled,
        settings.music_enabled,
        "播放原版背景音乐。",
        false,
    );
    spawn_range_row(
        page,
        font,
        "音乐音量",
        SettingsRangeField::MusicVolume,
        settings.music_volume,
        "背景音乐音量。",
    );
    spawn_range_row(
        page,
        font,
        "音效音量",
        SettingsRangeField::AudioVolume,
        settings.audio_volume,
        "打斗、脚步和环境音效音量。",
    );
}

fn spawn_camera_page(page: &mut ChildSpawnerCommands, font: &Handle<Font>, settings: &Settings) {
    spawn_section(page, font, "镜头", None);
    spawn_range_row(
        page,
        font,
        "视野 FOV",
        SettingsRangeField::CameraFov,
        settings.camera_fov_degrees(),
        "默认 60°，数值越大视野越宽。以 4:3 水平视角为基准。",
    );
    spawn_check_row(
        page,
        font,
        "自动镜头",
        SettingsBoolField::AutoCamera,
        settings.auto_camera,
        "镜头自动转到人物身后。",
        false,
    );
    spawn_check_row(
        page,
        font,
        "第一人称（娱乐）",
        SettingsBoolField::FirstPerson,
        settings.first_person,
        "从人物头部视角观看，仅供娱乐。",
        false,
    );
    spawn_check_row(
        page,
        font,
        "显示锁定标记",
        SettingsBoolField::LockFlag,
        settings.lock_flag,
        "在锁定目标头顶显示标记。",
        false,
    );
    spawn_check_row(
        page,
        font,
        "显示人名",
        SettingsBoolField::ShowNames,
        settings.show_names,
        "在人物头顶显示名字。",
        false,
    );
    spawn_section(page, font, "鼠标", None);
    spawn_check_row(
        page,
        font,
        "鼠标反向",
        SettingsBoolField::ReverseMouse,
        settings.reverse_mouse,
        "反转鼠标上下转动镜头的方向。",
        false,
    );
    spawn_check_row(
        page,
        font,
        "鼠标拖曳",
        SettingsBoolField::DragMouse,
        settings.drag_mouse,
        "按住鼠标按键时才转动镜头。",
        false,
    );
    spawn_range_row(
        page,
        font,
        "鼠标灵敏",
        SettingsRangeField::MouseSpeed,
        settings.mouse_speed,
        "鼠标转动镜头的速度。",
    );
}

fn spawn_controls_page(
    page: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    settings: &Settings,
    keymap: &Keymap,
) {
    spawn_section(page, font, "触屏", None);
    spawn_select_row(
        page,
        font,
        "触屏控件",
        SettingsSelectField::TouchControls,
        touch_controls_label(settings.touch_controls),
        "“自动”在检测到触屏时显示虚拟按键。",
        false,
    );
    if mobile_layout_editor_available() {
        spawn_section(page, font, "移动端操作", None);
        page.spawn(launcher_text(
            "布局只在本机保存。按住模拟按键拖动；选中后可调整大小。",
            13.0,
            theme::MUTED,
            font,
        ));
        page.spawn((
            Button,
            Node {
                width: Val::Px(200.0),
                min_height: Val::Px(48.0),
                margin: UiRect::top(Val::Px(10.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(theme::METAL.with_alpha(0.10)),
            BorderColor::all(GOLD.with_alpha(0.45)),
            Interaction::default(),
            LauncherStyledButton,
            OpenMobileLayoutEditorButton,
        ))
        .with_children(|button| {
            button.spawn(launcher_text(
                "调整按键布局",
                15.0,
                theme::ACCENT_TEXT,
                font,
            ));
        });
    } else {
        spawn_section(page, font, "按键设置", None);
        page.spawn(Node {
            width: Val::Percent(100.0),
            flex_shrink: 0.0,
            flex_wrap: FlexWrap::Wrap,
            column_gap: Val::Percent(4.0),
            ..default()
        })
        .with_children(|keys| {
            #[cfg(not(target_arch = "wasm32"))]
            let fields = KeyField::settings_actions();
            #[cfg(target_arch = "wasm32")]
            let fields = KeyField::BINDING_ACTIONS;
            for field in fields {
                spawn_keybinding_row(keys, font, field, field.get(keymap).label());
            }
        });
    }
}

fn spawn_section(
    page: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    title: &str,
    note: Option<&str>,
) {
    page.spawn(Node {
        width: Val::Percent(100.0),
        flex_shrink: 0.0,
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: Val::Px(12.0),
        padding: UiRect::new(Val::Px(4.0), Val::ZERO, Val::Px(18.0), Val::Px(6.0)),
        ..default()
    })
    .with_children(|header| {
        header.spawn(launcher_text(title, 14.0, GOLD, font));
        if let Some(note) = note {
            spawn_badge(header, font, note);
        }
        header.spawn((
            Node {
                height: Val::Px(1.0),
                flex_grow: 1.0,
                ..default()
            },
            BackgroundGradient(vec![LinearGradient::new(
                LinearGradient::TO_RIGHT,
                vec![
                    ColorStop::new(GOLD.with_alpha(0.35), Val::Percent(0.0)),
                    ColorStop::new(Color::NONE, Val::Percent(100.0)),
                ],
            )
            .into()]),
        ));
    });
}

fn spawn_badge(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, text: &str) {
    parent.spawn((
        launcher_text(text, 11.0, GOLD, font),
        Node {
            padding: UiRect::axes(Val::Px(6.0), Val::Px(1.0)),
            border: UiRect::all(Val::Px(1.0)),
            flex_shrink: 0.0,
            ..default()
        },
        BorderColor::all(GOLD.with_alpha(0.4)),
    ));
}

fn row_bundle(hint: &str) -> impl Bundle {
    row_bundle_with_width(hint, Val::Percent(100.0))
}

fn row_bundle_with_width(hint: &str, width: Val) -> impl Bundle {
    (
        Button,
        Node {
            width,
            min_height: Val::Px(ROW_H),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            column_gap: Val::Px(16.0),
            padding: UiRect::horizontal(Val::Px(16.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(Color::NONE),
        BackgroundGradient::default(),
        BorderColor {
            bottom: theme::LINE.with_alpha(0.07),
            ..BorderColor::all(Color::NONE)
        },
        Interaction::default(),
        LauncherStyledButton,
        SettingsRow {
            hint: hint.to_owned(),
        },
    )
}

fn spawn_row_label(
    row: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    restart: bool,
) {
    row.spawn(Node {
        flex_grow: 1.0,
        flex_basis: Val::Px(0.0),
        min_width: Val::Px(0.0),
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: Val::Px(10.0),
        ..default()
    })
    .with_children(|label_group| {
        label_group.spawn(launcher_text(label, 15.0, theme::TEXT, font));
        if restart {
            spawn_badge(label_group, font, "重启生效");
        }
    });
}

fn value_column() -> impl Bundle {
    value_column_kind(false)
}

fn value_column_kind(range: bool) -> impl Bundle {
    (
        Node {
            width: Val::Percent(44.0),
            min_width: Val::Px(180.0),
            max_width: Val::Px(380.0),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            column_gap: Val::Px(4.0),
            ..default()
        },
        SettingsValueColumn { range },
    )
}

fn spawn_chevron(parent: &mut ChildSpawnerCommands, right: bool, color: Color) {
    parent.spawn((
        Node {
            width: Val::Px(8.0),
            height: Val::Px(8.0),
            border: if right {
                UiRect {
                    top: Val::Px(2.0),
                    right: Val::Px(2.0),
                    ..default()
                }
            } else {
                UiRect {
                    bottom: Val::Px(2.0),
                    left: Val::Px(2.0),
                    ..default()
                }
            },
            ..default()
        },
        BorderColor::all(color),
        UiTransform {
            rotation: Rot2::degrees(45.0),
            ..default()
        },
        FocusPolicy::Pass,
    ));
}

fn spawn_arrow_button(parent: &mut ChildSpawnerCommands, right: bool, marker: impl Bundle) {
    parent
        .spawn((
            Button,
            Node {
                width: Val::Px(ARROW_HIT),
                height: Val::Px(ARROW_HIT),
                flex_shrink: 0.0,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::NONE),
            FocusPolicy::Pass,
            Interaction::default(),
            LauncherStyledButton,
            SettingsStepButton,
            marker,
        ))
        .with_children(|arrow| spawn_chevron(arrow, right, GOLD));
}

fn spawn_arrow_glyph(parent: &mut ChildSpawnerCommands, right: bool) {
    parent
        .spawn((
            Node {
                width: Val::Px(ARROW_HIT),
                height: Val::Px(ARROW_HIT),
                flex_shrink: 0.0,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            FocusPolicy::Pass,
        ))
        .with_children(|arrow| spawn_chevron(arrow, right, GOLD.with_alpha(0.7)));
}

fn value_text(font: &Handle<Font>, text: &str) -> impl Bundle {
    (
        launcher_text(text, 15.0, theme::ACCENT_TEXT, font),
        TextLayout::justify(Justify::Center),
        Node {
            flex_grow: 1.0,
            flex_basis: Val::Px(0.0),
            min_width: Val::Px(0.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
        FocusPolicy::Pass,
    )
}

fn spawn_check_row(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    field: SettingsBoolField,
    checked: bool,
    hint: &str,
    restart: bool,
) {
    parent
        .spawn((row_bundle(hint), field))
        .with_children(|row| {
            spawn_row_label(row, font, label, restart);
            row.spawn(value_column()).with_children(|value| {
                spawn_arrow_glyph(value, false);
                value.spawn((
                    value_text(font, bool_label(checked)),
                    SettingsBoolValue(field),
                ));
                spawn_arrow_glyph(value, true);
            });
        });
}

fn spawn_pack_row(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    toggle: PackToggle,
    checked: bool,
    hint: &str,
) {
    parent
        .spawn((row_bundle(hint), toggle.clone()))
        .with_children(|row| {
            spawn_row_label(row, font, label, false);
            row.spawn(value_column()).with_children(|value| {
                spawn_arrow_glyph(value, false);
                value.spawn((
                    value_text(font, bool_label(checked)),
                    SettingsPackValue(toggle),
                ));
                spawn_arrow_glyph(value, true);
            });
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
    hint: &str,
    restart: bool,
) {
    parent.spawn(row_bundle(hint)).with_children(|row| {
        spawn_row_label(row, font, label, restart);
        row.spawn(value_column()).with_children(|value| {
            spawn_arrow_button(value, false, SettingsCycle { field, step: -1 });

            value
                .spawn((
                    Button,
                    Node {
                        flex_grow: 1.0,
                        flex_basis: Val::Px(0.0),
                        min_width: Val::Px(0.0),
                        min_height: Val::Px(ARROW_HIT),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    BackgroundColor(Color::NONE),
                    FocusPolicy::Pass,
                    Interaction::default(),
                    LauncherStyledButton,
                    field,
                    crate::plugins::ui_components::UiSelectToggle(select_id(field)),
                ))
                .with_children(|toggle| {
                    toggle.spawn((value_text(font, value_label), SettingsSelectValue(field)));
                });
            spawn_arrow_button(value, true, SettingsCycle { field, step: 1 });
        });
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
    hint: &str,
) {
    parent
        .spawn((row_bundle(hint), SettingsRangeRow))
        .with_children(|row| {
            spawn_row_label(row, font, label, false);
            row.spawn(value_column_kind(true)).with_children(|column| {
                spawn_arrow_button(column, false, SettingsRangeDown(field));
                let (min, max, step) = range_bounds(field);
                crate::plugins::ui_components::slider::spawn_slider(
                    column,
                    crate::plugins::ui_components::slider::UiSlider {
                        min,
                        max,
                        step,
                        value,
                    },
                    field,
                );
                column.spawn((
                    launcher_text(
                        &format_range_value(field, value),
                        15.0,
                        theme::ACCENT_TEXT,
                        font,
                    ),
                    TextLayout::justify(Justify::Right),
                    Node {
                        width: Val::Px(40.0),
                        flex_shrink: 0.0,
                        ..default()
                    },
                    SettingsRangeValue(field),
                ));
                spawn_arrow_button(column, true, SettingsRangeUp(field));
            });
        });
}

fn spawn_keybinding_row(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    field: KeyField,
    binding: String,
) {
    let mut row = parent.spawn((
        row_bundle_with_width(
            "点击后按下新键位或鼠标按键。Esc 取消，冲突键位自动互换。",
            Val::Percent(48.0),
        ),
        KeyCaptureButton(field),
        SettingsKeyRow,
    ));

    row.with_children(|row| {
        row.spawn((
            launcher_text(field.label(), 14.0, theme::TEXT, font),
            Node {
                flex_grow: 1.0,
                min_width: Val::Px(0.0),
                ..default()
            },
        ));
        row.spawn((
            launcher_text(&binding, 13.0, theme::ACCENT_TEXT, font),
            TextLayout::justify(Justify::Center),
            Node {
                min_width: Val::Px(64.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(3.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(theme::METAL.with_alpha(0.10)),
            BorderColor::all(GOLD.with_alpha(0.35)),
            FocusPolicy::Pass,
        ));
    });
}

fn pack_stage_label(stages: &[String]) -> String {
    if stages.iter().any(|stage| stage == "*") {
        return "适用全部关卡".into();
    }
    let official = meteor_scene::stage_registry::official_stages();
    let names: Vec<_> = stages
        .iter()
        .map(|stage| {
            official
                .iter()
                .find(|info| info.id.eq_ignore_ascii_case(stage))
                .map_or_else(|| stage.clone(), |info| info.name.clone())
        })
        .collect();
    format!("适用：{}", names.join("、"))
}

fn fill_pack_lists(
    mut commands: Commands,
    lists: Query<Entity, Added<SettingsPackList>>,
    catalog: Option<Res<crate::plugins::graphics_pack::GraphicsPackCatalog>>,
    settings: Res<Settings>,
    font: Option<Res<crate::plugins::launcher_ui::UiFont>>,
) {
    if lists.is_empty() {
        return;
    }
    let font = font.map(|font| font.0.clone()).unwrap_or_default();
    let empty = crate::plugins::graphics_pack::GraphicsPackCatalog::default();
    let catalog = catalog.as_deref().unwrap_or(&empty);
    for list in &lists {
        commands.entity(list).with_children(|list| {
            if catalog.index_authoritative {
                list.spawn(launcher_text(
                    "当前由 METEOR_GRAPHICS_CONFIG 指定的配置控制，此处修改不生效。",
                    13.0,
                    theme::MUTED,
                    &font,
                ));
            }
            spawn_pack_row(
                list,
                &font,
                "启用图形增强包",
                PackToggle::Master,
                catalog.master_wanted(&settings),
                "总开关。关闭后使用原版画面，下面的单项设置保留。",
            );
            if catalog.entries.is_empty() {
                list.spawn((
                    launcher_text("未找到图形增强包。", 13.0, theme::MUTED, &font),
                    Node {
                        margin: UiRect::new(Val::Px(16.0), Val::ZERO, Val::Px(8.0), Val::ZERO),
                        ..default()
                    },
                ));
            }
            for entry in &catalog.entries {
                let mut hint = entry.description.clone();
                if !entry.stages.is_empty() {
                    if !hint.is_empty() {
                        hint.push('　');
                    }
                    hint.push_str(&pack_stage_label(&entry.stages));
                }
                if entry.error.is_some() {
                    hint = "配置文件有误，已跳过。".into();
                }
                spawn_pack_row(
                    list,
                    &font,
                    &entry.name,
                    PackToggle::Pack(entry.id.clone()),
                    catalog.pack_wanted(&settings, &entry.id),
                    &hint,
                );
            }
        });
    }
}

fn settings_pack_buttons(
    rows: Query<(&Interaction, &PackToggle), (Changed<Interaction>, With<Button>)>,
    added: Query<(), Added<PackToggle>>,
    catalog: Option<Res<crate::plugins::graphics_pack::GraphicsPackCatalog>>,
    keymap: Res<Keymap>,
    mut settings: ResMut<Settings>,
    mut values: Query<(&SettingsPackValue, &mut Text)>,
    labels: Query<(&PackToggle, &Children)>,
    mut colors: Query<&mut TextColor>,
    children: Query<&Children>,
) {
    let Some(catalog) = catalog else {
        return;
    };

    if !catalog.index_authoritative {
        for (interaction, toggle) in &rows {
            if *interaction != Interaction::Pressed {
                continue;
            }
            match toggle {
                PackToggle::Master => {
                    settings.graphics_packs = Some(!catalog.master_wanted(&settings));
                }
                PackToggle::Pack(id) => {
                    let next = !catalog.pack_wanted(&settings, id);
                    settings.graphics_pack_choices.insert(id.clone(), next);
                }
            }
            let _ = save_settings(&settings, &keymap);
        }
    }
    if !settings.is_changed() && added.is_empty() {
        return;
    }
    let master = catalog.master_wanted(&settings);
    for (value, mut text) in &mut values {
        let on = match &value.0 {
            PackToggle::Master => master,
            PackToggle::Pack(id) => catalog.pack_wanted(&settings, id),
        };
        let label = bool_label(on);
        if text.0 != label {
            text.0 = label.into();
        }
    }

    for (toggle, row_children) in &labels {
        if *toggle == PackToggle::Master {
            continue;
        }
        let alpha = if master { 1.0 } else { 0.45 };
        let mut stack: Vec<Entity> = row_children.iter().collect();
        while let Some(entity) = stack.pop() {
            if let Ok(mut color) = colors.get_mut(entity) {
                color.0 = color.0.with_alpha(alpha);
            }
            if let Ok(grand) = children.get(entity) {
                stack.extend(grand.iter());
            }
        }
    }
}

fn settings_cycle_buttons(
    arrows: Query<(&Interaction, &SettingsCycle), (Changed<Interaction>, With<Button>)>,
    keymap: Res<Keymap>,
    mut settings: ResMut<Settings>,
) {
    for (interaction, cycle) in &arrows {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let options = select_options(cycle.field);
        if options.is_empty() {
            continue;
        }
        let current = read_select(&settings, cycle.field);
        let start = options
            .iter()
            .position(|(value, _)| (value - current).abs() < 0.01)
            .unwrap_or(0);
        let len = options.len() as i32;
        let mut index = start as i32;
        for _ in 0..len {
            index = (index + i32::from(cycle.step)).rem_euclid(len);
            let value = options[index as usize].0;
            if graphics_field(cycle.field)
                && crate::graphics_settings::unavailable_reason(&settings, cycle.field, value)
                    .is_some()
            {
                continue;
            }
            write_select(&mut settings, cycle.field, value);
            let _ = save_settings(&settings, &keymap);
            break;
        }
    }
}

fn write_select(settings: &mut Settings, field: SettingsSelectField, value: f32) {
    match field {
        SettingsSelectField::WindowResolution => settings.window_resolution = value as u8,
        SettingsSelectField::GraphicsAdapter | SettingsSelectField::GraphicsBackend => {
            crate::graphics_settings::select(settings, field, value)
        }
        SettingsSelectField::RenderScale => settings.render_scale = value,
        SettingsSelectField::TextureQuality => settings.texture_quality = value as u8,
        SettingsSelectField::TargetFps => settings.target_fps = value as u32,
        SettingsSelectField::UiScale => settings.ui_scale = value,
        SettingsSelectField::TouchControls => settings.touch_controls = value as i8,
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
    let compact_nav = !portrait && window.height() < 500.0;
    for mut node in &mut navs {
        node.width = if portrait {
            Val::Percent(100.0)
        } else if compact_nav {
            Val::Px(140.0)
        } else {
            Val::Percent(16.0)
        };
        node.min_width = Val::Px(if portrait { 0.0 } else { 120.0 });
        node.max_width = if portrait { Val::Auto } else { Val::Px(220.0) };
        node.flex_direction = if portrait || compact_nav {
            FlexDirection::Row
        } else {
            FlexDirection::Column
        };
        node.height = if portrait {
            Val::Px(46.0)
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
        node.border = if portrait {
            UiRect::bottom(Val::Px(1.0))
        } else {
            UiRect::right(Val::Px(1.0))
        };
        node.padding = if portrait {
            UiRect::ZERO
        } else {
            UiRect::right(Val::Px(if compact_nav { 6.0 } else { 14.0 }))
        };
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
        node.flex_grow = 1.0;
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
        node.padding =
            UiRect::horizontal(Val::Px(if portrait || compact_nav { 6.0 } else { 12.0 }));
    }
}

pub(crate) fn adapt_settings_layout(
    window: Query<&Window>,
    mut roots: Query<&mut Node, With<SettingsFormRoot>>,
    mut rows: Query<
        (&mut Node, Has<SettingsKeyRow>, Has<SettingsRangeRow>),
        (With<SettingsRow>, Without<SettingsFormRoot>),
    >,
    mut values: Query<
        (&SettingsValueColumn, &mut Node),
        (Without<SettingsRow>, Without<SettingsFormRoot>),
    >,
    mut step_buttons: Query<
        &mut Node,
        (
            With<SettingsStepButton>,
            Without<SettingsValueColumn>,
            Without<SettingsRow>,
            Without<SettingsFormRoot>,
        ),
    >,
    mut chrome: Query<
        (&SettingsChrome, &mut Node),
        (
            Without<SettingsStepButton>,
            Without<SettingsValueColumn>,
            Without<SettingsRow>,
            Without<SettingsFormRoot>,
        ),
    >,
    mut key_hints: Query<
        &mut Node,
        (
            With<SettingsKeyHint>,
            Without<SettingsChrome>,
            Without<SettingsStepButton>,
            Without<SettingsValueColumn>,
            Without<SettingsRow>,
            Without<SettingsFormRoot>,
        ),
    >,
    mut titles: Query<&mut TextFont, With<SettingsTitle>>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let layout = responsive_ui_layout(window.width(), window.height(), cfg!(target_os = "android"));
    let portrait = window.width() < 600.0 && window.height() > window.width();
    let touch = layout.mobile || cfg!(target_os = "android");

    for mut root in &mut roots {
        root.flex_direction = if portrait {
            FlexDirection::Column
        } else {
            FlexDirection::Row
        };
        root.column_gap = Val::Px(if layout.mobile { 12.0 } else { 28.0 });
        root.row_gap = Val::Px(6.0);
    }

    let stack_ranges = portrait || window.width() < 520.0;
    for (mut node, key_row, range_row) in &mut rows {
        node.min_height = Val::Px(if touch { TOUCH_ROW_H } else { ROW_H });
        node.padding = UiRect::new(
            Val::Px(if layout.mobile { 10.0 } else { 16.0 }),
            Val::Px(if layout.mobile { 10.0 } else { 16.0 }),
            Val::Px(if range_row && stack_ranges { 6.0 } else { 0.0 }),
            Val::ZERO,
        );
        if key_row {
            node.width = Val::Percent(if layout.narrow || portrait {
                100.0
            } else {
                48.0
            });
        }
        if range_row {
            node.flex_wrap = if stack_ranges {
                FlexWrap::Wrap
            } else {
                FlexWrap::NoWrap
            };
        }
    }
    for (column, mut node) in &mut values {
        if column.range && stack_ranges {
            node.width = Val::Percent(100.0);
            node.max_width = Val::Percent(100.0);
            node.min_width = Val::Px(0.0);
            continue;
        }
        node.width = Val::Percent(if portrait || layout.narrow {
            54.0
        } else {
            44.0
        });
        node.max_width = Val::Px(380.0);
        node.min_width = Val::Px(if portrait { 150.0 } else { 180.0 });
    }
    for mut node in &mut step_buttons {
        node.width = Val::Px(ARROW_HIT);
        node.height = Val::Px(ARROW_HIT);
    }
    for (chrome, mut node) in &mut chrome {
        let height = match (chrome.footer, layout.mobile) {
            (false, false) => 64.0,
            (false, true) => 48.0,
            (true, false) => 52.0,
            (true, true) => 48.0,
        };
        node.min_height = Val::Px(height);
        node.height = Val::Px(height);
        node.max_height = Val::Px(height);
    }
    for mut node in &mut key_hints {
        node.display = if layout.mobile {
            Display::None
        } else {
            Display::Flex
        };
    }
    for mut font in &mut titles {
        font.font_size = FontSize::Px(if layout.mobile { 22.0 } else { 30.0 });
    }
}

pub(crate) fn spawn_settings_title(parent: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            ..default()
        })
        .with_children(|titles| {
            titles.spawn(launcher_text("SETTINGS", 11.0, GOLD, font));
            titles.spawn((
                launcher_text("设置", 30.0, theme::TEXT, font),
                SettingsTitle,
            ));
        });
}

pub(crate) fn settings_header_bundle() -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            min_height: Val::Px(64.0),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(20.0),
            padding: UiRect::bottom(Val::Px(4.0)),
            ..default()
        },
        SettingsChrome { footer: false },
    )
}

pub(crate) fn spawn_settings_rule(parent: &mut ChildSpawnerCommands) {
    parent.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(1.0),
            flex_shrink: 0.0,
            margin: UiRect::bottom(Val::Px(10.0)),
            ..default()
        },
        BackgroundGradient(vec![LinearGradient::new(
            LinearGradient::TO_RIGHT,
            vec![
                ColorStop::new(GOLD.with_alpha(0.55), Val::Percent(0.0)),
                ColorStop::new(GOLD.with_alpha(0.18), Val::Percent(45.0)),
                ColorStop::new(Color::NONE, Val::Percent(100.0)),
            ],
        )
        .into()]),
    ));
}

pub(crate) fn spawn_settings_footer(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    actions: impl FnOnce(&mut ChildSpawnerCommands),
) {
    parent
        .spawn((
            Node {
                width: Val::Percent(100.0),
                min_height: Val::Px(52.0),
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(14.0),
                padding: UiRect::top(Val::Px(6.0)),
                border: UiRect::top(Val::Px(1.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            BorderColor::all(theme::LINE.with_alpha(0.16)),
            SettingsChrome { footer: true },
        ))
        .with_children(|footer| {
            footer.spawn((
                launcher_text(DEFAULT_HINT, 13.0, theme::MUTED, font),
                Node {
                    flex_grow: 1.0,
                    flex_basis: Val::Px(0.0),
                    min_width: Val::Px(0.0),
                    max_height: Val::Px(40.0),
                    overflow: Overflow::clip(),
                    ..default()
                },
                SettingsHintText,
            ));
            footer
                .spawn((
                    Node {
                        display: Display::None,
                        flex_shrink: 0.0,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(6.0),
                        padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BackgroundColor(theme::METAL.with_alpha(0.10)),
                    BorderColor::all(GOLD.with_alpha(0.5)),
                    SettingsRestartNotice,
                ))
                .with_children(|notice| {
                    notice.spawn(launcher_text("◆", 10.0, GOLD, font));
                    notice.spawn(launcher_text(
                        "部分改动需重启游戏后生效",
                        13.0,
                        theme::ACCENT_TEXT,
                        font,
                    ));
                });
            footer
                .spawn((
                    Node {
                        flex_shrink: 0.0,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(6.0),
                        ..default()
                    },
                    SettingsKeyHint,
                ))
                .with_children(|hint| {
                    hint.spawn((
                        launcher_text("Esc", 12.0, theme::ACCENT_TEXT, font),
                        Node {
                            padding: UiRect::axes(Val::Px(6.0), Val::Px(1.0)),
                            border: UiRect::all(Val::Px(1.0)),
                            ..default()
                        },
                        BackgroundColor(theme::METAL.with_alpha(0.10)),
                        BorderColor::all(GOLD.with_alpha(0.4)),
                    ));
                    hint.spawn(launcher_text("返回", 13.0, theme::MUTED, font));
                });
            actions(footer);
        });
}

fn bool_label(checked: bool) -> &'static str {
    if checked {
        "开启"
    } else {
        "关闭"
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
        SettingsRangeField::CameraFov => format!("{:.0}°", value.round()),
        _ => format!("{}", value.round() as i32),
    }
}

fn range_bounds(field: SettingsRangeField) -> (f32, f32, f32) {
    match field {
        SettingsRangeField::Brightness => (45.0, 145.0, 1.0),
        SettingsRangeField::Contrast => (55.0, 145.0, 1.0),
        SettingsRangeField::Sharpness => (0.0, 100.0, 1.0),
        SettingsRangeField::Gamma => (0.0, 100.0, 1.0),
        SettingsRangeField::MusicVolume | SettingsRangeField::AudioVolume => (0.0, 100.0, 1.0),
        SettingsRangeField::MouseSpeed => {
            (Settings::MOUSE_SPEED_MIN, Settings::MOUSE_SPEED_MAX, 1.0)
        }
        SettingsRangeField::CameraFov => (Settings::CAMERA_FOV_MIN, Settings::CAMERA_FOV_MAX, 1.0),
    }
}

fn read_range(settings: &Settings, field: SettingsRangeField) -> f32 {
    match field {
        SettingsRangeField::Brightness => settings.brightness,
        SettingsRangeField::Contrast => settings.contrast,
        SettingsRangeField::Sharpness => settings.sharpness * 100.0,
        SettingsRangeField::Gamma => settings.gamma,
        SettingsRangeField::MusicVolume => settings.music_volume,
        SettingsRangeField::AudioVolume => settings.audio_volume,
        SettingsRangeField::MouseSpeed => settings.mouse_speed,
        SettingsRangeField::CameraFov => settings.camera_fov_degrees(),
    }
}

fn write_range(settings: &mut Settings, field: SettingsRangeField, value: f32) {
    let (min, max, _) = range_bounds(field);
    let v = value.clamp(min, max);
    match field {
        SettingsRangeField::Brightness => settings.brightness = v,
        SettingsRangeField::Contrast => settings.contrast = v,
        SettingsRangeField::Sharpness => settings.sharpness = v / 100.0,
        SettingsRangeField::Gamma => settings.gamma = v,
        SettingsRangeField::MusicVolume => settings.music_volume = v,
        SettingsRangeField::AudioVolume => settings.audio_volume = v,
        SettingsRangeField::MouseSpeed => settings.mouse_speed = v,
        SettingsRangeField::CameraFov => settings.camera_fov = v,
    }
}

fn read_bool(settings: &Settings, field: SettingsBoolField) -> bool {
    match field {
        SettingsBoolField::Fullscreen => settings.fullscreen,
        SettingsBoolField::BorderlessWindow => settings.borderless_window,
        SettingsBoolField::HitEffects => settings.hit_effects,
        SettingsBoolField::HitStop => settings.hit_stop,
        SettingsBoolField::HitShake => settings.hit_shake,
        SettingsBoolField::HitSway => settings.hit_sway,
        SettingsBoolField::CharacterMotion => settings.character_motion,
        SettingsBoolField::NewCharacterModels => settings.new_character_models,
        SettingsBoolField::VolumetricFog => settings.volumetric_fog,
        SettingsBoolField::SkyEnhance => settings.sky_enhance,
        SettingsBoolField::GrassEnhance => settings.grass_enhance,
        SettingsBoolField::EffectEnhance => settings.effect_enhance,
        SettingsBoolField::DynamicShadows => settings.dynamic_shadows,
        SettingsBoolField::SceneNormalMaps => settings.scene_normal_maps,
        SettingsBoolField::HdScene => settings.hd_scene,
        SettingsBoolField::WaterEnhance => settings.water_enhance,
        SettingsBoolField::WaterLiveReflections => settings.water_live_reflections,
        SettingsBoolField::EnvironmentLighting => settings.environment_lighting,
        SettingsBoolField::Dlss5NeuralRendering => settings.dlss5_neural_rendering,
        SettingsBoolField::MusicEnabled => settings.music_enabled,
        SettingsBoolField::AutoCamera => settings.auto_camera,
        SettingsBoolField::FirstPerson => settings.first_person,
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
        SettingsBoolField::HitShake => settings.hit_shake = value,
        SettingsBoolField::HitSway => settings.hit_sway = value,
        SettingsBoolField::CharacterMotion => settings.character_motion = value,
        SettingsBoolField::NewCharacterModels => settings.new_character_models = value,
        SettingsBoolField::VolumetricFog => settings.volumetric_fog = value,
        SettingsBoolField::SkyEnhance => settings.sky_enhance = value,
        SettingsBoolField::GrassEnhance => settings.grass_enhance = value,
        SettingsBoolField::EffectEnhance => settings.effect_enhance = value,
        SettingsBoolField::DynamicShadows => settings.dynamic_shadows = value,
        SettingsBoolField::SceneNormalMaps => settings.scene_normal_maps = value,
        SettingsBoolField::HdScene => settings.hd_scene = value,
        SettingsBoolField::WaterEnhance => settings.water_enhance = value,
        SettingsBoolField::WaterLiveReflections => settings.water_live_reflections = value,
        SettingsBoolField::EnvironmentLighting => settings.environment_lighting = value,
        SettingsBoolField::Dlss5NeuralRendering => settings.dlss5_neural_rendering = value,
        SettingsBoolField::MusicEnabled => settings.music_enabled = value,
        SettingsBoolField::AutoCamera => settings.auto_camera = value,
        SettingsBoolField::FirstPerson => settings.first_person = value,
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
        if matches!(
            field,
            SettingsBoolField::HitStop | SettingsBoolField::HitShake
        ) && !settings.hit_effects
        {
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
        write_select(&mut settings, option.field, option.value);
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
    fn settings_form_has_eight_real_sliders_visible_signs_and_cancelable_selection() {
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
                            false,
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
            8
        );
        let fov_slider = app
            .world_mut()
            .query::<(&SettingsRangeField, &UiSlider)>()
            .iter(app.world())
            .find(|(field, _)| **field == SettingsRangeField::CameraFov)
            .unwrap()
            .1;
        assert_eq!(
            (
                fov_slider.min,
                fov_slider.max,
                fov_slider.step,
                fov_slider.value
            ),
            (40.0, 100.0, 1.0, 60.0)
        );
        app.world_mut().resource_mut::<Settings>().camera_fov = 85.0;
        app.update();
        let fov_text = app
            .world_mut()
            .query::<(&SettingsRangeValue, &Text)>()
            .iter(app.world())
            .find(|(field, _)| field.0 == SettingsRangeField::CameraFov)
            .unwrap()
            .1;
        assert_eq!(fov_text.0, "85°");
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
            assert_eq!(children.len(), 1, "one drawn chevron");
            assert!(
                children
                    .iter()
                    .all(|child| app.world().get::<Text>(child).is_none()),
                "chevrons must not depend on font glyphs"
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
        fn bool_row(app: &mut App, wanted: SettingsBoolField) -> (Entity, String) {
            let value = app
                .world_mut()
                .query::<(&SettingsBoolValue, &Text)>()
                .iter(app.world())
                .find_map(|(field, text)| (field.0 == wanted).then(|| text.0.clone()))
                .expect("bool value text");
            let row = app
                .world_mut()
                .query::<(Entity, &SettingsBoolField, &ChildOf)>()
                .iter(app.world())
                .find_map(|(_, field, parent)| (*field == wanted).then_some(parent.parent()))
                .expect("bool row");
            (row, value)
        }
        for enabled in [false, true] {
            write_bool(
                &mut app.world_mut().resource_mut::<Settings>(),
                SettingsBoolField::HitShake,
                enabled,
            );
            app.update();
            let (parent, value) = bool_row(&mut app, SettingsBoolField::HitShake);
            assert_eq!(value, bool_label(enabled));
            assert_eq!(
                read_bool(
                    app.world().resource::<Settings>(),
                    SettingsBoolField::HitShake
                ),
                enabled
            );
            assert!(app.world().get::<HitEffectsOptions>(parent).is_some());
            assert_eq!(
                app.world().get::<Node>(parent).unwrap().flex_direction,
                FlexDirection::Column
            );
            assert!(!app.world().resource::<Settings>().hit_effects);
        }
        for enabled in [true, false] {
            app.world_mut().resource_mut::<Settings>().hit_sway = enabled;
            app.update();
            let (parent, value) = bool_row(&mut app, SettingsBoolField::HitSway);
            assert_eq!(value, bool_label(enabled));
            assert!(app.world().get::<HitEffectsOptions>(parent).is_none());
            assert!(!app.world().resource::<Settings>().hit_effects);
        }
        for enabled in [false, true] {
            write_bool(
                &mut app.world_mut().resource_mut::<Settings>(),
                SettingsBoolField::NewCharacterModels,
                enabled,
            );
            app.update();
            assert_eq!(
                bool_row(&mut app, SettingsBoolField::NewCharacterModels).1,
                bool_label(enabled)
            );
            assert_eq!(
                read_bool(
                    app.world().resource::<Settings>(),
                    SettingsBoolField::NewCharacterModels
                ),
                enabled
            );
        }
        for (category, _) in SettingsCategory::ALL {
            if !category.available() {
                continue;
            }
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
    fn restart_only_reflection_control_is_absent_from_in_game_settings() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        for in_game in [true, false] {
            app.world_mut()
                .run_system_once(move |mut commands: Commands| {
                    commands.spawn(Node::default()).with_children(|root| {
                        spawn_web_settings_form(
                            root,
                            &Handle::default(),
                            &Settings::default(),
                            &Keymap::default(),
                            in_game,
                        );
                    });
                })
                .unwrap();
            let count = app
                .world_mut()
                .query::<&SettingsBoolField>()
                .iter(app.world())
                .filter(|field| **field == SettingsBoolField::WaterLiveReflections)
                .count();
            assert_eq!(count, if in_game { 0 } else { 1 });
        }
    }

    #[test]
    fn rows_grow_for_touch_and_key_rows_stack_on_narrow_windows() {
        let mut app = App::new();
        app.add_systems(Update, adapt_settings_layout);
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: (1280_u32, 720_u32).into(),
                ..default()
            })
            .id();
        app.world_mut().spawn((Node::default(), SettingsFormRoot));
        let row = app
            .world_mut()
            .spawn((
                Node::default(),
                SettingsRow {
                    hint: String::new(),
                },
            ))
            .id();
        let key = app
            .world_mut()
            .spawn((
                Node::default(),
                SettingsRow {
                    hint: String::new(),
                },
                SettingsKeyRow,
            ))
            .id();
        let value = app
            .world_mut()
            .spawn((Node::default(), SettingsValueColumn { range: false }))
            .id();
        let slider = app
            .world_mut()
            .spawn((Node::default(), SettingsValueColumn { range: true }))
            .id();
        let footer = app
            .world_mut()
            .spawn((Node::default(), SettingsChrome { footer: true }))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Node>(row).unwrap().min_height,
            Val::Px(ROW_H)
        );
        assert_eq!(
            app.world().get::<Node>(key).unwrap().width,
            Val::Percent(48.0)
        );
        assert_eq!(
            app.world().get::<Node>(value).unwrap().width,
            Val::Percent(44.0)
        );
        assert_eq!(
            app.world().get::<Node>(footer).unwrap().height,
            Val::Px(52.0)
        );
        assert_eq!(
            app.world().get::<Node>(footer).unwrap().max_height,
            Val::Px(52.0)
        );

        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution = (412_u32, 915_u32).into();
        app.update();
        assert_eq!(
            app.world().get::<Node>(row).unwrap().min_height,
            Val::Px(TOUCH_ROW_H)
        );
        assert_eq!(
            app.world().get::<Node>(key).unwrap().width,
            Val::Percent(100.0)
        );
        assert_eq!(
            app.world().get::<Node>(value).unwrap().width,
            Val::Percent(54.0)
        );
        assert_eq!(
            app.world().get::<Node>(slider).unwrap().width,
            Val::Percent(100.0),
            "slider drops under the label"
        );
        assert_eq!(
            app.world().get::<Node>(footer).unwrap().height,
            Val::Px(48.0)
        );
        assert_eq!(
            app.world().get::<Node>(footer).unwrap().max_height,
            Val::Px(48.0)
        );
        let root = app
            .world_mut()
            .query_filtered::<&Node, With<SettingsFormRoot>>()
            .single(app.world())
            .unwrap();
        assert_eq!(root.flex_direction, FlexDirection::Column);
    }

    fn form_app(settings: Settings) -> App {
        use crate::plugins::ui_components::UiComponentsPlugin;
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.add_plugins((UiComponentsPlugin, SettingsUiPlugin))
            .insert_resource(settings.clone())
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
            .run_system_once(move |mut commands: Commands| {
                commands.spawn(Node::default()).with_children(|root| {
                    spawn_web_settings_form(
                        root,
                        &Handle::default(),
                        &settings,
                        &Keymap::default(),
                        false,
                    );
                    spawn_settings_footer(root, &Handle::default(), |_| {});
                });
            })
            .unwrap();
        app
    }

    fn press<T: Component>(app: &mut App, find: impl Fn(&T) -> bool) {
        let entity = app
            .world_mut()
            .query::<(Entity, &T)>()
            .iter(app.world())
            .find_map(|(entity, value)| find(value).then_some(entity))
            .expect("pressable entity");
        *app.world_mut().get_mut::<Interaction>(entity).unwrap() = Interaction::Pressed;
        app.update();
        *app.world_mut().get_mut::<Interaction>(entity).unwrap() = Interaction::None;
        app.update();
    }

    #[test]
    fn select_arrows_cycle_both_ways_and_wrap() {
        let mut app = form_app(Settings::default());
        app.update();
        assert_eq!(app.world().resource::<Settings>().target_fps, 0);
        let fps = |cycle: &SettingsCycle, step| {
            cycle.field == SettingsSelectField::TargetFps && cycle.step == step
        };
        press::<SettingsCycle>(&mut app, |c| fps(c, 1));
        assert_eq!(app.world().resource::<Settings>().target_fps, 30);
        press::<SettingsCycle>(&mut app, |c| fps(c, -1));
        press::<SettingsCycle>(&mut app, |c| fps(c, -1));
        assert_eq!(
            app.world().resource::<Settings>().target_fps,
            120,
            "wraps to the last option"
        );
        let label = app
            .world_mut()
            .query::<(&SettingsSelectValue, &Text)>()
            .iter(app.world())
            .find_map(|(value, text)| {
                (value.0 == SettingsSelectField::TargetFps).then(|| text.0.clone())
            })
            .unwrap();
        assert_eq!(label, "120 FPS");
    }

    #[test]
    fn hovered_row_drives_the_footer_hint_and_highlight() {
        let mut app = form_app(Settings::default());
        app.update();
        let row = app
            .world_mut()
            .query::<(Entity, &SettingsBoolField)>()
            .iter(app.world())
            .find_map(|(entity, field)| {
                (*field == SettingsBoolField::DynamicShadows).then_some(entity)
            })
            .unwrap();
        *app.world_mut().get_mut::<Interaction>(row).unwrap() = Interaction::Hovered;
        app.update();
        let hint = app
            .world_mut()
            .query_filtered::<&Text, With<SettingsHintText>>()
            .single(app.world())
            .unwrap()
            .0
            .clone();
        assert_eq!(hint, app.world().get::<SettingsRow>(row).unwrap().hint);
        assert!(!app
            .world()
            .get::<BackgroundGradient>(row)
            .unwrap()
            .0
            .is_empty());
        *app.world_mut().get_mut::<Interaction>(row).unwrap() = Interaction::None;
        app.update();
        let hint = app
            .world_mut()
            .query_filtered::<&Text, With<SettingsHintText>>()
            .single(app.world())
            .unwrap()
            .0
            .clone();
        assert_eq!(hint, DEFAULT_HINT);
        assert!(app
            .world()
            .get::<BackgroundGradient>(row)
            .unwrap()
            .0
            .is_empty());
    }

    #[test]
    fn graphics_pack_rows_save_choices_and_flag_a_restart() {
        use crate::plugins::graphics_pack::CatalogEntry;
        let catalog = crate::plugins::graphics_pack::GraphicsPackCatalog {
            index_enabled: false,
            master_active: false,
            index_authoritative: false,
            entries: vec![CatalogEntry {
                id: "combat-fx-sample".into(),
                name: "刀光与打击特效".into(),
                description: "说明".into(),
                stages: vec!["*".into()],
                active: false,
                error: None,
            }],
        };
        let mut app = form_app(Settings::default());
        app.insert_resource(catalog.clone())
            .insert_resource(crate::app_state::StartupSettings(Settings::default()))
            .insert_resource(crate::plugins::launcher_ui::UiFont(Handle::default()));
        app.update();
        app.update();
        let values = |app: &mut App| -> Vec<(PackToggle, String)> {
            app.world_mut()
                .query::<(&SettingsPackValue, &Text)>()
                .iter(app.world())
                .map(|(value, text)| (value.0.clone(), text.0.clone()))
                .collect()
        };
        assert_eq!(
            values(&mut app),
            vec![
                (PackToggle::Master, "关闭".into()),
                (PackToggle::Pack("combat-fx-sample".into()), "开启".into()),
            ]
        );
        let notice = |app: &mut App| {
            app.world_mut()
                .query_filtered::<&Node, With<SettingsRestartNotice>>()
                .single(app.world())
                .unwrap()
                .display
        };
        assert_eq!(notice(&mut app), Display::None);

        press::<PackToggle>(&mut app, |t| *t == PackToggle::Master);
        assert_eq!(
            app.world().resource::<Settings>().graphics_packs,
            Some(true)
        );
        assert_eq!(values(&mut app)[0].1, "开启");
        assert_eq!(
            notice(&mut app),
            Display::Flex,
            "master change needs a restart"
        );

        press::<PackToggle>(&mut app, |t| matches!(t, PackToggle::Pack(_)));
        assert_eq!(
            app.world()
                .resource::<Settings>()
                .graphics_pack_choices
                .get("combat-fx-sample"),
            Some(&false)
        );
        assert_eq!(values(&mut app)[1].1, "关闭");

        press::<PackToggle>(&mut app, |t| *t == PackToggle::Master);
        assert_eq!(notice(&mut app), Display::None);
    }

    #[test]
    fn every_settings_string_has_a_glyph_in_the_bundled_font() {
        use bevy::ecs::system::RunSystemOnce;
        let Some(path) = std::env::var_os("METEOR_TEST_UI_FONT") else {
            return;
        };
        let bytes = std::fs::read(path).expect("read UI font");
        let face = ttf_parser::Face::parse(&bytes, 0).expect("ui font parses");
        let mut app = form_app(Settings::default());
        app.world_mut()
            .run_system_once(|mut commands: Commands| {
                commands.spawn(Node::default()).with_children(|root| {
                    spawn_settings_select_dialogs(root, &Handle::default(), &Settings::default());
                });
            })
            .unwrap();
        app.update();
        let mut texts: Vec<String> = app
            .world_mut()
            .query::<&Text>()
            .iter(app.world())
            .map(|t| t.0.clone())
            .collect();
        texts.extend(
            app.world_mut()
                .query::<&SettingsRow>()
                .iter(app.world())
                .map(|row| row.hint.clone()),
        );
        texts.extend([
            DEFAULT_HINT.into(),
            "适用全部关卡".into(),
            "适用：".into(),
            "当前由 METEOR_GRAPHICS_CONFIG 指定的配置控制，此处修改不生效。".into(),
            "启用图形增强包".into(),
            "总开关。关闭后使用原版画面，下面的单项设置保留。".into(),
            "未找到图形增强包。".into(),
            "配置文件有误，已跳过。".into(),
        ]);
        let missing: std::collections::BTreeSet<char> = texts
            .iter()
            .flat_map(|text| text.chars())
            .filter(|c| !c.is_whitespace() && face.glyph_index(*c).is_none())
            .collect();
        assert!(missing.is_empty(), "missing glyphs: {missing:?}");
    }
}
