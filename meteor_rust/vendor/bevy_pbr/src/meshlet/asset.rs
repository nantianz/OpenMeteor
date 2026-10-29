use alloc::sync::Arc;
use bevy_asset::{
    io::{Reader, Writer},
    saver::{AssetSaver, SavedAsset},
    Asset, AssetLoader, AssetPath, AsyncReadExt, AsyncWriteExt, LoadContext,
};
use bevy_math::{Vec2, Vec3};
use bevy_reflect::TypePath;
use bevy_render::render_resource::ShaderType;
use bevy_tasks::block_on;
use bytemuck::{Pod, Zeroable};
use lz4_flex::frame::{FrameDecoder, FrameEncoder};
use std::io::{Read, Write};
use thiserror::Error;

const MESHLET_MESH_ASSET_MAGIC: u64 = 1717551717668;

pub const MESHLET_MESH_ASSET_VERSION: u64 = 3;

#[derive(Asset, TypePath, Clone)]
pub struct MeshletMesh {

    pub(crate) vertex_positions: Arc<[u32]>,

    pub(crate) vertex_normals: Arc<[u32]>,

    pub(crate) vertex_uvs: Arc<[Vec2]>,

    pub(crate) indices: Arc<[u8]>,

    pub(crate) bvh: Arc<[BvhNode]>,

    pub(crate) meshlets: Arc<[Meshlet]>,

    pub(crate) meshlet_cull_data: Arc<[MeshletCullData]>,

    pub(crate) aabb: MeshletAabb,

    pub(crate) bvh_depth: u32,
}

#[derive(Copy, Clone, Default, Pod, Zeroable)]
#[repr(C)]
pub struct BvhNode {

    pub aabbs: [MeshletAabbErrorOffset; 8],

    pub lod_bounds: [MeshletBoundingSphere; 8],

    pub child_counts: [u8; 8],
    pub _padding: [u32; 2],
}

#[derive(Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct Meshlet {

    pub start_vertex_position_bit: u32,

    pub start_vertex_attribute_id: u32,

    pub start_index_id: u32,

    pub vertex_count_minus_one: u8,

    pub triangle_count: u8,

    pub padding: u16,

    pub bits_per_vertex_position_channel_x: u8,

    pub bits_per_vertex_position_channel_y: u8,

    pub bits_per_vertex_position_channel_z: u8,

    pub vertex_position_quantization_factor: u8,

    pub min_vertex_position_channel_x: f32,

    pub min_vertex_position_channel_y: f32,

    pub min_vertex_position_channel_z: f32,
}

#[derive(Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MeshletCullData {

    pub aabb: MeshletAabbErrorOffset,

    pub lod_group_sphere: MeshletBoundingSphere,
}

#[derive(Copy, Clone, Default, Pod, Zeroable, ShaderType)]
#[repr(C)]
pub struct MeshletAabb {
    pub center: Vec3,
    pub half_extent: Vec3,
}

#[derive(Copy, Clone, Default, Pod, Zeroable, ShaderType)]
#[repr(C)]
pub struct MeshletAabbErrorOffset {
    pub center: Vec3,
    pub error: f32,
    pub half_extent: Vec3,
    pub child_offset: u32,
}

#[derive(Copy, Clone, Default, Pod, Zeroable)]
#[repr(C)]
pub struct MeshletBoundingSphere {
    pub center: Vec3,
    pub radius: f32,
}

#[derive(TypePath)]
pub struct MeshletMeshSaver;

impl AssetSaver for MeshletMeshSaver {
    type Asset = MeshletMesh;
    type Settings = ();
    type OutputLoader = MeshletMeshLoader;
    type Error = MeshletMeshSaveOrLoadError;

