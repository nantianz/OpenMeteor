use super::config::{EffectEmitter, EmitterBlend, EmitterKind};
use crate::app_state::AppState;
use crate::plugins::camera_plugin::GameplayCamera;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::material::AlphaMode;
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, RenderPipelineDescriptor,
    ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::{Shader, ShaderDefVal, ShaderRef};

const FX_SHADER_HANDLE: Handle<Shader> =
    bevy::asset::uuid_handle!("5b11ad3e-71aa-4001-8c61-115e77aa0003");

const MAX_EMITTERS: usize = 96;

const MAX_LIGHTS: usize = 8;

#[derive(ShaderType, Clone, Default)]
pub(crate) struct FxParams {
    pub misc: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Clone)]
#[bind_group_data(FxMaterialKey)]
pub(crate) struct FxMaterial {
    #[uniform(0)]
    pub params: FxParams,
    #[texture(1)]
    #[sampler(2)]
    pub texture: Option<Handle<Image>>,
    pub additive: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FxMaterialKey {
    additive: bool,
}

impl From<&FxMaterial> for FxMaterialKey {
    fn from(material: &FxMaterial) -> Self {
        Self {
            additive: material.additive,
        }
    }
}

impl Material for FxMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(FX_SHADER_HANDLE)
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn enable_shadows() -> bool {
        false
    }

    fn enable_prepass() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if descriptor
            .vertex
            .shader_defs
            .contains(&ShaderDefVal::Bool("PREPASS_PIPELINE".into(), true))
        {
            return Ok(());
        }
        descriptor.primitive.cull_mode = None;
        if let Some(fragment) = descriptor.fragment.as_mut() {
            let blend = if key.bind_group_data.additive {
                let additive = BlendComponent {
                    src_factor: BlendFactor::One,
                    dst_factor: BlendFactor::One,
                    operation: BlendOperation::Add,
                };
                BlendState {
                    color: additive,
                    alpha: additive,
                }
            } else {
                BlendState::ALPHA_BLENDING
            };
            for target in fragment.targets.iter_mut().flatten() {
                target.blend = Some(blend);
            }
        }
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FxParticle {
    pub position: Vec3,
    pub velocity: Vec3,
    pub age: f32,
    pub life: f32,
    pub size: f32,

    pub seed: f32,
}

#[derive(Component)]
pub(crate) struct FxEmitter {
    pub spec: EffectEmitter,
    pub root: Entity,
    pub root_world: GlobalTransform,
    pub delay_left: f32,
    pub started: bool,
    pub age: f32,
    pub particles: Vec<FxParticle>,
    pub rng: u32,
    pub mesh: Handle<Mesh>,
    pub material: Handle<FxMaterial>,
}

#[derive(Component)]
pub(crate) struct FxLight {
    pub root: Entity,
    pub offset: Vec3,
    pub delay_left: f32,
    pub age: f32,
    pub duration: f32,
    pub peak: f32,
}

#[derive(Resource, Default)]
pub(crate) struct FxBudget {
    pub emitters: usize,
    pub lights: usize,
}

#[derive(Component)]
pub(crate) struct FxAnchor {
    pub ttl: f32,
}

pub(crate) fn rule_duration(emitters: &[EffectEmitter]) -> f32 {
    emitters
        .iter()
        .map(|e| {
            e.delay
                + match e.kind {
                    EmitterKind::Burst | EmitterKind::Light => e.life[1].max(e.life[0]),
                    EmitterKind::Flipbook => (e.grid[0] * e.grid[1]).max(1) as f32 / e.fps.max(1.0),
                }
        })
        .fold(0.0, f32::max)
}

pub(super) fn install(app: &mut App) {
    app.add_plugins(MaterialPlugin::<FxMaterial>::default());
    bevy::asset::load_internal_asset!(app, FX_SHADER_HANDLE, "fx_particle.wgsl", Shader::from_wgsl);

    app.add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>();
    app.init_resource::<FxBudget>()
        .add_systems(
            Update,
            combat_impact_fx
                .after(crate::plugins::FighterFrameSet::CameraFinalize)
                .run_if(crate::plugins::simulation_frame)
                .run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            PostUpdate,
            (tick_emitters, tick_lights, tick_anchors)
                .after(bevy::transform::TransformSystems::Propagate)
                .run_if(crate::plugins::simulation_frame)
                .run_if(in_state(AppState::Playing).or_else(in_state(AppState::Paused))),
        )
        .add_systems(
            OnExit(AppState::Playing),
            clear_all.run_if(crate::plugins::ends_playing_session),
        );
}

fn placeholder_mesh() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32, 1.0, 0.0]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32; 2]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, vec![[0.0f32; 2]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0f32; 4]; 3]);
    mesh.insert_indices(Indices::U32(vec![0, 1, 2]));
    mesh
}

