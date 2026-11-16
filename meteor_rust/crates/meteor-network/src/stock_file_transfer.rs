use sha2::{Digest, Sha256};
use std::io::{self, Read, Seek, SeekFrom, Write};

pub const CHUNK_BYTES: usize = 1024;
pub const WINDOW_CHUNKS: u64 = 8;
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAGIC: &[u8; 4] = b"MFT1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Offer {
        id: u32,
        kind: u16,
        len: u64,
        sha256: [u8; 32],
    },
    Data {
        id: u32,
        offset: u64,
        bytes: Vec<u8>,
    },
    Ack {
        id: u32,
        next: u64,
    },
    Reject {
        id: u32,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("invalid file transfer frame")]
    InvalidFrame,
    #[error("file transfer exceeds its size limit")]
    TooLarge,
    #[error("file transfer ID or offset does not match")]
    UnexpectedFrame,
    #[error("file transfer SHA-256 mismatch")]
    HashMismatch,
    #[error("file transfer was rejected")]
    Rejected,
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl Frame {
    pub fn id(&self) -> u32 {
        match self {
            Self::Offer { id, .. }
            | Self::Data { id, .. }
            | Self::Ack { id, .. }
            | Self::Reject { id } => *id,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(17 + CHUNK_BYTES);
        out.extend_from_slice(MAGIC);
        match self {
            Self::Offer {
                id,
                kind,
                len,
                sha256,
            } => {
                out.push(1);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&kind.to_le_bytes());
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(sha256);
            }
            Self::Data { id, offset, bytes } => {
                out.push(2);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&offset.to_le_bytes());
                out.extend_from_slice(bytes);
            }
            Self::Ack { id, next } => {
                out.push(3);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&next.to_le_bytes());
            }
            Self::Reject { id } => {
                out.push(4);
                out.extend_from_slice(&id.to_le_bytes());
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, TransferError> {
        if bytes.len() < 9 || !bytes.starts_with(MAGIC) {
            return Err(TransferError::InvalidFrame);
        }
        let id = u32::from_le_bytes(
            bytes[5..9]
                .try_into()
                .map_err(|_| TransferError::InvalidFrame)?,
        );
        match bytes[4] {
            1 if bytes.len() == 51 => Ok(Self::Offer {
                id,
                kind: u16::from_le_bytes(
                    bytes[9..11]
                        .try_into()
                        .map_err(|_| TransferError::InvalidFrame)?,
                ),
                len: u64::from_le_bytes(
                    bytes[11..19]
                        .try_into()
                        .map_err(|_| TransferError::InvalidFrame)?,
                ),
                sha256: bytes[19..51]
                    .try_into()
                    .map_err(|_| TransferError::InvalidFrame)?,
            }),
            2 if bytes.len() > 17 && bytes.len() <= 17 + CHUNK_BYTES => Ok(Self::Data {
                id,
                offset: u64::from_le_bytes(
                    bytes[9..17]
                        .try_into()
                        .map_err(|_| TransferError::InvalidFrame)?,
                ),
                bytes: bytes[17..].to_vec(),
            }),
            3 if bytes.len() == 17 => Ok(Self::Ack {
                id,
                next: u64::from_le_bytes(
                    bytes[9..17]
                        .try_into()
                        .map_err(|_| TransferError::InvalidFrame)?,
                ),
            }),
            4 if bytes.len() == 9 => Ok(Self::Reject { id }),
            _ => Err(TransferError::InvalidFrame),
        }
    }
}

pub struct Sender<R> {
    source: R,
    offer: Frame,
    acknowledged: u64,
    sent: u64,
    started: bool,
}

impl<R: Read + Seek> Sender<R> {
    pub fn new(id: u32, kind: u16, mut source: R, max_len: u64) -> Result<Self, TransferError> {
        let len = source.seek(SeekFrom::End(0))?;
        if len > max_len.min(MAX_FILE_BYTES) {
            return Err(TransferError::TooLarge);
        }
        source.seek(SeekFrom::Start(0))?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 8192];
        loop {
            let read = source.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
        source.seek(SeekFrom::Start(0))?;
        Ok(Self {
            source,
            offer: Frame::Offer {
                id,
                kind,
                len,
                sha256: hash.finalize().into(),
            },
            acknowledged: 0,
            sent: 0,
            started: false,
        })
    }

    pub fn offer(&self) -> Frame {
        self.offer.clone()
    }
    pub fn complete(&self) -> bool {
        let Frame::Offer { len, .. } = self.offer else {
            unreachable!()
        };
        self.started && self.acknowledged == len
    }

    pub fn accept(&mut self, reply: &Frame) -> Result<Vec<Frame>, TransferError> {
        let Frame::Offer { id, len, .. } = self.offer else {
            unreachable!()
        };
        match reply {
            Frame::Reject { id: rejected } if *rejected == id => {
                return Err(TransferError::Rejected)
            }
            Frame::Ack { id: received, next }
                if *received == id && *next <= self.sent && *next >= self.acknowledged =>
            {
                self.started = true;
                self.acknowledged = *next;
            }
            _ => return Err(TransferError::UnexpectedFrame),
        }
        let window_end = self
            .acknowledged
            .saturating_add(WINDOW_CHUNKS * CHUNK_BYTES as u64)
            .min(len);
        let mut frames = Vec::new();
        while self.sent < window_end {
            self.source.seek(SeekFrom::Start(self.sent))?;
            let amount = (len - self.sent).min(CHUNK_BYTES as u64) as usize;
            let mut bytes = vec![0; amount];
            self.source.read_exact(&mut bytes)?;
            frames.push(Frame::Data {
                id,
                offset: self.sent,
                bytes,
            });
            self.sent += amount as u64;
        }
        Ok(frames)
    }
}

pub struct Receiver<W> {
    sink: W,
    id: u32,
    len: u64,
    expected_hash: [u8; 32],
    hash: Sha256,
    received: u64,
    complete: bool,
}

impl<W: Write> Receiver<W> {
    pub fn new(offer: &Frame, sink: W, max_len: u64) -> Result<Self, TransferError> {
        let Frame::Offer {
            id, len, sha256, ..
        } = offer
        else {
            return Err(TransferError::UnexpectedFrame);
        };
        if *len > max_len.min(MAX_FILE_BYTES) {
            return Err(TransferError::TooLarge);
        }
        let complete = *len == 0;
        if complete && Sha256::digest([]).as_slice() != sha256 {
            return Err(TransferError::HashMismatch);
        }
        Ok(Self {
            sink,
            id: *id,
            len: *len,
            expected_hash: *sha256,
            hash: Sha256::new(),
            received: 0,
            complete,
        })
    }

    pub fn ack(&self) -> Frame {
        Frame::Ack {
            id: self.id,
            next: self.received,
        }
    }
    pub fn complete(&self) -> bool {
        self.complete
    }
    pub fn into_inner(self) -> W {
        self.sink
    }

    pub fn receive(&mut self, frame: &Frame) -> Result<Frame, TransferError> {
        let Frame::Data { id, offset, bytes } = frame else {
            return Err(TransferError::UnexpectedFrame);
        };
        if *id != self.id || bytes.is_empty() || bytes.len() > CHUNK_BYTES {
            return Err(TransferError::UnexpectedFrame);
        }
        if *offset < self.received {
            if offset.saturating_add(bytes.len() as u64) > self.received {
                return Err(TransferError::UnexpectedFrame);
            }
            return Ok(self.ack());
        }
        if *offset != self.received
            || self.complete
            || offset.saturating_add(bytes.len() as u64) > self.len
        {
            return Err(TransferError::UnexpectedFrame);
        }
        self.sink.write_all(bytes)?;
        self.hash.update(bytes);
        self.received += bytes.len() as u64;
        if self.received == self.len {
            if self.hash.clone().finalize().as_slice() != self.expected_hash {
                return Err(TransferError::HashMismatch);
            }
            self.sink.flush()?;
            self.complete = true;
        }
        Ok(self.ack())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn streams_large_file_with_bounded_window_and_hash() {
        let bytes: Vec<u8> = (0..3_000_000).map(|i| (i % 251) as u8).collect();
        let mut sender = Sender::new(7, 2, Cursor::new(bytes.clone()), MAX_FILE_BYTES).unwrap();
        let offer = Frame::decode(&sender.offer().encode()).unwrap();
        let mut receiver = Receiver::new(&offer, Vec::new(), MAX_FILE_BYTES).unwrap();
        let mut frames = sender.accept(&receiver.ack()).unwrap();
        assert_eq!(frames.len(), WINDOW_CHUNKS as usize);
        while !sender.complete() {
            let frame = frames.remove(0);
            let decoded = Frame::decode(&frame.encode()).unwrap();
            let ack = receiver.receive(&decoded).unwrap();
            frames.extend(
                sender
                    .accept(&Frame::decode(&ack.encode()).unwrap())
                    .unwrap(),
            );
        }
        assert!(receiver.complete());
        assert_eq!(receiver.into_inner(), bytes);
    }

    #[test]
    fn rejects_corruption_and_oversize_offer() {
        let mut sender = Sender::new(1, 1, Cursor::new(vec![3; 1200]), 16_384).unwrap();
        let offer = sender.offer();
        assert!(matches!(
            Receiver::new(&offer, Vec::new(), 1000),
            Err(TransferError::TooLarge)
        ));
        let mut receiver = Receiver::new(&offer, Vec::new(), 16_384).unwrap();
        let mut frames = sender.accept(&receiver.ack()).unwrap();
        let Frame::Data { bytes, .. } = &mut frames[1] else {
            panic!()
        };
        bytes[0] ^= 1;
        receiver.receive(&frames[0]).unwrap();
        assert!(matches!(
            receiver.receive(&frames[1]),
            Err(TransferError::HashMismatch)
        ));
        assert!(Frame::decode(&[b'M', b'F', b'T', b'1', 2, 1, 0, 0, 0]).is_err());
    }

    #[test]
    fn duplicate_chunk_is_acknowledged_without_writing_twice() {
        let bytes = vec![7; CHUNK_BYTES * 2];
        let mut sender = Sender::new(9, 3, Cursor::new(bytes.clone()), MAX_FILE_BYTES).unwrap();
        let mut receiver = Receiver::new(&sender.offer(), Vec::new(), MAX_FILE_BYTES).unwrap();
        let frames = sender.accept(&receiver.ack()).unwrap();
        assert_eq!(frames.len(), 2);
        assert!(frames
            .iter()
            .all(|frame| frame.encode().len() + 2 <= crate::stock_udp::STOCK_MAX_BODY_BYTES));
        let first_ack = receiver.receive(&frames[0]).unwrap();
        assert_eq!(receiver.receive(&frames[0]).unwrap(), first_ack);
        assert!(
            matches!(sender.accept(&Frame::Ack { id: 9, next: 0 }), Ok(ref sent) if sent.is_empty())
        );
        sender.accept(&first_ack).unwrap();
        assert!(matches!(
            sender.accept(&Frame::Ack { id: 9, next: 0 }),
            Err(TransferError::UnexpectedFrame)
        ));
        let final_ack = receiver.receive(&frames[1]).unwrap();
        sender.accept(&final_ack).unwrap();
        assert!(sender.complete());
        assert_eq!(receiver.into_inner(), bytes);
    }
}
