use bevy::prelude::*;

#[derive(States, Debug, Clone, Copy, Default, Eq, PartialEq, Hash)]
pub enum AppState {
    StudioSplash,
    #[default]
    Menu,
    Loading,
    Playing,

    Paused,

    Room,

    NetRoom,

    Settings,

    Result,
    Error,
}

#[derive(Resource, Debug, Clone)]
pub struct LaunchRequest {
    pub mode: String,
    pub stage_id: String,
    pub stage_name: String,
    pub player_name: String,
    pub room_name: String,

    pub public_battle: bool,

    pub character_id: u8,

    pub weapon: u8,

    pub stock_server_address: String,

    pub stock_password: String,

    pub stock_rule: i32,

    pub stock_game_num: u32,

    pub stock_game_time_minutes: u32,

    pub stock_life_adjust_percent: u32,

    pub stock_hide_item: bool,
    pub stock_health_item: bool,
    pub stock_update_item: bool,
    pub stock_special_item: bool,

    pub stock_camp: u32,

    pub spawn_room_npcs: bool,
    pub error: Option<String>,
}

impl Default for LaunchRequest {
    fn default() -> Self {
        Self {
            mode: "single".into(),
            stage_id: "sn01".into(),
            stage_name: "钟乳洞".into(),
            player_name: "玩家".into(),
            room_name: "流星的房间".into(),
            public_battle: cfg!(target_arch = "wasm32"),
            character_id: 0,
            weapon: 6,
            stock_server_address: "127.0.0.1:5230".into(),
            stock_password: String::new(),
            stock_rule: 1,
            stock_game_num: 999,
            stock_game_time_minutes: 999,
            stock_life_adjust_percent: 200,
            stock_hide_item: true,
            stock_health_item: true,
            stock_update_item: true,
            stock_special_item: true,
            stock_camp: 0,
            spawn_room_npcs: false,
            error: None,
        }
    }
}

#[derive(Resource, Debug, Clone)]
pub struct RoomMember {
    pub name: String,
    pub character_id: u8,
    pub weapon: u8,
    pub is_npc: bool,
}

#[derive(Resource, Debug, Clone)]
pub struct RoomState {
    pub members: Vec<RoomMember>,
}

impl Default for RoomState {
    fn default() -> Self {
        Self {
            members: vec![
                RoomMember {
                    name: "玩家".into(),
                    character_id: 0,
                    weapon: 6,
                    is_npc: false,
                },
                RoomMember {
                    name: "冷燕".into(),
                    character_id: 1,
                    weapon: 6,
                    is_npc: true,
                },
            ],
        }
    }
}

pub const MAX_ROOM_NPC: usize = 3;

#[derive(Resource, Debug, Clone)]
pub struct Settings {
    pub window_resolution: u8,
    pub fullscreen: bool,
    pub borderless_window: bool,

    pub graphics_adapter: String,
    pub graphics_backend: String,
    pub render_scale: f32,
    pub texture_quality: u8,
    pub hit_effects: bool,
    pub hit_stop: bool,
    pub volumetric_fog: bool,
    pub target_fps: u32,
    pub brightness: f32,
    pub contrast: f32,
    pub gamma: f32,
    pub music_enabled: bool,

    pub music_volume: f32,

    pub audio_volume: f32,
    pub auto_camera: bool,
    pub lock_flag: bool,
    pub show_names: bool,
    pub reverse_mouse: bool,
    pub drag_mouse: bool,

    pub mouse_speed: f32,

    pub ui_scale: f32,

    pub touch_controls: i8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window_resolution: 0,
            fullscreen: false,
            borderless_window: false,
            graphics_adapter: String::new(),
            graphics_backend: String::new(),
            render_scale: 1.0,
            texture_quality: 0,
            hit_effects: false,
            hit_stop: false,
            volumetric_fog: false,
            target_fps: 0,
            brightness: 94.0,
            contrast: 94.0,
            gamma: 49.0,

            music_enabled: !cfg!(target_arch = "wasm32"),
            music_volume: 50.0,
            audio_volume: 50.0,
            auto_camera: true,
            lock_flag: true,
            show_names: true,
            reverse_mouse: false,
            drag_mouse: false,
            mouse_speed: 99.0,
            ui_scale: 0.0,
            touch_controls: -1,
        }
    }
}

impl Settings {
    pub const MOUSE_SPEED_MIN: f32 = 1.0;
    pub const MOUSE_SPEED_MAX: f32 = 300.0;

    pub fn music_volume_linear(&self) -> f32 {
        (self.music_volume / 100.0).clamp(0.0, 1.0)
    }

