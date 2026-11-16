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
    pub description: &'static str,
}

pub(crate) const MISSIONS: &[Mission] = &[
    Mission {
        id: "sn14",
        name: "炎硫岛",
        position: Vec2::new(0.15, 0.76),
        minutes: 10,
        description: "孤岛临海，赤焰映空。\n江湖路远，从此踏上征途。",
    },
    Mission {
        id: "sn01",
        name: "钟乳洞",
        position: Vec2::new(0.14, 0.43),
        minutes: 10,
        description: "幽谷深处，洞壑纵横。\n循着暗处的声息，踏入江湖。",
    },
    Mission {
        id: "sn03",
        name: "一线天",
        position: Vec2::new(0.17, 0.19),
        minutes: 15,
        description: "两崖相峙，一线通天。\n夺取镖物，返回己方安全区。",
    },
    Mission {
        id: "sn13",
        name: "金华城",
        position: Vec2::new(0.38, 0.42),
        minutes: 15,
        description: "烟水绕城，长街入夜。\n刀光隐于街巷，步步皆需留心。",
    },
    Mission {
        id: "sn04",
        name: "炽雪城",
        position: Vec2::new(0.39, 0.14),
        minutes: 20,
        description: "雪落孤城，兵戈未息。\n城墙之内，危机四伏。",
    },
    Mission {
        id: "sn15",
        name: "飞鹏堡",
        position: Vec2::new(0.39, 0.70),
        minutes: 25,
        description: "危堡高悬，云断山门。\n越过重重险隘，踏入堡中。",
    },
    Mission {
        id: "sn05",
        name: "皇天城",
        position: Vec2::new(0.65, 0.20),
        minutes: 30,
        description: "重门深锁，楼阙森严。\n穿过繁华，直面暗藏的杀机。",
    },
    Mission {
        id: "sn09",
        name: "决死阵",
        position: Vec2::new(0.61, 0.48),
        minutes: 5,
        description: "绝壁断路，生死一线。\n方寸之间，须臾定胜负。",
    },
    Mission {
        id: "sn10",
        name: "炼铁狱",
        position: Vec2::new(0.65, 0.77),
        minutes: 30,
        description: "炉火不熄，铁壁如狱。\n深入险地，寻找破局之机。",
    },
];

pub(crate) fn mission(id: &str) -> Option<&'static Mission> {
    MISSIONS.iter().find(|m| m.id == id)
}

pub(crate) fn next_mission(id: &str) -> Option<&'static Mission> {
    let index = MISSIONS.iter().position(|mission| mission.id == id)?;
    MISSIONS.get(index + 1)
}

pub(crate) fn prepare_launch(request: &mut LaunchRequest, progress: &CampaignProgress) -> bool {
    if !progress.is_unlocked(&request.stage_id) {
        return false;
    }
    let Some(mission) = mission(&request.stage_id) else {
        return false;
    };
    request.stage_name = mission.name.into();
    request.mode = "single".into();
    request.character_id = 0;
    request.spawn_room_npcs = false;

    true
}

