use crate::app_state::{AppState, Settings};
use crate::components::fighter::Fighter;
use crate::plugins::camera_plugin::GameplayCamera;
use crate::plugins::render_plugin::SceneDeform;
use crate::plugins::stage_material::FrameUniforms;
use bevy::camera::primitives::Frustum;
use bevy::prelude::*;
use bevy::render::render_resource::ShaderType;
use meteor_core::constants::MODEL_SCALE;
use std::collections::HashMap;

pub(crate) const MAX_RIPPLES: usize = 8;
const LIFETIME: f32 = 2.2;
const MAX_EVENTS: usize = 24;
const WATER_ENTRY_HEIGHT: f32 = 0.025;
const WATER_EXIT_HEIGHT: f32 = 0.18;
const WATER_APPROACH_HEIGHT: f32 = 1.5;

#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub(crate) struct UnderwaterView {
    pub kind: f32,
    pub depth: f32,
}

#[derive(ShaderType, Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LiquidContacts {
    pub positions: [Vec4; MAX_RIPPLES],

    pub styles: [Vec4; MAX_RIPPLES],
    pub counts: Vec4,
    pub bounds: Vec4,
}

#[derive(Component)]
pub(crate) struct LiquidSurface {
    triangles: Vec<[Vec3; 3]>,
    indices: Vec<[usize; 3]>,
    min: Vec3,
    max: Vec3,
    kind: f32,
    depth_limit: f32,
}

impl LiquidSurface {
    pub(crate) fn is_clear_water(&self) -> bool {
        self.kind == 1.0
    }
    pub(super) fn near_camera(&self, eye: Vec3, max_distance: f32) -> bool {
        if self.triangles.is_empty() {
            return true;
        }
        let closest = eye.xz().clamp(self.min.xz(), self.max.xz());
        eye.xz().distance_squared(closest) <= max_distance * max_distance
    }
    pub(super) fn may_be_visible(
        &self,
        frustum: Option<&Frustum>,
        wave: Option<&super::waves::WaterWaveMesh>,
        deformed: bool,
    ) -> bool {
        let Some(frustum) = frustum else {
            return true;
        };
        if deformed || self.triangles.is_empty() {
            return true;
        }
        let Some(wave_height) = wave.map_or(Some(0.0), |wave| wave.max_displacement()) else {
            return true;
        };
        self.triangles.iter().any(|triangle| {
            frustum.0.half_spaces.iter().all(|plane| {
                let margin = 0.01 + plane.normal().y.abs() * wave_height;
                !triangle
                    .iter()
                    .all(|vertex| plane.normal_d().dot(vertex.extend(1.0)) < -margin)
            })
        })
    }

    #[cfg(test)]
    pub(crate) fn test_pool(height: f32) -> Self {
        let a = Vec3::new(-20.0, height, -20.0);
        let b = Vec3::new(20.0, height, -20.0);
        let c = Vec3::new(20.0, height, 20.0);
        let d = Vec3::new(-20.0, height, 20.0);
        Self {
            triangles: vec![[a, b, c], [a, c, d]],
            indices: vec![],
            min: a,
            max: c,
            kind: 1.0,
            depth_limit: 10.0,
        }
    }

    pub(super) fn top_height(&self) -> f32 {
        self.max.y
    }

    #[cfg(test)]
    pub(super) fn test_surface(kind: f32) -> Self {
        Self {
            triangles: Vec::new(),
            indices: Vec::new(),
            min: Vec3::ZERO,
            max: Vec3::ZERO,
            kind,
            depth_limit: 1.0,
        }
    }

    pub(crate) fn from_geometry(
        object: &meteor_asset::gmdl::GmdlObject,
        indices: &[u32],
        center: [f32; 3],
        kind: f32,
        depth_limit: f32,
    ) -> Self {
        let triangles: Vec<_> = indices
            .chunks_exact(3)
            .map(|f| {
                std::array::from_fn(|i| {
                    let p = &object.positions[f[i] as usize * 3..];
                    Vec3::new(p[0] - center[0], p[2] - center[2], -(p[1] - center[1])) * MODEL_SCALE
                })
            })
            .collect();
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for v in triangles.iter().flatten() {
            min = min.min(*v);
            max = max.max(*v);
        }
        Self {
            indices: indices
                .chunks_exact(3)
                .map(|f| [f[0] as usize, f[1] as usize, f[2] as usize])
                .collect(),
            triangles,
            min,
            max,
            kind,
            depth_limit,
        }
    }

