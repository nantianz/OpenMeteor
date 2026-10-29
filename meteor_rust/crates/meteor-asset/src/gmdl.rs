pub const GMDL_MAGIC: &str = "GMDL V1.00";

#[derive(Debug, Clone)]
pub struct GmdlTexture {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct GmdlMaterial {
    pub tex_index: i32,

    pub blend: String,
    pub two_side: bool,

    pub state: String,
    pub alpha: f32,
}

#[derive(Debug, Clone)]
pub struct GmdlObject {
    pub name: String,
    pub num_verts: usize,
    pub num_faces: usize,

    pub positions: Vec<f32>,

    pub normals: Vec<f32>,

    pub uvs: Vec<f32>,

    pub colors: Vec<u8>,

    pub face_mat: Vec<i32>,

    pub indices: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct GmdlMesh {
    pub textures: Vec<GmdlTexture>,
    pub materials: Vec<GmdlMaterial>,
    pub objects: Vec<GmdlObject>,
}

impl GmdlMesh {
    pub fn resolve_texture_name(&self, mat: &GmdlMaterial) -> Option<&str> {
        let t = mat.tex_index;
        if t < 0 || t as usize >= self.textures.len() {
            return None;
        }
        Some(&self.textures[t as usize].name)
    }
}

pub fn decode_gmdl(data: &[u8]) -> Result<GmdlMesh, GmdlError> {
    let mut o = 0usize;
    let mut cursor = Cursor::new(data, &mut o);

    let magic = cursor.str_raw(10)?;
    if magic != GMDL_MAGIC {
        return Err(GmdlError::BadMagic(magic));
    }

    let tex_count = cursor.u32()? as usize;
    let mut textures = Vec::with_capacity(tex_count);
    for _ in 0..tex_count {
        textures.push(GmdlTexture {
            name: cursor.str()?,
        });
    }

    let mat_count = cursor.u32()? as usize;
    let mut materials = Vec::with_capacity(mat_count);
    for _ in 0..mat_count {
        let tex_index = cursor.s32()?;
        let blend = cursor.str()?;
        let two_side = cursor.u8()? != 0;
        let state = cursor.str()?;
        let alpha = cursor.f32()?;
        materials.push(GmdlMaterial {
            tex_index,
            blend,
            two_side,
            state,
            alpha,
        });
    }

    cursor.u32()?;
    cursor.u32()?;
    let total_verts = cursor.u32()? as usize;
    cursor.u32()?;

    let mut objects = Vec::new();
    let mut seen_verts = 0usize;
    while seen_verts < total_verts && cursor.remaining() > 8 {
        let name = cursor.str()?;
        let num_verts = cursor.u32()? as usize;
        let num_faces = cursor.u32()? as usize;

        let mut positions = Vec::with_capacity(num_verts * 3);
        let mut normals = Vec::with_capacity(num_verts * 3);
        let mut uvs = Vec::with_capacity(num_verts * 2);
        let mut colors = Vec::with_capacity(num_verts * 4);
        for _ in 0..num_verts {
            positions.push(cursor.f32()?);
            positions.push(cursor.f32()?);
            positions.push(cursor.f32()?);
            normals.push(cursor.f32()?);
            normals.push(cursor.f32()?);
            normals.push(cursor.f32()?);

            colors.push(cursor.u8()?);
            colors.push(cursor.u8()?);
            colors.push(cursor.u8()?);
            colors.push(cursor.u8()?);
            uvs.push(cursor.f32()?);
            uvs.push(cursor.f32()?);
        }

        let mut face_mat = Vec::with_capacity(num_faces);
        let mut indices = Vec::with_capacity(num_faces * 3);
        for _ in 0..num_faces {
            face_mat.push(cursor.s32()?);
            indices.push(cursor.u32()?);
            indices.push(cursor.u32()?);
            indices.push(cursor.u32()?);
            cursor.f32()?;
            cursor.f32()?;
            cursor.f32()?;
        }

        objects.push(GmdlObject {
            name,
            num_verts,
            num_faces,
            positions,
            normals,
            uvs,
            colors,
            face_mat,
            indices,
        });
        seen_verts += num_verts;
    }

    Ok(GmdlMesh {
        textures,
        materials,
        objects,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum GmdlError {
    #[error("无效的 GMDL 魔数: {0:?}")]
    BadMagic(String),
    #[error("数据截断 (offset {offset}, 需要 {need} 字节, 剩余 {remain})")]
    Truncated {
        offset: usize,
        need: usize,
        remain: usize,
    },
}

struct Cursor<'a> {
    data: &'a [u8],
    o: &'a mut usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8], o: &'a mut usize) -> Self {
        Self { data, o }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(*self.o)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], GmdlError> {
        let end = self.o.checked_add(n).ok_or(GmdlError::Truncated {
            offset: *self.o,
            need: n,
            remain: 0,
        })?;
        if end > self.data.len() {
            return Err(GmdlError::Truncated {
                offset: *self.o,
                need: n,
                remain: self.remaining(),
            });
        }
        let slice = &self.data[*self.o..end];
        *self.o = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32, GmdlError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn s32(&mut self) -> Result<i32, GmdlError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32, GmdlError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u8(&mut self) -> Result<u8, GmdlError> {
        Ok(self.take(1)?[0])
    }

    fn str(&mut self) -> Result<String, GmdlError> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    fn str_raw(&mut self, len: usize) -> Result<String, GmdlError> {
        let bytes = self.take(len)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_gmdl() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"GMDL V1.00");

        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&9u32.to_le_bytes());
        d.extend_from_slice(b"tex_a.tga");

        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&0i32.to_le_bytes());
        d.extend_from_slice(&6u32.to_le_bytes());
        d.extend_from_slice(b"NORMAL");
        d.push(0u8);
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(b"0");
        d.extend_from_slice(&1.0f32.to_le_bytes());

        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());