#[derive(Component)]
pub(crate) struct CampaignPin(pub &'static str);
#[derive(Component)]
struct MapViewport;
#[derive(Component)]
struct MapCanvas;
#[derive(Component)]
struct MapArt;
#[derive(Component)]
struct MissionTitle;
#[derive(Component)]
struct MissionDescription;
#[derive(Component)]
struct MissionTime;
#[derive(Component)]
struct MissionBest;
#[derive(Component)]
struct MissionOrdinal;
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
                row_gap: Val::Px(8.0),
                ..default()
            },
        ))
        .with_children(|screen| {
            screen
                .spawn(Node {
                    width: Val::Percent(100.0),
                    min_height: Val::Px(44.0),
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(22.0),
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
                    header.spawn(label("江 湖 舆 图", 26.0, theme::TEXT, font));
                    header.spawn(label("单人闯关 · 九处风云", 13.0, theme::METAL, font));
                });
            screen
                .spawn((
                    MapViewport,
                    Node {
                        width: Val::Percent(100.0),
                        flex_grow: 1.0,
                        min_height: Val::Px(0.0),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.065, 0.059, 0.045)),
                ))
                .with_children(|viewport| {
                    viewport
                        .spawn((
                            MapCanvas,
                            Node {
                                position_type: PositionType::Absolute,
                                width: Val::Percent(100.0),
                                height: Val::Percent(100.0),
                                ..default()
                            },
                        ))
                        .with_children(|map| {
                            map.spawn((
                                MapArt,
                                Node {
                                    position_type: PositionType::Absolute,
                                    width: Val::Percent(100.0),
                                    height: Val::Percent(100.0),
                                    ..default()
                                },
                            ));
                            for m in MISSIONS {
                                map.spawn((
                                    Button,
                                    LauncherStyledButton,
                                    CampaignPin(m.id),
                                    Node {
                                        position_type: PositionType::Absolute,
                                        left: Val::Percent(m.position.x * 100.0),
                                        top: Val::Percent(m.position.y * 100.0),
                                        margin: UiRect::new(
                                            Val::Px(-42.0),
                                            Val::ZERO,
                                            Val::Px(-22.0),
                                            Val::ZERO,
                                        ),
                                        width: Val::Px(84.0),
                                        height: Val::Px(64.0),
                                        flex_direction: FlexDirection::Column,
                                        align_items: AlignItems::Center,
                                        justify_content: JustifyContent::Center,
                                        ..default()
                                    },
                                    BackgroundColor(Color::NONE),
                                    ZIndex(2),
                                ))
                                .with_children(|pin| {
                                    pin.spawn((
                                        PinHalo(m.id),
                                        Node {
                                            position_type: PositionType::Absolute,
                                            width: Val::Px(66.0),
                                            height: Val::Px(66.0),
                                            top: Val::Px(-15.0),
                                            ..default()
                                        },
                                    ));
                                    pin.spawn((
                                        Node {
                                            width: Val::Px(18.0),
                                            height: Val::Px(18.0),
                                            border: UiRect::all(Val::Px(2.0)),
                                            border_radius: BorderRadius::all(Val::Px(9.0)),
                                            margin: UiRect::bottom(Val::Px(5.0)),
                                            ..default()
                                        },
                                        BackgroundColor(theme::PRIMARY),
                                        BorderColor::all(theme::METAL),
                                    ));
                                    pin.spawn((
                                        PinLabel,
                                        label(m.name, 16.0, theme::TEXT, font),
                                        Node {
                                            padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                                            ..default()
                                        },
                                        BackgroundColor(Color::srgba(0.04, 0.035, 0.025, 0.85)),
                                    ));
                                });
                            }
                        });
                    viewport
                        .spawn((
                            DetailPanel,
                            Node {
                                position_type: PositionType::Absolute,
                                right: Val::Px(12.0),
                                top: Val::Px(14.0),
                                bottom: Val::Px(14.0),
                                width: Val::Percent(26.0),
                                padding: UiRect::all(Val::Px(22.0)),
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(16.0),
                                border: UiRect::left(Val::Px(2.0)),
                                overflow: Overflow::scroll_y(),
                                ..default()
                            },
                            BackgroundColor(Color::srgba(0.045, 0.043, 0.034, 0.94)),
                            BorderColor::all(theme::METAL),
                        ))
                        .with_children(|panel| {
                            panel.spawn((
                                MissionOrdinal,
                                label("单人任务", 12.0, theme::METAL, font),
                            ));
                            panel.spawn((MissionTitle, label("炎硫岛", 32.0, theme::TEXT, font)));
                            panel.spawn((
                                Node {
                                    width: Val::Percent(100.0),
                                    height: Val::Px(1.0),
                                    flex_shrink: 0.0,
                                    ..default()
                                },
                                BackgroundColor(theme::LINE),
                            ));
                            panel.spawn((MissionDescription, label("", 15.0, theme::TEXT, font)));
                            panel.spawn(Node {
                                flex_grow: 1.0,
                                ..default()
                            });
                            panel.spawn(label("孟星魂", 16.0, theme::METAL, font));
                            panel.spawn((MissionTime, label("", 13.0, theme::MUTED, font)));
                            panel.spawn((MissionBest, label("", 13.0, theme::METAL, font)));
                            action(panel, font, "出 发", StartGameButton);
                        });
                });
            screen.spawn(label(
                "选择地点查看任务    ·    方向键切换    /    Enter 出发    /    Esc 返回",
                12.0,
                theme::METAL,
                font,
            ));
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
    maps: Query<Entity, Added<MapArt>>,
    halos: Query<Entity, Added<PinHalo>>,
    mut art: Local<Option<(Handle<Image>, Handle<theme::CalligraphyMaterial>)>>,
) {
    if maps.is_empty() && halos.is_empty() {
        return;
    }
    let (map, halo) = art.get_or_insert_with(|| {
        let mut decode = |bytes: &[u8]| {
            images.add(
                Image::from_buffer(
                    bytes,
                    ImageType::Extension("png"),
                    CompressedImageFormats::NONE,
                    true,
                    ImageSampler::linear(),
                    RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
                )
                .expect("embedded campaign art"),
            )
        };
        (
            decode(&crate::resource_io::required("ui/campaign-map.png")),
            materials.add(theme::CalligraphyMaterial {
                image: decode(&crate::resource_io::required("ui/campaign-halo.png")),
            }),
        )
    });
    for entity in &maps {
        commands.entity(entity).insert(ImageNode::new(map.clone()));
    }
    for entity in &halos {
        commands.entity(entity).insert(MaterialNode(halo.clone()));
    }
}

