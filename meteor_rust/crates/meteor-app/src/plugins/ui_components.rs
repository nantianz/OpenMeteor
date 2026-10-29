use bevy::prelude::*;
use bevy::ui::FocusPolicy;

use crate::plugins::UiTouchActionSet;

#[path = "ui_slider.rs"]
pub(crate) mod slider;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UiPageStack(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UiPageId(pub u16);

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiPageRoot {
    stack: UiPageStack,
    page: UiPageId,
    visible_display: Display,
}

impl UiPageRoot {
    pub const fn new(stack: UiPageStack, page: UiPageId, visible_display: Display) -> Self {
        Self {
            stack,
            page,
            visible_display,
        }
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiPageAction {
    stack: UiPageStack,
    target: UiPageId,
}

impl UiPageAction {
    pub const fn show(stack: UiPageStack, target: UiPageId) -> Self {
        Self { stack, target }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UiModalId(pub u16);

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiModalRoot {
    id: UiModalId,
    visible_display: Display,
}

impl UiModalRoot {
    pub const fn new(id: UiModalId, visible_display: Display) -> Self {
        Self {
            id,
            visible_display,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiModalCommand {
    Show,
    Hide,
    Toggle,
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiModalAction {
    id: UiModalId,
    command: UiModalCommand,
}

impl UiModalAction {
    pub const fn show(id: UiModalId) -> Self {
        Self {
            id,
            command: UiModalCommand::Show,
        }
    }

    pub const fn hide(id: UiModalId) -> Self {
        Self {
            id,
            command: UiModalCommand::Hide,
        }
    }

    pub const fn toggle(id: UiModalId) -> Self {
        Self {
            id,
            command: UiModalCommand::Toggle,
        }
    }
}

#[derive(Component)]
pub struct UiPageRegion;

#[derive(Bundle)]
pub struct UiPageRegionBundle {
    node: Node,
    root: UiPageRoot,
    region: UiPageRegion,
}

impl UiPageRegionBundle {
    pub fn new(node: Node, root: UiPageRoot) -> Self {
        Self {
            node,
            root,
            region: UiPageRegion,
        }
    }
}

#[derive(Component)]
pub struct UiSecondaryPage;

#[derive(Bundle)]
pub struct UiSecondaryPageBundle {
    node: Node,
    background: BackgroundColor,
    border: BorderColor,
    root: UiPageRoot,
    secondary_page: UiSecondaryPage,
}

impl UiSecondaryPageBundle {
    pub fn new(root: UiPageRoot, display: Display, background: Color, border: Color) -> Self {
        Self {
            node: Node {
                width: Val::Percent(100.0),
                display,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(10.0),
                padding: UiRect::all(Val::Px(12.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                ..default()
            },
            background: BackgroundColor(background),
            border: BorderColor::all(border),
            root,
            secondary_page: UiSecondaryPage,
        }
    }

    pub fn full_page(root: UiPageRoot, display: Display) -> Self {
        Self {
            node: Node {
                width: Val::Percent(100.0),
                display,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                ..default()
            },
            background: BackgroundColor(Color::NONE),
            border: BorderColor::all(Color::NONE),
            root,
            secondary_page: UiSecondaryPage,
        }
    }
}

#[derive(Component)]
pub struct UiSecondaryHeader;

#[derive(Bundle)]
pub struct UiSecondaryHeaderBundle {
    node: Node,
    header: UiSecondaryHeader,
}

impl Default for UiSecondaryHeaderBundle {
    fn default() -> Self {
        Self {
            node: Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(10.0),
                ..default()
            },
            header: UiSecondaryHeader,
        }
    }
}

#[derive(Component)]
pub struct UiActionBar;

#[derive(Bundle)]
pub struct UiActionBarBundle {
    node: Node,
    action_bar: UiActionBar,
}

impl UiActionBarBundle {
    pub fn wrapped() -> Self {
        Self {
            node: Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                column_gap: Val::Px(8.0),
                row_gap: Val::Px(8.0),
                ..default()
            },
            action_bar: UiActionBar,
        }
    }

    pub fn single_row() -> Self {
        Self {
            node: Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                ..default()
            },
            action_bar: UiActionBar,
        }
    }
}

#[derive(Component)]
pub struct UiPanel;

#[derive(Bundle)]
pub struct UiPanelBundle {
    node: Node,
    background: BackgroundColor,
    border: BorderColor,
    panel: UiPanel,
}

impl UiPanelBundle {
    pub fn new(mut node: Node, background: Color, border: Color) -> Self {
        node.border_radius = BorderRadius::all(Val::Px(3.0));
        Self {
            node,
            background: BackgroundColor(background),
            border: BorderColor::all(border),
            panel: UiPanel,
        }
    }
}

#[derive(Component)]
pub struct UiNoticePanel;

#[derive(Bundle)]
pub struct UiNoticePanelBundle {
    node: Node,
    background: BackgroundColor,
    border: BorderColor,
    notice: UiNoticePanel,
}

impl UiNoticePanelBundle {
    pub fn new(background: Color, border: Color) -> Self {
        Self {
            node: Node {
                width: Val::Percent(100.0),
                min_height: Val::Px(50.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(2.0),
                padding: UiRect::horizontal(Val::Px(14.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                ..default()
            },
            background: BackgroundColor(background),
            border: BorderColor::all(border),
            notice: UiNoticePanel,
        }
    }
}

#[derive(Component)]
pub struct UiModalOverlay;

#[derive(Component)]
pub struct UiInputBlocker;

#[derive(Bundle)]
pub struct UiModalOverlayBundle {
    node: Node,
    background: BackgroundColor,
    focus_policy: FocusPolicy,
    z_index: ZIndex,
    root: UiModalRoot,
    overlay: UiModalOverlay,
    input_blocker: UiInputBlocker,
}

impl UiModalOverlayBundle {
    pub fn new(root: UiModalRoot, display: Display, scrim: Color, z_index: i32) -> Self {
        Self {
            node: Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                display,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::all(Val::Px(12.0)),
                ..default()
            },
            background: BackgroundColor(scrim),
            focus_policy: FocusPolicy::Block,
            z_index: ZIndex(z_index),
            root,
            overlay: UiModalOverlay,
            input_blocker: UiInputBlocker,
        }
    }
}

#[derive(Component)]
pub struct UiDialogSurface;

#[derive(Bundle)]
pub struct UiDialogSurfaceBundle {
    node: Node,
    background: BackgroundColor,
    border: BorderColor,
    surface: UiDialogSurface,
}

impl UiDialogSurfaceBundle {
    pub fn new(max_width: f32, background: Color, border: Color) -> Self {
        Self {
            node: Node {
                width: Val::Percent(88.0),
                max_width: Val::Px(max_width),
                max_height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(12.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            background: BackgroundColor(background),
            border: BorderColor::all(border),
            surface: UiDialogSurface,
        }
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiSelectionState {
    pub selected: bool,
}

impl UiSelectionState {
    pub const fn new(selected: bool) -> Self {
        Self { selected }
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiSelectableKind {
    Checkbox,
    Radio,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct UiSelectableStyle {
    normal_background: Color,
    selected_background: Color,
    normal_border: Color,
    selected_border: Color,
}

impl UiSelectableStyle {
    pub const fn new(
        normal_background: Color,
        selected_background: Color,
        normal_border: Color,
        selected_border: Color,
    ) -> Self {
        Self {
            normal_background,
            selected_background,
            normal_border,
            selected_border,
        }
    }
}

impl Default for UiSelectableStyle {
    fn default() -> Self {
        use super::ui_theme as theme;
        Self::new(
            theme::CARD,
            theme::SELECTED,
            theme::LINE,
            theme::ACCENT_TEXT,
        )
    }
}

#[derive(Component)]
pub struct UiSelectionIndicator;

#[derive(Component)]
struct UiSelectionMark;

pub fn spawn_selection_indicator(
    parent: &mut ChildSpawnerCommands,
    kind: UiSelectableKind,
    selected: bool,
) {
    use super::ui_theme as theme;
    let radius = if kind == UiSelectableKind::Radio {
        8.0
    } else {
        3.0
    };
    parent
        .spawn((
            Node {
                width: Val::Px(16.0),
                height: Val::Px(16.0),
                flex_shrink: 0.0,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(radius)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BorderColor::all(if selected {
                theme::ACCENT_TEXT
            } else {
                theme::MUTED
            }),
            UiSelectionIndicator,
        ))
        .with_children(|outline| {
            let mark = if kind == UiSelectableKind::Radio {
                Node {
                    width: Val::Px(6.0),
                    height: Val::Px(6.0),
                    border_radius: BorderRadius::all(Val::Px(3.0)),
                    ..default()
                }
            } else {
                Node {
                    width: Val::Px(8.0),
                    height: Val::Px(5.0),
                    margin: UiRect::bottom(Val::Px(2.0)),
                    border: UiRect::new(Val::Px(2.0), Val::Px(0.0), Val::Px(0.0), Val::Px(2.0)),
                    ..default()
                }
            };
            outline.spawn((
                mark,
                UiTransform::from_rotation(Rot2::radians(if kind == UiSelectableKind::Radio {
                    0.0
                } else {
                    -std::f32::consts::FRAC_PI_4
                })),
                BackgroundColor(if kind == UiSelectableKind::Radio {
                    theme::ACCENT_TEXT
                } else {
                    Color::NONE
                }),
                BorderColor::all(theme::ACCENT_TEXT),
                if selected {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                },
                UiSelectionMark,
            ));
        });
}

#[derive(Bundle)]
pub struct UiSelectableButtonBundle {
    button: Button,
    node: Node,
    background: BackgroundColor,
    border: BorderColor,
    interaction: Interaction,
    selection: UiSelectionState,
    style: UiSelectableStyle,
    kind: UiSelectableKind,
}

impl UiSelectableButtonBundle {
    pub fn new(
        mut node: Node,
        selected: bool,
        kind: UiSelectableKind,
        style: UiSelectableStyle,
    ) -> Self {
        node.border_radius = BorderRadius::all(Val::Px(3.0));
        node.border = UiRect::all(Val::Px(1.0));
        node.column_gap = Val::Px(8.0);
        Self {
            button: Button,
            node,
            background: BackgroundColor(if selected {
                style.selected_background
            } else {
                style.normal_background
            }),
            border: BorderColor::all(if selected {
                style.selected_border
            } else {
                style.normal_border
            }),
            interaction: Interaction::default(),
            selection: UiSelectionState::new(selected),
            style,
            kind,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UiSelectId(pub u16);

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiSelectToggle(pub UiSelectId);

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiSelectOption(pub UiSelectId);

#[derive(Component)]
pub struct UiSelectDisabled;

#[derive(Component)]
pub struct UiSelectDismiss;

#[derive(Component)]
pub(crate) struct UiSelectScrollArea;

#[derive(Resource, Default)]
pub(crate) struct UiSelectState {
    pub open: bool,
    pub blocked_this_frame: bool,
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiSelectMenu {
    id: UiSelectId,
    visible_display: Display,
}

impl UiSelectMenu {
    pub const fn new(id: UiSelectId, visible_display: Display) -> Self {
        Self {
            id,
            visible_display,
        }
    }
}

#[derive(Bundle)]
pub struct UiSelectMenuBundle {
    node: Node,
    background: BackgroundColor,
    border: BorderColor,
    menu: UiSelectMenu,
    blocker: UiInputBlocker,
    focus: FocusPolicy,
    z_index: ZIndex,
}

impl UiSelectMenuBundle {
    pub fn new(id: UiSelectId, background: Color, border: Color) -> Self {
        Self {
            node: Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                display: Display::None,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::all(Val::Px(16.0)),
                ..default()
            },
            background: BackgroundColor(background),
            border: BorderColor::all(border),
            menu: UiSelectMenu::new(id, Display::Flex),
            blocker: UiInputBlocker,
            focus: FocusPolicy::Block,
            z_index: ZIndex(400),
        }
    }
}

pub(crate) fn spawn_select_dialog(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    id: UiSelectId,
    title: &str,
    options: impl FnOnce(&mut ChildSpawnerCommands),
) {
    use crate::plugins::launcher_ui::{
        launcher_text, LauncherScrollArea, LAUNCHER_RED_BRIGHT, PANEL,
    };
    parent
        .spawn(UiSelectMenuBundle::new(
            id,
            Color::srgba(0.01, 0.02, 0.03, 0.88),
            Color::NONE,
        ))
        .with_children(|overlay| {
            overlay
                .spawn(UiDialogSurfaceBundle::new(
                    440.0,
                    PANEL,
                    LAUNCHER_RED_BRIGHT,
                ))
                .with_children(|surface| {
                    surface.spawn(launcher_text(title, 18.0, Color::WHITE, font));
                    surface
                        .spawn((
                            Node {
                                width: Val::Percent(100.0),
                                min_height: Val::Px(0.0),
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(6.0),
                                overflow: Overflow::scroll_y(),
                                ..default()
                            },
                            ScrollPosition::default(),
                            UiSelectScrollArea,
                            LauncherScrollArea,
                            Interaction::default(),
                        ))
                        .with_children(options);
                    surface
                        .spawn((
                            Button,
                            Node {
                                width: Val::Percent(100.0),
                                min_height: Val::Px(44.0),
                                flex_shrink: 0.0,
                                justify_content: JustifyContent::Center,
                                align_items: AlignItems::Center,
                                ..default()
                            },
                            crate::app_state::LauncherStyledButton,
                            UiSelectDismiss,
                            BackgroundColor(Color::srgba(0.2, 0.23, 0.24, 0.8)),
                        ))
                        .with_children(|button| {
                            button.spawn(launcher_text("取消", 14.0, Color::WHITE, font));
                        });
                });
        });
}

pub(crate) fn select_dialog_input(
    mut state: ResMut<UiSelectState>,
    mut keys: Option<ResMut<ButtonInput<KeyCode>>>,
    mut menus: Query<&mut Node, With<UiSelectMenu>>,
    mut interactions: Query<&mut Interaction, (Without<UiSelectOption>, Without<UiSelectDismiss>)>,
) {
    state.blocked_this_frame = menus.iter().any(|node| node.display != Display::None);
    if !state.blocked_this_frame {
        return;
    }
    for mut interaction in &mut interactions {
        *interaction = Interaction::None;
    }
    if let Some(keys) = keys.as_mut() {
        if keys.just_pressed(KeyCode::Escape) {
            for mut menu in &mut menus {
                menu.display = Display::None;
            }
            keys.clear_just_pressed(KeyCode::Escape);
            state.open = false;
        }
    }
}

#[derive(Component)]
pub struct UiFormField;

#[derive(Bundle)]
pub struct UiFormFieldBundle {
    node: Node,
    field: UiFormField,
}

impl UiFormFieldBundle {
    pub fn responsive(min_width: f32) -> Self {
        Self {
            node: Node {
                min_width: Val::Px(min_width),
                flex_grow: 1.0,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            },
            field: UiFormField,
        }
    }
}

pub struct UiComponentsPlugin;

impl Plugin for UiComponentsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiSelectState>()
            .init_resource::<slider::UiSliderDrag>()
            .init_resource::<Messages<bevy::input::touch::TouchInput>>()
            .add_message::<slider::UiSliderChanged>()
            .add_systems(
                Update,
                slider::slider_input.before(crate::plugins::UiTouchGestureSet),
            )
            .add_systems(Update, slider::slider_visuals.after(UiTouchActionSet))
            .add_systems(
                Update,
                select_dialog_input
                    .after(crate::plugins::UiTouchGestureSet)
                    .after(crate::plugins::launcher_ui::launcher_ui_scroll)
                    .before(UiTouchActionSet),
            )
            .add_systems(
                Update,
                (
                    apply_page_actions,
                    apply_modal_actions,
                    apply_select_actions,
                    sync_selectable_visuals,
                )
                    .in_set(UiTouchActionSet),
            );
    }
}

type UiPageActionQuery<'w, 's> = Query<
    'w,
    's,
    (&'static Interaction, &'static UiPageAction),
    (Changed<Interaction>, With<Button>),
>;

type UiModalActionQuery<'w, 's> = Query<
    'w,
    's,
    (&'static Interaction, &'static UiModalAction),
    (Changed<Interaction>, With<Button>),
>;

type UiSelectToggleQuery<'w, 's> = Query<
    'w,
    's,
    (&'static Interaction, &'static UiSelectToggle),
    (Changed<Interaction>, With<Button>),
>;

type UiSelectOptionQuery<'w, 's> = Query<
    'w,
    's,
    (&'static Interaction, &'static UiSelectOption),
    (
        Changed<Interaction>,
        With<Button>,
        Without<UiSelectDisabled>,
    ),
>;

fn apply_page_actions(actions: UiPageActionQuery, mut roots: Query<(&UiPageRoot, &mut Node)>) {
    for (interaction, action) in &actions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        for (root, mut node) in &mut roots {
            if root.stack != action.stack {
                continue;
            }
            node.display = if root.page == action.target {
                root.visible_display
            } else {
                Display::None
            };
        }
    }
}

fn apply_modal_actions(actions: UiModalActionQuery, mut roots: Query<(&UiModalRoot, &mut Node)>) {
    for (interaction, action) in &actions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        for (root, mut node) in &mut roots {
            if root.id != action.id {
                continue;
            }
            node.display = match action.command {
                UiModalCommand::Show => root.visible_display,
                UiModalCommand::Hide => Display::None,
                UiModalCommand::Toggle if node.display == Display::None => root.visible_display,
                UiModalCommand::Toggle => Display::None,
            };
        }
    }
}

fn apply_select_actions(
    toggles: UiSelectToggleQuery,
    options: UiSelectOptionQuery,
    mut menus: Query<(&UiSelectMenu, &mut Node)>,
    dismiss: Query<&Interaction, (With<UiSelectDismiss>, Changed<Interaction>)>,
    mut state: ResMut<UiSelectState>,
) {
    for (interaction, toggle) in &toggles {
        if *interaction != Interaction::Pressed {
            continue;
        }
        for (menu, mut node) in &mut menus {
            node.display = if menu.id == toggle.0 && node.display == Display::None {
                menu.visible_display
            } else {
                Display::None
            };
        }
    }
    for (interaction, option) in &options {
        if *interaction != Interaction::Pressed {
            continue;
        }
        for (menu, mut node) in &mut menus {
            if menu.id == option.0 {
                node.display = Display::None;
            }
        }
    }
    if dismiss
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        for (_, mut node) in &mut menus {
            node.display = Display::None;
        }
    }
    state.open = menus.iter().any(|(_, node)| node.display != Display::None);
}

fn sync_selectable_visuals(
    mut buttons: Query<(
        &UiSelectionState,
        Option<&Interaction>,
        &UiSelectableStyle,
        &Children,
        &mut BackgroundColor,
        &mut BorderColor,
    )>,
    mut indicators: Query<
        (&mut BorderColor, &Children),
        (With<UiSelectionIndicator>, Without<UiSelectionState>),
    >,
    mut marks: Query<&mut Visibility, With<UiSelectionMark>>,
) {
    for (selection, interaction, style, children, mut background, mut border) in &mut buttons {
        background.0 = if selection.selected {
            style.selected_background
        } else {
            match interaction {
                Some(Interaction::Pressed) => style
                    .normal_background
                    .mix(&style.selected_background, 0.55),
                Some(Interaction::Hovered) => style
                    .normal_background
                    .mix(&style.selected_background, 0.25),
                _ => style.normal_background,
            }
        };
        *border = BorderColor::all(
            if selection.selected || interaction.is_some_and(|value| *value != Interaction::None) {
                style.selected_border
            } else {
                style.normal_border
            },
        );
        for child in children.iter() {
            if let Ok((mut outline, children)) = indicators.get_mut(child) {
                *outline = BorderColor::all(if selection.selected {
                    style.selected_border
                } else {
                    super::ui_theme::MUTED
                });
                for child in children.iter() {
                    if let Ok(mut visibility) = marks.get_mut(child) {
                        *visibility = if selection.selected {
                            Visibility::Inherited
                        } else {
                            Visibility::Hidden
                        };
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STACK: UiPageStack = UiPageStack(7);
    const PRIMARY: UiPageId = UiPageId(0);
    const SECONDARY: UiPageId = UiPageId(1);
    const MODAL: UiModalId = UiModalId(9);
    const SELECT_A: UiSelectId = UiSelectId(11);
    const SELECT_B: UiSelectId = UiSelectId(12);

    fn press(app: &mut App, entity: Entity) {
        *app.world_mut().get_mut::<Interaction>(entity).unwrap() = Interaction::Pressed;
        app.update();
    }

    #[test]
    fn page_action_switches_every_region_in_the_same_stack() {
        let mut app = App::new();
        app.add_plugins(UiComponentsPlugin);
        let heading = app
            .world_mut()
            .spawn((
                Node::default(),
                UiPageRoot::new(STACK, PRIMARY, Display::Flex),
            ))
            .id();
        let body = app
            .world_mut()
            .spawn((
                Node::default(),
                UiPageRoot::new(STACK, PRIMARY, Display::Flex),
            ))
            .id();
        let secondary = app
            .world_mut()
            .spawn((
                Node {
                    display: Display::None,
                    ..default()
                },
                UiPageRoot::new(STACK, SECONDARY, Display::Flex),
            ))
            .id();
        let open = app
            .world_mut()
            .spawn((
                Button,
                Interaction::None,
                UiPageAction::show(STACK, SECONDARY),
            ))
            .id();
        app.update();

        press(&mut app, open);

        assert_eq!(
            app.world().get::<Node>(heading).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().get::<Node>(body).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().get::<Node>(secondary).unwrap().display,
            Display::Flex
        );
    }

    #[test]
    fn modal_actions_show_hide_and_toggle_the_overlay() {
        let mut app = App::new();
        app.add_plugins(UiComponentsPlugin);
        let overlay = app
            .world_mut()
            .spawn((
                Node {
                    display: Display::None,
                    ..default()
                },
                UiModalRoot::new(MODAL, Display::Flex),
            ))
            .id();
        let show = app
            .world_mut()
            .spawn((Button, Interaction::None, UiModalAction::show(MODAL)))
            .id();
        let hide = app
            .world_mut()
            .spawn((Button, Interaction::None, UiModalAction::hide(MODAL)))
            .id();
        let toggle = app
            .world_mut()
            .spawn((Button, Interaction::None, UiModalAction::toggle(MODAL)))
            .id();
        app.update();

        press(&mut app, show);
        assert_eq!(
            app.world().get::<Node>(overlay).unwrap().display,
            Display::Flex
        );
        press(&mut app, hide);
        assert_eq!(
            app.world().get::<Node>(overlay).unwrap().display,
            Display::None
        );
        press(&mut app, toggle);
        assert_eq!(
            app.world().get::<Node>(overlay).unwrap().display,
            Display::Flex
        );
    }

    #[test]
    fn modal_and_secondary_bundles_keep_the_shared_layout_contract() {
        let modal = UiModalOverlayBundle::new(
            UiModalRoot::new(MODAL, Display::Flex),
            Display::None,
            Color::BLACK,
            300,
        );
        assert_eq!(modal.node.position_type, PositionType::Absolute);
        assert_eq!(modal.node.left, Val::Px(0.0));
        assert_eq!(modal.node.right, Val::Px(0.0));
        assert_eq!(modal.focus_policy, FocusPolicy::Block);
        assert_eq!(modal.z_index, ZIndex(300));

        let secondary = UiSecondaryPageBundle::new(
            UiPageRoot::new(STACK, SECONDARY, Display::Flex),
            Display::None,
            Color::BLACK,
            Color::WHITE,
        );
        assert_eq!(secondary.node.width, Val::Percent(100.0));
        assert_eq!(secondary.node.flex_direction, FlexDirection::Column);
        assert_eq!(secondary.node.display, Display::None);

        let actions = UiActionBarBundle::wrapped();
        assert_eq!(actions.node.flex_direction, FlexDirection::Row);
        assert_eq!(actions.node.flex_wrap, FlexWrap::Wrap);
    }

    #[test]
    fn settings_disabled_option_keeps_dialog_open() {
        let mut app = App::new();
        app.add_plugins(UiComponentsPlugin);
        let menu = app
            .world_mut()
            .spawn((Node::default(), UiSelectMenu::new(SELECT_A, Display::Flex)))
            .id();
        let option = app
            .world_mut()
            .spawn((
                Button,
                Interaction::None,
                UiSelectOption(SELECT_A),
                UiSelectDisabled,
            ))
            .id();
        app.update();
        press(&mut app, option);
        assert_eq!(
            app.world().get::<Node>(menu).unwrap().display,
            Display::Flex
        );
        *app.world_mut().get_mut::<Interaction>(option).unwrap() = Interaction::None;
        app.world_mut()
            .entity_mut(option)
            .remove::<UiSelectDisabled>();
        app.update();
        press(&mut app, option);
        assert_eq!(
            app.world().get::<Node>(menu).unwrap().display,
            Display::None
        );
    }

    #[test]
    fn select_dialog_blocks_background_until_dismissed_and_option_closes_it() {
        let mut app = App::new();
        app.add_plugins(UiComponentsPlugin);
        let menu_a = app
            .world_mut()
            .spawn((
                Node {
                    display: Display::None,
                    ..default()
                },
                UiSelectMenu::new(SELECT_A, Display::Flex),
            ))
            .id();
        let menu_b = app
            .world_mut()
            .spawn((
                Node {
                    display: Display::None,
                    ..default()
                },
                UiSelectMenu::new(SELECT_B, Display::Flex),
            ))
            .id();
        let open_a = app
            .world_mut()
            .spawn((Button, Interaction::None, UiSelectToggle(SELECT_A)))
            .id();
        let open_b = app
            .world_mut()
            .spawn((Button, Interaction::None, UiSelectToggle(SELECT_B)))
            .id();
        let choose_b = app
            .world_mut()
            .spawn((Button, Interaction::None, UiSelectOption(SELECT_B)))
            .id();
        app.update();

        press(&mut app, open_a);
        assert_eq!(
            app.world().get::<Node>(menu_a).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<Node>(menu_b).unwrap().display,
            Display::None
        );

        press(&mut app, open_b);
        assert_eq!(
            app.world().get::<Node>(menu_a).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<Node>(menu_b).unwrap().display,
            Display::None
        );
        let cancel = app
            .world_mut()
            .spawn((Button, Interaction::None, UiSelectDismiss))
            .id();
        press(&mut app, cancel);
        *app.world_mut().get_mut::<Interaction>(cancel).unwrap() = Interaction::None;
        press(&mut app, open_b);
        assert_eq!(
            app.world().get::<Node>(menu_a).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().get::<Node>(menu_b).unwrap().display,
            Display::Flex
        );

        press(&mut app, choose_b);
        assert_eq!(
            app.world().get::<Node>(menu_b).unwrap().display,
            Display::None
        );
    }

    #[test]
    fn selection_marks_follow_state_without_font_glyphs_or_hover_selection() {
        let mut app = App::new();
        app.add_plugins(UiComponentsPlugin);
        for kind in [UiSelectableKind::Radio, UiSelectableKind::Checkbox] {
            let button = app
                .world_mut()
                .spawn(UiSelectableButtonBundle::new(
                    Node::default(),
                    false,
                    kind,
                    UiSelectableStyle::default(),
                ))
                .id();
            app.world_mut()
                .commands()
                .entity(button)
                .with_children(|parent| {
                    spawn_selection_indicator(parent, kind, false);
                });
            app.world_mut().flush();
            let indicator = app.world().get::<Children>(button).unwrap()[0];
            let mark = app.world().get::<Children>(indicator).unwrap()[0];
            assert!(app.world().get::<Text>(indicator).is_none());
            for selected in [false, true, false] {
                app.world_mut()
                    .get_mut::<UiSelectionState>(button)
                    .unwrap()
                    .selected = selected;
                for interaction in [
                    Interaction::None,
                    Interaction::Hovered,
                    Interaction::Pressed,
                ] {
                    *app.world_mut().get_mut::<Interaction>(button).unwrap() = interaction;
                    app.update();
                    assert_eq!(
                        *app.world().get::<Visibility>(mark).unwrap(),
                        if selected {
                            Visibility::Inherited
                        } else {
                            Visibility::Hidden
                        }
                    );
                    assert_eq!(
                        *app.world().get::<BorderColor>(indicator).unwrap(),
                        BorderColor::all(if selected {
                            super::super::ui_theme::ACCENT_TEXT
                        } else {
                            super::super::ui_theme::MUTED
                        })
                    );
                    assert_eq!(
                        app.world().get::<Node>(button).unwrap().border,
                        UiRect::all(Val::Px(1.0))
                    );
                }
            }
        }
    }
}

pub(crate) fn spawn_loadout_arrow_icon(parent: &mut ChildSpawnerCommands, direction: i8) {
    let sign = f32::from(direction);
    for (x, y, width, angle) in [
        (0.0, 0.0, 18.0, 0.0),
        (sign * 6.0, -3.0, 8.5, sign * std::f32::consts::FRAC_PI_4),
        (sign * 6.0, 3.0, 8.5, -sign * std::f32::consts::FRAC_PI_4),
    ] {
        parent.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                width: Val::Px(width),
                height: Val::Px(2.0),
                margin: UiRect::new(
                    Val::Px(x - width * 0.5),
                    Val::ZERO,
                    Val::Px(y - 1.0),
                    Val::ZERO,
                ),
                ..default()
            },
            UiTransform::from_rotation(Rot2::radians(angle)),
            BackgroundColor(super::ui_theme::TEXT),
        ));
    }
}
