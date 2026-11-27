use super::character_plugin::preview::CharacterPreviewViewport;
use super::launcher_ui::scrollable_y;
use super::room_map_preview::MapImage;
use super::ui_theme as theme;
use crate::app_state::{
    AppState, LaunchRequest, LauncherScreenMarker, LauncherStyledButton, MenuBackButton,
    MenuScreen, StageListTab, StartGameButton,
};
use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;

#[derive(Resource, Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct CampaignProgress {
    completed: usize,
}

impl CampaignProgress {
    fn unlocked_count(&self) -> usize {
        self.completed.saturating_add(1).min(MISSIONS.len())
    }

    pub(crate) fn is_unlocked(&self, stage: &str) -> bool {
        MISSIONS
            .iter()
            .take(self.unlocked_count())
            .any(|mission| mission.id == stage)
    }

    fn complete(&mut self, stage: &str) -> bool {
        if MISSIONS
            .get(self.completed)
            .is_some_and(|mission| mission.id == stage)
        {
            self.completed += 1;
            true
        } else {
            false
        }
    }

    fn from_text(text: &str) -> Self {
        let mut progress: Self = serde_json::from_str(text).unwrap_or_default();
        progress.completed = progress.completed.min(MISSIONS.len());
        progress
    }

    fn load() -> Self {
        #[cfg(target_arch = "wasm32")]
        let text = web_sys::window()
            .and_then(|window| window.local_storage().ok().flatten())
            .and_then(|storage| storage.get_item("meteor.rust.campaign.v1").ok().flatten());
        #[cfg(not(target_arch = "wasm32"))]
        let text = std::fs::read_to_string(Self::file_path()).ok();
        Self::from_text(text.as_deref().unwrap_or_default())
    }

    fn save(&self) -> bool {
        let Ok(text) = serde_json::to_string(self) else {
            return false;
        };
        #[cfg(target_arch = "wasm32")]
        {
            web_sys::window()
                .and_then(|window| window.local_storage().ok().flatten())
                .is_some_and(|storage| storage.set_item("meteor.rust.campaign.v1", &text).is_ok())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::fs::write(Self::file_path(), text).is_ok()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn file_path() -> std::path::PathBuf {
        #[cfg(target_os = "android")]
        if let Some(path) = bevy::android::ANDROID_APP
            .get()
            .and_then(|app| app.internal_data_path())
        {
            return path.join("campaign-progress.json");
        }
        std::path::PathBuf::from("campaign-progress.json")
    }
}

#[derive(Resource, Default)]
struct CampaignRun(Option<String>);

fn begin_campaign_run(
    state: Res<State<AppState>>,
    request: Res<LaunchRequest>,
    loaded: Option<Res<crate::app_state::LoadedStageInfo>>,
    net: Option<Res<super::network_plugin::NetState>>,
    mut run: ResMut<CampaignRun>,
) {
    if *state.get() != AppState::Playing {
        return;
    }
    run.0 = loaded
        .filter(|loaded| loaded.ready)
        .filter(|_| {
            request.mode == "single"
                && !request.spawn_room_npcs
                && net
                    .is_none_or(|net| net.status == super::network_plugin::NetStatus::Disconnected)
        })
        .map(|loaded| loaded.stage_id.clone());
}

fn record_campaign_result(
    state: Res<State<AppState>>,
    end: Res<crate::components::fighter::MatchEnd>,
    net: Option<Res<super::network_plugin::NetState>>,
    mut run: ResMut<CampaignRun>,
    mut progress: ResMut<CampaignProgress>,
) {
    if *state.get() != AppState::Result {
        return;
    }
    let Some(stage) = run.0.take() else {
        return;
    };
    if end.settled
        && end.result == 4
        && net.is_none_or(|net| net.status == super::network_plugin::NetStatus::Disconnected)
    {
        progress.complete(&stage);
    }
}

fn save_campaign_progress(progress: Res<CampaignProgress>, mut saved: Local<Option<usize>>) {
    if *saved == Some(progress.completed) {
        return;
    }
    if !progress.save() {
        warn!("单人通关进度保存失败；本次会话仍保留已解锁关卡");
    }
    *saved = Some(progress.completed);
}

pub(crate) struct Mission {
    pub id: &'static str,
    pub name: &'static str,
    pub position: Vec2,
    pub minutes: u32,
    pub primary_item: i64,
    pub secondary_item: i64,
}

pub(crate) const MISSIONS: &[Mission] = &[
    Mission {
        id: "sn14",
        name: "炎硫岛",
        position: Vec2::new(0.15, 0.76),
        minutes: 10,
        primary_item: 6,
        secondary_item: 0,
    },
    Mission {
        id: "sn01",
        name: "钟乳洞",
        position: Vec2::new(0.14, 0.43),
        minutes: 10,
        primary_item: 5,
        secondary_item: 1,
    },
    Mission {
        id: "sn03",
        name: "一线天",
        position: Vec2::new(0.17, 0.19),
        minutes: 15,
        primary_item: 15,
        secondary_item: 0,
    },
    Mission {
        id: "sn13",
        name: "金华城",
        position: Vec2::new(0.38, 0.42),
        minutes: 15,
        primary_item: 24,
        secondary_item: 14,
    },
    Mission {
        id: "sn04",
        name: "炽雪城",
        position: Vec2::new(0.39, 0.14),
        minutes: 20,
        primary_item: 16,
        secondary_item: 13,
    },
    Mission {
        id: "sn15",
        name: "飞鹏堡",
        position: Vec2::new(0.39, 0.70),
        minutes: 25,
        primary_item: 13,
        secondary_item: 10,
    },
    Mission {
        id: "sn05",
        name: "皇天城",
        position: Vec2::new(0.65, 0.20),
        minutes: 30,
        primary_item: 24,
        secondary_item: 21,
    },
    Mission {
        id: "sn09",
        name: "决死阵",
        position: Vec2::new(0.61, 0.48),
        minutes: 5,
        primary_item: 5,
        secondary_item: 1,
    },
    Mission {
        id: "sn10",
        name: "炼铁狱",
        position: Vec2::new(0.65, 0.77),
        minutes: 30,
        primary_item: 31,
        secondary_item: 33,
    },
];

pub(crate) fn mission(id: &str) -> Option<&'static Mission> {
    MISSIONS.iter().find(|m| m.id == id)
}

pub(crate) fn mission_weapons(id: &str) -> ((u8, u8), (u8, u8)) {
    let m = mission(id).unwrap_or(&MISSIONS[0]);
    let primary = super::character_plugin::weapon_slot_from_item(m.primary_item).unwrap_or((6, 0));
    let secondary =
        super::character_plugin::weapon_slot_from_item(m.secondary_item).unwrap_or((0, 0));
    (primary, secondary)
}

fn weapon_class_name(weapon_id: u8) -> &'static str {
    match weapon_id {
        1 => "飞镖",
        2 => "飞轮",
        3 => "火铳",
        4 => "双刺",
        5 => "匕首",
        6 => "长剑",
        7 => "长枪",
        8 => "大刀",
        9 => "重锤",
        10 => "乾坤刀",
        11 => "指虎",
        12 => "忍刀",
        _ => "无",
    }
}

