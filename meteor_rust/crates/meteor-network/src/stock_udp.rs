#![cfg(not(target_arch = "wasm32"))]

use encoding_rs::GBK;
use std::collections::VecDeque;

pub const STOCK_PACKET_TYPE_CHALLENGE: u8 = 1;
pub const STOCK_PACKET_TYPE_CONNECT: u8 = 2;
pub const STOCK_PACKET_TYPE_CONNECTED: u8 = 3;
pub const STOCK_PACKET_TYPE_DISCONNECT: u8 = 4;
pub const STOCK_PACKET_TYPE_PING: u8 = 5;
pub const STOCK_PACKET_TYPE_PONG: u8 = 6;
pub const STOCK_PACKET_TYPE_DISCOVER: u8 = 7;
pub const STOCK_PACKET_TYPE_DISCOVER_REPLY: u8 = 8;
pub const STOCK_PACKET_TYPE_APPLICATION: u8 = 0x7f;
pub const STOCK_FLAG_RELIABLE: u8 = 0x01;
pub const STOCK_FLAG_ACK_DIRTY: u8 = 0x02;
pub const STOCK_FLAG_APPLICATION: u8 = 0x04;
pub const STOCK_FLAG_XOR_BODY: u8 = 0x10;
pub const STOCK_HEADER_BYTES: usize = 10;
pub const STOCK_APPLICATION_OPCODE_BYTES: usize = 2;
pub const STOCK_MAX_BODY_BYTES: usize = 1400;
pub const STOCK_MAX_DATAGRAM_BYTES: usize = STOCK_HEADER_BYTES + STOCK_MAX_BODY_BYTES;
pub const STOCK_SEQUENCE_MAX: u16 = 60_000;
pub const STOCK_RESEND_ELIGIBLE_MS: u64 = 300;
pub const STOCK_RESEND_SCAN_GATE_MS: u64 = 500;

pub const STOCK_UNRELIABLE_WRAP_TOLERANCE: i32 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockDatagram {
    pub packet_type: u8,
    pub flags: u8,
    pub sequence: u16,
    pub ack: u16,
    pub logical_channel: u16,
    pub body: Vec<u8>,
}

