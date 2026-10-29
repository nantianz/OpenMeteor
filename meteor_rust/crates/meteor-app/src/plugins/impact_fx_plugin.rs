use crate::app_state::{AppState, Settings};
use crate::plugins::camera_plugin::GameplayCamera;
use crate::plugins::network_plugin::NetState;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::material::AlphaMode;
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::mesh::Indices;
use bevy::render::render_resource::PrimitiveTopology;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, RenderPipelineDescriptor,
    SpecializedMeshPipelineError,
};
use bevy::shader::{Shader, ShaderRef};

const RECOVER_SECONDS: f64 = 0.070;
const OVERSHOOT_SECONDS: f64 = 0.070;
const OVERSHOOT: f32 = 0.1;
const MAX_FREEZE_SECONDS: f64 = 0.210;

const MAX_OFFSET: f32 = 0.22;
const PUNCH_RATIO: f32 = 0.55;
const DECAY: f32 = 6.0;
const FREQ_X: f32 = 27.0;
const FREQ_Y: f32 = 31.0;
const FREQ_Z: f32 = 19.0;
const MIN_TRAUMA: f32 = 0.002;
const SPARK_POOL_SIZE: usize = 2048;
const SPARK_GRAVITY: f32 = -8.0;
const SPARK_DRAG_TAU: f32 = 0.085;
const STREAK_LOOKAHEAD: f32 = 0.026;
const STREAK_GAIN: f32 = 52.0;
const STREAK_MAX: f32 = 8.0;
const GLOW_SCALE: f32 = 2.6;
const MAX_SPARK_PIXELS: f32 = 120.0;
const SPARK_SHADER_HANDLE: Handle<Shader> =
    bevy::asset::uuid_handle!("cb18e127-7295-4484-b0ec-4fb857aaff01");
const SPARK_BLEND: BlendState = BlendState {
    color: BlendComponent {
        src_factor: BlendFactor::SrcAlpha,
        dst_factor: BlendFactor::One,
        operation: BlendOperation::Add,
    },
    alpha: BlendComponent::OVER,
};

#[derive(Asset, AsBindGroup, TypePath, Clone)]
struct SparkMaterial {
    #[uniform(0)]
    gain: Vec4,
}

impl Material for SparkMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(SPARK_SHADER_HANDLE)
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &bevy::render::mesh::MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }

        if let Some(fragment) = descriptor.fragment.as_mut() {
            for target in fragment.targets.iter_mut().flatten() {
                target.blend = Some(SPARK_BLEND);
            }
        }
        Ok(())
    }
}

pub struct ImpactFxPlugin;

type ImpactFighterQuery<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static Transform),
    (
        With<crate::components::fighter::Fighter>,
        Without<GameplayCamera>,
    ),
>;

type ImpactCameraQuery<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static Camera, &'static Projection),
    (
        With<GameplayCamera>,
        Without<crate::components::fighter::Fighter>,
    ),
>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CombatImpactKind {
    Guard,
    Hit,
    Kill,
}

#[derive(Message, Debug, Clone, Copy)]
pub(crate) struct CombatImpactEvent {
    pub kind: CombatImpactKind,
    pub attacker: Entity,
    pub defender: Entity,

    pub reaction_pose: Option<u16>,

    pub effect_strategy: Option<u8>,
    pub contact: Vec3,
    pub direction: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum HitStopPhase {
    #[default]
    Idle,
    Hold,
    Recover,
    Overshoot,
}

#[derive(Component)]
struct SparkBurstMesh;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SparkKind {
    Streak,
    Flash,
}

#[derive(Debug, Clone, Copy)]
struct SparkParticle {
    origin: Vec3,
    launch: Vec3,
    color: Vec3,
    birth: f32,
    life: f32,
    size: f32,
    seed: f32,
    kind: SparkKind,
    ground: f32,
}

#[derive(Resource)]
struct SparkBurstRuntime {
    particles: Vec<Option<SparkParticle>>,
    cursor: usize,
    rng: u32,
    entity: Option<Entity>,
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<SparkMaterial>>,
}

impl Default for SparkBurstRuntime {
    fn default() -> Self {
        Self {
            particles: vec![None; SPARK_POOL_SIZE],
            cursor: 0,
            rng: 0x6d2b_79f5,
            entity: None,
            mesh: None,
            material: None,
        }
    }
}

impl SparkBurstRuntime {
    fn random(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((self.rng >> 8) as f32) / ((1_u32 << 24) as f32)
    }

    fn range(&mut self, low: f32, high: f32) -> f32 {
        low + self.random() * (high - low)
    }

    fn write(&mut self, particle: SparkParticle) {
        self.particles[self.cursor] = Some(particle);
        self.cursor = (self.cursor + 1) % SPARK_POOL_SIZE;
    }

