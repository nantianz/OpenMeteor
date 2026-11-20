mod field;
mod terrain;

use std::collections::{HashMap, VecDeque};

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoFrustumCulling, VisibilitySystems};
use bevy::mesh::{MeshTag, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::{PrimitiveTopology, TextureFormat};
use meteor_core::MovementMode;
use meteor_physics::collision::MeshCollider;

use super::{config::Snow, GraphicsPacks};
use crate::app_state::AppState;
use crate::components::fighter::{Concealment, Fighter, Health, MotionState};
use crate::plugins::{
    camera_plugin::GameplayCamera,
    render_plugin::PlayingSceneEntity,
    stage_collider::StageCollider,
    stage_material::{GmdlMaterial, GmdlMaterialParams, GmdlMode},
};
use field::{Field, SnowUpload};
pub(in crate::plugins) use terrain::SnowMaterial;
use terrain::{Coverage, SnowLod, SnowTile};

const UPDATE_INTERVAL: f32 = 1.0 / 30.0;
const MAX_SURFACE_TRIANGLES: usize = 4096;
const MAX_PUFFS: usize = 96;
const MAX_ACTORS: usize = 12;
const MAX_CONTACTS: usize = 32;
const CONTACT_RANGE: f32 = 42.0;
const CONTACT_TOLERANCE: f32 = 0.32;
const PUFF_LIFE: f32 = 0.64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Imprint {
    Sole,
    Landing,
    Slide,
}

impl Imprint {
    fn extent(self) -> Vec2 {
        match self {
            Self::Sole => Vec2::new(0.22, 0.37),
            Self::Landing => Vec2::new(0.63, 0.50),
            Self::Slide => Vec2::new(0.39, 0.46),
        }
    }

    fn shape(self, point: Vec2) -> f32 {
        match self {
            Self::Sole => {
                let toe = (point - Vec2::new(0.045, 0.32)) / Vec2::new(0.88, 0.61);
                let arch = (point - Vec2::new(-0.08, -0.05)) / Vec2::new(0.56, 0.52);
                let heel = (point - Vec2::new(-0.11, -0.42)) / Vec2::new(0.68, 0.48);
                toe.length_squared()
                    .min(arch.length_squared())
                    .min(heel.length_squared())
                    .sqrt()
            }
            Self::Landing => {
                (point.length() + 0.085 * (point.x * 7.2 + point.y * 4.6).sin()).max(0.0)
            }
            Self::Slide => point.length(),
        }
    }
}

#[derive(Clone, Copy)]
struct Contact {
    from: Vec3,
    position: Vec3,
    direction: Vec2,
    imprint: Imprint,
    left: bool,
    powder: u8,
}

struct Trail {
    position: Vec3,
    airborne: bool,
    remaining: f32,
    left: bool,
}

impl Trail {
    fn new(position: Vec3, mode: MovementMode) -> Self {
        Self {
            position,
            airborne: in_air(mode),
            remaining: 0.22,
            left: true,
        }
    }

    fn advance(
        &mut self,
        position: Vec3,
        mode: MovementMode,
        facing: f32,
        contacts: &mut Vec<Contact>,
    ) {
        let previous = std::mem::replace(&mut self.position, position);
        let was_airborne = std::mem::replace(&mut self.airborne, in_air(mode));
        let delta = position - previous;
        if self.airborne || delta.xz().length_squared() > 3.5 * 3.5 || delta.y.abs() > 4.0 {
            self.remaining = 0.22;
            return;
        }
        let heading = Vec2::new(facing.sin(), facing.cos());
        if was_airborne {
            if delta.y <= 0.08 {
                contacts.push(Contact {
                    from: position,
                    position,
                    direction: heading,
                    imprint: Imprint::Landing,
                    left: false,
                    powder: 12,
                });
            }
            return;
        }
        let distance = delta.xz().length();
        if distance <= 0.003 {
            return;
        }
        if mode == MovementMode::Dodge {
            const ROLL_POWDER_SPACING: f32 = 0.78;
            self.remaining = self.remaining.min(ROLL_POWDER_SPACING) - distance;
            let powder = if self.remaining <= 0.0 {
                self.remaining = self.remaining.rem_euclid(ROLL_POWDER_SPACING);
                4
            } else {
                0
            };
            contacts.push(Contact {
                from: previous,
                position,
                direction: delta.xz() / distance,
                imprint: Imprint::Slide,
                left: false,
                powder,
            });
            return;
        }

        let (stride, powder) = if mode == MovementMode::Run {
            (1.16, 3)
        } else {
            (0.76, 1)
        };
        self.remaining = self.remaining.min(stride);
        while self.remaining <= distance {
            let lateral = Vec2::new(heading.y, -heading.x) * if self.left { -0.19 } else { 0.19 };
            let point = previous.lerp(position, self.remaining / distance)
                + Vec3::new(lateral.x, 0.0, lateral.y);
            contacts.push(Contact {
                from: point,
                position: point,
                direction: heading,
                imprint: Imprint::Sole,
                left: self.left,
                powder,
            });
            self.left = !self.left;
            self.remaining += stride;
        }
        self.remaining -= distance;
    }
}

fn in_air(mode: MovementMode) -> bool {
    matches!(mode, MovementMode::Airborne | MovementMode::WallRun)
}

struct Puff {
    born: f32,
    origin: Vec3,
    velocity: Vec3,
    size: f32,
}

pub(in crate::plugins) struct SnowSurfaces {
    profile: Snow,
    triangles: Vec<[Vec3; 3]>,
}

impl SnowSurfaces {
    pub(in crate::plugins) fn for_stage(packs: &GraphicsPacks, stage: &str) -> Option<Self> {
        let profile = packs
            .scenes(stage)
            .filter_map(|(_, scene)| scene.snow.as_ref())
            .last()?;
        profile.enabled.then(|| Self {
            profile: profile.clone(),
            triangles: Vec::new(),
        })
    }

    pub(in crate::plugins) fn add_surface(
        &mut self,
        object: &str,
        texture: &str,
        mesh: &Mesh,
        transform: &Transform,
    ) {
        if !self
            .profile
            .surfaces
            .iter()
            .any(|selector| selector.matches(object, texture))
        {
            return;
        }
        let Some(VertexAttributeValues::Float32x3(points)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            return;
        };
        let Some(indices) = mesh.indices() else {
            return;
        };
        let to_world = |i| {
            points
                .get(i)
                .copied()
                .map(|p| transform.transform_point(Vec3::from_array(p)))
        };
        let mut indices = indices.iter();
        while let (Some(a), Some(b), Some(c)) = (indices.next(), indices.next(), indices.next()) {
            if self.triangles.len() > MAX_SURFACE_TRIANGLES {
                break;
            }
            if let (Some(a), Some(b), Some(c)) = (to_world(a), to_world(b), to_world(c)) {
                let normal = (b - a).cross(c - a).normalize_or_zero();
                if a.is_finite() && b.is_finite() && c.is_finite() && normal.y.abs() >= 0.6 {
                    self.triangles.push([a, b, c]);
                }
            }
        }
    }

    pub(in crate::plugins) fn spawn(
        self,
        stage: &str,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<GmdlMaterial>,
        snow_materials: &mut Assets<SnowMaterial>,
        images: &mut Assets<Image>,
    ) {
        let coverage = (self.triangles.len() <= MAX_SURFACE_TRIANGLES)
            .then(|| Coverage::build(&self.triangles, &self.profile))
            .flatten();
        let Some(coverage) = coverage else {
            warn!(
                "{stage}: snow surfaces must form a single near-flat layer within the raster budget"
            );
            return;
        };
        let field = Field::new(
            coverage.layout,
            images,
            self.profile.recovery_seconds,
            self.profile.imprint_strength,
        );
        let (lod, tile_count) = coverage.spawn(
            commands,
            meshes,
            images,
            snow_materials,
            field.image.clone(),
        );
        let size = coverage.layout.contact_size();
        info!(
            "{stage}: snow terrain {tile_count} tiles, contact field {}×{}",
            size.x, size.y
        );

        let mode = GmdlMode::Blend;
        let material = materials.add(GmdlMaterial::new_scene(
            GmdlMaterialParams {
                opacity: 1.0,
                alpha_cutoff: 0.0,
                mode: mode.as_u32(),
                has_texture: 1,
                uv_offset: Vec2::ZERO,
            },
            Some(images.add(powder_texture())),
            true,
            mode,
        ));
        commands.spawn((
            SnowField {
                coverage,
                field,
                lod,
                last_lod_cell: None,
                trails: HashMap::new(),
                puffs: VecDeque::new(),
                clock: 0.0,
                pending: 0.0,
            },
            PlayingSceneEntity,
            Mesh3d(meshes.add(PowderMesh::default().into_mesh())),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            NoFrustumCulling,
            bevy::light::NotShadowCaster,
        ));
    }
}

#[derive(Component)]
pub(in crate::plugins) struct SnowField {
    coverage: Coverage,
    field: Field,
    lod: SnowLod,
    last_lod_cell: Option<IVec2>,
    trails: HashMap<Entity, Trail>,
    puffs: VecDeque<Puff>,
    clock: f32,
    pending: f32,
}

impl SnowField {
    fn contact(&mut self, contact: Contact, ground: &impl MeshCollider) {
        let p = contact.position;
        let floor = self.coverage.layout.floor;
        if (p.y - floor).abs() > CONTACT_TOLERANCE
            || (contact.imprint != Imprint::Slide && self.coverage.at_world(p.xz()) == 0)
        {
            return;
        }
        let Some(hit) = ground
            .sweep_foot_segment(p.x, p.y + CONTACT_TOLERANCE, p.z, p.y - CONTACT_TOLERANCE)
            .filter(|hit| hit.ny >= 0.6 && (hit.y - floor).abs() < 0.16)
        else {
            return;
        };

        if !self.field.stamp(&self.coverage, contact, self.clock)
            || contact.powder == 0
            || self.coverage.at_world(p.xz()) == 0
        {
            return;
        }
        let thickness = self.coverage.layout.thickness;
        let origin = p.with_y(hit.y + 0.06 + thickness * 0.61);
        let seed = (p.x * 1.7 + p.z * 3.1 + self.clock * 29.0).rem_euclid(std::f32::consts::TAU);
        let energy = if contact.imprint == Imprint::Landing {
            1.0
        } else {
            0.64
        };
        for i in 0..contact.powder {
            let index = f32::from(i);
            let (sin, cos) = (seed + 2.4 * index).sin_cos();
            let speed = energy * (0.75 + 0.38 * (index % 4.0));
            self.puffs.push_back(Puff {
                born: self.clock,
                origin: origin + Vec3::new(cos * 0.13, 0.0, sin * 0.13),
                velocity: Vec3::new(
                    cos * speed,
                    energy * (1.35 + 0.24 * (index % 3.0)),
                    sin * speed,
                ),
                size: (0.09 + 0.03 * (index % 4.0)) * (0.55 + 0.45 * energy),
            });
        }
        self.puffs
            .drain(..self.puffs.len().saturating_sub(MAX_PUFFS));
    }

    fn draw_powder(&self, camera: &GlobalTransform) -> Mesh {
        let mut out = PowderMesh::default();
        let right = *camera.right();
        let up = *camera.up();
        for puff in &self.puffs {
            let age = self.clock - puff.born;
            let progress = age / PUFF_LIFE;
            let rise = (puff.velocity.y * age - 1.7 * age * age).max(0.0);
            let center = puff.origin + puff.velocity.with_y(0.0) * age + Vec3::Y * rise;
            let radius = puff.size * (0.68 + 1.25 * progress);
            let opacity = (1.0 - progress).powi(2) * 0.88;
            for uv in [
                Vec2::NEG_ONE,
                Vec2::new(1.0, -1.0),
                Vec2::ONE,
                Vec2::NEG_ONE,
                Vec2::ONE,
                Vec2::new(-1.0, 1.0),
            ] {
                out.push(
                    center + (right * uv.x + up * uv.y) * radius,
                    uv * 0.5 + 0.5,
                    opacity,
                );
            }
        }
        out.into_mesh()
    }

    #[cfg(test)]
    pub(in crate::plugins) fn resident_count(&self) -> usize {
        self.field.resident_count()
    }

    #[cfg(test)]
    pub(in crate::plugins) fn clock(&self) -> f32 {
        self.clock
    }

    #[cfg(test)]
    pub(in crate::plugins) fn has_deep_snow(&self, position: Vec2) -> bool {
        self.coverage.at_world(position) >= 160
    }
}

#[derive(Default)]
struct PowderMesh {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
}

impl PowderMesh {
    fn push(&mut self, point: Vec3, uv: Vec2, opacity: f32) {
        self.positions.push(point.to_array());
        self.uvs.push(uv.to_array());
        self.colors.push([1.0, 1.0, 1.0, opacity]);
    }

    fn into_mesh(mut self) -> Mesh {
        if self.positions.is_empty() {
            for _ in 0..3 {
                self.push(Vec3::ZERO, Vec2::ZERO, 0.0);
            }
        }
        let normals = vec![Vec3::Y.to_array(); self.positions.len()];
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
    }
}

pub(super) fn install(app: &mut App) {
    terrain::install(app);
    field::install(app);
    app.add_systems(
        PostUpdate,
        update
            .after(bevy::transform::TransformSystems::Propagate)
            .before(VisibilitySystems::CheckVisibility),
    );
}

fn update(
    state: Res<State<AppState>>,
    time: Res<Time>,
    collider: Option<Res<StageCollider>>,
    cameras: Query<&GlobalTransform, With<GameplayCamera>>,
    mut fields: Query<(&mut SnowField, &Mesh3d)>,
    mut tiles: Query<(&SnowTile, &mut Mesh3d, &mut MeshTag), Without<SnowField>>,
    fighters: Query<
        (
            Entity,
            &GlobalTransform,
            &MotionState,
            &Health,
            &Concealment,
        ),
        With<Fighter>,
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut contacts: Local<Vec<Contact>>,
    mut upload: ResMut<SnowUpload>,
) {
    let Ok((mut field, handle)) = fields.single_mut() else {
        if upload.is_bound() {
            *upload = SnowUpload::default();
        }
        return;
    };
    if *state.get() != AppState::Playing {
        if !field.trails.is_empty() {
            field.trails.clear();
        }
        return;
    }
    let ground = collider
        .as_deref()
        .and_then(|collider| collider.mesh.as_ref());
    let (Ok(camera), Some(ground)) = (cameras.single(), ground) else {
        return;
    };
    let dt = time.delta_secs().min(0.1);
    if dt <= 0.0 {
        return;
    }
    field.pending += dt;
    if field.pending < UPDATE_INTERVAL {
        return;
    }
    field.clock += field.pending;
    field.pending = 0.0;
    let eye = camera.translation();
    let mut nearby: Vec<_> = fighters
        .iter()
        .filter_map(|(id, transform, motion, health, conceal)| {
            let position = transform.translation();
            let distance = position.distance_squared(eye);
            (distance <= CONTACT_RANGE * CONTACT_RANGE
                && motion.facing.is_finite()
                && health.current > 0.0
                && conceal.remaining <= 0.0)
                .then_some((id, position, motion, distance))
        })
        .collect();
    nearby.sort_unstable_by(|a, b| {
        a.3.total_cmp(&b.3)
            .then_with(|| a.0.to_bits().cmp(&b.0.to_bits()))
    });
    nearby.truncate(MAX_ACTORS);
    field
        .trails
        .retain(|id, _| nearby.iter().any(|candidate| candidate.0 == *id));
    for (id, position, motion, _) in nearby {
        field
            .trails
            .entry(id)
            .or_insert_with(|| Trail::new(position, motion.movement_mode))
            .advance(position, motion.movement_mode, motion.facing, &mut contacts);
    }
    contacts.truncate(MAX_CONTACTS);
    let had_puffs = !field.puffs.is_empty();
    for contact in contacts.drain(..) {
        field.contact(contact, ground);
    }
    let clock = field.clock;
    let writes = field.field.flush(clock, &mut upload);
    let cell = (eye.xz() / 3.0).floor().as_ivec2();
    if writes > 0 || field.last_lod_cell != Some(cell) {
        field.last_lod_cell = Some(cell);
        field
            .lod
            .update(eye.xz(), |tile| field.field.disturbed(tile), &mut tiles);
    }
    field.puffs.retain(|puff| clock - puff.born < PUFF_LIFE);
    if had_puffs || !field.puffs.is_empty() {
        if let Some(mut mesh) = meshes.get_mut(handle) {
            *mesh = field.draw_powder(camera);
        }
    }
}

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn powder_texture() -> Image {
    const SIDE: u32 = 48;
    let mut data = Vec::with_capacity((4 * SIDE * SIDE) as usize);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let p = (Vec2::new(x as f32, y as f32) + Vec2::splat(0.5)) * (2.0 / SIDE as f32)
                - Vec2::ONE;
            let noise =
                0.82 + 0.18 * (p.x * 62.0 - p.y * 24.0).sin() * (p.y * 54.0 + p.x * 16.0).sin();
            let alpha = (1.0 - p.length_squared()).max(0.0).powf(2.1) * noise;
            data.extend([223, 238, 255, (255.0 * alpha).round() as u8]);
        }
    }
    terrain::field_image(UVec2::splat(SIDE), data, TextureFormat::Rgba8Unorm)
}
