use std::collections::HashMap;

pub const MPK1_MAGIC: &[u8; 4] = b"MPK1";
const HEADER_SIZE: usize = 8;

#[derive(Debug, Clone)]
pub struct PakEntry {
    pub name: String,

    pub offset: u64,

    pub size: u64,

    pub mime: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MpkiManifest {
    format_version: u32,
    #[serde(default)]
    kind: String,
    entries: Vec<MpkiEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct MpkiEntry {
    name: String,
    #[serde(rename = "off", alias = "offset")]
    offset: u64,
    #[serde(rename = "len", alias = "size")]
    size: u64,
    #[serde(default)]
    mime: String,
}

#[derive(Debug, Clone)]
pub struct PakArchive {
    entries: HashMap<String, PakEntry>,

    blob: Vec<u8>,
}

impl PakArchive {
    pub fn from_bytes(data: &[u8]) -> Result<Self, PakError> {
        if data.starts_with(MPK1_MAGIC) {
            Self::from_mpk1(data)
        } else if data.starts_with(b"PK\x03\x04") || data.starts_with(b"PK\x05\x06") {
            Self::from_zip(data)
        } else {
            Err(PakError::BadMagic)
        }
    }

    pub fn from_zip(data: &[u8]) -> Result<Self, PakError> {
        use std::io::{Cursor, Read};

        let mut archive = zip::ZipArchive::new(Cursor::new(data))?;
        let mut entries = HashMap::with_capacity(archive.len());
        let mut blob = Vec::new();
        for index in 0..archive.len() {
            let mut file = archive.by_index(index)?;
            if file.is_dir() {
                continue;
            }
            let name = file.name().to_string();
            let offset = blob.len() as u64;
            file.read_to_end(&mut blob)?;
            let size = blob.len() as u64 - offset;
            entries.insert(
                normalize_entry_name(&name),
                PakEntry {
                    name,
                    offset,
                    size,
                    mime: None,
                },
            );
        }
        Ok(Self { entries, blob })
    }

    pub fn from_mpk1(data: &[u8]) -> Result<Self, PakError> {
        if data.len() < HEADER_SIZE {
            return Err(PakError::TooSmall);
        }

        let magic = &data[0..4];
        if magic != MPK1_MAGIC {
            return Err(PakError::BadMagic);
        }

        let manifest_len = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;

        if data.len() < HEADER_SIZE + manifest_len {
            return Err(PakError::TooSmall);
        }

        let manifest_bytes = &data[HEADER_SIZE..HEADER_SIZE + manifest_len];
        let manifest: MpkiManifest = serde_json::from_slice(manifest_bytes)
            .map_err(|e| PakError::InvalidManifest(e.to_string()))?;
        if manifest.format_version != 1 {
            return Err(PakError::InvalidManifest(format!(
                "unsupported formatVersion {}",
                manifest.format_version
            )));
        }

        let blob_start = HEADER_SIZE + manifest_len;
        let blob = data[blob_start..].to_vec();

        let mut entries = HashMap::with_capacity(manifest.entries.len());
        let mut ranges = Vec::with_capacity(manifest.entries.len());
        for e in &manifest.entries {
            let name = normalize_entry_name(&e.name);
            if name.is_empty()
                || name.starts_with('/')
                || name.contains('\0')
                || name
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
            {
                return Err(PakError::InvalidManifest(format!(
                    "unsafe entry name: {}",
                    e.name
                )));
            }
            let end = e.offset.checked_add(e.size).ok_or_else(|| {
                PakError::InvalidManifest(format!("entry byte range overflow: {}", e.name))
            })?;
            if end > blob.len() as u64 {
                return Err(PakError::InvalidManifest(format!(
                    "entry out of range: {} ({end} > {})",
                    e.name,
                    blob.len()
                )));
            }
            if entries.contains_key(&name) {
                return Err(PakError::InvalidManifest(format!(
                    "duplicate entry: {}",
                    e.name
                )));
            }
            ranges.push((e.offset, end, name.clone()));
            entries.insert(
                name,
                PakEntry {
                    name: e.name.clone(),
                    offset: e.offset,
                    size: e.size,
                    mime: if e.mime.is_empty() {
                        None
                    } else {
                        Some(e.mime.clone())
                    },
                },
            );
        }
        ranges.sort_by_key(|(start, end, _)| (*start, *end));
        let mut previous_end = 0;
        for (start, end, name) in ranges {
            if start < previous_end && end > start {
                return Err(PakError::InvalidManifest(format!(
                    "overlapping byte range: {name}"
                )));
            }
            previous_end = previous_end.max(end);
        }

        Ok(Self { entries, blob })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&PakEntry> {
        self.entries.get(&normalize_entry_name(name))
    }

    pub fn read_entry(&self, name: &str) -> Option<Vec<u8>> {
        let entry = self.get(name)?;
        let start = usize::try_from(entry.offset).ok()?;
        let size = usize::try_from(entry.size).ok()?;
        let end = start.checked_add(size)?;
        if end <= self.blob.len() {
            Some(self.blob[start..end].to_vec())
        } else {
            None
        }
    }

    pub fn read_entry_str(&self, name: &str) -> Option<String> {
        let bytes = self.read_entry(name)?;
        String::from_utf8(bytes).ok()
    }

    pub fn entries(&self) -> impl Iterator<Item = &PakEntry> {
        self.entries.values()
    }
}

fn normalize_entry_name(name: &str) -> String {
    name.replace('\\', "/").to_lowercase()
}

#[derive(Debug, thiserror::Error)]
pub enum PakError {
    #[error("数据太短，无法解析 PAK 头部")]
    TooSmall,
    #[error("无效的 MPK1 魔数")]
    BadMagic,
    #[error("无效的 manifest JSON: {0}")]
    InvalidManifest(String),
    #[error("ZIP 解析错误: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("ZIP 读写错误: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    fn make_mpk1(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut manifest_entries = Vec::new();
        let mut blob = Vec::new();
        for (name, data) in entries {
            manifest_entries.push(MpkiEntry {
                name: name.to_string(),
                offset: blob.len() as u64,
                size: data.len() as u64,
                mime: String::new(),
            });
            blob.extend_from_slice(data);
        }

        let manifest = MpkiManifest {
            format_version: 1,
            kind: "test".into(),
            entries: manifest_entries,
        };

        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let manifest_len = manifest_bytes.len() as u32;

        let mut result = Vec::new();
        result.extend_from_slice(MPK1_MAGIC);
        result.extend_from_slice(&manifest_len.to_le_bytes());
        result.extend_from_slice(&manifest_bytes);
        result.extend_from_slice(&blob);
        result
    }

    #[test]
    fn test_mpk1_parse() {
        let data = make_mpk1(&[
            ("sn01/sn01.gmb", b"fake_gmb_data"),
            ("Common/character.amb", b"fake_amb"),
        ]);

        let pak = PakArchive::from_mpk1(&data).unwrap();
        assert_eq!(pak.len(), 2);

        assert_eq!(pak.read_entry("sn01/sn01.gmb").unwrap(), b"fake_gmb_data");
        assert_eq!(pak.read_entry("common/character.AMB").unwrap(), b"fake_amb");
    }

    #[test]
    fn current_web_off_len_schema_parses() {
        let payload = b"current-schema";
        let manifest = serde_json::json!({
            "formatVersion": 1,
            "packId": "test/current",
            "kind": "common",
            "requires": [],
            "covers": [],
            "entries": [{
                "name": "common/current.res",
                "off": 0,
                "len": payload.len(),
                "mime": "application/octet-stream"
            }]
        });
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MPK1_MAGIC);
        bytes.extend_from_slice(&(manifest.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&manifest);
        bytes.extend_from_slice(payload);

        let pak = PakArchive::from_mpk1(&bytes).expect("parse current Web MPK1 schema");
        assert_eq!(
            pak.read_entry("common/current.res").as_deref(),
            Some(payload.as_slice())
        );
    }

    #[test]
    fn current_web_pack_directory_parses_all_mpk_files() {
        let packs_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools/packs");
        let mut paths = std::fs::read_dir(&packs_dir)
            .expect("read asset-tools/packs")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|extension| extension == "mpk"))
            .collect::<Vec<_>>();
        paths.sort();
        let index: serde_json::Value = serde_json::from_slice(
            &std::fs::read(packs_dir.join("packs.json")).expect("read packs.json"),
        )
        .expect("parse packs.json");
        let expected_count = index
            .get("packs")
            .and_then(serde_json::Value::as_array)
            .expect("packs.json packs array")
            .len();
        assert_eq!(paths.len(), expected_count, "checked-in MPK/index mismatch");

        let mut entry_count = 0usize;
        for path in paths {
            let bytes = std::fs::read(&path).unwrap_or_else(|error| {
                panic!("read {}: {error}", path.display());
            });
            let pak = PakArchive::from_mpk1(&bytes).unwrap_or_else(|error| {
                panic!("parse {}: {error}", path.display());
            });
            entry_count += pak.len();
        }
        assert!(entry_count > 2_000, "unexpectedly sparse MPK set");
    }

    #[test]
    fn test_mpk1_bad_magic() {
        assert!(PakArchive::from_mpk1(b"BAD!").is_err());
    }

    #[test]
    fn test_mpk1_too_small() {
        assert!(PakArchive::from_mpk1(b"MP").is_err());
    }

    #[test]
    fn test_zip_reads_original_pak() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/sn01.pak");
        let data = std::fs::read(path).expect("original sn01.pak");
        let pak = PakArchive::from_bytes(&data).expect("valid ZIP PAK");
        assert!(!pak.is_empty());
        let first = pak.entries().next().expect("at least one file");
        assert!(pak.read_entry(&first.name).is_some());
    }

    #[test]
    fn test_zip_bad_data() {
        assert!(matches!(
            PakArchive::from_bytes(b"not a pak"),
            Err(PakError::BadMagic)
        ));
    }
}
