use super::interaction::LiquidInteractions;
use crate::app_state::AppState;
use crate::plugins::stage_material::{GmdlMaterial, GmdlMaterialParams, GmdlMode};
use bevy::asset::RenderAssetUsages;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;

const MAX_DRAWN_DROPS: usize = 2048;

#[derive(Component)]
struct SplashDraw;

#[derive(Resource, Default)]
struct SplashVisuals {
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<GmdlMaterial>>,
    entity: Option<Entity>,
    active: bool,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<SplashVisuals>()
        .add_systems(
            Update,
            update
                .after(super::interaction::update)
                .run_if(in_state(AppState::Playing).or_else(in_state(AppState::Paused))),
        )
        .add_systems(
            OnExit(AppState::Playing),
            clear.run_if(crate::plugins::ends_playing_session),
        );
}

fn clear(
    mut commands: Commands,
    mut visuals: ResMut<SplashVisuals>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
) {
    if let Some(entity) = visuals.entity.take() {
        commands.entity(entity).despawn();
    }
    if let Some(mesh) = visuals.mesh.take() {
        meshes.remove(mesh.id());
    }
    if let Some(material) = visuals.material.take() {
        materials.remove(material.id());
    }
    *visuals = SplashVisuals::default();
}

fn inactive_mesh() -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0; 3]; 3])
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 3])
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0; 2]; 3])
    .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, vec![[1.0, 0.0, 0.0, 1.0]; 3])
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0; 4]; 3])
}

fn build_mesh(splashes: &[(&super::spray::Spray, f32)], eye: Vec3, anchor: Vec3) -> Mesh {
    let mut drops = Vec::new();
    for &(spray, age) in splashes.iter().rev() {
        for p in &spray.particles {
            if p.visible(age) && drops.len() < MAX_DRAWN_DROPS {
                let (position, velocity) = p.at(age);
                drops.push((position, velocity, p.radius));
            }
        }
    }

    drops.sort_by(|a, b| {
        b.0.distance_squared(eye)
            .total_cmp(&a.0.distance_squared(eye))
    });
    if drops.is_empty() {
        return inactive_mesh();
    }
    let mut positions = Vec::with_capacity(drops.len() * 6);
    let mut normals = Vec::with_capacity(positions.capacity());
    let mut tangents = Vec::with_capacity(positions.capacity());
    let mut colors = Vec::with_capacity(positions.capacity());
    let mut uvs = Vec::with_capacity(positions.capacity());
    for (center, velocity, radius) in drops {
        let normal = (eye - center).normalize_or(Vec3::Z);
        let projected = velocity - normal * normal.dot(velocity);
        let up = projected.normalize_or(Vec3::Y);
        let right = up.cross(normal).normalize_or(Vec3::X);
        let up = normal.cross(right).normalize_or(Vec3::Y);
        let width = radius.max(0.012);
        let length = (radius * radius + (projected.length() / 240.0).powi(2))
            .sqrt()
            .clamp(width, width * 3.0);
        let coverage = (radius / width).powi(2);
        for (x, y) in [
            (-1.0, -1.0),
            (1.0, -1.0),
            (1.0, 1.0),
            (-1.0, -1.0),
            (1.0, 1.0),
            (-1.0, 1.0),
        ] {
            positions.push((center - anchor + right * (x * width) + up * (y * length)).to_array());
            normals.push(normal.to_array());
            tangents.push(right.extend(1.0).to_array());
            colors.push([0.68, 0.79, 0.81, coverage * 0.8]);
            uvs.push([(x + 1.0) * 0.5, (y + 1.0) * 0.5]);
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, tangents)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
}

fn update(
    mut commands: Commands,
    interactions: Res<LiquidInteractions>,
    cameras: Query<&Transform, With<Camera3d>>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut materials: Option<ResMut<Assets<GmdlMaterial>>>,
    mut draws: Query<(&mut Transform, &mut Visibility), (With<SplashDraw>, Without<Camera3d>)>,
    mut visuals: ResMut<SplashVisuals>,
) {
    let splashes: Vec<_> = interactions.splashes().collect();
    if splashes.is_empty() {
        if visuals.active {
            if let Some(entity) = visuals.entity {
                if let Ok((_, mut visibility)) = draws.get_mut(entity) {
                    *visibility = Visibility::Hidden;
                }
            }
            visuals.active = false;
        }
        return;
    }
    let Some(meshes) = meshes.as_deref_mut() else {
        return;
    };
    let camera = cameras.iter().next();
    let eye = camera.map_or(Vec3::ZERO, |c| c.translation);
    let anchor = splashes
        .iter()
        .min_by(|a, b| {
            a.0.center
                .distance_squared(eye)
                .total_cmp(&b.0.center.distance_squared(eye))
        })
        .map_or(Vec3::ZERO, |s| s.0.center);
    if visuals.mesh.is_none() {
        let Some(materials) = materials.as_deref_mut() else {
            return;
        };
        let mut material = GmdlMaterial::new(
            GmdlMaterialParams {
                opacity: 1.0,
                alpha_cutoff: 0.0,
                mode: GmdlMode::Blend.as_u32(),
                has_texture: 0,
                uv_offset: Vec2::ZERO,
            },
            None,
            true,
            GmdlMode::Blend,
        );
        material.camera_occlusion_enabled = false;

        material.water.x = 4.0;
        let material = materials.add(material);
        let mesh = meshes.add(inactive_mesh());
        let entity = commands
            .spawn((
                SplashDraw,
                bevy::camera::visibility::NoFrustumCulling,
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                Visibility::Visible,
                Transform::from_translation(anchor),
            ))
            .id();
        visuals.mesh = Some(mesh);
        visuals.material = Some(material);
        visuals.entity = Some(entity);
    }
    if let Some(mut mesh) = meshes.get_mut(visuals.mesh.as_ref().unwrap()) {
        *mesh = build_mesh(&splashes, eye, anchor);
    }
    if let Some(entity) = visuals.entity {
        if let Ok((mut transform, mut visibility)) = draws.get_mut(entity) {
            transform.translation = anchor;
            *visibility = Visibility::Visible;
        }
    }
    visuals.active = true;
}

#[cfg(test)]
mod tests {
    use super::super::spray::{Impact, Spray};
    use super::*;
    #[test]
    fn expired_spray_keeps_a_valid_gpu_mesh_and_active_spray_is_bounded() {
        let spray = Spray::new(
            Vec3::ZERO,
            Impact {
                velocity: Vec3::new(20.0, -30.0, 0.0),
                radius: 0.4,
                depth: 0.4,
            },
            1.0,
            7,
        );
        let mesh = build_mesh(&[(&spray, 0.08)], Vec3::new(0.0, 2.0, 8.0), Vec3::ZERO);
        assert!(mesh.count_vertices() > 3);
        assert!(mesh.count_vertices() <= MAX_DRAWN_DROPS * 6);
        assert_eq!(
            build_mesh(&[(&spray, 3.0)], Vec3::Z, Vec3::ZERO).count_vertices(),
            3
        );
        assert!(mesh.attribute(Mesh::ATTRIBUTE_TANGENT).is_some());
    }
}
