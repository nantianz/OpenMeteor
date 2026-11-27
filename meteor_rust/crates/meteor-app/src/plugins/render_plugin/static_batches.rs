use super::PlayingSceneEntity;
use crate::plugins::stage_material::GmdlMaterial;
use bevy::asset::{AssetId, RenderAssetUsages};
use bevy::camera::primitives::Aabb;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use std::collections::HashMap;

const CELL_SIZE: f32 = 40.0;
const MAX_BATCH_SPAN: f32 = 48.0;
const SPLIT_MIN_TRIS: usize = 4_096;
const SPLIT_MIN_SPAN: f32 = 96.0;
const SPLIT_CELL_SIZE: f32 = 64.0;

struct Batch {
    meshes: Vec<Mesh>,
    material: Handle<GmdlMaterial>,
    transform: Transform,
    bounds: Option<Bounds>,
    casts_static_shadow: bool,
}

#[derive(Clone, Copy)]
struct Bounds {
    local_min: Vec3,
    local_max: Vec3,
    world_min: Vec3,
    world_max: Vec3,
}

impl Bounds {
    fn join(self, other: Self) -> Self {
        Self {
            local_min: self.local_min.min(other.local_min),
            local_max: self.local_max.max(other.local_max),
            world_min: self.world_min.min(other.world_min),
            world_max: self.world_max.max(other.world_max),
        }
    }

    fn span(self) -> f32 {
        (self.world_max - self.world_min).max_element()
    }
}

#[derive(Default)]
pub(super) struct StaticBatches {
    cells: HashMap<(IVec3, Option<AssetId<Image>>), Vec<usize>>,
    batches: Vec<Batch>,
    input: usize,
    split_chunks: usize,
}

impl StaticBatches {
    pub(super) fn add(
        &mut self,
        mesh: Mesh,
        material: Handle<GmdlMaterial>,
        transform: Transform,
        materials: &Assets<GmdlMaterial>,
        casts_static_shadow: bool,
    ) {
        self.input += 1;
        let bounds = mesh_bounds(&mesh, &transform);
        if let Some(bounds) = bounds.filter(|bounds| {
            bounds.span() > SPLIT_MIN_SPAN
                && mesh
                    .indices()
                    .is_some_and(|indices| indices.len() >= SPLIT_MIN_TRIS * 3)
        }) {
            if let Some(chunks) = split_large_mesh(&mesh, &transform, SPLIT_CELL_SIZE) {
                self.split_chunks += chunks.len();
                for (chunk_mesh, chunk_bounds) in chunks {
                    self.batches.push(Batch {
                        meshes: vec![chunk_mesh],
                        material: material.clone(),
                        transform,
                        bounds: Some(chunk_bounds),
                        casts_static_shadow,
                    });
                }
                return;
            }
            let _ = bounds;
        }
        if let (Some(bounds), Some(surface)) = (
            bounds.filter(|bounds| bounds.span() <= MAX_BATCH_SPAN),
            materials.get(&material),
        ) {
            let cell = ((bounds.world_min + bounds.world_max) * (0.5 / CELL_SIZE))
                .floor()
                .as_ivec3();
            let texture = surface.texture.as_ref().map(Handle::id);
            let indices = self.cells.entry((cell, texture)).or_default();
            for &id in indices.iter() {
                let batch = &mut self.batches[id];
                let combined = batch.bounds.unwrap().join(bounds);
                if batch.transform == transform
                    && batch.casts_static_shadow == casts_static_shadow
                    && combined.span() <= MAX_BATCH_SPAN
                    && (batch.material == material
                        || materials.get(&batch.material) == Some(surface))
                    && batch.meshes[0]
                        .attributes()
                        .map(|(attribute, _)| attribute.id)
                        .eq(mesh.attributes().map(|(attribute, _)| attribute.id))
                {
                    batch.meshes.push(mesh);
                    batch.bounds = Some(combined);
                    return;
                }
            }
            indices.push(self.batches.len());
        }
        self.batches.push(Batch {
            meshes: vec![mesh],
            material,
            transform,
            bounds,
            casts_static_shadow,
        });
    }

