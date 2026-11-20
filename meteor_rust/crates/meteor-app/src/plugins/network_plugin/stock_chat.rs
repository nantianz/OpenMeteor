use super::{NetOutbound, NetState, NetStatus, NetTx};
use crate::app_state::{AppState, InputBinding, KeyField, Keymap};
use crate::plugins::{
    debug_plugin::DebugConsoleState,
    launcher_ui::UiFont,
    native_text_input::{self, NativeTextDialog, OpenTextEditor, TextField, TextSubmitted},
    system_notice, ui_theme as theme,
};
use bevy::input::{keyboard::KeyboardInput, ButtonState};
use bevy::prelude::*;
use meteor_network::protocol::Message;
use meteor_network::stock_protocol::{StockChatEvent, StockQuickChat};

const NUMBER_KEYS: [KeyCode; 9] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum QuickMenu {
    Team,
    Phrases(u32),
}

impl QuickMenu {
    fn select(self, index: u32) -> (Option<Self>, Option<StockQuickChat>) {
        match self {
            Self::Team if index < 3 => (Some(Self::Phrases(36 + index)), None),
            Self::Phrases(category) => {
                let chat = StockQuickChat { category, index };
                if chat.message_id().is_some() {
                    return (None, Some(chat));
                }
                (Some(self), None)
            }
            _ => (Some(self), None),
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct StockChatKeyboard {
    menu: Option<QuickMenu>,
    captures_gameplay: bool,
}

impl StockChatKeyboard {
    pub(crate) fn captures_gameplay(&self) -> bool {
        self.captures_gameplay
    }
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<StockChatKeyboard>()
        .add_systems(
            PreUpdate,
            route_keyboard.after(native_text_input::capture_editor_keys),
        )
        .add_systems(
            Update,
            (submit_text, render_quick_menu).after(native_text_input::NativeTextInputSet),
        );
}

fn in_stock_match(net: &NetState) -> bool {
    net.is_stock_udp() && net.status == NetStatus::InRoom
}

fn route_keyboard(
    state: Res<State<AppState>>,
    net: Res<NetState>,
    tx: Res<NetTx>,
    dialog: Res<NativeTextDialog>,
    debug: Option<Res<DebugConsoleState>>,
    keymap: Res<Keymap>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut chat: ResMut<StockChatKeyboard>,
    mut open: MessageWriter<OpenTextEditor>,
    mut keyboard: MessageReader<KeyboardInput>,
) {
    chat.captures_gameplay = false;
    if *state.get() != AppState::Playing
        || !in_stock_match(&net)
        || debug.is_some_and(|debug| debug.captures_input())
    {
        chat.menu = None;
        keyboard.clear();
        return;
    }

    let mut presses = std::collections::HashMap::new();
    for event in keyboard
        .read()
        .filter(|event| event.state == ButtonState::Pressed)
    {
        *presses.entry(event.key_code).or_insert(false) |= !event.repeat;
    }
    let pressed = |key| {
        presses
            .get(&key)
            .copied()
            .unwrap_or_else(|| keys.just_pressed(key))
    };
    let binding_pressed = |binding: InputBinding| {
        binding
            .as_key()
            .map_or_else(|| binding.just_pressed(&keys, &mouse), pressed)
    };
    let chat_all = binding_pressed(keymap.chat_all);
    let had_menu = chat.menu.is_some();
    let selection = chat
        .menu
        .zip(NUMBER_KEYS.iter().position(|key| pressed(*key)));
    if dialog.is_open() {
        chat.menu = None;
        chat.captures_gameplay = true;
    } else if had_menu && pressed(KeyCode::Escape) {
        chat.menu = None;
    } else if let Some((menu, index)) = selection {
        let (next, selected) = menu.select(index as u32);
        chat.menu = next;
        if let Some(selected) = selected {
            tx.send_stock_quick_chat(selected);
        }
    } else if chat_all || binding_pressed(keymap.chat_team) {
        open.write(OpenTextEditor(if chat_all {
            TextField::MatchChat
        } else {
            TextField::MatchTeamChat
        }));
        chat.menu = None;
        chat.captures_gameplay = true;
    } else if binding_pressed(keymap.quick_chat_all) {
        chat.menu = Some(QuickMenu::Phrases(39));
    } else if binding_pressed(keymap.quick_chat_team) {
        chat.menu = Some(QuickMenu::Team);
    }
    chat.captures_gameplay |= had_menu
        || chat.menu.is_some()
        || KeyField::STOCK_CHAT_ACTIONS
            .iter()
            .filter_map(|field| field.get(&keymap).as_key())
            .any(|key| presses.contains_key(&key));
    if chat.captures_gameplay {
        keys.reset_all();
        for field in KeyField::STOCK_CHAT_ACTIONS {
            if let InputBinding::Mouse(button) = field.get(&keymap) {
                mouse.clear_just_pressed(button);
            }
        }
    }
}

fn submit_text(
    state: Res<State<AppState>>,
    mut net: ResMut<NetState>,
    tx: Res<NetTx>,
    mut submitted: MessageReader<TextSubmitted>,
) {
    for event in submitted.read() {
        if *state.get() != AppState::Playing || !in_stock_match(&net) || event.value.is_empty() {
            continue;
        }
        if !matches!(event.field, TextField::MatchChat | TextField::MatchTeamChat) {
            continue;
        }
        if StockChatEvent::encode_request(&event.value).is_err() {
            system_notice::queue_net_notice(
                &mut net,
                "消息最多 255 个 GBK 字节，请缩短内容或移除不支持的字符",
            );
            continue;
        }
        match event.field {
            TextField::MatchChat => tx.send_control(Message::ChatSend {
                text: event.value.clone(),
            }),
            TextField::MatchTeamChat => {
                if let Some(sender) = &tx.active {
                    let _ = sender.send(NetOutbound::StockTeamChat(event.value.clone()));
                }
            }
            _ => {}
        }
    }
}

#[derive(Component)]
struct QuickMenuText;

fn render_quick_menu(
    mut commands: Commands,
    chat: Res<StockChatKeyboard>,
    mut previous: Local<Option<QuickMenu>>,
    mut net: ResMut<NetState>,
    font: Option<Res<UiFont>>,
    mut labels: Query<(Entity, &mut Text), With<QuickMenuText>>,
) {
    if *previous == chat.menu {
        return;
    }
    *previous = chat.menu;
    let Some(menu) = chat.menu else {
        for (entity, _) in &mut labels {
            commands.entity(entity).despawn();
        }
        return;
    };
    let table = net
        .stock_quick_chat
        .get_or_insert_with(system_notice::load_stock_quick_chat);
    let mut lines = Vec::new();
    match menu {
        QuickMenu::Team => {
            for (number, id, fallback) in [(1, 0, "攻击类"), (2, 8, "防守类"), (3, 17, "特殊类")]
            {
                let label = table.get(&id).map_or(fallback, String::as_str);
                lines.push(format!("{number}  {label}"));
            }
        }
        QuickMenu::Phrases(category) => {
            for index in 0..NUMBER_KEYS.len() as u32 {
                let Some(id) = (StockQuickChat { category, index }).message_id() else {
                    break;
                };
                let text = table
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| format!("快捷喊话 {id}"));
                lines.push(format!("{}  {text}", index + 1));
            }
        }
    }
    let text = lines.join("\n");
    if let Ok((_, mut label)) = labels.single_mut() {
        **label = text;
        return;
    }
    commands.spawn((
        QuickMenuText,
        Text::new(text),
        TextFont {
            font: font.map(|font| font.0.clone().into()).unwrap_or_default(),
            font_size: FontSize::Px(16.0),
            ..default()
        },
        TextColor(theme::TEXT),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(20.0),
            bottom: Val::Px(108.0),
            padding: UiRect::all(Val::Px(12.0)),
            ..default()
        },
        GlobalZIndex(650),
        BackgroundColor(theme::SURFACE),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_chat_keys_open_the_editors_and_route_quick_chat_without_leaking_keys() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut app = App::new();
        app.insert_resource(State::new(AppState::Playing))
            .insert_resource(NetState {
                address: "udp://127.0.0.1:5230".into(),
                status: NetStatus::InRoom,
                ..Default::default()
            })
            .insert_resource(NetTx {
                active: Some(sender),
                connecting: None,
            })
            .insert_resource(Keymap {
                chat_all: InputBinding::Key(KeyCode::Digit2),
                chat_team: InputBinding::Key(KeyCode::KeyK),
                quick_chat_all: InputBinding::Mouse(MouseButton::Middle),
                quick_chat_team: InputBinding::Key(KeyCode::KeyY),
                ..Default::default()
            })
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<StockChatKeyboard>()
            .init_resource::<NativeTextDialog>()
            .add_message::<OpenTextEditor>()
            .add_message::<TextSubmitted>()
            .add_message::<KeyboardInput>()
            .add_systems(PreUpdate, route_keyboard)
            .add_systems(Update, submit_text);

        let press = |app: &mut App, key| {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(key);
            app.update();
        };
        for (key, expected) in [
            (KeyCode::Digit2, TextField::MatchChat),
            (KeyCode::KeyK, TextField::MatchTeamChat),
        ] {
            press(&mut app, key);
            let mut requested = app.world_mut().resource_mut::<Messages<OpenTextEditor>>();
            assert_eq!(
                requested.drain().next().map(|message| message.0),
                Some(expected)
            );
        }
        app.world_mut().write_message(TextSubmitted {
            field: TextField::MatchChat,
            value: "你好".into(),
        });
        app.world_mut().write_message(TextSubmitted {
            field: TextField::MatchTeamChat,
            value: "集合".into(),
        });
        app.update();
        assert!(
            matches!(receiver.try_recv().unwrap(), NetOutbound::Control(Message::ChatSend { text }) if text == "你好")
        );
        assert!(
            matches!(receiver.try_recv().unwrap(), NetOutbound::StockTeamChat(text) if text == "集合")
        );

        press(&mut app, KeyCode::KeyB);
        assert!(!app
            .world()
            .resource::<StockChatKeyboard>()
            .captures_gameplay());
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Middle);
        app.update();
        press(&mut app, KeyCode::Digit2);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            NetOutbound::StockQuickChat(StockQuickChat {
                category: 39,
                index: 1
            })
        ));