    pub(super) fn contains_surface_point(&self, point: Vec3) -> bool {
        self.triangles
            .iter()
            .any(|t| Self::height_on_triangle(point, *t).is_some())
    }

    fn contact(&self, feet: Vec3) -> Option<Vec3> {
        if feet.x < self.min.x
            || feet.x > self.max.x
            || feet.z < self.min.z
            || feet.z > self.max.z
            || feet.y > self.max.y + self.contact_margin()
            || feet.y < self.min.y - 4.2
        {
            return None;
        }
        self.triangles
            .iter()
            .find_map(|t| self.on_triangle(feet, *t))
    }

    fn height_on_triangle(point: Vec3, [a, b, c]: [Vec3; 3]) -> Option<f32> {
        let u = (b - a).xz();
        let v = (c - a).xz();
        let p = (point - a).xz();
        let det = u.perp_dot(v);
        if det.abs() < 1e-7 {
            return None;
        }
        let s = p.perp_dot(v) / det;
        let t = u.perp_dot(p) / det;
        if s < -0.0001 || t < -0.0001 || s + t > 1.0001 {
            return None;
        }
        Some(a.y + s * (b.y - a.y) + t * (c.y - a.y))
    }

    fn contact_margin(&self) -> f32 {
        if self.is_clear_water() {
            WATER_APPROACH_HEIGHT
        } else {
            0.15
        }
    }

    fn on_triangle(&self, feet: Vec3, triangle: [Vec3; 3]) -> Option<Vec3> {
        let y = Self::height_on_triangle(feet, triangle)?;
        (feet.y <= y + self.contact_margin() && feet.y >= y - 4.2)
            .then_some(Vec3::new(feet.x, y, feet.z))
    }

    fn submersion(&self, eye: Vec3, current_mesh: Option<(&Mesh, &Transform)>) -> Option<f32> {
        if self.kind > 2.5
            || eye.x < self.min.x
            || eye.x > self.max.x
            || eye.z < self.min.z
            || eye.z > self.max.z
            || eye.y >= self.max.y + 1.2
            || eye.y < self.min.y - self.depth_limit - 1.2
        {
            return None;
        }
        let height = if let Some((mesh, transform)) = current_mesh {
            let bevy::mesh::VertexAttributeValues::Float32x3(positions) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
            else {
                return None;
            };
            self.indices.iter().find_map(|f| {
                Self::height_on_triangle(
                    eye,
                    std::array::from_fn(|i| {
                        transform.transform_point(Vec3::from_array(positions[f[i]]))
                    }),
                )
            })
        } else {
            self.triangles
                .iter()
                .find_map(|t| Self::height_on_triangle(eye, *t))
        }?;
        let depth = height - eye.y;
        (depth > 0.05 && depth < self.depth_limit).then_some(depth)
    }

    fn contact_mesh(&self, feet: Vec3, mesh: &Mesh, transform: &Transform) -> Option<Vec3> {
        let bevy::mesh::VertexAttributeValues::Float32x3(positions) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
        else {
            return None;
        };
        self.indices.iter().find_map(|f| {
            self.on_triangle(
                feet,
                std::array::from_fn(|i| {
                    transform.transform_point(Vec3::from_array(positions[f[i]]))
                }),
            )
        })
    }
}

#[derive(Clone, Copy)]
struct Contact {
    surface: Entity,
    center: Vec3,
    kind: f32,
}
impl Contact {
    fn touches(self, feet: Vec3, previous_surface: Option<Entity>) -> bool {
        let margin = if previous_surface == Some(self.surface) {
            WATER_EXIT_HEIGHT
        } else {
            WATER_ENTRY_HEIGHT
        };
        self.kind != 1.0 || feet.y - self.center.y <= margin
    }
}
#[derive(Clone, Copy)]
struct Sample {
    entity: Entity,
    feet: Vec3,
    contact: Option<Contact>,
}
struct History {
    feet: Vec3,
    surface: Option<Entity>,
    candidate: Option<Contact>,
    cooldown: f32,
    distance: f32,
    velocity: Vec3,
}
struct Ripple {
    center: Vec3,
    age: f32,
    strength: f32,
    kind: f32,
    spray: Option<super::spray::Spray>,
    surface: Option<Entity>,
}

