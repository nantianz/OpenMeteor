use super::replay_plugin::{MrcPlayback, ReplayPlayer};
use crate::app_state::{AppState, LaunchRequest, MenuMrcButton, MenuScreen};
use bevy::prelude::*;
use std::sync::{mpsc, Mutex};

type Selection = Result<Option<(String, Vec<u8>)>, String>;

#[derive(Resource, Default)]
pub(crate) struct MrcFilePicker {
    pending: Option<Mutex<mpsc::Receiver<Selection>>>,
    status: String,
}

#[derive(Component)]
pub(crate) struct MrcFileStatus;

pub(crate) fn pick_mrc_button(
    _main_thread: bevy::ecs::system::NonSendMarker,
    query: Query<&Interaction, (Changed<Interaction>, With<MenuMrcButton>)>,
    state: Res<State<AppState>>,
    screen: Res<MenuScreen>,
    mut picker: ResMut<MrcFilePicker>,
) {
    if *state.get() != AppState::Menu
        || *screen != MenuScreen::Replay
        || picker.pending.is_some()
        || !query.iter().any(|value| *value == Interaction::Pressed)
    {
        return;
    }
    let (sender, receiver) = mpsc::channel();
    picker.pending = Some(Mutex::new(receiver));
    picker.status = "正在选择录像…".into();
    open_picker(sender);
}

#[cfg(not(target_os = "android"))]
fn open_picker(sender: mpsc::Sender<Selection>) {
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("选择 MRC 录像")
        .add_filter("MRC 录像", &["mrc"])
        .pick_file();
    let task = async move {
        let selection = match dialog.await {
            None => Ok(None),
            Some(file) => {
                #[cfg(not(target_arch = "wasm32"))]
                let bytes = std::fs::read(file.path()).map_err(|error| error.to_string());
                #[cfg(target_arch = "wasm32")]
                let bytes: Result<Vec<u8>, String> = Ok(file.read().await);
                bytes.map(|bytes| Some((file.file_name(), bytes)))
            }
        };
        let _ = sender.send(selection);
    };
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(task);
    #[cfg(not(target_arch = "wasm32"))]
    bevy::tasks::AsyncComputeTaskPool::get()
        .spawn(task)
        .detach();
}

#[cfg(target_os = "android")]
static ANDROID_SELECTION: Mutex<Option<mpsc::Sender<Selection>>> = Mutex::new(None);

#[cfg(target_os = "android")]
fn open_picker(sender: mpsc::Sender<Selection>) {
    *ANDROID_SELECTION.lock().unwrap() = Some(sender);
    let result = (|| -> Result<(), String> {
        let app = bevy::android::ANDROID_APP
            .get()
            .ok_or("Android activity unavailable")?;

        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }
            .map_err(|error| error.to_string())?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|error| error.to_string())?;
        let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };
        env.call_method(&activity, "pickMrcFile", "()V", &[])
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if let Err(error) = result {
        complete_android_selection(Err(error));
    }
}

#[cfg(target_os = "android")]
pub fn complete_android_selection(result: Result<Option<(String, Vec<u8>)>, String>) {
    if let Some(sender) = ANDROID_SELECTION.lock().unwrap().take() {
        let _ = sender.send(result);
    }
}

fn load_selection(
    selection: Selection,
    playback: &mut MrcPlayback,
    player: &mut ReplayPlayer,
    request: &mut LaunchRequest,
) -> Result<bool, String> {
    let Some((name, bytes)) = selection? else {
        return Ok(false);
    };
    if !name.to_ascii_lowercase().ends_with(".mrc") {
        return Err("请选择 .mrc 格式的录像文件".into());
    }
    playback.load_bytes(name, &bytes)?;
    player.active = false;
    request.mode = "replay".into();
    request.spawn_room_npcs = false;
    if let Some(stage_id) = playback.stage_id() {
        let stage = meteor_scene::stage_registry::get_stage(&stage_id, &[]);
        request.stage_id = stage.id;
        request.stage_name = stage.name;
    }
    if let Some((weapon, _)) = playback.local_weapon() {
        request.weapon = weapon;
    }
    if let Some(character_id) = playback.local_character() {
        request.character_id = character_id;
    }
    Ok(true)
}

