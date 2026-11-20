use super::*;

pub const STOCK_REQUEST_CHAT: u16 = 15;
pub const STOCK_REQUEST_TEAM_CHAT: u16 = 16;
pub const STOCK_REQUEST_POSE_START: u16 = 25;
pub const STOCK_REQUEST_ENTRY_PREPARED: u16 = 27;
pub const STOCK_REQUEST_QUICK_CHAT: u16 = 38;
pub const STOCK_REQUEST_TARGET_CLEAR: u16 = 40;

pub const STOCK_EVENT_CHAT: u16 = 21;
pub const STOCK_EVENT_TEAM_CHAT: u16 = 22;
pub const STOCK_EVENT_FORCED_DEATH: u16 = 44;
pub const STOCK_EVENT_CARRIED_ITEM: u16 = 45;
pub const STOCK_EVENT_POSE_START: u16 = 50;
pub const STOCK_EVENT_ENTRY_PREPARED: u16 = 52;
pub const STOCK_EVENT_PLAYER_EXPELLED: u16 = 63;
pub const STOCK_EVENT_QUICK_CHAT: u16 = 73;
pub const STOCK_EVENT_TARGET_CLEAR: u16 = 76;
pub const STOCK_EVENT_PLAYER_RESTRICTED: u16 = 79;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockChatEvent {
    pub actor_id: u32,
    pub text: String,
}

impl StockChatEvent {
    const MAX_TEXT_BYTES: usize = 255;

    pub fn encode_request(text: &str) -> Result<Vec<u8>, StockProtocolError> {
        let (bytes, _, invalid) = encoding_rs::GBK.encode(text);
        if invalid || bytes.is_empty() || bytes.len() > Self::MAX_TEXT_BYTES || bytes.contains(&0) {
            return Err(StockProtocolError::InvalidStockMessage("chat"));
        }
        Ok(bytes.into_owned())
    }

    pub fn relay(actor_id: u32, request: &[u8]) -> Result<Vec<u8>, StockProtocolError> {
        let sender = u8::try_from(actor_id)
            .ok()
            .and_then(|id| id.checked_add(1))
            .ok_or(StockProtocolError::InvalidStockMessage("chat sender"))?;
        Self::decode_text(request)?;
        let mut payload = Vec::with_capacity(1 + request.len());
        payload.push(sender);
        payload.extend_from_slice(request);
        Ok(payload)
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        let (&sender, text) = payload
            .split_first()
            .ok_or(StockProtocolError::InvalidStockMessage("chat"))?;
        let actor = sender
            .checked_sub(1)
            .ok_or(StockProtocolError::InvalidStockMessage("chat sender"))?;
        Ok(Self {
            actor_id: u32::from(actor),
            text: Self::decode_text(text)?,
        })
    }

