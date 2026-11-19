use bevy::prelude::*;
use bevy::ui::{FocusPolicy, ScrollPosition};
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
    roots: Query<(Entity, &PstPanelRoot)>,
) {
    let shown =
        ui.0.as_ref()
            .filter(|_| active_in_play(&state, end.as_deref()))
            .and_then(PstUi::visible);
    if let Some(panel) = shown.as_ref() {
        if roots.single().is_ok_and(|(_, root)| &root.0 == panel) {
            return;
        }
    }
    for (entity, _) in &roots {
        commands.entity(entity).despawn();
    }
    if let (Some(panel), Some(font)) = (shown, font) {
        spawn_panel(&mut commands, panel, &font);
    }
}

pub(super) fn spawn_panel(commands: &mut Commands, panel: UiPanel, font: &UiFont) {
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
}

pub(super) fn dispatch_button(
    state: Res<State<AppState>>,
    end: Option<Res<MatchEnd>>,
    ui: Option<Res<StagePstUi>>,
    mut scene: Option<ResMut<StageScriptRuntime>>,
    mut mission: Option<ResMut<StageMissionRuntime>>,
    buttons: Query<(&Interaction, &PstPanelButton), Changed<Interaction>>,
) {
    if !active_in_play(&state, end.as_deref()) {
        return;
    }
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

    let Some(button) = buttons.iter().find_map(|(interaction, button)| {
        (*interaction == Interaction::Pressed && button.generation == panel.generation)
            .then_some(button)
    }) else {
        return;
    };
    let args = [PstValue::Str(panel.id), PstValue::Str(button.id.clone())];
    let scene = scene
        .as_deref_mut()
        .and_then(StageScriptRuntime::script_mut);
    let mission = mission
        .as_deref_mut()
        .and_then(StageMissionRuntime::script_mut);
    let result = match panel.owner {
        UiOwner::Scene => scene.map(|scene| scene.call_linked("OnUiButton", &args, mission)),
        UiOwner::Mission => mission.map(|mission| mission.call_linked("OnUiButton", &args, scene)),
    };
    if let Some(Err(error)) = result {
        warn!("PST button handler failed: {error}");
    }
}
