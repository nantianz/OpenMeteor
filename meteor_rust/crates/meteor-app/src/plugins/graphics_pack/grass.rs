mod combat;
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
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
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
pub(crate) use combat::GrassCombat;
use geometry::{DISTANT, LODS};
use interaction::{BendMap, BendUpload};
use meteor_core::types::MovementMode;
use meteor_physics::collision::MeshCollider;
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
    meadow: Vec4,
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
        result
            .bindings
            .push((6, OwnedBindingResource::Buffer(buffers.grass_direction()?)));
        result
            .bindings
            .push((7, OwnedBindingResource::Buffer(buffers.lighting()?)));
        Ok(result)
    }

    fn bind_group_layout_entries(
        device: &RenderDevice,
        force_no_bindless: bool,
    ) -> Vec<BindGroupLayoutEntry> {
        let mut entries = GrassBindings::bind_group_layout_entries(device, force_no_bindless);
        entries.push(uniform_buffer::<Vec4>(false).build(4, ShaderStages::VERTEX));
        entries.push(uniform_buffer::<Vec4>(false).build(5, ShaderStages::VERTEX));
        entries.push(uniform_buffer::<Vec4>(false).build(6, ShaderStages::VERTEX));
        entries.push(uniform_buffer::<Vec4>(false).build(7, ShaderStages::FRAGMENT));
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
        descriptor.multisample.alpha_to_coverage_enabled = descriptor.multisample.count > 1;
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
    lod_initialized: bool,
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
    roots: Aabb,
    row: u32,
    lod: f32,
    target_lod: usize,
    distance_lod: usize,
    in_view: bool,
}

const HIDDEN_LOD: usize = LODS.len();
const LOD_TRANSITION_SECONDS: f32 = 0.16;
const LOD_PHASE_STEPS: f32 = 256.0;
pub(super) fn install(app: &mut App) {
    interaction::install(app);
    app.init_resource::<GrassCombat>()
        .add_plugins(MaterialPlugin::<GrassMaterial>::default())
        .add_systems(
            PostUpdate,
            update
                .after(VisibilitySystems::UpdateFrusta)
                .before(VisibilitySystems::VisibilityPropagate),
        );
    bevy::asset::load_internal_asset!(app, SHADER, "grass.wgsl", Shader::from_wgsl);
}