pub(crate) fn receive_mrc_file(
    mut picker: ResMut<MrcFilePicker>,
    mut playback: ResMut<MrcPlayback>,
    mut player: ResMut<ReplayPlayer>,
    mut request: ResMut<LaunchRequest>,
    state: Res<State<AppState>>,
    screen: Res<MenuScreen>,
    mut next_state: ResMut<NextState<AppState>>,
    mut labels: Query<&mut Text, With<MrcFileStatus>>,
) {
    let result =
        picker
            .pending
            .as_ref()
            .and_then(|receiver| match receiver.lock().unwrap().try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("文件选择器已关闭，请重试".into()))
                }
            });
    if let Some(result) = result {
        picker.pending = None;
        picker.status.clear();

        if *state.get() == AppState::Menu && *screen == MenuScreen::Replay {
            match load_selection(result, &mut playback, &mut player, &mut request) {
                Ok(true) => next_state.set(AppState::Loading),
                Ok(false) => {}
                Err(error) => picker.status = format!("无法播放录像：{error}"),
            }
        }
    }
    for mut text in &mut labels {
        if text.0 != picker.status {
            text.0.clone_from(&picker.status);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording() -> Vec<u8> {
        use meteor_asset::mrc::*;
        encode_mrc(&MrcDocument {
            magic: MRC_MAGIC,
            header_size: MRC_HEADER_SIZE,
            mark0: 0,
            mark1: 0,
            tail: 0,
            state: [0; MRC_STATE_SIZE],
            meta: [0; 8],
            queue_a: vec![],
            queue_b: vec![],
            queue_c: vec![],
            queue_d: vec![],
            queue_e: vec![],
        })
    }

    fn app_with_result(result: Selection) -> App {
        let mut app = App::new();
        let (sender, receiver) = mpsc::channel();
        sender.send(result).unwrap();
        app.insert_resource(MrcFilePicker {
            pending: Some(Mutex::new(receiver)),
            status: String::new(),
        })
        .insert_resource(State::new(AppState::Menu))
        .insert_resource(MenuScreen::Replay)
        .init_resource::<NextState<AppState>>()
        .init_resource::<LaunchRequest>()
        .init_resource::<MrcPlayback>()
        .init_resource::<ReplayPlayer>()
        .add_systems(Update, receive_mrc_file);
        app.world_mut().spawn((Text::new(""), MrcFileStatus));
        app
    }

    #[test]
    fn selected_bytes_launch_replay_without_a_filesystem_path() {
        let mut app = app_with_result(Ok(Some(("用户录像.MRC".into(), recording()))));
        app.world_mut().resource_mut::<ReplayPlayer>().active = true;
        app.update();
        let playback = app.world().resource::<MrcPlayback>();
        assert!(playback.active);
        assert_eq!(playback.file_name, "用户录像.MRC");
        assert!(!app.world().resource::<ReplayPlayer>().active);
        let request = app.world().resource::<LaunchRequest>();
        assert_eq!(request.mode, "replay");
        assert!(!request.spawn_room_npcs);
        assert!(matches!(
            app.world().resource::<NextState<AppState>>(),
            NextState::Pending(AppState::Loading)
        ));
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn selected_original_spear_loads_its_recorded_local_character() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../mrcs/tests/长枪三连.mrc");
        let bytes = std::fs::read(path).expect("original spear recording");
        let mut playback = MrcPlayback::default();
        let mut player = ReplayPlayer::default();
        let mut request = LaunchRequest {
            character_id: 7,
            ..default()
        };
        assert!(load_selection(
            Ok(Some(("长枪三连.mrc".into(), bytes))),
            &mut playback,
            &mut player,
            &mut request,
        )
        .unwrap());
        assert_eq!(playback.local_slot(), Some(2));
        assert_eq!(playback.local_character(), Some(0));
        assert_eq!((request.character_id, request.weapon), (0, 7));
    }

    #[test]
    fn cancelled_or_invalid_selection_stays_in_menu_and_allows_retry() {
        for result in [
            Ok(None),
            Ok(Some(("bad.mrc".into(), vec![1, 2]))),
            Ok(Some(("bad.txt".into(), recording()))),
            Err("read failed".into()),
        ] {
            let cancelled = matches!(result, Ok(None));
            let mut app = app_with_result(result);
            let mode = app.world().resource::<LaunchRequest>().mode.clone();
            app.update();
            assert!(!app.world().resource::<MrcPlayback>().active);
            assert_eq!(app.world().resource::<LaunchRequest>().mode, mode);
            assert!(matches!(
                app.world().resource::<NextState<AppState>>(),
                NextState::Unchanged
            ));
            let picker = app.world().resource::<MrcFilePicker>();
            assert!(picker.pending.is_none());
            assert_eq!(picker.status.is_empty(), cancelled);
            let status = picker.status.clone();
            assert_eq!(
                app.world_mut()
                    .query::<&Text>()
                    .single(app.world())
                    .unwrap()
                    .0,
                status
            );
        }
    }

    #[test]
    fn late_selection_does_not_launch_after_leaving_replay_menu() {
        let mut app = app_with_result(Ok(Some(("test.mrc".into(), recording()))));
        app.insert_resource(MenuScreen::Main);
        app.update();
        assert!(!app.world().resource::<MrcPlayback>().active);
        assert!(app.world().resource::<MrcFilePicker>().pending.is_none());
    }
}
