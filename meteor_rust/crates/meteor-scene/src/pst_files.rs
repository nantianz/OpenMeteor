use std::collections::BTreeMap;

use crate::pst_game::ScriptStorage;
use crate::pst_runtime::{value_text, PstValue};
use PstValue::{Num, Str};

const MAX_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(crate) struct ScriptFiles {
    next_id: i64,
    handles: BTreeMap<i64, ScriptFile>,
}

struct ScriptFile {
    slot: Option<String>,
    content: String,
    cursor: usize,
}

fn slot(path: &str) -> Option<&str> {
    let path = path.strip_prefix("UserData/Saves/").unwrap_or(path);
    let name = path.strip_suffix(".sav").unwrap_or(path);
    (!name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    .then_some(name)
}

fn asset_path(path: &str) -> bool {
    path.len() <= 240
        && !path.is_empty()
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|character| !character.is_control() && !"\\:".contains(character))
        })
}

impl ScriptFiles {
    pub(crate) fn call(
        &mut self,
        storage: Option<ScriptStorage>,
        name: &str,
        args: &[PstValue],
    ) -> Option<PstValue> {
        let result = match (name, args) {
            ("OpenFile", [Str(path), Str(mode)]) => {
                let Some(storage) = storage else {
                    return Some(Num(-1.0));
                };
                if self.handles.len() >= 16 {
                    return Some(Num(-1.0));
                }
                let path = path.replace('\\', "/");
                let save = slot(&path);
                let content = match (mode.as_str(), save) {
                    ("r", Some(slot)) => (storage.read)(slot),
                    ("r", _) if asset_path(&path) => (storage.read_asset)(&path)
                        .filter(|bytes| bytes.len() <= MAX_BYTES)
                        .map(|bytes| crate::text::decode_cp936(&bytes)),
                    ("w", Some(_)) => Some(String::new()),
                    ("a", Some(slot)) => Some((storage.read)(slot).unwrap_or_default()),
                    _ => None,
                };
                let Some(content) = content.filter(|text| text.len() <= MAX_BYTES) else {
                    return Some(Num(-1.0));
                };
                if mode != "r"
                    && self
                        .handles
                        .values()
                        .any(|file| file.slot.as_deref() == save)
                {
                    return Some(Num(-1.0));
                }
                self.next_id += 1;
                self.handles.insert(
                    self.next_id,
                    ScriptFile {
                        slot: save.filter(|_| mode != "r").map(str::to_owned),
                        content,
                        cursor: 0,
                    },
                );
                PstValue::from(self.next_id)
            }
            ("ReadFile" | "ReadFileLine", [Num(id)]) if id.fract() == 0.0 => {
                let Some(file) = self.handles.get_mut(&(*id as i64)) else {
                    return Some(Num(-1.0));
                };
                let Some(remaining) = file
                    .content
                    .get(file.cursor..)
                    .filter(|line| !line.is_empty())
                else {
                    return Some(Num(-1.0));
                };
                let (line, consumed) = remaining
                    .split_once('\n')
                    .map_or((remaining, remaining.len()), |(line, _)| {
                        (line, line.len() + 1)
                    });
                file.cursor += consumed;
                if name == "ReadFile" {
                    line.trim()
                        .parse::<i32>()
                        .map_or(Num(-1.0), |value| Num(f64::from(value)))
                } else {
                    PstValue::from(line.trim_end_matches('\r'))
                }
            }
            ("WriteFile" | "WriteFileLine", [Num(id), value]) if id.fract() == 0.0 => {
                let Some(file) = self
                    .handles
                    .get_mut(&(*id as i64))
                    .filter(|file| file.slot.is_some())
                else {
                    return Some(Num(0.0));
                };
                let line = if name == "WriteFile" {
                    (value.as_f64() as i32).to_string()
                } else {
                    value_text(value)
                };
                if file
                    .content
                    .len()
                    .saturating_add(line.len())
                    .saturating_add(1)
                    > MAX_BYTES
                {
                    return Some(Num(0.0));
                }
                file.content.push_str(&line);
                file.content.push('\n');
                Num(1.0)
            }
            ("CloseFile", [Num(id)]) if id.fract() == 0.0 => {
                let Some(file) = self.handles.get(&(*id as i64)) else {
                    return Some(Num(0.0));
                };
                if let Some(slot) = &file.slot {
                    if !storage.is_some_and(|storage| (storage.write)(slot, &file.content)) {
                        return Some(Num(0.0));
                    }
                }
                self.handles.remove(&(*id as i64));
                Num(1.0)
            }
            (
                "OpenFile" | "ReadFile" | "ReadFileLine" | "WriteFile" | "WriteFileLine"
                | "CloseFile",
                _,
            ) => Num(-1.0),
            _ => return None,
        };
        Some(result)
    }
}