    pub fn audio_volume_linear(&self) -> f32 {
        (self.audio_volume / 100.0).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputBinding {
    Key(KeyCode),
    Mouse(MouseButton),
}

impl InputBinding {
    pub fn pressed(self, keys: &ButtonInput<KeyCode>, mouse: &ButtonInput<MouseButton>) -> bool {
        match self {
            Self::Key(key) => keys.pressed(key),
            Self::Mouse(button) => mouse.pressed(button),
        }
    }

    pub fn just_pressed(
        self,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        match self {
            Self::Key(key) => keys.just_pressed(key),
            Self::Mouse(button) => mouse.just_pressed(button),
        }
    }

    pub fn just_released(
        self,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        match self {
            Self::Key(key) => keys.just_released(key),
            Self::Mouse(button) => mouse.just_released(button),
        }
    }

    pub fn as_key(self) -> Option<KeyCode> {
        match self {
            Self::Key(key) => Some(key),
            Self::Mouse(_) => None,
        }
    }

    pub fn name(self) -> String {
        match self {
            Self::Key(key) => key_name(key),
            Self::Mouse(MouseButton::Left) => "Mouse0".to_string(),
            Self::Mouse(MouseButton::Right) => "Mouse2".to_string(),
            Self::Mouse(MouseButton::Middle) => "Mouse1".to_string(),
            Self::Mouse(button) => format!("Mouse{button:?}"),
        }
    }

    pub fn label(self) -> String {
        format_binding_label(self)
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Mouse0" => Some(Self::Mouse(MouseButton::Left)),
            "Mouse1" => Some(Self::Mouse(MouseButton::Middle)),
            "Mouse2" => Some(Self::Mouse(MouseButton::Right)),
            _ => parse_key(name).map(Self::Key),
        }
    }
}

#[derive(Resource, Debug, Clone)]
pub struct Keymap {
    pub move_up: InputBinding,
    pub move_down: InputBinding,
    pub move_left: InputBinding,
    pub move_right: InputBinding,
    pub jump: InputBinding,
    pub crouch: InputBinding,
    pub guard: InputBinding,
    pub run: InputBinding,
    pub attack: InputBinding,

    pub qi_burst: InputBinding,
    pub fake: InputBinding,
    pub weapon_switch: InputBinding,
    pub cure: InputBinding,
    pub drop_weapon: InputBinding,
    pub drop_item: InputBinding,
    pub camera_up: InputBinding,
    pub camera_down: InputBinding,
    pub left_turn: InputBinding,
    pub right_turn: InputBinding,

    pub lock_toggle: InputBinding,
    pub radar: InputBinding,
    pub debug_panel: InputBinding,

    pub dodge: InputBinding,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            move_up: InputBinding::Key(KeyCode::KeyW),
            move_down: InputBinding::Key(KeyCode::KeyS),
            move_left: InputBinding::Key(KeyCode::KeyA),
            move_right: InputBinding::Key(KeyCode::KeyD),
            jump: InputBinding::Key(KeyCode::Space),
            crouch: InputBinding::Key(KeyCode::KeyC),
            guard: InputBinding::Mouse(MouseButton::Right),
            run: InputBinding::Key(KeyCode::ControlLeft),
            attack: InputBinding::Mouse(MouseButton::Left),
            qi_burst: InputBinding::Key(KeyCode::KeyF),

            fake: InputBinding::Key(KeyCode::ShiftLeft),
            weapon_switch: InputBinding::Key(KeyCode::KeyE),
            cure: InputBinding::Key(KeyCode::KeyT),
            drop_weapon: InputBinding::Key(KeyCode::KeyG),
            drop_item: InputBinding::Key(KeyCode::KeyH),
            camera_up: InputBinding::Key(KeyCode::ArrowUp),
            camera_down: InputBinding::Key(KeyCode::ArrowDown),
            left_turn: InputBinding::Key(KeyCode::ArrowLeft),
            right_turn: InputBinding::Key(KeyCode::ArrowRight),
            lock_toggle: InputBinding::Key(KeyCode::KeyQ),
            radar: InputBinding::Key(KeyCode::KeyI),
            debug_panel: InputBinding::Key(KeyCode::Backquote),
            dodge: InputBinding::Key(KeyCode::KeyV),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyField {
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    Jump,
    Crouch,
    Guard,
    Run,
    Attack,
    QiBurst,
    Fake,
    WeaponSwitch,
    Cure,
    DropWeapon,
    DropItem,
    CameraUp,
    CameraDown,
    LeftTurn,
    RightTurn,
    LockToggle,
    Radar,
    DebugPanel,
    Dodge,
}

impl KeyField {
    pub fn label(&self) -> &'static str {
        match self {
            Self::MoveUp => "前进",
            Self::MoveDown => "后退",
            Self::MoveLeft => "左移",
            Self::MoveRight => "右移",
            Self::Jump => "跳跃",
            Self::Crouch => "蹲下",
            Self::Guard => "防御",
            Self::Run => "疾跑",
            Self::Attack => "攻击",
            Self::QiBurst => "爆气（消耗 60 怒气）",
            Self::Fake => "假死",
            Self::WeaponSwitch => "换武器",
            Self::Cure => "救治队员",
            Self::DropWeapon => "丢武器",
            Self::DropItem => "丢道具",
            Self::CameraUp => "镜头上仰",
            Self::CameraDown => "镜头下俯",
            Self::LeftTurn => "左转身",
            Self::RightTurn => "右转身",
            Self::LockToggle => "锁定 / 解除锁定",
            Self::Radar => "雷达开关",
            Self::DebugPanel => "调试面板",
            Self::Dodge => "闪避",
        }
    }

