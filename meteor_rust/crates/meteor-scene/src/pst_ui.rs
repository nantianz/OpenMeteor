use std::sync::{Arc, Mutex};

use crate::pst_runtime::PstValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiOwner {
    Scene,
    Mission,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiButton {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiPanel {
    pub generation: u64,
    pub owner: UiOwner,
    pub id: String,
    pub title: String,
    pub text: Vec<String>,
    pub buttons: Vec<UiButton>,
}

#[derive(Default)]
struct PanelState {
    panels: Vec<UiPanel>,
    shown: Option<(UiOwner, String)>,
    generation: u64,
}

#[derive(Clone, Default)]
pub struct PstUi(Arc<Mutex<PanelState>>);

impl PstUi {
    pub fn visible(&self) -> Option<UiPanel> {
        let state = self.0.lock().expect("PST UI state");
        let (owner, id) = state.shown.as_ref()?;
        state
            .panels
            .iter()
            .find(|p| p.owner == *owner && p.id == *id)
            .cloned()
    }

    pub fn has_modal(&self) -> bool {
        let state = self.0.lock().expect("PST UI state");
        state.shown.as_ref().is_some_and(|(owner, id)| {
            state
                .panels
                .iter()
                .any(|p| p.owner == *owner && p.id == *id && !p.buttons.is_empty())
        })
    }

    pub(crate) fn builtin(
        &self,
        owner: UiOwner,
        name: &str,
        args: &[PstValue],
    ) -> Option<PstValue> {
        if !matches!(
            name,
            "UiCreatePanel" | "UiAddText" | "UiAddButton" | "UiShowPanel" | "UiClosePanel"
        ) {
            return None;
        }
        let accepted = self.apply(owner, name, args);
        Some(PstValue::Num(if accepted { 1.0 } else { 0.0 }))
    }

    fn apply(&self, owner: UiOwner, name: &str, args: &[PstValue]) -> bool {
        use PstValue::Str;
        let [Str(id), rest @ ..] = args else {
            return false;
        };
        if !valid_id(id) {
            return false;
        }
        let mut state = self.0.lock().expect("PST UI state");
        let panel = state
            .panels
            .iter()
            .position(|panel| panel.owner == owner && panel.id == *id);
        match (name, rest, panel) {
            ("UiCreatePanel", [Str(title)], None)
                if state.panels.len() < 16 && valid_text(title, 64) =>
            {
                state.generation += 1;
                let generation = state.generation;
                state.panels.push(UiPanel {
                    generation,
                    owner,
                    id: id.clone(),
                    title: title.clone(),
                    text: vec![],
                    buttons: vec![],
                });
            }
            ("UiAddText", [Str(text)], Some(index))
                if state.panels[index].text.len() < 6 && valid_text(text, 256) =>
            {
                state.panels[index].text.push(text.clone());
            }
            ("UiAddButton", [Str(button), Str(label)], Some(index))
                if valid_id(button) && valid_text(label, 40) =>
            {
                let buttons = &mut state.panels[index].buttons;
                if buttons.len() >= 6 || buttons.iter().any(|entry| entry.id == *button) {
                    return false;
                }
                buttons.push(UiButton {
                    id: button.clone(),
                    label: label.clone(),
                });
            }
            ("UiShowPanel", [], Some(_)) => state.shown = Some((owner, id.clone())),
            ("UiClosePanel", [], Some(index)) => {
                state.panels.remove(index);
                if state
                    .shown
                    .as_ref()
                    .is_some_and(|(shown_owner, shown_id)| *shown_owner == owner && shown_id == id)
                {
                    state.shown = None;
                }
            }
            _ => return false,
        }
        true
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

fn valid_text(text: &str, limit: usize) -> bool {
    !text.trim().is_empty()
        && text.chars().take(limit + 1).count() <= limit
        && text.chars().all(|c| c == '\n' || !c.is_control())
}
