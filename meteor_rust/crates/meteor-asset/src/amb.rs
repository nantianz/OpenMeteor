pub const BANIM_MAGIC: &[u8; 5] = b"BANIM";

pub const BANIM_HEADER_SIZE: usize = 29;

#[derive(Debug, Clone)]
pub struct AmbAnimation {
    pub num_bones: usize,
    pub num_dummy: usize,
    pub num_frames: usize,

    pub stride: usize,

    data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct AmbFrame {
    pub root_pivot: [f32; 3],

    pub bone_quats: Vec<[f32; 4]>,

    pub dummy_pos: Vec<[f32; 3]>,

    pub dummy_quats: Vec<[f32; 4]>,
}

#[derive(Debug, Clone)]
pub struct BlendedFrame {
    pub root_pivot: [f32; 3],

    pub bone_quats: Vec<[f32; 4]>,

    pub dummy_pos: Vec<[f32; 3]>,

    pub dummy_quats: Vec<[f32; 4]>,

    pub weight: f32,
}

pub fn parse_amb(data: &[u8]) -> Result<AmbAnimation, AmbError> {
    if data.len() < BANIM_HEADER_SIZE {
        return Err(AmbError::TooSmall);
    }
    if &data[0..5] != BANIM_MAGIC {
        return Err(AmbError::BadMagic);
    }

    let num_bones = i32::from_le_bytes(data[5..9].try_into().unwrap()) as usize;
    let num_dummy = i32::from_le_bytes(data[9..13].try_into().unwrap()) as usize;
    let num_frames = i32::from_le_bytes(data[13..17].try_into().unwrap()) as usize;

    if num_bones == 0 || num_bones > 256 || num_dummy > 256 {
        return Err(AmbError::InvalidHeader);
    }

    let stride = 12 + num_bones * 16 + num_dummy * 33 + 8;
    let expected = BANIM_HEADER_SIZE + num_frames * stride;

    let bones_span = 12 + num_bones * 16;
    let min_required = BANIM_HEADER_SIZE + num_frames.saturating_sub(1) * stride + bones_span;
    if data.len() < min_required.min(expected) {
        return Err(AmbError::Truncated {
            expected,
            actual: data.len(),
        });
    }
    let available = data.len().min(expected);

    Ok(AmbAnimation {
        num_bones,
        num_dummy,
        num_frames,
        stride,
        data: data[BANIM_HEADER_SIZE..available].to_vec(),
    })
}

impl AmbAnimation {
    pub fn sample_position_at(&self, frame: f32, dummy: Option<usize>) -> [f32; 3] {
        let clamped = frame.clamp(0.0, self.num_frames.saturating_sub(1) as f32);
        let i0 = clamped.trunc() as usize;
        let weight = clamped - i0 as f32;
        let dummy = dummy.filter(|index| *index < self.num_dummy);
        let read = |index: usize| {
            let start = index * self.stride;
            let offset = if let Some(dummy) = dummy {
                let block = start + 12 + self.num_bones * 16 + dummy * 33;

                if block + 33 > self.data.len() {
                    return [0.0; 3];
                }
                block + 5
            } else {
                start
            };
            std::array::from_fn(|axis| {
                let offset = offset + axis * 4;
                f32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap())
            })
        };
        let a = read(i0);
        if weight < 1e-4 || i0 >= self.num_frames.saturating_sub(1) {
            return a;
        }
        let b = read(i0 + 1);
        std::array::from_fn(|axis| lerp(a[axis], b[axis], weight))
    }