    pub fn get(&self, keymap: &Keymap) -> InputBinding {
        match self {
            Self::MoveUp => keymap.move_up,
            Self::MoveDown => keymap.move_down,
            Self::MoveLeft => keymap.move_left,
            Self::MoveRight => keymap.move_right,
            Self::Jump => keymap.jump,
            Self::Crouch => keymap.crouch,
            Self::Guard => keymap.guard,
            Self::Run => keymap.run,
            Self::Attack => keymap.attack,
            Self::QiBurst => keymap.qi_burst,
            Self::Fake => keymap.fake,
            Self::WeaponSwitch => keymap.weapon_switch,
            Self::Cure => keymap.cure,
            Self::DropWeapon => keymap.drop_weapon,
            Self::DropItem => keymap.drop_item,
            Self::CameraUp => keymap.camera_up,
            Self::CameraDown => keymap.camera_down,
            Self::LeftTurn => keymap.left_turn,
            Self::RightTurn => keymap.right_turn,
            Self::LockToggle => keymap.lock_toggle,
            Self::Radar => keymap.radar,
            Self::DebugPanel => keymap.debug_panel,
            Self::Dodge => keymap.dodge,
        }
    }

    pub fn set(&self, keymap: &mut Keymap, key: KeyCode) {
        self.set_binding(keymap, InputBinding::Key(key));
    }

    pub fn set_binding(&self, keymap: &mut Keymap, binding: InputBinding) {
        match self {
            Self::MoveUp => keymap.move_up = binding,
            Self::MoveDown => keymap.move_down = binding,
            Self::MoveLeft => keymap.move_left = binding,
            Self::MoveRight => keymap.move_right = binding,
            Self::Jump => keymap.jump = binding,
            Self::Crouch => keymap.crouch = binding,
            Self::Guard => keymap.guard = binding,
            Self::Run => keymap.run = binding,
            Self::Attack => keymap.attack = binding,
            Self::QiBurst => keymap.qi_burst = binding,
            Self::Fake => keymap.fake = binding,
            Self::WeaponSwitch => keymap.weapon_switch = binding,
            Self::Cure => keymap.cure = binding,
            Self::DropWeapon => keymap.drop_weapon = binding,
            Self::DropItem => keymap.drop_item = binding,
            Self::CameraUp => keymap.camera_up = binding,
            Self::CameraDown => keymap.camera_down = binding,
            Self::LeftTurn => keymap.left_turn = binding,
            Self::RightTurn => keymap.right_turn = binding,
            Self::LockToggle => keymap.lock_toggle = binding,
            Self::Radar => keymap.radar = binding,
            Self::DebugPanel => keymap.debug_panel = binding,
            Self::Dodge => keymap.dodge = binding,
        }
    }

    pub const BINDING_ACTIONS: [KeyField; 22] = [
        Self::MoveUp,
        Self::MoveDown,
        Self::MoveLeft,
        Self::MoveRight,
        Self::Jump,
        Self::Crouch,
        Self::Guard,
        Self::Run,
        Self::Attack,
        Self::QiBurst,
        Self::Fake,
        Self::WeaponSwitch,
        Self::Cure,
        Self::DropWeapon,
        Self::DropItem,
        Self::CameraUp,
        Self::CameraDown,
        Self::LeftTurn,
        Self::RightTurn,
        Self::LockToggle,
        Self::Radar,
        Self::DebugPanel,
    ];
}

pub fn assign_key_binding(keymap: &mut Keymap, field: KeyField, binding: InputBinding) {
    let previous = field.get(keymap);
    if previous == binding {
        return;
    }
    for other in KeyField::BINDING_ACTIONS {
        if other != field && other.get(keymap) == binding {
            other.set_binding(keymap, previous);
            break;
        }
    }
    field.set_binding(keymap, binding);
}

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct KeyCaptureState {
    pub pending: Option<KeyField>,

    pub arming: bool,

    pub esc_consumed: bool,
}

impl KeyCaptureState {
    pub fn begin(&mut self, field: KeyField) {
        self.pending = Some(field);
        self.arming = true;
        self.esc_consumed = false;
    }

    pub fn clear(&mut self) {
        self.pending = None;
        self.arming = false;
    }

