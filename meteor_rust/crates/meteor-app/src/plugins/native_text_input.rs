use super::{launcher_ui::UiFont, ui_theme as theme};
use crate::app_state::{AppState, LaunchRequest};
use bevy::input_focus::{FocusCause, InputFocus, InputFocusSystems};
use bevy::prelude::*;
use bevy::text::{EditableText, EditableTextFilter, TextCursorStyle, TextEdit};
use bevy::ui::FocusPolicy;
use bevy::ui_widgets::ImeSystems;

#[cfg(any(target_os = "android", target_arch = "wasm32", test))]
#[path = "bevy_text_ime.rs"]
mod ime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextField {
    PlayerName,
    RoomName,
    Address,
    StockPassword,
    Chat,
    Console,
}

impl TextField {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::PlayerName => "玩家名称",
            Self::RoomName => "房间名称",
            Self::Address => "房主 IP:端口",
            Self::StockPassword => "房间密码（无密码留空）",
            Self::Chat => "发送消息",
            Self::Console => "输入命令",
        }
    }
    fn limit(self) -> usize {
        match self {
            Self::PlayerName => 16,
            Self::RoomName => 24,
            Self::Address => 96,
            Self::StockPassword => 127,
            _ => 256,
        }
    }
}

#[derive(Component)]
pub(crate) struct NativeTextEdit {
    pub field: TextField,
    pub value: String,
}

#[derive(Component)]
pub(crate) struct NativeTextLabel(pub TextField);

#[derive(Message, Debug)]
pub(crate) struct TextSubmitted {
    pub field: TextField,
    pub value: String,
}

struct Pending {
    field: TextField,
    state: AppState,
    source: Entity,
    root: Entity,
    editor: Entity,
    submitting: bool,
}

#[derive(Resource, Default)]
struct NativeTextDialog {
    pending: Option<Pending>,
}

#[derive(Resource, Default)]
struct EditorKeys {
    confirm: bool,
    cancel: bool,
}

#[derive(Component, Clone, Copy)]
enum EditorAction {
    Confirm,
    Cancel,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NativeTextInputSet;

pub(crate) fn install(app: &mut App) {
    #[cfg(any(target_os = "android", target_arch = "wasm32"))]
    ime::install(app);
    app.init_resource::<NativeTextDialog>()
        .init_resource::<EditorKeys>()
        .init_resource::<InputFocus>()
        .add_message::<TextSubmitted>()
        .add_systems(Startup, restore_profile)
        .add_systems(
            PreUpdate,
            capture_editor_keys
                .after(bevy::input::InputSystems)
                .after(InputFocusSystems::Dispatch)
                .after(ImeSystems::HandleEvents),
        )
        .add_systems(
            Update,
            edit.after(crate::plugins::UiTouchGestureSet)
                .after(crate::plugins::launcher_ui::launcher_ui_scroll)
                .before(crate::plugins::UiTouchActionSet)
                .in_set(NativeTextInputSet),
        )
        .add_systems(
            Update,
            (apply_profile, refresh_fields)
                .chain()
                .after(edit)
                .before(crate::plugins::UiTouchActionSet)
                .in_set(NativeTextInputSet),
        );
}

fn clean(field: TextField, value: &str) -> String {
    let value = if field == TextField::StockPassword {
        value
    } else {
        value.trim()
    };
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(field.limit())
        .collect()
}

fn capture_editor_keys(
    dialog: Res<NativeTextDialog>,
    mut input: ResMut<EditorKeys>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
) {
    *input = EditorKeys::default();
    if dialog.pending.is_some() {
        input.confirm =
            keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter);
        input.cancel = keys.just_pressed(KeyCode::Escape);
        keys.reset_all();
    }
}