    pub fn sample(&self, frame: usize) -> AmbFrame {
        let frame = frame.min(self.num_frames.saturating_sub(1));
        let offset = frame * self.stride;
        let mut o = offset;

        let root_pivot = [
            f32::from_le_bytes(self.data[o..o + 4].try_into().unwrap()),
            f32::from_le_bytes(self.data[o + 4..o + 8].try_into().unwrap()),
            f32::from_le_bytes(self.data[o + 8..o + 12].try_into().unwrap()),
        ];
        o += 12;

        let mut bone_quats = Vec::with_capacity(self.num_bones);
        for _ in 0..self.num_bones {
            let w = f32::from_le_bytes(self.data[o..o + 4].try_into().unwrap());
            let x = f32::from_le_bytes(self.data[o + 4..o + 8].try_into().unwrap());
            let y = f32::from_le_bytes(self.data[o + 8..o + 12].try_into().unwrap());
            let z = f32::from_le_bytes(self.data[o + 12..o + 16].try_into().unwrap());
            bone_quats.push([x, y, z, w]);
            o += 16;
        }

        let mut dummy_pos = Vec::with_capacity(self.num_dummy);
        let mut dummy_quats = Vec::with_capacity(self.num_dummy);
        for _ in 0..self.num_dummy {
            if o + 5 + 12 + 16 > self.data.len() {
                dummy_pos.push([0.0, 0.0, 0.0]);
                dummy_quats.push([0.0, 0.0, 0.0, 1.0]);
                o += 33;
                continue;
            }
            o += 5;
            let px = f32::from_le_bytes(self.data[o..o + 4].try_into().unwrap());
            let py = f32::from_le_bytes(self.data[o + 4..o + 8].try_into().unwrap());
            let pz = f32::from_le_bytes(self.data[o + 8..o + 12].try_into().unwrap());
            dummy_pos.push([px, py, pz]);
            o += 12;
            let w = f32::from_le_bytes(self.data[o..o + 4].try_into().unwrap());
            let x = f32::from_le_bytes(self.data[o + 4..o + 8].try_into().unwrap());
            let y = f32::from_le_bytes(self.data[o + 8..o + 12].try_into().unwrap());
            let z = f32::from_le_bytes(self.data[o + 12..o + 16].try_into().unwrap());
            dummy_quats.push([x, y, z, w]);
            o += 16;
        }

        AmbFrame {
            root_pivot,
            bone_quats,
            dummy_pos,
            dummy_quats,
        }
    }

    pub fn sample_at(&self, frame: f32) -> BlendedFrame {
        let clamped = frame.clamp(0.0, (self.num_frames.saturating_sub(1)) as f32);
        let i0 = clamped.trunc() as usize;
        let w = clamped - i0 as f32;

        let f0 = self.sample(i0);
        if w < 1e-4 || i0 >= self.num_frames.saturating_sub(1) {
            return BlendedFrame {
                root_pivot: f0.root_pivot,
                bone_quats: f0.bone_quats,
                dummy_pos: f0.dummy_pos,
                dummy_quats: f0.dummy_quats,
                weight: 0.0,
            };
        }

        let f1 = self.sample(i0 + 1);
        let root_pivot = [
            lerp(f0.root_pivot[0], f1.root_pivot[0], w),
            lerp(f0.root_pivot[1], f1.root_pivot[1], w),
            lerp(f0.root_pivot[2], f1.root_pivot[2], w),
        ];

        let bone_quats = slerp_quats(&f0.bone_quats, &f1.bone_quats, w);
        let dummy_pos = lerp_vec3s(&f0.dummy_pos, &f1.dummy_pos, w);
        let dummy_quats = slerp_quats(&f0.dummy_quats, &f1.dummy_quats, w);

        BlendedFrame {
            root_pivot,
            bone_quats,
            dummy_pos,
            dummy_quats,
            weight: w,
        }
    }
}

fn slerp_quats(a: &[[f32; 4]], b: &[[f32; 4]], t: f32) -> Vec<[f32; 4]> {
    a.iter()
        .zip(b.iter())
        .map(|(q0, q1)| {
            let qa = meteor_core::Quat {
                x: q0[0],
                y: q0[1],
                z: q0[2],
                w: q0[3],
            };
            let qb = meteor_core::Quat {
                x: q1[0],
                y: q1[1],
                z: q1[2],
                w: q1[3],
            };
            let r = qa.slerp(&qb, t);
            [r.x, r.y, r.z, r.w]
        })
        .collect()
}