    pub fn cancel_by_esc(&mut self) {
        self.clear();
        self.esc_consumed = true;
    }
}

pub fn parse_key(name: &str) -> Option<KeyCode> {
    Some(match name {
        "Space" => KeyCode::Space,
        "Tab" => KeyCode::Tab,
        "Enter" => KeyCode::Enter,
        "Escape" => KeyCode::Escape,
        "Backspace" => KeyCode::Backspace,
        "CapsLock" => KeyCode::CapsLock,
        "ShiftLeft" => KeyCode::ShiftLeft,
        "ShiftRight" => KeyCode::ShiftRight,
        "ControlLeft" => KeyCode::ControlLeft,
        "ControlRight" => KeyCode::ControlRight,
        "AltLeft" => KeyCode::AltLeft,
        "AltRight" => KeyCode::AltRight,
        "ArrowUp" => KeyCode::ArrowUp,
        "ArrowDown" => KeyCode::ArrowDown,
        "ArrowLeft" => KeyCode::ArrowLeft,
        "ArrowRight" => KeyCode::ArrowRight,
        "Backquote" => KeyCode::Backquote,
        "Minus" => KeyCode::Minus,
        "Equal" => KeyCode::Equal,
        "BracketLeft" => KeyCode::BracketLeft,
        "BracketRight" => KeyCode::BracketRight,
        "Backslash" => KeyCode::Backslash,
        "Semicolon" => KeyCode::Semicolon,
        "Quote" => KeyCode::Quote,
        "Comma" => KeyCode::Comma,
        "Period" => KeyCode::Period,
        "Slash" => KeyCode::Slash,
        "Digit0" => KeyCode::Digit0,
        "Digit1" => KeyCode::Digit1,
        "Digit2" => KeyCode::Digit2,
        "Digit3" => KeyCode::Digit3,
        "Digit4" => KeyCode::Digit4,
        "Digit5" => KeyCode::Digit5,
        "Digit6" => KeyCode::Digit6,
        "Digit7" => KeyCode::Digit7,
        "Digit8" => KeyCode::Digit8,
        "Digit9" => KeyCode::Digit9,
        "KeyA" => KeyCode::KeyA,
        "KeyB" => KeyCode::KeyB,
        "KeyC" => KeyCode::KeyC,
        "KeyD" => KeyCode::KeyD,
        "KeyE" => KeyCode::KeyE,
        "KeyF" => KeyCode::KeyF,
        "KeyG" => KeyCode::KeyG,
        "KeyH" => KeyCode::KeyH,
        "KeyI" => KeyCode::KeyI,
        "KeyJ" => KeyCode::KeyJ,
        "KeyK" => KeyCode::KeyK,
        "KeyL" => KeyCode::KeyL,
        "KeyM" => KeyCode::KeyM,
        "KeyN" => KeyCode::KeyN,
        "KeyO" => KeyCode::KeyO,
        "KeyP" => KeyCode::KeyP,
        "KeyQ" => KeyCode::KeyQ,
        "KeyR" => KeyCode::KeyR,
        "KeyS" => KeyCode::KeyS,
        "KeyT" => KeyCode::KeyT,
        "KeyU" => KeyCode::KeyU,
        "KeyV" => KeyCode::KeyV,
        "KeyW" => KeyCode::KeyW,
        "KeyX" => KeyCode::KeyX,
        "KeyY" => KeyCode::KeyY,
        "KeyZ" => KeyCode::KeyZ,
        "F1" => KeyCode::F1,
        "F2" => KeyCode::F2,
        "F3" => KeyCode::F3,
        "F4" => KeyCode::F4,
        "F5" => KeyCode::F5,
        "F6" => KeyCode::F6,
        "F7" => KeyCode::F7,
        "F8" => KeyCode::F8,
        "F9" => KeyCode::F9,
        "F10" => KeyCode::F10,
        "F11" => KeyCode::F11,
        "F12" => KeyCode::F12,
        "Numpad0" => KeyCode::Numpad0,
        "Numpad1" => KeyCode::Numpad1,
        "Numpad2" => KeyCode::Numpad2,
        "Numpad3" => KeyCode::Numpad3,
        "Numpad4" => KeyCode::Numpad4,
        "Numpad5" => KeyCode::Numpad5,
        "Numpad6" => KeyCode::Numpad6,
        "Numpad7" => KeyCode::Numpad7,
        "Numpad8" => KeyCode::Numpad8,
        "Numpad9" => KeyCode::Numpad9,
        _ => return None,
    })
}

pub fn key_name(key: KeyCode) -> String {
    format!("{key:?}")
}

pub fn format_binding_label(binding: InputBinding) -> String {
    match binding {
        InputBinding::Mouse(MouseButton::Left) => "鼠标左键".into(),
        InputBinding::Mouse(MouseButton::Middle) => "鼠标中键".into(),
        InputBinding::Mouse(MouseButton::Right) => "鼠标右键".into(),
        InputBinding::Mouse(button) => format!("鼠标{button:?}"),
        InputBinding::Key(key) => match key {
            KeyCode::Space => "空格".into(),
            KeyCode::Tab => "Tab".into(),
            KeyCode::Enter => "回车".into(),
            KeyCode::Backspace => "退格".into(),
            KeyCode::CapsLock => "CapsLock".into(),
            KeyCode::ShiftLeft => "左Shift".into(),
            KeyCode::ShiftRight => "右Shift".into(),
            KeyCode::ControlLeft => "左Ctrl".into(),
            KeyCode::ControlRight => "右Ctrl".into(),
            KeyCode::AltLeft => "左Alt".into(),
            KeyCode::AltRight => "右Alt".into(),
            KeyCode::ArrowUp => "↑".into(),
            KeyCode::ArrowDown => "↓".into(),
            KeyCode::ArrowLeft => "←".into(),
            KeyCode::ArrowRight => "→".into(),
            KeyCode::Backquote => "～".into(),
            KeyCode::Minus => "-".into(),
            KeyCode::Equal => "=".into(),
            KeyCode::BracketLeft => "[".into(),
            KeyCode::BracketRight => "]".into(),
            KeyCode::Backslash => "\\".into(),
            KeyCode::Semicolon => ";".into(),
            KeyCode::Quote => "'".into(),
            KeyCode::Comma => ",".into(),
            KeyCode::Period => ".".into(),
            KeyCode::Slash => "/".into(),
            other => {
                let name = key_name(other);
                if let Some(rest) = name.strip_prefix("Key") {
                    rest.to_string()
                } else if let Some(rest) = name.strip_prefix("Digit") {
                    rest.to_string()
                } else if let Some(rest) = name.strip_prefix("Numpad") {
                    format!("小键盘{rest}")
                } else {
                    name
                }
            }
        },
    }
}

pub const SETTINGS_FILE: &str = "settings.txt";
#[cfg(target_arch = "wasm32")]
const SETTINGS_STORAGE_KEY: &str = "meteor.rust.settings.v1";

#[cfg(not(target_arch = "wasm32"))]
fn settings_file_path() -> std::path::PathBuf {
    #[cfg(target_os = "android")]
    if let Some(path) = bevy::android::ANDROID_APP
        .get()
        .and_then(|app| app.internal_data_path())
    {
        return path.join(SETTINGS_FILE);
    }
    std::path::PathBuf::from(SETTINGS_FILE)
}

fn append_binding(out: &mut String, key: &str, binding: InputBinding) {
    out.push_str(key);
    out.push('=');
    out.push_str(&binding.name());
    out.push('\n');
}

fn settings_text(settings: &Settings, keymap: &Keymap) -> String {
    let mut text = format!(
        "render_scale={}
texture_quality={}
hit_effects={}
hit_stop={}
volumetric_fog={}
target_fps={}
brightness={}
contrast={}
gamma={}
music_enabled={}
music_volume={}
audio_volume={}
auto_camera={}
lock_flag={}
show_names={}
reverse_mouse={}
drag_mouse={}
mouse_speed={}
ui_scale={}
touch_controls={}
",
        settings.render_scale,
        settings.texture_quality,
        u8::from(settings.hit_effects),
        u8::from(settings.hit_stop),
        u8::from(settings.volumetric_fog),
        settings.target_fps,
        settings.brightness,
        settings.contrast,
        settings.gamma,
        u8::from(settings.music_enabled),
        settings.music_volume,
        settings.audio_volume,
        u8::from(settings.auto_camera),
        u8::from(settings.lock_flag),
        u8::from(settings.show_names),
        u8::from(settings.reverse_mouse),
        u8::from(settings.drag_mouse),
        settings.mouse_speed,
        settings.ui_scale,
        settings.touch_controls,
    );
    text.push_str(&format!(
        "graphics_adapter={}\ngraphics_backend={}\n",
        settings.graphics_adapter.replace(['\r', '\n'], ""),
        settings.graphics_backend.replace(['\r', '\n'], "")
    ));
    text.push_str(&format!(
        "window_resolution={}\nfullscreen={}\nborderless_window={}\n",
        crate::window_settings::resolution_label(settings.window_resolution),
        u8::from(settings.fullscreen),
        u8::from(settings.borderless_window)
    ));
    for field in KeyField::BINDING_ACTIONS {
        let storage_key = key_field_storage_key(field);
        append_binding(&mut text, storage_key, field.get(keymap));
    }
    append_binding(&mut text, "dodge", keymap.dodge);
    text
}

pub fn save_settings(settings: &Settings, keymap: &Keymap) -> bool {
    let text = settings_text(settings, keymap);
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.local_storage().ok().flatten())
            .is_some_and(|storage| storage.set_item(SETTINGS_STORAGE_KEY, &text).is_ok())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::write(settings_file_path(), text).is_ok()
    }
}