        press(&mut app, KeyCode::KeyY);
        assert!(app
            .world()
            .resource::<StockChatKeyboard>()
            .captures_gameplay());
        assert!(!app
            .world()
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::KeyY));
        press(&mut app, KeyCode::Digit3);
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Digit3,
            logical_key: bevy::input::keyboard::Key::Character("3".into()),
            text: Some("3".into()),
            state: ButtonState::Pressed,
            repeat: true,
            window: Entity::PLACEHOLDER,
        });
        press(&mut app, KeyCode::Digit3);
        assert!(receiver.try_recv().is_err());
        press(&mut app, KeyCode::Digit8);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            NetOutbound::StockQuickChat(StockQuickChat {
                category: 38,
                index: 7
            })
        ));

        app.world_mut().resource_mut::<NetState>().address = "ws://localhost".into();
        press(&mut app, KeyCode::KeyY);
        assert!(!app
            .world()
            .resource::<StockChatKeyboard>()
            .captures_gameplay());
        assert!(app
            .world()
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::KeyY));
    }

    #[test]
    fn stock_chat_original_number_keys_select_the_wire_category_and_index() {
        for (key, category, message) in [(0, 36, 1), (1, 37, 9), (2, 38, 18)] {
            let (menu, chat) = QuickMenu::Team.select(key);
            assert!(chat.is_none());
            let (next, chat) = menu.unwrap().select(0);
            assert!(next.is_none());
            let chat = chat.unwrap();
            assert_eq!(chat.category, category);
            assert_eq!(chat.message_id(), Some(message));
        }
        let (next, chat) = QuickMenu::Phrases(39).select(8);
        assert!(next.is_none());
        assert_eq!(chat.unwrap().message_id(), Some(35));
        let (next, chat) = QuickMenu::Phrases(36).select(7);
        assert!(next.is_some());
        assert!(chat.is_none());
    }
}