pub(super) struct SurfaceImpulse {
    pub surface: Entity,
    pub point: Vec3,
    pub strength: f32,
    pub radius: f32,
    pub direction: Vec2,
}

#[derive(Resource, Default)]
pub(crate) struct LiquidInteractions {
    history: HashMap<Entity, History>,
    ripples: Vec<Ripple>,
    surfaces: Vec<Entity>,
    pub(super) impulses: Vec<SurfaceImpulse>,
    spray_sequence: u32,
}
impl LiquidInteractions {
    #[cfg(test)]
    pub(super) fn emit(&mut self, center: Vec3, kind: f32, strength: f32) {
        self.emit_with_splash(center, kind, strength, 0.0);
    }

    #[cfg(test)]
    pub(super) fn emit_with_splash(&mut self, center: Vec3, kind: f32, strength: f32, splash: f32) {
        self.emit_with_direction(center, kind, strength, splash, Vec2::ZERO);
    }

    fn emit_with_direction(
        &mut self,
        center: Vec3,
        kind: f32,
        strength: f32,
        splash: f32,
        direction: Vec2,
    ) {
        if self.ripples.len() == MAX_EVENTS {
            self.ripples.remove(0);
        }
        self.ripples.push(Ripple {
            center,
            kind,
            strength,
            age: 0.0,
            spray: (kind == 1.0 && splash > 0.0).then(|| {
                super::spray::Spray::new(
                    center,
                    super::spray::Impact {
                        velocity: Vec3::new(
                            direction.x * 20.0,
                            -30.0 * splash.sqrt(),
                            direction.y * 20.0,
                        ),
                        radius: 0.4,
                        depth: 0.3,
                    },
                    1.0,
                    71,
                )
            }),
            surface: None,
        });
    }

    #[cfg(test)]
    pub(super) fn set_latest_age(&mut self, age: f32) {
        self.ripples.last_mut().expect("test splash exists").age = age;
    }