    fn clear(&mut self) {
        self.particles.fill(None);
        self.cursor = 0;
    }
}

#[derive(Resource, Debug, Clone)]
pub(crate) struct ImpactSimulationScale {
    pub scale: f32,
    pub frozen: bool,
    pub ripple_impact: f32,
    pub wall_time: f32,
    phase: HitStopPhase,
    target_scale: f32,
    freeze_start: f64,
    hold_until: f64,
    recover_start: f64,
    overshoot_start: f64,
    trauma: f32,
    shake_time: f32,
    applied_offset: Vec3,
    left_at: Option<Vec3>,
}

impl Default for ImpactSimulationScale {
    fn default() -> Self {
        Self {
            scale: 1.0,
            frozen: false,
            ripple_impact: 0.0,
            wall_time: 0.0,
            phase: HitStopPhase::Idle,
            target_scale: 1.0,
            freeze_start: 0.0,
            hold_until: 0.0,
            recover_start: 0.0,
            overshoot_start: 0.0,
            trauma: 0.0,
            shake_time: 0.0,
            applied_offset: Vec3::ZERO,
            left_at: None,
        }
    }
}

impl Plugin for ImpactFxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SparkMaterial>::default());
        bevy::asset::load_internal_asset!(
            app,
            SPARK_SHADER_HANDLE,
            "impact_spark.wgsl",
            Shader::from_wgsl
        );
        app.init_resource::<ImpactSimulationScale>()
            .init_resource::<SparkBurstRuntime>()
            .add_message::<CombatImpactEvent>()
            .add_systems(
                OnEnter(AppState::Playing),
                setup_spark_pool.run_if(crate::plugins::starts_fresh_playing_session),
            )
            .add_systems(
                Update,
                update_impact_fx
                    .after(crate::plugins::FighterFrameSet::CameraFinalize)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                OnExit(AppState::Playing),
                (clear_impact_fx, cleanup_spark_pool).run_if(crate::plugins::ends_playing_session),
            );
    }
}

fn inactive_spark_mesh() -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0; 3]; 3])
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 3])
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0; 2]; 3])
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, vec![[0.0; 2]; 3])
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0; 4]; 3])
    .with_inserted_indices(Indices::U32(vec![0, 1, 2]))
}

fn setup_spark_pool(
    mut commands: Commands,
    mut runtime: ResMut<SparkBurstRuntime>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<SparkMaterial>>,
) {
    runtime.clear();
    let mesh = meshes.add(inactive_spark_mesh());
    let material = materials.add(SparkMaterial { gain: Vec4::ONE });
    let entity = commands
        .spawn((
            SparkBurstMesh,
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            Visibility::Hidden,
            Transform::IDENTITY,
            NoFrustumCulling,
            NotShadowCaster,
            NotShadowReceiver,
        ))
        .id();
    runtime.entity = Some(entity);
    runtime.mesh = Some(mesh);
    runtime.material = Some(material);
}

fn cleanup_spark_pool(
    mut commands: Commands,
    mut runtime: ResMut<SparkBurstRuntime>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<SparkMaterial>>,
) {
    if let Some(entity) = runtime.entity.take() {
        commands.entity(entity).despawn();
    }
    if let Some(mesh) = runtime.mesh.take() {
        meshes.remove(mesh.id());
    }
    if let Some(material) = runtime.material.take() {
        materials.remove(material.id());
    }
    runtime.clear();
}

fn hitstop_profile(kind: CombatImpactKind) -> (f32, f64) {
    match kind {
        CombatImpactKind::Guard => (0.05, 0.125),
        CombatImpactKind::Hit => (0.05, 0.075),
        CombatImpactKind::Kill => (0.05, 0.185),
    }
}

fn trauma_for(kind: CombatImpactKind) -> f32 {
    match kind {
        CombatImpactKind::Guard => 0.85,
        CombatImpactKind::Hit => 0.7,
        CombatImpactKind::Kill => 1.0,
    }
}

fn begin_hitstop(state: &mut ImpactSimulationScale, kind: CombatImpactKind, now: f64) {
    let (scale, hold) = hitstop_profile(kind);
    if state.phase == HitStopPhase::Idle {
        state.freeze_start = now;
        state.target_scale = scale;
    } else {
        state.target_scale = state.target_scale.min(scale);
    }
    state.scale = state.target_scale;
    state.hold_until = (now + hold).min(state.freeze_start + MAX_FREEZE_SECONDS);
    state.phase = HitStopPhase::Hold;
    state.frozen = true;
}