    fn decode_text(bytes: &[u8]) -> Result<String, StockProtocolError> {
        let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        if bytes.is_empty() || bytes.len() > Self::MAX_TEXT_BYTES || bytes.contains(&0) {
            return Err(StockProtocolError::InvalidStockMessage("chat"));
        }
        encoding_rs::GBK
            .decode_without_bom_handling_and_without_replacement(bytes)
            .map(|text| text.into_owned())
            .ok_or(StockProtocolError::InvalidStockMessage("GBK chat"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockQuickChat {
    pub category: u32,
    pub index: u32,
}

impl StockQuickChat {
    pub fn message_id(self) -> Option<u32> {
        let (first, count) = match self.category {
            36 => (1, 7),
            37 => (9, 8),
            38 => (18, 8),
            39 => (27, 9),
            _ => return None,
        };
        (self.index < count).then(|| first + self.index)
    }

    pub fn encode(self) -> [u8; 8] {
        let mut bytes = [0; 8];
        bytes[..4].copy_from_slice(&self.category.to_le_bytes());
        bytes[4..].copy_from_slice(&self.index.to_le_bytes());
        bytes
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, 8)?;
        let chat = Self {
            category: read_u32(payload, 0),
            index: read_u32(payload, 4),
        };
        chat.message_id()
            .ok_or(StockProtocolError::InvalidStockMessage("quick chat"))?;
        Ok(chat)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockQuickChatEvent {
    pub actor_id: u32,
    pub chat: StockQuickChat,
}

impl StockQuickChatEvent {
    pub fn encode(self) -> [u8; 12] {
        let mut payload = [0; 12];
        payload[..4].copy_from_slice(&self.actor_id.to_le_bytes());
        payload[4..].copy_from_slice(&self.chat.encode());
        payload
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, 12)?;
        Ok(Self {
            actor_id: read_u32(payload, 0),
            chat: StockQuickChat::decode(&payload[4..])?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockWeaponObjectEvent {
    pub object: StockWeaponObjectRequest,
    pub actor_id: u32,
}

impl StockWeaponObjectEvent {
    pub fn encode(self) -> [u8; 36] {
        let mut payload = [0; 36];
        payload[..32].copy_from_slice(&self.object.encode());
        payload[32..].copy_from_slice(&self.actor_id.to_le_bytes());
        payload
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, 36)?;
        Ok(Self {
            object: StockWeaponObjectRequest::decode(&payload[..32])?,
            actor_id: read_u32(payload, 32),
        })
    }

    pub fn position(self) -> RoleVec3 {
        let [x, z, y] = self.object.position_tenths.map(decode_scene_coordinate);
        RoleVec3 { x, y, z }
    }

    pub fn forward(self) -> RoleVec3 {
        let [x, z, y] = self.object.forward_tenths.map(|value| value as f32 * 0.1);
        RoleVec3 { x, y, z }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockCarriedItemEvent {
    pub actor_id: u32,
    pub category: u32,
    pub item_id: u32,
    pub scene_reference: u32,
}

impl StockCarriedItemEvent {
    pub fn encode(self) -> [u8; 16] {
        encode_four_u32([
            self.actor_id,
            self.category,
            self.item_id,
            self.scene_reference,
        ])
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        let [actor_id, category, item_id, scene_reference] = decode_four_u32(payload)?;
        Ok(Self {
            actor_id,
            category,
            item_id,
            scene_reference,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockTargetClearEvent {
    pub actor_id: u32,
    pub target_id: u32,
}

impl StockTargetClearEvent {
    pub fn encode(self) -> [u8; 8] {
        let mut payload = [0; 8];
        payload[..4].copy_from_slice(&self.actor_id.to_le_bytes());
        payload[4..].copy_from_slice(&self.target_id.to_le_bytes());
        payload
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, 8)?;
        Ok(Self {
            actor_id: read_u32(payload, 0),
            target_id: read_u32(payload, 4),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_weapon_object_matches_original_event_30_bytes() {
        let bytes = [5i32, 1, 139, -161, 66, 8, -4, 0, 3]
            .into_iter()
            .flat_map(i32::to_le_bytes)
            .collect::<Vec<_>>();
        let event = StockWeaponObjectEvent::decode(&bytes).unwrap();
        assert_eq!(event.actor_id, 3);
        assert_eq!(event.object.record_key, 5);
        assert_eq!(event.object.selector, 1);
        assert_eq!(event.encode().as_slice(), bytes);
        assert!((event.position().z + 1.932).abs() < 0.001);
        assert_eq!(
            event.forward(),
            RoleVec3 {
                x: 0.8,
                y: 0.0,
                z: -0.4
            }
        );
        assert!(StockWeaponObjectEvent::decode(&bytes[..35]).is_err());

        let request = StockWeaponObjectRequest::from_role_vectors(
            5,
            0,
            RoleVec3::ZERO,
            RoleVec3 {
                x: -1.0,
                y: 0.0,
                z: 0.8,
            },
        );
        assert_eq!(request.forward_tenths, [-10, 8, 0]);
    }

    #[test]
    fn stock_chat_prefixes_the_authenticated_sender_and_validates_text() {
        let request = StockChatEvent::encode_request("竹子").unwrap();
        let packet = StockChatEvent::relay(3, &request).unwrap();
        assert_eq!(packet[0], 4);
        assert_eq!(&packet[1..], request);
        assert_eq!(StockChatEvent::decode(&packet).unwrap().text, "竹子");
        assert!(StockChatEvent::decode(&[0, b'x']).is_err());
        assert!(StockChatEvent::encode_request("a\0b").is_err());
        assert!(StockChatEvent::encode_request("🙂").is_err());
    }
}
