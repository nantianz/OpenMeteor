pub const MRC_MAGIC: u32 = 0x1e0d;

pub const MRC_HEADER_SIZE: u32 = 0x28;

pub const MRC_STATE_SIZE: usize = 20;

#[derive(Debug, Clone)]
pub struct MrcQueueAEntry {
    pub id: u32,
    pub time: f32,
    pub blob: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct MrcQueueBEntry {
    pub code: u32,
    pub value: u32,
    pub time: f32,
}

#[derive(Debug, Clone)]
pub struct MrcQueueCEntry {
    pub packed: u32,
    pub time: f32,
}

#[derive(Debug, Clone)]
pub struct MrcQueueDEntry {
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub time: f32,
}

#[derive(Debug, Clone)]
pub struct MrcQueueEEntry {
    pub a: u32,
    pub b: u32,
    pub time: f32,
}

#[derive(Debug, Clone)]
pub struct MrcDocument {
    pub magic: u32,
    pub header_size: u32,
    pub mark0: u32,
    pub mark1: u32,
    pub tail: u32,
    pub state: [u8; MRC_STATE_SIZE],
    pub meta: [u32; 8],
    pub queue_a: Vec<MrcQueueAEntry>,
    pub queue_b: Vec<MrcQueueBEntry>,
    pub queue_c: Vec<MrcQueueCEntry>,
    pub queue_d: Vec<MrcQueueDEntry>,
    pub queue_e: Vec<MrcQueueEEntry>,
}

impl MrcDocument {
    pub fn duration(&self) -> f32 {
        let last = |time: f32| time;
        [
            self.queue_a.last().map(|e| e.time).unwrap_or(0.0),
            self.queue_b.last().map(|e| e.time).unwrap_or(0.0),
            self.queue_c.last().map(|e| e.time).unwrap_or(0.0),
            self.queue_d.last().map(|e| e.time).unwrap_or(0.0),
            self.queue_e.last().map(|e| e.time).unwrap_or(0.0),
        ]
        .into_iter()
        .fold(0.0f32, |acc, t| acc.max(last(t)))
    }
}

pub fn decode_mrc(input: &[u8]) -> Result<MrcDocument, String> {
    let mut reader = MrcReader {
        bytes: input,
        offset: 0,
    };
    let magic = reader.u32()?;
    if magic != MRC_MAGIC {
        return Err(format!("mrc bad magic 0x{magic:x} (want 0x{MRC_MAGIC:x})"));
    }
    let header_size = reader.u32()?;
    if header_size != MRC_HEADER_SIZE {
        return Err(format!(
            "mrc bad headerSize {header_size} (want {MRC_HEADER_SIZE})"
        ));
    }
    let mark0 = reader.u32()?;
    let mark1 = reader.u32()?;
    let count_a = reader.u32()? as usize;
    let count_b = reader.u32()? as usize;
    let count_c = reader.u32()? as usize;
    let count_d = reader.u32()? as usize;
    let count_e = reader.u32()? as usize;
    let tail = reader.u32()?;

    let raw_state = reader.bytes(MRC_STATE_SIZE)?;
    let mut state = [0u8; MRC_STATE_SIZE];
    state.copy_from_slice(raw_state);
    let mut meta = [0u32; 8];
    for value in &mut meta {
        *value = reader.u32()?;
    }

    let mut queue_a = Vec::with_capacity(count_a);
    for _ in 0..count_a {
        let id = reader.u32()?;
        let time = reader.f32()?;
        let blob_len = reader.u32()? as usize;
        if blob_len > 0x1000000 {
            return Err(format!("mrc queueA blobLen {blob_len} too large"));
        }
        let blob = if blob_len > 0 {
            reader.bytes(blob_len)?.to_vec()
        } else {
            Vec::new()
        };
        queue_a.push(MrcQueueAEntry { id, time, blob });
    }
    let mut queue_b = Vec::with_capacity(count_b);
    for _ in 0..count_b {
        let code = reader.u32()?;
        let value = reader.u32()?;
        let time = reader.f32()?;
        queue_b.push(MrcQueueBEntry { code, value, time });
    }
    let mut queue_c = Vec::with_capacity(count_c);
    for _ in 0..count_c {
        let packed = reader.u32()?;
        let time = reader.f32()?;
        queue_c.push(MrcQueueCEntry { packed, time });
    }
    let mut queue_d = Vec::with_capacity(count_d);
    for _ in 0..count_d {
        let a = reader.u32()?;
        let b = reader.u32()?;
        let c = reader.u32()?;
        let time = reader.f32()?;
        queue_d.push(MrcQueueDEntry { a, b, c, time });
    }
    let mut queue_e = Vec::with_capacity(count_e);
    for _ in 0..count_e {
        let a = reader.u32()?;
        let b = reader.u32()?;
        let time = reader.f32()?;
        queue_e.push(MrcQueueEEntry { a, b, time });
    }
    Ok(MrcDocument {
        magic,
        header_size,
        mark0,
        mark1,
        tail,
        state,
        meta,
        queue_a,
        queue_b,
        queue_c,
        queue_d,
        queue_e,
    })
}

pub fn encode_mrc(doc: &MrcDocument) -> Vec<u8> {
    let mut out = Vec::with_capacity(4096);
    let push_u32 = |out: &mut Vec<u8>, value: u32| out.extend_from_slice(&value.to_le_bytes());
    let push_f32 = |out: &mut Vec<u8>, value: f32| out.extend_from_slice(&value.to_le_bytes());
    push_u32(&mut out, doc.magic);
    push_u32(&mut out, doc.header_size);
    push_u32(&mut out, doc.mark0);
    push_u32(&mut out, doc.mark1);
    push_u32(&mut out, doc.queue_a.len() as u32);
    push_u32(&mut out, doc.queue_b.len() as u32);
    push_u32(&mut out, doc.queue_c.len() as u32);
    push_u32(&mut out, doc.queue_d.len() as u32);
    push_u32(&mut out, doc.queue_e.len() as u32);
    push_u32(&mut out, doc.tail);
    out.extend_from_slice(&doc.state);
    for value in &doc.meta {
        push_u32(&mut out, *value);
    }
    for entry in &doc.queue_a {
        push_u32(&mut out, entry.id);
        push_f32(&mut out, entry.time);
        push_u32(&mut out, entry.blob.len() as u32);
        out.extend_from_slice(&entry.blob);
    }
    for entry in &doc.queue_b {
        push_u32(&mut out, entry.code);
        push_u32(&mut out, entry.value);
        push_f32(&mut out, entry.time);
    }
    for entry in &doc.queue_c {
        push_u32(&mut out, entry.packed);
        push_f32(&mut out, entry.time);
    }
    for entry in &doc.queue_d {
        push_u32(&mut out, entry.a);
        push_u32(&mut out, entry.b);
        push_u32(&mut out, entry.c);
        push_f32(&mut out, entry.time);
    }
    for entry in &doc.queue_e {
        push_u32(&mut out, entry.a);
        push_u32(&mut out, entry.b);
        push_f32(&mut out, entry.time);
    }
    out
}

struct MrcReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> MrcReader<'a> {
    fn ensure(&self, size: usize, label: &str) -> Result<(), String> {
        if self.offset + size > self.bytes.len() {
            Err(format!(
                "mrc truncated while reading {label} at 0x{:x}",
                self.offset
            ))
        } else {
            Ok(())
        }
    }

