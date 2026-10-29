use crate::components::fighter::{
    EntryBlend, EscortFlagCarrier, Fighter, PoseEffectSpawn, PoseRuntime,
};
use crate::plugins::audio_plugin::{resource_roots, SfxCache};
use crate::plugins::character_plugin::{CharacterAssetsSet, FighterModel};
use crate::plugins::stage_material::{GmdlMaterial, GmdlMaterialParams};
use bevy::asset::RenderAssetUsages;
use bevy::audio::AudioSource;
use bevy::material::AlphaMode;
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, Face,
    RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::{Shader, ShaderRef};

#[derive(Component, Debug, Clone)]
pub struct HitSpark {
    pub velocity: Vec3,

    pub life: f32,

    pub max_life: f32,
}

#[derive(Message, Debug, Clone)]
pub struct SceneEffectEvent {
    pub effect: String,
    pub position: Vec3,
}

#[derive(Component, Debug, Clone)]
pub(crate) struct PickupEffectRequest(pub SceneEffectEvent);

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct SceneEffectAnchor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PonEfBlendMode {
    Opaque,
    SrcColorOne,
    ZeroInvSrcColor,
    SrcAlphaInvSrcAlpha,
    SrcAlphaOne,
}

impl PonEfBlendMode {
    fn from_state(state: meteor_asset::pon_ef::PonEfRenderState) -> Self {
        if !state.blend_enabled {
            return Self::Opaque;
        }
        match (state.blend_src, state.blend_dst) {
            (2, 1) => Self::SrcColorOne,
            (0, 4) => Self::ZeroInvSrcColor,
            (6, 7) => Self::SrcAlphaInvSrcAlpha,
            (6, 1) => Self::SrcAlphaOne,
            _ => Self::SrcAlphaInvSrcAlpha,
        }
    }

    fn factors(self) -> Option<(BlendFactor, BlendFactor)> {
        match self {
            Self::Opaque => None,
            Self::SrcColorOne => Some((BlendFactor::Src, BlendFactor::One)),
            Self::ZeroInvSrcColor => Some((BlendFactor::Zero, BlendFactor::OneMinusSrc)),
            Self::SrcAlphaInvSrcAlpha => {
                Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha))
            }
            Self::SrcAlphaOne => Some((BlendFactor::SrcAlpha, BlendFactor::One)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PonEfMaterialKey {
    instanced: bool,
    blend: PonEfBlendMode,
    two_sided: bool,
}

#[path = "effect_plugin/dynamic_uniforms.rs"]
mod dynamic_uniforms;

#[derive(ShaderType, Clone, PartialEq)]
struct PonEfMaterialParams {
    color: Vec4,
    has_texture: u32,
}

#[derive(Asset, TypePath, Clone)]
pub struct PonEfMaterial {
    params: PonEfMaterialParams,
    texture: Option<Handle<Image>>,
    blend: PonEfBlendMode,
    two_sided: bool,

    uniform_id: u64,

    particle_colors: Option<Box<[Vec4; dynamic_uniforms::PARAMS_PER_PAGE]>>,
}

impl From<&PonEfMaterial> for PonEfMaterialKey {
    fn from(material: &PonEfMaterial) -> Self {
        Self {
            instanced: material.particle_colors.is_some(),
            blend: material.blend,
            two_sided: material.two_sided,
        }
    }
}

const PON_EF_SHADER_HANDLE: Handle<Shader> =
    bevy::asset::uuid_handle!("00000000-0000-0000-9d2a-1b3c4d5e6f70");

const PON_EF_PARTICLE_SHADER_HANDLE: Handle<Shader> =
    bevy::asset::uuid_handle!("e9bfb68a-76ac-4ba6-8395-8a7c630852ba");

impl Material for PonEfMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(PON_EF_SHADER_HANDLE)
    }

    fn alpha_mode(&self) -> AlphaMode {
        if self.blend == PonEfBlendMode::Opaque {
            AlphaMode::Opaque
        } else {
            AlphaMode::Blend
        }
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &bevy::render::mesh::MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if key.bind_group_data.instanced {
            descriptor.vertex.shader = PON_EF_PARTICLE_SHADER_HANDLE;
            if let Some(fragment) = &mut descriptor.fragment {
                fragment.shader = PON_EF_PARTICLE_SHADER_HANDLE;
            }
        }
        descriptor.primitive.cull_mode = if key.bind_group_data.two_sided {
            None
        } else {
            Some(Face::Back)
        };
        if let Some(depth_stencil) = &mut descriptor.depth_stencil {
            depth_stencil.depth_write_enabled =
                Some(key.bind_group_data.blend == PonEfBlendMode::Opaque);
        }
        if let Some((src_factor, dst_factor)) = key.bind_group_data.blend.factors() {
            if let Some(fragment) = &mut descriptor.fragment {
                if let Some(Some(target)) = fragment.targets.get_mut(0) {
                    let component = BlendComponent {
                        src_factor,
                        dst_factor,
                        operation: BlendOperation::Add,
                    };
                    target.blend = Some(BlendState {
                        color: component,
                        alpha: component,
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Resource)]
pub struct EffectAssets {
    pub spark_mesh: Handle<Mesh>,

    pub spark_material: Handle<StandardMaterial>,

    pub worn_material: Handle<StandardMaterial>,
}

#[derive(Component, Debug, Clone)]
pub struct PonEfInstance {
    pub ef: std::sync::Arc<meteor_asset::pon_ef::PonEf>,

    pub elapsed: f32,

    pub duration: f32,

    pub looping: bool,

    pub audio_playing: Vec<Option<Entity>>,

    pub audio_triggered: Vec<bool>,
}

#[derive(Component)]
pub(crate) struct PonEfStartTime(f32);

#[derive(Component)]
pub(crate) struct PonEfClockAdvanced(bool);

pub(crate) fn advance_pon_ef_clocks(
    mut commands: Commands,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut players: Query<(
        Entity,
        &mut PonEfInstance,
        Option<&PonEfStartTime>,
        Option<&mut PonEfClockAdvanced>,
    )>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    for (entity, mut player, start_time, advanced) in &mut players {
        if start_time.is_some() {
            continue;
        }
        player.elapsed += dt;
        if let Some(mut advanced) = advanced {
            advanced.0 = true;
        } else {
            commands.entity(entity).insert(PonEfClockAdvanced(true));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioWindowAction {
    None,
    Start,
    Stop,
}

fn update_audio_window_state(triggered: &mut bool, inside: bool) -> AudioWindowAction {
    match (inside, *triggered) {
        (true, false) => {
            *triggered = true;
            AudioWindowAction::Start
        }
        (false, true) => {
            *triggered = false;
            AudioWindowAction::Stop
        }
        _ => AudioWindowAction::None,
    }
}

#[derive(Component, Debug, Clone)]
pub struct PonEfNodeEntity(pub usize);

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct PonEfNodeOwner(Entity);

#[derive(Default)]
pub(crate) struct EffectEntityIndex(std::collections::HashMap<Entity, Vec<Entity>>);

impl EffectEntityIndex {
    fn rebuild(&mut self, entries: impl Iterator<Item = (Entity, Entity)>) {
        for entities in self.0.values_mut() {
            entities.clear();
        }
        for (owner, entity) in entries {
            self.0.entry(owner).or_default().push(entity);
        }
        self.0.retain(|_, entities| !entities.is_empty());
    }

    fn get(&self, owner: Entity) -> &[Entity] {
        self.0.get(&owner).map_or(&[], Vec::as_slice)
    }
}

#[derive(Component, Debug, Clone)]
pub struct PonEfParticleGroup {
    pub params: meteor_asset::pon_ef::PonEfParticleParams,

    pub anchor_a: String,
    pub anchor_b: String,

    pub started: bool,

    pub rng_state: u32,
}

#[derive(Component, Debug)]
pub struct PonEfParticle {
    pub group: Entity,

    pub material: Handle<PonEfMaterial>,
    pub color_slot: u32,
    pub age: f32,
    pub life: f32,

    pub size: f32,

    pub speed: f32,

    pub dir: bevy::math::Vec3,

    pub spawn_offset: bevy::math::Vec3,

    pub spawn_position: bevy::math::Vec3,

    pub offset: bevy::math::Vec3,
    pub alive: bool,
    pub spent: bool,
}

#[derive(Debug, Clone, Copy)]
struct PonEfDragSample {
    time: f32,
    base: Vec3,
    tip: Vec3,
}

#[derive(Component, Debug)]
struct PonEfDragRibbon {
    owner: Entity,
    anchor_indices: [Option<usize>; 2],
    lookback: f32,
    history: Vec<PonEfDragSample>,
    mesh: Handle<Mesh>,
}

const PON_EF_DRAG_HISTORY: f32 = 0.5;
const PON_EF_DRAG_SEGMENTS: usize = 20;

fn pon_ef_drag_mesh() -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0; 3]; 4])
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 4])
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0; 2]; 4])
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0; 4]; 4])
    .with_inserted_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]))
}

fn build_pon_ef_drag(
    history: &[PonEfDragSample],
    now: f32,
    lookback: f32,
) -> (Vec<[f32; 3]>, Vec<[f32; 4]>, Vec<u32>) {
    let Some(newest) = history.last() else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let start = now - lookback.clamp(0.0, PON_EF_DRAG_HISTORY);
    let mut samples: Vec<_> = history
        .iter()
        .copied()
        .filter(|s| s.time >= start)
        .collect();
    if samples.len() < 2 {
        return (Vec::new(), Vec::new(), Vec::new());
    }
    samples.sort_by(|a, b| a.time.total_cmp(&b.time));
    let mut positions = Vec::with_capacity(PON_EF_DRAG_SEGMENTS * 4);
    let mut colors = Vec::with_capacity(PON_EF_DRAG_SEGMENTS * 4);
    let mut indices = Vec::with_capacity(PON_EF_DRAG_SEGMENTS * 6);
    for segment in 0..PON_EF_DRAG_SEGMENTS {
        let front_t = newest.time - lookback * segment as f32 / PON_EF_DRAG_SEGMENTS as f32;
        let back_t = newest.time - lookback * (segment + 1) as f32 / PON_EF_DRAG_SEGMENTS as f32;
        let sample_at = |t: f32| -> PonEfDragSample {
            samples
                .windows(2)
                .find_map(|pair| {
                    (pair[0].time <= t && t <= pair[1].time).then(|| {
                        let f = (t - pair[0].time) / (pair[1].time - pair[0].time).max(1e-5);
                        PonEfDragSample {
                            time: t,
                            base: pair[0].base.lerp(pair[1].base, f),
                            tip: pair[0].tip.lerp(pair[1].tip, f),
                        }
                    })
                })
                .unwrap_or_else(|| {
                    if t < samples[0].time {
                        samples[0]
                    } else {
                        *samples.last().unwrap()
                    }
                })
        };
        let front = sample_at(front_t);
        let back = sample_at(back_t);
        let alpha_front =
            (PON_EF_DRAG_SEGMENTS - segment) as f32 / PON_EF_DRAG_SEGMENTS as f32 * 0.5;
        let alpha_back =
            (PON_EF_DRAG_SEGMENTS - segment - 1) as f32 / PON_EF_DRAG_SEGMENTS as f32 * 0.5;
        let first = positions.len() as u32;
        positions.extend([
            front.base.to_array(),
            back.base.to_array(),
            back.tip.to_array(),
            front.tip.to_array(),
        ]);
        colors.extend([
            [1.0, 1.0, 1.0, alpha_front],
            [1.0, 1.0, 1.0, alpha_back],
            [1.0, 1.0, 1.0, alpha_back],
            [1.0, 1.0, 1.0, alpha_front],
        ]);
        indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }
    (positions, colors, indices)
}

#[derive(Component, Debug, Clone)]
pub struct LockMarkerFx {
    pub kind: &'static str,

    pub target: Entity,
}

#[derive(Component, Debug, Clone)]
pub struct WornVisual {
    pub effect: String,
}

fn pon_ef_playback_time(instance: &PonEfInstance, worn: bool) -> (f32, bool) {
    let holds_models = worn
        && instance.duration > 0.0
        && instance.elapsed > instance.duration + 1e-4
        && instance
            .ef
            .nodes
            .iter()
            .any(|node| node.type_id == meteor_asset::pon_ef::TYPE_MODEL);
    if holds_models {
        (instance.duration, true)
    } else if instance.looping && instance.duration > 0.0 {
        (instance.elapsed % instance.duration, false)
    } else {
        (instance.elapsed, false)
    }
}

#[derive(Component, Debug, Clone, Copy)]
struct EscortGroundVisual {
    owner: Entity,
}

type WornVisualFighterQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static crate::components::fighter::WornEffect>,
        Option<&'static EscortFlagCarrier>,
        Option<&'static crate::plugins::stage_plugin::EscortCarrying>,
        Option<&'static crate::plugins::stage_plugin::LocalFighter>,
        Option<&'static crate::plugins::stage_plugin::MissionNpc>,
        Option<&'static crate::plugins::network_plugin::RemoteFighter>,
    ),
    With<Fighter>,
>;

type EscortGroundOwnerQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static crate::plugins::stage_plugin::ModeObjective>,
        Option<&'static Visibility>,
    ),
    Or<(
        With<crate::plugins::stage_plugin::ModeObjective>,
        With<crate::plugins::stage_plugin::NetworkEscortFlag>,
    )>,
>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PonEfShapeMeshKey {
    type_id: u8,
    common_value_0: u32,
    tail_bits: Vec<u32>,
}

fn pon_ef_shape_value(values: &[f32], index: usize, fallback: f32, minimum: f32) -> f32 {
    values
        .get(index)
        .copied()
        .filter(|value| value.is_finite() && *value != 0.0)
        .unwrap_or(fallback)
        .max(minimum)
}

fn pon_ef_cylinder_mesh(radius_start: f32, radius_end: f32, height: f32, shifted: bool) -> Mesh {
    const RESOLUTION: usize = 20;
    let mut positions = Vec::with_capacity((RESOLUTION + 1) * 2);
    let mut normals = Vec::with_capacity((RESOLUTION + 1) * 2);
    let mut uvs = Vec::with_capacity((RESOLUTION + 1) * 2);
    let z_offset = if shifted { height * 0.5 } else { 0.0 };
    let normal_z = (radius_end - radius_start) / height.max(0.01);
    for ring in 0..=1 {
        let t = ring as f32;
        let radius = radius_end + (radius_start - radius_end) * t;
        let z = -height * 0.5 + height * t + z_offset;
        for segment in 0..=RESOLUTION {
            let angle = segment as f32 / RESOLUTION as f32 * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            positions.push([radius * cos, radius * sin, z]);
            normals.push(Vec3::new(cos, sin, normal_z).normalize().to_array());
            uvs.push([segment as f32 / RESOLUTION as f32, t]);
        }
    }
    let stride = RESOLUTION + 1;
    let mut indices = Vec::with_capacity(RESOLUTION * 6);
    for segment in 0..RESOLUTION {
        let lower = segment as u32;
        let upper = (stride + segment) as u32;
        indices.extend([lower, lower + 1, upper, upper, lower + 1, upper + 1]);
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

fn pon_ef_shape_mesh(node: &meteor_asset::pon_ef::PonEfNode) -> Option<Mesh> {
    let meteor_asset::pon_ef::PonEfTail::Floats(values) = &node.tail else {
        return None;
    };
    Some(match node.type_id {
        meteor_asset::pon_ef::TYPE_SPHERE => {
            let radius = pon_ef_shape_value(values, 0, 1.0, 0.01);
            Mesh::from(Sphere::new(radius))
        }
        meteor_asset::pon_ef::TYPE_BOX => {
            let x = pon_ef_shape_value(values, 0, 1.0, 0.01);
            let y = pon_ef_shape_value(values, 1, 1.0, 0.01);
            let z = pon_ef_shape_value(values, 2, 1.0, 0.01);
            Mesh::from(Cuboid::new(x, y, z))
        }
        meteor_asset::pon_ef::TYPE_CYLINDER => {
            let radius_start = pon_ef_shape_value(values, 0, 1.0, 0.001);
            let radius_end = pon_ef_shape_value(values, 1, 0.0, 0.001);
            let height = pon_ef_shape_value(values, 2, 1.0, 0.01);
            pon_ef_cylinder_mesh(radius_start, radius_end, height, node.values[0] == 1)
        }
        meteor_asset::pon_ef::TYPE_DONUT => {
            let radius = pon_ef_shape_value(values, 0, 5.0, 0.01);
            let tube = pon_ef_shape_value(values, 1, radius * 0.25, 0.01);
            Mesh::from(Torus {
                major_radius: radius,
                minor_radius: tube,
            })
            .transformed_by(Transform::from_rotation(Quat::from_rotation_x(
                std::f32::consts::FRAC_PI_2,
            )))
        }
        meteor_asset::pon_ef::TYPE_PLANE | meteor_asset::pon_ef::TYPE_BILLBOARD => {
            let width = pon_ef_shape_value(values, 0, 10.0, 0.01);
            let height = pon_ef_shape_value(values, 1, 10.0, 0.01);
            let mesh = Mesh::from(Rectangle::new(width, height));
            if node.type_id == meteor_asset::pon_ef::TYPE_PLANE {
                mesh.transformed_by(Transform::from_rotation(Quat::from_rotation_x(
                    std::f32::consts::FRAC_PI_2,
                )))
            } else {
                mesh
            }
        }
        _ => return None,
    })
}

#[derive(Resource, Default)]
pub struct EfCache {
    map: std::collections::HashMap<String, std::sync::Arc<meteor_asset::pon_ef::PonEf>>,
    shape_meshes: std::collections::HashMap<PonEfShapeMeshKey, Handle<Mesh>>,
}

impl EfCache {
    pub fn load(&mut self, name: &str) -> Option<std::sync::Arc<meteor_asset::pon_ef::PonEf>> {
        if let Some(ef) = self.map.get(name) {
            return Some(ef.clone());
        }
        let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let roots = resource_roots(&root);
        let candidates = ef_candidates(name);
        let bytes = candidates
            .iter()
            .find_map(|candidate| {
                meteor_asset::runtime_assets::read_legacy_path(&format!("SFX/{candidate}.ef"))
            })
            .or_else(|| {
                candidates
                    .into_iter()
                    .flat_map(|candidate| {
                        roots
                            .iter()
                            .map(move |root| root.join("SFX").join(format!("{candidate}.ef")))
                    })
                    .find_map(|path| crate::resource_io::read(path).ok())
            })?;
        let ef = meteor_asset::pon_ef::parse_pon_ef(&bytes).ok()?;
        let ef = std::sync::Arc::new(ef);
        self.map.insert(name.to_string(), ef.clone());
        Some(ef)
    }

    fn shape_mesh_for(
        &mut self,
        meshes: &mut Assets<Mesh>,
        node: &meteor_asset::pon_ef::PonEfNode,
    ) -> Option<Handle<Mesh>> {
        let meteor_asset::pon_ef::PonEfTail::Floats(values) = &node.tail else {
            return None;
        };
        let key = PonEfShapeMeshKey {
            type_id: node.type_id,
            common_value_0: node.values[0],
            tail_bits: values.iter().map(|value| value.to_bits()).collect(),
        };
        if let Some(handle) = self.shape_meshes.get(&key) {
            return Some(handle.clone());
        }
        let handle = meshes.add(pon_ef_shape_mesh(node)?);
        self.shape_meshes.insert(key, handle.clone());
        Some(handle)
    }
}

fn ef_candidates(name: &str) -> Vec<String> {
    let base = name.trim_end_matches(".ef");
    let lower = base.to_lowercase();
    let upper_pos = if lower.starts_with("pos") {
        format!("Pos{}", &base[3..])
    } else {
        base.to_string()
    };
    let upper = base.to_uppercase();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for candidate in [base.to_string(), lower, upper_pos, upper] {
        if seen.insert(candidate.clone()) {
            out.push(candidate);
        }
    }
    out
}

pub fn effect_file_name(effect_id: &meteor_asset::pos::EffectId) -> Option<String> {
    match effect_id {
        meteor_asset::pos::EffectId::Int(0) => None,
        meteor_asset::pos::EffectId::Int(n) => Some(format!("{n}")),
        meteor_asset::pos::EffectId::Str(s) if s == "0" => None,
        meteor_asset::pos::EffectId::Str(s) => Some(s.clone()),
    }
}

#[derive(Resource, Default)]
pub struct EfTextureCache {
    paks: Option<Vec<meteor_asset::pak::PakArchive>>,

    model_paks: Option<Vec<meteor_asset::pak::PakArchive>>,

    materials: std::collections::HashMap<(String, u8, u8, bool), Handle<PonEfMaterial>>,

    ifl_frames: std::collections::HashMap<String, Vec<Handle<Image>>>,
}

impl EfTextureCache {
    fn roots() -> Vec<std::path::PathBuf> {
        let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        resource_roots(&root)
    }

    fn ensure_paks(&mut self) -> &[meteor_asset::pak::PakArchive] {
        if self.paks.is_none() {
            let roots = Self::roots();
            self.paks = Some(
                ["ETexture.pak", "TexturePatch.pak"]
                    .into_iter()
                    .filter_map(|name| {
                        roots.iter().find_map(|root| {
                            let bytes = crate::resource_io::read(root.join(name)).ok()?;
                            meteor_asset::pak::PakArchive::from_bytes(&bytes).ok()
                        })
                    })
                    .collect(),
            );
        }
        self.paks.as_deref().unwrap_or(&[])
    }

    pub fn texture_bytes(&mut self, name: &str) -> Option<Vec<u8>> {
        crate::plugins::pak_textures::resolve_runtime_texture_exact(&["etexture", "patch"], name)
            .or_else(|| {
                let paks: Vec<_> = self.ensure_paks().iter().collect();
                crate::plugins::pak_textures::resolve_pak_texture_exact(&paks, name)
            })
    }

    fn ensure_model_paks(&mut self) -> &[meteor_asset::pak::PakArchive] {
        if self.model_paks.is_none() {
            self.model_paks = Some(
                Self::roots()
                    .iter()
                    .find(|root| root.join("ctexture.pak").is_file())
                    .map_or_else(Vec::new, |root| {
                        crate::plugins::pak_textures::pak_chain(root, "ctexture")
                    }),
            );
        }
        self.model_paks.as_deref().unwrap_or(&[])
    }

    fn model_texture_bytes(&mut self, name: &str) -> Option<Vec<u8>> {
        crate::plugins::pak_textures::resolve_runtime_texture(
            &["patch", "ctexture", "ptexture"],
            name,
        )
        .or_else(|| {
            let refs: Vec<&meteor_asset::pak::PakArchive> =
                self.ensure_model_paks().iter().collect();
            crate::plugins::pak_textures::resolve_pak_texture(&refs, name)
        })
    }

    fn model_texture_handle(
        &mut self,
        images: &mut Assets<Image>,
        name: &str,
        mat: &meteor_asset::gmdl::GmdlMaterial,
    ) -> Option<Handle<Image>> {
        let bytes = self.model_texture_bytes(name)?;
        let alpha = if mat.blend.eq_ignore_ascii_case("ALPHACHANNEL") {
            super::pak_textures::legacy_alpha_companion_name(name).and_then(|name| {
                super::pak_textures::resolve_runtime_texture_exact(
                    &["patch", "ctexture", "ptexture"],
                    &name,
                )
                .or_else(|| {
                    let paks: Vec<_> = self.ensure_model_paks().iter().collect();
                    super::pak_textures::resolve_pak_texture_exact(&paks, &name)
                })
            })
        } else {
            None
        };
        super::pak_textures::image_handle_with_legacy_style(
            images,
            &bytes,
            alpha.as_deref(),
            mat.blend.eq_ignore_ascii_case("ALPHACHANNEL")
                && !name.to_ascii_lowercase().ends_with(".tga"),
            mat.blend.eq_ignore_ascii_case("COLORKEY"),
        )
    }

    pub fn material_for(
        &mut self,
        images: &mut Assets<Image>,
        materials: &mut Assets<PonEfMaterial>,
        node: &meteor_asset::pon_ef::PonEfNode,
    ) -> Handle<PonEfMaterial> {
        let texture = &node.texture;
        let state = meteor_asset::pon_ef::decode_render_state(&node.transform_header);
        let key = (
            texture.to_lowercase(),
            state.blend_src,
            state.blend_dst,
            state.two_sided,
        );
        if let Some(handle) = self.materials.get(&key) {
            return handle.clone();
        }

        let image = if texture.is_empty() {
            None
        } else if texture.to_ascii_lowercase().ends_with(".ifl") {
            self.ifl_frames(images, texture).first().cloned()
        } else {
            self.texture_bytes(texture)
                .and_then(|bytes| crate::plugins::pak_textures::image_handle(images, &bytes))
        };
        let handle = materials.add(PonEfMaterial {
            uniform_id: dynamic_uniforms::new_uniform_id(),
            particle_colors: None,
            params: PonEfMaterialParams {
                color: Vec4::ONE,
                has_texture: u32::from(image.is_some()),
            },
            texture: image,
            blend: PonEfBlendMode::from_state(state),
            two_sided: state.two_sided,
        });
        self.materials.insert(key, handle.clone());
        handle
    }

    pub fn ifl_frames(&mut self, images: &mut Assets<Image>, ifl_name: &str) -> Vec<Handle<Image>> {
        let key = ifl_name.to_lowercase();
        if let Some(frames) = self.ifl_frames.get(&key) {
            return frames.clone();
        }
        let mut frames = Vec::new();
        if let Some(bytes) = self.texture_bytes(ifl_name) {
            if let Ok(text) = String::from_utf8(bytes) {
                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
                        continue;
                    }
                    if let Some(image) = self
                        .texture_bytes(line)
                        .and_then(|b| crate::plugins::pak_textures::image_handle(images, &b))
                    {
                        frames.push(image);
                    }
                }
            }
        }
        self.ifl_frames.insert(key, frames.clone());
        frames
    }
}

