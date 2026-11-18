mod interaction;
mod placement;
#[cfg(test)]
mod tests;

use super::{config::Grass, GraphicsPacks};
use crate::app_state::{AppState, Settings};
use crate::components::fighter::{Concealment, Fighter, Health, MotionState};
use crate::plugins::{camera_plugin::GameplayCamera, render_plugin::PlayingSceneEntity};
use bevy::asset::RenderAssetUsages;
use bevy::camera::{
    primitives::{Aabb, Frustum},
    visibility::{NoAutoAabb, VisibilitySystems},
};
use bevy::mesh::{Indices, MeshTag, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    TextureDimension, TextureFormat,
};
use bevy::shader::{Shader, ShaderRef};
use interaction::{BendMap, BendUpload};
use meteor_core::types::MovementMode;
use placement::ROOTS_PER_TILE;
use std::sync::Arc;

const SHADER: Handle<Shader> = bevy::asset::uuid_handle!("9a315511-160c-4087-af9c-ef5cd526b723");
const MID_ROOTS: usize = 72;
const FAR_ROOTS: usize = 24;
const CONTACTS: usize = 8;
const CONTACT_INTERVAL: f32 = 1.0 / 30.0;

#[derive(Clone, ShaderType)]
struct GrassUniform {
    root_color: Vec4,
    tip_color: Vec4,
    blade: Vec4,
    distances: Vec4,
    contact: Vec4,
    wind: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub(crate) struct GrassMaterial {
    #[uniform(0)]
    field: GrassUniform,
    #[texture(1, filterable = false, visibility(vertex))]
    roots: Handle<Image>,
    #[texture(2, visibility(vertex))]
    #[sampler(3, visibility(vertex))]
    bend: Handle<Image>,
}

impl Material for GrassMaterial {
    fn vertex_shader() -> ShaderRef {
        ShaderRef::Handle(SHADER)
    }
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(SHADER)
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

#[derive(Component)]
struct GrassField {
    profile: Grass,
    lods: [Handle<Mesh>; 3],
    image: Handle<Image>,
    height_min: f32,
    height_range: f32,
    bend: BendMap,
    elapsed: f32,
    previous_contacts: Vec<(Entity, Vec3)>,
}

#[derive(Component)]
struct GrassTile;

pub(super) fn install(app: &mut App) {
    interaction::install(app);
    app.add_plugins(MaterialPlugin::<GrassMaterial>::default())
        .add_systems(
            PostUpdate,
            update
                .after(VisibilitySystems::UpdateFrusta)
                .before(VisibilitySystems::VisibilityPropagate),
        );
    bevy::asset::load_internal_asset!(app, SHADER, "grass.wgsl", Shader::from_wgsl);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<GrassMaterial>,
    packs: &GraphicsPacks,
    stage: &str,
    source: &meteor_asset::gmdl::GmdlMesh,
    description: Option<&meteor_asset::des::StageDescription>,
    faces: &std::collections::BTreeSet<u32>,
    support: Option<&crate::plugins::stage_collider::StageMeshCollider>,
    center: [f32; 3],
) {
    let Some(profile) = packs
        .scenes(stage)
        .filter_map(|(_, scene)| scene.grass.as_ref())
        .last()
    else {
        return;
    };
    if !profile.enabled {
        return;
    }
    let support = support.map(|mesh| mesh as &dyn meteor_physics::collision::MeshCollider);
    let tiles = match placement::place(profile, source, description, faces, support, center) {
        Ok(tiles) if !tiles.is_empty() => tiles,
        Ok(_) => {
            warn!("{stage}: grass area contains no eligible ground");
            return;
        }
        Err(error) => {
            warn!("{stage}: {error}");
            return;
        }
    };
    let wind = packs
        .scenes(stage)
        .filter_map(|(_, scene)| scene.wind.as_ref())
        .last()
        .map_or_else(super::config::Wind::default, Clone::clone);
    let mut bytes = Vec::with_capacity(tiles.len() * ROOTS_PER_TILE * 16);
    for tile in &tiles {
        for root in &tile.roots {
            for value in root.to_array() {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes.resize(bytes.len() + (ROOTS_PER_TILE - tile.roots.len()) * 16, 0);
    }
    let roots = images.add(Image::new(
        Extent3d {
            width: ROOTS_PER_TILE as u32,
            height: tiles.len() as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        bytes,
        TextureFormat::Rgba32Float,
        RenderAssetUsages::RENDER_WORLD,
    ));
    let bend = images.add(BendMap::image());
    let height_min = tiles
        .iter()
        .map(|tile| tile.min.y)
        .fold(f32::INFINITY, f32::min)
        - 1.0;
    let height_max = tiles
        .iter()
        .map(|tile| tile.max.y)
        .fold(f32::NEG_INFINITY, f32::max)
        + 1.0;
    let height_range = height_max - height_min;
    let wind_direction = Vec2::from_array(wind.direction).normalize_or_zero();
    let material = materials.add(GrassMaterial {
        roots,
        bend: bend.clone(),
        field: GrassUniform {
            root_color: Vec3::from_array(profile.root_color).extend(0.0),
            tip_color: Vec3::from_array(profile.tip_color).extend(0.0),
            blade: Vec4::new(
                profile.height[0],
                profile.height[1],
                profile.width[0],
                profile.width[1],
            ),
            distances: Vec4::new(
                profile.near_distance,
                profile.mid_distance,
                profile.max_distance,
                0.0,
            ),
            contact: Vec4::new(
                height_min,
                height_range,
                interaction::SPACING,
                interaction::RANGE,
            ),
            wind: Vec4::new(
                wind_direction.x,
                wind_direction.y,
                profile.wind_strength,
                wind.frequency.max(0.2),
            ),
        },
    });
    let lods = [
        meshes.add(blades(ROOTS_PER_TILE, true)),
        meshes.add(blades(MID_ROOTS, true)),
        meshes.add(blades(FAR_ROOTS, false)),
    ];
    let root_count: usize = tiles.iter().map(|tile| tile.roots.len()).sum();
    info!(
        "{stage}: interactive grass placed {root_count} blades in {} tiles",
        tiles.len()
    );
    commands
        .spawn((
            PlayingSceneEntity,
            GrassField {
                profile: profile.clone(),
                lods: lods.clone(),
                image: bend,
                height_min,
                height_range,
                bend: BendMap::default(),
                elapsed: 0.0,
                previous_contacts: Vec::new(),
            },
            Transform::IDENTITY,
            Visibility::Inherited,
        ))
        .with_children(|parent| {
            let pad = Vec3::splat(profile.height[1] * 2.1 + profile.width[1]);
            for (i, tile) in tiles.into_iter().enumerate() {
                parent.spawn((
                    GrassTile,
                    MeshTag(i as u32),
                    Mesh3d(lods[0].clone()),
                    MeshMaterial3d(material.clone()),
                    Aabb::from_min_max(tile.min - pad, tile.max + pad),
                    NoAutoAabb,
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    bevy::light::NotShadowCaster,
                ));
            }
        });
}

fn blades(count: usize, curved: bool) -> Mesh {
    let mut vertices = Vec::with_capacity(count * if curved { 5 } else { 3 });
    let mut indices = Vec::with_capacity(count * if curved { 9 } else { 3 });
    for i in 0..count {
        let first = vertices.len() as u16;
        let blade = i as f32;
        vertices.extend([[blade, 0.0, -1.0], [blade, 0.0, 1.0]]);
        if curved {
            vertices.extend([[blade, 0.52, -1.0], [blade, 0.52, 1.0]]);
            indices.extend([
                first,
                first + 1,
                first + 2,
                first + 2,
                first + 1,
                first + 3,
                first + 2,
                first + 3,
                first + 4,
            ]);
        } else {
            indices.extend([first, first + 1, first + 2]);
        }
        vertices.push([blade, 1.0, 0.0]);
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vertices)
    .with_inserted_indices(Indices::U16(indices))
}

fn update(
    settings: Option<Res<Settings>>,
    state: Option<Res<State<AppState>>>,
    time: Res<Time>,
    camera: Query<(&GlobalTransform, &Frustum), With<GameplayCamera>>,
    mut field: Query<&mut GrassField>,
    mut tiles: Query<(Entity, &Aabb, &mut Visibility, &mut Mesh3d), With<GrassTile>>,
    actors: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&MotionState>,
            Option<&Health>,
            Option<&Concealment>,
        ),
        With<Fighter>,
    >,
    mut visible: Local<Vec<(f32, Entity)>>,
    mut upload: ResMut<BendUpload>,
) {
    let Ok(mut field) = field.single_mut() else {
        upload.data = None;
        return;
    };
    let Ok((camera, frustum)) = camera.single() else {
        return;
    };
    let eye = camera.translation();
    let enabled = settings.as_ref().is_none_or(|s| s.grass_enhance);
    let compact = cfg!(any(target_arch = "wasm32", target_os = "android"));
    let max_tiles = if compact {
        field.profile.max_visible_tiles.min(40)
    } else {
        field.profile.max_visible_tiles
    };
    let [near, mid, far] = [
        field.profile.near_distance,
        field.profile.mid_distance,
        field.profile.max_distance,
    ];
    visible.clear();
    for (entity, aabb, mut visibility, _) in &mut tiles {
        let distance = ((eye - Vec3::from(aabb.center)).abs() - Vec3::from(aabb.half_extents))
            .max(Vec3::ZERO)
            .length_squared();
        if enabled
            && distance < far * far
            && frustum.intersects_obb(aabb, &bevy::math::Affine3A::IDENTITY, false, false)
        {
            visible.push((distance, entity));
        } else {
            visibility.set_if_neq(Visibility::Hidden);
        }
    }
    if visible.len() > max_tiles {
        visible.select_nth_unstable_by(max_tiles, |(a, e), (b, f)| {
            a.total_cmp(b).then_with(|| e.to_bits().cmp(&f.to_bits()))
        });
    }
    for (i, &(distance, entity)) in visible.iter().enumerate() {
        let Ok((_, _, mut visibility, mut mesh)) = tiles.get_mut(entity) else {
            continue;
        };
        if i >= max_tiles {
            visibility.set_if_neq(Visibility::Hidden);
        } else {
            visibility.set_if_neq(Visibility::Inherited);
            let level = if distance >= mid * mid {
                2
            } else if compact || distance >= near * near {
                1
            } else {
                0
            };
            if mesh.0 != field.lods[level] {
                mesh.0 = field.lods[level].clone();
            }
        }
    }
    if !enabled || !state.is_some_and(|state| *state.get() == AppState::Playing) {
        field.previous_contacts.clear();
        return;
    }
    field.elapsed += time.delta_secs().min(0.1);
    if field.elapsed < CONTACT_INTERVAL {
        return;
    }
    let remainder = field.elapsed % CONTACT_INTERVAL;
    let factor = (-4.0 * (field.elapsed - remainder) / field.profile.recovery_seconds).exp();
    field.elapsed = remainder;
    let mut dirty = field.bend.decay(eye.xz(), factor);
    let mut contacts = actors
        .iter()
        .filter_map(|(id, transform, motion, health, conceal)| {
            if health.is_some_and(|h| h.current <= 0.0)
                || conceal.is_some_and(|c| c.remaining > 0.0)
                || motion.is_some_and(|motion| {
                    matches!(
                        motion.movement_mode,
                        MovementMode::Airborne | MovementMode::WallRun
                    )
                })
            {
                return None;
            }
            let feet = transform.translation();
            if !feet.is_finite()
                || feet.y < field.height_min
                || feet.y > field.height_min + field.height_range
                || (feet.xz() - eye.xz()).abs().max_element() > interaction::RANGE
            {
                return None;
            }
            Some((id, feet))
        })
        .collect::<Vec<_>>();
    contacts.sort_unstable_by(|(a, p), (b, q)| {
        p.distance_squared(eye)
            .total_cmp(&q.distance_squared(eye))
            .then_with(|| a.to_bits().cmp(&b.to_bits()))
    });
    contacts.truncate(CONTACTS);
    let radius = field.profile.interaction_radius;
    for &(id, feet) in &contacts {
        let previous = field
            .previous_contacts
            .iter()
            .find(|(actor, _)| *actor == id)
            .map(|(_, previous)| *previous)
            .filter(|&previous| previous.distance_squared(feet) <= 16.0)
            .unwrap_or(feet);
        dirty |= field.bend.stamp(previous, feet, eye.xz(), radius);
    }
    field.previous_contacts = contacts;
    if dirty {
        upload.revision = upload.revision.wrapping_add(1);
        upload.data = Some((
            field.image.clone(),
            Arc::new(field.bend.pixels(field.height_min, field.height_range)),
        ));
    }
}