impl StockDatagram {
    pub fn encode(&self) -> Result<Vec<u8>, StockPacketError> {
        if self.body.len() > STOCK_MAX_BODY_BYTES {
            return Err(StockPacketError::TooLarge(self.body.len()));
        }
        let body_len = u16::try_from(self.body.len())
            .map_err(|_| StockPacketError::TooLarge(self.body.len()))?;
        let mut bytes = Vec::with_capacity(STOCK_HEADER_BYTES + self.body.len());
        bytes.push(self.packet_type);
        bytes.push(self.flags);
        bytes.extend_from_slice(&self.sequence.to_le_bytes());
        bytes.extend_from_slice(&self.ack.to_le_bytes());
        bytes.extend_from_slice(&self.logical_channel.to_le_bytes());
        bytes.extend_from_slice(&body_len.to_le_bytes());
        bytes.extend_from_slice(&self.body);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StockPacketError> {
        if bytes.len() < STOCK_HEADER_BYTES {
            return Err(StockPacketError::Truncated(bytes.len()));
        }
        let body_len = usize::from(u16::from_le_bytes([bytes[8], bytes[9]]));
        if body_len > STOCK_MAX_BODY_BYTES || bytes.len() != STOCK_HEADER_BYTES + body_len {
            return Err(StockPacketError::InvalidLength {
                declared: body_len,
                actual: bytes.len().saturating_sub(STOCK_HEADER_BYTES),
            });
        }
        Ok(Self {
            packet_type: bytes[0],
            flags: bytes[1],
            sequence: u16::from_le_bytes([bytes[2], bytes[3]]),
            ack: u16::from_le_bytes([bytes[4], bytes[5]]),
            logical_channel: u16::from_le_bytes([bytes[6], bytes[7]]),
            body: bytes[STOCK_HEADER_BYTES..].to_vec(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StockControlPacket {
    Challenge { cookie: u32 },

    Connect { cookie: u32, record: Vec<u8> },
    Connected { transport_id: u32 },
    Disconnect { reason: u32 },
    Ping { client_time: u32, latency: u32 },
    Pong { client_time: u32, server_time: u32 },
    Discover { sent_at: u32, query: String },

    DiscoverReply { sent_at: u32, info: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockDiscoveryInfo {
    pub fields_0_to_6: [u8; 7],
    pub port: u16,
    pub field_0e: u8,
    pub field_0f: u8,
    pub server_name: String,

    pub server_address: String,
    pub tail: Option<u8>,

    pub stage_id: Option<String>,
}

impl StockDiscoveryInfo {
    pub fn resolved_stage_id(&self) -> Option<String> {
        let number = self.fields_0_to_6[2];
        if number != 0 {
            Some(format!("sn{number:02}"))
        } else {
            self.stage_id.clone()
        }
    }

    pub fn max_players(&self) -> Option<u8> {
        let capacity = self.fields_0_to_6[4];
        (capacity != 0).then_some(capacity)
    }

    pub fn encode_room_summary(&self) -> Result<Vec<u8>, StockPacketError> {
        let base = self.encode()?;
        let stage = self.stage_id.as_deref().unwrap_or("");
        if stage.len() > 128 || stage.chars().any(char::is_control) {
            return Err(StockPacketError::InvalidDiscoveryInfo);
        }
        let mut bytes = b"MRD1".to_vec();

        bytes.extend_from_slice(format!("{:04x}", base.len()).as_bytes());
        bytes.extend_from_slice(&base);
        bytes.extend_from_slice(stage.as_bytes());
        Ok(bytes)
    }

    pub fn encode_unicode_room_summary(&self) -> Result<Vec<u8>, StockPacketError> {
        let mut bytes = b"MRD2".to_vec();
        let legacy = self.encode_room_summary()?;
        if self.server_name.len() > 768 || self.server_name.chars().any(char::is_control) {
            return Err(StockPacketError::InvalidDiscoveryInfo);
        }
        bytes.extend(
            serde_json::to_vec(&(legacy, &self.server_name))
                .map_err(|_| StockPacketError::InvalidDiscoveryInfo)?,
        );
        Ok(bytes)
    }

    pub fn decode_room_summary(bytes: &[u8]) -> Result<Self, StockPacketError> {
        if let Some(json) = bytes.strip_prefix(b"MRD2") {
            let (legacy, name): (Vec<u8>, String) =
                serde_json::from_slice(json).map_err(|_| StockPacketError::InvalidDiscoveryInfo)?;
            if !legacy.starts_with(b"MRD1")
                || name.len() > 768
                || name.chars().any(char::is_control)
            {
                return Err(StockPacketError::InvalidDiscoveryInfo);
            }
            let mut info = Self::decode_room_summary(&legacy)?;
            info.server_name = name;
            return Ok(info);
        }
        if !bytes.starts_with(b"MRD1") {
            return Self::decode(bytes);
        }
        if bytes.len() < 8 {
            return Err(StockPacketError::InvalidDiscoveryInfo);
        }
        let length = std::str::from_utf8(&bytes[4..8])
            .ok()
            .and_then(|value| usize::from_str_radix(value, 16).ok())
            .ok_or(StockPacketError::InvalidDiscoveryInfo)?;
        let end = 8 + length;
        let base = bytes
            .get(8..end)
            .ok_or(StockPacketError::InvalidDiscoveryInfo)?;
        let stage = std::str::from_utf8(&bytes[end..])
            .map_err(|_| StockPacketError::InvalidDiscoveryInfo)?;
        if stage.len() > 128 || stage.chars().any(char::is_control) {
            return Err(StockPacketError::InvalidDiscoveryInfo);
        }
        let mut info = Self::decode(base)?;
        info.stage_id = (!stage.is_empty()).then(|| stage.to_owned());
        Ok(info)
    }

    pub fn rule(&self) -> Option<i32> {
        let rule = i32::from(self.fields_0_to_6[5]);
        (1..=5).contains(&rule).then_some(rule)
    }
    pub fn default_room(
        port: u16,
        server_name: impl Into<String>,
        server_address: impl Into<String>,
    ) -> Self {
        Self {
            fields_0_to_6: [9, 7, 0, 0, 0, 0, 0],
            port,
            field_0e: 0,
            field_0f: 0,
            server_name: server_name.into(),
            server_address: server_address.into(),
            tail: Some(0),
            stage_id: None,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, StockPacketError> {
        let mut bytes = Vec::with_capacity(32 + self.server_name.len() + self.server_address.len());
        bytes.extend_from_slice(b"ME");
        for value in [
            self.fields_0_to_6[0],
            self.fields_0_to_6[1],
            self.fields_0_to_6[2],
            self.fields_0_to_6[5],
            self.fields_0_to_6[6],
            self.fields_0_to_6[3],
            self.fields_0_to_6[4],
            self.field_0e,
            self.field_0f,
        ] {
            bytes.push(discovery_increment(value)?);
        }
        append_discovery_segment(&mut bytes, self.port.to_string().as_bytes(), 5)?;
        append_discovery_name(&mut bytes, &self.server_name, 63)?;
        append_discovery_cp936(&mut bytes, &self.server_address, 19)?;
        if let Some(tail) = self.tail {
            bytes.push(discovery_increment(tail)?);
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StockPacketError> {
        if bytes.len() < 14 || !bytes.starts_with(b"ME") {
            return Err(StockPacketError::InvalidDiscoveryInfo);
        }
        let mut cursor = 2;
        let mut encoded_fields = [0u8; 9];
        for field in &mut encoded_fields {
            *field = discovery_decrement(
                *bytes
                    .get(cursor)
                    .ok_or(StockPacketError::InvalidDiscoveryInfo)?,
            )?;
            cursor += 1;
        }
        let port_bytes = take_discovery_segment(bytes, &mut cursor)?;
        let port = std::str::from_utf8(port_bytes)
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|value| *value != 0)
            .ok_or(StockPacketError::InvalidDiscoveryInfo)?;
        let server_name = decode_discovery_cp936(take_discovery_segment(bytes, &mut cursor)?)?;
        let server_address = decode_discovery_cp936(take_discovery_segment(bytes, &mut cursor)?)?;
        let tail = match bytes.get(cursor..) {
            Some([]) => None,
            Some([value]) => Some(discovery_decrement(*value)?),
            _ => return Err(StockPacketError::InvalidDiscoveryInfo),
        };
        Ok(Self {
            fields_0_to_6: [
                encoded_fields[0],
                encoded_fields[1],
                encoded_fields[2],
                encoded_fields[5],
                encoded_fields[6],
                encoded_fields[3],
                encoded_fields[4],
            ],
            port,
            field_0e: encoded_fields[7],
            field_0f: encoded_fields[8],
            server_name,
            server_address,
            tail,
            stage_id: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockConnectRecord {
    pub compatibility: [u8; 2],
    pub field2: u8,
    pub password: String,

    pub player_name: String,
    pub cdkey: String,
    pub field3: String,
    pub field4: String,

    pub data_checksum: i32,
}

pub const STOCK_CDKEY_BYPASS: &str = "Agent";

impl StockConnectRecord {
    pub fn new(password: impl Into<String>, data_checksum: i32) -> Self {
        Self {
            compatibility: [9, 7],
            field2: 0,
            password: password.into(),
            player_name: String::new(),
            cdkey: STOCK_CDKEY_BYPASS.to_string(),
            field3: String::new(),
            field4: String::new(),
            data_checksum,
        }
    }

    pub fn for_open_room() -> Self {
        Self::new("", 0)
    }

    pub fn encode(&self) -> Result<Vec<u8>, StockPacketError> {
        let mut bytes = Vec::with_capacity(32);
        for value in [self.compatibility[0], self.compatibility[1], self.field2] {
            bytes.push(discovery_increment(value)?);
        }
        append_discovery_cp936(&mut bytes, &self.password, 254)?;
        append_discovery_name(&mut bytes, &self.player_name, 254)?;
        for segment in [&self.cdkey, &self.field3, &self.field4] {
            append_discovery_cp936(&mut bytes, segment, 254)?;
        }
        append_discovery_segment(&mut bytes, self.data_checksum.to_string().as_bytes(), 254)?;
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StockPacketError> {
        if bytes.len() < 3 {
            return Err(StockPacketError::InvalidConnectRecord);
        }
        let compatibility = [
            discovery_decrement(bytes[0])?,
            discovery_decrement(bytes[1])?,
        ];
        let field2 = discovery_decrement(bytes[2])?;
        let mut cursor = 3;
        let mut segments = Vec::with_capacity(6);
        for _ in 0..6 {
            let segment = take_discovery_segment(bytes, &mut cursor)
                .map_err(|_| StockPacketError::InvalidConnectRecord)?;
            segments.push(decode_discovery_cp936(segment)?);
        }

        let data_checksum = segments[5].parse::<i32>().unwrap_or(0);
        Ok(Self {
            compatibility,
            field2,
            password: std::mem::take(&mut segments[0]),
            player_name: std::mem::take(&mut segments[1]),
            cdkey: std::mem::take(&mut segments[2]),
            field3: std::mem::take(&mut segments[3]),
            field4: std::mem::take(&mut segments[4]),
            data_checksum,
        })
    }
}

pub const STOCK_INTEGRITY_FILES: [&str; 6] = [
    r"common\character.act",
    r"PscriptLib\def.pst",
    r"PscriptLib\box.pst",
    r"PscriptLib\desk.pst",
    r"PscriptLib\chair.pst",
    r"PscriptLib\jug.pst",
];

pub fn stock_weighted_checksum(bytes: &[u8]) -> i32 {
    bytes.iter().enumerate().fold(0i32, |sum, (index, byte)| {
        let weight = i32::try_from(index + 1).unwrap_or(i32::MAX);
        sum.wrapping_add((*byte as i8 as i32).wrapping_mul(weight))
    })
}

pub fn stock_integrity_checksum(mut read: impl FnMut(&str) -> Option<Vec<u8>>) -> i32 {
    STOCK_INTEGRITY_FILES
        .iter()
        .filter_map(|path| read(path))
        .fold(0i32, |sum, bytes| {
            sum.wrapping_add(stock_weighted_checksum(&bytes))
        })
}

impl StockControlPacket {
    pub fn encode(&self, logical_channel: u16) -> Result<Vec<u8>, StockPacketError> {
        let (packet_type, mut flags, mut body) = match self {
            Self::Challenge { cookie } => (
                STOCK_PACKET_TYPE_CHALLENGE,
                0,
                cookie.to_le_bytes().to_vec(),
            ),
            Self::Connect { cookie, record } => {
                let mut body = cookie.to_le_bytes().to_vec();
                if *cookie != 0 {
                    append_stock_bytes(&mut body, record)?;
                    xor_body(&mut body, *cookie);
                }
                (
                    STOCK_PACKET_TYPE_CONNECT,
                    if *cookie == 0 { 0 } else { STOCK_FLAG_XOR_BODY },
                    body,
                )
            }
            Self::Connected { transport_id } => (
                STOCK_PACKET_TYPE_CONNECTED,
                0,
                transport_id.to_le_bytes().to_vec(),
            ),
            Self::Disconnect { reason } => (
                STOCK_PACKET_TYPE_DISCONNECT,
                0,
                reason.to_le_bytes().to_vec(),
            ),
            Self::Ping {
                client_time,
                latency,
            } => (
                STOCK_PACKET_TYPE_PING,
                0,
                two_u32_body(*client_time, *latency),
            ),
            Self::Pong {
                client_time,
                server_time,
            } => (
                STOCK_PACKET_TYPE_PONG,
                0,
                two_u32_body(*client_time, *server_time),
            ),
            Self::Discover { sent_at, query } => {
                let mut body = sent_at.to_le_bytes().to_vec();
                append_stock_string(&mut body, query)?;
                (STOCK_PACKET_TYPE_DISCOVER, 0, body)
            }
            Self::DiscoverReply { sent_at, info } => {
                let mut body = sent_at.to_le_bytes().to_vec();
                append_stock_bytes(&mut body, info)?;
                (STOCK_PACKET_TYPE_DISCOVER_REPLY, 0, body)
            }
        };
        if body.len() > STOCK_MAX_BODY_BYTES {
            return Err(StockPacketError::TooLarge(body.len()));
        }
        if packet_type != STOCK_PACKET_TYPE_CONNECT {
            flags &= !STOCK_FLAG_XOR_BODY;
        }
        StockDatagram {
            packet_type,
            flags,
            sequence: 0,
            ack: 0,
            logical_channel,
            body: std::mem::take(&mut body),
        }
        .encode()
    }

    pub fn decode(bytes: &[u8], xor_key: Option<u32>) -> Result<Self, StockPacketError> {
        let datagram = StockDatagram::decode(bytes)?;
        let mut body = datagram.body;
        if datagram.flags & STOCK_FLAG_XOR_BODY != 0 {
            let key = xor_key.ok_or(StockPacketError::MissingXorKey)?;
            xor_body(&mut body, key);
        }
        match datagram.packet_type {
            STOCK_PACKET_TYPE_CHALLENGE => Ok(Self::Challenge {
                cookie: read_single_u32(&body)?,
            }),
            STOCK_PACKET_TYPE_CONNECT => {
                let cookie = read_u32_at(&body, 0)?;
                let record = if cookie == 0 {
                    if body.len() != 4 {
                        return Err(StockPacketError::InvalidControlBody {
                            packet_type: datagram.packet_type,
                            length: body.len(),
                        });
                    }
                    Vec::new()
                } else {
                    read_stock_bytes(&body, 4)?
                };
                Ok(Self::Connect { cookie, record })
            }
            STOCK_PACKET_TYPE_CONNECTED => Ok(Self::Connected {
                transport_id: read_single_u32(&body)?,
            }),
            STOCK_PACKET_TYPE_DISCONNECT => Ok(Self::Disconnect {
                reason: read_single_u32(&body)?,
            }),
            STOCK_PACKET_TYPE_PING => Ok(Self::Ping {
                client_time: read_u32_pair(&body)?.0,
                latency: read_u32_pair(&body)?.1,
            }),
            STOCK_PACKET_TYPE_PONG => Ok(Self::Pong {
                client_time: read_u32_pair(&body)?.0,
                server_time: read_u32_pair(&body)?.1,
            }),
            STOCK_PACKET_TYPE_DISCOVER => Ok(Self::Discover {
                sent_at: read_u32_at(&body, 0)?,
                query: read_stock_string(&body, 4)?,
            }),
            STOCK_PACKET_TYPE_DISCOVER_REPLY => Ok(Self::DiscoverReply {
                sent_at: read_u32_at(&body, 0)?,
                info: read_stock_bytes(&body, 4)?,
            }),
            packet_type => Err(StockPacketError::UnsupportedControlType(packet_type)),
        }
    }
}

fn two_u32_body(first: u32, second: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&first.to_le_bytes());
    body.extend_from_slice(&second.to_le_bytes());
    body
}

fn read_single_u32(body: &[u8]) -> Result<u32, StockPacketError> {
    if body.len() != 4 {
        return Err(StockPacketError::InvalidControlBody {
            packet_type: 0,
            length: body.len(),
        });
    }
    read_u32_at(body, 0)
}

fn read_u32_pair(body: &[u8]) -> Result<(u32, u32), StockPacketError> {
    if body.len() != 8 {
        return Err(StockPacketError::InvalidControlBody {
            packet_type: 0,
            length: body.len(),
        });
    }
    Ok((read_u32_at(body, 0)?, read_u32_at(body, 4)?))
}

fn read_u32_at(body: &[u8], offset: usize) -> Result<u32, StockPacketError> {
    let bytes = body
        .get(offset..offset + 4)
        .ok_or(StockPacketError::InvalidControlBody {
            packet_type: 0,
            length: body.len(),
        })?;
    Ok(u32::from_le_bytes(
        bytes.try_into().expect("four-byte slice"),
    ))
}

fn append_stock_string(body: &mut Vec<u8>, value: &str) -> Result<(), StockPacketError> {
    if value.as_bytes().contains(&0) {
        return Err(StockPacketError::InteriorNul);
    }
    let (encoded, _, had_errors) = GBK.encode(value);
    if had_errors {
        return Err(StockPacketError::UnencodableString(value.to_string()));
    }
    if body.len() + encoded.len() + 1 > STOCK_MAX_BODY_BYTES {
        return Err(StockPacketError::TooLarge(body.len() + encoded.len() + 1));
    }
    body.extend_from_slice(&encoded);
    body.push(0);
    Ok(())
}

fn append_stock_bytes(body: &mut Vec<u8>, value: &[u8]) -> Result<(), StockPacketError> {
    if value.contains(&0) {
        return Err(StockPacketError::InteriorNul);
    }
    if body.len() + value.len() + 1 > STOCK_MAX_BODY_BYTES {
        return Err(StockPacketError::TooLarge(body.len() + value.len() + 1));
    }
    body.extend_from_slice(value);
    body.push(0);
    Ok(())
}

fn read_stock_string(body: &[u8], offset: usize) -> Result<String, StockPacketError> {
    let bytes = body
        .get(offset..)
        .ok_or(StockPacketError::InvalidControlBody {
            packet_type: 0,
            length: body.len(),
        })?;
    let nul = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(StockPacketError::UnterminatedString)?;
    if nul + 1 != bytes.len() {
        return Err(StockPacketError::InvalidControlBody {
            packet_type: 0,
            length: body.len(),
        });
    }
    let (decoded, _, had_errors) = GBK.decode(&bytes[..nul]);
    if had_errors {
        return Err(StockPacketError::InvalidStockString);
    }
    Ok(decoded.into_owned())
}

fn read_stock_bytes(body: &[u8], offset: usize) -> Result<Vec<u8>, StockPacketError> {
    let bytes = body
        .get(offset..)
        .ok_or(StockPacketError::InvalidControlBody {
            packet_type: 0,
            length: body.len(),
        })?;
    let nul = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(StockPacketError::UnterminatedString)?;
    if nul + 1 != bytes.len() {
        return Err(StockPacketError::InvalidControlBody {
            packet_type: 0,
            length: body.len(),
        });
    }
    Ok(bytes[..nul].to_vec())
}

fn discovery_increment(value: u8) -> Result<u8, StockPacketError> {
    value
        .checked_add(1)
        .ok_or(StockPacketError::InvalidDiscoveryInfo)
}

fn discovery_decrement(value: u8) -> Result<u8, StockPacketError> {
    value
        .checked_sub(1)
        .ok_or(StockPacketError::InvalidDiscoveryInfo)
}

fn append_discovery_segment(
    output: &mut Vec<u8>,
    value: &[u8],
    max_len: usize,
) -> Result<(), StockPacketError> {
    if value.len() > max_len || value.len() >= u8::MAX as usize {
        return Err(StockPacketError::DiscoveryFieldTooLong(value.len()));
    }
    output.push((value.len() as u8) + 1);
    output.extend_from_slice(value);
    Ok(())
}

fn append_discovery_name(
    output: &mut Vec<u8>,
    value: &str,
    max_len: usize,
) -> Result<(), StockPacketError> {
    let mut bytes = Vec::with_capacity(value.len());
    for character in value.chars() {
        let mut buffer = [0; 4];
        let (encoded, _, had_errors) = GBK.encode(character.encode_utf8(&mut buffer));
        if had_errors {
            bytes.push(b'?');
        } else {
            bytes.extend_from_slice(&encoded);
        }
    }
    append_discovery_segment(output, &bytes, max_len)
}

fn append_discovery_cp936(
    output: &mut Vec<u8>,
    value: &str,
    max_len: usize,
) -> Result<(), StockPacketError> {
    let (encoded, _, had_errors) = GBK.encode(value);
    if had_errors {
        return Err(StockPacketError::UnencodableString(value.to_string()));
    }
    append_discovery_segment(output, &encoded, max_len)
}

fn take_discovery_segment<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
) -> Result<&'a [u8], StockPacketError> {
    let encoded_len = *bytes
        .get(*cursor)
        .ok_or(StockPacketError::InvalidDiscoveryInfo)?;
    let len = usize::from(discovery_decrement(encoded_len)?);
    *cursor += 1;
    let end = cursor
        .checked_add(len)
        .ok_or(StockPacketError::InvalidDiscoveryInfo)?;
    let value = bytes
        .get(*cursor..end)
        .ok_or(StockPacketError::InvalidDiscoveryInfo)?;
    *cursor = end;
    Ok(value)
}

fn decode_discovery_cp936(bytes: &[u8]) -> Result<String, StockPacketError> {
    let (decoded, _, had_errors) = GBK.decode(bytes);
    if had_errors {
        return Err(StockPacketError::InvalidStockString);
    }
    Ok(decoded.into_owned())
}

fn xor_body(body: &mut [u8], key: u32) {
    let key = key.to_le_bytes();
    for (index, byte) in body.iter_mut().enumerate() {
        *byte ^= key[index & 3];
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockApplicationPacket {
    pub flags: u8,
    pub sequence: u16,
    pub ack: u16,
    pub logical_channel: u16,
    pub opcode: u16,
    pub payload: Vec<u8>,
}

impl StockApplicationPacket {
    pub fn new(
        logical_channel: u16,
        opcode: u16,
        payload: impl Into<Vec<u8>>,
        transport_flags: u8,
    ) -> Result<Self, StockPacketError> {
        let payload = payload.into();
        if payload.len() + STOCK_APPLICATION_OPCODE_BYTES > STOCK_MAX_BODY_BYTES {
            return Err(StockPacketError::TooLarge(payload.len()));
        }
        Ok(Self {
            flags: STOCK_FLAG_APPLICATION | map_application_transport_flags(transport_flags),
            sequence: 0,
            ack: 0,
            logical_channel,
            opcode,
            payload,
        })
    }

    pub fn reliable(&self) -> bool {
        self.flags & STOCK_FLAG_RELIABLE != 0
    }

    pub fn encode(&self) -> Result<Vec<u8>, StockPacketError> {
        let body_len = self.payload.len() + STOCK_APPLICATION_OPCODE_BYTES;
        if body_len > STOCK_MAX_BODY_BYTES {
            return Err(StockPacketError::TooLarge(self.payload.len()));
        }
        let mut body = Vec::with_capacity(body_len);
        body.extend_from_slice(&self.opcode.to_le_bytes());
        body.extend_from_slice(&self.payload);
        StockDatagram {
            packet_type: STOCK_PACKET_TYPE_APPLICATION,
            flags: self.flags,
            sequence: self.sequence,
            ack: self.ack,
            logical_channel: self.logical_channel,
            body,
        }
        .encode()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StockPacketError> {
        let datagram = StockDatagram::decode(bytes)?;
        if datagram.packet_type != STOCK_PACKET_TYPE_APPLICATION {
            return Err(StockPacketError::UnsupportedType(datagram.packet_type));
        }
        if datagram.body.len() < STOCK_APPLICATION_OPCODE_BYTES {
            return Err(StockPacketError::InvalidLength {
                declared: datagram.body.len(),
                actual: datagram.body.len(),
            });
        }
        Ok(Self {
            flags: datagram.flags,
            sequence: datagram.sequence,
            ack: datagram.ack,
            logical_channel: datagram.logical_channel,
            opcode: u16::from_le_bytes([datagram.body[0], datagram.body[1]]),
            payload: datagram.body[2..].to_vec(),
        })
    }
}

pub fn map_application_transport_flags(flags: u8) -> u8 {
    (if flags & 0x01 != 0 { 0x01 } else { 0 })
        | (if flags & 0x02 != 0 { 0x08 } else { 0 })
        | (if flags & 0x04 != 0 { 0x10 } else { 0 })
        | (if flags & 0x08 != 0 { 0x40 } else { 0 })
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StockPacketError {
    #[error("stock UDP datagram is truncated: {0} bytes")]
    Truncated(usize),
    #[error("stock UDP packet type 0x{0:02x} is not an application envelope")]
    UnsupportedType(u8),
    #[error("stock UDP packet type 0x{0:02x} is not a connection-control packet")]
    UnsupportedControlType(u8),
    #[error("stock UDP body length mismatch: declared {declared}, actual {actual}")]
    InvalidLength { declared: usize, actual: usize },
    #[error("stock UDP application payload is too large: {0} bytes")]
    TooLarge(usize),
    #[error("stock UDP encrypted control packet is missing its endpoint XOR key")]
    MissingXorKey,
    #[error("stock UDP control packet 0x{packet_type:02x} has invalid body length {length}")]
    InvalidControlBody { packet_type: u8, length: usize },
    #[error("stock UDP control string contains an interior NUL")]
    InteriorNul,
    #[error("stock UDP control string cannot be encoded as CP936: {0}")]
    UnencodableString(String),
    #[error("stock UDP control string is not NUL terminated")]
    UnterminatedString,
    #[error("stock UDP control string is not valid CP936")]
    InvalidStockString,
    #[error("stock UDP LAN discovery record is malformed")]
    InvalidDiscoveryInfo,
    #[error("stock UDP connect record is malformed")]
    InvalidConnectRecord,
    #[error("stock UDP LAN discovery field is too long: {0} bytes")]
    DiscoveryFieldTooLong(usize),
}

#[derive(Debug, Clone)]
struct PendingReliable {
    sequence: u16,
    sent_at_ms: u64,
    packet: StockApplicationPacket,
}

#[derive(Debug, Clone, Default)]
pub struct StockReliability {
    reliable_send_sequence: u16,
    unreliable_send_sequence: u16,
    reliable_receive_sequence: u16,
    unreliable_receive_sequence: u16,
    ack_to_send: u16,
    last_processed_ack: u16,
    last_successful_resend_scan_ms: u64,
    pending: VecDeque<PendingReliable>,
}

impl StockReliability {
    pub fn prepare_control(&mut self, bytes: &[u8]) -> Result<Vec<u8>, StockPacketError> {
        let mut datagram = StockDatagram::decode(bytes)?;
        datagram.sequence = next_sequence(&mut self.unreliable_send_sequence);
        datagram.ack = self.ack_to_send;
        if self.ack_to_send != 0 {
            datagram.flags |= STOCK_FLAG_ACK_DIRTY;
        }
        datagram.encode()
    }

    pub fn refresh_reliable(&self, mut packet: StockApplicationPacket) -> StockApplicationPacket {
        packet.ack = self.ack_to_send;
        if self.ack_to_send != 0 {
            packet.flags |= STOCK_FLAG_ACK_DIRTY;
        } else {
            packet.flags &= !STOCK_FLAG_ACK_DIRTY;
        }
        packet
    }

    pub fn prepare_outbound(
        &mut self,
        mut packet: StockApplicationPacket,
        now_ms: u64,
    ) -> StockApplicationPacket {
        if packet.reliable() {
            packet.sequence = next_sequence(&mut self.reliable_send_sequence);
        } else {
            packet.sequence = next_sequence(&mut self.unreliable_send_sequence);
        }
        packet.ack = self.ack_to_send;
        if self.ack_to_send != 0 {
            packet.flags |= STOCK_FLAG_ACK_DIRTY;
        }
        if packet.reliable() {
            self.pending.push_back(PendingReliable {
                sequence: packet.sequence,
                sent_at_ms: now_ms,
                packet: packet.clone(),
            });
        }
        packet
    }

    pub fn accept_inbound(&mut self, packet: &StockApplicationPacket) -> bool {
        self.accept_header(packet.flags, packet.sequence, packet.ack)
    }

    pub fn accept_datagram(&mut self, datagram: &StockDatagram) -> bool {
        self.accept_header(datagram.flags, datagram.sequence, datagram.ack)
    }

    fn accept_header(&mut self, flags: u8, sequence: u16, ack: u16) -> bool {
        self.apply_ack(ack);

        if sequence == 0 {
            return true;
        }
        if flags & STOCK_FLAG_RELIABLE != 0 {
            let expected = next_sequence_value(self.reliable_receive_sequence);
            if sequence != expected {
                return false;
            }
            self.reliable_receive_sequence = expected;
            self.ack_to_send = expected;
            true
        } else {
            let last = self.unreliable_receive_sequence;
            if sequence == last {
                return false;
            }
            let diff = i32::from(sequence) - i32::from(last);
            if diff < 0 && diff > -STOCK_UNRELIABLE_WRAP_TOLERANCE {
                return false;
            }
            self.unreliable_receive_sequence = sequence;
            true
        }
    }

    pub fn pending_reliable(&self) -> usize {
        self.pending.len()
    }

    pub fn resend_due(&self, now_ms: u64) -> Vec<StockApplicationPacket> {
        if self.pending.is_empty()
            || now_ms.saturating_sub(self.last_successful_resend_scan_ms)
                <= STOCK_RESEND_SCAN_GATE_MS
        {
            return Vec::new();
        }
        self.pending
            .iter()
            .filter(|record| now_ms.saturating_sub(record.sent_at_ms) >= STOCK_RESEND_ELIGIBLE_MS)
            .map(|record| record.packet.clone())
            .collect()
    }

    pub fn mark_resend_sent(&mut self, now_ms: u64) {
        self.last_successful_resend_scan_ms = now_ms;
    }

    fn apply_ack(&mut self, ack: u16) {
        if ack == 0 || ack == self.last_processed_ack {
            return;
        }
        self.last_processed_ack = ack;
        while let Some(record) = self.pending.pop_front() {
            if record.sequence == ack {
                break;
            }
        }
    }
}

fn next_sequence(sequence: &mut u16) -> u16 {
    *sequence = next_sequence_value(*sequence);
    *sequence
}

fn next_sequence_value(sequence: u16) -> u16 {
    if sequence >= STOCK_SEQUENCE_MAX {
        1
    } else {
        sequence + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_discovery_exposes_map_capacity_rule_and_password_without_extensions() {
        let mut info = StockDiscoveryInfo::default_room(5230, "original", "127.0.0.1");
        info.fields_0_to_6 = [9, 7, 6, 1, 16, 1, 1];
        let decoded = StockDiscoveryInfo::decode(&info.encode().unwrap()).unwrap();
        assert_eq!(decoded.stage_id, None);
        assert_eq!(decoded.resolved_stage_id().as_deref(), Some("sn06"));
        assert_eq!(decoded.max_players(), Some(16));
        assert_eq!(decoded.rule(), Some(1));
        assert_eq!(decoded.fields_0_to_6[6], 1);
        info.fields_0_to_6[2] = 0;
        assert_eq!(info.resolved_stage_id(), None);
        info.stage_id = Some("custom-map".into());
        assert_eq!(info.resolved_stage_id().as_deref(), Some("custom-map"));
    }

    #[test]
    fn extended_discovery_keeps_legacy_record_and_rejects_bad_metadata() {
        let mut info = StockDiscoveryInfo::default_room(5230, "room", "127.0.0.1");
        let legacy = info.encode().unwrap();
        info.stage_id = Some("sn03".into());
        assert_eq!(info.encode().unwrap(), legacy);
        assert_eq!(
            StockDiscoveryInfo::decode_room_summary(&legacy)
                .unwrap()
                .stage_id,
            None
        );
        let extended = info.encode_room_summary().unwrap();
        assert!(!extended.contains(&0));
        assert_eq!(
            StockDiscoveryInfo::decode_room_summary(&extended).unwrap(),
            info
        );
        assert!(StockDiscoveryInfo::decode(&extended).is_err());
        for bytes in [b"MRD1".as_slice(), b"MRD1ffff", b"MRD1xxxx"] {
            assert!(StockDiscoveryInfo::decode_room_summary(bytes).is_err());
        }
        let mut invalid = extended.clone();
        invalid.push(0);
        assert!(StockDiscoveryInfo::decode_room_summary(&invalid).is_err());
        invalid.pop();
        invalid.push(255);
        assert!(StockDiscoveryInfo::decode_room_summary(&invalid).is_err());
    }

    #[test]
    fn application_frame_matches_reversed_inline_buffer_layout() {
        let packet =
            StockApplicationPacket::new(0x1234, 1, [0xaa, 0xbb, 0xcc], 1).expect("packet builds");
        assert_eq!(packet.flags, 0x05);
        assert_eq!(
            packet.encode().expect("packet encodes"),
            [
                0x7f, 0x05, 0x00, 0x00, 0x00, 0x00, 0x34, 0x12, 0x05, 0x00, 0x01, 0x00, 0xaa, 0xbb,
                0xcc,
            ]
        );
        assert_eq!(
            StockApplicationPacket::decode(&packet.encode().unwrap()).unwrap(),
            packet
        );
    }

    #[test]
    fn transport_flag_mapping_matches_0054f670() {
        assert_eq!(map_application_transport_flags(0), 0);
        assert_eq!(map_application_transport_flags(1), 0x01);
        assert_eq!(map_application_transport_flags(2), 0x08);
        assert_eq!(map_application_transport_flags(4), 0x10);
        assert_eq!(map_application_transport_flags(8), 0x40);
        assert_eq!(map_application_transport_flags(0x0f), 0x59);
    }

    #[test]
    fn connection_control_vectors_match_stock_dispatch_layout() {
        let challenge = StockControlPacket::Challenge {
            cookie: 0x1122_3344,
        }
        .encode(0)
        .unwrap();
        assert_eq!(
            challenge,
            [1, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0x44, 0x33, 0x22, 0x11,]
        );
        assert_eq!(
            StockControlPacket::decode(&challenge, None).unwrap(),
            StockControlPacket::Challenge {
                cookie: 0x1122_3344
            }
        );

        let initial = StockControlPacket::Connect {
            cookie: 0,
            record: b"ignored until challenged".to_vec(),
        }
        .encode(0)
        .unwrap();
        assert_eq!(&initial[0..10], &[2, 0, 0, 0, 0, 0, 0, 0, 4, 0]);
        assert_eq!(&initial[10..], &[0, 0, 0, 0]);

        let encrypted = StockControlPacket::Connect {
            cookie: 0x1122_3344,
            record: b"abc".to_vec(),
        }
        .encode(0)
        .unwrap();
        assert_eq!(&encrypted[0..10], &[2, 0x10, 0, 0, 0, 0, 0, 0, 8, 0]);
        assert_eq!(&encrypted[10..], &[0, 0, 0, 0, 0x25, 0x51, 0x41, 0x11]);
        assert_eq!(
            StockControlPacket::decode(&encrypted, Some(0x1122_3344)).unwrap(),
            StockControlPacket::Connect {
                cookie: 0x1122_3344,
                record: b"abc".to_vec(),
            }
        );
        assert!(matches!(
            StockControlPacket::decode(&encrypted, None),
            Err(StockPacketError::MissingXorKey)
        ));
    }

    #[test]
    fn control_packets_share_unreliable_sequence_and_carry_reliable_ack() {
        let mut reliability = StockReliability::default();
        let mut reliable = StockApplicationPacket::new(7, 9, [1, 2, 3], 1).unwrap();
        reliable.sequence = 1;
        assert!(reliability.accept_inbound(&reliable));

        let ping = StockControlPacket::Ping {
            client_time: 123,
            latency: 45,
        }
        .encode(7)
        .unwrap();
        let first = StockDatagram::decode(&reliability.prepare_control(&ping).unwrap()).unwrap();
        assert_eq!(first.sequence, 1);
        assert_eq!(first.ack, 1);
        assert_ne!(first.flags & STOCK_FLAG_ACK_DIRTY, 0);

        let second = StockDatagram::decode(&reliability.prepare_control(&ping).unwrap()).unwrap();
        assert_eq!(second.sequence, 2);
        assert_eq!(second.ack, 1);
    }

    #[test]
    fn discovery_and_cp936_strings_round_trip() {
        let packet = StockControlPacket::Discover {
            sent_at: 1234,
            query: "流星服务器".into(),
        };
        let bytes = packet.encode(0).unwrap();
        assert_eq!(bytes[0], STOCK_PACKET_TYPE_DISCOVER);
        assert_eq!(StockControlPacket::decode(&bytes, None).unwrap(), packet);

        let info = StockDiscoveryInfo {
            fields_0_to_6: [1, 2, 3, 4, 5, 6, 7],
            port: 5230,
            field_0e: 8,
            field_0f: 9,
            server_name: "Rust".into(),

            server_address: "192.168.2.203".into(),
            tail: Some(0),
            stage_id: None,
        };
        let encoded_info = info.encode().unwrap();
        assert_eq!(
            encoded_info,
            [
                b'M', b'E', 2, 3, 4, 7, 8, 5, 6, 9, 10, 5, b'5', b'2', b'3', b'0', 5, b'R', b'u',
                b's', b't', 14, b'1', b'9', b'2', b'.', b'1', b'6', b'8', b'.', b'2', b'.', b'2',
                b'0', b'3', 1,
            ]
        );
        assert_eq!(StockDiscoveryInfo::decode(&encoded_info).unwrap(), info);

        let reply = StockControlPacket::DiscoverReply {
            sent_at: 1234,
            info: encoded_info,
        };
        let bytes = reply.encode(0).unwrap();
        assert_eq!(bytes[0], STOCK_PACKET_TYPE_DISCOVER_REPLY);
        assert_eq!(StockControlPacket::decode(&bytes, None).unwrap(), reply);
    }

    #[test]
    fn reliable_sequence_ack_and_wrap_are_separate_from_unreliable_stream() {
        let mut sender = StockReliability::default();
        let reliable = StockApplicationPacket::new(7, 30, [], 1).unwrap();
        let unreliable = StockApplicationPacket::new(7, 1, [1; 20], 0).unwrap();
        let reliable1 = sender.prepare_outbound(reliable.clone(), 0);
        let state1 = sender.prepare_outbound(unreliable.clone(), 0);
        assert_eq!(reliable1.sequence, 1);
        assert_eq!(state1.sequence, 1);
        assert_eq!(sender.pending_reliable(), 1);

        let mut receiver = StockReliability::default();
        assert!(receiver.accept_inbound(&reliable1));
        assert!(!receiver.accept_inbound(&reliable1));
        let ack_carrier = receiver.prepare_outbound(unreliable, 10);
        assert_eq!(ack_carrier.ack, 1);
        assert_ne!(ack_carrier.flags & STOCK_FLAG_ACK_DIRTY, 0);
        assert!(sender.accept_inbound(&ack_carrier));
        assert_eq!(sender.pending_reliable(), 0);

        sender.reliable_send_sequence = STOCK_SEQUENCE_MAX;
        assert_eq!(sender.prepare_outbound(reliable, 20).sequence, 1);
    }

    #[test]
    fn connect_record_matches_wire_accepted_by_a_live_stock_host() {
        let record = StockConnectRecord::new("", -1_994_769_449);
        let encoded = record.encode().expect("record encodes");
        assert_eq!(
            encoded,
            [
                0x0a, 0x08, 0x01, 0x01, 0x01, 0x06, b'A', b'g', b'e', b'n', b't', 0x01, 0x01, 0x0c,
                b'-', b'1', b'9', b'9', b'4', b'7', b'6', b'9', b'4', b'4', b'9',
            ]
        );
        assert_eq!(StockConnectRecord::decode(&encoded).unwrap(), record);

        let secured = StockConnectRecord::new("门派", 1);
        let encoded = secured.encode().unwrap();
        assert_eq!(encoded[..3], [0x0a, 0x08, 0x01]);
        assert_eq!(encoded[3], 5, "CP936 两字 + 1 的长度前缀");
        assert_eq!(StockConnectRecord::decode(&encoded).unwrap(), secured);
    }

    #[test]
    fn connect_player_name_uses_second_cp936_segment() {
        let mut record = StockConnectRecord::new("", 0);
        record.player_name = "Rust测试".into();
        let wire = record.encode().unwrap();

        assert_eq!(
            wire,
            [
                10, 8, 1, 1, 9, b'R', b'u', b's', b't', 0xb2, 0xe2, 0xca, 0xd4, 6, b'A', b'g',
                b'e', b'n', b't', 1, 1, 2, b'0',
            ]
        );
        assert_eq!(StockConnectRecord::decode(&wire).unwrap(), record);
        record.player_name = "测".repeat(127);
        assert!(record.encode().is_ok());
        record.player_name.push('试');
        assert!(
            record.encode().is_err(),
            "never split a CP936 character or wrap the length"
        );
        record.player_name = "🦋".into();
        assert_eq!(
            StockConnectRecord::decode(&record.encode().unwrap())
                .unwrap()
                .player_name,
            "?"
        );
    }

    #[test]
    fn unicode_names_fall_back_only_in_legacy_display_fields() {
        let name = "玩家3️⃣🦋☆";
        let expected = "玩家3???☆";
        let mut record = StockConnectRecord::for_open_room();
        record.player_name = name.into();
        let wire = record.encode().unwrap();
        assert_eq!(
            StockConnectRecord::decode(&wire).unwrap().player_name,
            expected
        );
        assert_eq!(record.player_name, name, "preserve the Unicode source name");
        let room = StockDiscoveryInfo::default_room(5230, name, "127.0.0.1");
        assert_eq!(
            StockDiscoveryInfo::decode(&room.encode().unwrap())
                .unwrap()
                .server_name,
            expected
        );
        record.password = "🔑".into();
        assert!(matches!(
            record.encode(),
            Err(StockPacketError::UnencodableString(_))
        ));
        let room = StockDiscoveryInfo::default_room(5230, "🦋".repeat(63), "");
        assert!(room.encode().is_ok(), "limit counts fallback wire bytes");
        let room = StockDiscoveryInfo::default_room(5230, "🦋".repeat(64), "");
        assert!(matches!(
            room.encode(),
            Err(StockPacketError::DiscoveryFieldTooLong(64))
        ));
    }

    #[test]
    fn integrity_checksum_uses_signed_1_based_weighting_of_0x476120() {
        assert_eq!(stock_weighted_checksum(&[]), 0);
        assert_eq!(stock_weighted_checksum(&[1, 2, 3]), 1 + 2 * 2 + 3 * 3);
        assert_eq!(stock_weighted_checksum(&[0xff]), -1, "0xff 是 -1，不是 255");

        let checksum = stock_integrity_checksum(|path| {
            (path == STOCK_INTEGRITY_FILES[2]).then(|| vec![1, 2, 3])
        });
        assert_eq!(checksum, stock_weighted_checksum(&[1, 2, 3]));
        assert_eq!(stock_integrity_checksum(|_| None), 0);
    }

    #[test]
    fn unsequenced_zero_and_unreliable_reorder_match_005540e0() {
        let mut reliability = StockReliability::default();

        for _ in 0..3 {
            assert!(reliability.accept_header(0, 0, 0));
        }
        assert_eq!(reliability.unreliable_receive_sequence, 0);

        assert!(reliability.accept_header(0, 5, 0));
        assert!(!reliability.accept_header(0, 5, 0));
        assert!(!reliability.accept_header(0, 3, 0));
        assert!(reliability.accept_header(0, 6, 0));

        reliability.unreliable_receive_sequence = STOCK_SEQUENCE_MAX;
        assert!(reliability.accept_header(0, 1, 0));
        reliability.unreliable_receive_sequence = 30_000;
        assert!(!reliability.accept_header(0, 1, 0));
    }

    #[test]
    fn resend_requires_both_300ms_record_age_and_strict_500ms_scan_gate() {
        let mut transport = StockReliability::default();
        let reliable = StockApplicationPacket::new(7, 30, [], 1).unwrap();
        transport.prepare_outbound(reliable, 100);
        assert!(transport.resend_due(500).is_empty());
        assert_eq!(transport.resend_due(501).len(), 1);
        transport.mark_resend_sent(501);
        assert!(transport.resend_due(1001).is_empty());
        assert_eq!(transport.resend_due(1002).len(), 1);
    }

    #[test]
    fn malformed_lengths_are_rejected_before_opcode_access() {
        let mut bytes = StockApplicationPacket::new(1, 2, [3, 4], 0)
            .unwrap()
            .encode()
            .unwrap();
        bytes[8] = 8;
        assert!(matches!(
            StockApplicationPacket::decode(&bytes),
            Err(StockPacketError::InvalidLength { .. })
        ));
    }
}