fn fit_map(
    viewports: Query<&ComputedNode, With<MapViewport>>,
    mut nodes: Query<
        (&mut Node, Option<&MapCanvas>, Option<&DetailPanel>),
        Or<(With<MapCanvas>, With<DetailPanel>)>,
    >,
) {
    let Ok(viewport) = viewports.single() else {
        return;
    };
    let size = viewport.size() * viewport.inverse_scale_factor;
    if size.x <= 0.0 || size.y <= 0.0 {
        return;
    }
    let portrait = size.x < 600.0 && size.y > size.x;
    let map_height = if portrait { size.y * 0.53 } else { size.y };

    let width = if portrait {
        (size.x / 0.74).min(map_height * 16.0 / 9.0)
    } else {
        size.x.min(map_height * 16.0 / 9.0)
    };
    let height = width * 9.0 / 16.0;
    for (mut node, canvas, panel) in &mut nodes {
        if canvas.is_some() {
            node.width = Val::Px(width);
            node.height = Val::Px(height);
            node.left = Val::Px(0.0);
            node.top = Val::Px((map_height - height) * 0.5);
        }
        if panel.is_some() {
            node.width = if portrait {
                Val::Percent(100.0)
            } else {
                Val::Percent(26.0)
            };
            node.top = if portrait {
                Val::Percent(53.0)
            } else {
                Val::Px(8.0)
            };
            node.right = Val::Px(if portrait { 0.0 } else { 8.0 });
            node.bottom = Val::Px(0.0);
            node.padding = UiRect::all(Val::Px(if size.y < 400.0 { 10.0 } else { 22.0 }));
            node.row_gap = Val::Px(if size.y < 400.0 { 7.0 } else { 16.0 });
        }
    }
}

fn selection(
    mut screen: ResMut<MenuScreen>,
    state: Option<Res<State<AppState>>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    pins: Query<(&CampaignPin, &Interaction), Changed<Interaction>>,
    progress: Res<CampaignProgress>,
    mut request: ResMut<LaunchRequest>,
    mut tab: ResMut<StageListTab>,
    mut next: Option<ResMut<NextState<AppState>>>,
) {
    if *screen != MenuScreen::Stages || state.is_some_and(|s| *s.get() != AppState::Menu) {
        return;
    }
    *tab = StageListTab::Official;
    if !progress.is_unlocked(&request.stage_id) {
        request.stage_id = MISSIONS[0].id.into();
    }
    for (pin, interaction) in &pins {
        if *interaction == Interaction::Pressed && progress.is_unlocked(pin.0) {
            request.stage_id = pin.0.into();
        }
    }
    if let Some(keys) = keys {
        if keys.just_pressed(KeyCode::Escape) {
            *screen = MenuScreen::Main;
            return;
        }
        let direction =
            if keys.just_pressed(KeyCode::ArrowRight) || keys.just_pressed(KeyCode::ArrowDown) {
                1
            } else if keys.just_pressed(KeyCode::ArrowLeft) || keys.just_pressed(KeyCode::ArrowUp) {
                -1
            } else {
                0
            };
        let index = MISSIONS
            .iter()
            .position(|m| m.id == request.stage_id)
            .unwrap_or(0);
        if direction != 0 {
            request.stage_id = MISSIONS
                [(index as i32 + direction).rem_euclid(progress.unlocked_count() as i32) as usize]
                .id
                .into();
        }
        if keys.just_pressed(KeyCode::Enter) && prepare_launch(&mut request, &progress) {
            if let Some(next) = next.as_mut() {
                next.set(AppState::Loading);
            }
        }
    }
    request.stage_name = mission(&request.stage_id).unwrap().name.into();
}

fn feedback(
    request: Res<LaunchRequest>,
    progress: Res<CampaignProgress>,
    records: Option<Res<super::battle_result::timing::BestTimes>>,
    mut texts: Query<(
        &mut Text,
        Option<&MissionTitle>,
        Option<&MissionDescription>,
        Option<&MissionTime>,
        Option<&MissionOrdinal>,
        Option<&MissionBest>,
    )>,
    mut pins: Query<(&CampaignPin, &Interaction, &Children, &mut Node)>,
    mut labels: Query<(&mut TextColor, &mut BackgroundColor), With<PinLabel>>,
    mut halos: Query<(&PinHalo, &mut Visibility)>,
) {
    let Some(m) = mission(&request.stage_id) else {
        return;
    };
    for (mut text, title, description, time, ordinal, best) in &mut texts {
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
            text.0 = m.name.into();
        }
        if description.is_some() {
            text.0 = m.description.into();
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
    }
    for (pin, interaction, children, mut node) in &mut pins {
        node.display = if progress.is_unlocked(pin.0) {
            Display::Flex
        } else {
            Display::None
        };
        let selected = pin.0 == m.id;
        for child in children.iter() {
            if let Ok((mut color, mut background)) = labels.get_mut(child) {
                color.0 = if selected {
                    Color::srgb(1.0, 0.86, 0.52)
                } else {
                    theme::TEXT
                };
                background.0 = if selected {
                    theme::PRIMARY
                } else if *interaction != Interaction::None {
                    theme::HOVER
                } else {
                    Color::srgba(0.04, 0.035, 0.025, 0.85)
                };
            }
        }
    }
    for (halo, mut visibility) in &mut halos {
        *visibility = if halo.0 == m.id {
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
                assert!(text.contains(&format!(
                    "int RoundTime = {};",
                    mission(&id).unwrap().minutes
                )));
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