    async fn save(
        &self,
        writer: &mut Writer,
        asset: SavedAsset<'_, '_, MeshletMesh>,
        _settings: &(),
        _asset_path: AssetPath<'_>,
    ) -> Result<(), MeshletMeshSaveOrLoadError> {

        writer
            .write_all(&MESHLET_MESH_ASSET_MAGIC.to_le_bytes())
            .await?;

        writer
            .write_all(&MESHLET_MESH_ASSET_VERSION.to_le_bytes())
            .await?;

        writer.write_all(bytemuck::bytes_of(&asset.aabb)).await?;
        writer
            .write_all(bytemuck::bytes_of(&asset.bvh_depth))
            .await?;

        let mut writer = FrameEncoder::new(AsyncWriteSyncAdapter(writer));
        write_slice(&asset.vertex_positions, &mut writer)?;
        write_slice(&asset.vertex_normals, &mut writer)?;
        write_slice(&asset.vertex_uvs, &mut writer)?;
        write_slice(&asset.indices, &mut writer)?;
        write_slice(&asset.bvh, &mut writer)?;
        write_slice(&asset.meshlets, &mut writer)?;
        write_slice(&asset.meshlet_cull_data, &mut writer)?;
        writer.finish()?;

        Ok(())
    }
}

#[derive(TypePath)]
pub struct MeshletMeshLoader;

impl AssetLoader for MeshletMeshLoader {
    type Asset = MeshletMesh;
    type Settings = ();
    type Error = MeshletMeshSaveOrLoadError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<MeshletMesh, MeshletMeshSaveOrLoadError> {

        let magic = async_read_u64(reader).await?;
        if magic != MESHLET_MESH_ASSET_MAGIC {
            return Err(MeshletMeshSaveOrLoadError::WrongFileType);
        }

        let version = async_read_u64(reader).await?;
        if version != MESHLET_MESH_ASSET_VERSION {
            return Err(MeshletMeshSaveOrLoadError::WrongVersion { found: version });
        }

        let mut bytes = [0u8; size_of::<MeshletAabb>()];
        reader.read_exact(&mut bytes).await?;
        let aabb = bytemuck::cast(bytes);
        let mut bytes = [0u8; size_of::<u32>()];
        reader.read_exact(&mut bytes).await?;
        let bvh_depth = u32::from_le_bytes(bytes);

        let reader = &mut FrameDecoder::new(AsyncReadSyncAdapter(reader));
        let vertex_positions = read_slice(reader)?;
        let vertex_normals = read_slice(reader)?;
        let vertex_uvs = read_slice(reader)?;
        let indices = read_slice(reader)?;
        let bvh = read_slice(reader)?;
        let meshlets = read_slice(reader)?;
        let meshlet_cull_data = read_slice(reader)?;

        Ok(MeshletMesh {
            vertex_positions,
            vertex_normals,
            vertex_uvs,
            indices,
            bvh,
            meshlets,
            meshlet_cull_data,
            aabb,
            bvh_depth,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["meshlet_mesh"]
    }
}

#[derive(Error, Debug)]
pub enum MeshletMeshSaveOrLoadError {
    #[error("file was not a MeshletMesh asset")]
    WrongFileType,
    #[error("expected asset version {MESHLET_MESH_ASSET_VERSION} but found version {found}")]
    WrongVersion { found: u64 },
    #[error("failed to compress or decompress asset data")]
    CompressionOrDecompression(#[from] lz4_flex::frame::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

async fn async_read_u64(reader: &mut dyn Reader) -> Result<u64, std::io::Error> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes).await?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_u64(reader: &mut dyn Read) -> Result<u64, std::io::Error> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn write_slice<T: Pod>(
    field: &[T],
    writer: &mut dyn Write,
) -> Result<(), MeshletMeshSaveOrLoadError> {
    writer.write_all(&(field.len() as u64).to_le_bytes())?;
    writer.write_all(bytemuck::cast_slice(field))?;
    Ok(())
}

fn read_slice<T: Pod>(reader: &mut dyn Read) -> Result<Arc<[T]>, std::io::Error> {
    let len = read_u64(reader)? as usize;

    let mut data: Arc<[T]> = core::iter::repeat_with(T::zeroed).take(len).collect();
    let slice = Arc::get_mut(&mut data).unwrap();
    reader.read_exact(bytemuck::cast_slice_mut(slice))?;

    Ok(data)
}

struct AsyncWriteSyncAdapter<'a>(&'a mut Writer);

impl Write for AsyncWriteSyncAdapter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        block_on(self.0.write(buf))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        block_on(self.0.flush())
    }
}

struct AsyncReadSyncAdapter<'a>(&'a mut dyn Reader);

impl Read for AsyncReadSyncAdapter<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        block_on(self.0.read(buf))
    }
}
