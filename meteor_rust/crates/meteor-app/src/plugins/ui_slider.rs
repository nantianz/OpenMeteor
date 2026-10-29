use super::UiInputBlocker;
use crate::plugins::ui_scale_plugin::EffectiveUiScale;
use bevy::input::touch::{TouchInput, TouchPhase};
use bevy::prelude::*;
use bevy::ui::ComputedStackIndex;
use std::collections::HashSet;

const INSET: f32 = 12.0;

#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct UiSlider {
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub value: f32,
}
impl UiSlider {
    fn ratio(self) -> f32 {
        ((self.value - self.min) / (self.max - self.min).max(f32::EPSILON)).clamp(0.0, 1.0)
    }
    fn at(self, ratio: f32) -> f32 {
        (self.min
            + ((ratio.clamp(0.0, 1.0) * (self.max - self.min)) / self.step).round() * self.step)
            .clamp(self.min, self.max)
    }
}

#[derive(Component)]
pub(super) struct SliderFill(Entity);
#[derive(Component)]
pub(super) struct SliderThumb(Entity);

#[derive(Message, Clone, Copy)]
pub(crate) struct UiSliderChanged {
    pub entity: Entity,
    pub value: f32,
    pub finished: bool,
}

#[derive(Clone, Copy)]
struct Capture {
    entity: Entity,
    window: Entity,
    touch: Option<u64>,
    value: f32,
}
#[derive(Resource, Default)]
pub(crate) struct UiSliderDrag {
    capture: Option<Capture>,
    consumed: HashSet<u64>,
}
impl UiSliderDrag {
    pub(crate) fn is_active(&self) -> bool {
        self.capture.is_some()
    }
    pub(crate) fn consumes_touch(&self, id: u64) -> bool {
        self.consumed.contains(&id)
    }
}

pub(crate) fn spawn_slider(
    parent: &mut ChildSpawnerCommands,
    slider: UiSlider,
    binding: impl Bundle,
) {
    let mut root = parent.spawn((
        Node {
            flex_grow: 1.0,
            flex_basis: Val::Px(0.0),
            min_width: Val::Px(48.0),
            height: Val::Px(44.0),
            ..default()
        },
        slider,
        binding,
    ));
    let entity = root.id();
    root.with_children(|hit| {
        hit.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(INSET),
                right: Val::Px(INSET),
                top: Val::Percent(50.0),
                height: Val::Px(4.0),
                margin: UiRect::top(Val::Px(-2.0)),
                border_radius: BorderRadius::all(Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(crate::plugins::ui_theme::CARD),
        ))
        .with_children(|track| {
            track.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(slider.ratio() * 100.0),
                    height: Val::Percent(100.0),
                    border_radius: BorderRadius::all(Val::Px(2.0)),
                    ..default()
                },
                BackgroundColor(crate::plugins::ui_theme::ACCENT),
                SliderFill(entity),
            ));
            track.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(slider.ratio() * 100.0),
                    top: Val::Percent(50.0),
                    width: Val::Px(20.0),
                    height: Val::Px(20.0),
                    margin: UiRect::new(Val::Px(-10.0), Val::ZERO, Val::Px(-10.0), Val::ZERO),
                    border_radius: BorderRadius::all(Val::Percent(50.0)),
                    ..default()
                },
                BackgroundColor(crate::plugins::ui_theme::TEXT),
                SliderThumb(entity),
            ));
        });
    });
}

type Sliders<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Node,
        &'static ComputedNode,
        &'static UiGlobalTransform,
        Option<&'static InheritedVisibility>,
        Option<&'static CalculatedClip>,
        &'static ComputedStackIndex,
        &'static mut UiSlider,
    ),
>;

