use super::ui_theme as theme;
use crate::app_state::{
    AppState, LaunchRequest, LauncherScreenMarker, LauncherStyledButton, MenuBackButton,
    MenuScreen, StageListTab, StartGameButton,
};
use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;

pub(crate) struct Mission {
    pub id: &'static str,
    pub name: &'static str,
    pub position: Vec2,
    pub minutes: u32,
    pub description: &'static str,
}

pub(crate) const MISSIONS: &[Mission] = &[
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
        id: "sn04",
        name: "炽雪城",
        position: Vec2::new(0.39, 0.14),
        minutes: 20,
        description: "雪落孤城，兵戈未息。\n城墙之内，危机四伏。",
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
    Mission {
        id: "sn13",
        name: "金华城",
        position: Vec2::new(0.38, 0.42),
        minutes: 15,
        description: "烟水绕城，长街入夜。\n刀光隐于街巷，步步皆需留心。",
    },
    Mission {
        id: "sn14",
        name: "炎硫岛",
        position: Vec2::new(0.15, 0.76),
        minutes: 10,
        description: "孤岛临海，赤焰映空。\n江湖路远，从此踏上征途。",
    },
    Mission {
        id: "sn15",
        name: "飞鹏堡",
        position: Vec2::new(0.39, 0.70),
        minutes: 25,
        description: "危堡高悬，云断山门。\n越过重重险隘，踏入堡中。",
    },
];

pub(crate) fn mission(id: &str) -> Option<&'static Mission> {
    MISSIONS.iter().find(|m| m.id == id)
}

pub(crate) fn prepare_launch(request: &mut LaunchRequest) -> bool {
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
                            panel.spawn((MissionTitle, label("钟乳洞", 32.0, theme::TEXT, font)));
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
        app.add_systems(
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
    mut request: ResMut<LaunchRequest>,
    mut tab: ResMut<StageListTab>,
    mut next: Option<ResMut<NextState<AppState>>>,
) {
    if *screen != MenuScreen::Stages || state.is_some_and(|s| *s.get() != AppState::Menu) {
        return;
    }
    *tab = StageListTab::Official;
    if mission(&request.stage_id).is_none() {
        request.stage_id = MISSIONS[0].id.into();
    }
    for (pin, interaction) in &pins {
        if *interaction == Interaction::Pressed {
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
                [(index as i32 + direction).rem_euclid(MISSIONS.len() as i32) as usize]
                .id
                .into();
        }
        if keys.just_pressed(KeyCode::Enter) && prepare_launch(&mut request) {
            if let Some(next) = next.as_mut() {
                next.set(AppState::Loading);
            }
        }
    }
    request.stage_name = mission(&request.stage_id).unwrap().name.into();
}

fn feedback(
    request: Res<LaunchRequest>,
    mut texts: Query<(
        &mut Text,
        Option<&MissionTitle>,
        Option<&MissionDescription>,
        Option<&MissionTime>,
        Option<&MissionOrdinal>,
    )>,
    pins: Query<(&CampaignPin, &Interaction, &Children)>,
    mut labels: Query<(&mut TextColor, &mut BackgroundColor), With<PinLabel>>,
    mut halos: Query<(&PinHalo, &mut Visibility)>,
) {
    let Some(m) = mission(&request.stage_id) else {
        return;
    };
    for (mut text, title, description, time, ordinal) in &mut texts {
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
    for (pin, interaction, children) in &pins {
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
        app.insert_resource(MenuScreen::Stages)
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
            .spawn((CampaignPin("sn03"), Interaction::Pressed))
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
        assert_eq!(app.world().resource::<LaunchRequest>().stage_id, "sn04");
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
        assert_eq!(
            authored,
            MISSIONS.iter().map(|m| m.id.to_owned()).collect::<Vec<_>>()
        );
        assert_eq!(meteor_scene::stage_registry::official_stages().len(), 22);
    }
    #[test]
    fn campaign_launch_rejects_arena_and_custom_and_resets_lan_character() {
        for id in ["sn02", "sn06", "sn22", "custom01", "snSample"] {
            let mut request = LaunchRequest {
                stage_id: id.into(),
                ..default()
            };
            assert!(!prepare_launch(&mut request));
        }
        let mut request = LaunchRequest {
            stage_id: "sn03".into(),
            character_id: 19,
            mode: "deathmatch".into(),
            spawn_room_npcs: true,
            ..default()
        };
        assert!(prepare_launch(&mut request));
        assert_eq!(request.character_id, 0);
        assert_eq!(request.mode, "single");
        assert!(!request.spawn_room_npcs);
    }
}