        d.extend_from_slice(&4u32.to_le_bytes());
        d.extend_from_slice(b"di01");
        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());

        for v in [0f32, 0.0, 0.0, 0.0, 0.0, 1.0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&[255u8, 0, 0, 255]);
        d.extend_from_slice(&0f32.to_le_bytes());
        d.extend_from_slice(&0f32.to_le_bytes());

        for v in [1f32, 0.0, 0.0, 0.0, 0.0, 1.0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&[0u8, 255, 0, 255]);
        d.extend_from_slice(&1f32.to_le_bytes());
        d.extend_from_slice(&0f32.to_le_bytes());

        d.extend_from_slice(&0i32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&0f32.to_le_bytes());
        d.extend_from_slice(&0f32.to_le_bytes());
        d.extend_from_slice(&1f32.to_le_bytes());

        d
    }

    #[test]
    fn test_decode_basic() {
        let gmdl = decode_gmdl(&make_gmdl()).unwrap();
        assert_eq!(gmdl.textures.len(), 1);
        assert_eq!(gmdl.textures[0].name, "tex_a.tga");
        assert_eq!(gmdl.materials.len(), 1);
        assert_eq!(gmdl.materials[0].blend, "NORMAL");
        assert_eq!(gmdl.materials[0].tex_index, 0);
        assert_eq!(gmdl.objects.len(), 1);
        assert_eq!(gmdl.objects[0].name, "di01");
        assert_eq!(gmdl.objects[0].num_verts, 2);
        assert_eq!(gmdl.objects[0].num_faces, 1);
        assert_eq!(gmdl.objects[0].colors[0], 255);
        assert_eq!(gmdl.objects[0].colors[1], 0);
    }

    #[test]
    fn test_resolve_texture() {
        let gmdl = decode_gmdl(&make_gmdl()).unwrap();
        let mat = &gmdl.materials[0];
        assert_eq!(gmdl.resolve_texture_name(mat), Some("tex_a.tga"));
    }

    #[test]
    fn test_bad_magic() {
        assert!(decode_gmdl(b"NOT GMDL!!00000000").is_err());
    }
}