    pub(super) fn splashes(&self) -> impl Iterator<Item = (&super::spray::Spray, f32)> + '_ {
        self.ripples.iter().filter_map(|r| {
            r.spray
                .as_ref()
                .filter(|s| s.particles.iter().any(|p| r.age < p.landing))
                .map(|s| (s, r.age))
        })
    }
    fn step(&mut self, samples: &[Sample], dt: f32) {
        let config = &super::config::get().interaction;
        for ripple in &mut self.ripples {
            if let (Some(surface), Some(spray)) = (ripple.surface, &ripple.spray) {
                for (point, strength) in spray.returns(ripple.age, ripple.age + dt) {
                    if self.impulses.len() < 96 {
                        self.impulses.push(SurfaceImpulse {
                            surface,
                            point,
                            strength,
                            radius: 0.3,
                            direction: Vec2::ZERO,
                        });
                    }
                }
            }
            ripple.age += dt;
        }
        self.ripples.retain(|r| r.age < LIFETIME);
        self.history
            .retain(|id, _| samples.iter().any(|s| s.entity == *id));
        for s in samples {
            let Some(h) = self.history.get_mut(&s.entity) else {
                self.history.insert(
                    s.entity,
                    History {
                        feet: s.feet,
                        surface: s
                            .contact
                            .filter(|c| c.touches(s.feet, None))
                            .map(|c| c.surface),
                        candidate: s.contact,
                        cooldown: 0.0,
                        distance: 0.0,
                        velocity: Vec3::ZERO,
                    },
                );
                continue;
            };
            let delta = s.feet - h.feet;
            h.cooldown = (h.cooldown - dt).max(0.0);
            let mut event = None;
            let continuous = delta.length_squared() < 64.0 && dt > 0.0 && dt <= 0.25;
            let measured = if continuous { delta / dt } else { Vec3::ZERO };
            let contact = s.contact.filter(|c| c.touches(s.feet, h.surface));
            let current = contact.map(|c| c.surface);
            if continuous {
                if let Some(mut contact) = contact {
                    let entered = h.surface != current;
                    h.distance += delta.xz().length();
                    if h.cooldown <= 0.0
                        && ((entered && measured.length_squared() > 0.0025)
                            || h.distance >= config.step_distance)
                    {
                        let velocity = if entered {
                            measured.with_y(measured.y.min(h.velocity.y).min(0.0))
                        } else {
                            measured.with_y(0.0)
                        };
                        let immersion = (contact.center.y - s.feet.y).max(0.0);
                        if entered {
                            if let Some(previous) =
                                h.candidate.filter(|p| p.surface == contact.surface)
                            {
                                let old_gap = h.feet.y - previous.center.y;
                                let new_gap = s.feet.y - contact.center.y;
                                if old_gap > 0.0 && new_gap < 0.0 {
                                    contact.center = previous
                                        .center
                                        .lerp(contact.center, old_gap / (old_gap - new_gap));
                                }
                            }
                        }
                        let impact = super::spray::Impact {
                            velocity,
                            radius: config.foot_radius * if entered { 0.65 } else { 0.35 },
                            depth: if entered && (contact.kind != 1.0 || velocity.y < -1.0) {
                                0.45
                            } else {
                                immersion.clamp(0.03, 0.25)
                            },
                        };
                        event = Some((contact, impact));
                        h.distance = 0.0;
                        h.cooldown = if entered {
                            config.entry_interval
                        } else {
                            config.step_interval
                        };
                    }
                } else {
                    h.distance = 0.0;
                }
            } else {
                h.distance = 0.0;
            }
            h.feet = s.feet;
            h.surface = current;
            h.candidate = s.contact;
            h.velocity = measured;
            if let Some((c, impact)) = event {
                self.disturb(c.surface, c.center, c.kind, impact);
            }
        }
    }
    pub(super) fn disturb(
        &mut self,
        surface: Entity,
        center: Vec3,
        kind: f32,
        mut impact: super::spray::Impact,
    ) {
        if !impact.velocity.is_finite() {
            return;
        }

        impact.velocity = impact.velocity.clamp_length_max(80.0);
        let config = &super::config::get().interaction;
        let strength = impact.wave_strength() * config.strength_scale;
        if strength <= 0.00001 {
            return;
        }
        self.spray_sequence = self.spray_sequence.wrapping_add(1);
        let spray = if kind == 1.0 {
            let s =
                super::spray::Spray::new(center, impact, config.splash_scale, self.spray_sequence);
            (!s.particles.is_empty()).then_some(s)
        } else {
            None
        };
        if self.ripples.len() == MAX_EVENTS {
            self.ripples.remove(0);
        }
        self.ripples.push(Ripple {
            center,
            kind,
            strength,
            age: 0.0,
            spray,
            surface: Some(surface),
        });
        if kind == 1.0 && self.impulses.len() < 128 {
            self.impulses.push(SurfaceImpulse {
                surface,
                point: center,
                strength,
                radius: (impact.radius * 2.0).max(0.35),
                direction: impact.velocity.xz().normalize_or_zero(),
            });
        }
    }

    fn uniforms(&self) -> LiquidContacts {
        let mut result = LiquidContacts::default();
        let mut lo = Vec2::splat(f32::INFINITY);
        let mut hi = Vec2::splat(f32::NEG_INFINITY);
        for (i, r) in self.ripples.iter().rev().take(MAX_RIPPLES).enumerate() {
            result.positions[i] = r.center.extend(r.age);
            result.styles[i] = Vec4::new(r.strength, r.kind, 0.0, 0.0);
            let speed = if r.kind > 2.5 {
                0.75
            } else if r.kind > 1.5 {
                1.35
            } else {
                2.4
            };
            let extent = Vec2::splat(0.8 + r.age * speed);
            lo = lo.min(r.center.xz() - extent);
            hi = hi.max(r.center.xz() + extent);
        }
        result.counts.x = self.ripples.len().min(MAX_RIPPLES) as f32;
        if !self.ripples.is_empty() {
            result.bounds = Vec4::new(lo.x, lo.y, hi.x, hi.y);
        }
        result
    }
}

