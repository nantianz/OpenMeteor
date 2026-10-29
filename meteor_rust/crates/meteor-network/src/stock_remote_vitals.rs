use crate::stock_protocol::StockVitalsEvent;

pub const REQUEST_REMOTE_VITALS: u16 = 0x7f00;
pub const EVENT_REMOTE_VITALS: u16 = 0x7f01;
pub const REMOTE_VITALS_V1: &[u8; 4] = b"MRV1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteVitals {
    pub actor_id: u32,
    pub revision: u32,
    pub vitals: StockVitalsEvent,
}

impl RemoteVitals {
    pub fn encode(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(18);
        bytes.extend_from_slice(REMOTE_VITALS_V1);
        bytes.extend_from_slice(&self.actor_id.to_le_bytes());
        bytes.extend_from_slice(&self.revision.to_le_bytes());
        bytes.extend_from_slice(&self.vitals.encode());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 18 || &bytes[..4] != REMOTE_VITALS_V1 {
            return None;
        }
        Some(Self {
            actor_id: u32::from_le_bytes(bytes[4..8].try_into().ok()?),
            revision: u32::from_le_bytes(bytes[8..12].try_into().ok()?),
            vitals: StockVitalsEvent::decode(&bytes[12..]).ok()?,
        })
    }

    pub fn newer_than(self, previous: u32) -> bool {
        let delta = self.revision.wrapping_sub(previous);
        delta != 0 && delta < (1 << 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_and_revision_wrap_reject_malformed_or_stale_samples() {
        let sample = RemoteVitals {
            actor_id: 7,
            revision: 0,
            vitals: StockVitalsEvent {
                health: 700,
                max_health: 1500,
                anger: 42,
            },
        };
        let bytes = sample.encode();
        assert_eq!(RemoteVitals::decode(&bytes), Some(sample));
        for len in 0..bytes.len() {
            assert_eq!(RemoteVitals::decode(&bytes[..len]), None);
        }
        let mut invalid = bytes.clone();
        invalid[3] = b'2';
        assert_eq!(RemoteVitals::decode(&invalid), None);
        invalid = bytes;
        invalid.push(0);
        assert_eq!(RemoteVitals::decode(&invalid), None);
        assert!(sample.newer_than(u32::MAX));
        assert!(!sample.newer_than(0));
        assert!(!sample.newer_than(1));
    }
}