fn mission_loadout_summary(m: &Mission) -> String {
    let ((pw, _), (sw, _)) = mission_weapons(m.id);
    let primary = weapon_class_name(pw);
    let secondary = if sw == 0 {
        "无"
    } else {
        weapon_class_name(sw)
    };
    format!("主兵器 · {primary}      副兵器 · {secondary}")
}

pub(crate) fn next_mission(id: &str) -> Option<&'static Mission> {
    let index = MISSIONS.iter().position(|mission| mission.id == id)?;
    MISSIONS.get(index + 1)
}

pub(super) fn mission_installed(id: &str) -> bool {
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    if let Some(installed) = meteor_asset::runtime_assets::original_mission_available(id) {
        return installed;
    }
    let _ = id;
    true
}

pub(crate) fn prepare_launch(request: &mut LaunchRequest, progress: &CampaignProgress) -> bool {
    if !progress.is_unlocked(&request.stage_id) || !mission_installed(&request.stage_id) {
        return false;
    }
    let Some(mission) = mission(&request.stage_id) else {
        return false;
    };
    request.stage_name = super::stage_presentation::display_name(mission.id, mission.name);
    request.mode = "single".into();
    request.character_id = 0;
    request.spawn_room_npcs = false;

    true
}

#[derive(Component)]
pub(crate) struct CampaignPin(pub &'static str);

#[derive(Component, Clone, Copy)]
pub(crate) struct CampaignStepArrow(pub i8);

#[derive(Component)]
struct MapViewport;
#[derive(Component)]
struct HeroShowcaseColumn;
#[derive(Component)]
struct MissionTitle;
#[derive(Component)]
struct MissionTime;
#[derive(Component)]
struct MissionBest;
#[derive(Component)]
struct MissionOrdinal;
#[derive(Component)]
struct MissionWeaponText;
#[derive(Component)]
struct DetailPanel;
#[derive(Component)]
struct PinLabel;
#[derive(Component)]
struct PinHalo(&'static str);

fn label(value: impl Into<String>, size: f32, color: Color, font: &Handle<Font>) -> impl Bundle {
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

fn action(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    title: &str,
    marker: impl Bundle,
) {
    parent
        .spawn((
            Button,
            LauncherStyledButton,
            marker,
            theme::ActionStyle::PRIMARY,
            Node {
                min_height: Val::Px(44.0),
                width: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(Val::Px(1.0)),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(theme::PRIMARY),
            BorderColor::all(theme::METAL),
        ))
        .with_children(|b| {
            b.spawn(label(title, 18.0, theme::TEXT, font));
        });
}

fn step_button(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, text: &str, step: i8) {
    parent
        .spawn((
            Button,
            LauncherStyledButton,
            CampaignStepArrow(step),
            theme::ActionStyle::SECONDARY,
            Node {
                min_height: Val::Px(44.0),
                height: Val::Px(44.0),
                padding: UiRect::horizontal(Val::Px(14.0)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(Val::Px(1.0)),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(theme::CARD),
            BorderColor::all(theme::LINE),
        ))
        .with_children(|b| {
            b.spawn(label(text, 14.0, theme::ACCENT_TEXT, font));
        });
}

const CHAPTER_NUMERALS: [&str; 9] = ["壹", "贰", "叁", "肆", "伍", "陆", "柒", "捌", "玖"];

pub(crate) fn spawn(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, active: bool) {
    parent
        .spawn((
            LauncherScreenMarker(MenuScreen::Stages),
            if active {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            },
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                display: if active { Display::Flex } else { Display::None },
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(10.0),
                ..default()
            },
        ))
        .with_children(|screen| {
            screen
                .spawn(Node {
                    width: Val::Percent(100.0),
                    min_height: Val::Px(44.0),
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(20.0),
                    flex_shrink: 0.0,
                    ..default()
                })
                .with_children(|header| {
                    header
                        .spawn(Node {
                            width: Val::Px(88.0),
                            ..default()
                        })
                        .with_children(|b| {
                            action(b, font, "返回", MenuBackButton);
                        });
                    header.spawn(label("单 人 闯 关", 26.0, theme::TEXT, font));
                    header.spawn(label("孟星魂 · 江湖九章", 13.0, theme::METAL, font));
                });

            screen
                .spawn((
                    MapViewport,
                    Node {
                        width: Val::Percent(100.0),
                        flex_grow: 1.0,
                        min_height: Val::Px(0.0),
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(16.0),
                        row_gap: Val::Px(12.0),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                ))
                .with_children(|viewport| {
                    viewport
                        .spawn((
                            HeroShowcaseColumn,
                            theme::OrnateFrame,
                            Node {
                                width: Val::Percent(52.0),
                                height: Val::Percent(100.0),
                                min_width: Val::Px(0.0),
                                min_height: Val::Px(180.0),
                                flex_direction: FlexDirection::Column,
                                justify_content: JustifyContent::SpaceBetween,
                                padding: UiRect::all(Val::Px(22.0)),
                                border: UiRect::all(Val::Px(1.0)),
                                overflow: Overflow::clip(),
                                ..default()
                            },
                            BackgroundColor(theme::SURFACE.with_alpha(0.72)),
                            BorderColor::all(theme::LINE),
                        ))
                        .with_children(|stage| {
                            stage.spawn((
                                CharacterPreviewViewport,
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(0.0),
                                    right: Val::Px(0.0),
                                    top: Val::Px(0.0),
                                    bottom: Val::Px(0.0),
                                    width: Val::Percent(100.0),
                                    height: Val::Percent(100.0),
                                    ..default()
                                },
                                ZIndex(0),
                            ));

                            stage
                                .spawn((
                                    Node {
                                        flex_direction: FlexDirection::Column,
                                        row_gap: Val::Px(6.0),
                                        padding: UiRect::left(Val::Px(12.0)),
                                        border: UiRect::left(Val::Px(2.0)),
                                        ..default()
                                    },
                                    BorderColor::all(theme::ACCENT_TEXT),
                                    ZIndex(2),
                                ))
                                .with_children(|titles| {
                                    titles.spawn((
                                        MissionOrdinal,
                                        label("单人任务   01 / 09", 12.0, theme::METAL, font),
                                    ));
                                    titles.spawn((
                                        MissionTitle,
                                        label("炎硫岛", 34.0, theme::TEXT, font),
                                    ));
                                });

                            stage
                                .spawn((
                                    Node {
                                        width: Val::Percent(100.0),
                                        flex_direction: FlexDirection::Column,
                                        row_gap: Val::Px(5.0),
                                        padding: UiRect::axes(Val::Px(14.0), Val::Px(10.0)),
                                        border: UiRect {
                                            left: Val::Px(2.0),
                                            bottom: Val::Px(1.0),
                                            ..default()
                                        },
                                        ..default()
                                    },
                                    BackgroundColor(theme::SCRIM.with_alpha(0.88)),
                                    BorderColor::all(theme::METAL.with_alpha(0.65)),
                                    ZIndex(2),
                                ))
                                .with_children(|pedestal| {
                                    pedestal.spawn(label(
                                        "出战人物 · 孟星魂",
                                        12.0,
                                        theme::METAL,
                                        font,
                                    ));
                                    pedestal.spawn((
                                        MissionWeaponText,
                                        label(
                                            mission_loadout_summary(&MISSIONS[0]),
                                            16.0,
                                            theme::ACCENT_TEXT,
                                            font,
                                        ),
                                    ));
                                });
                        });

                    viewport
                        .spawn((
                            DetailPanel,
                            theme::OrnateFrame,
                            scrollable_y(Node {
                                width: Val::Percent(48.0),
                                height: Val::Percent(100.0),
                                min_width: Val::Px(0.0),
                                min_height: Val::Px(0.0),
                                padding: UiRect::all(Val::Px(20.0)),
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(12.0),
                                border: UiRect::left(Val::Px(2.0)),
                                ..default()
                            }),
                            BackgroundColor(theme::SURFACE),
                            BorderColor::all(theme::METAL),
                        ))
                        .with_children(|panel| {
                            panel
                                .spawn((
                                    Node {
                                        width: Val::Percent(100.0),
                                        height: Val::Px(168.0),
                                        flex_shrink: 0.0,
                                        overflow: Overflow::clip(),
                                        border: UiRect::all(Val::Px(1.0)),
                                        flex_direction: FlexDirection::Column,
                                        justify_content: JustifyContent::FlexEnd,
                                        ..default()
                                    },
                                    BackgroundColor(theme::BACKGROUND),
                                    BorderColor::all(theme::METAL.with_alpha(0.45)),
                                ))
                                .with_children(|banner| {
                                    banner.spawn((
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
                                        MapImage,
                                    ));
                                    banner
                                        .spawn((
                                            Node {
                                                width: Val::Percent(100.0),
                                                padding: UiRect::axes(Val::Px(12.0), Val::Px(6.0)),
                                                justify_content: JustifyContent::SpaceBetween,
                                                align_items: AlignItems::Center,
                                                border: UiRect::top(Val::Px(1.0)),
                                                ..default()
                                            },
                                            BackgroundColor(theme::SURFACE.with_alpha(0.88)),
                                            BorderColor::all(theme::LINE),
                                        ))
                                        .with_children(|meta| {
                                            meta.spawn((
                                                MissionTime,
                                                label("", 12.0, theme::MUTED, font),
                                            ));
                                            meta.spawn((
                                                MissionBest,
                                                label("", 12.0, theme::METAL, font),
                                            ));
                                        });
                                });

                            panel.spawn((
                                super::stage_presentation::StageDescription::default(),
                                label("", 14.0, theme::TEXT, font),
                            ));

                            panel.spawn((
                                Node {
                                    width: Val::Percent(100.0),
                                    height: Val::Px(1.0),
                                    flex_shrink: 0.0,
                                    ..default()
                                },
                                BackgroundColor(theme::LINE),
                            ));

                            panel.spawn(label("选择关卡章节", 12.0, theme::METAL, font));
                            panel
                                .spawn(Node {
                                    width: Val::Percent(100.0),
                                    flex_direction: FlexDirection::Row,
                                    flex_wrap: FlexWrap::Wrap,
                                    column_gap: Val::Percent(2.0),
                                    row_gap: Val::Px(8.0),
                                    ..default()
                                })
                                .with_children(|grid| {
                                    for (idx, m) in MISSIONS.iter().enumerate() {
                                        let numeral =
                                            CHAPTER_NUMERALS.get(idx).copied().unwrap_or("章");
                                        grid.spawn((
                                            Button,
                                            LauncherStyledButton,
                                            CampaignPin(m.id),
                                            Node {
                                                width: Val::Percent(32.0),
                                                min_height: Val::Px(44.0),
                                                align_items: AlignItems::Center,
                                                justify_content: JustifyContent::Center,
                                                ..default()
                                            },
                                            BackgroundColor(Color::NONE),
                                        ))
                                        .with_children(
                                            |pin| {
                                                pin.spawn((
                                                    PinHalo(m.id),
                                                    Node {
                                                        position_type: PositionType::Absolute,
                                                        width: Val::Px(28.0),
                                                        height: Val::Px(28.0),
                                                        right: Val::Px(4.0),
                                                        ..default()
                                                    },
                                                ));
                                                pin.spawn((
                                                    PinLabel,
                                                    label(
                                                        format!(
                                                            "{numeral} · {}",
                                                            super::stage_presentation::display_name(
                                                                m.id, m.name
                                                            )
                                                        ),
                                                        14.0,
                                                        theme::TEXT,
                                                        font,
                                                    ),
                                                    Node {
                                                        width: Val::Percent(100.0),
                                                        height: Val::Percent(100.0),
                                                        padding: UiRect::axes(
                                                            Val::Px(10.0),
                                                            Val::Px(8.0),
                                                        ),
                                                        border: UiRect {
                                                            left: Val::Px(2.0),
                                                            bottom: Val::Px(1.0),
                                                            ..default()
                                                        },
                                                        align_items: AlignItems::Center,
                                                        ..default()
                                                    },
                                                    BorderColor::all(theme::LINE),
                                                    BackgroundColor(
                                                        theme::SURFACE.with_alpha(0.85),
                                                    ),
                                                ));
                                            },
                                        );
                                    }
                                });
                        });
                });
            screen
                .spawn(Node {
                    align_self: AlignSelf::FlexEnd,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(8.0),
                    flex_shrink: 0.0,
                    ..default()
                })
                .with_children(|bar| {
                    step_button(bar, font, "← 上一关", -1);
                    step_button(bar, font, "下一关 →", 1);
                    bar.spawn(Node {
                        min_width: Val::Px(110.0),
                        ..default()
                    })
                    .with_children(|go| {
                        action(go, font, "出 发", StartGameButton);
                    });
                });
        });
}

pub(crate) struct CampaignUiPlugin;
impl Plugin for CampaignUiPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CampaignProgress::load())
            .init_resource::<CampaignRun>()
            .add_systems(OnEnter(AppState::Playing), begin_campaign_run)
            .add_systems(
                OnEnter(AppState::Result),
                (record_campaign_result, save_campaign_progress).chain(),
            )
            .add_systems(
                Update,
                (attach_art, fit_map, selection, feedback)
                    .chain()
                    .after(super::launcher_ui::adapt_launcher_layout)
                    .after(super::ui_plugin::refresh_selection_visuals),
            );
    }
}

fn attach_art(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<theme::CalligraphyMaterial>>,
    halos: Query<Entity, Added<PinHalo>>,
    mut art: Local<Option<Handle<theme::CalligraphyMaterial>>>,
) {
    if halos.is_empty() {
        return;
    }
    let halo = art
        .get_or_insert_with(|| {
            let image = images.add(
                Image::from_buffer(
                    &crate::resource_io::required("ui/campaign-halo.png"),
                    ImageType::Extension("png"),
                    CompressedImageFormats::NONE,
                    true,
                    ImageSampler::linear(),
                    RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
                )
                .expect("embedded campaign art"),
            );
            materials.add(theme::CalligraphyMaterial { image })
        })
        .clone();
    for entity in &halos {
        commands.entity(entity).insert(MaterialNode(halo.clone()));
    }
}

fn fit_map(
    viewports: Query<&ComputedNode, With<MapViewport>>,
    mut layout_roots: Query<&mut Node, With<MapViewport>>,
    mut columns: Query<
        (&mut Node, Has<HeroShowcaseColumn>, Has<DetailPanel>),
        (
            Or<(With<HeroShowcaseColumn>, With<DetailPanel>)>,
            Without<MapViewport>,
        ),
    >,
) {
    let Ok(viewport) = viewports.single() else {
        return;
    };
    let size = viewport.size() * viewport.inverse_scale_factor;
    if size.x <= 0.0 || size.y <= 0.0 {
        return;
    }
    let portrait = size.x < 680.0 && size.y > size.x;
    for mut root in &mut layout_roots {
        root.flex_direction = if portrait {
            FlexDirection::Column
        } else {
            FlexDirection::Row
        };
    }
    for (mut node, is_hero, is_detail) in &mut columns {
        if is_hero {
            node.width = if portrait {
                Val::Percent(100.0)
            } else {
                Val::Percent(52.0)
            };
            node.height = if portrait {
                Val::Auto
            } else {
                Val::Percent(100.0)
            };
            node.flex_basis = if portrait { Val::Px(0.0) } else { Val::Auto };
            node.flex_grow = if portrait { 0.44 } else { 0.0 };
        } else if is_detail {
            node.width = if portrait {
                Val::Percent(100.0)
            } else {
                Val::Percent(48.0)
            };
            node.height = if portrait {
                Val::Auto
            } else {
                Val::Percent(100.0)
            };
            node.flex_basis = if portrait { Val::Px(0.0) } else { Val::Auto };
            node.flex_grow = if portrait { 0.56 } else { 0.0 };
            node.border = if portrait {
                UiRect::top(Val::Px(2.0))
            } else {
                UiRect::left(Val::Px(2.0))
            };
        }
    }
}

fn select(request: &mut LaunchRequest, tab: &mut StageListTab, m: &Mission) {
    if request.stage_id != m.id || request.mode != "single" {
        request.stage_id = m.id.into();
        request.stage_name = super::stage_presentation::display_name(m.id, m.name);
        request.mode = "single".into();
        request.character_id = 0;
        request.spawn_room_npcs = false;
    }
    if *tab != StageListTab::Official {
        *tab = StageListTab::Official;
    }
}

fn selection(
    screen: Res<MenuScreen>,
    progress: Res<CampaignProgress>,
    mut tab: ResMut<StageListTab>,
    mut request: ResMut<LaunchRequest>,
    pins: Query<(&Interaction, &CampaignPin), Changed<Interaction>>,
    arrows: Query<(&Interaction, &CampaignStepArrow), Changed<Interaction>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mut next_state: Option<ResMut<NextState<AppState>>>,
) {
    if *screen != MenuScreen::Stages {
        return;
    }
    let unlocked: Vec<_> = MISSIONS
        .iter()
        .take(progress.unlocked_count())
        .filter(|mission| mission_installed(mission.id))
        .collect();
    let count = unlocked.len();
    if count == 0 {
        return;
    }
    let mut index = unlocked
        .iter()
        .position(|m| m.id == request.stage_id)
        .unwrap_or(0);
    for (interaction, pin) in &pins {
        if *interaction == Interaction::Pressed {
            if let Some(i) = unlocked.iter().position(|m| m.id == pin.0) {
                index = i;
            }
        }
    }
    for (interaction, arrow) in &arrows {
        if *interaction == Interaction::Pressed {
            index = (index as isize + arrow.0 as isize).rem_euclid(count as isize) as usize;
        }
    }
    if let Some(keys) = keys {
        let step =
            if keys.any_just_pressed([KeyCode::ArrowRight, KeyCode::ArrowDown, KeyCode::KeyD]) {
                1
            } else if keys.any_just_pressed([KeyCode::ArrowLeft, KeyCode::ArrowUp, KeyCode::KeyA]) {
                -1
            } else {
                0
            };
        if step != 0 {
            index = (index as isize + step).rem_euclid(count as isize) as usize;
        }
        select(&mut request, &mut tab, unlocked[index]);
        if keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter])
            && prepare_launch(&mut request, &progress)
        {
            if let Some(ref mut state) = next_state {
                state.set(AppState::Loading);
            }
        }
        return;
    }
    select(&mut request, &mut tab, unlocked[index]);
}