pub(crate) fn spawn_emitters(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<FxMaterial>,
    budget: &mut FxBudget,
    root: Entity,
    root_world: GlobalTransform,
    emitters: &[EffectEmitter],
    textures: &[Option<Handle<Image>>],
    seed: u32,
) -> usize {
    let mut spawned = 0;
    for (index, spec) in emitters.iter().enumerate() {
        match spec.kind {
            EmitterKind::Light => {
                if budget.lights >= MAX_LIGHTS {
                    continue;
                }
                budget.lights += 1;
                let color = Vec3::from_array(spec.color);
                commands.spawn((
                    FxLight {
                        root,
                        offset: Vec3::from_array(spec.offset),
                        delay_left: spec.delay,
                        age: 0.0,
                        duration: spec.life[0].max(0.01),
                        peak: spec.intensity,
                    },
                    PointLight {
                        color: Color::linear_rgb(color.x, color.y, color.z),
                        intensity: 0.0,
                        range: spec.radius,
                        radius: 0.05,
                        shadow_maps_enabled: false,
                        ..default()
                    },
                    Transform::from_translation(
                        root_world.transform_point(Vec3::from_array(spec.offset)),
                    ),
                    Visibility::Hidden,
                ));
                spawned += 1;
            }
            EmitterKind::Burst | EmitterKind::Flipbook => {
                if budget.emitters >= MAX_EMITTERS {
                    continue;
                }
                budget.emitters += 1;
                let texture = textures.get(index).cloned().flatten();
                let mesh = meshes.add(placeholder_mesh());
                let material = materials.add(FxMaterial {
                    params: FxParams {
                        misc: Vec4::new(
                            f32::from(texture.is_some()),
                            f32::from(spec.blend == EmitterBlend::Add),
                            0.0,
                            0.0,
                        ),
                    },
                    texture,
                    additive: spec.blend == EmitterBlend::Add,
                });
                commands.spawn((
                    FxEmitter {
                        spec: spec.clone(),
                        root,
                        root_world,
                        delay_left: spec.delay,
                        started: false,
                        age: 0.0,
                        particles: Vec::new(),
                        rng: seed
                            .wrapping_mul(0x9e37_79b9)
                            .wrapping_add(index as u32 * 7919)
                            | 1,
                        mesh: mesh.clone(),
                        material: material.clone(),
                    },
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    NoFrustumCulling,
                    NotShadowCaster,
                    NotShadowReceiver,
                ));
                spawned += 1;
            }
        }
    }
    spawned
}

