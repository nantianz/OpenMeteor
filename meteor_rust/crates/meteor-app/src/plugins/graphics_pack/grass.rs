mod geometry;
mod interaction;
mod placement;
#[cfg(test)]
mod tests;

use super::{config::Grass, GraphicsPacks};
use crate::app_state::{AppState, Settings};
use crate::components::fighter::{Concealment, Fighter, Health, MotionState};
use crate::plugins::stage_material::SharedBuffers;
use crate::plugins::{camera_plugin::GameplayCamera, render_plugin::PlayingSceneEntity};
use bevy::asset::RenderAssetUsages;
use bevy::camera::{
    primitives::{Aabb, Frustum},
    visibility::{NoAutoAabb, VisibilitySystems},
};
use bevy::ecs::system::SystemParamItem;
use bevy::mesh::{MeshTag, MeshVertexBufferLayoutRef};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::{
    render_resource::{
        binding_types::uniform_buffer, AsBindGroup, AsBindGroupError, BindGroupLayout,
        BindGroupLayoutEntry, Extent3d, OwnedBindingResource, RenderPipelineDescriptor,
        ShaderStages, ShaderType, SpecializedMeshPipelineError, TextureDimension, TextureFormat,
        UnpreparedBindGroup,
    },
    renderer::RenderDevice,
};
use bevy::shader::{Shader, ShaderRef};
use geometry::{DISTANT, LODS};
use interaction::{BendMap, BendUpload};
use meteor_core::types::MovementMode;
use placement::ROOTS_PER_TILE;
use std::sync::Arc;

const SHADER: Handle<Shader> = bevy::asset::uuid_handle!("9a315511-160c-4087-af9c-ef5cd526b723");
const CLUMP_BUCKET_SIZE: usize = ROOTS_PER_TILE / 4;
const COMPACT_TRIANGLE_BUDGET: usize = 28_000;
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
    wind_environment: Vec4,
}

#[derive(AsBindGroup, Clone)]
pub(crate) struct GrassBindings {
    #[uniform(0)]
    field: GrassUniform,
    #[texture(1, filterable = false, visibility(vertex))]
    roots: Handle<Image>,
    #[texture(2, visibility(vertex))]
    #[sampler(3, visibility(vertex))]
    bend: Handle<Image>,
}

#[derive(Asset, TypePath, Clone)]
pub(crate) struct GrassMaterial(GrassBindings);

impl AsBindGroup for GrassMaterial {
    type Data = ();
    type Param = (
        <GrassBindings as AsBindGroup>::Param,
        Res<'static, SharedBuffers>,
    );

    fn label() -> &'static str {
        "grass"
    }
    fn bind_group_data(&self) -> Self::Data {}

    fn unprepared_bind_group(
        &self,
        layout: &BindGroupLayout,
        device: &RenderDevice,
        (bindings, buffers): &mut SystemParamItem<'_, '_, Self::Param>,
        force_no_bindless: bool,
    ) -> Result<UnpreparedBindGroup, AsBindGroupError> {
        let mut result =
            self.0
                .unprepared_bind_group(layout, device, bindings, force_no_bindless)?;
        result
            .bindings
            .push((4, OwnedBindingResource::Buffer(buffers.clock()?)));
        result
            .bindings
            .push((5, OwnedBindingResource::Buffer(buffers.grass_light()?)));
        Ok(result)
    }

    fn bind_group_layout_entries(
        device: &RenderDevice,
        force_no_bindless: bool,
    ) -> Vec<BindGroupLayoutEntry> {
        let mut entries = GrassBindings::bind_group_layout_entries(device, force_no_bindless);
        entries.push(uniform_buffer::<Vec4>(false).build(4, ShaderStages::VERTEX));
        entries.push(uniform_buffer::<Vec4>(false).build(5, ShaderStages::VERTEX));
        entries
    }
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
        layout: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            geometry::LEAF.at_shader_location(1),
        ])?];
        Ok(())
    }
}

