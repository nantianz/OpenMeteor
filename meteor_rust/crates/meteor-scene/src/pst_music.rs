use crate::pst_runtime::PstValue;
use std::sync::{Arc, Mutex};

pub struct PstMusicTrack {
    pub path: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone)]
pub struct PstMusicRequests {
    state: Arc<Mutex<MusicState>>,
    load: fn(&str) -> Option<Vec<u8>>,
}

#[derive(Default)]
struct MusicState {
    selected_path: Option<String>,
    pending: Option<PstMusicTrack>,
}

impl PstMusicRequests {
    pub fn new(load: fn(&str) -> Option<Vec<u8>>) -> Self {
        Self {
            state: Arc::new(Mutex::new(MusicState::default())),
            load,
        }
    }

    pub fn take(&self) -> Option<PstMusicTrack> {
        self.state.lock().ok()?.pending.take()
    }

    pub(crate) fn play(&self, args: &[PstValue]) -> bool {
        let Some(PstValue::Str(input)) = args.first() else {
            return false;
        };
        let path = input.replace('\\', "/").to_lowercase();
        let Some(track) = path.strip_prefix("music/") else {
            return false;
        };
        if !track.ends_with(".mp3")
            || track.contains([':', '\0'])
            || track.split('/').any(|part| matches!(part, "" | "." | ".."))
        {
            return false;
        }
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.selected_path.as_deref() == Some(&path) {
            return true;
        }
        let Some(bytes) = (self.load)(&path).filter(|bytes| !bytes.is_empty()) else {
            return false;
        };
        state.selected_path = Some(path.clone());
        state.pending = Some(PstMusicTrack { path, bytes });
        true
    }
}