fn ime_busy(editor: &EditableText) -> bool {
    editor.is_composing()
        || editor.pending_edits.iter().any(|edit| {
            matches!(
                edit,
                TextEdit::ImeSetCompose { .. } | TextEdit::ImeCommit { .. }
            )
        })
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn edit(
    mut commands: Commands,
    mut dialog: ResMut<NativeTextDialog>,
    input: Res<EditorKeys>,
    state: Res<State<AppState>>,
    font: Option<Res<UiFont>>,
    fields: Query<(
        Entity,
        &Interaction,
        &NativeTextEdit,
        Option<&InheritedVisibility>,
    )>,
    mut interactions: Query<&mut Interaction, (Without<NativeTextEdit>, Without<EditorAction>)>,
    actions: Query<(&Interaction, &EditorAction), Changed<Interaction>>,
    editors: Query<&EditableText>,
    mut focus: ResMut<InputFocus>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut submitted: MessageWriter<TextSubmitted>,
) {
    let mut block = dialog.pending.is_some();
    if let Some(pending) = dialog.pending.as_mut() {
        let stale = pending.state != *state.get()
            || fields.get(pending.source).is_err()
            || fields
                .get(pending.source)
                .is_ok_and(|(_, _, _, v)| v.is_some_and(|v| !v.get()));
        let editor = editors.get(pending.editor).ok();
        let busy = editor.is_some_and(ime_busy);
        let cancel = input.cancel && !busy
            || actions
                .iter()
                .any(|(i, a)| *i == Interaction::Pressed && matches!(a, EditorAction::Cancel));
        let mut close = stale || cancel || editor.is_none();
        if !close {
            let editor = editor.unwrap();
            if pending.submitting
                && !busy
                && editor.pending_edits.is_empty()
                && editor.pending_paste.is_none()
            {
                submitted.write(TextSubmitted {
                    field: pending.field,
                    value: clean(pending.field, &editor.value().to_string()),
                });
                close = true;
            } else if !busy
                && (input.confirm
                    || actions.iter().any(|(i, a)| {
                        *i == Interaction::Pressed && matches!(a, EditorAction::Confirm)
                    }))
            {
                pending.submitting = true;
            }
        }
        if close {
            commands.entity(pending.root).despawn();
            focus.clear();
            dialog.pending = None;
        }
    }
    if !block {
        for (entity, interaction, field, visible) in &fields {
            if *interaction != Interaction::Pressed || visible.is_some_and(|v| !v.get()) {
                continue;
            }
            let font = font.as_ref().map(|font| font.0.clone()).unwrap_or_default();
            let (root, editor) = spawn_editor(&mut commands, field, font);
            focus.set(editor, FocusCause::Navigated);
            dialog.pending = Some(Pending {
                field: field.field,
                state: *state.get(),
                source: entity,
                root,
                editor,
                submitting: false,
            });
            block = true;
            break;
        }
    }
    if block {
        keys.reset_all();
        for mut interaction in &mut interactions {
            if *interaction != Interaction::None {
                *interaction = Interaction::None;
            }
        }
    }
}

fn spawn_editor(
    commands: &mut Commands,
    field: &NativeTextEdit,
    font: Handle<Font>,
) -> (Entity, Entity) {
    let text_font = TextFont {
        font: font.into(),
        font_size: FontSize::Px(16.0),
        ..default()
    };
    let mut editable = EditableText::new(&field.value);
    editable.max_characters = Some(field.field.limit());
    editable.visible_width = Some(1.0);
    editable.queue_edit(TextEdit::SelectAll);
    let editor = commands
        .spawn((
            editable,
            EditableTextFilter::new(|ch| !ch.is_control()),
            TextCursorStyle::default(),
            TextLayout::no_wrap(),
            text_font.clone(),
            TextColor(theme::TEXT),
            Node {
                width: Val::Percent(100.0),
                min_width: Val::Px(0.0),
                min_height: Val::Px(48.0),
                flex_shrink: 0.0,
                padding: UiRect::all(Val::Px(10.0)),
                border: UiRect::all(Val::Px(1.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(theme::BACKGROUND),
            BorderColor::all(theme::METAL),
        ))
        .id();
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                padding: UiRect::all(Val::Px(16.0)),
                ..default()
            },
            BackgroundColor(theme::SCRIM),
            GlobalZIndex(900),
            FocusPolicy::Block,
            Interaction::default(),
            super::ui_components::UiInputBlocker,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Percent(100.0),
                    max_width: Val::Px(520.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(18.0)),
                    row_gap: Val::Px(12.0),
                    ..default()
                },
                BackgroundColor(theme::SURFACE),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new(field.field.title()),
                    text_font.clone(),
                    TextColor(theme::TEXT),
                ));
                let panel_entity = panel.target_entity();
                panel
                    .commands()
                    .entity(editor)
                    .insert(ChildOf(panel_entity));
                panel.spawn((
                    Text::new("Enter 确认 · Esc 取消"),
                    text_font.clone(),
                    TextColor(theme::MUTED),
                ));
                panel
                    .spawn(Node {
                        justify_content: JustifyContent::End,
                        column_gap: Val::Px(12.0),
                        ..default()
                    })
                    .with_children(|row| {
                        for (label, action) in [
                            ("取消", EditorAction::Cancel),
                            ("确定", EditorAction::Confirm),
                        ] {
                            row.spawn((
                                Button,
                                action,
                                Node {
                                    min_width: Val::Px(84.0),
                                    min_height: Val::Px(44.0),
                                    align_items: AlignItems::Center,
                                    justify_content: JustifyContent::Center,
                                    ..default()
                                },
                                BackgroundColor(if matches!(action, EditorAction::Confirm) {
                                    theme::PRIMARY
                                } else {
                                    theme::CARD
                                }),
                            ))
                            .with_children(|button| {
                                button.spawn((
                                    Text::new(label),
                                    text_font.clone(),
                                    TextColor(theme::TEXT),
                                ));
                            });
                        }
                    });
            });
        })
        .id();
    (root, editor)
}