fn advance_hitstop(state: &mut ImpactSimulationScale, now: f64) {
    if state.phase == HitStopPhase::Idle {
        state.scale = 1.0;
        state.frozen = false;
        return;
    }
    if state.phase == HitStopPhase::Hold {
        if now < state.hold_until {
            state.scale = state.target_scale;
            state.frozen = true;
            return;
        }
        state.phase = HitStopPhase::Recover;
        state.recover_start = state.hold_until;
    }
    if state.phase == HitStopPhase::Recover {
        let t = ((now - state.recover_start) / RECOVER_SECONDS).clamp(0.0, 1.0) as f32;
        let eased = 1.0 - (1.0 - t).powi(3);
        state.scale = state.target_scale + (1.0 - state.target_scale) * eased;
        state.frozen = true;
        if t < 1.0 {
            return;
        }
        state.phase = HitStopPhase::Overshoot;
        state.overshoot_start = state.recover_start + RECOVER_SECONDS;
    }
    let t = ((now - state.overshoot_start) / OVERSHOOT_SECONDS).clamp(0.0, 1.0) as f32;
    state.scale = 1.0 + OVERSHOOT * (1.0 - t);
    state.frozen = true;
    if t >= 1.0 {
        state.scale = 1.0;
        state.frozen = false;
        state.phase = HitStopPhase::Idle;
    }
}

fn wobble(time: f32, frequency: f32, phase: f32) -> f32 {
    0.72 * (time * frequency + phase).sin() + 0.28 * (time * frequency * 1.7 + phase * 2.3).sin()
}

fn spark_motion(origin: Vec3, launch: Vec3, age: f32) -> (Vec3, Vec3) {
    let decay = (-age / SPARK_DRAG_TAU).exp();
    let travel = SPARK_DRAG_TAU * (1.0 - decay);
    (
        origin + launch * travel + Vec3::Y * (0.5 * SPARK_GRAVITY * age * age),
        launch * decay + Vec3::Y * (SPARK_GRAVITY * age),
    )
}

fn sample_cone(runtime: &mut SparkBurstRuntime, axis: Vec3, spread: f32) -> Vec3 {
    let axis = axis.normalize_or_zero();
    let axis = if axis.length_squared() < 1e-8 {
        Vec3::Y
    } else {
        axis
    };
    let mut side_a = Vec3::Y;
    if side_a.dot(axis).abs() > 0.9 {
        side_a = Vec3::X;
    }
    side_a = side_a.cross(axis).normalize_or_zero();
    let side_b = axis.cross(side_a).normalize_or_zero();
    let cos_max = spread.cos();
    let cos_theta = cos_max + runtime.random() * (1.0 - cos_max);
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    let phi = runtime.random() * std::f32::consts::TAU;
    axis * cos_theta + side_a * (phi.cos() * sin_theta) + side_b * (phi.sin() * sin_theta)
}

