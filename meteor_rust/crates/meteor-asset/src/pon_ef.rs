use meteor_core::{Mat4, Quat, Vec3};

pub const TYPE_AUDIO: u8 = 2;

pub const PON_EF_SIGNATURE: &[u8; 11] = b"PonEF V1.00";
pub const TYPE_BOX: u8 = 3;
pub const TYPE_BILLBOARD: u8 = 4;
pub const TYPE_CYLINDER: u8 = 5;
pub const TYPE_DRAG: u8 = 6;
pub const TYPE_DONUT: u8 = 7;
pub const TYPE_MODEL: u8 = 8;
pub const TYPE_PARTICLE: u8 = 9;
pub const TYPE_PATH: u8 = 10;
pub const TYPE_PLANE: u8 = 11;
pub const TYPE_SPHERE: u8 = 12;

pub const HEADER_TWO_SIDED: usize = 0x41;
pub const HEADER_HIDDEN: usize = 0x45;
pub const HEADER_BLEND_MODE: usize = 0x51;
pub const HEADER_BLEND_SRC: usize = 0x55;
pub const HEADER_BLEND_DST: usize = 0x59;

const CUSTOM_BLEND_REMAP: [u8; 10] = [1, 0, 2, 4, 6, 7, 3, 5, 8, 9];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PonEfRenderState {
    pub hidden: bool,
    pub two_sided: bool,
    pub blend_mode: u32,
    pub blend_src_slot: u32,
    pub blend_dst_slot: u32,
    pub blend_enabled: bool,
    pub blend_src: u8,
    pub blend_dst: u8,
}