fn key_field_storage_key(field: KeyField) -> &'static str {
    match field {
        KeyField::MoveUp => "move_up",
        KeyField::MoveDown => "move_down",
        KeyField::MoveLeft => "move_left",
        KeyField::MoveRight => "move_right",
        KeyField::Jump => "jump",
        KeyField::Crouch => "crouch",
        KeyField::Guard => "guard",
        KeyField::Run => "run",
        KeyField::Attack => "attack",
        KeyField::QiBurst => "qi_burst",
        KeyField::Fake => "fake",
        KeyField::WeaponSwitch => "weapon_switch",
        KeyField::Cure => "cure",
        KeyField::DropWeapon => "drop_weapon",
        KeyField::DropItem => "drop_item",
        KeyField::CameraUp => "camera_up",
        KeyField::CameraDown => "camera_down",
        KeyField::LeftTurn => "left_turn",
        KeyField::RightTurn => "right_turn",
        KeyField::LockToggle => "lock_toggle",
        KeyField::Radar => "radar",
        KeyField::DebugPanel => "debug_panel",
        KeyField::Dodge => "dodge",
    }
}

fn parse_bool(value: &str) -> bool {
    matches!(value, "1" | "true" | "True" | "yes")
}

fn apply_binding_value(keymap: &mut Keymap, field: KeyField, value: &str) {
    if let Some(binding) = InputBinding::from_name(value) {
        field.set_binding(keymap, binding);
    }
}