fn spawn_guard_sparks(
    runtime: &mut SparkBurstRuntime,
    impact: &CombatImpactEvent,
    now: f32,
    attacker: Option<Vec3>,
    defender: Option<Vec3>,
) {
    let fallback = Vec3::new(impact.direction.x, 0.0, impact.direction.z).normalize_or_zero();
    let toward_defender = defender
        .map(|position| {
            Vec3::new(
                position.x - impact.contact.x,
                0.22,
                position.z - impact.contact.z,
            )
        })
        .filter(|axis| axis.length_squared() > 1e-6)
        .unwrap_or_else(|| (fallback + Vec3::Y * 0.22).normalize_or_zero())
        .normalize_or_zero();
    let toward_attacker = attacker
        .map(|position| {
            Vec3::new(
                position.x - impact.contact.x,
                0.22,
                position.z - impact.contact.z,
            )
        })
        .filter(|axis| axis.length_squared() > 1e-6)
        .unwrap_or_else(|| (-fallback + Vec3::Y * 0.22).normalize_or_zero())
        .normalize_or_zero();
    let ground = defender
        .or(attacker)
        .map_or(impact.contact.y - 2.2, |position| position.y);
    for axis in [toward_attacker, toward_defender] {
        for _ in 0..36 {
            let launch = sample_cone(runtime, axis, 0.48) * runtime.range(58.0, 105.0) + axis * 8.0;
            let mix = runtime.random();
            let color_a = Vec3::new(1.0, 0.66, 0.26);
            let color_b = Vec3::new(1.0, 0.40, 0.10);
            let life = runtime.range(0.26, 0.56);
            let size = runtime.range(0.38, 0.78);
            let seed = runtime.random() * 100.0;
            runtime.write(SparkParticle {
                origin: impact.contact,
                launch,
                color: color_a * mix + color_b * (1.0 - mix),
                birth: now,
                life,
                size,
                seed,
                kind: SparkKind::Streak,
                ground,
            });
        }
    }
    for (color, size, life) in [
        (Vec3::new(1.0, 0.72, 0.36), 2.0, 0.06),
        (Vec3::new(1.0, 0.42, 0.12), 5.4, 0.22),
    ] {
        let seed = runtime.random() * 100.0;
        runtime.write(SparkParticle {
            origin: impact.contact,
            launch: Vec3::ZERO,
            color,
            birth: now,
            life,
            size,
            seed,
            kind: SparkKind::Flash,
            ground: -1e9,
        });
    }
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn cooling_color(color: Vec3, age: f32) -> Vec3 {
    let warm = color * Vec3::new(1.0, 0.34, 0.06);
    let dying = color * Vec3::new(0.62, 0.10, 0.012);
    if age < 0.35 {
        color.lerp(warm, age / 0.35)
    } else {
        warm.lerp(dying, (age - 0.35) / 0.65)
    }
}

#[derive(Default)]
struct SparkMeshData {
    positions: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    uvs: Vec<[f32; 2]>,
    kinds: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl SparkMeshData {
    fn push(&mut self, center: Vec3, axis: Vec3, perp: Vec3, color: Vec4, kind: SparkKind) {
        let first = self.positions.len() as u32;
        for [x, y] in [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]] {
            self.positions
                .push((center + axis * x + perp * y).to_array());
            self.colors.push(color.to_array());
            self.uvs.push([x, y]);
            self.kinds
                .push([if kind == SparkKind::Flash { 1.0 } else { 0.0 }, 0.0]);
        }
        self.indices
            .extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }
}

struct SparkView {
    view_from_world: Mat4,
    clip_from_view: Mat4,
    viewport: Vec2,
    right: Vec3,
    up: Vec3,
}

impl SparkView {
    fn axes(
        &self,
        spark: &SparkParticle,
        position: Vec3,
        velocity: Vec3,
        landed: bool,
    ) -> Option<(Vec3, Vec3)> {
        let view = self.view_from_world * position.extend(1.0);
        if view.z >= -0.001 {
            return None;
        }
        let clip = self.clip_from_view * view;
        let mut direction = Vec2::X;
        let mut stretch = 1.0;
        let streak = spark.kind == SparkKind::Streak;
        if streak && !landed {
            let ahead = self.clip_from_view
                * self.view_from_world
                * (position + velocity * STREAK_LOOKAHEAD).extend(1.0);
            if clip.w > 0.001 && ahead.w > 0.001 {
                let delta = (ahead.truncate().truncate() / ahead.w
                    - clip.truncate().truncate() / clip.w)
                    * Vec2::new(self.viewport.x / self.viewport.y, 1.0);
                let length = delta.length();
                if length > 1e-6 {
                    direction = delta / length;
                    stretch = (1.0 + length * STREAK_GAIN).clamp(1.0, STREAK_MAX);
                }
            }
        }
        let pixels = (spark.size * 140.0 / (-view.z).max(1.0)
            * if streak { GLOW_SCALE * stretch } else { 1.0 })
        .min(MAX_SPARK_PIXELS);
        let radius = pixels * clip.w / (self.viewport.y * self.clip_from_view.y_axis.y.abs());
        Some((
            (self.right * direction.x + self.up * direction.y) * radius,
            (-self.right * direction.y + self.up * direction.x) * (radius / stretch),
        ))
    }
}

fn rebuild_spark_mesh(
    runtime: &mut SparkBurstRuntime,
    now: f32,
    view: &SparkView,
    meshes: &mut Assets<Mesh>,
) -> bool {
    let mut data = SparkMeshData::default();
    for particle in &mut runtime.particles {
        let Some(spark) = *particle else {
            continue;
        };
        let age_seconds = now - spark.birth;
        let age = age_seconds / spark.life.max(1e-4);
        if !(0.0..1.0).contains(&age) {
            if age >= 1.0 {
                *particle = None;
            }
            continue;
        }
        let fade = 1.0 - age;
        let (mut position, velocity) = match spark.kind {
            SparkKind::Streak => spark_motion(spark.origin, spark.launch, age_seconds),
            SparkKind::Flash => (spark.origin, Vec3::ZERO),
        };
        let landed = spark.kind == SparkKind::Streak && position.y < spark.ground;
        if landed {
            position.y = spark.ground;
        }
        let cool_age = if landed { age.max(0.68) } else { age };
        let rgb = cooling_color(spark.color, cool_age) * (0.55 + 1.25 * fade);
        let mut alpha = match spark.kind {
            SparkKind::Flash => fade,
            SparkKind::Streak => {
                let crackle = 0.62
                    + 0.24 * (spark.seed * 53.0 + age * 96.0).sin()
                    + 0.14 * (spark.seed * 17.0 + age * 231.0).sin();
                crackle * smoothstep(0.0, 0.16, fade) * if landed { 0.55 } else { 1.0 }
            }
        };
        alpha = alpha.max(0.0);
        let Some((axis, perp)) = view.axes(&spark, position, velocity, landed) else {
            continue;
        };
        data.push(position, axis, perp, rgb.extend(alpha), spark.kind);
    }
    let visible = !data.indices.is_empty();

    if visible {
        if let Some(handle) = runtime.mesh.as_ref() {
            if let Some(mut mesh) = meshes.get_mut(handle) {
                let normals = vec![[0.0, 1.0, 0.0]; data.positions.len()];
                mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, data.positions);
                mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
                mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, data.uvs);
                mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, data.kinds);
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, data.colors);
                mesh.insert_indices(Indices::U32(data.indices));
            }
        }
    }
    visible
}