    pub(super) fn spawn(self, commands: &mut Commands, meshes: &mut Assets<Mesh>, stage: &str) {
        if self.input == 0 {
            return;
        }
        info!(
            "{stage} static geometry: {} source batches -> {} draw batches (split chunks: {})",
            self.input,
            self.batches.len(),
            self.split_chunks,
        );
        for batch in self.batches {
            let mesh = merge_scene_meshes(batch.meshes);
            let mut entity = commands.spawn((
                PlayingSceneEntity,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(batch.material),
                batch.transform,
            ));
            if batch.casts_static_shadow {
                entity.insert(bevy::pbr::StaticShadowCaster);
            }
            if let Some(bounds) = batch.bounds {
                entity.insert(Aabb::from_min_max(bounds.local_min, bounds.local_max));
            }
        }
    }
}

fn merge_scene_meshes(mut meshes: Vec<Mesh>) -> Mesh {
    if meshes.len() == 1 {
        return meshes.pop().unwrap();
    }
    let has_normals = meshes[0].attribute(Mesh::ATTRIBUTE_NORMAL).is_some();
    let has_uvs = meshes[0].attribute(Mesh::ATTRIBUTE_UV_0).is_some();
    let has_colors = meshes[0].attribute(Mesh::ATTRIBUTE_COLOR).is_some();
    let has_tangents = meshes[0].attribute(Mesh::ATTRIBUTE_TANGENT).is_some();

    let mut total_verts = 0usize;
    let mut total_indices = 0usize;
    for mesh in &meshes {
        let Some(VertexAttributeValues::Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            return fallback_merge(meshes);
        };
        if has_normals
            && !matches!(
                mesh.attribute(Mesh::ATTRIBUTE_NORMAL),
                Some(VertexAttributeValues::Float32x3(_))
            )
            || has_uvs
                && !matches!(
                    mesh.attribute(Mesh::ATTRIBUTE_UV_0),
                    Some(VertexAttributeValues::Float32x2(_))
                )
            || has_colors
                && !matches!(
                    mesh.attribute(Mesh::ATTRIBUTE_COLOR),
                    Some(VertexAttributeValues::Float32x4(_))
                )
            || has_tangents
                && !matches!(
                    mesh.attribute(Mesh::ATTRIBUTE_TANGENT),
                    Some(VertexAttributeValues::Float32x4(_))
                )
            || mesh.indices().is_none()
        {
            return fallback_merge(meshes);
        }
        total_verts += pos.len();
        total_indices += mesh.indices().map_or(0, Indices::len);
    }

    let mut positions = Vec::with_capacity(total_verts);
    let mut normals = has_normals.then(|| Vec::with_capacity(total_verts));
    let mut uvs = has_uvs.then(|| Vec::with_capacity(total_verts));
    let mut colors = has_colors.then(|| Vec::with_capacity(total_verts));
    let mut tangents = has_tangents.then(|| Vec::with_capacity(total_verts));
    let mut indices = Vec::with_capacity(total_indices);

    for mesh in &meshes {
        let base = positions.len() as u32;
        if let Some(VertexAttributeValues::Float32x3(p)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        {
            positions.extend_from_slice(p);
        }
        if let (Some(dst), Some(VertexAttributeValues::Float32x3(n))) =
            (normals.as_mut(), mesh.attribute(Mesh::ATTRIBUTE_NORMAL))
        {
            dst.extend_from_slice(n);
        }
        if let (Some(dst), Some(VertexAttributeValues::Float32x2(u))) =
            (uvs.as_mut(), mesh.attribute(Mesh::ATTRIBUTE_UV_0))
        {
            dst.extend_from_slice(u);
        }
        if let (Some(dst), Some(VertexAttributeValues::Float32x4(c))) =
            (colors.as_mut(), mesh.attribute(Mesh::ATTRIBUTE_COLOR))
        {
            dst.extend_from_slice(c);
        }
        if let (Some(dst), Some(VertexAttributeValues::Float32x4(t))) =
            (tangents.as_mut(), mesh.attribute(Mesh::ATTRIBUTE_TANGENT))
        {
            dst.extend_from_slice(t);
        }
        if let Some(src_indices) = mesh.indices() {
            indices.extend(src_indices.iter().map(|i| base + i as u32));
        }
    }

    let mut combined = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    combined.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    if let Some(normals) = normals {
        combined.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    }
    if let Some(uvs) = uvs {
        combined.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    }
    if let Some(colors) = colors {
        combined.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    }
    if let Some(tangents) = tangents {
        combined.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
    }
    combined.insert_indices(Indices::U32(indices));
    combined
}

fn fallback_merge(meshes: Vec<Mesh>) -> Mesh {
    let mut iter = meshes.into_iter();
    let mut combined = iter.next().unwrap();
    for mesh in iter {
        combined.merge(&mesh).expect("identical scene mesh layouts");
    }
    combined
}

fn split_large_mesh(
    mesh: &Mesh,
    transform: &Transform,
    cell_size: f32,
) -> Option<Vec<(Mesh, Bounds)>> {
    let VertexAttributeValues::Float32x3(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
    else {
        return None;
    };
    let normals = match mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
        Some(VertexAttributeValues::Float32x3(v)) => Some(v.as_slice()),
        None => None,
        _ => return None,
    };
    let uvs = match mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
        Some(VertexAttributeValues::Float32x2(v)) => Some(v.as_slice()),
        None => None,
        _ => return None,
    };
    let colors = match mesh.attribute(Mesh::ATTRIBUTE_COLOR) {
        Some(VertexAttributeValues::Float32x4(v)) => Some(v.as_slice()),
        None => None,
        _ => return None,
    };
    let tangents = match mesh.attribute(Mesh::ATTRIBUTE_TANGENT) {
        Some(VertexAttributeValues::Float32x4(v)) => Some(v.as_slice()),
        None => None,
        _ => return None,
    };
    let src_indices: Vec<usize> = mesh.indices()?.iter().collect();
    if src_indices.len() < 3 {
        return None;
    }

    let inv_cell = 1.0 / cell_size;
    let mut buckets: HashMap<(i32, i32), Vec<[usize; 3]>> = HashMap::new();
    for tri in src_indices.chunks_exact(3) {
        let (a, b, c) = (tri[0], tri[1], tri[2]);
        let (Some(&pa), Some(&pb), Some(&pc)) =
            (positions.get(a), positions.get(b), positions.get(c))
        else {
            continue;
        };
        let center_local =
            (Vec3::from_array(pa) + Vec3::from_array(pb) + Vec3::from_array(pc)) * (1.0 / 3.0);
        let center_world = transform.transform_point(center_local);
        let cx = (center_world.x * inv_cell).floor() as i32;
        let cz = (center_world.z * inv_cell).floor() as i32;
        buckets.entry((cx, cz)).or_default().push([a, b, c]);
    }
    if buckets.len() <= 1 {
        return None;
    }

    let mut keys: Vec<(i32, i32)> = buckets.keys().copied().collect();
    keys.sort_unstable();

    let mut remap = vec![u32::MAX; positions.len()];
    let mut touched = Vec::new();
    let mut chunks = Vec::with_capacity(keys.len());

    for key in keys {
        let tris = &buckets[&key];
        let mut out_pos = Vec::new();
        let mut out_nrm = normals.map(|_| Vec::new());
        let mut out_uv = uvs.map(|_| Vec::new());
        let mut out_col = colors.map(|_| Vec::new());
        let mut out_tan = tangents.map(|_| Vec::new());
        let mut out_idx = Vec::with_capacity(tris.len() * 3);
        let mut bounds: Option<Bounds> = None;

        for &tri in tris {
            for old in tri {
                let new_idx = if remap[old] != u32::MAX {
                    remap[old]
                } else {
                    let id = out_pos.len() as u32;
                    remap[old] = id;
                    touched.push(old);
                    let p = positions[old];
                    out_pos.push(p);
                    if let (Some(dst), Some(src)) = (out_nrm.as_mut(), normals) {
                        dst.push(src[old]);
                    }
                    if let (Some(dst), Some(src)) = (out_uv.as_mut(), uvs) {
                        dst.push(src[old]);
                    }
                    if let (Some(dst), Some(src)) = (out_col.as_mut(), colors) {
                        dst.push(src[old]);
                    }
                    if let (Some(dst), Some(src)) = (out_tan.as_mut(), tangents) {
                        dst.push(src[old]);
                    }
                    let local = Vec3::from_array(p);
                    let world = transform.transform_point(local);
                    let b = Bounds {
                        local_min: local,
                        local_max: local,
                        world_min: world,
                        world_max: world,
                    };
                    bounds = Some(match bounds {
                        Some(acc) => acc.join(b),
                        None => b,
                    });
                    id
                };
                out_idx.push(new_idx);
            }
        }
        for old in touched.drain(..) {
            remap[old] = u32::MAX;
        }
        let Some(bounds) = bounds else { continue };
        let mut chunk = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        chunk.insert_attribute(Mesh::ATTRIBUTE_POSITION, out_pos);
        if let Some(n) = out_nrm {
            chunk.insert_attribute(Mesh::ATTRIBUTE_NORMAL, n);
        }
        if let Some(u) = out_uv {
            chunk.insert_attribute(Mesh::ATTRIBUTE_UV_0, u);
        }
        if let Some(c) = out_col {
            chunk.insert_attribute(Mesh::ATTRIBUTE_COLOR, c);
        }
        if let Some(t) = out_tan {
            chunk.insert_attribute(Mesh::ATTRIBUTE_TANGENT, t);
        }
        chunk.insert_indices(Indices::U32(out_idx));
        chunks.push((chunk, bounds));
    }
    Some(chunks)
}

