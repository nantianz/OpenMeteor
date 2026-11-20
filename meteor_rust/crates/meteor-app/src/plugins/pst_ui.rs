use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy::ui::{FocusPolicy, ScrollPosition};
use meteor_scene::pst_game::ScriptEvent;
use meteor_scene::pst_runtime::PstValue;
use meteor_scene::pst_ui::{PstUi, UiOwner, UiPanel};

use super::{
    launcher_ui::{LauncherScrollArea, UiFont},
    stage_plugin::{PlayerPerformance, StageMissionRuntime, StageScriptRuntime},
    ui_components::{UiDialogSurfaceBundle, UiInputBlocker},
    ui_plugin::{net_room_action, text},
    ui_theme as theme,
};
use crate::{app_state::AppState, components::fighter::MatchEnd};

#[derive(Resource, Default)]
pub(super) struct StagePstUi(pub Option<PstUi>);

#[derive(Component)]
pub(super) struct PstPanelRoot(UiPanel);

#[derive(Component)]
pub(crate) struct PstPanelButton {
    generation: u64,
    id: String,
}

pub(super) struct PstUiPlugin;

impl Plugin for PstUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StagePstUi>()
            .add_systems(PreUpdate, sync_modal_input)
            .add_systems(Update, sync_panel.after(dispatch_button));
    }
}

fn active_in_play(state: &State<AppState>, end: Option<&MatchEnd>) -> bool {
    *state.get() == AppState::Playing && !end.is_some_and(|end| end.settled)
}

pub(super) fn sync_modal_input(
    state: Res<State<AppState>>,
    end: Option<Res<MatchEnd>>,
    ui: Res<StagePstUi>,
    performance: Option<ResMut<PlayerPerformance>>,
) {
    if let Some(mut performance) = performance {
        performance.ui_modal =
            active_in_play(&state, end.as_deref()) && ui.0.as_ref().is_some_and(PstUi::has_modal);
    }
}

pub(super) fn sync_panel(
    mut commands: Commands,
    state: Res<State<AppState>>,
    end: Option<Res<MatchEnd>>,
    ui: Res<StagePstUi>,
    font: Option<Res<UiFont>>,
    mut images: Option<ResMut<Assets<Image>>>,
    roots: Query<(Entity, &PstPanelRoot)>,
) {
    let shown =
        ui.0.as_ref()
            .and_then(PstUi::visible)
            .filter(|panel| presents_panel(&state, end.as_deref(), panel.owner));
    if let Some(panel) = shown.as_ref() {
        if roots.single().is_ok_and(|(_, root)| &root.0 == panel) {
            return;
        }
    }
    for (entity, _) in &roots {
        commands.entity(entity).despawn();
    }
    if let (Some(panel), Some(font)) = (shown, font) {
        let pictures = panel.images.clone();
        let surface = spawn_panel(&mut commands, panel, &font);
        if let Some(images) = images.as_deref_mut() {
            for picture in pictures {
                let image =
                    super::stage_plugin::pst_extensions::read_installed_script_asset(&picture.path)
                        .filter(|bytes| bytes.len() <= 4 * 1024 * 1024)
                        .and_then(|bytes| {
                            let (extension, width, height) = picture_header(&bytes)?;
                            if !(1..=2048).contains(&width) || !(1..=2048).contains(&height) {
                                return None;
                            }
                            Image::from_buffer(
                                &bytes,
                                ImageType::Extension(extension),
                                CompressedImageFormats::NONE,
                                true,
                                ImageSampler::linear(),
                                RenderAssetUsages::default(),
                            )
                            .ok()
                        });
                if let Some(image) = image {
                    let image_size = image.size().as_vec2();
                    let [x0, y0, x1, y1] = picture
                        .uv
                        .map(|value| f32::from(value) / f32::from(u16::MAX));
                    let a = Vec2::new(x0, y0) * image_size;
                    let b = Vec2::new(x1, y1) * image_size;
                    let rect = Rect {
                        min: a.min(b),
                        max: a.max(b),
                    };
                    let size = rect.size();
                    let scale = (310.0 / size.x).min(165.0 / size.y).min(1.0);
                    let [r, g, b, alpha] = picture.tint;
                    let mut node = ImageNode::new(images.add(image));
                    node.rect = Some(rect);
                    node.flip_x = x0 > x1;
                    node.flip_y = y0 > y1;
                    node.color = Color::srgba_u8(r, g, b, alpha);
                    let child = commands
                        .spawn((
                            node,
                            Node {
                                width: Val::Px(size.x * scale),
                                height: Val::Px(size.y * scale),
                                ..default()
                            },
                        ))
                        .id();
                    commands.entity(surface).add_child(child);
                }
            }
        }
    }
}