fn change(
    sliders: &mut Sliders,
    changes: &mut MessageWriter<UiSliderChanged>,
    capture: &mut Capture,
    position: Vec2,
    scale: f32,
    finished: bool,
) {
    if let Ok((_, _, computed, transform, _, _, _, mut slider)) = sliders.get_mut(capture.entity) {
        if let Some(point) = computed.normalize_point(*transform, position) {
            let inset = INSET * scale;
            let ratio = ((point.x + 0.5) * computed.size().x - inset)
                / (computed.size().x - inset * 2.0).max(1.0);
            let value = slider.at(ratio);
            if slider.value != value || finished {
                slider.value = value;
                capture.value = value;
                changes.write(UiSliderChanged {
                    entity: capture.entity,
                    value,
                    finished,
                });
            }
        }
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(super) fn slider_input(
    mut touches: MessageReader<TouchInput>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    windows: Query<(Entity, &Window)>,
    ui_scale: Option<Res<EffectiveUiScale>>,
    mut drag: ResMut<UiSliderDrag>,
    select: Res<super::UiSelectState>,
    editor: Option<Res<crate::plugins::mobile_controls_plugin::MobileLayoutEditorState>>,
    mut sliders: Sliders,
    blockers: Query<
        (
            &Node,
            &ComputedNode,
            &UiGlobalTransform,
            &ComputedStackIndex,
        ),
        With<UiInputBlocker>,
    >,
    mut changes: MessageWriter<UiSliderChanged>,
) {
    drag.consumed.clear();
    let covered = select.open || editor.is_some_and(|editor| editor.is_open());
    let unit = ui_scale.as_deref().map_or(1.0, |scale| scale.0);
    let pick = |sliders: &Sliders, position: Vec2| {
        let floor = blockers
            .iter()
            .filter(|(node, computed, transform, _)| {
                node.display != Display::None && computed.contains_point(**transform, position)
            })
            .map(|(_, _, _, stack)| stack.0)
            .max();
        sliders
            .iter()
            .filter(
                |(_, node, computed, transform, visibility, clip, stack, _)| {
                    node.display != Display::None
                        && visibility.is_none_or(|v| v.get())
                        && clip.is_none_or(|c| c.clip.contains(position))
                        && floor.is_none_or(|floor| stack.0 >= floor)
                        && computed.contains_point(**transform, position)
                },
            )
            .max_by_key(|(_, _, _, _, _, _, stack, _)| stack.0)
            .map(|(entity, _, _, _, _, _, _, slider)| (entity, slider.value))
    };
    if let Some(capture) = drag.capture {
        let available = !covered
            && sliders.get(capture.entity).is_ok_and(
                |(_, node, computed, _, visibility, _, _, _)| {
                    node.display != Display::None
                        && computed.size().min_element() > 0.0
                        && visibility.is_none_or(|v| v.get())
                },
            )
            && windows
                .get(capture.window)
                .is_ok_and(|(_, window)| window.focused);
        if !available {
            changes.write(UiSliderChanged {
                entity: capture.entity,
                value: capture.value,
                finished: true,
            });
            drag.capture = None;
        }
    }
    if covered {
        touches.read().for_each(drop);
        return;
    }
    let mut saw_touch = false;
    for event in touches.read() {
        saw_touch = true;
        let Ok((_, window)) = windows.get(event.window) else {
            continue;
        };
        let position = event.position * window.scale_factor();
        if event.phase == TouchPhase::Started && drag.capture.is_none() {
            if let Some((entity, value)) = pick(&sliders, position) {
                drag.capture = Some(Capture {
                    entity,
                    window: event.window,
                    touch: Some(event.id),
                    value,
                });
            }
        }
        let Some(mut capture) = drag.capture.filter(|c| c.touch == Some(event.id)) else {
            continue;
        };
        drag.consumed.insert(event.id);
        let finished = matches!(event.phase, TouchPhase::Ended | TouchPhase::Canceled);
        change(
            &mut sliders,
            &mut changes,
            &mut capture,
            position,
            window.scale_factor() * unit,
            finished,
        );
        drag.capture = (!finished).then_some(capture);
    }
    if let Some(mouse) = mouse {
        if !saw_touch && drag.capture.is_none() && mouse.just_pressed(MouseButton::Left) {
            if let Ok((entity, window)) = windows.single() {
                if let Some(position) = window.physical_cursor_position() {
                    if let Some((slider, value)) = pick(&sliders, position) {
                        drag.capture = Some(Capture {
                            entity: slider,
                            window: entity,
                            touch: None,
                            value,
                        });
                    }
                }
            }
        }
        if let Some(mut capture) = drag.capture.filter(|c| c.touch.is_none()) {
            if let Ok((_, window)) = windows.get(capture.window) {
                let finished = !mouse.pressed(MouseButton::Left);
                if let Some(position) = window.physical_cursor_position() {
                    change(
                        &mut sliders,
                        &mut changes,
                        &mut capture,
                        position,
                        window.scale_factor() * unit,
                        finished,
                    );
                } else if finished {
                    changes.write(UiSliderChanged {
                        entity: capture.entity,
                        value: capture.value,
                        finished: true,
                    });
                }
                drag.capture = (!finished).then_some(capture);
            }
        }
    }
}

pub(super) fn slider_visuals(
    sliders: Query<&UiSlider>,
    mut fills: Query<(&SliderFill, &mut Node), Without<SliderThumb>>,
    mut thumbs: Query<(&SliderThumb, &mut Node)>,
) {
    for (fill, mut node) in &mut fills {
        if let Ok(slider) = sliders.get(fill.0) {
            node.width = Val::Percent(slider.ratio() * 100.0);
        }
    }
    for (thumb, mut node) in &mut thumbs {
        if let Ok(slider) = sliders.get(thumb.0) {
            node.left = Val::Percent(slider.ratio() * 100.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::launcher_ui::{
        launcher_touch_scroll, LauncherScrollArea, LauncherTouchScrollState,
    };
    use crate::plugins::ui_components::{UiComponentsPlugin, UiSelectId, UiSelectMenu};
    use bevy::ecs::system::RunSystemOnce;

    fn app_with_slider(dpi: f32, unit: f32) -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(UiComponentsPlugin)
            .add_message::<TouchInput>()
            .insert_resource(EffectiveUiScale(unit))
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<LauncherTouchScrollState>()
            .add_systems(
                Update,
                launcher_touch_scroll.in_set(crate::plugins::UiTouchGestureSet),
            );
        let window = app
            .world_mut()
            .spawn(Window {
                resolution: bevy::window::WindowResolution::new(1000, 600)
                    .with_scale_factor_override(dpi),
                ..default()
            })
            .id();
        app.world_mut()
            .run_system_once(|mut commands: Commands| {
                commands.spawn(Node::default()).with_children(|parent| {
                    spawn_slider(
                        parent,
                        UiSlider {
                            min: 10.0,
                            max: 100.0,
                            step: 1.0,
                            value: 10.0,
                        },
                        (),
                    );
                });
            })
            .unwrap();
        let slider = app
            .world_mut()
            .query_filtered::<Entity, With<UiSlider>>()
            .single(app.world())
            .unwrap();
        app.world_mut().entity_mut(slider).insert((
            ComputedNode {
                size: Vec2::new(240.0, 44.0) * dpi * unit,
                ..default()
            },
            UiGlobalTransform::from_xy(400.0, 200.0),
            InheritedVisibility::VISIBLE,
            ComputedStackIndex(5),
        ));
        (app, window, slider)
    }

    fn touch(app: &mut App, window: Entity, phase: TouchPhase, physical: Vec2, dpi: f32) {
        app.world_mut().write_message(TouchInput {
            window,
            phase,
            position: physical / dpi,
            id: 17,
            force: None,
        });
        app.update();
    }

    #[test]
    fn slider_touch_updates_value_fill_and_thumb_without_scrolling_at_all_scales() {
        for (dpi, unit) in [(1.0, 1.0), (2.75, 1.0), (2.0, 0.7), (2.0, 1.2)] {
            let (mut app, window, slider) = app_with_slider(dpi, unit);
            let scroll = app
                .world_mut()
                .spawn((
                    Node::default(),
                    LauncherScrollArea,
                    ScrollPosition::default(),
                    ComputedNode {
                        size: Vec2::new(1000.0, 600.0),
                        ..default()
                    },
                    UiGlobalTransform::from_xy(500.0, 300.0),
                    InheritedVisibility::VISIBLE,
                    ComputedStackIndex(1),
                ))
                .id();
            touch(
                &mut app,
                window,
                TouchPhase::Started,
                Vec2::new(400.0, 200.0),
                dpi,
            );
            assert_eq!(app.world().get::<UiSlider>(slider).unwrap().value, 55.0);
            assert!(app.world().resource::<UiSliderDrag>().is_active());
            let track_end = 400.0 + (120.0 - INSET) * dpi * unit;
            touch(
                &mut app,
                window,
                TouchPhase::Moved,
                Vec2::new(track_end + 100.0, 260.0),
                dpi,
            );
            assert_eq!(app.world().get::<UiSlider>(slider).unwrap().value, 100.0);
            let thumb = app
                .world_mut()
                .query_filtered::<&Node, With<SliderThumb>>()
                .single(app.world())
                .unwrap();
            assert_eq!(thumb.left, Val::Percent(100.0));
            let fill = app
                .world_mut()
                .query_filtered::<&Node, With<SliderFill>>()
                .single(app.world())
                .unwrap();
            assert_eq!(fill.width, Val::Percent(100.0));
            assert_eq!(
                app.world().get::<ScrollPosition>(scroll).unwrap().0,
                Vec2::ZERO
            );
            touch(
                &mut app,
                window,
                TouchPhase::Ended,
                Vec2::new(track_end, 260.0),
                dpi,
            );
            assert!(!app.world().resource::<UiSliderDrag>().is_active());
            assert_eq!(
                app.world().get::<ScrollPosition>(scroll).unwrap().0,
                Vec2::ZERO
            );
        }
    }

    #[test]
    fn slider_mouse_drag_clamps_and_releases_outside_the_track() {
        let (mut app, window, slider) = app_with_slider(2.0, 1.0);
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .set_physical_cursor_position(Some(Vec2::new(400.0, 200.0).into()));
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        assert_eq!(app.world().get::<UiSlider>(slider).unwrap().value, 55.0);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .set_physical_cursor_position(Some(Vec2::new(0.0, 500.0).into()));
        app.update();
        assert_eq!(app.world().get::<UiSlider>(slider).unwrap().value, 10.0);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        app.update();
        assert!(!app.world().resource::<UiSliderDrag>().is_active());
    }

    #[test]
    fn slider_cannot_start_under_a_dialog_and_hidden_control_releases_capture() {
        let (mut app, window, slider) = app_with_slider(2.0, 1.0);
        let modal = app
            .world_mut()
            .spawn((
                Node::default(),
                UiInputBlocker,
                UiSelectMenu::new(UiSelectId(7), Display::Flex),
                ComputedNode {
                    size: Vec2::new(1000.0, 600.0),
                    ..default()
                },
                UiGlobalTransform::from_xy(500.0, 300.0),
                ComputedStackIndex(10),
            ))
            .id();
        touch(
            &mut app,
            window,
            TouchPhase::Started,
            Vec2::new(400.0, 200.0),
            2.0,
        );
        assert!(!app.world().resource::<UiSliderDrag>().is_active());
        app.world_mut().despawn(modal);
        app.update();
        touch(
            &mut app,
            window,
            TouchPhase::Started,
            Vec2::new(400.0, 200.0),
            2.0,
        );
        assert!(app.world().resource::<UiSliderDrag>().is_active());
        app.world_mut().get_mut::<Node>(slider).unwrap().display = Display::None;
        app.update();
        assert!(!app.world().resource::<UiSliderDrag>().is_active());
    }
}