    fn bytes(&mut self, size: usize) -> Result<&'a [u8], String> {
        self.ensure(size, "bytes")?;
        let value = &self.bytes[self.offset..self.offset + size];
        self.offset += size;
        Ok(value)
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn f32(&mut self) -> Result<f32, String> {
        let bytes = self.bytes(4)?;
        Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

pub mod codes {

    pub const FORWARD: u32 = 0x57;
    pub const BACKWARD: u32 = 0x53;
    pub const LEFT: u32 = 0x41;
    pub const RIGHT: u32 = 0x44;

    pub const ATTACK: u32 = 0x1000;
    pub const JUMP: u32 = 0x1200;

    pub const GUARD: u32 = 0x20;
    pub const CROUCH: u32 = 0x43;

    pub const QI_BURST: u32 = 0x46;

    pub const SWITCH_WEAPON: u32 = 0x45;

    pub const UNLOCK_CAMERA: u32 = 0x51;
    pub const DROP_WEAPON: u32 = 0x47;
    pub const DROP_ITEM: u32 = 0x48;
    pub const CURE: u32 = 0x54;
    pub const FAKE: u32 = 0x59;
    pub const SCORE: u32 = 0x09;
    pub const QUIT: u32 = 0x1b;

    pub const CAMERA_UP: u32 = 0x26;
    pub const CAMERA_DOWN: u32 = 0x28;
    pub const LEFT_TURN: u32 = 0x25;
    pub const RIGHT_TURN: u32 = 0x27;
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_empty_document() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MRC_MAGIC.to_le_bytes());
        bytes.extend_from_slice(&MRC_HEADER_SIZE.to_le_bytes());
        for value in [0u32, 0, 0, 0, 0, 0, 0, 0, 0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&[0u8; MRC_STATE_SIZE]);
        bytes.extend_from_slice(
            &[0u32; 8]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        let doc = decode_mrc(&bytes).unwrap();
        assert_eq!(doc.magic, MRC_MAGIC);
        assert_eq!(doc.queue_a.len(), 0);
        assert_eq!(doc.duration(), 0.0);
    }

    #[test]
    fn decode_rejects_bad_magic_and_header() {
        let mut bytes = vec![0u8; 64];
        assert!(decode_mrc(&bytes).is_err());
        bytes[0..4].copy_from_slice(&MRC_MAGIC.to_le_bytes());
        assert!(decode_mrc(&bytes).is_err());
        bytes[4..8].copy_from_slice(&0x29u32.to_le_bytes());
        assert!(decode_mrc(&bytes).is_err());
    }

    #[test]
    fn decode_rejects_truncated() {
        let bytes = [0u8; 8];
        assert!(decode_mrc(&bytes).is_err());
    }

    #[test]
    fn all_queue_wire_fixture_round_trips() {
        let mut bytes = Vec::new();
        for word in [MRC_MAGIC, MRC_HEADER_SIZE, 7, 9, 1, 1, 1, 1, 1, 0] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&[0x12; MRC_STATE_SIZE]);
        bytes.extend_from_slice(&[0; 32]);

        for word in [42u32, 1.0f32.to_bits(), 3] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&[0xaa, 0xbb, 0xcc]);
        for word in [
            codes::ATTACK,
            1,
            2.0f32.to_bits(),
            0x1234,
            3.0f32.to_bits(),
            10,
            20,
            30,
            4.0f32.to_bits(),
            40,
            50,
            5.0f32.to_bits(),
        ] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let doc = decode_mrc(&bytes).unwrap();
        assert_eq!(doc.queue_a[0].blob, [0xaa, 0xbb, 0xcc]);
        assert_eq!(doc.queue_b[0].code, codes::ATTACK);
        assert_eq!(doc.queue_c[0].packed, 0x1234);
        assert_eq!(doc.queue_d[0].c, 30);
        assert_eq!(doc.queue_e[0].b, 50);
        assert_eq!(doc.duration(), 5.0);
        assert_eq!(encode_mrc(&doc), bytes);
    }

    #[test]
    #[ignore = "selected 9.07 resource release contains no mrcfile demos; requires external replay fixtures"]
    fn origin_res_mrc_demos_round_trip() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let dir = root.join("mrcfile");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let mut checked = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("mrc") {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let doc = decode_mrc(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let reencoded = encode_mrc(&doc);
            assert_eq!(
                reencoded,
                bytes,
                "{} round-trips byte-exact",
                path.display()
            );
            checked += 1;
        }
        assert!(checked > 0, "golden demos round-trip");
    }

    #[test]
    #[ignore = "selected 9.07 resource release contains no mrcfile demos; requires external replay fixtures"]
    fn origin_res_mrc_demo_decodes() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let dir = root.join("mrcfile");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let mut parsed = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("mrc") {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let doc = decode_mrc(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(doc.magic, MRC_MAGIC);
            assert_eq!(doc.header_size, MRC_HEADER_SIZE);

            assert!(doc.duration() >= 0.0);
            parsed += 1;
            eprintln!(
                "{}: A{} B{} C{} D{} E{} {:.1}s",
                path.file_name().unwrap().to_string_lossy(),
                doc.queue_a.len(),
                doc.queue_b.len(),
                doc.queue_c.len(),
                doc.queue_d.len(),
                doc.queue_e.len(),
                doc.duration()
            );
        }
        assert!(parsed >= 1, "no .mrc samples found");
    }
}
