use super::*;
use crate::plugins::native_text_input::{capture_editor_keys, EditorKeys, NativeTextDialog};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/plugins/bevy_text_web.mjs")]
extern "C" {
    #[wasm_bindgen(js_name = syncTextEditor)]
    fn sync_text_editor(state: &str);
    #[wasm_bindgen(js_name = readTextEditor)]
    fn read_text_editor() -> String;
    #[wasm_bindgen(js_name = closeTextEditor)]
    fn close_text_editor();
}

#[derive(Resource, Default)]
struct Bridge {
    editor: Option<Entity>,
    last: ImeState,
}

#[derive(serde::Deserialize)]
struct Reply {
    state: ImeState,
    confirm: bool,
    cancel: bool,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<Bridge>()
        .add_systems(PreUpdate, receive.after(capture_editor_keys))
        .add_systems(
            PostUpdate,
            publish
                .after(bevy::text::EditableTextSystems)
                .after(bevy::input_focus::InputFocusSystems::FocusChangeEvents),
        );
}

fn receive(
    dialog: Res<NativeTextDialog>,
    mut bridge: ResMut<Bridge>,
    mut input: ResMut<EditorKeys>,
    mut editors: Query<&mut EditableText>,
    mut fonts: ResMut<FontCx>,
    mut layout: ResMut<LayoutCx>,
) {
    let Some(pending) = dialog.pending.as_ref() else {
        return;
    };
    if bridge.editor != Some(pending.editor) {
        return;
    }
    let Ok(reply) = serde_json::from_str::<Reply>(&read_text_editor()) else {
        return;
    };
    input.confirm |= reply.confirm;
    input.cancel |= reply.cancel;
    if reply.state != bridge.last {
        if let Ok(mut editor) = editors.get_mut(pending.editor) {
            apply_ime(&mut editor, &reply.state, &mut fonts, &mut layout);
            bridge.last = reply.state;
        }
    }
}

fn publish(
    dialog: Res<NativeTextDialog>,
    mut bridge: ResMut<Bridge>,
    editors: Query<&EditableText>,
) {
    let Some(pending) = dialog.pending.as_ref() else {
        if bridge.editor.take().is_some() {
            close_text_editor();
        }
        return;
    };
    let Ok(editor) = editors.get(pending.editor) else {
        return;
    };
    let state = read_editor(editor);
    if bridge.editor != Some(pending.editor) || state != bridge.last {
        sync_text_editor(&serde_json::to_string(&state).expect("text state is serializable"));
        bridge.editor = Some(pending.editor);
        bridge.last = state;
    }
}