pub(crate) fn install(app: &mut App) {
    super::splash::install(app);
    super::waves::install(app);
    app.init_resource::<super::combat::WaterCombat>()
        .init_resource::<LiquidInteractions>()
        .init_resource::<UnderwaterView>()
        .add_systems(
            Update,
            update
                .after(crate::plugins::FighterFrameSet::CameraFinalize)
                .after(crate::plugins::render_plugin::update_scene_deform)
                .after(super::waves::update),
        );
}

pub(super) fn update(
    state: Res<State<AppState>>,
    settings: Res<Settings>,
    time: Res<Time>,
    surfaces: Query<(
        Entity,
        &LiquidSurface,
        Option<&Mesh3d>,
        Option<&Transform>,
        Option<&SceneDeform>,
        Option<&super::waves::WaterWaveMesh>,
    )>,
    meshes: Option<Res<Assets<Mesh>>>,
    fighters: Query<(Entity, &Transform), With<Fighter>>,
    cameras: Query<&Transform, With<GameplayCamera>>,
    mut combat: Option<ResMut<super::combat::WaterCombat>>,
    mut interactions: ResMut<LiquidInteractions>,
    mut underwater: ResMut<UnderwaterView>,
    mut frame: ResMut<FrameUniforms>,
) {
    if !settings.water_enhance || !matches!(state.get(), AppState::Playing | AppState::Paused) {
        if let Some(combat) = combat.as_deref_mut() {
            *combat = default();
        }
        *interactions = LiquidInteractions::default();
        *underwater = UnderwaterView::default();
        frame.liquid_contacts = LiquidContacts::default();
        return;
    }
    let eye = cameras
        .iter()
        .next()
        .map_or(frame.camera.eye.truncate(), |c| c.translation);
    *underwater = surfaces
        .iter()
        .filter_map(|(_, surface, mesh, transform, deform, wave)| {
            let current_mesh = if deform.is_some() {
                Some((meshes.as_ref()?.get(&mesh?.0)?, transform?))
            } else {
                None
            };
            let surface_eye = eye
                - Vec3::Y
                    * wave.map_or(0.0, |wave| {
                        wave.world_height_at(eye.xz(), frame.seconds, transform)
                    });
            surface
                .submersion(surface_eye, current_mesh)
                .map(|depth| UnderwaterView {
                    kind: surface.kind,
                    depth,
                })
        })
        .min_by(|a, b| a.depth.total_cmp(&b.depth))
        .unwrap_or_default();
    if *state.get() == AppState::Paused {
        return;
    }
    let mut ids: Vec<_> = surfaces.iter().map(|(id, ..)| id).collect();
    ids.sort();
    if interactions.surfaces != ids {
        *interactions = LiquidInteractions {
            surfaces: ids,
            ..default()
        };
    }
    let mut near: Vec<_> = fighters
        .iter()
        .filter_map(|(id, t)| {
            let d = t.translation.distance_squared(eye);
            (d < super::config::get().interaction.max_distance.powi(2)).then_some((
                d,
                id,
                t.translation,
            ))
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    near.truncate(super::config::get().interaction.max_actors);
    let samples: Vec<_> = near
        .iter()
        .map(|&(_, entity, feet)| Sample {
            entity,
            feet,
            contact: surfaces
                .iter()
                .find_map(|(id, s, mesh, transform, deform, wave)| {
                    let height = wave.map_or(0.0, |wave| {
                        wave.world_height_at(feet.xz(), frame.seconds, transform)
                    });
                    let feet_on_surface = feet - Vec3::Y * height;
                    let point = if deform.is_some() {
                        s.contact_mesh(feet_on_surface, meshes.as_ref()?.get(&mesh?.0)?, transform?)
                    } else {
                        s.contact(feet_on_surface)
                    };
                    point.map(|center| Contact {
                        surface: id,
                        center: center + Vec3::Y * height,
                        kind: s.kind,
                    })
                }),
        })
        .collect();
    interactions.step(&samples, time.delta_secs());
    if let Some(combat) = combat.as_deref_mut() {
        let planes: Vec<_> = surfaces.iter().map(|(id, s, ..)| (id, s)).collect();
        for strike in combat.drain(time.elapsed_secs_f64(), &planes, eye) {
            interactions.disturb(strike.surface, strike.point, 1.0, strike.impact);
        }
    }
    frame.liquid_contacts = interactions.uniforms();
}

#[cfg(test)]
mod tests;
