use crate::plugins::{
    render_plugin::PlayingSceneEntity,
    stage_material::{GmdlMaterial, GmdlMaterialParams, GmdlMode},
};
use bevy::{mesh::VertexAttributeValues, prelude::*};

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::plugins) enum WaterfallLayer {
    Curtain,
    Mist,
}

pub(in crate::plugins) fn spawn(
    commands: &mut Commands,
    mesh: &Mesh,
    transform: Transform,
    texture: Handle<Image>,
    floor: Option<f32>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<GmdlMaterial>,
) {
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return;
    };
    let Some(VertexAttributeValues::Float32x3(normals)) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
    else {
        return;
    };
    let Some(normal) = normals
        .iter()
        .map(|n| Vec3::from_array(*n))
        .find(|n| n.length_squared() > 0.5)
    else {
        return;
    };
    let normal = (transform.rotation * normal).normalize_or_zero();
    let world = transform.compute_affine();
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for p in positions {
        let point = world.transform_point3(Vec3::from_array(*p));
        min = min.min(point);
        max = max.max(point);
    }
    let width = (max.x - min.x).hypot(max.z - min.z);
    let height = max.y - min.y;
    if !width.is_finite() || !height.is_finite() || width < 0.05 || height < 0.1 {
        return;
    }
    let layer_material = |kind: f32, opacity: f32| {
        let mut material = GmdlMaterial::new_scene(
            GmdlMaterialParams {
                opacity,
                alpha_cutoff: 0.,
                mode: GmdlMode::Blend.as_u32(),
                has_texture: 1,
                uv_offset: Vec2::ZERO,
            },
            Some(texture.clone()),
            true,
            GmdlMode::Blend,
        );
        material.water = Vec4::new(kind, 0., 0., 0.);
        material
    };
    let layer = materials.add(layer_material(5., 0.34));
    let mesh_handle = meshes.add(mesh.clone());
    for distance in [-0.35f32, 0.35] {
        let mut layer_transform = transform;
        layer_transform.translation += normal * distance;
        commands.spawn((
            PlayingSceneEntity,
            WaterfallLayer::Curtain,
            Mesh3d(mesh_handle.clone()),
            MeshMaterial3d(layer.clone()),
            layer_transform,
        ));
    }
    let mut face_normal = Vec3::new(normal.x, 0., normal.z).normalize_or_zero();
    if face_normal.length_squared() < 0.5 {
        face_normal = Vec3::Z;
    }
    let mist_height = (height * 0.28).clamp(1., 18.);
    let base = floor.unwrap_or(min.y).clamp(min.y, max.y);
    let mist_center = Vec3::new(
        (min.x + max.x) * 0.5,
        base + mist_height * 0.30,
        (min.z + max.z) * 0.5,
    );
    let mist_mesh = meshes.add(Rectangle::new(width * 2.0, mist_height));
    let mist = materials.add(layer_material(6., 0.46));
    let rotation = Quat::from_rotation_arc(Vec3::Z, face_normal);
    commands.spawn((
        PlayingSceneEntity,
        WaterfallLayer::Mist,
        Mesh3d(mist_mesh),
        MeshMaterial3d(mist),
        Transform::from_translation(mist_center + face_normal * 0.7).with_rotation(rotation),
    ));
}