fn apply_profile(mut events: MessageReader<TextSubmitted>, mut request: ResMut<LaunchRequest>) {
    let mut changed = false;
    for event in events.read() {
        let target = match event.field {
            TextField::PlayerName => &mut request.player_name,
            TextField::RoomName => &mut request.room_name,
            TextField::Address => &mut request.stock_server_address,
            TextField::StockPassword => {
                request.stock_password.clone_from(&event.value);
                continue;
            }
            _ => continue,
        };

        if event.value.is_empty() && event.field != TextField::Address {
            continue;
        }
        target.clone_from(&event.value);
        changed = true;
    }
    if changed {
        let profile = serde_json::json!({"player_name":request.player_name, "room_name":request.room_name, "address":request.stock_server_address});
        save_profile(&profile.to_string());
    }
}

fn profile_value(request: &LaunchRequest, field: TextField) -> Option<&str> {
    match field {
        TextField::PlayerName => Some(&request.player_name),
        TextField::RoomName => Some(&request.room_name),
        TextField::Address => Some(&request.stock_server_address),
        TextField::StockPassword => Some(&request.stock_password),
        _ => None,
    }
}

fn refresh_fields(
    request: Res<LaunchRequest>,
    mut room: Option<ResMut<crate::app_state::RoomState>>,
    mut fields: Query<&mut NativeTextEdit>,
    mut labels: Query<(&NativeTextLabel, &mut Text)>,
) {
    if let Some(room) = room.as_mut() {
        if let Some(player) = room.members.first_mut().filter(|member| !member.is_npc) {
            if player.name != request.player_name {
                player.name.clone_from(&request.player_name);
            }
        }
    }
    for mut field in &mut fields {
        if let Some(value) = profile_value(&request, field.field) {
            if field.value != value {
                field.value = value.into();
            }
        }
    }
    for (label, mut text) in &mut labels {
        if let Some(value) = profile_value(&request, label.0) {
            let value = if value.is_empty() {
                "点击输入"
            } else {
                value
            };
            let masked = "*".repeat(value.chars().count());
            let value = if label.0 == TextField::StockPassword
                && !value.is_empty()
                && !request.stock_password.is_empty()
            {
                &masked
            } else {
                value
            };
            let display = format!("{}：{}", label.0.title(), value);
            if text.0 != display {
                text.0 = display;
            }
        }
    }
}