fn clone_effect_material(
    materials: &mut Assets<PonEfMaterial>,
    template: &Handle<PonEfMaterial>,
) -> Handle<PonEfMaterial> {
    materials
        .get(template)
        .cloned()
        .map(|mut material| {
            material.uniform_id = dynamic_uniforms::new_uniform_id();
            materials.add(material)
        })
        .unwrap_or_else(|| template.clone())
}

#[derive(Component, Debug, Clone)]
pub struct PonEfModelNode;

#[derive(Component, Debug)]
pub struct IflAnimated {
    pub material: Handle<PonEfMaterial>,
    pub frames: Vec<Handle<Image>>,
    pub index: usize,
    pub age: f32,
}

fn attach_effect_ifl(
    commands: &mut Commands,
    entity: Entity,
    material: Handle<PonEfMaterial>,
    texture: &str,
    textures: &mut EfTextureCache,
    images: &mut Assets<Image>,
) {
    if texture.to_ascii_lowercase().ends_with(".ifl") {
        let frames = textures.ifl_frames(images, texture);
        if frames.len() > 1 {
            commands.entity(entity).insert(IflAnimated {
                material,
                frames,
                index: 0,
                age: 0.0,
            });
        }
    }
}

#[derive(Resource, Default)]
pub struct PonEfShapeMeshes {
    pub particle_plane: Option<Handle<Mesh>>,
}

const SPARK_COUNT: usize = 8;

const SPARK_LIFE: f32 = 0.35;

const SPARK_SPEED: f32 = 3.0;

pub struct EffectPlugin;

fn install_effect_playback_systems(app: &mut App) {
    let clocks = advance_pon_ef_clocks.run_if(crate::plugins::simulation_frame);
    app.add_systems(
        Update,
        if super::fighter_frame::installed(app) {
            clocks
                .after(crate::plugins::FighterFrameSet::PoseDecision)
                .before(super::fighter_frame::SerialFighterSet)
        } else {
            clocks
                .after(crate::plugins::FighterFrameSet::SkeletonSample)
                .before(crate::plugins::FighterFrameSet::HitJudge)
        },
    );

    app.add_systems(
        Update,
        (
            scene_script_effects,
            combat_hit_effects,
            pose_effect_spawn,
            tick_pon_ef_players,
            tick_pon_ef_drags,
            tick_pon_ef_particles,
            tick_ifl_animations,
        )
            .chain()
            .after(crate::plugins::FighterFrameSet::CameraFinalize)
            .run_if(crate::plugins::simulation_frame),
    );
}

impl Plugin for EffectPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<PonEfMaterial>::default());
        dynamic_uniforms::install(app);
        bevy::asset::load_internal_asset!(
            app,
            PON_EF_SHADER_HANDLE,
            "pon_ef_material.wgsl",
            Shader::from_wgsl
        );
        bevy::asset::load_internal_asset!(
            app,
            PON_EF_PARTICLE_SHADER_HANDLE,
            "pon_ef_particle.wgsl",
            Shader::from_wgsl
        );
        app.init_resource::<EfTextureCache>()
            .init_resource::<EfCache>()
            .add_message::<SceneEffectEvent>()
            .add_systems(PreStartup, setup_effect_assets)
            .add_systems(
                OnEnter(crate::app_state::AppState::Playing),
                spawn_stage_ambient_effects,
            )
            .add_systems(
                Update,
                (
                    worn_effect_visual,
                    escort_ground_effect_visual,
                    spin_worn_visual,
                )
                    .run_if(crate::plugins::simulation_frame),
            )
            .add_systems(Update, lock_marker_effects)
            .add_systems(
                Update,
                tick_hit_sparks.run_if(in_state(crate::app_state::AppState::Playing)),
            )
            .add_systems(
                OnExit(crate::app_state::AppState::Playing),
                (despawn_hit_sparks, despawn_worn_visuals, despawn_pon_ef),
            );
        install_effect_playback_systems(app);
    }
}

fn stage_ambient_effects(des: &str, center: [f32; 3]) -> Vec<(String, Vec3)> {
    let desc = meteor_asset::des::parse_stage_description(des);
    desc.scene_objects
        .iter()
        .chain(&desc.dummies)
        .filter_map(|object| {
            let effect = object.effect.as_ref()?.clone();
            let p = object.position;
            Some((
                effect,
                Vec3::new(
                    (p.x - center[0]) * meteor_core::constants::MODEL_SCALE,
                    (p.z - center[2]) * meteor_core::constants::MODEL_SCALE,
                    -(p.y - center[1]) * meteor_core::constants::MODEL_SCALE,
                ),
            ))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn spawn_stage_ambient_effects(
    mut commands: Commands,
    loaded: Res<crate::app_state::LoadedStageInfo>,
    loaded_assets: Res<crate::app_state::LoadedStageAssets>,
    mut cache: ResMut<EfCache>,
    shapes: Res<PonEfShapeMeshes>,
    assets: Res<EffectAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<PonEfMaterial>>,
    mut textures: ResMut<EfTextureCache>,
    mut images: ResMut<Assets<Image>>,
    mut gmdl_materials: ResMut<Assets<GmdlMaterial>>,
) {
    let Some(des) = loaded_assets
        .des
        .as_deref()
        .map(crate::text_codec::decode_text)
    else {
        return;
    };
    let effects = stage_ambient_effects(&des, loaded.center);
    let mut spawned = 0usize;
    for (effect, position) in &effects {
        let mut ctx = EfSpawnContext {
            shapes: &shapes,
            assets: &assets,
            meshes: &mut meshes,
            materials: &mut materials,
            gmdl_materials: &mut gmdl_materials,
            textures: &mut textures,
            images: &mut images,
        };
        spawned += usize::from(
            spawn_pon_ef(
                &mut commands,
                &mut cache,
                &mut ctx,
                effect,
                *position,
                None,
                true,
            )
            .is_some(),
        );
    }
    info!(
        "stage ambient effects: {spawned}/{} spawned (stage={})",
        effects.len(),
        loaded.stage_id
    );
}

pub(crate) fn setup_effect_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-0.05, -0.05, 0.0],
            [-0.05, 0.05, 0.0],
            [0.05, 0.05, 0.0],
            [0.05, 0.05, 0.0],
            [0.05, -0.05, 0.0],
            [-0.05, -0.05, 0.0],
        ],
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 6]);
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![
            [0.0, 0.0],
            [0.0, 1.0],
            [1.0, 1.0],
            [1.0, 1.0],
            [1.0, 0.0],
            [0.0, 0.0],
        ],
    );
    mesh.insert_indices(bevy::render::mesh::Indices::U32(vec![0, 1, 2, 3, 4, 5]));
    let spark_mesh = meshes.add(mesh);
    commands.insert_resource(EffectAssets {
        spark_mesh: spark_mesh.clone(),
        spark_material: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.75, 0.25),
            emissive: LinearRgba::rgb(1.0, 0.5, 0.1),
            unlit: true,
            ..default()
        }),
        worn_material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.3, 0.9, 0.8),
            emissive: LinearRgba::rgb(0.1, 0.6, 0.5),
            unlit: true,
            ..default()
        }),
    });

    commands.insert_resource(PonEfShapeMeshes {
        particle_plane: Some(meshes.add(Mesh::from(Rectangle::new(1.0, 1.0)))),
    });
}

pub fn spawn_hit_burst(
    commands: &mut Commands,
    assets: &EffectAssets,
    position: Vec3,
    hit_direction: Vec3,
) {
    let hit_dir = if hit_direction.length_squared() > 1e-6 {
        hit_direction.normalize()
    } else {
        Vec3::Y
    };
    for i in 0..SPARK_COUNT {
        let angle = i as f32 * std::f32::consts::TAU / SPARK_COUNT as f32;

        let spread = Quat::from_rotation_arc(Vec3::Y, hit_dir)
            * Quat::from_rotation_y(angle)
            * Quat::from_rotation_x((i as f32 * 0.7).sin() * 0.8);
        let velocity = spread * Vec3::Y * SPARK_SPEED;
        commands.spawn((
            HitSpark {
                velocity,
                life: SPARK_LIFE,
                max_life: SPARK_LIFE,
            },
            Mesh3d(assets.spark_mesh.clone()),
            MeshMaterial3d(assets.spark_material.clone()),
            Transform::from_translation(position).with_rotation(Quat::from_rotation_arc(
                Vec3::Z,
                velocity.normalize_or_zero(),
            )),
        ));
    }
}

fn tick_hit_sparks(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut HitSpark, &mut Transform)>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let dt = frame_clock
        .as_deref()
        .map(|clock| clock.wall_dt)
        .unwrap_or(dt);
    for (entity, mut spark, mut transform) in &mut query {
        spark.life -= dt;
        if spark.life <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        transform.translation += spark.velocity * dt;
        spark.velocity *= 0.88;
        let scale = (spark.life / spark.max_life).max(0.05);
        transform.scale = Vec3::splat(scale);
    }
}

fn despawn_hit_sparks(mut commands: Commands, query: Query<Entity, With<HitSpark>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

fn vip_effect_for_fighter(
    request: &crate::app_state::LaunchRequest,
    net: &crate::plugins::network_plugin::NetState,
    local: Option<&crate::plugins::stage_plugin::LocalFighter>,
    mission: Option<&crate::plugins::stage_plugin::MissionNpc>,
    remote: Option<&crate::plugins::network_plugin::RemoteFighter>,
) -> Option<&'static str> {
    if request.mode != "assassin" {
        return None;
    }
    let team = mission
        .filter(|mission| mission.is_vip)
        .map(|mission| mission.team)
        .or_else(|| {
            let player_id = if local.is_some() {
                (!net.player_id.is_empty()).then_some(net.player_id.as_str())
            } else {
                remote.map(|remote| remote.player_id.as_str())
            }?;
            net.vip_team(player_id)
        })?;
    match team {
        1 => Some("vipblue.ef"),
        2 => Some("vipred.ef"),
        _ => None,
    }
}

fn wanted_worn_effects(
    request: &crate::app_state::LaunchRequest,
    net: &crate::plugins::network_plugin::NetState,
    worn: Option<&crate::components::fighter::WornEffect>,
    flag_carrier: bool,
    local: Option<&crate::plugins::stage_plugin::LocalFighter>,
    mission: Option<&crate::plugins::stage_plugin::MissionNpc>,
    remote: Option<&crate::plugins::network_plugin::RemoteFighter>,
) -> Vec<String> {
    let mut effects = Vec::with_capacity(3);
    if let Some(worn) = worn {
        effects.extend(
            worn.effects
                .iter()
                .filter(|effect| !effect.name.is_empty() && effect.remaining > 0.0)
                .map(|effect| effect.name.clone()),
        );
    }
    if flag_carrier && !effects.iter().any(|effect| effect == "Flag.ef") {
        effects.push("Flag.ef".to_string());
    }
    if let Some(vip) = vip_effect_for_fighter(request, net, local, mission, remote) {
        if !effects.iter().any(|effect| effect == vip) {
            effects.push(vip.to_string());
        }
    }
    effects
}

