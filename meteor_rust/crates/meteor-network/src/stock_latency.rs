pub const REQUEST: u16 = 0x7f04;
pub const EVENT: u16 = 0x7f05;
pub const MAGIC: &[u8; 4] = b"MLT1";

pub fn encode(samples: &[(u32, u32)]) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    for (actor, ms) in samples {
        bytes.extend_from_slice(&actor.to_le_bytes());
        bytes.extend_from_slice(&ms.to_le_bytes());
    }
    bytes
}

pub fn decode(bytes: &[u8]) -> Option<Vec<(u32, u32)>> {
    let body = bytes.strip_prefix(MAGIC)?;
    if body.len() % 8 != 0 || body.len() > 32 * 8 {
        return None;
    }
    body.chunks_exact(8)
        .map(|row| {
            let actor = u32::from_le_bytes(row[..4].try_into().ok()?);
            let ms = u32::from_le_bytes(row[4..].try_into().ok()?);
            (ms <= 60_000).then_some((actor, ms))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_rejects_truncation_unknown_version_and_invalid_rtt() {
        let bytes = encode(&[(7, 25), (8, 0)]);
        assert_eq!(decode(&bytes), Some(vec![(7, 25), (8, 0)]));
        assert!(decode(&bytes[..bytes.len() - 1]).is_none());
        assert!(decode(b"MLT2").is_none());
        assert!(decode(&encode(&[(7, 60_001)])).is_none());
    }
}