#[allow(clippy::too_many_arguments)]
pub(in crate::plugins) fn spawn<'a>(
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
    campfires: &[super::campfires::Placement],
) -> Option<GrassGround<'a>> {
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
    let (mut tiles, cover) =
        match placement::place(profile, source, description, faces, support, center) {
            Ok(placement) => placement,
            Err(error) => {
                warn!("{stage}: {error}");
                return None;
            }
        };
    if !campfires.is_empty() {
        for tile in &mut tiles {
            tile.roots.retain(|root| {
                !campfires
                    .iter()
                    .any(|fire| fire.clears_grass(root.truncate()))
            });
        }
        tiles.retain(|tile| !tile.roots.is_empty());
    }
    if tiles.is_empty() {
        warn!("{stage}: grass area contains no eligible ground");
        return None;
    }
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
            tip_color: Vec3::from_array(profile.tip_color)
                .extend(f32::from(profile.plume_color.is_some())),
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
            meadow: Vec3::from_array(profile.plume_color.unwrap_or_default())
                .extend(profile.patch_variation),
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
    let bounds = cover.bounds();
    let underlay = packs
        .scenes(stage)
        .filter_map(|(pack, scene)| scene.grass_ground.as_ref().map(|ground| (pack, ground)))
        .last()
        .and_then(|(pack, ground)| {
            let source = pack.images.get(&ground.image)?;
            let (bytes, mip_count) = super::surfaces::mip_chain(source, true);
            let mut image = Image::new_uninit(
                Extent3d {
                    width: source.width,
                    height: source.height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::RENDER_WORLD,
            );
            image.data = Some(bytes);
            image.texture_descriptor.mip_level_count = mip_count;
            Some((images.add(image), ground.world_size.recip()))
        });
    let coverage = if underlay.is_some() {
        let mut image = Image::new(
            Extent3d {
                width: cover.size.x,
                height: cover.size.y,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            placement::ground_channels(&cover, &tiles),
            TextureFormat::Rg8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        );

        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            mipmap_filter: ImageFilterMode::Linear,
            ..default()
        });
        image
    } else {
        cover.into_image()
    };
    commands
        .spawn((
            PlayingSceneEntity,
            GrassField {
                profile: profile.clone(),
                lods: lods.clone(),
                lod_initialized: false,
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
                    GrassTile {
                        bucket,
                        roots: Aabb::from_min_max(tile.min, tile.max),
                        row: i as u32,
                        lod: HIDDEN_LOD as f32,
                        target_lod: HIDDEN_LOD,
                        distance_lod: DISTANT,
                        in_view: false,
                    },
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
    Some(GrassGround {
        profile,
        bounds,
        coverage: images.add(coverage),
        underlay,
    })
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
fn lod_cost(lod: f32, bucket: usize) -> usize {
    LODS.get(lod as usize)
        .map_or(0, |mesh| mesh.triangles(clump_capacity(bucket)))
}
fn distance_lod(
    distance_squared: f32,
    boundaries: [f32; 3],
    previous: usize,
    compact: bool,
) -> usize {
    const HYSTERESIS: f32 = 1.5;
    let minimum = usize::from(compact);
    let mut level = previous.clamp(minimum, DISTANT);
    while level < DISTANT && distance_squared >= (boundaries[level] + HYSTERESIS).powi(2) {
        level += 1;
    }
    while level > minimum && distance_squared < boundaries[level - 1].powi(2) {
        level -= 1;
    }
    level
}

pub(crate) struct GrassGround<'a> {
    profile: &'a Grass,
    bounds: Vec4,
    coverage: Handle<Image>,
    underlay: Option<(Handle<Image>, f32)>,
}

impl GrassGround<'_> {
    pub(crate) fn configure(
        &self,
        object: &str,
        texture: &str,
        material: &mut crate::plugins::stage_material::GmdlMaterial,
    ) {
        if self
            .profile
            .surfaces
            .iter()
            .any(|target| target.matches(object, texture))
        {
            material.grass_ground =
                Vec3::from_array(self.profile.tip_color).extend(self.profile.ground_cover);
            material.grass_bounds = self.bounds;
            material.grass_coverage = Some(self.coverage.clone());
            if let Some((texture, scale)) = &self.underlay {
                material.grass_underlay = Some(texture.clone());
                material.grass_underlay_scale = *scale;
            }
        }
    }
}

fn update(
    settings: Option<Res<Settings>>,
    state: Option<Res<State<AppState>>>,
    time: Res<Time>,
    real_time: Res<Time<Real>>,
    camera: Query<(&GlobalTransform, &Frustum), With<GameplayCamera>>,
    mut field: Query<&mut GrassField>,
    mut tiles: Query<(
        Entity,
        &Aabb,
        &mut GrassTile,
        &mut Visibility,
        &mut Mesh3d,
        &mut MeshTag,
    )>,
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
    mut candidates: Local<Vec<(f32, Entity, usize)>>,
    mut chosen: Local<Vec<bool>>,
    mut order: Local<Vec<usize>>,
    mut upload: ResMut<BendUpload>,
    mut combat: ResMut<GrassCombat>,
    collider: Option<Res<crate::plugins::stage_collider::StageCollider>>,
) {
    let Ok(mut field) = field.single_mut() else {
        if upload.data.is_some() {
            upload.data = None;
        }
        combat.set_eye(None);
        return;
    };
    let Ok((camera, frustum)) = camera.single() else {
        combat.set_eye(None);
        return;
    };
    let eye = camera.translation();
    let enabled = settings.as_ref().is_none_or(|s| s.grass_enhance);
    let running = enabled && state.is_some_and(|state| *state.get() == AppState::Playing);
    let ground = collider
        .as_deref()
        .and_then(|collider| collider.mesh.as_ref());
    combat.set_eye((running && ground.is_some()).then_some(eye.xz()));
    if running
        && collider
            .as_ref()
            .is_some_and(|collider| collider.is_changed())
    {
        combat.invalidate_ground();
    }
    let compact = cfg!(any(target_arch = "wasm32", target_os = "android"));
    let max_tiles = if compact {
        field.profile.max_visible_tiles.min(40)
    } else {
        field.profile.max_visible_tiles
    };
    let [near, mid, detail, far] = field.profile.lod_distances();
    candidates.clear();
    for (entity, aabb, mut tile, _, _, _) in &mut tiles {
        let distance = ((eye - Vec3::from(tile.roots.center)).abs()
            - Vec3::from(tile.roots.half_extents))
        .max(Vec3::ZERO)
        .length_squared();
        let in_view = enabled
            && distance < far * far
            && frustum.intersects_obb(aabb, &bevy::math::Affine3A::IDENTITY, false, false);

        tile.in_view = in_view;
        tile.target_lod = HIDDEN_LOD;
        if enabled && distance < far * far && (!compact || in_view) {
            candidates.push((distance, entity, tile.bucket));
        }
    }
    candidates.sort_unstable_by(|(a, e, _), (b, f, _)| {
        a.total_cmp(b).then_with(|| e.to_bits().cmp(&f.to_bits()))
    });
    let budget = if compact {
        field.profile.max_triangles.min(COMPACT_TRIANGLE_BUDGET)
    } else {
        field.profile.max_triangles
    };
    let mut remaining = budget;
    let far_cost = LODS[DISTANT].triangles(ROOTS_PER_TILE);
    let drawn_tiles = candidates.len().min(max_tiles).min(budget / far_cost);
    let foreground = if drawn_tiles == candidates.len() {
        drawn_tiles
    } else {
        drawn_tiles * 3 / 4
    };
    let coverage_slots = drawn_tiles - foreground;

    chosen.clear();
    chosen.resize(candidates.len(), false);
    chosen[..foreground].fill(true);
    if coverage_slots > 0 {
        order.clear();
        order.extend(foreground..candidates.len());
        let key = |i: usize| {
            let h = (candidates[i].1.to_bits()).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            (h ^ (h >> 29)).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        };
        if coverage_slots < order.len() {
            order.select_nth_unstable_by_key(coverage_slots, |&i| key(i));
        }
        for &i in &order[..coverage_slots.min(order.len())] {
            chosen[i] = true;
        }
    }
    let selected = |i: usize| chosen[i];
    remaining -= candidates
        .iter()
        .enumerate()
        .filter(|(i, _)| selected(*i))
        .map(|(_, &(_, _, bucket))| LODS[DISTANT].triangles(clump_capacity(bucket)))
        .sum::<usize>();
    for (i, &(distance, entity, bucket)) in candidates.iter().enumerate() {
        if !selected(i) {
            continue;
        }
        if let Ok((_, _, mut tile, _, _, _)) = tiles.get_mut(entity) {
            let previous = if field.lod_initialized {
                tile.distance_lod
            } else {
                DISTANT
            };
            tile.distance_lod = distance_lod(distance, [near, mid, detail], previous, compact);
            tile.target_lod = upgrade_lod(tile.distance_lod, bucket, &mut remaining);
        }
    }
    let step = real_time.delta_secs().min(0.05) / LOD_TRANSITION_SECONDS;
    let mut triangles = 0;
    let mut active_tiles = 0;
    for (_, _, mut tile, _, _, _) in &mut tiles {
        let target = tile.target_lod as f32;
        if !field.lod_initialized || !enabled {
            tile.lod = target;
        } else if tile.lod < target {
            tile.lod = (tile.lod + step).min(target);
        }
        let cost = lod_cost(tile.lod, tile.bucket);
        triangles += cost;
        active_tiles += usize::from(cost > 0);
    }
    field.lod_initialized = enabled;
    for &(_, entity, _) in &candidates {
        let Ok((_, _, mut tile, _, _, _)) = tiles.get_mut(entity) else {
            continue;
        };
        let target = tile.target_lod as f32;
        if tile.lod <= target {
            continue;
        }
        let next = (tile.lod - step).max(target);
        let cost = lod_cost(tile.lod, tile.bucket);
        let next_cost = lod_cost(next, tile.bucket);
        let extra = next_cost - cost;
        let new_tile = usize::from(cost == 0 && next_cost > 0);
        if triangles + extra <= budget && active_tiles + new_tile <= max_tiles {
            tile.lod = next;
            triangles += extra;
            active_tiles += new_tile;
        }
    }
    for (_, _, tile, mut visibility, mut mesh, mut tag) in &mut tiles {
        let level = tile.lod as usize;
        let visible = tile.in_view && level < HIDDEN_LOD;
        visibility.set_if_neq(if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        if level < HIDDEN_LOD {
            let lod = &field.lods[tile.bucket][level];
            if mesh.0 != *lod {
                mesh.0 = lod.clone();
            }
            let phase = (tile.lod * LOD_PHASE_STEPS) as u32;
            let packed = tile.row | (phase << 16);
            if tag.0 != packed {
                tag.0 = packed;
            }
        }
    }
    if !running {
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
    if let Some(ground) = ground {
        let blade_height = field.profile.height[1] * 1.35;
        dirty |= combat.paint(
            &mut field.bend,
            eye.xz(),
            blade_height,
            |p, reach, headroom| {
                ground
                    .sweep_foot_segment(p.x, p.y + headroom, p.z, p.y - reach)
                    .filter(|hit| hit.ny >= 0.42)
                    .map(|hit| p.with_y(hit.y))
            },
        );
    }
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