fn next_unit(rng: &mut u32) -> f32 {
    let mut x = *rng;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *rng = x;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

fn lerp_range(range: [f32; 2], t: f32) -> f32 {
    range[0] + (range[1] - range[0]) * t
}

pub(crate) fn cone_direction(rng: &mut u32, axis: Vec3, spread_degrees: f32) -> Vec3 {
    let axis = axis.normalize_or(Vec3::Y);
    let cos_max = spread_degrees
        .to_radians()
        .clamp(0.0, std::f32::consts::PI)
        .cos();
    let cos_theta = 1.0 - next_unit(rng) * (1.0 - cos_max);
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    let phi = next_unit(rng) * std::f32::consts::TAU;
    let local = Vec3::new(sin_theta * phi.cos(), cos_theta, sin_theta * phi.sin());
    Quat::from_rotation_arc(Vec3::Y, axis) * local
}

pub(crate) fn start_burst(emitter: &mut FxEmitter) {
    let spec = &emitter.spec;
    let origin = emitter
        .root_world
        .transform_point(Vec3::from_array(spec.offset));
    let axis = emitter
        .root_world
        .affine()
        .transform_vector3(Vec3::from_array(spec.direction));
    let count = spec.count.clamp(1, 512) as usize;
    emitter.particles.clear();
    emitter.particles.reserve(count);
    let mut rng = emitter.rng;
    for _ in 0..count {
        let direction = cone_direction(&mut rng, axis, spec.spread);
        let speed = lerp_range(spec.speed, next_unit(&mut rng));
        emitter.particles.push(FxParticle {
            position: origin,
            velocity: direction * speed,
            age: 0.0,
            life: lerp_range(spec.life, next_unit(&mut rng)).max(0.01),
            size: lerp_range(spec.size, next_unit(&mut rng)),
            seed: next_unit(&mut rng),
        });
    }
    emitter.rng = rng;
}

pub(crate) fn advance_particles(emitter: &mut FxEmitter, dt: f32) {
    let gravity = Vec3::Y * emitter.spec.gravity;
    let damping = (-emitter.spec.drag * dt).exp();
    for particle in &mut emitter.particles {
        particle.age += dt;
        particle.velocity = (particle.velocity + gravity * dt) * damping;
        particle.position += particle.velocity * dt;
    }
    emitter.particles.retain(|p| p.age < p.life);
}

pub(crate) fn envelope(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let attack = (t / 0.08).min(1.0);
    attack * (1.0 - t).powf(1.5)
}

pub(crate) struct SpriteBuffers {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub uvs_b: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl SpriteBuffers {
    fn with_capacity(quads: usize) -> Self {
        Self {
            positions: Vec::with_capacity(quads * 4),
            uvs: Vec::with_capacity(quads * 4),
            uvs_b: Vec::with_capacity(quads * 4),
            colors: Vec::with_capacity(quads * 4),
            indices: Vec::with_capacity(quads * 6),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_quad(
        &mut self,
        center: Vec3,
        right: Vec3,
        up: Vec3,
        uv_min: Vec2,
        uv_max: Vec2,
        soft: f32,
        color: [f32; 4],
    ) {
        let first = self.positions.len() as u32;
        self.positions.extend([
            (center - right - up).to_array(),
            (center + right - up).to_array(),
            (center + right + up).to_array(),
            (center - right + up).to_array(),
        ]);
        self.uvs.extend([
            [uv_min.x, uv_max.y],
            [uv_max.x, uv_max.y],
            [uv_max.x, uv_min.y],
            [uv_min.x, uv_min.y],
        ]);
        self.uvs_b.extend([[soft, 0.0]; 4]);
        self.colors.extend([color; 4]);
        self.indices
            .extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }

    fn write(&self, mesh: &mut Mesh) {
        let normals = vec![[0.0, 1.0, 0.0]; self.positions.len()];
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, self.uvs_b.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, self.colors.clone());
        mesh.insert_indices(Indices::U32(self.indices.clone()));
    }
}

fn particle_color(spec: &EffectEmitter, t: f32) -> [f32; 4] {
    let start = Vec3::from_array(spec.color);
    let rgb = match spec.end_color {
        Some(end) => start.lerp(Vec3::from_array(end), t.clamp(0.0, 1.0)),
        None => start,
    } * spec.intensity;
    [rgb.x, rgb.y, rgb.z, envelope(t)]
}

pub(crate) fn build_burst_sprites(emitter: &FxEmitter, camera: &GlobalTransform) -> SpriteBuffers {
    let spec = &emitter.spec;
    let cam_right = camera.right().as_vec3();
    let cam_up = camera.up().as_vec3();
    let soft = if spec.soft {
        spec.size[1].max(0.05) * 2.0
    } else {
        0.0
    };
    let mut out = SpriteBuffers::with_capacity(emitter.particles.len());
    for particle in &emitter.particles {
        let t = particle.age / particle.life;

        let half = particle.size * 0.5 * (1.0 - 0.35 * t);
        let (right, up) = if spec.stretch > 1.0 && particle.velocity.length_squared() > 1e-4 {
            let view_dir = camera.forward().as_vec3();
            let v = particle.velocity - view_dir * particle.velocity.dot(view_dir);
            let axis = v.normalize_or(cam_up);
            let perp = view_dir.cross(axis).normalize_or(cam_right);
            let stretch = 1.0
                + (spec.stretch - 1.0)
                    * (particle.velocity.length() / spec.speed[1].max(0.01)).min(1.0);
            (perp * half, axis * half * stretch)
        } else {
            (cam_right * half, cam_up * half)
        };
        out.push_quad(
            particle.position,
            right,
            up,
            Vec2::ZERO,
            Vec2::ONE,
            soft,
            particle_color(spec, t),
        );
    }
    out
}

pub(crate) fn build_flipbook_sprite(
    emitter: &FxEmitter,
    camera: &GlobalTransform,
) -> Option<SpriteBuffers> {
    let spec = &emitter.spec;
    let frames = (spec.grid[0] * spec.grid[1]).max(1);
    let duration = frames as f32 / spec.fps.max(1.0);
    if emitter.age >= duration {
        return None;
    }
    let frame = ((emitter.age / duration) * frames as f32).floor() as u32;
    let frame = frame.min(frames - 1);
    let col = frame % spec.grid[0];
    let row = frame / spec.grid[0];
    let cell = Vec2::new(1.0 / spec.grid[0] as f32, 1.0 / spec.grid[1] as f32);
    let uv_min = Vec2::new(col as f32, row as f32) * cell;
    let uv_max = uv_min + cell;
    let center = emitter
        .root_world
        .transform_point(Vec3::from_array(spec.offset));
    let half = spec.size[0] * 0.5;
    let soft = if spec.soft {
        spec.size[0].max(0.05) * 0.5
    } else {
        0.0
    };
    let mut out = SpriteBuffers::with_capacity(1);
    out.push_quad(
        center,
        camera.right().as_vec3() * half,
        camera.up().as_vec3() * half,
        uv_min,
        uv_max,
        soft,
        particle_color(spec, emitter.age / duration),
    );
    Some(out)
}

fn tick_emitters(
    mut commands: Commands,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    camera: Query<&GlobalTransform, With<GameplayCamera>>,
    roots: Query<&GlobalTransform>,
    mut emitters: Query<(Entity, &mut FxEmitter, &mut Visibility)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FxMaterial>>,
    mut budget: ResMut<FxBudget>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let Ok(camera) = camera.single() else {
        return;
    };
    for (entity, mut emitter, mut visibility) in &mut emitters {
        if let Ok(root) = roots.get(emitter.root) {
            if !emitter.started || emitter.spec.kind == EmitterKind::Flipbook {
                emitter.root_world = *root;
            }
        }
        if !emitter.started {
            emitter.delay_left -= dt;
            if emitter.delay_left > 0.0 {
                continue;
            }
            emitter.started = true;
            if emitter.spec.kind == EmitterKind::Burst {
                start_burst(&mut emitter);
            }
        } else {
            emitter.age += dt;
            if emitter.spec.kind == EmitterKind::Burst {
                advance_particles(&mut emitter, dt);
            }
        }
        let buffers = match emitter.spec.kind {
            EmitterKind::Burst => {
                (!emitter.particles.is_empty()).then(|| build_burst_sprites(&emitter, camera))
            }
            EmitterKind::Flipbook => build_flipbook_sprite(&emitter, camera),
            EmitterKind::Light => None,
        };
        match buffers {
            Some(buffers) if !buffers.indices.is_empty() => {
                if let Some(mut mesh) = meshes.get_mut(&emitter.mesh) {
                    buffers.write(&mut mesh);
                }
                visibility.set_if_neq(Visibility::Visible);
            }
            _ => {
                meshes.remove(emitter.mesh.id());
                materials.remove(emitter.material.id());
                budget.emitters = budget.emitters.saturating_sub(1);
                commands.entity(entity).despawn();
            }
        }
    }
}

fn tick_lights(
    mut commands: Commands,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    roots: Query<&GlobalTransform, Without<FxLight>>,
    mut lights: Query<(
        Entity,
        &mut FxLight,
        &mut PointLight,
        &mut Transform,
        &mut Visibility,
    )>,
    mut budget: ResMut<FxBudget>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    for (entity, mut light, mut point, mut transform, mut visibility) in &mut lights {
        if let Ok(root) = roots.get(light.root) {
            transform.translation = root.transform_point(light.offset);
        }
        if light.delay_left > 0.0 {
            light.delay_left -= dt;
            continue;
        }
        light.age += dt;
        let t = light.age / light.duration;
        if t >= 1.0 {
            budget.lights = budget.lights.saturating_sub(1);
            commands.entity(entity).despawn();
            continue;
        }
        point.intensity = light.peak * envelope(t);
        visibility.set_if_neq(Visibility::Visible);
    }
}

fn tick_anchors(
    mut commands: Commands,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut anchors: Query<(Entity, &mut FxAnchor)>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    for (entity, mut anchor) in &mut anchors {
        anchor.ttl -= dt;
        if anchor.ttl <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

fn combat_impact_fx(
    mut commands: Commands,
    mut events: MessageReader<crate::plugins::impact_fx_plugin::CombatImpactEvent>,
    settings: Option<Res<crate::app_state::Settings>>,
    enhancements: Option<Res<super::effects::EffectEnhancements>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FxMaterial>>,
    mut budget: ResMut<FxBudget>,
    mut seed: Local<u32>,
) {
    use crate::plugins::impact_fx_plugin::CombatImpactKind;
    let Some(enhancements) = enhancements.filter(|e| e.enabled) else {
        events.clear();
        return;
    };
    if settings
        .as_ref()
        .is_some_and(|settings| !settings.hit_effects)
    {
        events.clear();
        return;
    }
    for event in events.read() {
        let resolved = match event.kind {
            CombatImpactKind::Guard => enhancements.rule("@guard"),
            CombatImpactKind::Hit => enhancements.rule("@hit"),
            CombatImpactKind::Kill => enhancements
                .rule("@kill")
                .or_else(|| enhancements.rule("@hit")),
        };
        let Some(resolved) = resolved.filter(|r| !r.rule.emitters.is_empty()) else {
            continue;
        };
        let direction = event.direction.normalize_or(Vec3::NEG_Z);
        let up = if direction.dot(Vec3::Y).abs() > 0.98 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let transform = Transform::from_translation(event.contact).looking_to(direction, up);
        let root_world = GlobalTransform::from(transform);
        let root = commands
            .spawn((
                FxAnchor {
                    ttl: rule_duration(&resolved.rule.emitters) + 0.1,
                },
                transform,
            ))
            .id();
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        spawn_emitters(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut budget,
            root,
            root_world,
            &resolved.rule.emitters,
            &resolved.textures,
            *seed,
        );
    }
}

fn clear_all(
    mut commands: Commands,
    emitters: Query<Entity, With<FxEmitter>>,
    lights: Query<Entity, With<FxLight>>,
    anchors: Query<Entity, With<FxAnchor>>,
    mut budget: ResMut<FxBudget>,
) {
    for entity in emitters.iter().chain(lights.iter()).chain(anchors.iter()) {
        commands.entity(entity).despawn();
    }
    *budget = FxBudget::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emitter(spec: EffectEmitter) -> FxEmitter {
        FxEmitter {
            spec,
            root: Entity::PLACEHOLDER,
            root_world: GlobalTransform::from_translation(Vec3::new(1.0, 2.0, 3.0)),
            delay_left: 0.0,
            started: false,
            age: 0.0,
            particles: Vec::new(),
            rng: 12345,
            mesh: Handle::default(),
            material: Handle::default(),
        }
    }

    #[test]
    fn cone_directions_stay_inside_the_spread() {
        let mut rng = 7;
        for _ in 0..200 {
            let d = cone_direction(&mut rng, Vec3::X, 30.0);
            assert!((d.length() - 1.0).abs() < 1e-4);
            assert!(d.dot(Vec3::X) >= 30f32.to_radians().cos() - 1e-4, "{d}");
        }
        let d = cone_direction(&mut rng, Vec3::NEG_Y, 0.0);
        assert!((d - Vec3::NEG_Y).length() < 1e-4);
    }

    #[test]
    fn burst_spawns_count_particles_at_offset_and_dies_out() {
        let mut e = emitter(EffectEmitter {
            count: 10,
            offset: [0.0, 0.5, 0.0],
            life: [0.1, 0.2],
            speed: [1.0, 1.0],
            gravity: 0.0,
            drag: 0.0,
            ..default()
        });
        start_burst(&mut e);
        assert_eq!(e.particles.len(), 10);
        assert!(e
            .particles
            .iter()
            .all(|p| (p.position - Vec3::new(1.0, 2.5, 3.0)).length() < 1e-5));
        assert!(e
            .particles
            .iter()
            .all(|p| (p.velocity.length() - 1.0).abs() < 1e-4));
        advance_particles(&mut e, 0.05);
        assert_eq!(e.particles.len(), 10);
        assert!(e
            .particles
            .iter()
            .all(|p| (p.position - Vec3::new(1.0, 2.5, 3.0)).length() > 0.04));
        advance_particles(&mut e, 0.2);
        assert!(e.particles.is_empty());
    }

    #[test]
    fn burst_sprites_are_camera_facing_quads_with_hdr_colour() {
        let mut e = emitter(EffectEmitter {
            count: 3,
            color: [1.0, 0.5, 0.25],
            intensity: 4.0,
            size: [0.2, 0.2],
            stretch: 1.0,
            soft: true,
            ..default()
        });
        start_burst(&mut e);
        advance_particles(&mut e, 0.02);
        let camera = GlobalTransform::from(Transform::from_xyz(0.0, 0.0, 10.0));
        let buffers = build_burst_sprites(&e, &camera);
        assert_eq!(buffers.indices.len(), 18);
        assert_eq!(buffers.positions.len(), 12);
        let c = buffers.colors[0];
        assert!(
            (c[0] - 4.0).abs() < 1e-5 && (c[1] - 2.0).abs() < 1e-5 && (c[2] - 1.0).abs() < 1e-5
        );
        assert!(c[3] > 0.0 && c[3] <= 1.0);

        let width = Vec3::from_array(buffers.positions[1]) - Vec3::from_array(buffers.positions[0]);
        assert!(
            width.length() > 0.19 && width.length() <= 0.2,
            "{}",
            width.length()
        );
        assert!(width.normalize().dot(Vec3::X).abs() > 0.999);
        assert!(buffers.uvs_b.iter().all(|uv| uv[0] > 0.0));
    }

    #[test]
    fn flipbook_steps_through_the_atlas_and_ends() {
        let mut e = emitter(EffectEmitter {
            kind: EmitterKind::Flipbook,
            grid: [4, 2],
            fps: 8.0,
            size: [1.0, 1.0],
            soft: false,
            ..default()
        });
        let camera = GlobalTransform::IDENTITY;
        e.age = 0.0;
        let first = build_flipbook_sprite(&e, &camera).unwrap();
        assert_eq!(first.uvs[3], [0.0, 0.0]);
        assert_eq!(first.uvs[1], [0.25, 0.5]);
        e.age = 0.5;
        let later = build_flipbook_sprite(&e, &camera).unwrap();
        assert_eq!(later.uvs[3], [0.0, 0.5]);
        assert!(later.uvs_b.iter().all(|uv| uv[0] == 0.0));
        e.age = 1.0;
        assert!(build_flipbook_sprite(&e, &camera).is_none());
    }

    #[test]
    fn rule_duration_covers_delay_plus_longest_emitter() {
        let emitters = [
            EffectEmitter {
                delay: 0.2,
                life: [0.1, 0.5],
                ..default()
            },
            EffectEmitter {
                kind: EmitterKind::Flipbook,
                delay: 0.0,
                grid: [4, 4],
                fps: 10.0,
                ..default()
            },
            EffectEmitter {
                kind: EmitterKind::Light,
                delay: 0.0,
                life: [0.3, 0.3],
                ..default()
            },
        ];
        assert!((rule_duration(&emitters) - 1.6).abs() < 1e-5);
        assert_eq!(rule_duration(&[]), 0.0);
    }

    #[test]
    fn envelope_attacks_fast_and_fades_to_zero() {
        assert_eq!(envelope(0.0), 0.0);
        assert!(envelope(0.08) > 0.85);
        assert!(envelope(0.5) < envelope(0.1));
        assert_eq!(envelope(1.0), 0.0);
    }

    #[test]
    fn fx_shader_validates_with_and_without_depth_prepass() {
        for depth in [false, true] {
            let mut source = include_str!("fx_particle.wgsl")
                .replace(
                    "#import bevy_pbr::forward_io::VertexOutput",
                    "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(2) uv: vec2<f32>, @location(3) uv_b: vec2<f32>, @location(5) color: vec4<f32>, };",
                )
                .replace(
                    "#import bevy_pbr::view_transformations::depth_ndc_to_view_z",
                    "fn depth_ndc_to_view_z(d: f32) -> f32 { return -1.0 / max(d, 1e-6); }",
                )
                .replace("#{MATERIAL_BIND_GROUP}", "3");
            if depth {
                source = source
                    .replace(
                        "#import bevy_pbr::prepass_utils::prepass_depth",
                        "@group(0) @binding(30) var depth_prepass_texture: texture_depth_2d; fn prepass_depth(p: vec4<f32>, s: u32) -> f32 { return textureLoad(depth_prepass_texture, vec2<i32>(p.xy), 0); }",
                    )
                    .replace("#ifdef DEPTH_PREPASS", "")
                    .replace("#endif", "");
            } else {
                let mut out = String::new();
                let mut skipping = false;
                for line in source.lines() {
                    if line.starts_with("#ifdef DEPTH_PREPASS") {
                        skipping = true;
                        continue;
                    }
                    if line.starts_with("#endif") {
                        skipping = false;
                        continue;
                    }
                    if !skipping {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                source = out;
            }
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("fx shader parses (depth={depth}): {e}"));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("fx shader validates (depth={depth}): {e:?}"));
        }
    }
}