#[allow(clippy::too_many_arguments)]
fn worn_effect_visual(
    mut commands: Commands,
    request: Res<crate::app_state::LaunchRequest>,
    net: Res<crate::plugins::network_plugin::NetState>,
    assets: Res<EffectAssets>,
    mut cache: ResMut<EfCache>,
    shapes: Option<Res<PonEfShapeMeshes>>,
    mut materials: ResMut<Assets<PonEfMaterial>>,
    mut textures: ResMut<EfTextureCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut gmdl_materials: ResMut<Assets<GmdlMaterial>>,
    fighters: WornVisualFighterQuery<'_, '_>,
    visuals: Query<(Entity, &WornVisual, &ChildOf)>,
) {
    for (visual_entity, visual, parent) in &visuals {
        let mismatch = fighters
            .get(parent.parent())
            .map(
                |(_, worn, flag_carrier, local_carrying, local, mission, remote)| {
                    !wanted_worn_effects(
                        &request,
                        &net,
                        worn,
                        flag_carrier.is_some() || local_carrying.is_some(),
                        local,
                        mission,
                        remote,
                    )
                    .contains(&visual.effect)
                },
            )
            .unwrap_or(true);
        if mismatch {
            commands.entity(visual_entity).despawn();
        }
    }

    for (fighter_entity, worn, flag_carrier, local_carrying, local, mission, remote) in &fighters {
        let effects = wanted_worn_effects(
            &request,
            &net,
            worn,
            flag_carrier.is_some() || local_carrying.is_some(),
            local,
            mission,
            remote,
        );
        for effect in effects {
            let exists = visuals.iter().any(|(_, visual, parent)| {
                parent.parent() == fighter_entity && visual.effect == effect
            });
            if exists {
                continue;
            }
            let spawned = shapes.as_ref().and_then(|shapes| {
                let mut ctx = EfSpawnContext {
                    shapes,
                    assets: &assets,
                    meshes: &mut meshes,
                    materials: &mut materials,
                    gmdl_materials: &mut gmdl_materials,
                    textures: &mut textures,
                    images: &mut images,
                };
                spawn_pon_ef(
                    &mut commands,
                    &mut cache,
                    &mut ctx,
                    &effect,
                    Vec3::ZERO,
                    Some(fighter_entity),
                    true,
                )
            });
            if let Some(root) = spawned {
                commands.entity(root).insert(WornVisual { effect });
                continue;
            }
            commands
                .spawn((
                    WornVisual { effect },
                    Mesh3d(assets.spark_mesh.clone()),
                    MeshMaterial3d(assets.worn_material.clone()),
                    Transform::from_xyz(0.0, 1.3, 0.0),
                ))
                .insert(ChildOf(fighter_entity));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn escort_ground_effect_visual(
    mut commands: Commands,
    escort: Option<Res<super::escort_assets::LoadedEscort>>,
    owners: EscortGroundOwnerQuery<'_, '_>,
    visuals: Query<(Entity, &EscortGroundVisual)>,
    assets: Res<EffectAssets>,
    mut cache: ResMut<EfCache>,
    shapes: Option<Res<PonEfShapeMeshes>>,
    mut materials: ResMut<Assets<PonEfMaterial>>,
    mut textures: ResMut<EfTextureCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut gmdl_materials: ResMut<Assets<GmdlMaterial>>,
) {
    let desired: std::collections::HashSet<Entity> = owners
        .iter()
        .filter(|_| escort.as_ref().is_none_or(|assets| assets.0.is_none()))
        .filter(|(_, objective, visibility)| {
            objective.is_none_or(|objective| {
                objective.kind == crate::plugins::stage_plugin::ModeObjectiveKind::EscortCargo
            }) && !visibility.is_some_and(|visibility| *visibility == Visibility::Hidden)
        })
        .map(|(entity, _, _)| entity)
        .collect();

    for (entity, visual) in &visuals {
        if !desired.contains(&visual.owner) {
            commands.entity(entity).despawn();
        }
    }
    let Some(shapes) = shapes else {
        return;
    };
    for owner in desired {
        if visuals.iter().any(|(_, visual)| visual.owner == owner) {
            continue;
        }
        let mut ctx = EfSpawnContext {
            shapes: &shapes,
            assets: &assets,
            meshes: &mut meshes,
            materials: &mut materials,
            gmdl_materials: &mut gmdl_materials,
            textures: &mut textures,
            images: &mut images,
        };
        if let Some(root) = spawn_pon_ef(
            &mut commands,
            &mut cache,
            &mut ctx,
            "Flag",
            Vec3::ZERO,
            Some(owner),
            true,
        ) {
            commands.entity(root).insert(EscortGroundVisual { owner });
        }
    }
}

fn spin_worn_visual(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut query: Query<(&mut Transform, &WornVisual), Without<PonEfInstance>>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    for (mut transform, _) in &mut query {
        transform.rotation =
            Quat::from_rotation_y(transform.rotation.to_euler(EulerRot::XYZ).1 + dt * 2.0);
        let pulse = 1.0 + (time.elapsed_secs() * 4.0).sin() * 0.15;
        transform.scale = Vec3::splat(pulse);
    }
}

fn despawn_worn_visuals(mut commands: Commands, query: Query<Entity, With<WornVisual>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

#[allow(clippy::too_many_arguments)]

pub struct EfSpawnContext<'a> {
    pub shapes: &'a PonEfShapeMeshes,
    pub assets: &'a EffectAssets,
    pub meshes: &'a mut Assets<Mesh>,
    pub materials: &'a mut Assets<PonEfMaterial>,
    pub gmdl_materials: &'a mut Assets<GmdlMaterial>,
    pub textures: &'a mut EfTextureCache,
    pub images: &'a mut Assets<bevy::image::Image>,
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_pon_ef(
    commands: &mut Commands,
    cache: &mut EfCache,
    ctx: &mut EfSpawnContext<'_>,
    name: &str,
    world_position: Vec3,
    parent: Option<Entity>,
    looping: bool,
) -> Option<Entity> {
    let ef = cache.load(name)?;
    let duration = ef.nodes.iter().map(|n| n.end_time).fold(0.0f32, f32::max);
    let audio_playing = ef.nodes.iter().map(|_| None).collect();
    let audio_triggered = vec![false; ef.nodes.len()];
    let root = commands
        .spawn((
            PonEfInstance {
                ef: ef.clone(),
                elapsed: 0.0,
                duration,
                looping,
                audio_playing,
                audio_triggered,
            },
            Transform::from_translation(world_position),
        ))
        .id();
    if let Some(parent) = parent {
        commands.entity(root).insert(ChildOf(parent));
    } else {
        commands.entity(root).insert(SceneEffectAnchor);
    }

    let mut spawned = 0usize;
    for (index, node) in ef.nodes.iter().enumerate() {
        if meteor_asset::pon_ef::decode_render_state(&node.transform_header).hidden {
            continue;
        }
        if node.type_id == meteor_asset::pon_ef::TYPE_PARTICLE {
            let params = match &node.tail {
                meteor_asset::pon_ef::PonEfTail::Particle { parameters, .. } => {
                    meteor_asset::pon_ef::decode_pon_ef_particle(parameters)
                }
                _ => continue,
            };
            let group = commands
                .spawn((
                    PonEfNodeEntity(index),
                    PonEfNodeOwner(root),
                    PonEfParticleGroup {
                        params,
                        anchor_a: match &node.tail {
                            meteor_asset::pon_ef::PonEfTail::Particle { strings, .. } => {
                                strings[0].clone()
                            }
                            _ => String::new(),
                        },
                        anchor_b: match &node.tail {
                            meteor_asset::pon_ef::PonEfTail::Particle { strings, .. } => {
                                strings[1].clone()
                            }
                            _ => String::new(),
                        },
                        started: false,
                        rng_state: (root.to_bits() as u32)
                            .wrapping_mul(0x9e3779b9)
                            .wrapping_add(index as u32)
                            ^ 0x1234abcd,
                    },
                    Transform::from_translation(Vec3::ZERO),
                ))
                .insert(ChildOf(root))
                .id();
            let capacity = params.count.clamp(1, 256) as usize;
            let particle_material_template =
                ctx.textures.material_for(ctx.images, ctx.materials, node);
            let mut page = particle_material_template.clone();
            for index in 0..capacity {
                if index % dynamic_uniforms::PARAMS_PER_PAGE == 0 {
                    page = clone_effect_material(ctx.materials, &particle_material_template);
                    ctx.materials
                        .get_mut(&page)
                        .expect("particle template")
                        .particle_colors =
                        Some(Box::new([Vec4::ONE; dynamic_uniforms::PARAMS_PER_PAGE]));
                }
                let material = page.clone();
                let color_slot = (index % dynamic_uniforms::PARAMS_PER_PAGE) as u32;
                let particle_entity = commands
                    .spawn((
                        PonEfParticle {
                            group,
                            material: material.clone(),
                            color_slot,
                            age: 0.0,
                            life: 0.01,
                            size: 1.0,
                            speed: 0.0,
                            dir: bevy::math::Vec3::ZERO,
                            spawn_offset: bevy::math::Vec3::ZERO,
                            spawn_position: bevy::math::Vec3::ZERO,
                            offset: bevy::math::Vec3::ZERO,
                            alive: false,
                            spent: false,
                        },
                        Mesh3d(
                            ctx.shapes
                                .particle_plane
                                .clone()
                                .expect("particle plane mesh"),
                        ),
                        MeshMaterial3d(material.clone()),
                        bevy::mesh::MeshTag(color_slot),
                        Transform::from_translation(Vec3::ZERO),
                        Visibility::Hidden,
                    ))
                    .insert(ChildOf(root))
                    .id();

                if index % dynamic_uniforms::PARAMS_PER_PAGE == 0 {
                    attach_effect_ifl(
                        commands,
                        particle_entity,
                        page.clone(),
                        &node.texture,
                        ctx.textures,
                        ctx.images,
                    );
                }
            }
            spawned += 1;
            continue;
        }
        if node.type_id == meteor_asset::pon_ef::TYPE_MODEL {
            if let Some(model_entity) = spawn_pon_ef_model(commands, node, ctx, root) {
                let _ = model_entity;
                spawned += 1;
            }
            continue;
        }
        if node.type_id == meteor_asset::pon_ef::TYPE_DRAG {
            let meteor_asset::pon_ef::PonEfTail::Drag { value, anchors } = &node.tail else {
                continue;
            };
            let anchor_indices = anchors.clone().map(|name| {
                ef.nodes
                    .iter()
                    .position(|candidate| candidate.name == name)
                    .or_else(|| {
                        ef.nodes
                            .iter()
                            .position(|candidate| candidate.name.eq_ignore_ascii_case(&name))
                    })
            });
            let mesh = ctx.meshes.add(pon_ef_drag_mesh());
            let material = ctx.textures.material_for(ctx.images, ctx.materials, node);
            let material = clone_effect_material(ctx.materials, &material);
            let ribbon_entity = commands
                .spawn((
                    PonEfDragRibbon {
                        owner: root,
                        anchor_indices,
                        lookback: *value,
                        history: Vec::new(),
                        mesh: mesh.clone(),
                    },
                    Mesh3d(mesh),
                    MeshMaterial3d(material.clone()),
                    Transform::IDENTITY,
                    Visibility::Hidden,
                ))
                .insert(ChildOf(root))
                .id();
            attach_effect_ifl(
                commands,
                ribbon_entity,
                material,
                &node.texture,
                ctx.textures,
                ctx.images,
            );
            spawned += 1;
            continue;
        }
        let Some(mesh) = cache.shape_mesh_for(ctx.meshes, node) else {
            continue;
        };

        let material_template = ctx.textures.material_for(ctx.images, ctx.materials, node);
        let material = clone_effect_material(ctx.materials, &material_template);
        let node_entity = commands
            .spawn((
                PonEfNodeEntity(index),
                PonEfNodeOwner(root),
                Mesh3d(mesh),
                MeshMaterial3d(material.clone()),
                Transform::from_translation(Vec3::ZERO),
                Visibility::Hidden,
            ))
            .insert(ChildOf(root))
            .id();
        attach_effect_ifl(
            commands,
            node_entity,
            material,
            &node.texture,
            ctx.textures,
            ctx.images,
        );
        spawned += 1;
    }
    if spawned == 0
        && !ef
            .nodes
            .iter()
            .any(|node| node.type_id == meteor_asset::pon_ef::TYPE_AUDIO)
    {
        commands.entity(root).despawn();
        return None;
    }
    Some(root)
}

type PonEfPlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut PonEfInstance,
        &'static GlobalTransform,
        Option<&'static ChildOf>,
        Option<&'static WornVisual>,
        Option<&'static SceneEffectAnchor>,
        Option<&'static PonEfStartTime>,
        Option<&'static mut PonEfClockAdvanced>,
    ),
    Without<PonEfNodeEntity>,
>;

type PonEfParticlePlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static PonEfInstance,
        &'static GlobalTransform,
        Option<&'static ChildOf>,
        Option<&'static WornVisual>,
        Option<&'static SceneEffectAnchor>,
    ),
>;

type PonEfVisualNodeQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static PonEfNodeEntity,
        &'static PonEfNodeOwner,
        &'static mut Transform,
        Option<&'static MeshMaterial3d<PonEfMaterial>>,
        Option<&'static mut Visibility>,
    ),
    Without<PonEfInstance>,
>;

fn bevy_mat4_to_core(matrix: bevy::math::Mat4) -> meteor_core::Mat4 {
    meteor_core::Mat4 {
        m: matrix.to_cols_array(),
    }
}

fn scene_effect_source_frame() -> meteor_core::Mat4 {
    bevy_mat4_to_core(bevy::math::Mat4::from_scale_rotation_translation(
        Vec3::splat(meteor_asset::pon_ef::PON_EF_SCALE),
        Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
        Vec3::ZERO,
    ))
}

fn external_frame_in_root(
    root_inverse: bevy::math::Mat4,
    world: bevy::math::Mat4,
) -> meteor_core::Mat4 {
    bevy_mat4_to_core(root_inverse * world)
}

fn camera_facing_local_rotation(root_global: &GlobalTransform, camera_rotation: Quat) -> Quat {
    root_global.to_scale_rotation_translation().1.inverse() * camera_rotation
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn tick_pon_ef_players(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut commands: Commands,
    mut sfx: ResMut<SfxCache>,
    mut audio_assets: ResMut<Assets<AudioSource>>,
    mut players: PonEfPlayerQuery<'_, '_>,
    mut node_access: ParamSet<(
        bevy::transform::helper::TransformHelper,
        PonEfVisualNodeQuery<'_, '_>,
    )>,
    mut materials: ResMut<Assets<PonEfMaterial>>,
    endpoints: Query<(
        Entity,
        &crate::plugins::character_plugin::WeaponEndpoint,
        &GlobalTransform,
    )>,
    fighters: Query<(
        &crate::components::fighter::Fighter,
        &crate::components::fighter::WeaponForm,
        &crate::components::fighter::PoseRuntime,
    )>,
    fighter_models: Query<(
        Entity,
        &crate::plugins::character_plugin::FighterModel,
        &GlobalTransform,
    )>,
    bones: Query<(
        Entity,
        &crate::plugins::character_plugin::FighterBone,
        &GlobalTransform,
    )>,
    dummies: Query<(
        Entity,
        &crate::plugins::character_plugin::FighterDummy,
        &GlobalTransform,
    )>,
    cameras: Query<Entity, With<crate::plugins::camera_plugin::GameplayCamera>>,
    mut node_index: Local<EffectEntityIndex>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(16);
    let current_camera_rotation = cameras.iter().next().and_then(|camera| {
        node_access
            .p0()
            .compute_global_transform(camera)
            .ok()
            .map(|frame| frame.to_scale_rotation_translation().1)
    });
    node_index.rebuild(
        node_access
            .p1()
            .iter()
            .map(|(entity, _, owner, ..)| (owner.0, entity)),
    );
    if players.is_empty() {
        return;
    }
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let roots =
        resource_roots(&std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
    let mut finished = Vec::new();

    let endpoint_matrices: std::collections::HashMap<(Entity, String), bevy::math::Mat4> =
        endpoints
            .iter()
            .filter_map(|(entity, endpoint, transform)| {
                let (fighter, form, pose) = fighters.get(endpoint.fighter).ok()?;
                (fighter.weapon_slot == endpoint.slot
                    && fighter.weapon == endpoint.weapon
                    && fighter.weapon_variant == endpoint.variant
                    && endpoint.forms.as_ref().is_none_or(|forms| {
                        forms.contains(&meteor_combat::combo_engine::weapon_visual_form(
                            fighter.weapon,
                            form.current,
                            pose.pose_id,
                        ))
                    }))
                .then(|| {
                    (
                        (endpoint.fighter, endpoint.name.clone()),
                        node_access
                            .p0()
                            .compute_global_transform(entity)
                            .unwrap_or(*transform)
                            .to_matrix(),
                    )
                })
            })
            .collect();
    let mut model_frames = std::collections::HashMap::<(Entity, String), bevy::math::Mat4>::new();
    for (entity, bone, transform) in &bones {
        model_frames.insert(
            (bone.model, bone.name.to_ascii_lowercase()),
            node_access
                .p0()
                .compute_global_transform(entity)
                .unwrap_or(*transform)
                .to_matrix(),
        );
    }
    for (entity, dummy, transform) in &dummies {
        model_frames.insert(
            (dummy.model, dummy.name.to_ascii_lowercase()),
            node_access
                .p0()
                .compute_global_transform(entity)
                .unwrap_or(*transform)
                .to_matrix(),
        );
    }
    for (
        entity,
        mut player,
        root_global,
        parent,
        worn_visual,
        scene_anchor,
        start_time,
        clock_advanced,
    ) in &mut players
    {
        let root_global = node_access
            .p0()
            .compute_global_transform(entity)
            .unwrap_or(*root_global);
        let endpoint_owner = parent.and_then(|parent| {
            fighters
                .get(parent.parent())
                .ok()
                .map(|_| parent.parent())
                .or_else(|| {
                    fighter_models
                        .get(parent.parent())
                        .ok()
                        .map(|(_, model, _)| model.fighter)
                })
        });
        let model_owner = parent.and_then(|parent| {
            fighter_models
                .get(parent.parent())
                .ok()
                .map(|(entity, _, transform)| {
                    (
                        entity,
                        node_access
                            .p0()
                            .compute_global_transform(entity)
                            .unwrap_or(*transform)
                            .to_matrix(),
                    )
                })
                .or_else(|| {
                    fighters.get(parent.parent()).ok().and_then(|_| {
                        fighter_models
                            .iter()
                            .find_map(|(entity, model, transform)| {
                                (model.fighter == parent.parent()).then(|| {
                                    (
                                        entity,
                                        node_access
                                            .p0()
                                            .compute_global_transform(entity)
                                            .unwrap_or(*transform)
                                            .to_matrix(),
                                    )
                                })
                            })
                    })
                })
        });
        if let Some(start_time) = start_time {
            player.elapsed = start_time.0;
            commands.entity(entity).remove::<PonEfStartTime>();
        } else if !clock_advanced.as_deref().is_some_and(|clock| clock.0) {
            player.elapsed += dt;
        }
        if let Some(mut clock) = clock_advanced {
            clock.0 = false;
        }
        let (time_secs, worn_model_hold) = pon_ef_playback_time(&player, worn_visual.is_some());

        let mut triggers: Vec<(usize, meteor_asset::pon_ef::PonEfTail)> = Vec::new();
        let mut clears: Vec<usize> = Vec::new();
        let ef = player.ef.clone();
        for (index, node) in ef.nodes.iter().enumerate() {
            if node.type_id != meteor_asset::pon_ef::TYPE_AUDIO {
                continue;
            }
            let inside = !worn_model_hold
                && time_secs > node.start_time - 1e-4
                && time_secs < node.end_time - 1e-4;
            match update_audio_window_state(&mut player.audio_triggered[index], inside) {
                AudioWindowAction::Start => triggers.push((index, node.tail.clone())),
                AudioWindowAction::Stop => clears.push(index),
                AudioWindowAction::None => {}
            }
        }
        for index in clears {
            if let Some(entity) = player.audio_playing[index].take() {
                if let Ok(mut entity_commands) = commands.get_entity(entity) {
                    entity_commands.despawn();
                }
            }
        }
        for (index, tail) in triggers {
            if let meteor_asset::pon_ef::PonEfTail::Audio { sound, value } = &tail {
                let entity = if *value != 0 {
                    let audio = crate::plugins::audio_plugin::play_sfx_loop_at(
                        &mut commands,
                        &mut sfx,
                        &mut audio_assets,
                        &roots,
                        sound,
                        root_global.translation(),
                    );
                    if let Some(audio) = audio {
                        commands
                            .entity(audio)
                            .insert(ChildOf(entity))
                            .insert(Transform::IDENTITY);
                    }
                    audio
                } else {
                    let audio = crate::plugins::audio_plugin::play_sfx_windowed_at(
                        &mut commands,
                        &mut sfx,
                        &mut audio_assets,
                        &roots,
                        sound,
                        1.0,
                        root_global.translation(),
                    );
                    if let Some(audio) = audio {
                        commands
                            .entity(audio)
                            .insert(ChildOf(entity))
                            .insert(Transform::IDENTITY);
                    }

                    audio
                };
                player.audio_playing[index] = entity;
            }
        }

        let mut memo = std::collections::HashMap::new();
        let mut visiting = std::collections::HashSet::new();
        let root_inverse = root_global.to_matrix().inverse();
        let mut external = |name: &str| -> Option<meteor_core::Mat4> {
            let lower = name.to_lowercase();
            if lower == "character" || lower == "channel" || lower == "root" {
                model_owner
                    .map(|(_, world)| external_frame_in_root(root_inverse, world))
                    .or_else(|| scene_anchor.map(|_| scene_effect_source_frame()))
                    .or_else(|| Some(meteor_core::Mat4::identity()))
            } else if lower.starts_with("d_wpn") {
                endpoint_owner
                    .and_then(|owner| endpoint_matrices.get(&(owner, lower.clone())))
                    .map(|world| external_frame_in_root(root_inverse, *world))
                    .or_else(|| {
                        model_owner
                            .and_then(|(model, _)| model_frames.get(&(model, lower)))
                            .map(|world| external_frame_in_root(root_inverse, *world))
                    })
            } else {
                model_owner
                    .and_then(|(model, _)| model_frames.get(&(model, lower)))
                    .map(|world| external_frame_in_root(root_inverse, *world))
            }
        };

        for (index, audio) in player.audio_playing.iter().enumerate() {
            let Some(audio) = audio else {
                continue;
            };
            if let Some(world) = meteor_asset::pon_ef::node_world_matrix(
                &player.ef,
                index,
                time_secs,
                &mut memo,
                &mut visiting,
                &mut external,
            ) {
                let matrix = bevy::math::Mat4::from_cols_array(&world.m);
                if let Ok(mut entity_commands) = commands.get_entity(*audio) {
                    entity_commands.insert(Transform::from_translation(matrix.w_axis.truncate()));
                }
            }
        }
        let mut nodes = node_access.p1();
        let mut owned_nodes = nodes.iter_many_mut(node_index.get(entity));
        while let Some((_, node_index, _, mut node_transform, material_handle, visibility)) =
            owned_nodes.fetch_next()
        {
            let Some(node) = player.ef.nodes.get(node_index.0) else {
                continue;
            };
            let model_hold = worn_model_hold && node.type_id == meteor_asset::pon_ef::TYPE_MODEL;
            let active = if worn_model_hold {
                model_hold
            } else {
                time_secs >= node.start_time - 1e-4 && time_secs <= node.end_time + 1e-4
            };
            if !active {
                if let Some(mut visibility) = visibility {
                    visibility.set_if_neq(Visibility::Hidden);
                }
                continue;
            }
            let sample_time = if model_hold { node.end_time } else { time_secs };
            let Some(world) = meteor_asset::pon_ef::node_world_matrix(
                &player.ef,
                node_index.0,
                sample_time,
                &mut memo,
                &mut visiting,
                &mut external,
            ) else {
                if let Some(mut visibility) = visibility {
                    visibility.set_if_neq(Visibility::Hidden);
                }
                continue;
            };
            let bevy_mat = bevy::math::Mat4::from_cols_array(&world.m);
            *node_transform = Transform::from_matrix(bevy_mat);
            if node.type_id == meteor_asset::pon_ef::TYPE_BILLBOARD {
                if let Some(camera_rotation) = current_camera_rotation {
                    node_transform.rotation =
                        camera_facing_local_rotation(&root_global, camera_rotation);
                }
            }

            let sample = meteor_asset::pon_ef::sample_transform(&node.keyframes, sample_time);
            let color = [
                sample.render[0].max(0.0),
                sample.render[1].max(0.0),
                sample.render[2].max(0.0),
            ];
            let opacity = sample.render[13].clamp(0.0, 1.0);
            if let Some(material_handle) = material_handle {
                dynamic_uniforms::set_color(
                    &mut materials,
                    &material_handle.0,
                    Vec4::new(color[0], color[1], color[2], opacity),
                );
            }
            if let Some(mut visibility) = visibility {
                let degenerate = node_transform.scale.abs().max_element() <= 1e-4;
                let state = meteor_asset::pon_ef::decode_render_state(&node.transform_header);

                visibility.set_if_neq(if degenerate || (state.blend_enabled && opacity <= 0.01) {
                    Visibility::Hidden
                } else {
                    Visibility::Visible
                });
            }
        }
        if !player.looping && player.elapsed >= player.duration {
            finished.push(entity);
        }
    }
    for entity in finished {
        commands.entity(entity).despawn();
    }
}

fn tick_pon_ef_drags(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut ribbons: Query<(&mut PonEfDragRibbon, &mut Visibility)>,
    nodes: Query<(&PonEfNodeEntity, &PonEfNodeOwner, &Transform)>,
    roots: Query<(&PonEfInstance, &GlobalTransform)>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    if dt <= 0.0 {
        return;
    }
    let mut anchors = std::collections::HashMap::<(Entity, usize), Vec3>::new();
    for (node, owner, transform) in &nodes {
        anchors.insert((owner.0, node.0), transform.translation);
    }
    for (mut ribbon, mut visibility) in &mut ribbons {
        let Some((player, root_transform)) = roots.get(ribbon.owner).ok() else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        let Some(a) = ribbon.anchor_indices[0]
            .and_then(|index| anchors.get(&(ribbon.owner, index)).copied())
            .map(|point| root_transform.transform_point(point))
        else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        let Some(b) = ribbon.anchor_indices[1]
            .and_then(|index| anchors.get(&(ribbon.owner, index)).copied())
            .map(|point| root_transform.transform_point(point))
        else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        let now = player.elapsed;
        if ribbon
            .history
            .last()
            .is_some_and(|sample| (sample.time - now).abs() < f32::EPSILON)
        {
            continue;
        }
        ribbon.history.push(PonEfDragSample {
            time: now,
            base: a,
            tip: b,
        });
        let cutoff = now - PON_EF_DRAG_HISTORY;
        ribbon.history.retain(|sample| sample.time >= cutoff);
        let (positions, colors, indices) = build_pon_ef_drag(&ribbon.history, now, ribbon.lookback);
        if let Some(mut mesh) = meshes.get_mut(&ribbon.mesh) {
            if !indices.is_empty() {
                mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
                mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; colors.len()]);
                mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; colors.len()]);
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
                mesh.insert_indices(Indices::U32(indices));
            }
        }
        *visibility = if ribbon.history.len() >= 2 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

fn tick_ifl_animations(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut materials: ResMut<Assets<PonEfMaterial>>,
    mut query: Query<&mut IflAnimated>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    for mut anim in &mut query {
        anim.age += dt;
        if anim.age < 1.0 / 30.0 {
            continue;
        }
        anim.age = 0.0;
        anim.index = (anim.index + 1) % anim.frames.len();
        let next = &anim.frames[anim.index];
        if materials.get(&anim.material).is_some_and(|material| {
            material.texture.as_ref() != Some(next) || material.params.has_texture != 1
        }) {
            if let Some(mut material) = materials.get_mut(&anim.material) {
                material.texture = Some(next.clone());
                material.params.has_texture = 1;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn tick_pon_ef_particles(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut materials: ResMut<Assets<PonEfMaterial>>,
    mut groups: Query<(
        Entity,
        &PonEfNodeEntity,
        &PonEfNodeOwner,
        &mut PonEfParticleGroup,
    )>,
    mut particle_access: ParamSet<(
        bevy::transform::helper::TransformHelper,
        Query<(Entity, &mut PonEfParticle, &mut Transform, &mut Visibility)>,
    )>,
    players: PonEfParticlePlayerQuery,
    endpoints: Query<(
        Entity,
        &crate::plugins::character_plugin::WeaponEndpoint,
        &GlobalTransform,
    )>,
    fighters: Query<(
        &crate::components::fighter::Fighter,
        &crate::components::fighter::WeaponForm,
        &crate::components::fighter::PoseRuntime,
    )>,
    fighter_models: Query<(
        Entity,
        &crate::plugins::character_plugin::FighterModel,
        &GlobalTransform,
    )>,
    bones: Query<(
        Entity,
        &crate::plugins::character_plugin::FighterBone,
        &GlobalTransform,
    )>,
    dummies: Query<(
        Entity,
        &crate::plugins::character_plugin::FighterDummy,
        &GlobalTransform,
    )>,
    cameras: Query<Entity, With<crate::plugins::camera_plugin::GameplayCamera>>,
    mut particle_index: Local<EffectEntityIndex>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(17);
    if groups.is_empty() {
        return;
    }

    let root_frames: std::collections::HashMap<_, _> = groups
        .iter()
        .map(|(_, _, owner, _)| owner.0)
        .chain(endpoints.iter().map(|(entity, ..)| entity))
        .chain(fighter_models.iter().map(|(entity, ..)| entity))
        .chain(bones.iter().map(|(entity, ..)| entity))
        .chain(dummies.iter().map(|(entity, ..)| entity))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .filter_map(|entity| {
            particle_access
                .p0()
                .compute_global_transform(entity)
                .ok()
                .map(|transform| (entity, transform))
        })
        .collect();
    let camera_rotation = cameras.iter().next().and_then(|camera| {
        particle_access
            .p0()
            .compute_global_transform(camera)
            .ok()
            .map(|frame| frame.to_scale_rotation_translation().1)
    });
    let mut particles = particle_access.p1();
    particle_index.rebuild(
        particles
            .iter()
            .map(|(entity, particle, ..)| (particle.group, entity)),
    );
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let endpoint_matrices: std::collections::HashMap<(Entity, String), bevy::math::Mat4> =
        endpoints
            .iter()
            .filter_map(|(entity, endpoint, transform)| {
                let transform = root_frames.get(&entity).unwrap_or(transform);
                let (fighter, form, pose) = fighters.get(endpoint.fighter).ok()?;
                (fighter.weapon_slot == endpoint.slot
                    && fighter.weapon == endpoint.weapon
                    && fighter.weapon_variant == endpoint.variant
                    && endpoint.forms.as_ref().is_none_or(|forms| {
                        forms.contains(&meteor_combat::combo_engine::weapon_visual_form(
                            fighter.weapon,
                            form.current,
                            pose.pose_id,
                        ))
                    }))
                .then(|| {
                    (
                        (endpoint.fighter, endpoint.name.clone()),
                        transform.to_matrix(),
                    )
                })
            })
            .collect();
    let mut model_frames = std::collections::HashMap::<(Entity, String), bevy::math::Mat4>::new();
    for (entity, bone, transform) in &bones {
        let transform = root_frames.get(&entity).unwrap_or(transform);
        model_frames.insert(
            (bone.model, bone.name.to_ascii_lowercase()),
            transform.to_matrix(),
        );
    }
    for (entity, dummy, transform) in &dummies {
        let transform = root_frames.get(&entity).unwrap_or(transform);
        model_frames.insert(
            (dummy.model, dummy.name.to_ascii_lowercase()),
            transform.to_matrix(),
        );
    }
    for (group_entity, node_entity, owner, mut group) in &mut groups {
        let Ok((instance, root_global, parent, worn_visual, scene_anchor)) = players.get(owner.0)
        else {
            continue;
        };
        let root_global = root_frames.get(&owner.0).unwrap_or(root_global);
        let (time_secs, worn_model_hold) = pon_ef_playback_time(instance, worn_visual.is_some());
        let ef = &instance.ef;
        let Some(node) = ef.nodes.get(node_entity.0) else {
            continue;
        };
        let active = !worn_model_hold
            && time_secs >= node.start_time - 1e-4
            && time_secs <= node.end_time + 1e-4;
        if !active {
            let mut owned_particles = particles.iter_many_mut(particle_index.get(group_entity));
            while let Some((_, mut particle, _, mut visibility)) = owned_particles.fetch_next() {
                particle.alive = false;
                particle.spent = false;
                visibility.set_if_neq(Visibility::Hidden);
            }
            group.started = false;
            continue;
        }
        let endpoint_owner = parent.and_then(|parent| {
            fighters
                .get(parent.parent())
                .ok()
                .map(|_| parent.parent())
                .or_else(|| {
                    fighter_models
                        .get(parent.parent())
                        .ok()
                        .map(|(_, model, _)| model.fighter)
                })
        });
        let model_owner = parent.and_then(|parent| {
            fighter_models
                .get(parent.parent())
                .ok()
                .map(|(entity, _, transform)| {
                    (
                        entity,
                        root_frames.get(&entity).unwrap_or(transform).to_matrix(),
                    )
                })
                .or_else(|| {
                    fighters.get(parent.parent()).ok().and_then(|_| {
                        fighter_models
                            .iter()
                            .find_map(|(entity, model, transform)| {
                                (model.fighter == parent.parent()).then_some((
                                    entity,
                                    root_frames.get(&entity).unwrap_or(transform).to_matrix(),
                                ))
                            })
                    })
                })
        });

        let root_scale = root_global
            .to_scale_rotation_translation()
            .0
            .abs()
            .max_element()
            .max(1e-4);

        let mut memo = std::collections::HashMap::new();
        let mut visiting = std::collections::HashSet::new();
        let root_inverse = root_global.to_matrix().inverse();
        let mut external = |name: &str| -> Option<meteor_core::Mat4> {
            let lower = name.to_lowercase();
            if lower == "character" || lower == "channel" || lower == "root" {
                model_owner
                    .map(|(_, world)| external_frame_in_root(root_inverse, world))
                    .or_else(|| scene_anchor.map(|_| scene_effect_source_frame()))
                    .or_else(|| Some(meteor_core::Mat4::identity()))
            } else if lower.starts_with("d_wpn") {
                endpoint_owner
                    .and_then(|owner| endpoint_matrices.get(&(owner, lower.clone())))
                    .map(|world| external_frame_in_root(root_inverse, *world))
                    .or_else(|| {
                        model_owner
                            .and_then(|(model, _)| model_frames.get(&(model, lower)))
                            .map(|world| external_frame_in_root(root_inverse, *world))
                    })
            } else {
                model_owner
                    .and_then(|(model, _)| model_frames.get(&(model, lower)))
                    .map(|world| external_frame_in_root(root_inverse, *world))
            }
        };
        let matrix = meteor_asset::pon_ef::node_world_matrix(
            ef,
            node_entity.0,
            time_secs,
            &mut memo,
            &mut visiting,
            &mut external,
        );

        let sample = meteor_asset::pon_ef::sample_transform(&node.keyframes, time_secs);
        let style_color = [
            sample.render[0].max(0.0),
            sample.render[1].max(0.0),
            sample.render[2].max(0.0),
        ];
        let style_opacity = sample.render[13].clamp(0.0, 1.0);

        let group_pos = matrix
            .map(|m| bevy::math::Vec3::new(m.m[12], m.m[13], m.m[14]))
            .unwrap_or_default();
        let mut anchor =
            |ef: &meteor_asset::pon_ef::PonEf,
             name: &str,
             memo: &mut std::collections::HashMap<usize, Option<meteor_core::Mat4>>,
             visiting: &mut std::collections::HashSet<usize>|
             -> bevy::math::Vec3 {
                if name.is_empty() {
                    return group_pos;
                }
                if let Some(index) = ef.nodes.iter().position(|n| n.name == name).or_else(|| {
                    ef.nodes
                        .iter()
                        .position(|n| n.name.eq_ignore_ascii_case(name))
                }) {
                    if let Some(m) = meteor_asset::pon_ef::node_world_matrix(
                        ef,
                        index,
                        time_secs,
                        memo,
                        visiting,
                        &mut external,
                    ) {
                        return bevy::math::Vec3::new(m.m[12], m.m[13], m.m[14]);
                    }
                }
                if let Some(m) = external(name) {
                    return bevy::math::Vec3::new(m.m[12], m.m[13], m.m[14]);
                }
                group_pos
            };
        let anchor_a = anchor(ef, &group.anchor_a, &mut memo, &mut visiting);
        let anchor_b = if group.anchor_b.is_empty() {
            anchor_a
        } else {
            anchor(ef, &group.anchor_b, &mut memo, &mut visiting)
        };

        let group_rot = matrix
            .map(|m| {
                let bevy_mat = bevy::math::Mat4::from_cols_array(&m.m);
                let (_, rotation, _) = bevy_mat.to_scale_rotation_translation();
                rotation
            })
            .unwrap_or_default();
        let spawn_frame = PonEfParticleSpawnFrame {
            group_pos,
            group_rot,
            anchor_a,
            anchor_b,
            root_world: root_global.to_matrix(),
        };
        let mut rng = Lcg(group.rng_state);

        if !group.started {
            group.started = true;
            let mut owned_particles = particles.iter_many_mut(particle_index.get(group_entity));
            while let Some((_, mut p, _, _)) = owned_particles.fetch_next() {
                spawn_particle(&mut p, &group.params, spawn_frame, &mut rng);
            }
        }
        let billboard_rotation =
            camera_rotation.map(|rotation| camera_facing_local_rotation(root_global, rotation));
        let mut owned_particles = particles.iter_many_mut(particle_index.get(group_entity));
        while let Some((_, mut p, mut transform, mut visibility)) = owned_particles.fetch_next() {
            if !p.alive {
                if p.spent || group.params.one_shot {
                    visibility.set_if_neq(Visibility::Hidden);
                    continue;
                }

                spawn_particle(&mut p, &group.params, spawn_frame, &mut rng);
            }
            p.age += dt;
            if p.age >= p.life {
                p.alive = false;
                if group.params.one_shot {
                    p.spent = true;
                }
                visibility.set_if_neq(Visibility::Hidden);
                continue;
            }
            let fade = 1.0 - p.age / p.life;
            let speed = if group.params.speed_decay {
                p.speed * fade
            } else {
                p.speed
            };
            let delta = p.dir * speed * dt;
            p.offset += delta;
            transform.translation = root_inverse.transform_point3(p.spawn_position + p.offset);
            transform.scale = Vec3::splat(p.size / root_scale);
            if let Some(rotation) = billboard_rotation {
                transform.rotation = rotation;
            }
            visibility.set_if_neq(Visibility::Visible);
            dynamic_uniforms::set_particle_color(
                &mut materials,
                &p,
                Vec4::new(
                    style_color[0],
                    style_color[1],
                    style_color[2],
                    style_opacity * fade,
                ),
            );
        }
        group.rng_state = rng.0;
    }
}

#[derive(Clone, Copy)]
struct PonEfParticleSpawnFrame {
    group_pos: bevy::math::Vec3,
    group_rot: bevy::math::Quat,
    anchor_a: bevy::math::Vec3,
    anchor_b: bevy::math::Vec3,
    root_world: bevy::math::Mat4,
}

fn spawn_particle(
    p: &mut PonEfParticle,
    params: &meteor_asset::pon_ef::PonEfParticleParams,
    frame: PonEfParticleSpawnFrame,
    rng: &mut Lcg,
) {
    p.age = 0.0;
    p.life = params.life_min + rng.next() * (params.life_max - params.life_min).max(0.0);
    let span = frame.anchor_b - frame.anchor_a;
    let spawn = frame.anchor_a + span * rng.next();
    p.spawn_offset = spawn - frame.group_pos;
    p.spawn_position = frame.root_world.transform_point3(spawn);
    p.offset = bevy::math::Vec3::ZERO;
    let local_dir = bevy::math::Vec3::new(
        params.dir_min[0] + rng.next() * (params.dir_max[0] - params.dir_min[0]),
        params.dir_min[1] + rng.next() * (params.dir_max[1] - params.dir_min[1]),
        params.dir_min[2] + rng.next() * (params.dir_max[2] - params.dir_min[2]),
    );
    p.speed = (params.speed_min + rng.next() * (params.speed_max - params.speed_min).max(0.0))
        * meteor_asset::pon_ef::PON_EF_SCALE;
    p.size = (params.size_min + rng.next() * (params.size_max - params.size_min).max(0.0))
        * meteor_asset::pon_ef::PON_EF_SCALE;
    p.dir = frame.root_world.to_scale_rotation_translation().1 * frame.group_rot * local_dir;
    p.alive = true;
    p.spent = false;
}

struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

fn cmodel_stem(model: &str) -> Option<String> {
    let stem = model
        .rsplit(['/', '\\'])
        .next()?
        .trim_end_matches(".des")
        .trim_end_matches(".DES");
    if stem.is_empty() {
        None
    } else {
        Some(stem.to_string())
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_pon_ef_model(
    commands: &mut Commands,
    node: &meteor_asset::pon_ef::PonEfNode,
    ctx: &mut EfSpawnContext<'_>,
    root: Entity,
) -> Option<Entity> {
    let meteor_asset::pon_ef::PonEfTail::Model { model } = &node.tail else {
        return None;
    };

    let stem = cmodel_stem(model)?;
    let root_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let dirs = resource_roots(&root_dir)
        .into_iter()
        .map(|root| root.join("cmodel"))
        .collect::<Vec<_>>();
    let read = |path: &str| {
        meteor_asset::runtime_assets::read_legacy_path(&format!("cmodel/{path}")).or_else(|| {
            dirs.iter()
                .find_map(|d| crate::resource_io::read(d.join(path)).ok())
        })
    };

    let bytes = read(&format!("{stem}.gmb")).or_else(|| read(&format!("{stem}.GMB")))?;
    let gmdl = meteor_asset::gmdl::decode_gmdl(&bytes).ok()?;
    let node_entity = commands
        .spawn((
            PonEfNodeEntity(node.index),
            PonEfNodeOwner(root),
            PonEfModelNode,
            Transform::IDENTITY,
        ))
        .insert(ChildOf(root))
        .id();
    let mut spawned = 0usize;
    for object in &gmdl.objects {
        if object.indices.is_empty() || object.positions.is_empty() {
            continue;
        }
        let mut groups: std::collections::HashMap<i32, Vec<u32>> = std::collections::HashMap::new();
        for (face, mat_idx) in object.face_mat.iter().enumerate() {
            if *mat_idx < 0 || *mat_idx as usize >= gmdl.materials.len() {
                continue;
            }
            groups
                .entry(*mat_idx)
                .or_default()
                .extend_from_slice(&object.indices[face * 3..face * 3 + 3]);
        }
        for (mat_idx, indices) in groups {
            let mat = &gmdl.materials[mat_idx as usize];
            let (mode, opacity, alpha_cutoff) =
                crate::plugins::render_plugin::classify_gmdl_material(mat);

            let texture = gmdl
                .resolve_texture_name(mat)
                .and_then(|name| ctx.textures.model_texture_handle(ctx.images, name, mat));
            let material = ctx.gmdl_materials.add(GmdlMaterial::new(
                GmdlMaterialParams {
                    opacity,
                    alpha_cutoff,
                    mode: mode.as_u32(),
                    has_texture: texture.is_some() as u32,

                    uv_offset: Vec2::ZERO,
                },
                texture,
                mat.two_side,
                mode,
            ));
            let mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_POSITION,
                object
                    .positions
                    .chunks_exact(3)
                    .map(|v| [v[0], v[1], v[2]])
                    .collect::<Vec<_>>(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_NORMAL,
                object
                    .normals
                    .chunks_exact(3)
                    .map(|v| [v[0], v[1], v[2]])
                    .collect::<Vec<_>>(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_UV_0,
                object
                    .uvs
                    .chunks_exact(2)
                    .map(|v| [v[0], v[1]])
                    .collect::<Vec<_>>(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_COLOR,
                object
                    .colors
                    .chunks_exact(4)
                    .map(|color| {
                        [
                            f32::from(color[0]) / 255.0,
                            f32::from(color[1]) / 255.0,
                            f32::from(color[2]) / 255.0,
                            f32::from(color[3]) / 255.0,
                        ]
                    })
                    .collect::<Vec<_>>(),
            )
            .with_inserted_indices(Indices::U32(indices));
            commands
                .spawn((
                    Mesh3d(ctx.meshes.add(mesh)),
                    MeshMaterial3d(material),
                    Transform::IDENTITY,
                ))
                .insert(ChildOf(node_entity));
            spawned += 1;
        }
    }
    if spawned == 0 {
        commands.entity(node_entity).despawn();
        return None;
    }
    Some(node_entity)
}

type EffectCleanupEntities<'w, 's> =
    Query<'w, 's, Entity, Or<(With<PonEfInstance>, With<PickupEffectRequest>)>>;

fn despawn_pon_ef(mut commands: Commands, query: EffectCleanupEntities<'_, '_>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

#[allow(clippy::too_many_arguments)]
fn combat_hit_effects(
    mut commands: Commands,
    mut events: MessageReader<crate::plugins::impact_fx_plugin::CombatImpactEvent>,
    fighters: Query<(&Fighter, &PoseRuntime, &Transform)>,
    mut cache: ResMut<EfCache>,
    shapes: Option<Res<PonEfShapeMeshes>>,
    assets: Option<Res<EffectAssets>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<PonEfMaterial>>,
    mut textures: ResMut<EfTextureCache>,
    mut images: ResMut<Assets<Image>>,
    mut gmdl_materials: ResMut<Assets<GmdlMaterial>>,
    mut sfx: ResMut<SfxCache>,
    mut audio_assets: ResMut<Assets<AudioSource>>,
    mut variant_seed: Local<u32>,
) {
    let roots =
        resource_roots(&std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
    for event in events.read() {
        let Ok((_, pose, transform)) = fighters.get(event.defender) else {
            continue;
        };
        let strategy = event.effect_strategy.unwrap_or_else(|| {
            fighters.get(event.attacker).map_or(0, |(fighter, _, _)| {
                meteor_combat::weapon_strategy::weapon_to_strategy(fighter.weapon)
            })
        });
        let guard = event.kind == crate::plugins::impact_fx_plugin::CombatImpactKind::Guard;
        let kind = if guard { "GD" } else { "BL" };
        let pose_id = event.reaction_pose.unwrap_or(pose.pose_id);
        let name = format!("W{strategy:02}{kind}{pose_id:03}");
        if cache.load(&name).is_some() {
            if let (Some(shapes), Some(assets)) = (shapes.as_deref(), assets.as_deref()) {
                let mut ctx = EfSpawnContext {
                    shapes,
                    assets,
                    meshes: &mut meshes,
                    materials: &mut materials,
                    gmdl_materials: &mut gmdl_materials,
                    textures: &mut textures,
                    images: &mut images,
                };
                spawn_pon_ef(
                    &mut commands,
                    &mut cache,
                    &mut ctx,
                    &name,
                    transform.translation,
                    None,
                    false,
                );
            }
        } else {
            *variant_seed = variant_seed
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            let variant = (*variant_seed >> 8) % 3 + 1;
            let stem = if guard { "GRD" } else { "HIT" };
            crate::plugins::audio_plugin::play_sfx_at(
                &mut commands,
                &mut sfx,
                &mut audio_assets,
                &roots,
                &format!("W{strategy:02}{stem}0{variant}.wav"),
                1.0,
                transform.translation,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn scene_script_effects(
    mut commands: Commands,
    mut events: MessageReader<SceneEffectEvent>,
    mut cache: ResMut<EfCache>,
    shapes: Option<Res<PonEfShapeMeshes>>,
    assets: Option<Res<EffectAssets>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<PonEfMaterial>>,
    mut textures: ResMut<EfTextureCache>,
    mut images: ResMut<Assets<bevy::image::Image>>,
    mut gmdl_materials: ResMut<Assets<GmdlMaterial>>,
    pickups: Query<(Entity, &PickupEffectRequest)>,
) {
    let (Some(shapes), Some(assets)) = (shapes, assets) else {
        return;
    };
    for event in events
        .read()
        .chain(pickups.iter().map(|(_, request)| &request.0))
    {
        if event.effect.is_empty() {
            continue;
        }
        let mut ctx = EfSpawnContext {
            shapes: &shapes,
            assets: &assets,
            meshes: &mut meshes,
            materials: &mut materials,
            gmdl_materials: &mut gmdl_materials,
            textures: &mut textures,
            images: &mut images,
        };
        spawn_pon_ef(
            &mut commands,
            &mut cache,
            &mut ctx,
            &event.effect,
            event.position,
            None,
            false,
        );
    }
    for (entity, _) in &pickups {
        commands.entity(entity).despawn();
    }
}

fn requested_lock_marker(
    app_state: &crate::app_state::AppState,
    lock_flag: bool,
    lock: Option<Entity>,
    track: Option<Entity>,
) -> Option<(&'static str, Entity)> {
    if *app_state != crate::app_state::AppState::Playing || !lock_flag {
        return None;
    }
    lock.map(|target| ("Lock", target))
        .or_else(|| track.map(|target| ("Track", target)))
}

#[allow(clippy::too_many_arguments)]
fn lock_marker_effects(
    mut commands: Commands,
    app_state: Res<State<crate::app_state::AppState>>,
    lock: Res<crate::components::fighter::LockOnController>,
    settings: Res<crate::app_state::Settings>,
    mut cache: ResMut<EfCache>,
    shapes: Option<Res<PonEfShapeMeshes>>,
    assets: Option<Res<EffectAssets>>,
    mut materials: ResMut<Assets<PonEfMaterial>>,
    mut textures: ResMut<EfTextureCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut gmdl_materials: ResMut<Assets<GmdlMaterial>>,
    markers: Query<(Entity, &LockMarkerFx)>,
) {
    let requested = requested_lock_marker(
        app_state.get(),
        settings.lock_flag,
        lock.lock_entity,
        lock.track_entity,
    );

    for (entity, marker) in &markers {
        let mismatch =
            requested.is_none_or(|(kind, target)| marker.kind != kind || marker.target != target);
        if mismatch {
            commands.entity(entity).despawn();
        }
    }
    let Some((kind, target_entity)) = requested else {
        return;
    };
    let (Some(shapes), Some(assets)) = (shapes, assets) else {
        return;
    };
    let exists = markers
        .iter()
        .any(|(_, marker)| marker.kind == kind && marker.target == target_entity);
    if exists {
        return;
    }
    let mut ctx = EfSpawnContext {
        shapes: &shapes,
        assets: &assets,
        meshes: &mut meshes,
        materials: &mut materials,
        gmdl_materials: &mut gmdl_materials,
        textures: &mut textures,
        images: &mut images,
    };
    if let Some(root) = spawn_pon_ef(
        &mut commands,
        &mut cache,
        &mut ctx,
        kind,
        Vec3::ZERO,
        Some(target_entity),
        true,
    ) {
        commands.entity(root).insert(LockMarkerFx {
            kind,
            target: target_entity,
        });
    }
}

fn pose_effect_spawn_ready(
    spawn: &mut PoseEffectSpawn,
    pose: &PoseRuntime,
    blend: &EntryBlend,
) -> bool {
    if spawn.pose_id != pose.pose_id || spawn.pose_instance_id != pose.pose_instance_id {
        spawn.pose_id = pose.pose_id;
        spawn.pose_instance_id = pose.pose_instance_id;
        spawn.spawned = false;
    }
    let blend_active = blend.duration > 0.0 && blend.pose_id == pose.pose_id;
    !spawn.spawned && !blend_active && pose.frame >= pose.start as f32 && !pose.finished()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn pose_effect_spawn(
    mut commands: Commands,
    assets: Option<Res<CharacterAssetsSet>>,
    mut cache: ResMut<EfCache>,
    shapes: Option<Res<PonEfShapeMeshes>>,
    effect_assets: Option<Res<EffectAssets>>,
    mut materials: ResMut<Assets<PonEfMaterial>>,
    mut textures: ResMut<EfTextureCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut gmdl_materials: ResMut<Assets<GmdlMaterial>>,
    models: Query<(Entity, &FighterModel)>,
    mut query: Query<(
        Entity,
        &Fighter,
        &PoseRuntime,
        &EntryBlend,
        &mut PoseEffectSpawn,
    )>,
) {
    let (Some(assets), Some(shapes), Some(effect_assets)) = (assets, shapes, effect_assets) else {
        return;
    };
    for (entity, fighter, pose, blend, mut spawn) in &mut query {
        if !pose_effect_spawn_ready(&mut spawn, pose, blend) {
            continue;
        }
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        let Some(pose_rec) = character_assets.poses.get(&pose.pose_id) else {
            continue;
        };
        let Some(name) = pose_rec.effect_id.as_ref().and_then(effect_file_name) else {
            continue;
        };
        if !models.iter().any(|(_, model)| model.fighter == entity) {
            continue;
        }
        spawn.spawned = true;
        let mut ctx = EfSpawnContext {
            shapes: &shapes,
            assets: &effect_assets,
            meshes: &mut meshes,
            materials: &mut materials,
            gmdl_materials: &mut gmdl_materials,
            textures: &mut textures,
            images: &mut images,
        };
        if let Some(effect) = spawn_pon_ef(
            &mut commands,
            &mut cache,
            &mut ctx,
            &name,
            Vec3::ZERO,
            Some(entity),
            false,
        ) {
            commands.entity(effect).insert(PonEfStartTime(
                meteor_anim::pose_controller::pose_elapsed_seconds_at_frame(pose_rec, pose.frame),
            ));
        }
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn spawn_test_animated_particles(
        mut commands: Commands,
        mut cache: ResMut<EfCache>,
        shapes: Res<PonEfShapeMeshes>,
        assets: Res<EffectAssets>,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<PonEfMaterial>>,
        mut gmdl_materials: ResMut<Assets<GmdlMaterial>>,
        mut textures: ResMut<EfTextureCache>,
        mut images: ResMut<Assets<Image>>,
    ) {
        let mut ctx = EfSpawnContext {
            shapes: &shapes,
            assets: &assets,
            meshes: &mut meshes,
            materials: &mut materials,
            gmdl_materials: &mut gmdl_materials,
            textures: &mut textures,
            images: &mut images,
        };
        for effect in ["XHammer", "XBell", "Pos474", "Item12"] {
            assert!(spawn_pon_ef(
                &mut commands,
                &mut cache,
                &mut ctx,
                effect,
                Vec3::ZERO,
                None,
                true
            )
            .is_some());
        }
    }

    #[test]
    fn original_particle_ifls_advance_once_per_material() {
        let mut app = feedback_test_app();
        app.add_systems(
            Startup,
            spawn_test_animated_particles.after(setup_effect_assets),
        );
        app.update();
        let mut expected = std::collections::HashSet::new();
        let mut particles = app
            .world_mut()
            .query::<(&PonEfParticle, &MeshMaterial3d<PonEfMaterial>)>();
        for (particle, material) in particles.iter(app.world()) {
            let group = app.world().get::<PonEfNodeEntity>(particle.group).unwrap();
            let owner = app.world().get::<PonEfNodeOwner>(particle.group).unwrap();
            let ef = app.world().get::<PonEfInstance>(owner.0).unwrap();
            if ef.ef.nodes[group.0]
                .texture
                .to_ascii_lowercase()
                .ends_with(".ifl")
            {
                expected.insert(material.0.id());
            }
        }
        assert!(expected.len() > 1);
        let mut animations = app.world_mut().query::<&IflAnimated>();
        let mut initial = Vec::new();
        for id in expected {
            let drivers: Vec<_> = animations
                .iter(app.world())
                .filter(|anim| anim.material.id() == id)
                .collect();
            assert_eq!(
                drivers.len(),
                1,
                "one IFL driver per particle page / ribbon"
            );
            let anim = drivers[0];
            initial.push((anim.material.clone(), anim.frames[1].clone()));
        }
        app.add_systems(Update, tick_ifl_animations);
        app.update();
        for (material, expected_frame) in initial {
            assert_eq!(
                app.world()
                    .resource::<Assets<PonEfMaterial>>()
                    .get(&material)
                    .unwrap()
                    .texture
                    .as_ref(),
                Some(&expected_frame)
            );
        }
    }

    fn feedback_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::transform::TransformPlugin)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_secs_f32(1.0 / 60.0),
            ))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<PonEfMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<Assets<AudioSource>>()
            .init_resource::<EfCache>()
            .init_resource::<EfTextureCache>()
            .init_resource::<SfxCache>()
            .add_message::<SceneEffectEvent>()
            .add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>()
            .add_systems(Startup, setup_effect_assets);
        crate::plugins::configure_fighter_frame_sets(&mut app);
        install_effect_playback_systems(&mut app);
        app
    }

    fn feedback_fighter(weapon: u8) -> Fighter {
        Fighter {
            character_id: 0,
            player_slot: 0,
            weapon,
            weapon_slots: [weapon, weapon],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        }
    }

    #[test]
    fn successful_pickup_plays_original_flash_burst_audio_and_expires() {
        use crate::components::fighter::{Health, Qi};
        use crate::plugins::item_plugin::{item_pickup, ItemBuffs, StageProp};
        use bevy::ecs::system::RunSystemOnce;
        for (model, expected, dynamic) in [
            ("item01", "GetItem", false),
            ("item0901", "WeaponUP", false),
            ("w0_0", "GetItem", true),
        ] {
            let mut app = feedback_test_app();
            app.init_resource::<ItemBuffs>()
                .init_resource::<crate::plugins::stage_plugin::StageScriptRuntime>();
            let position = Vec3::new(6.0, 0.0, 8.0);
            let mut fighter = feedback_fighter(5);
            fighter.weapon_slots = [5, 0];
            app.world_mut().spawn((
                fighter,
                crate::plugins::stage_plugin::LocalFighter,
                Health::default(),
                Qi::default(),
                Transform::from_translation(position),
            ));
            let prop = app
                .world_mut()
                .spawn((
                    StageProp {
                        name: "D_itPickupTest".into(),
                        model: model.into(),
                        center_offset: Vec3::ZERO,
                        half_extents: Vec3::ONE,
                        hit_count: 0,
                        respawn_timer: 0.0,
                        interactive: true,
                        collision_enabled: false,
                        breakable: false,
                        broken: false,
                    },
                    Transform::from_translation(position),
                    Visibility::Visible,
                ))
                .id();
            if dynamic {
                app.world_mut().entity_mut(prop).insert(
                    crate::plugins::render_plugin::DynamicWeaponDrop {
                        reference: "D_itPickupTest".into(),
                    },
                );
            }
            app.world_mut().run_system_once(item_pickup).unwrap();
            assert_eq!(
                app.world_mut()
                    .query::<&PickupEffectRequest>()
                    .iter(app.world())
                    .count(),
                1
            );
            if dynamic {
                assert!(app.world().get_entity(prop).is_err());
            }
            for _ in 0..8 {
                app.update();
            }
            let expected_ef = app
                .world_mut()
                .resource_mut::<EfCache>()
                .load(expected)
                .unwrap();
            let (root, instance, transform) = app
                .world_mut()
                .query::<(Entity, &PonEfInstance, &GlobalTransform)>()
                .single(app.world())
                .unwrap();
            assert!(std::sync::Arc::ptr_eq(&instance.ef, &expected_ef));
            assert!(!instance.looping);
            assert_eq!(transform.translation(), position);
            assert_eq!(
                app.world_mut()
                    .query::<&PickupEffectRequest>()
                    .iter(app.world())
                    .count(),
                0
            );
            let visible_meshes = app
                .world_mut()
                .query::<(&PonEfNodeOwner, &MeshMaterial3d<PonEfMaterial>, &Visibility)>()
                .iter(app.world())
                .filter(|(owner, _, visibility)| {
                    owner.0 == root && **visibility != Visibility::Hidden
                })
                .map(|(_, material, _)| material.0.clone())
                .collect::<Vec<_>>();
            assert!(
                visible_meshes.len() >= 2,
                "{expected} must draw its authored plane/cylinder layers"
            );
            for handle in visible_meshes {
                assert!(
                    app.world()
                        .resource::<Assets<PonEfMaterial>>()
                        .get(&handle)
                        .unwrap()
                        .texture
                        .is_some(),
                    "{expected} must resolve its original texture"
                );
            }
            assert!(
                app.world_mut()
                    .query::<&PonEfParticle>()
                    .iter(app.world())
                    .any(|particle| particle.alive),
                "{expected} must emit its particles"
            );
            assert_eq!(
                app.world_mut()
                    .query::<&bevy::audio::AudioPlayer>()
                    .iter(app.world())
                    .count(),
                if expected == "WeaponUP" { 3 } else { 1 },
                "play the authored audio without duplicate Get.wav"
            );
            for _ in 0..110 {
                app.update();
            }
            assert!(
                app.world().get_entity(root).is_err(),
                "{expected} must finish once and disappear"
            );
            assert_eq!(
                app.world_mut()
                    .query::<&PonEfInstance>()
                    .iter(app.world())
                    .count(),
                0
            );
        }
    }

    #[test]
    fn production_combo_effect_starts_at_authored_entry_after_blend() {
        let roots = resource_roots(&std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        for (pose_id, skipped_frames, expected_time) in [
            (253, 18, 0.4),
            (325, 109, 109.0 / 30.0),
            (416, 30, 2.0 / 3.0),
        ] {
            let character =
                crate::plugins::character_plugin::load_character_asset_for_test(&roots, "p0")
                    .expect("p0 original resources");
            let pose = character.poses[&pose_id].clone();
            assert_eq!(pose.action_entry_frame - pose.start, skipped_frames);
            let mut app = feedback_test_app();
            app.insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
                "p0".to_string(),
                character,
            )])));
            let fighter = app
                .world_mut()
                .spawn((
                    feedback_fighter(5),
                    PoseRuntime {
                        pose_id,
                        frame: pose.action_entry_frame as f32,
                        start: pose.start,
                        end: pose.end,
                        loop_start: pose.start,
                        loop_end: pose.end,
                        loops: false,
                        pose_instance_id: 1,
                    },
                    EntryBlend {
                        pose_id,
                        duration: 0.1,
                        ..default()
                    },
                ))
                .id();
            app.world_mut().spawn((
                FighterModel {
                    fighter,
                    character: "p0".into(),
                },
                Transform::IDENTITY,
            ));
            app.update();
            assert_eq!(
                app.world_mut()
                    .query::<&PonEfInstance>()
                    .iter(app.world())
                    .count(),
                0
            );
            *app.world_mut().get_mut::<EntryBlend>(fighter).unwrap() = EntryBlend::default();
            app.update();
            let player = app
                .world_mut()
                .query::<&PonEfInstance>()
                .single(app.world())
                .unwrap();
            assert!(
                (player.elapsed - expected_time).abs() < 1e-5,
                "pose {pose_id} must sample its entry clock on the spawn frame"
            );
            for (index, node) in player.ef.nodes.iter().enumerate() {
                if node.type_id == meteor_asset::pon_ef::TYPE_AUDIO {
                    let inside = expected_time >= node.start_time - 1e-4
                        && expected_time < node.end_time - 1e-4;
                    assert_eq!(player.audio_triggered[index], inside, "{}", node.name);
                    if inside {
                        assert!(
                            player.audio_playing[index].is_some(),
                            "{} must sound immediately",
                            node.name
                        );
                    }
                }
            }
            if pose_id == 253 {
                assert!(
                    app.world_mut()
                        .query::<(&PonEfNodeEntity, &Visibility)>()
                        .iter(app.world())
                        .any(|(node, visible)| node.0 == 7 && *visible == Visibility::Hidden),
                    "dagger combo enters at 0.4s, before AttackWave starts at 0.5067s"
                );
            }
            app.update();
            let player = app
                .world_mut()
                .query::<&PonEfInstance>()
                .single(app.world())
                .unwrap();
            assert!((player.elapsed - expected_time - 1.0 / 60.0).abs() < 1e-5);
        }
    }

    #[test]
    fn production_hit_feedback_uses_attacker_strategy_and_remote_reaction_override() {
        use crate::plugins::impact_fx_plugin::{CombatImpactEvent, CombatImpactKind};
        for (weapon, stock_context) in (1..=12).flat_map(|weapon| [(weapon, false), (weapon, true)])
        {
            for (kind, pose_id) in [
                (CombatImpactKind::Hit, 93),
                (CombatImpactKind::Kill, 117),
                (CombatImpactKind::Guard, 56),
            ] {
                let mut app = feedback_test_app();
                let attacker = app
                    .world_mut()
                    .spawn((
                        feedback_fighter(weapon),
                        PoseRuntime::default(),
                        Transform::IDENTITY,
                    ))
                    .id();
                let defender = app
                    .world_mut()
                    .spawn((
                        feedback_fighter(5),
                        PoseRuntime::default(),
                        Transform::IDENTITY,
                    ))
                    .id();
                let strategy = meteor_combat::weapon_strategy::weapon_to_strategy(weapon);
                let family = if kind == CombatImpactKind::Guard {
                    "GD"
                } else {
                    "BL"
                };
                let name = format!("W{strategy:02}{family}{pose_id:03}");
                let expected = app.world_mut().resource_mut::<EfCache>().load(&name);
                app.update();
                app.world_mut().write_message(CombatImpactEvent {
                    kind,
                    attacker: if stock_context { defender } else { attacker },
                    defender,
                    reaction_pose: Some(pose_id),
                    effect_strategy: stock_context.then_some(strategy),
                    contact: Vec3::ZERO,
                    direction: Vec3::Z,
                });
                app.update();
                let players: Vec<_> = app
                    .world_mut()
                    .query::<&PonEfInstance>()
                    .iter(app.world())
                    .collect();
                if let Some(expected) = expected {
                    if expected.nodes.is_empty() {
                        assert!(players.is_empty(), "{name}");
                        assert_eq!(
                            app.world_mut()
                                .query::<&bevy::audio::AudioPlayer>()
                                .iter(app.world())
                                .count(),
                            0,
                            "{name}"
                        );
                        continue;
                    }
                    assert_eq!(players.len(), 1, "{name}");
                    assert!(std::sync::Arc::ptr_eq(&players[0].ef, &expected), "{name}");
                    let expected_audio = players[0].audio_playing.iter().flatten().count();
                    assert_eq!(
                        app.world_mut()
                            .query::<&bevy::audio::AudioPlayer>()
                            .iter(app.world())
                            .count(),
                        expected_audio,
                        "authored feedback must not add a fixed hit WAV: {name}"
                    );
                } else {
                    assert!(players.is_empty());
                    assert_eq!(
                        app.world_mut()
                            .query::<&bevy::audio::AudioPlayer>()
                            .iter(app.world())
                            .count(),
                        1,
                        "missing {name} must fall back to its weapon's WAV"
                    );
                }
            }
        }
    }

    #[test]
    fn origin_sn22_des_supplies_all_teleport_curtain_anchors() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let path = root.join("sn22/sn22.des");
        if !path.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let effects = stage_ambient_effects(
            &std::fs::read_to_string(path).expect("sn22.des"),
            meteor_scene::stage_loader::ARENA_CENTER,
        );
        assert_eq!(effects.len(), 64);
        assert_eq!(
            effects
                .iter()
                .filter(|(effect, _)| effect == "POT_Free")
                .count(),
            32
        );
        assert_eq!(
            effects
                .iter()
                .filter(|(effect, _)| effect == "POT_Red")
                .count(),
            16
        );
        assert_eq!(
            effects
                .iter()
                .filter(|(effect, _)| effect == "POT_Blue")
                .count(),
            16
        );
        assert!(effects.iter().all(|(_, position)| position.is_finite()));
        let mut cache = EfCache::default();
        for effect in ["POT_Free", "POT_Red", "POT_Blue"] {
            assert!(
                cache.load(effect).is_some(),
                "{effect}.ef must be renderable through the production PonEF cache"
            );
        }
    }

    #[test]
    fn origin_sn22_curtain_uses_the_character_source_frame_in_world_space() {
        let mut cache = EfCache::default();
        let Some(effect) = cache.load("POT_Free") else {
            eprintln!("origin_res missing, skipping");
            return;
        };
        let cylinder = effect
            .nodes
            .iter()
            .position(|node| node.name == "Cylinder_4")
            .expect("POT_Free cylinder");
        let mut memo = std::collections::HashMap::new();
        let mut visiting = std::collections::HashSet::new();
        let mut external = |_name: &str| Some(scene_effect_source_frame());
        let matrix = meteor_asset::pon_ef::node_world_matrix(
            &effect,
            cylinder,
            1.0,
            &mut memo,
            &mut visiting,
            &mut external,
        )
        .expect("curtain matrix");
        let (scale, _, translation) =
            bevy::math::Mat4::from_cols_array(&matrix.m).to_scale_rotation_translation();
        assert!(
            scale.min_element() >= 0.119 && scale.max_element() < 0.2,
            "POT source geometry must be scaled once by 0.12, got {scale:?}"
        );
        assert!(
            translation.length() < 1.0,
            "authored local curtain offset must stay near its world anchor: {translation:?}"
        );
        let frame = bevy::math::Mat4::from_cols_array(&scene_effect_source_frame().m);
        let source_up = frame.transform_vector3(Vec3::Z);
        assert!((source_up - Vec3::Y * 0.12).length() < 1e-5);
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_test_pos256(
        mut commands: Commands,
        mut cache: ResMut<EfCache>,
        shapes: Res<PonEfShapeMeshes>,
        assets: Res<EffectAssets>,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<PonEfMaterial>>,
        mut gmdl_materials: ResMut<Assets<GmdlMaterial>>,
        mut textures: ResMut<EfTextureCache>,
        mut images: ResMut<Assets<Image>>,
    ) {
        let model = commands
            .spawn((
                crate::plugins::character_plugin::FighterModel {
                    fighter: Entity::PLACEHOLDER,
                    character: "p0".to_string(),
                },
                Transform::from_scale(Vec3::splat(meteor_asset::pon_ef::PON_EF_SCALE)),
            ))
            .id();
        let mut ctx = EfSpawnContext {
            shapes: &shapes,
            assets: &assets,
            meshes: &mut meshes,
            materials: &mut materials,
            gmdl_materials: &mut gmdl_materials,
            textures: &mut textures,
            images: &mut images,
        };
        let root = spawn_pon_ef(
            &mut commands,
            &mut cache,
            &mut ctx,
            "pos256",
            Vec3::ZERO,
            Some(model),
            false,
        );
        assert!(root.is_some(), "pos256.ef has visible nodes and must spawn");
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_test_pos253(
        mut commands: Commands,
        mut cache: ResMut<EfCache>,
        shapes: Res<PonEfShapeMeshes>,
        assets: Res<EffectAssets>,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<PonEfMaterial>>,
        mut gmdl_materials: ResMut<Assets<GmdlMaterial>>,
        mut textures: ResMut<EfTextureCache>,
        mut images: ResMut<Assets<Image>>,
    ) {
        let fighter = commands
            .spawn(Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 5,
                weapon_slots: [5, 5],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            })
            .insert(crate::components::fighter::PoseRuntime::idle())
            .id();
        let model = commands
            .spawn((
                crate::plugins::character_plugin::FighterModel {
                    fighter,
                    character: "p0".to_string(),
                },
                Transform::from_scale(Vec3::splat(meteor_asset::pon_ef::PON_EF_SCALE)),
            ))
            .id();
        commands
            .spawn((
                crate::plugins::character_plugin::FighterDummy {
                    model,
                    name: "d_base".to_string(),
                    dummy_index: 0,
                },
                Transform::from_xyz(-4.0, 0.0, 0.0),
            ))
            .insert(ChildOf(model));
        commands
            .spawn((
                crate::plugins::character_plugin::FighterDummy {
                    model,
                    name: "bad_R_Foot".to_string(),
                    dummy_index: 1,
                },
                Transform::from_xyz(2.0, 0.0, 0.0),
            ))
            .insert(ChildOf(model));
        commands
            .spawn((
                crate::plugins::character_plugin::WeaponEndpoint {
                    name: "d_wpnre".to_string(),
                    fighter,
                    weapon: 5,
                    slot: 0,
                    variant: 0,
                    part: "w1_0".to_string(),
                    left: false,
                    forms: None,
                },
                Transform::from_xyz(4.0, 0.0, 0.0),
            ))
            .insert(ChildOf(model));
        let mut ctx = EfSpawnContext {
            shapes: &shapes,
            assets: &assets,
            meshes: &mut meshes,
            materials: &mut materials,
            gmdl_materials: &mut gmdl_materials,
            textures: &mut textures,
            images: &mut images,
        };
        let root = spawn_pon_ef(
            &mut commands,
            &mut cache,
            &mut ctx,
            "pos253",
            Vec3::ZERO,
            Some(model),
            false,
        );
        assert!(root.is_some(), "pos253.ef has visible nodes and must spawn");
    }

    fn mesh_position_bounds(mesh: &Mesh) -> (Vec3, Vec3) {
        let bevy::mesh::VertexAttributeValues::Float32x3(positions) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("mesh positions")
        else {
            panic!("mesh positions must be Float32x3");
        };
        positions.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(min, max), position| {
                let point = Vec3::from_array(*position);
                (min.min(point), max.max(point))
            },
        )
    }

    fn mesh_ring_radius_at_z(mesh: &Mesh, z: f32) -> f32 {
        let bevy::mesh::VertexAttributeValues::Float32x3(positions) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("mesh positions")
        else {
            panic!("mesh positions must be Float32x3");
        };
        positions
            .iter()
            .filter(|position| (position[2] - z).abs() < 1e-3)
            .map(|position| Vec2::new(position[0], position[1]).length())
            .fold(0.0f32, f32::max)
    }

    #[test]
    fn effect_index_preserves_exhaustive_query_order_after_storage_and_owner_changes() {
        #[derive(Component)]
        struct OtherArchetype;
        let mut world = World::new();
        let owners: Vec<_> = (0..3).map(|_| world.spawn_empty().id()).collect();
        let entities: Vec<_> = (0..75)
            .map(|i| {
                world
                    .spawn((PonEfNodeEntity(i), PonEfNodeOwner(owners[i % 3])))
                    .id()
            })
            .collect();
        let mut query = world.query::<(Entity, &PonEfNodeOwner)>();
        let mut index = EffectEntityIndex::default();
        for frame in 0..4 {
            match frame {
                1 => {
                    for entity in entities.iter().step_by(4) {
                        world.entity_mut(*entity).insert(OtherArchetype);
                    }
                }
                2 => {
                    world.get_mut::<PonEfNodeOwner>(entities[1]).unwrap().0 = owners[0];
                    world.despawn(entities[5]);
                    world.entity_mut(entities[8]).remove::<OtherArchetype>();
                }
                3 => {
                    let retired: Vec<_> = query
                        .iter(&world)
                        .filter(|(_, owner)| owner.0 == owners[2])
                        .map(|(entity, _)| entity)
                        .collect();
                    for entity in retired {
                        world.despawn(entity);
                    }
                }
                _ => {}
            }
            index.rebuild(query.iter(&world).map(|(entity, owner)| (owner.0, entity)));
            for owner in &owners {
                let exhaustive: Vec<_> = query
                    .iter(&world)
                    .filter(|(_, parent)| parent.0 == *owner)
                    .map(|(entity, _)| entity)
                    .collect();
                assert_eq!(
                    index.get(*owner),
                    exhaustive,
                    "particle RNG must follow existing query order"
                );
            }
        }
        assert!(
            !index.0.contains_key(&owners[2]),
            "retired groups must release their scratch storage"
        );
        world.clear_entities();
        index.rebuild(query.iter(&world).map(|(entity, owner)| (owner.0, entity)));
        assert!(index.0.is_empty());
    }

    #[test]
    #[ignore = "real-resource crowd CPU benchmark; no device FPS assertion"]
    fn origin_effect_crowd_benchmark() {
        use bevy::ecs::system::RunSystemOnce;
        #[derive(Component)]
        struct MovedArchetype;
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::transform::TransformPlugin))
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_secs_f64(1.0 / 60.0),
            ))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<PonEfMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<Assets<AudioSource>>()
            .init_resource::<EfCache>()
            .init_resource::<EfTextureCache>()
            .init_resource::<SfxCache>()
            .add_systems(Startup, setup_effect_assets)
            .add_systems(Update, (tick_pon_ef_players, tick_pon_ef_particles).chain());
        app.update();
        for _ in 0..15 {
            app.world_mut().run_system_once(spawn_test_pos253).unwrap();
            app.world_mut().run_system_once(spawn_test_pos256).unwrap();
        }
        for mut player in app
            .world_mut()
            .query::<&mut PonEfInstance>()
            .iter_mut(app.world_mut())
        {
            player.looping = true;
        }
        app.world_mut().spawn((
            crate::plugins::camera_plugin::GameplayCamera,
            Transform::from_rotation(Quat::from_rotation_y(0.7)),
        ));
        let particles: Vec<_> = app
            .world_mut()
            .query_filtered::<Entity, With<PonEfParticle>>()
            .iter(app.world())
            .collect();
        let mut digests = Vec::new();
        let mut samples = Vec::new();
        for frame in 0..180 {
            if frame == 30 || frame == 60 {
                for entity in particles.iter().step_by(11) {
                    if frame == 30 {
                        app.world_mut().entity_mut(*entity).insert(MovedArchetype);
                    } else {
                        app.world_mut()
                            .entity_mut(*entity)
                            .remove::<MovedArchetype>();
                    }
                }
            }
            if frame == 90 {
                app.world_mut().despawn(particles[0]);
            }
            if frame == 120 {
                let root = app
                    .world_mut()
                    .query_filtered::<Entity, With<PonEfInstance>>()
                    .iter(app.world())
                    .next()
                    .unwrap();
                app.world_mut().despawn(root);
            }
            let start = std::time::Instant::now();
            app.update();
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
            let mut rows: Vec<(u64, Vec<u32>)> = Vec::new();
            let materials = app.world().resource::<Assets<PonEfMaterial>>();
            for entity in &particles {
                let Some(p) = app.world().get::<PonEfParticle>(*entity) else {
                    continue;
                };
                let t = app.world().get::<Transform>(*entity).unwrap();
                let v = app.world().get::<Visibility>(*entity).unwrap();
                let mut bits: Vec<_> = [p.age, p.life, p.size, p.speed]
                    .into_iter()
                    .chain(p.dir.to_array())
                    .chain(p.spawn_offset.to_array())
                    .chain(p.offset.to_array())
                    .chain(t.to_matrix().to_cols_array())
                    .chain(
                        materials
                            .get(&p.material)
                            .unwrap()
                            .particle_colors
                            .as_ref()
                            .unwrap()[p.color_slot as usize]
                            .to_array(),
                    )
                    .map(f32::to_bits)
                    .collect();
                bits.extend([
                    u32::from(p.alive),
                    u32::from(p.spent),
                    u32::from(*v == Visibility::Visible),
                ]);
                rows.push((entity.to_bits(), bits));
            }
            for (entity, transform) in app
                .world_mut()
                .query_filtered::<(Entity, &Transform), With<PonEfNodeEntity>>()
                .iter(app.world())
            {
                rows.push((
                    entity.to_bits(),
                    transform
                        .to_matrix()
                        .to_cols_array()
                        .into_iter()
                        .map(f32::to_bits)
                        .collect(),
                ));
            }
            rows.sort_by_key(|(entity, _)| *entity);
            let mut digest = 0xcbf29ce484222325u64;
            for (entity, bits) in rows {
                for byte in entity
                    .to_le_bytes()
                    .into_iter()
                    .chain(bits.iter().flat_map(|b| b.to_le_bytes()))
                {
                    digest = (digest ^ u64::from(byte)).wrapping_mul(0x100000001b3);
                }
            }
            digests.push(format!("{digest:016x}"));
        }
        let mean_ms = samples.iter().sum::<f64>() / samples.len() as f64;
        eprintln!(
            "crowd: {} particles, mean {mean_ms:.3} ms/frame",
            particles.len()
        );
        if let Ok(path) = std::env::var("METEOR_EFFECT_BENCH_OUT") {
            std::fs::write(path, serde_json::to_string_pretty(&serde_json::json!({
                "particles": particles.len(), "mean_ms": mean_ms, "frame_ms": samples, "digests": digests,
            })).unwrap()).unwrap();
        }
    }

    #[derive(Resource)]
    struct TestNinjaSmoke(&'static str);

    #[allow(clippy::too_many_arguments)]
    fn spawn_test_ninja_smoke(
        effect: Res<TestNinjaSmoke>,
        mut commands: Commands,
        mut cache: ResMut<EfCache>,
        shapes: Res<PonEfShapeMeshes>,
        assets: Res<EffectAssets>,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<PonEfMaterial>>,
        mut gmdl_materials: ResMut<Assets<GmdlMaterial>>,
        mut textures: ResMut<EfTextureCache>,
        mut images: ResMut<Assets<Image>>,
    ) {
        let model = commands
            .spawn((
                crate::plugins::character_plugin::FighterModel {
                    fighter: Entity::PLACEHOLDER,
                    character: "p0".to_string(),
                },
                Transform::from_scale(Vec3::splat(meteor_asset::pon_ef::PON_EF_SCALE)),
            ))
            .id();
        let mut ctx = EfSpawnContext {
            shapes: &shapes,
            assets: &assets,
            meshes: &mut meshes,
            materials: &mut materials,
            gmdl_materials: &mut gmdl_materials,
            textures: &mut textures,
            images: &mut images,
        };
        let root = spawn_pon_ef(
            &mut commands,
            &mut cache,
            &mut ctx,
            effect.0,
            Vec3::ZERO,
            Some(model),
            false,
        );
        assert!(
            root.is_some(),
            "ninja smoke has visible nodes and must spawn"
        );
    }

    #[test]
    fn origin_res_ninja_smoke_stays_in_world_after_fighter_moves() {
        for effect in ["pos526", "pos536"] {
            let mut app = App::new();
            app.insert_resource(TestNinjaSmoke(effect));
            app.add_plugins(MinimalPlugins)
                .add_plugins(bevy::transform::TransformPlugin)
                .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                    std::time::Duration::from_millis(16),
                ))
                .init_resource::<Assets<Mesh>>()
                .init_resource::<Assets<StandardMaterial>>()
                .init_resource::<Assets<PonEfMaterial>>()
                .init_resource::<Assets<Image>>()
                .init_resource::<Assets<GmdlMaterial>>()
                .init_resource::<Assets<AudioSource>>()
                .init_resource::<EfCache>()
                .init_resource::<EfTextureCache>()
                .init_resource::<SfxCache>()
                .add_systems(
                    Startup,
                    (setup_effect_assets, spawn_test_ninja_smoke).chain(),
                )
                .add_systems(Update, (tick_pon_ef_players, tick_pon_ef_particles).chain());
            for _ in 0..150 {
                app.update();
                if app
                    .world_mut()
                    .query::<&PonEfParticle>()
                    .iter(app.world())
                    .any(|p| p.alive)
                {
                    break;
                }
            }
            let samples: Vec<_> = app
                .world_mut()
                .query::<(Entity, &PonEfParticle, &GlobalTransform)>()
                .iter(app.world())
                .filter(|(_, p, _)| p.alive)
                .map(|(entity, p, global)| {
                    (
                        entity,
                        global.translation(),
                        global.to_scale_rotation_translation().0,
                        p.dir,
                    )
                })
                .collect();
            assert!(!samples.is_empty(), "real ninja smoke must emit particles");
            let model = app
                .world_mut()
                .query_filtered::<Entity, With<crate::plugins::character_plugin::FighterModel>>()
                .single(app.world())
                .unwrap();
            app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::ZERO,
            ));

            for translation in [Vec3::new(10.0, 3.0, -8.0), Vec3::new(-7.0, 5.0, 12.0)] {
                *app.world_mut().get_mut::<Transform>(model).unwrap() = Transform {
                    translation,
                    rotation: Quat::from_rotation_y(1.2),
                    scale: Vec3::splat(0.24),
                };
                app.update();
                for (entity, position, scale, direction) in &samples {
                    let global = app.world().get::<GlobalTransform>(*entity).unwrap();
                    assert!(
                        global.translation().distance(*position) < 1e-4,
                        "emitted smoke followed the character: {:?} -> {:?}",
                        position,
                        global.translation()
                    );
                    assert!(global.to_scale_rotation_translation().0.distance(*scale) < 1e-4);
                    assert_eq!(
                        app.world().get::<PonEfParticle>(*entity).unwrap().dir,
                        *direction
                    );
                }
            }

            for (entity, ..) in &samples {
                let mut p = app.world_mut().get_mut::<PonEfParticle>(*entity).unwrap();
                p.alive = false;
                p.spent = false;
            }
            for mut group in app
                .world_mut()
                .query::<&mut PonEfParticleGroup>()
                .iter_mut(app.world_mut())
            {
                group.params.one_shot = false;
            }
            app.update();
            for (entity, position, ..) in &samples {
                let p = app.world().get::<PonEfParticle>(*entity).unwrap();
                assert!(p.alive);
                assert!(
                    p.spawn_position.distance(*position) > 5.0,
                    "new smoke must use the moved emitter"
                );
            }
        }
    }

    #[test]
    fn origin_res_pos253_keeps_authored_red_cylinder_and_plane_volume() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !origin.join("SFX/pos253.ef").is_file() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::transform::TransformPlugin)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(16),
            ))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<PonEfMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<Assets<AudioSource>>()
            .init_resource::<EfCache>()
            .init_resource::<EfTextureCache>()
            .init_resource::<SfxCache>()
            .add_systems(Startup, setup_effect_assets)
            .add_systems(Startup, spawn_test_pos253.after(setup_effect_assets))
            .add_systems(Update, (tick_pon_ef_players, tick_pon_ef_particles).chain());

        let camera_rotation = Quat::from_rotation_y(0.7) * Quat::from_rotation_x(-0.3);
        let camera = app
            .world_mut()
            .spawn((
                crate::plugins::camera_plugin::GameplayCamera,
                Transform::IDENTITY,
            ))
            .id();

        for frame in 0..43 {
            if frame == 42 {
                app.world_mut()
                    .get_mut::<Transform>(camera)
                    .unwrap()
                    .rotation = camera_rotation;
            }
            app.update();
        }
        let visuals = app
            .world_mut()
            .query::<(
                &PonEfNodeEntity,
                &Mesh3d,
                &MeshMaterial3d<PonEfMaterial>,
                &Visibility,
            )>()
            .iter(app.world())
            .filter(|(node, _, _, _)| node.0 == 7 || node.0 == 8)
            .map(|(node, mesh, material, visibility)| {
                (node.0, (mesh.0.clone(), material.0.clone(), *visibility))
            })
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(visuals.len(), 2, "Pos253 red cylinder and plane must spawn");

        let meshes = app.world().resource::<Assets<Mesh>>();
        let (cylinder_mesh, cylinder_material, cylinder_visibility) = &visuals[&7];
        let cylinder_mesh = meshes.get(cylinder_mesh).expect("AttackWave mesh");
        let (cylinder_min, cylinder_max) = mesh_position_bounds(cylinder_mesh);
        assert!(
            cylinder_max.x >= 29.9 && cylinder_min.x <= -29.9,
            "AttackWave must keep authored radiusStart=30, got {cylinder_min:?}..{cylinder_max:?}"
        );
        assert!(
            cylinder_min.z >= -1e-4 && cylinder_max.z >= 39.9,
            "values[0]=1 must shift the authored height=40 cone to z=0..40, got {cylinder_min:?}..{cylinder_max:?}"
        );
        assert!(
            (mesh_ring_radius_at_z(cylinder_mesh, 0.0) - 3.0).abs() < 0.01
                && (mesh_ring_radius_at_z(cylinder_mesh, 40.0) - 30.0).abs() < 0.01,
            "AttackWave must preserve the authored 3→30 taper along local +Z"
        );
        assert_eq!(*cylinder_visibility, Visibility::Visible);

        let (plane_mesh, plane_material, plane_visibility) = &visuals[&8];
        let (plane_min, plane_max) =
            mesh_position_bounds(meshes.get(plane_mesh).expect("Rectangle_9 mesh"));
        assert!(
            (plane_max.x - plane_min.x) >= 19.9 && (plane_max.z - plane_min.z) >= 19.9,
            "Rectangle_9 must keep authored 20x20 plane in source XZ, got {plane_min:?}..{plane_max:?}"
        );
        assert_eq!(*plane_visibility, Visibility::Visible);

        let materials = app.world().resource::<Assets<PonEfMaterial>>();
        for handle in [cylinder_material, plane_material] {
            let material = materials.get(handle).expect("Pos253 red material");
            assert_eq!(material.blend, PonEfBlendMode::SrcAlphaOne);
            let color = material.params.color;
            assert!(
                color.x > 0.9 && color.y < 0.2 && color.z < 0.2,
                "Pos253 visual must consume the authored red keyframe, got {color:?}"
            );
        }

        let particle_samples = app
            .world_mut()
            .query::<(&PonEfParticle, &GlobalTransform)>()
            .iter(app.world())
            .filter(|(particle, _)| particle.alive)
            .map(|(particle, transform)| {
                let position = transform.translation();
                let rotation = transform.to_scale_rotation_translation().1;
                (
                    (
                        (position.x * 100.0).round() as i32,
                        (position.y * 100.0).round() as i32,
                        (position.z * 100.0).round() as i32,
                    ),
                    rotation.dot(camera_rotation).abs(),
                    particle.group,
                )
            })
            .collect::<Vec<_>>();
        let unique_positions = particle_samples
            .iter()
            .map(|(position, _, _)| *position)
            .collect::<std::collections::HashSet<_>>();
        assert!(
            unique_positions.len() > 10,
            "Pos253 particle pools must advance one persistent RNG instead of collapsing to a few points"
        );
        assert!(
            particle_samples.iter().all(|(_, dot, _)| *dot > 0.999),
            "every visible particle quad must face the gameplay camera"
        );
        let foot_group = app
            .world_mut()
            .query::<(Entity, &PonEfNodeEntity)>()
            .iter(app.world())
            .find_map(|(entity, node)| (node.0 == 10).then_some(entity))
            .expect("Pos253 Particle_2 group");
        let foot_offsets = app
            .world_mut()
            .query::<&PonEfParticle>()
            .iter(app.world())
            .filter(|particle| particle.group == foot_group && particle.alive)
            .map(|particle| particle.spawn_offset.x)
            .collect::<Vec<_>>();
        assert!(!foot_offsets.is_empty(), "foot emitter has live particles");
        assert!(
            foot_offsets.iter().all(|offset| (*offset - 2.0).abs() < 1e-3),
            "bad_R_Foot must resolve through the model frame and one-anchor emitters must stay on it: {foot_offsets:?}"
        );
    }

    #[test]
    fn origin_res_pos256_effect_nodes_turn_visible_in_window() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !origin.join("SFX/pos256.ef").is_file() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::transform::TransformPlugin)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(16),
            ))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<PonEfMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<Assets<AudioSource>>()
            .init_resource::<EfCache>()
            .init_resource::<EfTextureCache>()
            .init_resource::<SfxCache>()
            .add_systems(Startup, setup_effect_assets)
            .add_systems(Startup, spawn_test_pos256.after(setup_effect_assets))
            .add_systems(Update, (tick_pon_ef_players, tick_pon_ef_particles).chain());

        app.update();
        let roots = app
            .world_mut()
            .query::<&PonEfInstance>()
            .iter(app.world())
            .count();
        assert_eq!(roots, 1, "pos256.ef player root must exist");
        let particle_plane = app
            .world()
            .resource::<PonEfShapeMeshes>()
            .particle_plane
            .as_ref()
            .expect("particle plane handle");
        let particle_plane = app
            .world()
            .resource::<Assets<Mesh>>()
            .get(particle_plane)
            .expect("particle plane mesh");
        let bevy::mesh::VertexAttributeValues::Float32x3(positions) = particle_plane
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("particle plane positions")
        else {
            panic!("particle plane positions must be Float32x3");
        };
        let (min_x, max_x) = positions
            .iter()
            .map(|position| position[0])
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), value| {
                (min.min(value), max.max(value))
            });
        assert!(
            ((max_x - min_x) - 1.0).abs() < 1e-5,
            "PARTICLE must use Web's unit quad before authored size, got width {}",
            max_x - min_x
        );

        for _ in 0..20 {
            app.update();
        }
        let (sphere_scale, sphere_mesh) = app
            .world_mut()
            .query::<(&PonEfNodeEntity, &GlobalTransform, &Mesh3d)>()
            .iter(app.world())
            .find(|(node, _, _)| node.0 == 5)
            .map(|(_, transform, mesh)| {
                (transform.to_scale_rotation_translation().0, mesh.0.clone())
            })
            .expect("Sphere_4 node entity and mesh must exist");
        let (sphere_min, sphere_max) = mesh_position_bounds(
            app.world()
                .resource::<Assets<Mesh>>()
                .get(&sphere_mesh)
                .expect("Sphere_4 authored mesh"),
        );
        let authored_radius = sphere_min.abs().max(sphere_max.abs()).max_element();

        assert!(
            sphere_scale.max_element() * authored_radius > 8.0,
            "Sphere_4 must include its authored radius=10, got mesh radius {authored_radius} and global scale {sphere_scale:?}"
        );
        let (sphere_material, sphere_visibility) = app
            .world_mut()
            .query::<(
                &PonEfNodeEntity,
                &MeshMaterial3d<PonEfMaterial>,
                &Visibility,
            )>()
            .iter(app.world())
            .find(|(node, _, _)| node.0 == 5)
            .map(|(_, material, visibility)| (material.0.clone(), *visibility))
            .expect("Sphere_4 material and visibility");
        assert_eq!(
            sphere_visibility,
            Visibility::Visible,
            "Sphere_4 must be visible inside its authored window"
        );
        let sphere_material = app
            .world()
            .resource::<Assets<PonEfMaterial>>()
            .get(&sphere_material)
            .expect("Sphere_4 material asset");
        assert_eq!(
            sphere_material.blend,
            PonEfBlendMode::SrcAlphaOne,
            "Sphere_4 must keep Pos256's authored mode-4 additive blend"
        );
        assert!(
            sphere_material.texture.is_some(),
            "Sphere_4 must start with Light-02.ifl's first frame"
        );
        let color = sphere_material.params.color;
        assert!(
            color.x > 0.9 && color.y < 0.6 && color.z < 0.6 && color.w > 0.3,
            "Sphere_4 must consume its red render keyframe, got {color:?}"
        );

        for _ in 0..20 {
            app.update();
        }
        let alive_visible = app
            .world_mut()
            .query::<(&PonEfParticle, &Visibility, &GlobalTransform)>()
            .iter(app.world())
            .filter(|(p, visibility, _)| p.alive && **visibility == Visibility::Visible)
            .map(|(_, _, transform)| {
                transform
                    .to_scale_rotation_translation()
                    .0
                    .abs()
                    .max_element()
            })
            .fold(0.0f32, f32::max);
        assert!(alive_visible > 0.0, "particles must be alive and visible");
        assert!(
            alive_visible >= 0.2,
            "Pos256 particles must not apply the 0.12 source scale twice, got {alive_visible}"
        );

        for _ in 0..60 {
            app.update();
        }
        let roots = app
            .world_mut()
            .query::<&PonEfInstance>()
            .iter(app.world())
            .count();
        assert_eq!(roots, 0, "finished effect must despawn");
    }

    #[test]
    fn one_shot_audio_starts_once_until_its_authored_window_ends() {
        let mut triggered = false;
        let actions = [false, true, true, true, false, false, true]
            .map(|inside| update_audio_window_state(&mut triggered, inside));
        assert_eq!(
            actions,
            [
                AudioWindowAction::None,
                AudioWindowAction::Start,
                AudioWindowAction::None,
                AudioWindowAction::None,
                AudioWindowAction::Stop,
                AudioWindowAction::None,
                AudioWindowAction::Start,
            ]
        );
        assert!(
            triggered,
            "a later window entry may trigger exactly once again"
        );
    }

    #[test]
    fn audio_emitter_samples_its_node_offset_on_start_and_while_playing() {
        let mut app = feedback_test_app();
        app.add_systems(Startup, spawn_test_pos253.after(setup_effect_assets));
        app.update();
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<PonEfInstance>>()
            .single(app.world())
            .unwrap();
        app.world_mut().entity_mut(root).remove::<ChildOf>();
        let root_transform =
            Transform::from_xyz(100.0, 10.0, 20.0).with_rotation(Quat::from_rotation_y(0.5));
        app.world_mut().entity_mut(root).insert(root_transform);
        let index = {
            let mut player = app.world_mut().get_mut::<PonEfInstance>(root).unwrap();
            let ef = std::sync::Arc::make_mut(&mut player.ef);
            let index = ef
                .nodes
                .iter()
                .position(|node| node.type_id == meteor_asset::pon_ef::TYPE_AUDIO)
                .unwrap();
            let node = &mut ef.nodes[index];
            node.parents = [String::new(), String::new()];
            node.values[3] = 0;
            node.start_time = 0.0;
            node.end_time = 10.0;
            for keyframe in &mut node.keyframes {
                keyframe.position = [10.0, 20.0, 30.0];
            }
            index
        };
        app.update();
        let audio = app
            .world()
            .get::<PonEfInstance>(root)
            .unwrap()
            .audio_playing[index]
            .unwrap();
        let expected = root_transform.transform_point(Vec3::new(1.2, 3.6, -2.4));
        let position = app
            .world()
            .get::<GlobalTransform>(audio)
            .unwrap()
            .translation();
        assert!(
            position.distance(expected) < 1e-4,
            "first frame {position:?} != {expected:?}"
        );
        {
            let mut player = app.world_mut().get_mut::<PonEfInstance>(root).unwrap();
            for keyframe in &mut std::sync::Arc::make_mut(&mut player.ef).nodes[index].keyframes {
                keyframe.position[0] = 20.0;
            }
        }
        app.update();
        assert_eq!(
            app.world()
                .get::<PonEfInstance>(root)
                .unwrap()
                .audio_playing[index],
            Some(audio)
        );
        let expected = root_transform.transform_point(Vec3::new(2.4, 3.6, -2.4));
        assert!(
            app.world()
                .get::<GlobalTransform>(audio)
                .unwrap()
                .translation()
                .distance(expected)
                < 1e-4
        );
    }

    #[test]
    fn pon_ef_drag_builds_faded_ribbon_from_two_anchor_samples() {
        let history = vec![
            PonEfDragSample {
                time: 0.0,
                base: Vec3::ZERO,
                tip: Vec3::X,
            },
            PonEfDragSample {
                time: 0.1,
                base: Vec3::Y,
                tip: Vec3::X + Vec3::Y,
            },
        ];
        let (positions, colors, indices) = build_pon_ef_drag(&history, 0.1, 0.1);
        assert_eq!(positions.len(), PON_EF_DRAG_SEGMENTS * 4);
        assert_eq!(colors.len(), positions.len());
        assert_eq!(indices.len(), PON_EF_DRAG_SEGMENTS * 6);
        assert!(colors[0][3] > colors[1][3]);
        assert_eq!(positions[0], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn pon_ef_drag_lookback_is_clamped_to_history_window() {
        let history = vec![
            PonEfDragSample {
                time: 0.0,
                base: Vec3::ZERO,
                tip: Vec3::X,
            },
            PonEfDragSample {
                time: 0.1,
                base: Vec3::Y,
                tip: Vec3::X + Vec3::Y,
            },
        ];
        let (_, _, indices) = build_pon_ef_drag(&history, 0.1, 10.0);
        assert_eq!(indices.len(), PON_EF_DRAG_SEGMENTS * 6);
    }

    #[test]
    fn effect_material_instances_do_not_share_keyframe_tint() {
        let mut materials = Assets::<PonEfMaterial>::default();
        let template = materials.add(PonEfMaterial {
            uniform_id: dynamic_uniforms::new_uniform_id(),
            particle_colors: None,
            params: PonEfMaterialParams {
                color: Vec4::ONE,
                has_texture: 0,
            },
            texture: None,
            blend: PonEfBlendMode::Opaque,
            two_sided: true,
        });
        let red = clone_effect_material(&mut materials, &template);
        let green = clone_effect_material(&mut materials, &template);
        assert_ne!(red, green, "every PonEF node owns a material instance");

        materials.get_mut(&red).expect("red material").params.color = Vec4::new(1.0, 0.0, 0.0, 0.5);
        let untouched = materials.get(&green).expect("green material").params.color;
        assert_eq!(
            untouched,
            Vec4::ONE,
            "one node's render keyframe must not overwrite another node"
        );
    }

    fn test_pose(pose_id: u16, pose_instance_id: u32) -> PoseRuntime {
        PoseRuntime {
            pose_id,
            frame: 10.0,
            start: 10,
            end: 20,
            loop_start: 10,
            loop_end: 20,
            loops: false,
            pose_instance_id,
        }
    }

    #[test]
    fn pose_effect_waits_for_entry_blend_completion() {
        let pose = test_pose(200, 7);
        let mut spawn = PoseEffectSpawn::default();
        let mut blend = EntryBlend {
            duration: 0.12,
            pose_id: pose.pose_id,
            ..default()
        };

        assert!(!pose_effect_spawn_ready(&mut spawn, &pose, &blend));
        assert_eq!(spawn.pose_id, 200);
        assert_eq!(spawn.pose_instance_id, 7);
        assert!(!spawn.spawned);

        blend.duration = 0.0;
        assert!(pose_effect_spawn_ready(&mut spawn, &pose, &blend));
        spawn.spawned = true;
        assert!(!pose_effect_spawn_ready(&mut spawn, &pose, &blend));
    }

    #[test]
    fn pose_effect_latch_uses_pose_and_action_instance() {
        let mut spawn = PoseEffectSpawn {
            pose_id: 200,
            pose_instance_id: 7,
            spawned: true,
        };
        let blend = EntryBlend::default();

        let replay = test_pose(200, 8);
        assert!(pose_effect_spawn_ready(&mut spawn, &replay, &blend));
        assert_eq!(spawn.pose_instance_id, 8);
        assert!(!spawn.spawned);

        spawn.spawned = true;
        let linked = test_pose(201, 8);
        assert!(pose_effect_spawn_ready(&mut spawn, &linked, &blend));
        assert_eq!(spawn.pose_id, 201);

        let mut finished = linked;
        finished.frame = finished.end as f32;
        spawn.pose_id = finished.pose_id;
        spawn.pose_instance_id = finished.pose_instance_id;
        spawn.spawned = false;
        assert!(!pose_effect_spawn_ready(&mut spawn, &finished, &blend));
    }

    #[test]
    fn every_fighter_generation_path_requires_a_pose_effect_latch() {
        let mut app = App::new();
        let entity = app
            .world_mut()
            .spawn(Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 6,
                weapon_slots: [6, 5],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            })
            .id();

        assert!(app.world().get::<PoseEffectSpawn>(entity).is_some());
    }

    #[test]
    fn lock_marker_request_exists_only_during_active_gameplay() {
        let lock = Entity::from_bits(11);
        let track = Entity::from_bits(12);

        assert_eq!(
            requested_lock_marker(
                &crate::app_state::AppState::Playing,
                true,
                Some(lock),
                Some(track),
            ),
            Some(("Lock", lock)),
            "Lock must remain mutually exclusive and win over Track"
        );
        assert_eq!(
            requested_lock_marker(
                &crate::app_state::AppState::Playing,
                true,
                None,
                Some(track),
            ),
            Some(("Track", track))
        );
        for state in [
            crate::app_state::AppState::Paused,
            crate::app_state::AppState::Settings,
            crate::app_state::AppState::Result,
        ] {
            assert_eq!(
                requested_lock_marker(&state, true, Some(lock), Some(track)),
                None,
                "non-Playing states must hide the world marker"
            );
        }
        assert_eq!(
            requested_lock_marker(
                &crate::app_state::AppState::Playing,
                false,
                Some(lock),
                Some(track),
            ),
            None,
            "Lock Flag setting disables both marker slots"
        );
    }

    #[test]
    fn assassin_vip_effect_uses_the_authoritative_team_color() {
        let request = crate::app_state::LaunchRequest {
            mode: "assassin".into(),
            ..default()
        };
        let mut net = crate::plugins::network_plugin::NetState {
            player_id: "p1".into(),
            ..default()
        };
        net.members.push(meteor_network::protocol::RoomMemberWire {
            connected: true,
            player_id: "p1".into(),
            name: "Meteor".into(),
            character_id: 0,
            weapon: 6,
            team: Some(1),
            ready: true,
        });
        net.radar_star_players.insert("p1".into());
        assert_eq!(
            vip_effect_for_fighter(
                &request,
                &net,
                Some(&crate::plugins::stage_plugin::LocalFighter),
                None,
                None,
            ),
            Some("vipblue.ef")
        );

        net.stock_vips[1] = Some("stock-9".into());
        let remote = crate::plugins::network_plugin::RemoteFighter {
            player_id: "stock-9".into(),
        };
        assert_eq!(
            vip_effect_for_fighter(&request, &net, None, None, Some(&remote)),
            Some("vipred.ef")
        );
    }

    #[test]
    fn original_hud_and_objective_effect_assets_are_present_and_parseable() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        for name in ["Lock.ef", "Track.ef", "Flag.ef", "vipblue.ef", "vipred.ef"] {
            let path = origin.join("SFX").join(name);
            let bytes = std::fs::read(&path).unwrap_or_else(|error| {
                panic!(
                    "missing original objective marker {}: {error}",
                    path.display()
                )
            });
            let effect = meteor_asset::pon_ef::parse_pon_ef(&bytes)
                .unwrap_or_else(|error| panic!("invalid {}: {error}", path.display()));
            assert!(
                effect.nodes.iter().any(|node| {
                    !meteor_asset::pon_ef::decode_render_state(&node.transform_header).hidden
                }),
                "{} must contain at least one visible node",
                path.display()
            );
        }
    }

    #[test]
    fn origin_res_pose253_effect_spawns_through_production_path() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !origin.join("SFX/pos253.ef").is_file() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let roots = resource_roots(
            &std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        );
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&roots, "p0")
                .expect("p0 assets load from origin_res");
        assert_eq!(
            character
                .poses
                .get(&253)
                .and_then(|pose| pose.effect_id.as_ref())
                .and_then(effect_file_name)
                .as_deref(),
            Some("Pos253")
        );

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::transform::TransformPlugin)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(16),
            ))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<PonEfMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<Assets<AudioSource>>()
            .init_resource::<EfCache>()
            .init_resource::<EfTextureCache>()
            .init_resource::<SfxCache>()
            .insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
                std::collections::HashMap::from([("p0".to_string(), character)]),
            ))
            .add_systems(Startup, setup_effect_assets)
            .add_systems(
                Update,
                (
                    pose_effect_spawn,
                    tick_pon_ef_players,
                    tick_pon_ef_particles,
                )
                    .chain(),
            );

        let fighter = app
            .world_mut()
            .spawn((
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 5,
                    weapon_slots: [5, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                PoseRuntime {
                    pose_id: 253,
                    frame: 9193.0,
                    start: 9193,
                    end: 9236,
                    loop_start: 9193,
                    loop_end: 9236,
                    loops: false,
                    pose_instance_id: 1,
                },
                EntryBlend::default(),
            ))
            .id();
        app.world_mut().spawn((
            crate::plugins::character_plugin::FighterModel {
                fighter,
                character: "p0".to_string(),
            },
            Transform::from_scale(Vec3::splat(meteor_asset::pon_ef::PON_EF_SCALE)),
        ));

        for _ in 0..43 {
            app.update();
        }
        assert_eq!(
            app.world_mut()
                .query::<&PonEfInstance>()
                .iter(app.world())
                .count(),
            1,
            "pose 253 must spawn exactly one Pos253 player"
        );
        let attack_wave_visible = app
            .world_mut()
            .query::<(&PonEfNodeEntity, &Visibility)>()
            .iter(app.world())
            .any(|(node, visibility)| node.0 == 7 && *visibility == Visibility::Visible);
        assert!(
            attack_wave_visible,
            "production Pose 253 must reach its red AttackWave window"
        );
    }

    #[test]
    fn origin_res_weapon_switch_flash_preserves_height_through_production_path() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !origin.join("SFX/Pos024.ef").is_file() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let roots = resource_roots(
            &std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        );
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&roots, "p0")
                .expect("p0 assets load from origin_res");
        assert!(
            character
                .poses
                .get(&24)
                .and_then(|p| p.effect_id.as_ref())
                .is_some(),
            "pose 24 must carry EffectID Pos024"
        );

        let pose_start = character.poses[&24].start;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::transform::TransformPlugin)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(16),
            ))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<PonEfMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<Assets<AudioSource>>()
            .init_resource::<EfCache>()
            .init_resource::<EfTextureCache>()
            .init_resource::<SfxCache>()
            .insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
                std::collections::HashMap::from([("p0".to_string(), character)]),
            ))
            .add_systems(Startup, setup_effect_assets)
            .add_systems(
                Update,
                (
                    pose_effect_spawn,
                    tick_pon_ef_players,
                    tick_pon_ef_particles,
                )
                    .chain(),
            );

        let fighter = app
            .world_mut()
            .spawn((
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 5,
                    weapon_slots: [5, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                PoseRuntime {
                    pose_id: 24,
                    frame: pose_start as f32,
                    start: pose_start,
                    end: pose_start + 30,
                    loop_start: pose_start,
                    loop_end: pose_start + 30,
                    loops: false,
                    pose_instance_id: 1,
                },
                EntryBlend::default(),
                crate::components::fighter::WeaponForm::default(),
                Transform::from_xyz(10.0, 5.0, 20.0),
            ))
            .id();
        app.world_mut().spawn((
            crate::plugins::character_plugin::FighterModel {
                fighter,
                character: "p0".to_string(),
            },
            Transform::from_rotation(
                Quat::from_rotation_y(0.7) * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            )
            .with_scale(Vec3::splat(0.12)),
            ChildOf(fighter),
        ));

        app.update();
        let (effect, parent) = app
            .world_mut()
            .query::<(Entity, &ChildOf)>()
            .iter(app.world())
            .find(|(entity, _)| app.world().get::<PonEfInstance>(*entity).is_some())
            .map(|(entity, parent)| (entity, parent.parent()))
            .unwrap();
        assert_eq!(parent, fighter);
        let flash_time = app
            .world()
            .get::<PonEfInstance>(effect)
            .unwrap()
            .ef
            .nodes
            .iter()
            .find(|node| node.type_id == meteor_asset::pon_ef::TYPE_BILLBOARD)
            .unwrap()
            .start_time
            + 0.01;
        for height in [5.0, 12.0] {
            app.world_mut()
                .get_mut::<Transform>(fighter)
                .unwrap()
                .translation
                .y = height;
            app.world_mut()
                .entity_mut(effect)
                .insert(PonEfStartTime(flash_time));
            app.update();
            let flash = app
                .world_mut()
                .query::<(&PonEfNodeOwner, &PonEfNodeEntity, &GlobalTransform)>()
                .iter(app.world())
                .find(|(owner, node, _)| {
                    owner.0 == effect
                        && app.world().get::<PonEfInstance>(effect).unwrap().ef.nodes[node.0]
                            .type_id
                            == meteor_asset::pon_ef::TYPE_BILLBOARD
                })
                .map(|(_, _, transform)| transform.translation())
                .unwrap();
            assert!(
                (flash.y - height - 21.263 * 0.12).abs() < 1e-4,
                "switch flash must retain authored hand-height offset: {flash:?}"
            );
        }
    }

    #[test]
    fn origin_res_pose256_effect_spawns_through_production_path() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !origin.join("SFX/pos256.ef").is_file() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let roots = resource_roots(
            &std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        );
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&roots, "p0")
                .expect("p0 assets load from origin_res");
        assert!(
            character
                .poses
                .get(&256)
                .and_then(|p| p.effect_id.as_ref())
                .is_some(),
            "pose 256 must carry EffectID Pos256"
        );

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::transform::TransformPlugin)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(16),
            ))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<PonEfMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<Assets<AudioSource>>()
            .init_resource::<EfCache>()
            .init_resource::<EfTextureCache>()
            .init_resource::<SfxCache>()
            .insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
                std::collections::HashMap::from([("p0".to_string(), character)]),
            ))
            .add_systems(Startup, setup_effect_assets)
            .add_systems(
                Update,
                (
                    pose_effect_spawn,
                    tick_pon_ef_players,
                    tick_pon_ef_particles,
                )
                    .chain(),
            );

        let fighter = app
            .world_mut()
            .spawn((
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 5,
                    weapon_slots: [5, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                PoseRuntime {
                    pose_id: 256,
                    frame: 9377.0,
                    start: 9377,
                    end: 9413,
                    loop_start: 9377,
                    loop_end: 9413,
                    loops: false,
                    pose_instance_id: 1,
                },
                EntryBlend::default(),
                crate::components::fighter::WeaponForm::default(),
                Transform::IDENTITY,
            ))
            .id();
        app.world_mut().spawn((
            crate::plugins::character_plugin::FighterModel {
                fighter,
                character: "p0".to_string(),
            },
            Transform::IDENTITY,
        ));

        app.update();
        let spawned = app
            .world_mut()
            .query::<&PonEfInstance>()
            .iter(app.world())
            .count();
        assert_eq!(spawned, 1, "pose 256 must spawn its pos256.ef player");
        let latch = app.world().get::<PoseEffectSpawn>(fighter).expect("latch");
        assert!(latch.spawned, "latch records the spawn");

        for _ in 0..3 {
            app.update();
        }
        let spawned = app
            .world_mut()
            .query::<&PonEfInstance>()
            .iter(app.world())
            .count();
        assert_eq!(spawned, 1, "same pose instance must not respawn");

        for _ in 0..22 {
            app.update();
        }
        let sphere_scale = app
            .world_mut()
            .query::<(&PonEfNodeEntity, &Transform)>()
            .iter(app.world())
            .find(|(node, _)| node.0 == 5)
            .map(|(_, transform)| transform.scale);
        let Some(scale) = sphere_scale else {
            panic!("Sphere_4 node entity must exist");
        };
        assert!(
            scale.y > 2.0,
            "Sphere_4 should be scaling up inside its window, got {scale:?}"
        );
    }

    #[test]
    fn ef_candidates_cover_mixed_casings() {
        assert_eq!(ef_candidates("Pos042"), vec!["Pos042", "pos042", "POS042"]);

        assert_eq!(ef_candidates("pos024"), vec!["pos024", "Pos024", "POS024"]);

        assert_eq!(ef_candidates("24"), vec!["24"]);

        assert_eq!(ef_candidates("HOT.ef"), vec!["HOT", "hot"]);
    }

    #[test]
    fn effect_file_name_matches_effect_id_forms() {
        use meteor_asset::pos::EffectId;

        assert_eq!(effect_file_name(&EffectId::Int(0)), None);
        assert_eq!(effect_file_name(&EffectId::Str("0".into())), None);

        assert_eq!(
            effect_file_name(&EffectId::Str("Pos042".into())),
            Some("Pos042".into())
        );
        assert_eq!(
            effect_file_name(&EffectId::Str("pos022".into())),
            Some("pos022".into())
        );

        assert_eq!(effect_file_name(&EffectId::Int(42)), Some("42".into()));
    }

    #[test]
    fn external_bone_frame_preserves_root_relative_rotation_scale_and_translation() {
        let root = bevy::math::Mat4::from_scale_rotation_translation(
            Vec3::splat(0.12),
            Quat::from_rotation_y(0.7),
            Vec3::new(12.0, 3.0, -4.0),
        );
        let local = bevy::math::Mat4::from_scale_rotation_translation(
            Vec3::new(1.2, 0.8, 1.0),
            Quat::from_rotation_x(-0.35),
            Vec3::new(2.0, 5.0, -1.0),
        );
        let relative = external_frame_in_root(root.inverse(), root * local);
        for (actual, expected) in relative.m.iter().zip(local.to_cols_array()) {
            assert!((actual - expected).abs() < 1e-4);
        }
    }

    #[test]
    fn billboard_local_rotation_cancels_parent_and_matches_camera_world_rotation() {
        let root_rotation = Quat::from_rotation_y(0.8) * Quat::from_rotation_x(-0.4);
        let root = GlobalTransform::from(
            Transform::from_rotation(root_rotation).with_scale(Vec3::splat(0.12)),
        );
        let camera_rotation = Quat::from_rotation_y(-0.3) * Quat::from_rotation_x(0.2);
        let local = camera_facing_local_rotation(&root, camera_rotation);
        let world = root_rotation * local;
        assert!(world.dot(camera_rotation).abs() > 0.99999);
    }

    #[test]
    fn origin_res_pose_effects_resolve_to_sfx_files() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("SFX/Pos024.ef").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let pos_bytes = std::fs::read(root.join("pmodel/P0.POS")).expect("P0.POS");
        let poses = meteor_asset::pos::parse_poses(&crate::text_codec::decode_text(&pos_bytes));
        let by_id: std::collections::BTreeMap<u16, _> =
            poses.into_iter().map(|p| (p.id, p)).collect();

        for id in [24u16, 262, 272, 288, 305, 310, 315, 320] {
            let pose = by_id.get(&id).unwrap_or_else(|| panic!("pose {id} exists"));
            let name = pose
                .effect_id
                .as_ref()
                .and_then(effect_file_name)
                .unwrap_or_else(|| panic!("pose {id} carries EffectID"));
            assert_eq!(name, format!("Pos{id:03}"), "pose {id} EffectID");

            let bytes = ef_candidates(&name)
                .into_iter()
                .map(|c| root.join("SFX").join(format!("{c}.ef")))
                .find_map(|path| std::fs::read(path).ok())
                .unwrap_or_else(|| panic!("SFX file for {name}"));
            let ef = meteor_asset::pon_ef::parse_pon_ef(&bytes)
                .unwrap_or_else(|e| panic!("{name} parses: {e}"));
            assert!(!ef.nodes.is_empty(), "{name} has nodes");
        }

        let pos24 = meteor_asset::pon_ef::parse_pon_ef(
            &std::fs::read(root.join("SFX/Pos024.ef")).expect("Pos024.ef"),
        )
        .expect("Pos024.ef parses");
        let flash = pos24
            .nodes
            .iter()
            .find(|n| n.type_id == meteor_asset::pon_ef::TYPE_BILLBOARD)
            .expect("Pose 24 switch flash is a BILLBOARD");
        assert_eq!(flash.texture, "zs07.bmp");
        assert_eq!(flash.parents[0], "Character");
        assert_eq!(flash.values[3], 0, "values[3]==0 world-axis offset");
        let first = flash.keyframes.first().expect("flash keyframes");
        assert!(
            (first.position[0] - 4.421).abs() < 1e-3,
            "waist-height offset x"
        );
        assert!(
            (first.position[2] - 21.263).abs() < 1e-3,
            "waist-height offset z"
        );
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod texture_tests {
    use super::*;

    #[test]
    fn all_original_visible_effect_textures_resolve_except_missing_item12_art() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut cache = EfTextureCache::default();
        let mut images = Assets::<Image>::default();
        let mut materials = Assets::<PonEfMaterial>::default();
        let mut files = 0;
        let mut missing = Vec::new();
        for entry in std::fs::read_dir(root.join("SFX")).unwrap() {
            let path = entry.unwrap().path();
            if !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ef"))
            {
                continue;
            }
            let ef = meteor_asset::pon_ef::parse_pon_ef(&std::fs::read(&path).unwrap()).unwrap();
            files += 1;
            for node in &ef.nodes {
                if node.texture.is_empty()
                    || meteor_asset::pon_ef::decode_render_state(&node.transform_header).hidden
                    || matches!(
                        node.type_id,
                        meteor_asset::pon_ef::TYPE_AUDIO
                            | meteor_asset::pon_ef::TYPE_PATH
                            | meteor_asset::pon_ef::TYPE_MODEL
                    )
                {
                    continue;
                }
                let handle = cache.material_for(&mut images, &mut materials, node);
                if materials.get(&handle).unwrap().texture.is_none() {
                    missing.push(format!(
                        "{}:{}:{}",
                        path.file_name().unwrap().to_string_lossy(),
                        node.name,
                        node.texture
                    ));
                }
                if node.texture.to_ascii_lowercase().ends_with(".ifl") {
                    let bytes = cache.texture_bytes(&node.texture).expect("IFL list");
                    let expected =
                        super::super::ifl_plugin::parse_ifl(std::str::from_utf8(&bytes).unwrap());
                    assert_eq!(
                        cache.ifl_frames(&mut images, &node.texture).len(),
                        expected.len(),
                        "all frames: {}",
                        node.texture
                    );
                }
            }
        }
        missing.sort();
        assert_eq!(files, 1092);
        assert_eq!(
            missing,
            [
                "Item12.ef:Drag_5:slashF00.bmp",
                "Item12.ef:Drag_7:slashF00.bmp"
            ]
        );
    }

    #[test]
    fn original_worn_models_use_bmp_alpha_instead_of_color_luminance() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let patch = meteor_asset::pak::PakArchive::from_bytes(
            &std::fs::read(root.join("TexturePatch.pak")).unwrap(),
        )
        .unwrap();
        let mut cache = EfTextureCache::default();
        let mut images = Assets::<Image>::default();
        for model in ["Item13L", "Item14L01", "Item1501", "item1701"] {
            let mesh = meteor_asset::gmdl::decode_gmdl(
                &std::fs::read(root.join(format!("cmodel/{model}.gmb"))).unwrap(),
            )
            .unwrap();
            for mat in mesh
                .materials
                .iter()
                .filter(|mat| mat.blend == "ALPHACHANNEL")
            {
                let name = mesh.resolve_texture_name(mat).unwrap();
                let handle = cache.model_texture_handle(&mut images, name, mat).unwrap();
                let companion =
                    super::super::pak_textures::legacy_alpha_companion_name(name).unwrap();
                let bytes =
                    super::super::pak_textures::resolve_pak_texture_exact(&[&patch], &companion)
                        .unwrap();
                let alpha = meteor_asset::texture::decode_image(&bytes).unwrap();
                let image = images.get(&handle).unwrap();
                assert_eq!(image.texture_descriptor.size.width, alpha.width);
                let actual: Vec<_> = image
                    .data
                    .as_ref()
                    .unwrap()
                    .chunks_exact(4)
                    .map(|p| p[3])
                    .collect();
                let expected: Vec<_> = alpha.rgba.chunks_exact(4).map(|p| p[0]).collect();
                assert_eq!(actual, expected, "{model}: authored alpha");
            }
        }
    }

    #[test]
    fn origin_res_etexture_resolves_mixed_case_and_ifl() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("ETexture.pak").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let mut cache = EfTextureCache::default();

        let bytes = cache.texture_bytes("zs07.bmp").expect("zs07.bmp resolves");
        assert!(bytes.len() > 100, "zs07.bmp has image data");

        let mut images = Assets::<Image>::default();
        let frames = cache.ifl_frames(&mut images, "bom.ifl");
        assert!(
            frames.len() >= 2,
            "bom.ifl yields frames (got {})",
            frames.len()
        );

        let again = cache.ifl_frames(&mut images, "bom.ifl");
        assert_eq!(frames.len(), again.len());
    }

    #[test]
    fn origin_res_pos024_flash_gets_textured_material() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("SFX/Pos024.ef").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let ef =
            meteor_asset::pon_ef::parse_pon_ef(&std::fs::read(root.join("SFX/Pos024.ef")).unwrap())
                .expect("Pos024 parses");
        let flash = ef
            .nodes
            .iter()
            .find(|n| n.type_id == meteor_asset::pon_ef::TYPE_BILLBOARD)
            .expect("flash node");
        assert_eq!(flash.texture, "zs07.bmp");
        let mut cache = EfTextureCache::default();
        let mut images = Assets::<Image>::default();
        let mut materials = Assets::<PonEfMaterial>::default();
        let handle = cache.material_for(&mut images, &mut materials, flash);
        let material = materials.get(&handle).expect("material asset");
        assert!(material.texture.is_some(), "flash is textured");
        assert_eq!(material.blend, PonEfBlendMode::SrcColorOne);
    }

    #[test]
    fn pon_ef_material_preserves_all_authored_fixed_function_blend_pairs() {
        assert_eq!(PonEfBlendMode::Opaque.factors(), None);
        assert_eq!(
            PonEfBlendMode::SrcColorOne.factors(),
            Some((BlendFactor::Src, BlendFactor::One))
        );
        assert_eq!(
            PonEfBlendMode::ZeroInvSrcColor.factors(),
            Some((BlendFactor::Zero, BlendFactor::OneMinusSrc))
        );
        assert_eq!(
            PonEfBlendMode::SrcAlphaInvSrcAlpha.factors(),
            Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha))
        );
        assert_eq!(
            PonEfBlendMode::SrcAlphaOne.factors(),
            Some((BlendFactor::SrcAlpha, BlendFactor::One))
        );
        let shader = include_str!("pon_ef_material.wgsl");
        assert!(shader.contains("#{MATERIAL_BIND_GROUP}"));
        assert!(shader.contains("return texel * params.color"));
    }

    #[test]
    fn origin_res_untextured_gunhit_keeps_mode2_material_instead_of_opaque_spark() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("SFX/GunHit.ef").is_file() {
            return;
        }
        let ef = meteor_asset::pon_ef::parse_pon_ef(
            &std::fs::read(root.join("SFX/GunHit.ef")).expect("GunHit.ef"),
        )
        .expect("GunHit parses");
        let sphere = ef
            .nodes
            .iter()
            .find(|node| node.type_id == meteor_asset::pon_ef::TYPE_SPHERE)
            .expect("GunHit sphere");
        assert!(sphere.texture.is_empty());
        let mut cache = EfTextureCache::default();
        let mut images = Assets::<Image>::default();
        let mut materials = Assets::<PonEfMaterial>::default();
        let handle = cache.material_for(&mut images, &mut materials, sphere);
        let material = materials.get(&handle).expect("GunHit material");
        assert!(material.texture.is_none());
        assert_eq!(material.params.has_texture, 0);
        assert_eq!(material.blend, PonEfBlendMode::ZeroInvSrcColor);
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod model_tests {
    use super::*;

    #[test]
    fn cmodel_stem_strips_path_and_extension() {
        assert_eq!(cmodel_stem(r"D:\EFC\CMODEL\HOT.des"), Some("HOT".into()));
        assert_eq!(
            cmodel_stem("D:\\EFC\\CMODEL\\Item1201.des"),
            Some("Item1201".into())
        );
        assert_eq!(cmodel_stem("Item13L.des"), Some("Item13L".into()));

        assert_eq!(cmodel_stem(""), None);
        assert_eq!(cmodel_stem(".."), Some("..".into()));
    }

    #[test]
    fn origin_res_worn_models_resolve_to_cmodel_parts() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("SFX/HOT.ef").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let worn = [
            ("HOT.ef", "HOT"),
            ("Item12.ef", "Item1201"),
            ("Item13.ef", "Item13L"),
            ("Item14.ef", "Item14L01"),
            ("Item15.ef", "Item1501"),
            ("Item16.ef", "item16"),
            ("Item17.ef", "item1701"),
        ];
        let mut saw_parented_keyframed_model = false;
        let mut resolved_model_textures = 0usize;
        let mut texture_cache = EfTextureCache::default();
        for (file, expected) in worn {
            let ef = meteor_asset::pon_ef::parse_pon_ef(
                &std::fs::read(root.join("SFX").join(file)).expect(file),
            )
            .unwrap_or_else(|e| panic!("{file}: {e}"));
            let models: Vec<String> = ef
                .nodes
                .iter()
                .filter(|n| n.type_id == meteor_asset::pon_ef::TYPE_MODEL)
                .filter_map(|node| {
                    saw_parented_keyframed_model |=
                        !node.parents[0].is_empty() && !node.keyframes.is_empty();
                    match &node.tail {
                        meteor_asset::pon_ef::PonEfTail::Model { model } => cmodel_stem(model),
                        _ => None,
                    }
                })
                .collect();
            assert!(
                models
                    .iter()
                    .any(|stem| stem.eq_ignore_ascii_case(expected)),
                "{file} ships {expected} (got {models:?})"
            );

            for stem in models {
                let gmb = root.join("cmodel").join(format!("{stem}.gmb")).exists()
                    || root
                        .join("cmodel")
                        .join(format!("{}.GMB", stem.to_uppercase()))
                        .exists();
                let des = root.join("cmodel").join(format!("{stem}.des")).exists();
                assert!(gmb, "{file}: cmodel/{stem}.gmb ships");
                assert!(des, "{file}: cmodel/{stem}.des ships");
                if gmb {
                    let path = [
                        root.join("cmodel").join(format!("{stem}.gmb")),
                        root.join("cmodel")
                            .join(format!("{}.GMB", stem.to_uppercase())),
                    ]
                    .into_iter()
                    .find(|path| path.is_file())
                    .expect("worn gmb path");
                    let gmdl = meteor_asset::gmdl::decode_gmdl(
                        &std::fs::read(path).expect("read worn gmb"),
                    )
                    .expect("decode worn gmb");
                    resolved_model_textures += gmdl
                        .materials
                        .iter()
                        .filter_map(|material| gmdl.resolve_texture_name(material))
                        .filter(|name| texture_cache.model_texture_bytes(name).is_some())
                        .count();
                }
            }
        }
        assert!(
            saw_parented_keyframed_model,
            "shipped worn MODEL nodes exercise parent + keyframe transforms"
        );
        assert!(
            resolved_model_textures > 0,
            "shipped worn MODEL textures resolve through ctexture chain"
        );
    }

    #[test]
    fn model_worn_holds_geometry_after_flourish_while_pure_aura_keeps_looping() {
        let origin =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/SFX");
        if !origin.join("Item15.ef").is_file() {
            return;
        }
        let instance = |name: &str, elapsed: f32| {
            let ef = std::sync::Arc::new(
                meteor_asset::pon_ef::parse_pon_ef(
                    &std::fs::read(origin.join(format!("{name}.ef"))).expect(name),
                )
                .unwrap_or_else(|error| panic!("parse {name}: {error}")),
            );
            let duration = ef
                .nodes
                .iter()
                .map(|node| node.end_time)
                .fold(0.0, f32::max);
            let node_count = ef.nodes.len();
            PonEfInstance {
                ef,
                elapsed,
                duration,
                looping: true,
                audio_playing: vec![None; node_count],
                audio_triggered: vec![false; node_count],
            }
        };

        let hat = instance("Item15", 9.0);
        let (hat_time, hat_hold) = pon_ef_playback_time(&hat, true);
        assert!(hat_hold, "MODEL equipment enters the persistent held phase");
        assert!((hat_time - hat.duration).abs() < f32::EPSILON);

        let aura = instance("DefUP", 9.0);
        let (aura_time, aura_hold) = pon_ef_playback_time(&aura, true);
        assert!(!aura_hold, "pure aura keeps its full authored loop");
        assert!((aura_time - (aura.elapsed % aura.duration)).abs() < f32::EPSILON);
    }

    #[test]
    fn christmas_worn_model_parents_exist_on_every_character_skeleton() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !origin.join("SFX/Item12.ef").is_file() {
            return;
        }
        let mut required = std::collections::BTreeSet::new();
        for effect in ["Item12", "Item13", "Item14", "Item15", "Item16", "Item17"] {
            let ef = meteor_asset::pon_ef::parse_pon_ef(
                &std::fs::read(origin.join("SFX").join(format!("{effect}.ef")))
                    .unwrap_or_else(|error| panic!("read {effect}.ef: {error}")),
            )
            .unwrap_or_else(|error| panic!("parse {effect}.ef: {error}"));
            for node in ef
                .nodes
                .iter()
                .filter(|node| node.type_id == meteor_asset::pon_ef::TYPE_MODEL)
            {
                required.extend(
                    node.parents
                        .iter()
                        .filter(|name| !name.is_empty())
                        .map(|name| name.to_ascii_lowercase()),
                );
            }
        }
        assert!(!required.is_empty());

        for character in 0..=19 {
            let file = if character < 10 {
                format!("P{character}.bnc")
            } else {
                format!("p{character}.bnc")
            };
            let bytes = std::fs::read(origin.join("pmodel").join(&file))
                .unwrap_or_else(|error| panic!("read {file}: {error}"));
            let text = crate::text_codec::decode_text(&bytes);
            let bones = meteor_asset::bnc::parse_bones(&text)
                .all
                .into_iter()
                .map(|bone| bone.name.to_ascii_lowercase())
                .collect::<std::collections::HashSet<_>>();
            for parent in &required {
                assert!(
                    bones.contains(parent),
                    "{file} must expose Christmas worn bone {parent}"
                );
            }
        }
    }

    #[test]
    fn production_model_node_spawns_a_keyframed_owned_container() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !origin.join("SFX/HOT.ef").is_file() {
            return;
        }
        let ef = meteor_asset::pon_ef::parse_pon_ef(
            &std::fs::read(origin.join("SFX/HOT.ef")).expect("HOT.ef"),
        )
        .expect("parse HOT.ef");
        let node = ef
            .nodes
            .iter()
            .find(|node| node.type_id == meteor_asset::pon_ef::TYPE_MODEL)
            .expect("HOT MODEL node");

        let mut world = World::new();
        let root = world.spawn_empty().id();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let shapes = PonEfShapeMeshes::default();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<PonEfMaterial>::default();
        let mut gmdl_materials = Assets::<GmdlMaterial>::default();
        let mut textures = EfTextureCache::default();
        let mut images = Assets::<Image>::default();
        let assets = EffectAssets {
            spark_mesh: Handle::default(),
            spark_material: Handle::default(),
            worn_material: Handle::default(),
        };
        let container = {
            let mut commands = Commands::new(&mut queue, &world);
            let mut ctx = EfSpawnContext {
                shapes: &shapes,
                assets: &assets,
                meshes: &mut meshes,
                materials: &mut materials,
                gmdl_materials: &mut gmdl_materials,
                textures: &mut textures,
                images: &mut images,
            };
            spawn_pon_ef_model(&mut commands, node, &mut ctx, root).expect("MODEL container")
        };
        queue.apply(&mut world);

        assert_eq!(
            world.get::<PonEfNodeEntity>(container).unwrap().0,
            node.index
        );
        assert_eq!(world.get::<PonEfNodeOwner>(container).unwrap().0, root);
        assert!(world.get::<PonEfModelNode>(container).is_some());
        assert_eq!(world.get::<ChildOf>(container).unwrap().parent(), root);
        assert!(
            world
                .get::<Children>(container)
                .is_some_and(|children| !children.is_empty()),
            "decoded cmodel meshes are children of the animated node container"
        );
    }

    #[test]
    fn origin_res_particle_parent_inventory() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/SFX");
        if !root.is_dir() {
            return;
        }
        let mut parents = std::collections::BTreeSet::new();
        let mut count = 0usize;
        for path in std::fs::read_dir(root)
            .expect("SFX directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("ef"))
            })
        {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let Ok(ef) = meteor_asset::pon_ef::parse_pon_ef(&bytes) else {
                continue;
            };
            for node in ef
                .nodes
                .iter()
                .filter(|node| node.type_id == meteor_asset::pon_ef::TYPE_PARTICLE)
            {
                count += 1;
                parents.insert(node.parents[0].to_ascii_lowercase());
                parents.insert(node.parents[1].to_ascii_lowercase());
            }
        }
        assert!(count > 0);
        assert_eq!(
            parents,
            std::collections::BTreeSet::from([String::new()]),
            "shipped PARTICLE nodes use the effect container frame"
        );
    }
}

#[cfg(test)]
#[path = "effect_plugin/instancing_tests.rs"]
mod instancing_tests;