pub fn load_settings() -> (Settings, Keymap) {
    #[cfg(target_arch = "wasm32")]
    let text = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(SETTINGS_STORAGE_KEY).ok().flatten());
    #[cfg(not(target_arch = "wasm32"))]
    let text = std::fs::read_to_string(settings_file_path()).ok();
    parse_settings(text.as_deref().unwrap_or_default())
}

fn parse_settings(text: &str) -> (Settings, Keymap) {
    let mut settings = Settings::default();
    let mut keymap = Keymap::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "window_resolution" => {
                settings.window_resolution = crate::window_settings::parse_resolution(value)
            }
            "fullscreen" => settings.fullscreen = parse_bool(value),
            "borderless_window" => settings.borderless_window = parse_bool(value),
            "graphics_adapter" => settings.graphics_adapter = value.into(),
            "graphics_backend" => settings.graphics_backend = value.into(),
            "render_scale" => {
                settings.render_scale = value.parse().unwrap_or(settings.render_scale);
            }
            "texture_quality" => {
                settings.texture_quality = value.parse().unwrap_or(settings.texture_quality);
            }
            "hit_effects" => settings.hit_effects = parse_bool(value),
            "hit_stop" => settings.hit_stop = parse_bool(value),
            "volumetric_fog" => settings.volumetric_fog = parse_bool(value),
            "target_fps" => settings.target_fps = value.parse().unwrap_or(settings.target_fps),
            "brightness" => settings.brightness = value.parse().unwrap_or(settings.brightness),
            "contrast" => settings.contrast = value.parse().unwrap_or(settings.contrast),
            "gamma" => settings.gamma = value.parse().unwrap_or(settings.gamma),
            "music_enabled" => settings.music_enabled = parse_bool(value),
            "music_volume" => {
                settings.music_volume = value.parse().unwrap_or(settings.music_volume);
            }
            "audio_volume" => {
                settings.audio_volume = value.parse().unwrap_or(settings.audio_volume);
            }
            "auto_camera" => settings.auto_camera = parse_bool(value),
            "lock_flag" => settings.lock_flag = parse_bool(value),
            "show_names" => settings.show_names = parse_bool(value),
            "reverse_mouse" => settings.reverse_mouse = parse_bool(value),
            "drag_mouse" => settings.drag_mouse = parse_bool(value),
            "mouse_speed" => {
                settings.mouse_speed = value.parse().unwrap_or(settings.mouse_speed);
            }
            "ui_scale" => {
                settings.ui_scale = value.parse().unwrap_or(settings.ui_scale);
            }
            "touch_controls" => {
                settings.touch_controls = value.parse().unwrap_or(settings.touch_controls);
            }

            "bgm_volume" => {
                if let Ok(v) = value.parse::<f32>() {
                    settings.music_volume = (v * 100.0).clamp(0.0, 100.0);
                }
            }
            "sfx_volume" => {
                if let Ok(v) = value.parse::<f32>() {
                    settings.audio_volume = (v * 100.0).clamp(0.0, 100.0);
                }
            }
            "move_up" | "forward" => apply_binding_value(&mut keymap, KeyField::MoveUp, value),
            "move_down" | "backward" => apply_binding_value(&mut keymap, KeyField::MoveDown, value),
            "move_left" | "left" => apply_binding_value(&mut keymap, KeyField::MoveLeft, value),
            "move_right" | "right" => apply_binding_value(&mut keymap, KeyField::MoveRight, value),
            "jump" => apply_binding_value(&mut keymap, KeyField::Jump, value),
            "crouch" => apply_binding_value(&mut keymap, KeyField::Crouch, value),
            "guard" => apply_binding_value(&mut keymap, KeyField::Guard, value),
            "run" => apply_binding_value(&mut keymap, KeyField::Run, value),
            "attack" => apply_binding_value(&mut keymap, KeyField::Attack, value),
            "qi_burst" | "hit_attack" => apply_binding_value(&mut keymap, KeyField::QiBurst, value),
            "fake" => apply_binding_value(&mut keymap, KeyField::Fake, value),
            "weapon_switch" | "switch_weapon" => {
                apply_binding_value(&mut keymap, KeyField::WeaponSwitch, value)
            }
            "cure" => apply_binding_value(&mut keymap, KeyField::Cure, value),
            "drop_weapon" => apply_binding_value(&mut keymap, KeyField::DropWeapon, value),
            "drop_item" => apply_binding_value(&mut keymap, KeyField::DropItem, value),
            "camera_up" | "cameraUp" => apply_binding_value(&mut keymap, KeyField::CameraUp, value),
            "camera_down" | "cameraDown" => {
                apply_binding_value(&mut keymap, KeyField::CameraDown, value)
            }
            "left_turn" | "leftTurn" => apply_binding_value(&mut keymap, KeyField::LeftTurn, value),
            "right_turn" | "rightTurn" => {
                apply_binding_value(&mut keymap, KeyField::RightTurn, value)
            }
            "lock_toggle" | "unlockCamera" => {
                apply_binding_value(&mut keymap, KeyField::LockToggle, value)
            }
            "radar" => apply_binding_value(&mut keymap, KeyField::Radar, value),
            "debug_panel" => apply_binding_value(&mut keymap, KeyField::DebugPanel, value),
            "dodge" => {
                if let Some(binding) = InputBinding::from_name(value) {
                    keymap.dodge = binding;
                }
            }
            _ => {}
        }
    }
    (settings, keymap)
}

