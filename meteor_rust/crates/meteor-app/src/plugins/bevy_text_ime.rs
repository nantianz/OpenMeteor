#[cfg(any(target_os = "android", target_arch = "wasm32"))]
use bevy::prelude::*;
use bevy::text::{EditableText, FontCx, LayoutCx};

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ImeState {
    text: String,
    selection: (usize, usize),
    compose: Option<(usize, usize)>,
}

fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units + ch.len_utf16() > utf16 {
            return byte;
        }
        units += ch.len_utf16();
    }
    text.len()
}

fn read_editor(editor: &EditableText) -> ImeState {
    let text = editor.editor.raw_text();
    let offset = |byte: usize| text[..byte].encode_utf16().count();
    let selection = editor.editor.raw_selection();
    ImeState {
        text: text.into(),
        selection: (
            offset(selection.anchor().index()),
            offset(selection.focus().index()),
        ),
        compose: editor
            .editor
            .raw_compose()
            .as_ref()
            .map(|r| (offset(r.start), offset(r.end))),
    }
}

fn apply_ime(
    editor: &mut EditableText,
    state: &ImeState,
    fonts: &mut FontCx,
    layout: &mut LayoutCx,
) {
    let was_composing = editor.is_composing();

    editor.pending_edits.clear();
    editor.pending_paste = None;

    let text = if state.compose.is_none() {
        state
            .text
            .chars()
            .filter(|ch| !ch.is_control())
            .take(editor.max_characters.unwrap_or(usize::MAX))
            .collect::<String>()
    } else {
        state.text.clone()
    };
    editor.editor.set_text(&text);
    let mut driver = editor.editor.driver(fonts, layout);
    driver.select_byte_range(
        byte_offset(&text, state.selection.0),
        byte_offset(&text, state.selection.1),
    );
    if let Some((start, end)) = state.compose.filter(|(start, end)| start != end) {
        driver.set_compose_byte_range(
            byte_offset(&state.text, start),
            byte_offset(&state.text, end),
        );
    }
    if was_composing && state.compose.is_none() {
        editor.queue_edit(bevy::text::TextEdit::clear_ime_compose());
    }
}

#[cfg(target_os = "android")]
mod platform {
    use super::*;
    use crate::plugins::native_text_input::{capture_editor_keys, NativeTextDialog};
    use bevy::android::android_activity::input::{
        ImeOptions, InputType, TextInputAction, TextInputState, TextSpan,
    };

    #[derive(Resource, Default)]
    struct Bridge {
        editor: Option<Entity>,
        last: ImeState,
    }

    pub(crate) fn install(app: &mut App) {
        app.init_resource::<Bridge>()
            .add_systems(
                PreUpdate,
                receive
                    .before(capture_editor_keys)
                    .after(bevy::input_focus::InputFocusSystems::Dispatch)
                    .after(bevy::ui_widgets::ImeSystems::HandleEvents),
            )
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
        let Some(app) = bevy::android::ANDROID_APP.get() else {
            return;
        };
        let state = app.text_input_state();
        let state = ImeState {
            text: state.text,
            selection: (state.selection.start, state.selection.end),
            compose: state.compose_region.map(|r| (r.start, r.end)),
        };
        if state != bridge.last {
            if let Ok(mut editor) = editors.get_mut(pending.editor) {
                apply_ime(&mut editor, &state, &mut fonts, &mut layout);
                bridge.last = state;
            }
        }
    }

    fn publish(
        dialog: Res<NativeTextDialog>,
        mut bridge: ResMut<Bridge>,
        editors: Query<&EditableText>,
    ) {
        let Some(pending) = dialog.pending.as_ref() else {
            bridge.editor = None;
            return;
        };
        let Some(app) = bevy::android::ANDROID_APP.get() else {
            return;
        };
        let Ok(editor) = editors.get(pending.editor) else {
            return;
        };
        let state = read_editor(editor);
        if bridge.editor != Some(pending.editor) {
            app.set_ime_editor_info(
                InputType::TYPE_CLASS_TEXT,
                TextInputAction::Done,
                ImeOptions::IME_FLAG_NO_FULLSCREEN | ImeOptions::IMG_FLAG_NO_EXTRACT_UI,
            );
        }
        if bridge.editor != Some(pending.editor) || state != bridge.last {
            app.set_text_input_state(TextInputState {
                text: state.text.clone(),
                selection: TextSpan {
                    start: state.selection.0,
                    end: state.selection.1,
                },
                compose_region: state.compose.map(|(start, end)| TextSpan { start, end }),
            });
            bridge.editor = Some(pending.editor);
            bridge.last = state;
        }
    }
}

#[cfg(target_os = "android")]
pub(super) use platform::install;

#[cfg(target_arch = "wasm32")]
#[path = "bevy_text_web.rs"]
mod web;
#[cfg(target_arch = "wasm32")]
pub(super) use web::install;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_text_platform_unicode_selection_and_preedit_round_trip() {
        let mut fonts = FontCx::default();
        let font = bevy::text::Font::from_bytes(bevy::text::DEFAULT_FONT_DATA.to_vec());
        let families = fonts.collection.register_fonts(font.data, None);
        let family = fonts
            .collection
            .family_name(families[0].0)
            .unwrap()
            .to_owned();
        fonts.set_sans_serif_family(&family).unwrap();
        let mut layout = LayoutCx::default();
        let mut editor = EditableText::default();
        let state = ImeState {
            text: "孟🦋xing魂".into(),
            selection: (5, 5),
            compose: Some((3, 7)),
        };
        apply_ime(&mut editor, &state, &mut fonts, &mut layout);
        assert!(editor.is_composing());
        assert_eq!(editor.value().to_string(), "孟🦋魂");
        assert_eq!(read_editor(&editor), state);
        let committed = ImeState {
            text: "孟🦋星魂".into(),
            selection: (4, 4),
            compose: None,
        };
        apply_ime(&mut editor, &committed, &mut fonts, &mut layout);
        assert!(!editor.is_composing());
        assert_eq!(read_editor(&editor), committed);
        assert_eq!(byte_offset("🦋", 1), 0);
    }
}