#[derive(Component)]
pub(super) struct GrassField {
    profile: Grass,
    lods: [[Handle<Mesh>; LODS.len()]; 4],
    image: Handle<Image>,
    height_min: f32,
    height_range: f32,
    bend: BendMap,
    elapsed: f32,
    previous_contacts: Vec<(Entity, Vec3)>,
}

#[derive(Component)]
struct GrassTile {
    bucket: usize,
}

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
pub(crate) fn spawn<'a>(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<GrassMaterial>,
    packs: &'a GraphicsPacks,
    stage: &'a str,
    source: &meteor_asset::gmdl::GmdlMesh,
    description: Option<&meteor_asset::des::StageDescription>,
    faces: &std::collections::BTreeSet<u32>,
    support: Option<&crate::plugins::stage_collider::StageMeshCollider>,
    center: [f32; 3],
) -> Option<&'a Grass> {
    let Some(profile) = packs
        .scenes(stage)
        .filter_map(|(_, scene)| scene.grass.as_ref())
        .last()
    else {
        return None;
    };
    if !profile.enabled {
        return None;
    }
    let support = support.map(|mesh| mesh as &dyn meteor_physics::collision::MeshCollider);
    let tiles = match placement::place(profile, source, description, faces, support, center) {
        Ok(tiles) if !tiles.is_empty() => tiles,
        Ok(_) => {
            warn!("{stage}: grass area contains no eligible ground");
            return None;
        }
        Err(error) => {
            warn!("{stage}: {error}");
            return None;
        }
    };
    let fallback = super::config::Wind::default();
    let wind = packs.wind(stage).unwrap_or(&fallback);
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
    let (flow, wind_environment) = wind.response.map_or_else(
        || {
            (
                wind_direction
                    .extend(profile.wind_strength)
                    .extend(wind.frequency.max(0.2)),
                Vec4::ZERO,
            )
        },
        |response| {
            (
                super::wind_flow(wind, response.grass * profile.wind_strength),
                Vec4::new(wind.spatial_scale, 1.0, wind_direction.x, wind_direction.y),
            )
        },
    );
    let material = materials.add(GrassMaterial(GrassBindings {
        roots,
        bend: bend.clone(),
        field: GrassUniform {
            root_color: Vec3::from_array(profile.root_color).extend(profile.clump_radius),
            tip_color: Vec3::from_array(profile.tip_color).extend(0.0),
            blade: Vec4::new(
                profile.height[0],
                profile.height[1],
                profile.width[0],
                profile.width[1],
            ),
            distances: Vec4::from_array(profile.lod_distances()),
            contact: Vec4::new(
                height_min,
                height_range,
                interaction::SPACING,
                interaction::RANGE,
            ),
            wind: flow,
            wind_environment,
        },
    }));
    let lods: [[Handle<Mesh>; LODS.len()]; 4] = std::array::from_fn(|bucket| {
        LODS.each_ref()
            .map(|lod| meshes.add(lod.mesh(clump_capacity(bucket))))
    });
    let root_count: usize = tiles.iter().map(|tile| tile.roots.len()).sum();
    info!(
        "{stage}: interactive grass placed {root_count} clumps in {} tiles",
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
            let pad =
                Vec3::splat(profile.height[1] * 4.1 + profile.width[1] + profile.clump_radius);
            for (i, tile) in tiles.into_iter().enumerate() {
                let bucket = (tile.roots.len() - 1) / CLUMP_BUCKET_SIZE;
                parent.spawn((
                    GrassTile { bucket },
                    MeshTag(i as u32),
                    Mesh3d(lods[bucket][DISTANT].clone()),
                    MeshMaterial3d(material.clone()),
                    Aabb::from_min_max(tile.min - pad, tile.max + pad),
                    NoAutoAabb,
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    bevy::light::NotShadowCaster,
                ));
            }
        });
    Some(profile)
}

fn clump_capacity(bucket: usize) -> usize {
    (bucket + 1) * CLUMP_BUCKET_SIZE
}