pub fn decode_render_state(header: &[u8]) -> PonEfRenderState {
    let mode = read_u32(header, HEADER_BLEND_MODE).unwrap_or(0);
    let src_slot = read_u32(header, HEADER_BLEND_SRC).unwrap_or(0);
    let dst_slot = read_u32(header, HEADER_BLEND_DST).unwrap_or(0);

    let (enabled, src, dst) = match mode {
        0 => (false, 1u8, 0u8),
        1 => (true, 2u8, 1u8),
        2 => (true, 0u8, 4u8),
        3 => (true, 6u8, 7u8),
        4 => (
            true,
            CUSTOM_BLEND_REMAP
                .get(src_slot as usize)
                .copied()
                .unwrap_or(6),
            CUSTOM_BLEND_REMAP
                .get(dst_slot as usize)
                .copied()
                .unwrap_or(1),
        ),
        _ => (mode != 0, 6u8, 1u8),
    };
    PonEfRenderState {
        hidden: read_u32(header, HEADER_HIDDEN).unwrap_or(0) != 0,
        two_sided: read_u32(header, HEADER_TWO_SIDED).unwrap_or(0) != 0,
        blend_mode: mode,
        blend_src_slot: src_slot,
        blend_dst_slot: dst_slot,
        blend_enabled: enabled,
        blend_src: src,
        blend_dst: dst,
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

#[derive(Debug, Clone)]
pub struct PonEfKeyframe {
    pub time: f32,
    pub position: [f32; 3],
    pub quaternion: [f32; 4],
    pub scale: [f32; 3],
    pub render: [f32; 17],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PonEfParticleParams {
    pub count: u32,

    pub size_min: f32,
    pub size_max: f32,

    pub stretch: f32,

    pub life_min: f32,
    pub life_max: f32,

    pub speed_min: f32,
    pub speed_max: f32,

    pub dir_min: [f32; 3],
    pub dir_max: [f32; 3],

    pub one_shot: bool,

    pub speed_decay: bool,
}

pub fn decode_pon_ef_particle(parameters: &[u8]) -> PonEfParticleParams {
    let f = |offset: usize, fallback: f32| -> f32 {
        parameters
            .get(offset..offset + 4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .filter(|v| v.is_finite())
            .unwrap_or(fallback)
    };
    let u = |offset: usize| -> u32 {
        parameters
            .get(offset..offset + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .unwrap_or(0)
    };
    let life_a = f(24, 0.2);
    let life_b = f(28, 0.5);
    let speed_a = f(32, 0.0);
    let speed_b = f(36, 0.0);
    PonEfParticleParams {
        count: u(0),
        size_min: f(4, 8.0),
        size_max: f(8, 15.0),
        stretch: f(20, 0.0),
        life_min: life_a.min(life_b).max(0.01),
        life_max: life_a.max(life_b).max(0.01),
        speed_min: speed_a.min(speed_b),
        speed_max: speed_a.max(speed_b),
        dir_min: [f(48, -1.0), f(52, -1.0), f(56, 0.0)],
        dir_max: [f(60, 1.0), f(64, 1.0), f(68, 0.1)],
        one_shot: u(72) != 0,
        speed_decay: u(76) != 0,
    }
}

#[derive(Debug, Clone)]
pub enum PonEfTail {
    Audio {
        sound: String,
        value: u32,
    },

    Particle {
        parameters: [u8; 96],
        strings: [String; 3],
    },

    Model {
        model: String,
    },

    Drag {
        value: f32,
        anchors: [String; 2],
    },

    Floats(Vec<f32>),
}

#[derive(Debug, Clone)]
pub struct PonEfNode {
    pub index: usize,

    pub type_name: String,
    pub type_id: u8,
    pub name: String,
    pub values: [u32; 4],

    pub parents: [String; 2],
    pub texture: String,

    pub transform_header: [u8; 93],
    pub keyframes: Vec<PonEfKeyframe>,
    pub blob: Vec<u8>,
    pub tail: PonEfTail,
    pub start_time: f32,
    pub end_time: f32,
}

impl PonEfNode {
    pub fn half_extents(&self) -> Option<[f32; 3]> {
        let values = match &self.tail {
            PonEfTail::Floats(values) => values,
            _ => return None,
        };
        let n = |i: usize| {
            values
                .get(i)
                .copied()
                .filter(|v| v.is_finite())
                .unwrap_or(0.0)
        };
        match self.type_id {
            TYPE_SPHERE => {
                let r = n(0);
                Some([r, r, r])
            }
            TYPE_CYLINDER => {
                let r = n(0).max(n(1));
                Some([r, r, n(2) / 2.0])
            }
            TYPE_BOX => Some([n(0) / 2.0, n(1) / 2.0, n(2) / 2.0]),

            TYPE_PLANE | TYPE_BILLBOARD => Some([n(0) / 2.0, n(1) / 2.0, 3.0]),
            TYPE_DONUT => Some([n(1), n(1), (n(1) - n(0)) / 2.0]),
            _ => None,
        }
    }

    pub fn is_attack_node(&self) -> bool {
        let first = self.name.as_bytes().first().copied().unwrap_or(0);
        if first != b'a' && first != b'A' {
            return false;
        }
        if self.type_id == TYPE_AUDIO || self.type_id == TYPE_PATH {
            return false;
        }
        self.half_extents().is_some()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PonEfSample {
    pub position: [f32; 3],
    pub quaternion: [f32; 4],
    pub scale: [f32; 3],

    pub render: [f32; 17],
}

fn slerp(a: [f32; 4], b: [f32; 4], weight: f32) -> [f32; 4] {
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let mut b = b;
    if dot < 0.0 {
        dot = -dot;
        for value in &mut b {
            *value = -*value;
        }
    }
    if dot > 0.9995 {
        let mut out = [
            a[0] + (b[0] - a[0]) * weight,
            a[1] + (b[1] - a[1]) * weight,
            a[2] + (b[2] - a[2]) * weight,
            a[3] + (b[3] - a[3]) * weight,
        ];
        let len = (out[0] * out[0] + out[1] * out[1] + out[2] * out[2] + out[3] * out[3]).sqrt();
        for value in &mut out {
            *value /= len;
        }
        return out;
    }
    let theta = dot.clamp(-1.0, 1.0).acos();
    let sin_theta = theta.sin();
    if sin_theta.abs() < 1e-6 {
        return a;
    }
    let wa = ((1.0 - weight) * theta).sin() / sin_theta;
    let wb = (weight * theta).sin() / sin_theta;
    [
        a[0] * wa + b[0] * wb,
        a[1] * wa + b[1] * wb,
        a[2] * wa + b[2] * wb,
        a[3] * wa + b[3] * wb,
    ]
}

fn lerp(a: f32, b: f32, weight: f32) -> f32 {
    a + (b - a) * weight
}

pub fn sample_transform(keyframes: &[PonEfKeyframe], time: f32) -> PonEfSample {
    let fallback = PonEfSample {
        position: [0.0, 0.0, 0.0],
        quaternion: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
        render: [0.0; 17],
    };
    let Some(first) = keyframes.first() else {
        return fallback;
    };
    let Some(last) = keyframes.last() else {
        return fallback;
    };
    let take = |k: &PonEfKeyframe| PonEfSample {
        position: k.position,
        quaternion: k.quaternion,
        scale: k.scale,
        render: k.render,
    };
    if time <= first.time || keyframes.len() == 1 {
        return take(first);
    }
    if time >= last.time {
        return take(last);
    }
    let mut upper_index = 1usize;
    while upper_index < keyframes.len() && keyframes[upper_index].time < time {
        upper_index += 1;
    }
    let lower = &keyframes[upper_index - 1];
    let upper = &keyframes[upper_index];
    let duration = upper.time - lower.time;
    let weight = if duration > 1e-8 {
        (time - lower.time) / duration
    } else {
        0.0
    };
    let pos = |i: usize| lerp(lower.position[i], upper.position[i], weight);
    let scl = |i: usize| lerp(lower.scale[i], upper.scale[i], weight);
    let render = std::array::from_fn(|i| lerp(lower.render[i], upper.render[i], weight));
    PonEfSample {
        position: [pos(0), pos(1), pos(2)],
        quaternion: slerp(lower.quaternion, upper.quaternion, weight),
        scale: [scl(0), scl(1), scl(2)],
        render,
    }
}

#[derive(Debug, Clone, Default)]
pub struct PonEf {
    pub signature: String,
    pub count: usize,
    pub nodes: Vec<PonEfNode>,
    pub byte_length: usize,
}

pub fn parse_pon_ef(input: &[u8]) -> Result<PonEf, String> {
    let mut reader = PonEfReader {
        bytes: input,
        offset: 0,
    };
    let signature = reader.raw(PON_EF_SIGNATURE.len())?;
    if signature != PON_EF_SIGNATURE {
        return Err(format!(
            "unsupported PonEF signature {:?}",
            String::from_utf8_lossy(signature)
        ));
    }
    let count = reader.u32()? as usize;
    let mut nodes = Vec::with_capacity(count);
    for index in 0..count {
        let type_raw = reader.raw(16)?;
        let type_name = String::from_utf8_lossy(type_raw)
            .trim_end_matches('\0')
            .to_string();
        let type_id = type_id_for_name(&type_name).ok_or_else(|| {
            format!("unsupported PonEF record type {type_name:?} at node {index}")
        })?;
        let name = reader.string()?;
        let values = [reader.u32()?, reader.u32()?, reader.u32()?, reader.u32()?];
        let parents = [reader.string()?, reader.string()?];
        let texture = reader.string()?;
        let header = reader.raw(93)?;
        let mut transform_header = [0u8; 93];
        transform_header.copy_from_slice(header);
        let key_count = reader.u32()? as usize;
        let mut keyframes = Vec::with_capacity(key_count);
        for _ in 0..key_count {
            keyframes.push(reader.keyframe()?);
        }
        let blob_size = reader.u32()? as usize;
        let blob = reader.raw(blob_size)?.to_vec();
        let tail = reader.tail(type_id, &type_name, index)?;
        let start_time = keyframes.first().map(|k| k.time).unwrap_or(0.0);
        let end_time = keyframes.last().map(|k| k.time).unwrap_or(0.0);
        nodes.push(PonEfNode {
            index,
            type_name,
            type_id,
            name,
            values,
            parents,
            texture,
            transform_header,
            keyframes,
            blob,
            tail,
            start_time,
            end_time,
        });
    }
    if reader.offset != input.len() {
        return Err(format!(
            "PonEF has {} trailing bytes at 0x{:x}",
            input.len() - reader.offset,
            reader.offset
        ));
    }
    Ok(PonEf {
        signature: String::from_utf8_lossy(signature).to_string(),
        count,
        nodes,
        byte_length: input.len(),
    })
}

fn type_id_for_name(name: &str) -> Option<u8> {
    Some(match name {
        "AUDIO" => TYPE_AUDIO,
        "BOX" => TYPE_BOX,
        "BILLBOARD" => TYPE_BILLBOARD,
        "CYLINDER" => TYPE_CYLINDER,
        "DRAG" => TYPE_DRAG,
        "DONUT" => TYPE_DONUT,
        "MODEL" => TYPE_MODEL,
        "PARTICLE" => TYPE_PARTICLE,
        "PATH" => TYPE_PATH,
        "PLANE" => TYPE_PLANE,
        "SPHERE" => TYPE_SPHERE,
        _ => return None,
    })
}

struct PonEfReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> PonEfReader<'a> {
    fn ensure(&self, size: usize, label: &str) -> Result<(), String> {
        if self.offset + size > self.bytes.len() {
            Err(format!(
                "PonEF truncated while reading {label} at 0x{:x}",
                self.offset
            ))
        } else {
            Ok(())
        }
    }

    fn raw(&mut self, size: usize) -> Result<&'a [u8], String> {
        self.ensure(size, "bytes")?;
        let value = &self.bytes[self.offset..self.offset + size];
        self.offset += size;
        Ok(value)
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.raw(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn f32(&mut self) -> Result<f32, String> {
        let bytes = self.raw(4)?;
        Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn string(&mut self) -> Result<String, String> {
        let length = self.u32()? as usize;
        let bytes = self.raw(length)?;
        Ok(String::from_utf8_lossy(bytes).to_string())
    }

    fn keyframe(&mut self) -> Result<PonEfKeyframe, String> {
        let mut floats = [0.0f32; 28];
        for value in &mut floats {
            *value = self.f32()?;
        }
        Ok(PonEfKeyframe {
            time: floats[0],
            position: [floats[1], floats[2], floats[3]],
            quaternion: [floats[4], floats[5], floats[6], floats[7]],
            scale: [floats[8], floats[9], floats[10]],
            render: floats[11..].try_into().unwrap(),
        })
    }

    fn tail(
        &mut self,
        type_id: u8,
        type_name: &str,
        node_index: usize,
    ) -> Result<PonEfTail, String> {
        let label = |what: &str| format!("node {node_index} {type_name} {what}");
        match type_id {
            TYPE_AUDIO => Ok(PonEfTail::Audio {
                sound: self.string()?,
                value: self.u32()?,
            }),
            TYPE_PARTICLE => {
                let raw = self.raw(96)?;
                let mut parameters = [0u8; 96];
                parameters.copy_from_slice(raw);
                Ok(PonEfTail::Particle {
                    parameters,
                    strings: [self.string()?, self.string()?, self.string()?],
                })
            }
            TYPE_MODEL => Ok(PonEfTail::Model {
                model: self.string()?,
            }),
            TYPE_DRAG => Ok(PonEfTail::Drag {
                value: self.f32()?,
                anchors: [self.string()?, self.string()?],
            }),
            _ => {
                let count = match type_id {
                    TYPE_BOX => 3,
                    TYPE_BILLBOARD => 2,
                    TYPE_CYLINDER => 5,
                    TYPE_DONUT => 6,
                    TYPE_PATH => 0,
                    TYPE_PLANE => 2,
                    TYPE_SPHERE => 5,
                    _ => return Err(format!("unsupported {}", label("tail"))),
                };
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(self.f32().map_err(|e| format!("{e} ({})", label("tail")))?);
                }
                Ok(PonEfTail::Floats(values))
            }
        }
    }
}

pub const PON_EF_SCALE: f32 = 0.12;

pub fn resolve_parent_matrix(
    ef: &PonEf,
    name: &str,
    time: f32,
    memo: &mut std::collections::HashMap<usize, Option<Mat4>>,
    visiting: &mut std::collections::HashSet<usize>,
    resolve_external: &mut dyn FnMut(&str) -> Option<Mat4>,
) -> Option<Mat4> {
    if name.is_empty() {
        return None;
    }
    if let Some(index) = ef.nodes.iter().position(|n| n.name == name).or_else(|| {
        ef.nodes
            .iter()
            .position(|n| n.name.eq_ignore_ascii_case(name))
    }) {
        return node_world_matrix(ef, index, time, memo, visiting, resolve_external);
    }
    resolve_external(name)
}

pub fn node_world_matrix(
    ef: &PonEf,
    node_index: usize,
    time: f32,
    memo: &mut std::collections::HashMap<usize, Option<Mat4>>,
    visiting: &mut std::collections::HashSet<usize>,
    resolve_external: &mut dyn FnMut(&str) -> Option<Mat4>,
) -> Option<Mat4> {
    if let Some(cached) = memo.get(&node_index) {
        return *cached;
    }
    if !visiting.insert(node_index) {
        return None;
    }
    let node = ef.nodes.get(node_index)?;
    let sample = sample_transform(&node.keyframes, time);
    let fallback = resolve_external("Character").unwrap_or_else(Mat4::identity);
    let basis = resolve_parent_matrix(ef, &node.parents[1], time, memo, visiting, resolve_external)
        .unwrap_or(fallback);
    let anchor =
        resolve_parent_matrix(ef, &node.parents[0], time, memo, visiting, resolve_external)
            .unwrap_or(fallback);
    let mut base = basis;
    base.set_translation(anchor.translation());
    let world_axis_offset = node.values[3] == 0;
    let local = Mat4::compose(
        if world_axis_offset {
            Vec3::ZERO
        } else {
            Vec3::new(sample.position[0], sample.position[1], sample.position[2])
        },
        Quat {
            x: sample.quaternion[0],
            y: sample.quaternion[1],
            z: sample.quaternion[2],
            w: sample.quaternion[3],
        }
        .normalize(),
        Vec3::new(sample.scale[0], sample.scale[1], sample.scale[2]),
    );
    let mut world = base.mul(&local);
    if world_axis_offset {
        world.m[12] += sample.position[0] * PON_EF_SCALE;
        world.m[13] += sample.position[2] * PON_EF_SCALE;
        world.m[14] -= sample.position[1] * PON_EF_SCALE;
    }
    visiting.remove(&node_index);
    memo.insert(node_index, Some(world));
    Some(world)
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_matrix_parent_basis_applies_rotation_and_scale_to_offset() {
        let mut raw = test_two_node_ef();
        raw.nodes[1].values[3] = 1;
        raw.nodes[1].parents = ["Parent".into(), "Parent".into()];
        let mut memo = std::collections::HashMap::new();
        let mut visiting = std::collections::HashSet::new();
        let mut no_external = |_: &str| None;
        let matrix = node_world_matrix(&raw, 1, 0.0, &mut memo, &mut visiting, &mut no_external)
            .expect("child resolves");
        assert!(
            (matrix.m[12] - 5.0 * 0.12).abs() < 1e-4,
            "x = parent world-axis translation"
        );
        assert!((matrix.m[13] - 2.0).abs() < 1e-4, "y = 1×2 rotated 90°");
    }

    #[test]
    fn node_matrix_world_axis_offset_when_values3_zero() {
        let mut raw = test_two_node_ef();
        raw.nodes[1].values[3] = 0;
        raw.nodes[1].parents = [String::new(), String::new()];
        raw.nodes[1].keyframes[0].position = [4.421, 0.0, 21.263];
        let mut memo = std::collections::HashMap::new();
        let mut visiting = std::collections::HashSet::new();
        let mut no_external = |_: &str| None;
        let matrix = node_world_matrix(&raw, 1, 0.0, &mut memo, &mut visiting, &mut no_external)
            .expect("child resolves");
        assert!((matrix.m[12] - 4.421 * 0.12).abs() < 1e-4, "world x");
        assert!((matrix.m[13] - 21.263 * 0.12).abs() < 1e-4, "world z→y");
        assert!((matrix.m[14]).abs() < 1e-5, "-y×0.12 = 0");
    }

    #[test]
    fn node_matrix_external_anchor_resolves() {
        let ef = test_two_node_ef();
        let mut memo = std::collections::HashMap::new();
        let mut visiting = std::collections::HashSet::new();
        let mut external = |name: &str| {
            if name == "Character" {
                Some(Mat4::from_translation(Vec3::new(10.0, 20.0, 30.0)))
            } else {
                None
            }
        };

        let mut ef2 = ef;
        ef2.nodes[1].parents[0] = "Character".to_string();
        ef2.nodes[1].parents[1] = String::new();
        ef2.nodes[1].keyframes[0].position = [0.0, 0.0, 0.0];
        let matrix = node_world_matrix(&ef2, 1, 0.0, &mut memo, &mut visiting, &mut external)
            .expect("child resolves");
        assert!(
            (matrix.m[12] - 10.0).abs() < 1e-4,
            "anchor x from Character"
        );
        assert!((matrix.m[13] - 20.0).abs() < 1e-4, "anchor y");
        assert!((matrix.m[14] - 30.0).abs() < 1e-4, "anchor z");
    }

    fn test_two_node_ef() -> PonEf {
        let parent = PonEfNode {
            index: 0,
            type_name: "BOX".into(),
            type_id: TYPE_BOX,
            name: "Parent".into(),
            values: [0; 4],
            parents: [String::new(), String::new()],
            texture: String::new(),
            transform_header: [0u8; 93],
            keyframes: vec![PonEfKeyframe {
                time: 0.0,
                position: [5.0, 0.0, 0.0],
                quaternion: [0.0, 0.0, 0.70710677, 0.70710677],
                scale: [2.0, 2.0, 2.0],
                render: [0.0; 17],
            }],
            blob: Vec::new(),
            tail: PonEfTail::Floats(vec![1.0, 1.0, 1.0]),
            start_time: 0.0,
            end_time: 1.0,
        };
        let child = PonEfNode {
            index: 1,
            type_name: "BOX".into(),
            type_id: TYPE_BOX,
            name: "Child".into(),
            values: [0; 4],
            parents: ["Anchor".into(), "Parent".into()],
            texture: String::new(),
            transform_header: [0u8; 93],
            keyframes: vec![PonEfKeyframe {
                time: 0.0,
                position: [1.0, 0.0, 0.0],
                quaternion: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                render: [0.0; 17],
            }],
            blob: Vec::new(),
            tail: PonEfTail::Floats(vec![1.0, 1.0, 1.0]),
            start_time: 0.0,
            end_time: 1.0,
        };
        PonEf {
            signature: String::new(),
            count: 2,
            nodes: vec![parent, child],
            byte_length: 0,
        }
    }

    #[test]
    fn decode_particle_params_matches_web_layout() {
        let mut raw = [0u8; 96];
        let put_u32 = |raw: &mut [u8; 96], offset: usize, v: u32| {
            raw[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
        };
        let put_f32 = |raw: &mut [u8; 96], offset: usize, v: f32| {
            raw[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
        };
        put_u32(&mut raw, 0, 40);
        put_f32(&mut raw, 4, 2.0);
        put_f32(&mut raw, 8, 6.0);
        put_f32(&mut raw, 24, 1.0);
        put_f32(&mut raw, 28, 1.0);
        put_f32(&mut raw, 32, 150.0);
        put_f32(&mut raw, 36, 250.0);
        put_f32(&mut raw, 48, -1.0);
        put_f32(&mut raw, 52, -1.0);
        put_f32(&mut raw, 56, 0.0);
        put_f32(&mut raw, 60, 1.0);
        put_f32(&mut raw, 64, 1.0);
        put_f32(&mut raw, 68, 0.1);
        put_u32(&mut raw, 72, 1);
        put_u32(&mut raw, 76, 1);
        let p = decode_pon_ef_particle(&raw);
        assert_eq!(p.count, 40);
        assert_eq!(p.size_min, 2.0);
        assert_eq!(p.size_max, 6.0);
        assert_eq!((p.life_min, p.life_max), (1.0, 1.0));
        assert_eq!((p.speed_min, p.speed_max), (150.0, 250.0));
        assert_eq!(p.dir_min, [-1.0, -1.0, 0.0]);
        assert!((p.dir_max[2] - 0.1).abs() < 1e-6);
        assert!(p.one_shot);
        assert!(p.speed_decay);
    }

    #[test]
    fn decode_particle_params_sorts_and_falls_back() {
        let mut raw = [0u8; 96];
        let put_f32 = |raw: &mut [u8; 96], offset: usize, v: f32| {
            raw[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
        };
        put_f32(&mut raw, 24, 1.0);
        put_f32(&mut raw, 28, 0.1);
        put_f32(&mut raw, 32, 300.0);
        put_f32(&mut raw, 36, 50.0);
        let p = decode_pon_ef_particle(&raw);
        assert_eq!((p.life_min, p.life_max), (0.1, 1.0));
        assert_eq!((p.speed_min, p.speed_max), (50.0, 300.0));

        let d = decode_pon_ef_particle(&[0u8; 96]);
        assert_eq!(d.count, 0);
        assert_eq!((d.size_min, d.size_max), (0.0, 0.0));
        assert_eq!((d.life_min, d.life_max), (0.01, 0.01));
        assert_eq!((d.speed_min, d.speed_max), (0.0, 0.0));
        assert_eq!(d.dir_min, [0.0, 0.0, 0.0]);
        assert!(!d.one_shot);
    }

    #[test]
    fn origin_res_particle_tails_decode() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("SFX/pos305.ef").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let ef = parse_pon_ef(&std::fs::read(root.join("SFX/pos305.ef")).unwrap()).unwrap();
        let particle = ef
            .nodes
            .iter()
            .find(|n| n.name == "Particle_8")
            .expect("pos305 Particle_8");
        let params = match &particle.tail {
            PonEfTail::Particle { parameters, .. } => decode_pon_ef_particle(parameters),
            other => panic!("expected particle tail, got {other:?}"),
        };
        assert_eq!(params.count, 40);
        assert_eq!((params.life_min, params.life_max), (1.0, 1.0));
        assert_eq!((params.speed_min, params.speed_max), (150.0, 250.0));
        assert_eq!(params.dir_min, [-1.0, -1.0, 0.0]);
        assert!((params.dir_max[2] - 0.1).abs() < 1e-6);
        assert!(params.one_shot);
        assert!(params.speed_decay);
        assert!(params.size_min > 0.0 && params.size_max >= params.size_min);

        let pois = parse_pon_ef(&std::fs::read(root.join("SFX/AttrPOIS.ef")).unwrap()).unwrap();
        let p = pois
            .nodes
            .iter()
            .find(|n| n.type_id == TYPE_PARTICLE)
            .expect("AttrPOIS particle");
        let params = match &p.tail {
            PonEfTail::Particle { parameters, .. } => decode_pon_ef_particle(parameters),
            other => panic!("expected particle tail, got {other:?}"),
        };
        assert!((params.life_min - 0.1).abs() < 1e-3, "poison lifeMin 0.1");
        assert_eq!(params.life_max, 1.0);
        assert!(!params.one_shot, "poison emitter refills");
    }

    #[test]
    fn sample_transform_interpolates() {
        let kf = |time: f32, pos: [f32; 3], quat: [f32; 4], scale: [f32; 3]| PonEfKeyframe {
            time,
            position: pos,
            quaternion: quat,
            scale,
            render: [0.0; 17],
        };
        let keyframes = vec![
            kf(0.0, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0]),
            kf(1.0, [2.0, 4.0, 6.0], [0.0, 0.0, 0.0, 1.0], [2.0, 2.0, 2.0]),
        ];

        let mid = sample_transform(&keyframes, 0.5);
        assert!((mid.position[0] - 1.0).abs() < 1e-5);
        assert!((mid.position[1] - 2.0).abs() < 1e-5);
        assert!((mid.position[2] - 3.0).abs() < 1e-5);
        assert!((mid.scale[0] - 1.5).abs() < 1e-5);

        let start = sample_transform(&keyframes, -1.0);
        assert_eq!(start.position, [0.0, 0.0, 0.0]);
        let end = sample_transform(&keyframes, 5.0);
        assert_eq!(end.position, [2.0, 4.0, 6.0]);

        let empty = sample_transform(&[], 0.5);
        assert_eq!(empty.quaternion, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn sample_transform_slerps_quaternions() {
        let kf = |time: f32, quat: [f32; 4]| PonEfKeyframe {
            time,
            position: [0.0, 0.0, 0.0],
            quaternion: quat,
            scale: [1.0, 1.0, 1.0],
            render: [0.0; 17],
        };
        let keyframes = vec![
            kf(
                0.0,
                [
                    std::f32::consts::FRAC_1_SQRT_2,
                    0.0,
                    0.0,
                    std::f32::consts::FRAC_1_SQRT_2,
                ],
            ),
            kf(1.0, [1.0, 0.0, 0.0, 0.0]),
        ];
        let mid = sample_transform(&keyframes, 0.5);

        assert!(
            (mid.quaternion[0].abs() - 0.9239).abs() < 1e-3,
            "{:?}",
            mid.quaternion
        );
        assert!((mid.quaternion[3].abs() - 0.3827).abs() < 1e-3);
    }

    #[test]
    fn render_state_decode_modes() {
        let mut header = [0u8; 93];

        header[HEADER_BLEND_MODE..HEADER_BLEND_MODE + 4].copy_from_slice(&3u32.to_le_bytes());
        let state = decode_render_state(&header);
        assert!(state.blend_enabled);
        assert_eq!((state.blend_src, state.blend_dst), (6, 7));
        assert!(!state.hidden);

        header[HEADER_BLEND_MODE..HEADER_BLEND_MODE + 4].copy_from_slice(&0u32.to_le_bytes());
        let state = decode_render_state(&header);
        assert!(!state.blend_enabled);

        header[HEADER_BLEND_MODE..HEADER_BLEND_MODE + 4].copy_from_slice(&4u32.to_le_bytes());
        header[HEADER_BLEND_SRC..HEADER_BLEND_SRC + 4].copy_from_slice(&1u32.to_le_bytes());
        header[HEADER_BLEND_DST..HEADER_BLEND_DST + 4].copy_from_slice(&0u32.to_le_bytes());
        let state = decode_render_state(&header);
        assert_eq!((state.blend_src, state.blend_dst), (0, 1));
    }

    #[test]
    fn half_extents_per_type() {
        let node = |type_id: u8, values: Vec<f32>| PonEfNode {
            index: 0,
            type_name: String::new(),
            type_id,
            name: String::new(),
            values: [0; 4],
            parents: [String::new(), String::new()],
            texture: String::new(),
            transform_header: [0; 93],
            keyframes: Vec::new(),
            blob: Vec::new(),
            tail: PonEfTail::Floats(values),
            start_time: 0.0,
            end_time: 0.0,
        };

        assert_eq!(
            node(TYPE_SPHERE, vec![2.0]).half_extents(),
            Some([2.0, 2.0, 2.0])
        );

        assert_eq!(
            node(TYPE_CYLINDER, vec![1.0, 3.0, 4.0]).half_extents(),
            Some([3.0, 3.0, 2.0])
        );

        assert_eq!(
            node(TYPE_BOX, vec![2.0, 4.0, 6.0]).half_extents(),
            Some([1.0, 2.0, 3.0])
        );

        assert_eq!(
            node(TYPE_PLANE, vec![4.0, 6.0]).half_extents(),
            Some([2.0, 3.0, 3.0])
        );

        assert_eq!(
            node(TYPE_DONUT, vec![2.0, 5.0]).half_extents(),
            Some([5.0, 5.0, 1.5])
        );

        let audio = PonEfNode {
            tail: PonEfTail::Audio {
                sound: String::new(),
                value: 0,
            },
            ..node(TYPE_AUDIO, Vec::new())
        };
        assert_eq!(audio.half_extents(), None);
    }

    #[test]
    fn attack_node_predicate() {
        let node = |type_id: u8, name: &str, values: Vec<f32>| PonEfNode {
            index: 0,
            type_name: String::new(),
            type_id,
            name: name.to_string(),
            values: [0; 4],
            parents: [String::new(), String::new()],
            texture: String::new(),
            transform_header: [0; 93],
            keyframes: Vec::new(),
            blob: Vec::new(),
            tail: PonEfTail::Floats(values),
            start_time: 0.0,
            end_time: 0.0,
        };

        assert!(node(TYPE_SPHERE, "aSphere", vec![1.0]).is_attack_node());

        assert!(node(TYPE_BOX, "AttackBox", vec![1.0, 1.0, 1.0]).is_attack_node());

        assert!(!node(TYPE_SPHERE, "bSphere", vec![1.0]).is_attack_node());

        let audio = PonEfNode {
            tail: PonEfTail::Audio {
                sound: String::new(),
                value: 0,
            },
            ..node(TYPE_AUDIO, "aSound", Vec::new())
        };
        assert!(!audio.is_attack_node());

        let particle = PonEfNode {
            tail: PonEfTail::Particle {
                parameters: [0; 96],
                strings: [String::new(), String::new(), String::new()],
            },
            ..node(TYPE_PARTICLE, "aPart", Vec::new())
        };
        assert!(!particle.is_attack_node());
    }

    #[test]
    fn origin_res_pon_ef_parses() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        for name in ["Pos024.ef", "FireHIT.ef", "pos288.ef"] {
            let path = root.join("SFX").join(name);
            if !path.exists() {
                eprintln!("{name} missing, skipping");
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let ef = parse_pon_ef(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(ef.count > 0, "{name} should have nodes");
            assert_eq!(ef.byte_length, bytes.len());
            for node in &ef.nodes {
                let _ = node.half_extents();
            }
            eprintln!(
                "{name}: {} nodes, {} attack nodes",
                ef.count,
                ef.nodes.iter().filter(|n| n.is_attack_node()).count()
            );
        }
    }
}