#[allow(clippy::too_many_arguments)]
fn update_impact_fx(
    settings: Res<Settings>,
    net: Res<NetState>,
    real: Res<Time<Real>>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut state: ResMut<ImpactSimulationScale>,
    mut sparks: ResMut<SparkBurstRuntime>,
    mut impacts: MessageReader<CombatImpactEvent>,
    fighters: ImpactFighterQuery<'_, '_>,
    mut cameras: ImpactCameraQuery<'_, '_>,
    mut spark_visibility: Query<&mut Visibility, With<SparkBurstMesh>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let wall_dt = frame_clock
        .as_deref()
        .map(|clock| clock.wall_dt)
        .unwrap_or_else(|| real.delta_secs().max(0.0));
    let now = real.elapsed_secs_f64();
    let mut camera = cameras.single_mut().ok();
    if let Some((transform, _, _)) = camera.as_mut() {
        remove_camera_shake(&mut state, transform);
    }
    if !settings.hit_effects {
        impacts.clear();
        state.scale = 1.0;
        state.frozen = false;
        state.phase = HitStopPhase::Idle;
        state.trauma = 0.0;
        state.ripple_impact = 0.0;
        sparks.clear();
        if let Some(entity) = sparks.entity {
            if let Ok(mut visibility) = spark_visibility.get_mut(entity) {
                *visibility = Visibility::Hidden;
            }
        }
        return;
    }

    let multiplayer_match = net.round_active;
    for impact in impacts.read() {
        state.ripple_impact = 1.0;
        state.trauma = state.trauma.max(trauma_for(impact.kind)).min(1.0);
        if !multiplayer_match && settings.hit_stop {
            begin_hitstop(&mut state, impact.kind, now);
        }
        if impact.kind == CombatImpactKind::Guard {
            let attacker = fighters
                .get(impact.attacker)
                .ok()
                .map(|(_, transform)| transform.translation);
            let defender = fighters
                .get(impact.defender)
                .ok()
                .map(|(_, transform)| transform.translation);
            spawn_guard_sparks(&mut sparks, impact, state.wall_time, attacker, defender);
        }
    }
    if multiplayer_match || !settings.hit_stop {
        state.scale = 1.0;
        state.frozen = false;
        state.phase = HitStopPhase::Idle;
    } else {
        advance_hitstop(&mut state, now);
    }

    state.wall_time += wall_dt;
    state.ripple_impact = (state.ripple_impact - wall_dt / 0.26).max(0.0);
    state.shake_time += wall_dt;
    let Some((mut camera, camera_info, projection)) = camera else {
        return;
    };
    if state.trauma > MIN_TRAUMA {
        state.trauma *= (-DECAY * wall_dt).exp();
        let amount = MAX_OFFSET * state.trauma * state.trauma;
        let right = camera.rotation * Vec3::X;
        let up = camera.rotation * Vec3::Y;
        let backward = camera.rotation * Vec3::Z;
        let offset = right * (wobble(state.shake_time, FREQ_X, 0.0) * amount)
            + up * (wobble(state.shake_time, FREQ_Y, 1.7) * amount)
            - backward * (wobble(state.shake_time, FREQ_Z, 3.1) * amount * PUNCH_RATIO);
        camera.translation += offset;
        state.applied_offset = offset;
        state.left_at = Some(camera.translation);
    } else {
        state.trauma = 0.0;
    }
    let view = SparkView {
        view_from_world: camera.to_matrix().inverse(),
        clip_from_view: projection.get_clip_from_view(),
        viewport: camera_info
            .physical_viewport_size()
            .unwrap_or(UVec2::new(1280, 720))
            .max(UVec2::ONE)
            .as_vec2(),
        right: camera.rotation * Vec3::X,
        up: camera.rotation * Vec3::Y,
    };
    let visible = rebuild_spark_mesh(&mut sparks, state.wall_time, &view, &mut meshes);
    if let Some(entity) = sparks.entity {
        if let Ok(mut visibility) = spark_visibility.get_mut(entity) {
            *visibility = if visible {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
        }
    }
}

fn remove_camera_shake(state: &mut ImpactSimulationScale, camera: &mut Transform) {
    if state
        .left_at
        .is_some_and(|left_at| camera.translation.distance_squared(left_at) < 1e-10)
    {
        camera.translation -= state.applied_offset;
    }
    state.applied_offset = Vec3::ZERO;
    state.left_at = None;
}

fn clear_impact_fx(
    mut state: ResMut<ImpactSimulationScale>,
    mut cameras: Query<&mut Transform, With<GameplayCamera>>,
) {
    if let Ok(mut camera) = cameras.single_mut() {
        remove_camera_shake(&mut state, &mut camera);
    }
    *state = ImpactSimulationScale::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spark_fragment_validates_with_the_quad_vertex_layout() {
        assert_eq!(SPARK_BLEND.color.src_factor, BlendFactor::SrcAlpha);
        assert_eq!(SPARK_BLEND.color.dst_factor, BlendFactor::One);
        assert_eq!(
            SparkMaterial { gain: Vec4::ONE }.alpha_mode(),
            AlphaMode::Blend
        );
        let source = include_str!("impact_spark.wgsl")
            .replace("#import bevy_pbr::forward_io::VertexOutput;", "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(2) uv: vec2<f32>, @location(3) uv_b: vec2<f32>, @location(5) color: vec4<f32>, };")
            .replace("#{MATERIAL_BIND_GROUP}", "3");
        let module = naga::front::wgsl::parse_str(&source).expect("spark shader parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("spark shader validates including fragment derivatives");
    }

    fn test_spark() -> SparkParticle {
        SparkParticle {
            origin: Vec3::ZERO,
            launch: Vec3::ZERO,
            color: Vec3::ONE,
            birth: 0.0,
            life: 1.0,
            size: 0.6,
            seed: 0.0,
            kind: SparkKind::Streak,
            ground: -10.0,
        }
    }

    #[test]
    fn projected_sparks_keep_web_pixel_size_and_constant_streak_width() {
        let spark = test_spark();
        for height in [480.0, 1080.0, 2160.0] {
            for fov in [0.7, 1.2] {
                let view = SparkView {
                    view_from_world: Mat4::IDENTITY,
                    clip_from_view: Mat4::perspective_infinite_reverse_rh(fov, 16.0 / 9.0, 0.1),
                    viewport: Vec2::new(height * 16.0 / 9.0, height),
                    right: Vec3::X,
                    up: Vec3::Y,
                };
                for depth in [10.0, 20.0] {
                    let at = Vec3::NEG_Z * depth;
                    let (round_x, round_y) = view.axes(&spark, at, Vec3::ZERO, false).unwrap();
                    let to_pixels = height * view.clip_from_view.y_axis.y / depth;
                    let expected = spark.size * 140.0 / depth * GLOW_SCALE;
                    assert!((round_x.length() * to_pixels - expected).abs() < 1e-4);
                    assert!(
                        (round_y.length() * to_pixels - expected).abs() < 1e-4,
                        "zero projected velocity must retain a non-degenerate ember"
                    );
                    let (axis, perp) = view.axes(&spark, at, Vec3::X * 30.0, false).unwrap();
                    assert!(axis.length() > perp.length());
                    assert!(axis.length() * to_pixels <= MAX_SPARK_PIXELS + 1e-4);
                    assert!(perp.length() <= round_y.length() + 1e-5);
                    assert!(axis.dot(perp).abs() < 1e-6);
                    let (landed_x, landed_y) = view.axes(&spark, at, Vec3::X * 30.0, true).unwrap();
                    assert!((landed_x.length() - landed_y.length()).abs() < 1e-6);
                }
                assert!(view.axes(&spark, Vec3::Z, Vec3::X, false).is_none());
            }
        }
    }

    fn impact_test_app() -> (App, Entity, Vec3) {
        let mut app = App::new();
        app.init_resource::<ImpactSimulationScale>()
            .init_resource::<SparkBurstRuntime>()
            .init_resource::<NetState>()
            .insert_resource(Settings {
                hit_effects: true,
                hit_stop: true,
                ..default()
            })
            .init_resource::<Time<Real>>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<SparkMaterial>>()
            .add_message::<CombatImpactEvent>()
            .add_systems(Startup, setup_spark_pool)
            .add_systems(Update, update_impact_fx);
        let at = Vec3::new(0.0, 3.0, 12.0);
        let camera = app
            .world_mut()
            .spawn((
                GameplayCamera,
                Camera3d::default(),
                Transform::from_translation(at),
            ))
            .id();
        (app, camera, at)
    }

    fn impact_step(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
        app.update();
    }

    fn emit_test_impact(app: &mut App, kind: CombatImpactKind) {
        let actor = app.world_mut().spawn_empty().id();
        app.world_mut().write_message(CombatImpactEvent {
            kind,
            attacker: actor,
            defender: actor,
            reaction_pose: None,
            effect_strategy: None,
            contact: Vec3::Y * 2.2,
            direction: Vec3::X,
        });
    }

    #[test]
    fn production_impact_switch_shakes_camera_and_builds_soft_quad_batch() {
        for online in [false, true] {
            for kind in [
                CombatImpactKind::Hit,
                CombatImpactKind::Guard,
                CombatImpactKind::Kill,
            ] {
                let (mut app, camera, base) = impact_test_app();
                app.world_mut().resource_mut::<NetState>().round_active = online;
                emit_test_impact(&mut app, kind);
                impact_step(&mut app);
                let eye = app.world().get::<Transform>(camera).unwrap().translation;
                assert!(
                    eye.distance(base) > 1e-5,
                    "{kind:?} must shake on the impact frame"
                );
                assert!(eye.distance(base) < MAX_OFFSET * 2.0);
                let state = app.world().resource::<ImpactSimulationScale>();
                assert_eq!(
                    state.scale == 1.0,
                    online,
                    "online shake must not slow simulation"
                );
                let sparks = app.world().resource::<SparkBurstRuntime>();
                if kind == CombatImpactKind::Guard {
                    let mesh = app
                        .world()
                        .resource::<Assets<Mesh>>()
                        .get(sparks.mesh.as_ref().unwrap())
                        .unwrap();
                    assert_eq!(mesh.count_vertices(), 74 * 4);
                    assert_eq!(mesh.indices().unwrap().len(), 74 * 6);
                    assert!(mesh.attribute(Mesh::ATTRIBUTE_UV_1).is_some());
                    assert!(app
                        .world()
                        .get::<MeshMaterial3d<SparkMaterial>>(sparks.entity.unwrap())
                        .is_some());
                } else {
                    assert!(sparks.particles.iter().all(Option::is_none));
                }
                app.world_mut().resource_mut::<Settings>().hit_effects = false;
                impact_step(&mut app);
                assert!(
                    app.world()
                        .get::<Transform>(camera)
                        .unwrap()
                        .translation
                        .distance(base)
                        < 1e-6,
                    "disable must remove the last shake even when the camera is held"
                );
                assert_eq!(app.world().resource::<ImpactSimulationScale>().scale, 1.0);
                let entity = app.world().resource::<SparkBurstRuntime>().entity.unwrap();
                assert_eq!(
                    *app.world().get::<Visibility>(entity).unwrap(),
                    Visibility::Hidden
                );
            }
        }
    }

    #[test]
    fn disabling_hitstop_restores_speed_and_preserves_visual_effects() {
        let (mut app, camera, base) = impact_test_app();
        emit_test_impact(&mut app, CombatImpactKind::Guard);
        impact_step(&mut app);
        assert!(app.world().resource::<ImpactSimulationScale>().scale < 1.0);
        app.world_mut().resource_mut::<Settings>().hit_stop = false;
        impact_step(&mut app);
        for kind in [
            CombatImpactKind::Guard,
            CombatImpactKind::Hit,
            CombatImpactKind::Kill,
        ] {
            emit_test_impact(&mut app, kind);
            impact_step(&mut app);
            let state = app.world().resource::<ImpactSimulationScale>();
            assert_eq!(state.scale, 1.0);
            assert!(!state.frozen);
            assert!(
                app.world()
                    .get::<Transform>(camera)
                    .unwrap()
                    .translation
                    .distance(base)
                    > 1e-5
            );
            assert!(app
                .world()
                .resource::<SparkBurstRuntime>()
                .particles
                .iter()
                .any(Option::is_some));
        }
    }

    #[test]
    fn held_camera_returns_to_base_without_accumulating_shake() {
        let (mut app, camera, base) = impact_test_app();
        emit_test_impact(&mut app, CombatImpactKind::Guard);
        for _ in 0..120 {
            impact_step(&mut app);
        }
        assert!(
            app.world()
                .get::<Transform>(camera)
                .unwrap()
                .translation
                .distance(base)
                < 1e-6
        );

        let mut state = ImpactSimulationScale {
            applied_offset: Vec3::X,
            left_at: Some(base + Vec3::X),
            ..default()
        };
        let mut transform = Transform::from_translation(base + Vec3::Y);
        remove_camera_shake(&mut state, &mut transform);
        assert_eq!(transform.translation, base + Vec3::Y);
    }

    #[test]
    fn hitstop_uses_enhanced_hold_recover_overshoot_timeline() {
        let mut state = ImpactSimulationScale::default();
        begin_hitstop(&mut state, CombatImpactKind::Hit, 1.0);
        advance_hitstop(&mut state, 1.02);
        assert_eq!(state.scale, 0.05);
        assert!(state.frozen);
        advance_hitstop(&mut state, 1.145);
        assert!((state.scale - 1.1).abs() < 1e-5);
        advance_hitstop(&mut state, 1.215);
        assert_eq!(state.scale, 1.0);
        assert!(!state.frozen);
    }

    #[test]
    fn chained_hit_deepens_and_caps_hold() {
        let mut state = ImpactSimulationScale::default();
        begin_hitstop(&mut state, CombatImpactKind::Hit, 2.0);
        begin_hitstop(&mut state, CombatImpactKind::Kill, 2.1);
        assert_eq!(state.target_scale, 0.05);
        assert_eq!(state.hold_until, 2.21);
    }

    #[test]
    fn trauma_profiles_preserve_stronger_guard_and_kill() {
        assert_eq!(trauma_for(CombatImpactKind::Guard), 0.85);
        assert_eq!(trauma_for(CombatImpactKind::Hit), 0.7);
        assert_eq!(trauma_for(CombatImpactKind::Kill), 1.0);
    }

    #[test]
    fn spark_motion_matches_web_analytic_drag_and_gravity() {
        let origin = Vec3::new(1.0, 2.0, 3.0);
        let launch = Vec3::new(10.0, 20.0, -5.0);
        let age = 0.13;
        let (position, velocity) = spark_motion(origin, launch, age);
        let decay = (-age / SPARK_DRAG_TAU).exp();
        let travel = SPARK_DRAG_TAU * (1.0 - decay);
        assert!((position.x - (origin.x + launch.x * travel)).abs() < 1e-6);
        assert!(
            (position.y - (origin.y + launch.y * travel + 0.5 * SPARK_GRAVITY * age * age)).abs()
                < 1e-6
        );
        assert!((velocity.y - (launch.y * decay + SPARK_GRAVITY * age)).abs() < 1e-6);
    }

    #[test]
    fn guard_shower_writes_seventy_two_streaks_and_two_flashes() {
        let mut world = World::new();
        let attacker = world
            .spawn((
                crate::components::fighter::Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::from_xyz(-1.0, 0.0, 0.0),
            ))
            .id();
        let defender = world
            .spawn((
                crate::components::fighter::Fighter {
                    character_id: 1,
                    player_slot: 1,
                    weapon: 8,
                    weapon_slots: [8, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::from_xyz(1.0, 0.0, 0.0),
            ))
            .id();
        let mut runtime = SparkBurstRuntime::default();
        spawn_guard_sparks(
            &mut runtime,
            &CombatImpactEvent {
                kind: CombatImpactKind::Guard,
                attacker,
                defender,
                reaction_pose: None,
                effect_strategy: None,
                contact: Vec3::Y * 2.2,
                direction: Vec3::X,
            },
            3.0,
            Some(Vec3::new(-1.0, 0.0, 0.0)),
            Some(Vec3::new(1.0, 0.0, 0.0)),
        );
        let particles: Vec<SparkParticle> = runtime.particles.iter().flatten().copied().collect();
        assert_eq!(particles.len(), 74);
        assert_eq!(
            particles
                .iter()
                .filter(|p| p.kind == SparkKind::Streak)
                .count(),
            72
        );
        assert_eq!(
            particles
                .iter()
                .filter(|p| p.kind == SparkKind::Flash)
                .count(),
            2
        );
        assert!(particles
            .iter()
            .filter(|p| p.kind == SparkKind::Streak)
            .all(|p| (0.26..=0.56).contains(&p.life) && (0.38..=0.78).contains(&p.size)));
    }

    #[test]
    fn spark_pool_wraps_without_growing() {
        let mut runtime = SparkBurstRuntime::default();
        let spark = SparkParticle {
            origin: Vec3::ZERO,
            launch: Vec3::Y,
            color: Vec3::ONE,
            birth: 0.0,
            life: 1.0,
            size: 1.0,
            seed: 0.0,
            kind: SparkKind::Streak,
            ground: 0.0,
        };
        for _ in 0..SPARK_POOL_SIZE + 7 {
            runtime.write(spark);
        }
        assert_eq!(runtime.particles.len(), SPARK_POOL_SIZE);
        assert_eq!(runtime.cursor, 7);
        assert_eq!(runtime.particles.iter().flatten().count(), SPARK_POOL_SIZE);
    }
}