fn upgrade_lod(desired: usize, bucket: usize, remaining: &mut usize) -> usize {
    let capacity = clump_capacity(bucket);
    let baseline = LODS[DISTANT].triangles(capacity);
    for (level, lod) in LODS.iter().enumerate().take(DISTANT).skip(desired) {
        let extra = lod.triangles(capacity) - baseline;
        if extra <= *remaining {
            *remaining -= extra;
            return level;
        }
    }
    DISTANT
}

pub(crate) fn ground_material(
    profile: &Grass,
    object: &str,
    texture: &str,
    material: &mut crate::plugins::stage_material::GmdlMaterial,
) {
    if !profile
        .surfaces
        .iter()
        .any(|target| target.matches(object, texture))
    {
        return;
    }
    let [min, max] = profile.bounds.unwrap_or([[-65536.0; 2], [65536.0; 2]]);
    material.grass_ground = Vec3::from_array(profile.tip_color).extend(profile.ground_cover);
    material.grass_bounds = Vec4::new(min[0], min[1], max[0], max[1]);
}

fn update(
    settings: Option<Res<Settings>>,
    state: Option<Res<State<AppState>>>,
    time: Res<Time>,
    camera: Query<(&GlobalTransform, &Frustum), With<GameplayCamera>>,
    mut field: Query<&mut GrassField>,
    mut tiles: Query<(Entity, &Aabb, &GrassTile, &mut Visibility, &mut Mesh3d)>,
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
    mut visible: Local<Vec<(f32, Entity, usize)>>,
    mut upload: ResMut<BendUpload>,
) {
    let Ok(mut field) = field.single_mut() else {
        if upload.data.is_some() {
            upload.data = None;
        }
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
    let [near, mid, detail, far] = field.profile.lod_distances();
    visible.clear();
    for (entity, aabb, tile, mut visibility, _) in &mut tiles {
        let distance = ((eye - Vec3::from(aabb.center)).abs() - Vec3::from(aabb.half_extents))
            .max(Vec3::ZERO)
            .length_squared();
        if enabled
            && distance < far * far
            && frustum.intersects_obb(aabb, &bevy::math::Affine3A::IDENTITY, false, false)
        {
            visible.push((distance, entity, tile.bucket));
        } else {
            visibility.set_if_neq(Visibility::Hidden);
        }
    }
    visible.sort_unstable_by(|(a, e, _), (b, f, _)| {
        a.total_cmp(b).then_with(|| e.to_bits().cmp(&f.to_bits()))
    });
    let mut budget = if compact {
        field.profile.max_triangles.min(COMPACT_TRIANGLE_BUDGET)
    } else {
        field.profile.max_triangles
    };
    let far_cost = LODS[DISTANT].triangles(ROOTS_PER_TILE);
    let drawn_tiles = visible.len().min(max_tiles).min(budget / far_cost);
    let foreground = if drawn_tiles == visible.len() {
        drawn_tiles
    } else {
        drawn_tiles * 3 / 4
    };
    let coverage_slots = drawn_tiles - foreground;
    let coverage_candidates = visible.len() - foreground;
    let selected = |i: usize| {
        if i < foreground {
            return true;
        }
        let rank = i - foreground;
        rank * coverage_slots / coverage_candidates
            != (rank + 1) * coverage_slots / coverage_candidates
    };
    budget -= visible
        .iter()
        .enumerate()
        .filter(|(i, _)| selected(*i))
        .map(|(_, &(_, _, bucket))| LODS[DISTANT].triangles(clump_capacity(bucket)))
        .sum::<usize>();
    for (i, &(distance, entity, bucket)) in visible.iter().enumerate() {
        let Ok((_, _, _, mut visibility, mut mesh)) = tiles.get_mut(entity) else {
            continue;
        };
        if !selected(i) {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        let desired = if distance >= detail * detail {
            DISTANT
        } else if distance >= mid * mid {
            2
        } else if compact || distance >= near * near {
            1
        } else {
            0
        };
        let level = upgrade_lod(desired, bucket, &mut budget);
        let lod = &field.lods[bucket][level];
        visibility.set_if_neq(Visibility::Inherited);
        if mesh.0 != *lod {
            mesh.0 = lod.clone();
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