fn mesh_bounds(mesh: &Mesh, transform: &Transform) -> Option<Bounds> {
    let VertexAttributeValues::Float32x3(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
    else {
        return None;
    };
    let point = |index| {
        let local = Vec3::from_array(*positions.get(index)?);
        let world = transform.transform_point(local);
        Some(Bounds {
            local_min: local,
            local_max: local,
            world_min: world,
            world_max: world,
        })
    };
    let mut indices = mesh.indices()?.iter();
    let first = point(indices.next()?)?;
    indices.try_fold(first, |bounds, index| Some(bounds.join(point(index)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_large_mesh_preserves_triangles_and_partitions_spatially() {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut colors = Vec::new();
        let mut indices = Vec::new();

        for i in 0..4200u32 {
            let x = (i as f32 / 4200.0) * 300.0;
            let base = positions.len() as u32;
            positions.push([x, 0.0, 0.0]);
            positions.push([x + 1.0, 0.0, 0.0]);
            positions.push([x, 0.0, 1.0]);
            for _ in 0..3 {
                normals.push([0.0, 1.0, 0.0]);
                uvs.push([0.0, 0.0]);
                colors.push([1.0, 1.0, 1.0, 1.0]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_indices(Indices::U32(indices));

        let chunks = split_large_mesh(&mesh, &Transform::IDENTITY, SPLIT_CELL_SIZE).unwrap();
        assert!(
            chunks.len() >= 4,
            "expected spatial chunks, got {}",
            chunks.len()
        );
        let total_tris: usize = chunks
            .iter()
            .map(|(m, _)| m.indices().unwrap().len() / 3)
            .sum();
        assert_eq!(total_tris, 4200);
        for (_, b) in &chunks {
            assert!(b.span() <= SPLIT_CELL_SIZE + 2.0);
        }
    }

    #[test]
    fn deferred_merge_combines_attributes_and_offsets_indices() {
        let make = |x: f32| {
            let mut m = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            );
            m.insert_attribute(
                Mesh::ATTRIBUTE_POSITION,
                vec![[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 1.0, 0.0]],
            );
            m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 3]);
            m.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; 3]);
            m.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0, 1.0, 1.0, 1.0]; 3]);
            m.insert_indices(Indices::U32(vec![0, 1, 2]));
            m
        };
        let merged = merge_scene_meshes(vec![make(0.0), make(10.0)]);
        assert_eq!(merged.count_vertices(), 6);
        let idx: Vec<usize> = merged.indices().unwrap().iter().collect();
        assert_eq!(idx, vec![0, 1, 2, 3, 4, 5]);
    }
}