fn picture_header(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let width = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
        let height = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
        return Some(("png", width, height));
    }
    if !bytes.starts_with(b"\xff\xd8") {
        return None;
    }
    let mut cursor = 2;
    while bytes.get(cursor) == Some(&0xff) {
        while bytes.get(cursor) == Some(&0xff) {
            cursor += 1;
        }
        let marker = *bytes.get(cursor)?;
        cursor += 1;
        if matches!(marker, 0xd9 | 0xda) {
            return None;
        }
        let size = usize::from(u16::from_be_bytes(
            bytes.get(cursor..cursor + 2)?.try_into().ok()?,
        ));
        if size < 2 {
            return None;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if size < 7 {
                return None;
            }
            let height = u16::from_be_bytes(bytes.get(cursor + 3..cursor + 5)?.try_into().ok()?);
            let width = u16::from_be_bytes(bytes.get(cursor + 5..cursor + 7)?.try_into().ok()?);
            return Some(("jpg", u32::from(width), u32::from(height)));
        }
        cursor += size;
    }
    None
}

fn presents_panel(state: &State<AppState>, end: Option<&MatchEnd>, owner: UiOwner) -> bool {
    active_in_play(state, end)
        || *state.get() == AppState::Menu && matches!(owner, UiOwner::Startup(_))
}

pub(super) fn spawn_panel(commands: &mut Commands, panel: UiPanel, font: &UiFont) -> Entity {
    let modal = !panel.buttons.is_empty();
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                padding: UiRect::all(Val::Px(16.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            GlobalZIndex(260),
            PstPanelRoot(panel.clone()),
        ))
        .id();
    if modal {
        commands.entity(root).insert((
            FocusPolicy::Block,
            UiInputBlocker,
            BackgroundColor(theme::SCRIM),
        ));
    }
    let surface = commands
        .spawn(UiDialogSurfaceBundle::new(460.0, theme::CARD, theme::LINE))
        .id();
    if modal {
        commands.entity(surface).insert((
            LauncherScrollArea,
            ScrollPosition::default(),
            Interaction::default(),
        ));
    }
    commands.entity(root).add_child(surface);
    let font = Some(font.0.clone());
    let heading = text(commands, &panel.title, 25.0, theme::TEXT, font.clone());
    commands.entity(surface).add_child(heading);
    for line in &panel.text {
        let label = text(commands, line, 16.0, theme::TEXT, font.clone());
        commands.entity(surface).add_child(label);
    }
    if modal {
        let actions = commands
            .spawn(Node {
                flex_wrap: FlexWrap::Wrap,
                column_gap: Val::Px(8.0),
                row_gap: Val::Px(6.0),
                justify_content: JustifyContent::FlexEnd,
                padding: UiRect::top(Val::Px(6.0)),
                ..default()
            })
            .id();
        commands.entity(surface).add_child(actions);
        for (index, item) in panel.buttons.iter().enumerate() {
            let button = net_room_action(commands, &item.label, 132.0, index == 0, font.clone());
            commands
                .entity(button)
                .entry::<Node>()
                .and_modify(|mut node| {
                    node.height = Val::Auto;
                    node.padding.top = Val::Px(6.0);
                    node.padding.bottom = Val::Px(6.0);
                });
            commands.entity(button).insert(PstPanelButton {
                generation: panel.generation,
                id: item.id.clone(),
            });
            commands.entity(actions).add_child(button);
        }
    }
    surface
}

pub(super) fn dispatch_button(
    state: Res<State<AppState>>,
    end: Option<Res<MatchEnd>>,
    ui: Option<Res<StagePstUi>>,
    mut scene: Option<ResMut<StageScriptRuntime>>,
    mut mission: Option<ResMut<StageMissionRuntime>>,
    mut startup: Option<ResMut<super::stage_plugin::pst_extensions::GlobalPst>>,
    buttons: Query<(&Interaction, &PstPanelButton), Changed<Interaction>>,
) {
    if buttons.is_empty() {
        return;
    }
    let Some(panel) = ui
        .as_deref()
        .and_then(|ui| ui.0.as_ref())
        .and_then(PstUi::visible)
    else {
        return;
    };
    if !presents_panel(&state, end.as_deref(), panel.owner) {
        return;
    }

    let Some(button) = buttons.iter().find_map(|(interaction, button)| {
        (*interaction == Interaction::Pressed && button.generation == panel.generation)
            .then_some(button)
    }) else {
        return;
    };
    let args = [PstValue::Str(panel.id), PstValue::Str(button.id.clone())];
    let playing = active_in_play(&state, end.as_deref());
    let scene = scene
        .as_deref_mut()
        .filter(|_| playing)
        .and_then(StageScriptRuntime::script_mut);
    let mission = mission
        .as_deref_mut()
        .filter(|_| playing)
        .and_then(StageMissionRuntime::script_mut);
    let result = match panel.owner {
        UiOwner::Scene => scene.map(|scene| scene.call_linked("OnUiButton", &args, mission)),
        UiOwner::Mission => mission.map(|mission| mission.call_linked("OnUiButton", &args, scene)),
        UiOwner::Startup(index) => startup.as_deref_mut().map(|startup| {
            startup
                .0
                .dispatch(
                    Some(index),
                    &[ScriptEvent {
                        name: "OnUiButton".into(),
                        args: args.to_vec(),
                    }],
                    scene,
                    mission,
                )
                .map(|_| PstValue::Num(0.0))
        }),
    };
    if let Some(Err(error)) = result {
        warn!("PST button handler failed: {error}");
    }
}
