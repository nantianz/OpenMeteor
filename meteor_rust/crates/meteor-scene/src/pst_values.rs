use crate::pst_runtime::{value_text, PstValue, ARRAY_LIMIT};
use PstValue::{Array, Num, Str};

fn index(value: f64) -> Option<usize> {
    (value.is_finite() && value >= 0.0 && value.fract() == 0.0).then_some(value as usize)
}

pub(crate) fn call(name: &str, args: &[PstValue]) -> Option<PstValue> {
    Some(match (name, args) {
        ("Array", values) => Array(values.to_vec()),
        ("ArraySize", [Array(values)]) => Num(values.len() as f64),
        ("ArrayPush", [Array(values), value]) if values.len() < ARRAY_LIMIT => {
            let mut result = values.clone();
            result.push(value.clone());
            Array(result)
        }
        ("ArrayErase", [Array(values), Num(at)]) => {
            let mut result = values.clone();
            if let Some(at) = index(*at).filter(|&at| at < result.len()) {
                result.remove(at);
            }
            Array(result)
        }
        ("ArrayResize", [Array(values), Num(size)]) => {
            let mut result = values.clone();
            if let Some(size) = index(*size).filter(|&size| size <= ARRAY_LIMIT) {
                result.resize(size, Num(0.0));
            }
            Array(result)
        }
        ("ArrayFind", [Array(values), needle]) => Num(values
            .iter()
            .position(|value| value == needle)
            .map_or(-1.0, |at| at as f64)),
        ("Map", []) => Array(vec![]),
        ("MapGet", [Array(entries), Str(key)]) => {
            map_entry(entries, key).map_or(Num(0.0), |(_, value)| value.clone())
        }
        ("MapHas", [Array(entries), Str(key)]) => {
            Num(f64::from(u8::from(map_entry(entries, key).is_some())))
        }
        ("MapSet", [Array(entries), Str(key), value]) => {
            let mut result = entries.clone();
            let entry = Array(vec![Str(key.clone()), value.clone()]);
            if let Some((at, _)) = map_entry(entries, key) {
                result[at] = entry;
            } else if entries.len() < ARRAY_LIMIT {
                result.push(entry);
            }
            Array(result)
        }
        ("MapErase", [Array(entries), Str(key)]) => {
            let mut result = entries.clone();
            if let Some((at, _)) = map_entry(entries, key) {
                result.remove(at);
            }
            Array(result)
        }
        ("MapKeys", [Array(entries)]) => Array(
            entries
                .iter()
                .filter_map(|entry| match entry {
                    Array(pair) => pair.first().cloned(),
                    _ => None,
                })
                .collect(),
        ),
        ("StringLength", [Str(text)]) => Num(text.chars().count() as f64),
        ("StringLower", [Str(text)]) => Str(text.to_lowercase()),
        ("StringUpper", [Str(text)]) => Str(text.to_uppercase()),
        ("StringTrim", [Str(text)]) => Str(text.trim().to_owned()),
        ("StringFind", [Str(text), Str(needle)]) => Num(text
            .find(needle)
            .map_or(-1.0, |at| text[..at].chars().count() as f64)),
        ("StringSlice", [Str(text), Num(start), Num(len)]) => Str(text
            .chars()
            .skip(index(*start).unwrap_or(usize::MAX))
            .take(index(*len).unwrap_or_default())
            .collect()),
        ("StringSplit", [Str(text), Str(separator)]) if !separator.is_empty() => Array(
            text.split(separator)
                .take(ARRAY_LIMIT)
                .map(PstValue::from)
                .collect(),
        ),
        ("StringJoin", [Array(values), Str(separator)]) => {
            let values = values.iter().map(value_text).collect::<Vec<_>>();
            let size: usize = values.iter().map(String::len).sum();
            if size.saturating_add(
                separator
                    .len()
                    .saturating_mul(values.len().saturating_sub(1)),
            ) <= 1024 * 1024
            {
                Str(values.join(separator))
            } else {
                Str(String::new())
            }
        }
        ("ToInt", [value]) => Num(value.as_f64().trunc()),
        ("ToFloat", [value]) => Num(value.as_f64()),
        ("ToString", [value]) => Str(value_text(value)),
        ("FloatDivide", [left, right]) => {
            let result = left.as_f64() / right.as_f64();
            Num(if result.is_finite() { result } else { 0.0 })
        }
        ("Vector2", [Num(_), Num(_)])
        | ("Vector3", [Num(_), Num(_), Num(_)])
        | ("Vector4" | "Color", [Num(_), Num(_), Num(_), Num(_)]) => Array(args.to_vec()),
        _ => return None,
    })
}

fn map_entry<'a>(entries: &'a [PstValue], key: &str) -> Option<(usize, &'a PstValue)> {
    entries
        .iter()
        .enumerate()
        .find_map(|(at, entry)| match entry {
            Array(pair) => match pair.as_slice() {
                [Str(entry_key), value] if entry_key == key => Some((at, value)),
                _ => None,
            },
            _ => None,
        })
}
