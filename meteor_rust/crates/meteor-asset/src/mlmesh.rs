use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_MESHLETS: usize = 4096;
pub const MAX_VERTICES: usize = 64;
pub const MAX_TRIANGLES: usize = 124;
const MAGIC: &[u8; 8] = b"MLMESH01";

#[derive(Debug)]
pub struct Model {
    pub primitives: Vec<Primitive>,
}

#[derive(Debug)]
pub struct Primitive {
    pub slot: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub tangents: Option<Vec<[f32; 4]>>,
    pub colors: Option<Vec<[f32; 4]>>,
    pub meshlets: Vec<Meshlet>,
}

#[derive(Debug)]
pub struct Meshlet {
    pub vertices: Vec<u32>,
    pub indices: Vec<u8>,
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        let (head, tail) = self.0.split_at_checked(n).ok_or("truncated mlmesh")?;
        self.0 = tail;
        Ok(head)
    }

    fn u32(&mut self) -> Result<u32, &'static str> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn vector<const N: usize>(&mut self, limit: f32) -> Result<[f32; N], &'static str> {
        let mut values = [0.; N];
        for value in &mut values {
            *value = f32::from_bits(self.u32()?);
            if !value.is_finite() || value.abs() > limit {
                return Err("invalid mlmesh vertex attribute");
            }
        }
        Ok(values)
    }
}

impl Model {
    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > MAX_FILE_BYTES || bytes.get(..8) != Some(MAGIC) {
            return Err("invalid mlmesh size or format version");
        }
        let body = bytes.get(40..).ok_or("truncated mlmesh header")?;
        if Sha256::digest(body)[..] != bytes[8..40] {
            return Err("mlmesh checksum mismatch");
        }

        let mut reader = Reader(body);
        let count = reader.u32()? as usize;
        if !(1..=32).contains(&count) {
            return Err("expected 1 to 32 mlmesh primitives");
        }
        let mut total_meshlets = 0;
        let mut primitives = Vec::with_capacity(count);
        for _ in 0..count {
            let slot_len = reader.u32()? as usize;
            let flags = reader.u32()?;
            let vertex_count = reader.u32()? as usize;
            let meshlet_count = reader.u32()? as usize;
            if !(1..=80).contains(&slot_len)
                || flags & !3 != 0
                || meshlet_count == 0
                || meshlet_count > MAX_MESHLETS - total_meshlets
            {
                return Err("invalid mlmesh primitive header");
            }
            let stride = 32 + 16 * flags.count_ones() as usize;
            if vertex_count == 0 || vertex_count > reader.0.len() / stride {
                return Err("invalid mlmesh vertex count");
            }
            total_meshlets += meshlet_count;
            let slot =
                std::str::from_utf8(reader.take(slot_len)?).map_err(|_| "invalid mlmesh slot")?;
            if slot.chars().any(char::is_control) {
                return Err("invalid mlmesh slot");
            }
            let mut primitive = Primitive {
                slot: slot.into(),
                positions: Vec::with_capacity(vertex_count),
                normals: Vec::with_capacity(vertex_count),
                uvs: Vec::with_capacity(vertex_count),
                tangents: (flags & 1 != 0).then(|| Vec::with_capacity(vertex_count)),
                colors: (flags & 2 != 0).then(|| Vec::with_capacity(vertex_count)),
                meshlets: Vec::with_capacity(meshlet_count),
            };
            for _ in 0..vertex_count {
                primitive.positions.push(reader.vector(10_000.)?);
                primitive.normals.push(reader.vector(1.01)?);
                primitive.uvs.push(reader.vector(10_000.)?);
                if let Some(tangents) = &mut primitive.tangents {
                    tangents.push(reader.vector(1.01)?);
                }
                if let Some(colors) = &mut primitive.colors {
                    colors.push(reader.vector(1.)?);
                }
            }
            for _ in 0..meshlet_count {
                let counts = reader.take(2)?;
                let (vertices, triangles) = (counts[0] as usize, counts[1] as usize);
                if !(3..=MAX_VERTICES).contains(&vertices)
                    || !(1..=MAX_TRIANGLES).contains(&triangles)
                {
                    return Err("invalid meshlet dimensions");
                }
                let mut remap = Vec::with_capacity(vertices);
                let mut unique = BTreeSet::new();
                for _ in 0..vertices {
                    let vertex = reader.u32()?;
                    if vertex as usize >= vertex_count || !unique.insert(vertex) {
                        return Err("invalid meshlet vertex remap");
                    }
                    remap.push(vertex);
                }
                let indices = reader.take(triangles * 3)?;
                if indices.iter().any(|&i| usize::from(i) >= vertices) {
                    return Err("meshlet index outside its vertex table");
                }
                primitive.meshlets.push(Meshlet {
                    vertices: remap,
                    indices: indices.into(),
                });
            }
            primitives.push(primitive);
        }
        if !reader.0.is_empty() {
            return Err("trailing data after mlmesh");
        }
        Ok(Self { primitives })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mlmesh_export_roundtrip_and_corrupt_cluster_indices() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/graphics-packs/meshlet-demo/models/bronze-ornament.mlmesh");
        let mut bytes = std::fs::read(path).unwrap();
        let model = Model::decode(&bytes).unwrap();
        let [primitive] = model.primitives.as_slice() else {
            panic!("demo material count");
        };
        assert_eq!(primitive.slot, "bronze");
        assert_eq!(primitive.meshlets.len(), 74);
        assert_eq!(
            primitive
                .meshlets
                .iter()
                .map(|m| m.indices.len())
                .sum::<usize>(),
            6912 * 3
        );
        let end = bytes.len() - 1;
        bytes[end] = 255;
        assert_eq!(
            Model::decode(&bytes).unwrap_err(),
            "mlmesh checksum mismatch"
        );
        let hash = Sha256::digest(&bytes[40..]);
        bytes[8..40].copy_from_slice(&hash);
        assert_eq!(
            Model::decode(&bytes).unwrap_err(),
            "meshlet index outside its vertex table"
        );
        assert!(Model::decode(&bytes[..39]).is_err());
    }
}
