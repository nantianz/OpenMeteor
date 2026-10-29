use std::path::Path;

struct Edit {
    name: &'static str,
    find: &'static [u8],
    replace: &'static [u8],
}

const SITE_A: Edit = Edit {
    name: "arch-target",
    find: &[
        0x48, 0x8b, 0xd3, 0xc7, 0x44, 0x24, 0x3c, 0xb0, 0x01, 0x00, 0x00, 0xe8, 0xc3, 0x98, 0x04,
        0x00,
    ],
    replace: &[
        0x48, 0x8b, 0xd3, 0xc7, 0x44, 0x24, 0x3c, 0x90, 0x01, 0x00, 0x00, 0xe8, 0xc3, 0x98, 0x04,
        0x00,
    ],
};

const SITE_B: Edit = Edit {
    name: "arch-dispatch",
    find: &[0xeb, 0x1c, 0xbe, 0x90, 0x01, 0x00, 0x00, 0xeb, 0x15],
    replace: &[0xeb, 0x1c, 0x33, 0xf6, 0x90, 0x90, 0x90, 0xeb, 0x4a],
};

#[derive(Debug, Clone)]
pub struct PatchReport {
    pub offsets: Vec<(&'static str, usize)>,
}

#[derive(Debug)]
pub enum PatchError {
    NotFound(&'static str),

    Ambiguous(&'static str, usize),

    AlreadyPatched,
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchError::NotFound(s) => write!(
                f,
                "patch site '{s}' not found (unsupported or non-genuine DLL)"
            ),
            PatchError::Ambiguous(s, n) => {
                write!(f, "patch site '{s}' matched {n} times (ambiguous)")
            }
            PatchError::AlreadyPatched => write!(f, "DLL is already gate-patched"),
        }
    }
}
impl std::error::Error for PatchError {}

fn find_unique(hay: &[u8], needle: &[u8]) -> Result<Option<usize>, usize> {
    let mut found = None;
    let mut count = 0;
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            count += 1;
            found = Some(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    if count > 1 {
        return Err(count);
    }
    Ok(found)
}

pub fn patch_gate(bytes: &mut [u8]) -> Result<PatchReport, PatchError> {
    let a_done = find_unique(bytes, SITE_A.replace).ok().flatten().is_some();
    let b_done = find_unique(bytes, SITE_B.replace).ok().flatten().is_some();
    let a_orig = find_unique(bytes, SITE_A.find);
    let b_orig = find_unique(bytes, SITE_B.find);
    if a_done && b_done && matches!(a_orig, Ok(None)) && matches!(b_orig, Ok(None)) {
        return Err(PatchError::AlreadyPatched);
    }

    let mut offsets = Vec::new();
    for edit in [&SITE_A, &SITE_B] {
        match find_unique(bytes, edit.find) {
            Ok(Some(off)) => {
                bytes[off..off + edit.replace.len()].copy_from_slice(edit.replace);
                offsets.push((edit.name, off));
            }
            Ok(None) => return Err(PatchError::NotFound(edit.name)),
            Err(n) => return Err(PatchError::Ambiguous(edit.name, n)),
        }
    }
    Ok(PatchReport { offsets })
}

pub fn is_patched(bytes: &[u8]) -> bool {
    find_unique(bytes, SITE_A.find).ok().flatten().is_none()
        && find_unique(bytes, SITE_A.replace).ok().flatten().is_some()
}

pub fn patch_file(src: &Path, dst: &Path) -> Result<PatchReport, Box<dyn std::error::Error>> {
    let mut bytes = std::fs::read(src)?;
    let report = patch_gate(&mut bytes)?;
    std::fs::write(dst, &bytes)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_synthetic_image() {
        let mut buf = vec![0u8; 64];
        buf.extend_from_slice(SITE_A.find);
        buf.extend_from_slice(&[0u8; 32]);
        buf.extend_from_slice(SITE_B.find);
        buf.extend_from_slice(&[0u8; 16]);

        let report = patch_gate(&mut buf).expect("should patch");
        assert_eq!(report.offsets.len(), 2);
        assert!(is_patched(&buf));

        assert!(matches!(
            patch_gate(&mut buf),
            Err(PatchError::AlreadyPatched)
        ));
    }

    #[test]
    fn rejects_unrelated_bytes() {
        let mut buf = vec![0x90u8; 4096];
        assert!(matches!(patch_gate(&mut buf), Err(PatchError::NotFound(_))));
    }
}