pub fn reset_settings_defaults(settings: &mut Settings, keymap: &mut Keymap) {
    *settings = Settings::default();
    *keymap = Keymap::default();
    let _ = save_settings(settings, keymap);
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsBoolField {
    Fullscreen,
    BorderlessWindow,
    HitEffects,
    HitStop,
    VolumetricFog,
    MusicEnabled,
    AutoCamera,
    LockFlag,
    ShowNames,
    ReverseMouse,
    DragMouse,
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSelectField {
    WindowResolution,
    GraphicsAdapter,
    GraphicsBackend,
    RenderScale,
    TextureQuality,
    TargetFps,

    UiScale,

    TouchControls,
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRangeField {
    Brightness,
    Contrast,
    Gamma,
    MusicVolume,
    AudioVolume,
    MouseSpeed,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct SettingsRangeValue(pub SettingsRangeField);

#[derive(Component, Debug, Clone, Copy)]
pub struct SettingsRangeDown(pub SettingsRangeField);

#[derive(Component, Debug, Clone, Copy)]
pub struct SettingsRangeUp(pub SettingsRangeField);

#[derive(Component, Debug, Clone, Copy)]
pub struct KeyCaptureButton(pub KeyField);

#[derive(Component)]
pub struct SettingsResetButton;

#[derive(Component)]
pub struct SettingsFormRoot;

#[derive(Resource, Debug, Clone, Default)]
pub struct LoadedStageInfo {
    pub stage_id: String,
    pub ready: bool,
    pub scene_object_count: usize,
    pub collision_face_count: usize,
    pub center: [f32; 3],
    pub error: Option<String>,
}

#[derive(Resource, Debug, Clone, Default)]
pub struct LoadedStageAssets {
    pub gmb: Option<Vec<u8>>,
    pub des: Option<Vec<u8>>,
    pub cob: Option<Vec<u8>>,
    pub from_pack: bool,
}

#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MenuScreen {
    #[default]
    Main,
    Stages,
    Network,
    Settings,
    Replay,
}

#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StageListTab {
    #[default]
    Official,
    Custom,
}

#[derive(Component)]
pub struct MenuRoot;

#[derive(Component)]
pub struct MenuNavigateButton(pub MenuScreen);

#[derive(Component)]
pub struct MenuBackButton;

#[derive(Component)]
pub struct LauncherNavButton;

#[derive(Component)]
pub struct LauncherStyledButton;

#[derive(Component)]
pub struct LauncherScreenMarker(pub MenuScreen);

#[derive(Component)]
pub struct LauncherBrandPanel;

#[derive(Component)]
pub struct StageTabOfficialButton;

#[derive(Component)]
pub struct StageTabCustomButton;

#[derive(Component)]
pub struct SelectedStageNameText;

#[derive(Component)]
pub struct SelectedStageCodeText;

#[derive(Component)]
pub struct OfficialStageGrid;

#[derive(Component)]
pub struct CustomStageGrid;

#[derive(Component)]
pub struct LoadingRoot;

#[derive(Component)]
pub struct PlayingRoot;

#[derive(Component)]
pub struct ModeButton(pub String);

#[derive(Component)]
pub struct StageButton(pub String);

#[derive(Component)]
pub struct CharacterButton(pub u8);

#[derive(Component)]
pub struct WeaponButton(pub u8);

#[derive(Component)]
pub struct StartGameButton;

#[derive(Component)]
pub struct BackToMenuButton;

#[derive(Component)]
pub struct MenuRoomButton;

#[derive(Component)]
pub struct MenuSettingsButton;

#[derive(Component)]
pub struct SettingsRoot;

#[derive(Component)]
pub struct SettingsBackButton;

#[derive(Component)]
pub struct MenuRecordButton;

#[derive(Component)]
pub struct MenuReplayButton;

#[derive(Component)]
pub struct MenuMrcButton;

#[derive(Component)]
pub struct MenuAutoRecordButton;

#[derive(Component)]
pub struct RoomRoot;

#[derive(Component)]
pub struct RoomAddNpcButton;

#[derive(Component)]
pub struct RoomRemoveNpcButton;

#[derive(Component)]
pub struct RoomStartButton;

#[derive(Component)]
pub struct RoomBackButton;

#[derive(Component)]
pub struct MenuNetButton;

#[derive(Component)]
pub struct MenuStockJoinButton;

#[derive(Component)]
pub struct MenuStockHostButton;

#[derive(Component)]
pub struct MenuPublicBattleButton;

#[derive(Component)]
pub struct MenuStockLoadoutDialog;

#[derive(Component)]
pub struct MenuStockLoadoutConfirm;

#[derive(Component)]
pub struct MenuStockLoadoutCancel;

#[derive(Component)]
pub struct MenuStockDiscoveryText;

#[derive(Component)]
pub struct MenuStockRefreshButton;

#[derive(Component, Debug, Clone, Copy)]
pub struct MenuStockCampButton(pub u32);

#[derive(Component, Debug, Clone, Copy)]
pub struct MenuStockRuleButton(pub i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockItemRule {
    Hide,
    Health,
    Update,
    Special,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct MenuStockItemRuleButton(pub StockItemRule);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockConfigField {
    GameNum,
    GameTime,
    LifeAdjust,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct MenuStockSelectOption {
    pub field: StockConfigField,
    pub value: u32,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct MenuStockSelectText(pub StockConfigField);

#[derive(Component, Debug, Clone)]
pub struct MenuStockStageButton {
    pub id: String,
    pub name: String,
}

#[derive(Component)]
pub struct MenuStockStageText;

#[derive(Component)]
pub struct MenuNetStatusText;

#[derive(Component)]
pub struct NetRoomRoot;

#[derive(Component)]
pub struct NetRoomMembersText;

#[derive(Component)]
pub struct NetRoomStartButton;

#[derive(Component)]
pub struct NetRoomTeamSelectButton {
    pub team: u32,
}

#[derive(Component)]
pub struct NetRoomLeaveButton;

#[derive(Component)]
pub struct NetRoomChatText;

#[derive(Component)]
pub struct NetRoomChatInputText;

#[derive(Component)]
pub struct ResultRoot;

#[derive(Component)]
pub struct ResultBackToMenuButton;

#[derive(Component)]
pub struct ResultRetryButton;

#[cfg(test)]
mod keymap_tests {
    use super::*;

    #[test]
    fn debug_panel_binding_swaps_and_restores_from_saved_value() {
        let mut keymap = Keymap::default();
        assert_eq!(keymap.debug_panel.label(), "～");
        assign_key_binding(
            &mut keymap,
            KeyField::DebugPanel,
            InputBinding::Key(KeyCode::KeyI),
        );
        assert_eq!(keymap.radar, InputBinding::Key(KeyCode::Backquote));
        let saved = keymap.debug_panel.name();
        let mut restored = Keymap::default();
        apply_binding_value(&mut restored, KeyField::DebugPanel, &saved);
        assert_eq!(restored.debug_panel, keymap.debug_panel);
        assert_eq!(key_field_storage_key(KeyField::DebugPanel), "debug_panel");
    }

    #[test]
    fn native_defaults_enable_bgm_while_browser_keeps_autoplay_opt_in() {
        assert_eq!(
            Settings::default().music_enabled,
            !cfg!(target_arch = "wasm32")
        );
    }

    #[test]
    fn default_keymap_keeps_fake_on_the_original_left_shift() {
        let keymap = Keymap::default();
        assert_eq!(keymap.fake, InputBinding::Key(KeyCode::ShiftLeft));
        assert_ne!(keymap.dodge, keymap.fake, "桌面闪避键不得与假死同键");
    }

    #[test]
    fn default_keymap_has_no_duplicate_bindings() {
        let keymap = Keymap::default();
        let mut seen: Vec<(KeyField, InputBinding)> = Vec::new();
        for field in KeyField::BINDING_ACTIONS
            .into_iter()
            .chain([KeyField::Dodge])
        {
            let binding = field.get(&keymap);
            if let Some((other, _)) = seen.iter().find(|(_, b)| *b == binding) {
                panic!("{field:?} 与 {other:?} 重复绑定 {binding:?}");
            }
            seen.push((field, binding));
        }
    }
}

#[cfg(test)]
mod graphics_settings_persistence_tests {
    #[test]
    fn hit_stop_is_opt_in_and_persisted() {
        let (legacy, _) = super::parse_settings("hit_effects=1\n");
        assert!(legacy.hit_effects);
        assert!(!legacy.hit_stop);
        for enabled in [false, true] {
            let settings = super::Settings {
                hit_stop: enabled,
                ..Default::default()
            };
            let (loaded, _) =
                super::parse_settings(&super::settings_text(&settings, &super::Keymap::default()));
            assert_eq!(loaded.hit_stop, enabled);
        }
    }
    use super::*;
    #[test]
    fn graphics_preferences_round_trip_and_old_configs_default_to_auto() {
        let settings = Settings {
            window_resolution: 4,
            fullscreen: true,
            borderless_window: true,
            graphics_adapter: "GPU = test".into(),
            graphics_backend: "Vulkan".into(),
            ..Default::default()
        };
        let (loaded, _) = parse_settings(&settings_text(&settings, &Keymap::default()));
        assert_eq!(loaded.graphics_adapter, settings.graphics_adapter);
        assert_eq!(loaded.graphics_backend, settings.graphics_backend);
        assert_eq!(loaded.window_resolution, 4);
        assert!(loaded.fullscreen);
        assert!(loaded.borderless_window);
        let (old, _) = parse_settings("music_volume=23\n");
        assert!(old.graphics_adapter.is_empty());
        assert!(old.graphics_backend.is_empty());
        assert_eq!(old.window_resolution, 0);
        assert!(!old.fullscreen);
        assert!(!old.borderless_window);
        assert_eq!(old.music_volume, 23.0);
    }
}