fn lerp_vec3s(a: &[[f32; 3]], b: &[[f32; 3]], t: f32) -> Vec<[f32; 3]> {
    a.iter()
        .zip(b.iter())
        .map(|(p0, p1)| {
            [
                lerp(p0[0], p1[0], t),
                lerp(p0[1], p1[1], t),
                lerp(p0[2], p1[2], t),
            ]
        })
        .collect()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[derive(Debug, thiserror::Error)]
pub enum AmbError {
    #[error("数据太短，无法解析 BANIM 头部")]
    TooSmall,
    #[error("无效的 BANIM 魔数")]
    BadMagic,
    #[error("无效的头部字段 (numBones/numDummy 超出范围)")]
    InvalidHeader,
    #[error("数据截断: 需要 {expected} 字节, 实际 {actual}")]
    Truncated { expected: usize, actual: usize },
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    fn make_banim() -> Vec<u8> {
        let num_bones: i32 = 2;
        let num_dummy: i32 = 0;
        let num_frames: i32 = 3;

        let mut data = Vec::new();
        data.extend_from_slice(BANIM_MAGIC);
        data.extend_from_slice(&num_bones.to_le_bytes());
        data.extend_from_slice(&num_dummy.to_le_bytes());
        data.extend_from_slice(&num_frames.to_le_bytes());
        data.extend_from_slice(&num_bones.to_le_bytes());
        data.extend_from_slice(&(-1i32).to_le_bytes());
        data.extend_from_slice(&(0i32).to_le_bytes());

        for frame in 0..num_frames {
            let base = frame as f32;
            data.extend_from_slice(&(1.0 + base).to_le_bytes());
            data.extend_from_slice(&(2.0 + base).to_le_bytes());
            data.extend_from_slice(&(3.0 + base).to_le_bytes());

            data.extend_from_slice(&1.0f32.to_le_bytes());
            data.extend_from_slice(&0.0f32.to_le_bytes());
            data.extend_from_slice(&0.0f32.to_le_bytes());
            data.extend_from_slice(&0.0f32.to_le_bytes());

            data.extend_from_slice(&0.707f32.to_le_bytes());
            data.extend_from_slice(&0.0f32.to_le_bytes());
            data.extend_from_slice(&0.0f32.to_le_bytes());
            data.extend_from_slice(&0.707f32.to_le_bytes());

            data.extend_from_slice(&[0u8; 8]);
        }
        data
    }

    #[test]
    fn test_parse_header() {
        let amb = parse_amb(&make_banim()).unwrap();
        assert_eq!(amb.num_bones, 2);
        assert_eq!(amb.num_frames, 3);
        assert_eq!(amb.stride, 12 + 2 * 16 + 8);
    }

    #[test]
    fn test_sample_frame() {
        let amb = parse_amb(&make_banim()).unwrap();
        let f = amb.sample(0);
        assert_eq!(f.root_pivot, [1.0, 2.0, 3.0]);
        assert_eq!(f.bone_quats[0], [0.0, 0.0, 0.0, 1.0]);

        let b1 = f.bone_quats[1];
        assert!((b1[2] - 0.707).abs() < 0.001);
        assert!((b1[3] - 0.707).abs() < 0.001);
    }

    #[test]
    fn test_sample_clamped() {
        let amb = parse_amb(&make_banim()).unwrap();
        let f = amb.sample(100);
        assert_eq!(f.root_pivot, [3.0, 4.0, 5.0]);
    }

    #[test]
    fn test_bad_magic() {
        assert!(parse_amb(b"XXXXX000000000000000000000000").is_err());
    }

    #[test]
    fn position_sampling_matches_full_sampling_at_boundaries() {
        let amb = parse_amb(&make_banim()).unwrap();
        for frame in [-4.0, 0.0, 0.00001, 0.0001, 0.25, 1.5, 2.0, 999.0] {
            for dummy in [None, Some(0), Some(usize::MAX)] {
                assert_eq!(
                    amb.sample_position_at(frame, dummy),
                    amb.sample_at(frame).root_pivot
                );
            }
        }
    }

    #[test]
    fn position_sampling_matches_original_animation_and_truncated_dummy() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        for path in ["Common/character.amb", "pmodel/p0.amb", "pmodel/p1.amb"] {
            let bytes = std::fs::read(root.join(path)).expect("original AMB required");
            let mut amb = parse_amb(&bytes).unwrap();

            for i in 0..amb.num_frames {
                for fraction in [0.0, 0.375] {
                    let frame = i as f32 + fraction;
                    let full = amb.sample_at(frame);
                    assert_eq!(amb.sample_position_at(frame, None), full.root_pivot);
                    for (dummy, position) in full.dummy_pos.iter().enumerate() {
                        assert_eq!(amb.sample_position_at(frame, Some(dummy)), *position);
                    }
                }
            }

            let last = amb.num_frames - 1;
            amb.data
                .truncate(last * amb.stride + 12 + amb.num_bones * 16 + 5 + 12);
            for frame in [last as f32 - 0.5, last as f32, last as f32 + 10.0] {
                let full = amb.sample_at(frame);
                for (dummy, position) in full.dummy_pos.iter().enumerate() {
                    assert_eq!(amb.sample_position_at(frame, Some(dummy)), *position);
                }
            }
        }
    }

    #[test]
    #[ignore = "manual CPU benchmark; run optimized with --ignored --nocapture"]
    fn benchmark_motion_position_sampling() {
        use std::{hint::black_box, time::Instant};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let amb = parse_amb(&std::fs::read(root.join("Common/character.amb")).unwrap()).unwrap();
        let iterations = 100_000;
        for position_only in [false, true] {
            let start = Instant::now();
            for i in 0..iterations {
                let frame = black_box((i % (amb.num_frames - 1)) as f32 + 0.375);
                let position = if position_only {
                    amb.sample_position_at(frame, Some(0))
                } else {
                    amb.sample_at(frame).dummy_pos[0]
                };
                black_box(position);
            }
            eprintln!(
                "motion position_only={position_only} samples={iterations} elapsed_us={}",
                start.elapsed().as_micros()
            );
        }
    }
}
