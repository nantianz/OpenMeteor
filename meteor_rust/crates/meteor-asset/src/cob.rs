use std::collections::BTreeSet;

pub const COB_SIGNATURE: &str = "COL V1.0";

const MIN_SIZE: usize = 62;

#[derive(Debug, Clone)]
pub struct CobGrid {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    pub cell_count: usize,

    pub face_ids: BTreeSet<u32>,
}

pub fn parse_cob(data: &[u8]) -> Result<CobGrid, CobError> {
    if data.len() < MIN_SIZE {
        return Err(CobError::TooSmall);
    }

    let signature = &data[0..8];
    if signature != COB_SIGNATURE.as_bytes() {
        return Err(CobError::BadSignature(
            String::from_utf8_lossy(signature).into_owned(),
        ));
    }

    let mut o = 10usize;

    let read_u32 = |o: &mut usize| -> Result<u32, CobError> {
        if *o + 4 > data.len() {
            return Err(CobError::Truncated { offset: *o });
        }
        let v = u32::from_le_bytes(data[*o..*o + 4].try_into().unwrap());
        *o += 4;
        Ok(v)
    };

    let nx = read_u32(&mut o)? as usize;
    let ny = read_u32(&mut o)? as usize;
    let nz = read_u32(&mut o)? as usize;
    let cell_count = read_u32(&mut o)? as usize;

    o += 9 * 4;

    if cell_count != nx * ny * nz {
        return Err(CobError::CellCountMismatch {
            declared: cell_count,
            computed: nx * ny * nz,
        });
    }

    let mut face_ids = BTreeSet::new();
    for cell in 0..cell_count {
        if o + 4 > data.len() {
            return Err(CobError::TruncatedAtCell { cell });
        }
        let count = i32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        o += 4;
        if count < 0 || o + count as usize * 4 > data.len() {
            return Err(CobError::InvalidFaceCount {
                cell,
                count,
                remaining: data.len().saturating_sub(o),
            });
        }
        for _ in 0..count {
            let id = u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
            face_ids.insert(id);
            o += 4;
        }
    }

    if o != data.len() {
        return Err(CobError::TrailingData {
            parsed: o,
            total: data.len(),
        });
    }

    Ok(CobGrid {
        nx,
        ny,
        nz,
        cell_count,
        face_ids,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum CobError {
    #[error("COB 文件太小 (< {MIN_SIZE} 字节)")]
    TooSmall,
    #[error("不支持的 COB 签名: {0:?}")]
    BadSignature(String),
    #[error("数据截断 (offset {offset})")]
    Truncated { offset: usize },
    #[error("COB cell 数不匹配: 声明 {declared} != nx*ny*nz={computed}")]
    CellCountMismatch { declared: usize, computed: usize },
    #[error("COB 在 cell {cell} 处截断")]
    TruncatedAtCell { cell: usize },
    #[error("COB cell {cell} 面数无效: count={count}, 剩余 {remaining} 字节")]
    InvalidFaceCount {
        cell: usize,
        count: i32,
        remaining: usize,
    },
    #[error("COB 尾部多余数据: 解析 {parsed} / 共 {total}")]
    TrailingData { parsed: usize, total: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cob() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"COL V1.0");
        d.extend_from_slice(&[0u8, 0]);

        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&2u32.to_le_bytes());

        for _ in 0..9 {
            d.extend_from_slice(&0f32.to_le_bytes());
        }

        d.extend_from_slice(&2i32.to_le_bytes());
        d.extend_from_slice(&5u32.to_le_bytes());
        d.extend_from_slice(&12u32.to_le_bytes());

        d.extend_from_slice(&1i32.to_le_bytes());
        d.extend_from_slice(&3u32.to_le_bytes());

        d
    }

    #[test]
    fn test_parse_basic() {
        let cob = parse_cob(&make_cob()).unwrap();
        assert_eq!(cob.nx, 2);
        assert_eq!(cob.ny, 1);
        assert_eq!(cob.nz, 1);
        assert_eq!(cob.cell_count, 2);

        assert_eq!(cob.face_ids.len(), 3);
        assert!(cob.face_ids.contains(&3));
        assert!(cob.face_ids.contains(&5));
        assert!(cob.face_ids.contains(&12));
    }

    #[test]
    fn test_bad_signature() {
        assert!(parse_cob(b"BAD SIGNATURE!!!!!").is_err());
    }

    #[test]
    fn test_too_small() {
        assert!(parse_cob(b"COL V1.0").is_err());
    }
}