#[allow(clippy::type_complexity)]
fn feedback(
    request: Res<LaunchRequest>,
    progress: Res<CampaignProgress>,
    records: Option<Res<super::battle_result::timing::BestTimes>>,
    mut pins: Query<(&CampaignPin, &Interaction, &Children, &mut Node)>,
    mut start: Query<&mut Node, (With<StartGameButton>, Without<CampaignPin>)>,
    mut labels: Query<(&mut TextColor, &mut BackgroundColor), With<PinLabel>>,
    mut halos: Query<(&PinHalo, &mut Visibility)>,
    mut title_cache: Local<(&'static str, String)>,
    mut copy: Query<
        (
            &mut Text,
            Option<&MissionTitle>,
            Option<&MissionTime>,
            Option<&MissionBest>,
            Option<&MissionOrdinal>,
            Option<&MissionWeaponText>,
        ),
        Or<(
            With<MissionTitle>,
            With<MissionTime>,
            With<MissionBest>,
            With<MissionOrdinal>,
            With<MissionWeaponText>,
        )>,
    >,
) {
    let mut installed = MISSIONS
        .iter()
        .take(progress.unlocked_count())
        .filter(|mission| mission_installed(mission.id));
    let first = installed.clone().next();
    let m = installed
        .find(|mission| mission.id == request.stage_id)
        .or(first);
    for mut node in &mut start {
        node.display = if m.is_some() {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (mut text, title, time, best, ordinal, weapon_text) in &mut copy {
        let Some(m) = m else {
            text.0 = if title.is_some() {
                "缺少战役资源".into()
            } else {
                String::new()
            };
            continue;
        };
        if best.is_some() {
            let value = records.as_ref().and_then(|r| r.best(m.id));
            text.0 = format!(
                "历史最佳   {}",
                value.map_or_else(
                    || "暂无通关纪录".into(),
                    |ms| super::battle_result::timing::format_time(Some(ms))
                )
            );
        }
        if title.is_some() {
            if title_cache.0 != m.id {
                *title_cache = (m.id, super::stage_presentation::display_name(m.id, m.name));
            }
            if text.0 != title_cache.1 {
                text.0.clone_from(&title_cache.1);
            }
        }
        if time.is_some() {
            text.0 = format!("任务时限   {} 分钟", m.minutes);
        }
        if ordinal.is_some() {
            text.0 = format!(
                "单人任务   {:02} / 09",
                MISSIONS.iter().position(|v| v.id == m.id).unwrap() + 1
            );
        }
        if weapon_text.is_some() {
            text.0 = mission_loadout_summary(m);
        }
    }
    for (pin, interaction, children, mut node) in &mut pins {
        node.display = if progress.is_unlocked(pin.0) && mission_installed(pin.0) {
            Display::Flex
        } else {
            Display::None
        };
        let selected = m.is_some_and(|mission| pin.0 == mission.id);
        for child in children.iter() {
            if let Ok((mut color, mut background)) = labels.get_mut(child) {
                color.0 = if selected {
                    theme::ACCENT_TEXT
                } else {
                    theme::TEXT
                };
                background.0 = if selected {
                    theme::PRIMARY
                } else if *interaction != Interaction::None {
                    theme::HOVER
                } else {
                    theme::SURFACE.with_alpha(0.85)
                };
            }
        }
    }
    for (halo, mut visibility) in &mut halos {
        *visibility = if m.is_some_and(|mission| halo.0 == mission.id) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn click_keyboard_and_persistent_selection_use_the_same_campaign() {
        let mut app = App::new();
        app.insert_resource(CampaignProgress { completed: 9 })
            .insert_resource(MenuScreen::Stages)
            .insert_resource(StageListTab::Custom)
            .insert_resource(LaunchRequest {
                stage_id: "sn02".into(),
                ..default()
            })
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<NextState<AppState>>()
            .add_systems(Update, (selection, feedback).chain());
        let pin = app
            .world_mut()
            .spawn((CampaignPin("sn03"), Interaction::Pressed, Node::default()))
            .id();
        let label = app
            .world_mut()
            .spawn((
                PinLabel,
                TextColor(Color::WHITE),
                BackgroundColor(Color::NONE),
            ))
            .id();
        app.world_mut().entity_mut(pin).add_child(label);
        let halo = app
            .world_mut()
            .spawn((PinHalo("sn03"), Visibility::Hidden))
            .id();
        app.update();
        assert_eq!(app.world().resource::<LaunchRequest>().stage_id, "sn03");
        *app.world_mut().get_mut::<Interaction>(pin).unwrap() = Interaction::None;
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(halo).unwrap(),
            Visibility::Inherited
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ArrowRight);
        app.update();
        assert_eq!(app.world().resource::<LaunchRequest>().stage_id, "sn13");
        assert_eq!(
            *app.world().get::<Visibility>(halo).unwrap(),
            Visibility::Hidden
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        let next_btn = app
            .world_mut()
            .spawn((CampaignStepArrow(1), Interaction::Pressed))
            .id();
        app.update();
        assert_eq!(app.world().resource::<LaunchRequest>().stage_id, "sn04");
        *app.world_mut().get_mut::<Interaction>(next_btn).unwrap() = Interaction::None;
        app.world_mut().resource_mut::<LaunchRequest>().character_id = 19;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Enter);
        app.update();
        assert_eq!(app.world().resource::<LaunchRequest>().character_id, 0);
        assert!(matches!(
            app.world().resource::<NextState<AppState>>(),
            NextState::Pending(AppState::Loading)
        ));
    }
    #[test]
    fn campaign_catalog_matches_real_mission_scripts_and_times() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
        let mut authored = Vec::new();
        for n in 1..=22 {
            let id = format!("sn{n:02}");
            let Ok(bytes) = std::fs::read(root.join(format!("{id}_.pst"))) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            if text.contains("OnUpdate()") && text.contains("AddNPC(") {
                authored.push(id.clone());
                let m = mission(&id).unwrap();
                assert!(text.contains(&format!("int RoundTime = {};", m.minutes)));
                assert!(text.contains(&format!("int PlayerWeapon = {};", m.primary_item)));
                assert!(text.contains(&format!("int PlayerWeapon2 = {};", m.secondary_item)));
            }
        }
        let mut listed = MISSIONS.iter().map(|m| m.id.to_owned()).collect::<Vec<_>>();
        listed.sort();
        assert_eq!(authored, listed);
        assert_eq!(meteor_scene::stage_registry::official_stages().len(), 22);
    }
    #[test]
    fn campaign_progression_matches_original_registry_and_has_no_wrap() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let registry =
            meteor_asset::res::parse_meteor_res(&std::fs::read(root.join("Meteor.res")).unwrap())
                .unwrap();
        let mut authored: Vec<_> = registry
            .records
            .iter()
            .filter(|r| r.category == 5)
            .filter_map(|r| {
                mission(&r.name.to_ascii_lowercase()).map(|m| (r.prop_seed_value(1).unwrap(), m.id))
            })
            .collect();
        authored.sort();
        assert_eq!(
            authored.iter().map(|(rank, _)| *rank).collect::<Vec<_>>(),
            (0..9).collect::<Vec<_>>()
        );
        assert_eq!(
            MISSIONS.iter().map(|m| m.id).collect::<Vec<_>>(),
            authored.iter().map(|(_, id)| *id).collect::<Vec<_>>()
        );
        for pair in authored.windows(2) {
            assert_eq!(next_mission(pair[0].1).unwrap().id, pair[1].1);
        }
        assert_eq!(MISSIONS[0].id, "sn14");
        assert!(next_mission("sn10").is_none());
        assert!(next_mission("sn02").is_none());
        assert!(next_mission("custom01").is_none());
    }

    #[test]
    fn campaign_launch_rejects_arena_and_custom_and_resets_lan_character() {
        let progress = CampaignProgress { completed: 9 };
        for id in ["sn02", "sn06", "sn22", "custom01", "snSample"] {
            let mut request = LaunchRequest {
                stage_id: id.into(),
                ..default()
            };
            assert!(!prepare_launch(&mut request, &progress));
        }
        let mut request = LaunchRequest {
            stage_id: "sn03".into(),
            character_id: 19,
            mode: "deathmatch".into(),
            spawn_room_npcs: true,
            ..default()
        };
        assert!(prepare_launch(&mut request, &progress));
        assert_eq!(request.character_id, 0);
        assert_eq!(request.mode, "single");
        assert!(!request.spawn_room_npcs);
    }

    #[test]
    fn campaign_order_and_unlocks_match_resource_progression() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let registry =
            meteor_asset::res::parse_meteor_res(&std::fs::read(root.join("Meteor.res")).unwrap())
                .unwrap();
        let mut authored: Vec<_> = registry
            .records
            .iter()
            .filter(|record| {
                record.category == 5
                    && record
                        .prop_seed_value(11)
                        .is_some_and(|mask| mask & 32 != 0)
            })
            .collect();
        authored.sort_by_key(|record| record.prop_seed_value(1).unwrap());
        assert_eq!(
            authored
                .iter()
                .map(|record| format!("sn{:02}", record.id))
                .collect::<Vec<_>>(),
            MISSIONS
                .iter()
                .map(|mission| mission.id)
                .collect::<Vec<_>>()
        );
        let mut progress = CampaignProgress::default();
        assert!(progress.is_unlocked("sn14"));
        assert!(!progress.is_unlocked("sn01"));
        assert!(!progress.complete("sn01"));
        assert!(progress.complete("sn14"));
        assert!(!progress.complete("sn14"));
        assert!(progress.is_unlocked("sn01"));
        assert!(!progress.is_unlocked("sn03"));
        let restored = CampaignProgress::from_text(&serde_json::to_string(&progress).unwrap());
        assert_eq!(restored.completed, 1);
        assert_eq!(CampaignProgress::from_text("bad save").completed, 0);
        assert_eq!(
            CampaignProgress::from_text("{\"completed\":999}").completed,
            9
        );
    }

    #[test]
    fn only_a_solo_victory_unlocks_the_next_campaign_stage_once() {
        let mut app = App::new();
        app.insert_resource(LaunchRequest {
            stage_id: "sn14".into(),
            ..default()
        })
        .insert_resource(crate::app_state::LoadedStageInfo {
            stage_id: "sn14".into(),
            ready: true,
            ..default()
        })
        .insert_resource(State::new(AppState::Playing))
        .init_resource::<CampaignProgress>()
        .init_resource::<CampaignRun>()
        .init_resource::<crate::components::fighter::MatchEnd>()
        .add_systems(Update, (begin_campaign_run, record_campaign_result).chain());
        for (mode, result, settled, expected) in [
            ("single", 4, false, 0),
            ("single", 2, true, 0),
            ("single", 1, true, 0),
            ("replay", 4, true, 0),
            ("leader", 4, true, 0),
            ("single", 4, true, 1),
            ("single", 4, true, 1),
        ] {
            app.insert_resource(State::new(AppState::Playing));
            app.world_mut().resource_mut::<LaunchRequest>().mode = mode.into();
            app.update();
            app.insert_resource(State::new(AppState::Result));
            *app.world_mut()
                .resource_mut::<crate::components::fighter::MatchEnd>() =
                crate::components::fighter::MatchEnd {
                    result,
                    settled,
                    ..default()
                };
            app.update();
            assert_eq!(
                app.world().resource::<CampaignProgress>().completed,
                expected
            );
        }
        app.insert_resource(State::new(AppState::Menu));
        app.world_mut().resource_mut::<LaunchRequest>().stage_id = "sn01".into();
        app.update();
        assert_eq!(
            app.world().resource::<CampaignProgress>().completed,
            1,
            "selecting the next mission must not reuse the previous victory"
        );
        let persisted = serde_json::to_string(app.world().resource::<CampaignProgress>()).unwrap();
        assert_eq!(CampaignProgress::from_text(&persisted).completed, 1);
    }

    #[test]
    fn campaign_result_uses_loaded_stage_identity_and_consumes_it_once() {
        let mut app = App::new();
        app.insert_resource(LaunchRequest {
            mode: "single".into(),
            stage_id: "sn14".into(),
            ..default()
        })
        .insert_resource(crate::app_state::LoadedStageInfo {
            stage_id: "sn14".into(),
            ready: true,
            ..default()
        })
        .insert_resource(State::new(AppState::Playing))
        .init_resource::<CampaignProgress>()
        .init_resource::<CampaignRun>()
        .init_resource::<crate::components::fighter::MatchEnd>()
        .add_systems(Update, (begin_campaign_run, record_campaign_result).chain());
        app.update();
        app.world_mut().resource_mut::<LaunchRequest>().stage_id = "sn01".into();
        app.insert_resource(State::new(AppState::Result));
        *app.world_mut()
            .resource_mut::<crate::components::fighter::MatchEnd>() =
            crate::components::fighter::MatchEnd {
                result: 4,
                settled: true,
                ..default()
            };
        app.update();
        assert_eq!(app.world().resource::<CampaignProgress>().completed, 1);
        app.update();
        assert_eq!(app.world().resource::<CampaignProgress>().completed, 1);
    }
}