fn restore_profile(mut request: ResMut<LaunchRequest>) {
    let Some(profile) =
        read_profile().and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    else {
        return;
    };
    for (key, field) in [
        ("player_name", TextField::PlayerName),
        ("room_name", TextField::RoomName),
        ("address", TextField::Address),
    ] {
        if let Some(value) = profile[key].as_str() {
            let value = clean(field, value);
            if value.is_empty() {
                continue;
            }
            match field {
                TextField::PlayerName => request.player_name = value,
                TextField::RoomName => request.room_name = value,
                TextField::Address => request.stock_server_address = value,
                _ => {}
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn profile_path() -> std::path::PathBuf {
    #[cfg(target_os = "android")]
    if let Some(path) = bevy::android::ANDROID_APP
        .get()
        .and_then(|app| app.internal_data_path())
    {
        return path.join("network-profile.json");
    }
    std::path::PathBuf::from("network-profile.json")
}

fn read_profile() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::read_to_string(profile_path()).ok()
    }
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()?
            .local_storage()
            .ok()??
            .get_item("meteor.rust.network-profile.v1")
            .ok()?
    }
}

fn save_profile(value: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Err(error) = std::fs::write(profile_path(), value) {
        warn!("save network profile: {error}");
    }
    #[cfg(target_arch = "wasm32")]
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item("meteor.rust.network-profile.v1", value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_text_profile_updates_local_player_and_all_field_copies() {
        let mut app = App::new();
        app.insert_resource(LaunchRequest {
            player_name: "江湖故人".into(),
            ..default()
        })
        .init_resource::<crate::app_state::RoomState>()
        .add_systems(Update, refresh_fields);
        for _ in 0..2 {
            app.world_mut().spawn((
                NativeTextEdit {
                    field: TextField::PlayerName,
                    value: "玩家".into(),
                },
                NativeTextLabel(TextField::PlayerName),
                Text::new("玩家"),
            ));
        }
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::app_state::RoomState>()
                .members[0]
                .name,
            "江湖故人"
        );
        for (field, text) in app
            .world_mut()
            .query::<(&NativeTextEdit, &Text)>()
            .iter(app.world())
        {
            assert_eq!(field.value, "江湖故人");
            assert!(text.0.ends_with("江湖故人"));
        }
    }
    #[test]
    fn native_text_unicode_limits_preserve_names_and_address_syntax() {
        assert_eq!(clean(TextField::PlayerName, "  孟星魂🦋\n "), "孟星魂🦋");
        assert_eq!(
            clean(TextField::RoomName, &"蝶".repeat(30)).chars().count(),
            24
        );
        assert_eq!(clean(TextField::Address, " [::1]:5230 "), "[::1]:5230");
    }
    fn editor_app() -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(State::new(AppState::Menu))
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<InputFocus>()
            .init_resource::<NativeTextDialog>()
            .init_resource::<EditorKeys>()
            .add_message::<TextSubmitted>()
            .add_systems(PreUpdate, capture_editor_keys)
            .add_systems(Update, edit);
        let source = app
            .world_mut()
            .spawn((
                Interaction::Pressed,
                NativeTextEdit {
                    field: TextField::PlayerName,
                    value: "原名".into(),
                },
            ))
            .id();
        app.update();
        *app.world_mut().get_mut::<Interaction>(source).unwrap() = Interaction::None;
        (app, source)
    }

    #[test]
    fn native_text_bevy_editor_submits_once_after_pending_edits() {
        let (mut app, _) = editor_app();
        let editor = app
            .world()
            .resource::<NativeTextDialog>()
            .pending
            .as_ref()
            .unwrap()
            .editor;
        {
            let mut text = app.world_mut().get_mut::<EditableText>(editor).unwrap();
            text.editor.set_text("中文名字🦋");
            text.pending_edits.clear();
        }
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Enter);
        let background = app.world_mut().spawn(Interaction::Pressed).id();
        app.update();
        assert!(app.world().resource::<Messages<TextSubmitted>>().is_empty());
        assert!(!app
            .world()
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::Enter));
        assert_eq!(
            *app.world().get::<Interaction>(background).unwrap(),
            Interaction::None
        );
        app.update();
        let events: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<TextSubmitted>>()
            .drain()
            .collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].value, "中文名字🦋");
        assert!(app.world().resource::<NativeTextDialog>().pending.is_none());
        assert!(app.world().resource::<InputFocus>().get().is_none());
        assert!(app.world().get_entity(editor).is_err());
        app.update();
        assert!(app.world().resource::<Messages<TextSubmitted>>().is_empty());
    }

    #[test]
    fn native_text_cancel_and_stale_state_do_not_commit_or_leak_keys() {
        for stale in [false, true] {
            let (mut app, _) = editor_app();
            if stale {
                app.insert_resource(State::new(AppState::NetRoom));
            } else {
                app.world_mut()
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .press(KeyCode::Escape);
            }
            app.update();
            assert!(app.world().resource::<Messages<TextSubmitted>>().is_empty());
            assert!(app.world().resource::<NativeTextDialog>().pending.is_none());
            assert!(!app
                .world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::Escape));
        }
    }

    #[test]
    fn native_text_ime_enter_does_not_submit_candidate_confirmation() {
        let (mut app, _) = editor_app();
        let editor = app
            .world()
            .resource::<NativeTextDialog>()
            .pending
            .as_ref()
            .unwrap()
            .editor;
        app.world_mut()
            .get_mut::<EditableText>(editor)
            .unwrap()
            .queue_edit(TextEdit::ImeCommit {
                value: "孟星魂".into(),
            });
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Enter);
        app.update();
        assert!(
            !app.world()
                .resource::<NativeTextDialog>()
                .pending
                .as_ref()
                .unwrap()
                .submitting
        );
        assert!(app.world().resource::<Messages<TextSubmitted>>().is_empty());
    }

    #[test]
    fn native_text_real_bevy_keyboard_and_ime_pipeline() {
        use bevy::input::keyboard::{Key, KeyboardInput};
        use bevy::input::ButtonState;
        use bevy::input_focus::{InputDispatchPlugin, InputFocusPlugin};
        use bevy::picking::events::{Pointer, Release};
        use bevy::text::{apply_text_edits, EditableTextSystems, FontCx, LayoutCx};
        use bevy::ui_widgets::EditableTextInputPlugin;

        let mut app = App::new();
        app.add_plugins((
            bevy::input::InputPlugin,
            InputFocusPlugin,
            InputDispatchPlugin,
            EditableTextInputPlugin,
        ))
        .init_resource::<FontCx>()
        .init_resource::<LayoutCx>()
        .init_resource::<bevy::clipboard::Clipboard>()
        .init_resource::<UiScale>()
        .init_resource::<LaunchRequest>()
        .insert_resource(State::new(AppState::NetRoom))
        .add_message::<Ime>()
        .add_message::<Pointer<Release>>()
        .add_systems(PostUpdate, apply_text_edits.in_set(EditableTextSystems));
        install(&mut app);
        let window = app
            .world_mut()
            .spawn((Window::default(), bevy::window::PrimaryWindow))
            .id();
        let source = app
            .world_mut()
            .spawn((
                Interaction::Pressed,
                NativeTextEdit {
                    field: TextField::Chat,
                    value: String::new(),
                },
            ))
            .id();
        app.update();
        *app.world_mut().get_mut::<Interaction>(source).unwrap() = Interaction::None;
        let editor = app
            .world()
            .resource::<NativeTextDialog>()
            .pending
            .as_ref()
            .unwrap()
            .editor;
        let key = |code, logical, text| KeyboardInput {
            key_code: code,
            logical_key: logical,
            state: ButtonState::Pressed,
            text,
            repeat: false,
            window,
        };
        app.world_mut().write_message(key(
            KeyCode::KeyA,
            Key::Character("a".into()),
            Some("a".into()),
        ));
        app.update();
        assert_eq!(
            app.world()
                .get::<EditableText>(editor)
                .unwrap()
                .value()
                .to_string(),
            "a"
        );
        assert!(!app
            .world()
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::KeyA));
        app.world_mut().write_message(Ime::Preedit {
            window,
            value: "meng".into(),
            cursor: Some((4, 4)),
        });
        app.update();
        assert!(app
            .world()
            .get::<EditableText>(editor)
            .unwrap()
            .is_composing());
        app.world_mut().write_message(Ime::Commit {
            window,
            value: "孟🦋".into(),
        });
        app.world_mut()
            .write_message(key(KeyCode::Enter, Key::Enter, None));
        app.update();
        assert!(app.world().resource::<Messages<TextSubmitted>>().is_empty());
        assert_eq!(
            app.world()
                .get::<EditableText>(editor)
                .unwrap()
                .value()
                .to_string(),
            "a孟🦋"
        );
        app.world_mut()
            .write_message(key(KeyCode::Enter, Key::Enter, None));
        app.update();
        app.update();
        let events: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<TextSubmitted>>()
            .drain()
            .collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].value, "a孟🦋");
    }
}
