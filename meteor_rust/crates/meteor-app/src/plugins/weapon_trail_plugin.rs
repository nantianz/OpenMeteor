#![allow(dead_code)]

use crate::components::fighter::{Fighter, PoseRuntime, WeaponCollisionGeometry, WeaponForm};
use crate::plugins::character_plugin::{CharacterAssetsSet, WeaponEndpoint};
use crate::plugins::stage_material::{GmdlMaterial, GmdlMaterialParams, GmdlMode};
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::material::AlphaMode;
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::mesh::Indices;
use bevy::render::mesh::MeshVertexBufferLayoutRef;
use bevy::render::render_resource::{
    AsBindGroup, BlendState, PrimitiveTopology, RenderPipelineDescriptor, ShaderType,
    SpecializedMeshPipelineError,
};
use bevy::shader::{Shader, ShaderRef};
use bevy::transform::TransformSystems;
use std::collections::HashMap;

const DRAG_SEGMENTS: usize = 20;
const DRAG_HISTORY_SECONDS: f32 = 0.5;
const DRAG_CONTROL_SECONDS: f32 = 0.03;
const DRAG_CONTROL_POINTS: usize = 5;
const MAX_TRAIL_STREAMS: usize = 64;
const RIPPLE_LOOKBACK: f32 = 0.14;
const RIPPLE_SEGMENTS: usize = 20;
const RIPPLE_MAX_STREAMS: usize = 16;
const RIPPLE_SPEED_MIN: f32 = 2.6;
const RIPPLE_SPEED_MAX: f32 = 9.0;

const RIPPLE_EDGE_OVERSHOOT: f32 = 0.60;
const RIPPLE_BASE_OVERSHOOT: f32 = 0.05;
mod ripple_wake;
use ripple_wake::{build_wakes, record_ripple, RippleWake};

const BLADE_RIPPLE_SHADER_HANDLE: Handle<Shader> =
    bevy::asset::uuid_handle!("5b11ad3e-71aa-4001-8c61-115e77aa0001");

pub struct WeaponTrailPlugin;

#[derive(Component)]
struct WeaponTrailRibbon;

#[derive(Component)]
struct BladeRippleRibbon;

#[derive(ShaderType, Clone)]
struct BladeRippleParams {
    time: f32,
    impact: f32,
    color: Vec3,
}

#[derive(Asset, AsBindGroup, TypePath, Clone)]
struct BladeRippleMaterial {
    #[uniform(0)]
    params: BladeRippleParams,
}

impl Material for BladeRippleMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(BLADE_RIPPLE_SHADER_HANDLE)
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Opaque
    }

    fn reads_view_transmission_texture(&self) -> bool {
        true
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        if let Some(fragment) = descriptor.fragment.as_mut() {
            for target in fragment.targets.iter_mut().flatten() {
                target.blend = Some(BlendState::ALPHA_BLENDING);
            }
        }
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TrailKey {
    fighter: Entity,
    slot: u8,
    part: String,
}

#[derive(Debug, Clone, Copy)]
struct EdgeSample {
    time: f32,
    base: Vec3,
    tip: Vec3,
}

#[derive(Resource, Default)]
struct WeaponTrailRuntime {
    entity: Option<Entity>,
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<GmdlMaterial>>,
    ripple_entity: Option<Entity>,
    ripple_mesh: Option<Handle<Mesh>>,
    ripple_material: Option<Handle<BladeRippleMaterial>>,
    now: f32,
    histories: HashMap<TrailKey, Vec<EdgeSample>>,
    wakes: Vec<RippleWake>,
}

#[derive(Default)]
struct PartEndpoints {
    left: bool,
    right_start: Option<Vec3>,
    right_end: Option<Vec3>,
    left_start: Option<Vec3>,
    left_end: Option<Vec3>,
}

impl PartEndpoints {
    fn edge(&self) -> Option<(Vec3, Vec3)> {
        if self.left {
            self.left_start
                .zip(self.left_end)
                .or_else(|| self.right_start.zip(self.right_end))
        } else {
            self.right_start
                .zip(self.right_end)
                .or_else(|| self.left_start.zip(self.left_end))
        }
    }
}

impl Plugin for WeaponTrailPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<BladeRippleMaterial>::default());
        bevy::asset::load_internal_asset!(
            app,
            BLADE_RIPPLE_SHADER_HANDLE,
            "blade_ripple.wgsl",
            Shader::from_wgsl
        );
        app.init_resource::<WeaponTrailRuntime>()
            .add_systems(
                OnEnter(crate::app_state::AppState::Playing),
                setup_weapon_trail,
            )
            .add_systems(
                OnExit(crate::app_state::AppState::Playing),
                cleanup_weapon_trail,
            )
            .add_systems(
                PostUpdate,
                (
                    update_weapon_trail.run_if(crate::plugins::simulation_frame),
                    update_blade_ripples,
                )
                    .chain()
                    .after(TransformSystems::Propagate)
                    .run_if(in_state(crate::app_state::AppState::Playing)),
            );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod render_tests;

#[cfg(test)]
mod shader_binding_tests {
    #[test]
    fn refractive_shader_validates_with_and_without_vertex_colors() {
        use super::*;
        let material = BladeRippleMaterial {
            params: BladeRippleParams {
                time: 0.0,
                impact: 0.0,
                color: Vec3::ONE,
            },
        };
        assert!(material.reads_view_transmission_texture());
        assert_eq!(material.alpha_mode(), AlphaMode::Opaque);
        for vertex_colors in [false, true] {
            let source = include_str!("blade_ripple.wgsl")
                .replace("#import bevy_pbr::forward_io::VertexOutput;", "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(2) uv: vec2<f32>, @location(5) color: vec4<f32>, };")
                .replace("#import bevy_pbr::mesh_view_bindings::{view_transmission_texture, view_transmission_sampler};", "@group(0) @binding(24) var view_transmission_texture: texture_2d<f32>; @group(0) @binding(25) var view_transmission_sampler: sampler;")
                .replace("#{MATERIAL_BIND_GROUP}", "3")
                .replace("#ifdef VERTEX_COLORS", "")
                .replace("#endif", "");
            let source = if vertex_colors {
                source
            } else {
                source.replace("strength = in.color.r;", "")
            };
            let module = naga::front::wgsl::parse_str(&source).expect("refraction shader parses");
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .expect("refraction shader validates");
        }
    }

    #[test]
    fn blade_ripple_shader_uses_bevy_material_bind_group() {
        let shader = include_str!("blade_ripple.wgsl");
        assert!(shader.contains("@group(#{MATERIAL_BIND_GROUP}) @binding(0)"));
        assert!(
            !shader.contains("@group(2)"),
            "Bevy reserves bind group 2 for per-mesh buffers"
        );
    }
}

fn inactive_trail_mesh() -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0; 3]; 3])
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 3])
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0; 2]; 3])
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0; 4]; 3])
    .with_inserted_indices(Indices::U32(vec![0, 1, 2]))
}

fn setup_weapon_trail(
    mut commands: Commands,
    mut runtime: ResMut<WeaponTrailRuntime>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    mut ripple_materials: ResMut<Assets<BladeRippleMaterial>>,
) {
    runtime.now = 0.0;
    runtime.histories.clear();
    runtime.wakes.clear();
    let mesh = meshes.add(inactive_trail_mesh());
    let material = materials.add(GmdlMaterial::new(
        GmdlMaterialParams {
            opacity: 1.0,
            alpha_cutoff: 0.0,
            mode: GmdlMode::Add.as_u32(),
            has_texture: 0,
            uv_offset: Vec2::ZERO,
        },
        None,
        true,
        GmdlMode::Add,
    ));
    let entity = commands
        .spawn((
            WeaponTrailRibbon,
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
    let ripple_mesh = meshes.add(inactive_trail_mesh());
    let ripple_material = ripple_materials.add(BladeRippleMaterial {
        params: BladeRippleParams {
            time: 0.0,
            impact: 0.0,
            color: Vec3::new(0.42, 0.58, 0.66),
        },
    });
    let ripple_entity = commands
        .spawn((
            BladeRippleRibbon,
            Mesh3d(ripple_mesh.clone()),
            MeshMaterial3d(ripple_material.clone()),
            Visibility::Hidden,
            Transform::IDENTITY,
            NoFrustumCulling,
            NotShadowCaster,
            NotShadowReceiver,
        ))
        .id();
    runtime.ripple_entity = Some(ripple_entity);
    runtime.ripple_mesh = Some(ripple_mesh);
    runtime.ripple_material = Some(ripple_material);
}

fn cleanup_weapon_trail(
    mut commands: Commands,
    mut runtime: ResMut<WeaponTrailRuntime>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    mut ripple_materials: ResMut<Assets<BladeRippleMaterial>>,
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
    if let Some(entity) = runtime.ripple_entity.take() {
        commands.entity(entity).despawn();
    }
    if let Some(mesh) = runtime.ripple_mesh.take() {
        meshes.remove(mesh.id());
    }
    if let Some(material) = runtime.ripple_material.take() {
        ripple_materials.remove(material.id());
    }
    runtime.now = 0.0;
    runtime.histories.clear();
    runtime.wakes.clear();
}

fn part_is_current(
    fighter: &Fighter,
    visual_form: u8,
    weapon: u8,
    slot: u8,
    variant: u8,
    forms: Option<&[u8]>,
) -> bool {
    fighter.weapon_slot == slot
        && fighter.weapon == weapon
        && fighter.weapon_variant == variant
        && forms.is_none_or(|forms| forms.contains(&visual_form))
}

fn endpoint_slot(part: &mut PartEndpoints, name: &str, position: Vec3) {
    match name {
        "d_wpnrs" => part.right_start = Some(position),
        "d_wpnre" => part.right_end = Some(position),
        "d_wpnls" => part.left_start = Some(position),
        "d_wpnle" => part.left_end = Some(position),
        _ => {}
    }
}

fn fallback_edge(
    geometry: &WeaponCollisionGeometry,
    world: &GlobalTransform,
) -> Option<(Vec3, Vec3)> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for bounds in &geometry.boxes {
        let center = Vec3::from_array(bounds.center);
        let half = Vec3::from_array(bounds.half_size);
        for x in [-half.x, half.x] {
            for y in [-half.y, half.y] {
                for z in [-half.z, half.z] {
                    let point = world.transform_point(center + Vec3::new(x, y, z));
                    min = min.min(point);
                    max = max.max(point);
                }
            }
        }
    }
    if !min.is_finite() {
        return None;
    }
    let size = max - min;
    let center = (min + max) * 0.5;
    if size.y >= size.x && size.y >= size.z {
        Some((
            Vec3::new(center.x, min.y, center.z),
            Vec3::new(center.x, max.y, center.z),
        ))
    } else if size.z >= size.x && size.z >= size.y {
        Some((
            Vec3::new(center.x, center.y, min.z),
            Vec3::new(center.x, center.y, max.z),
        ))
    } else {
        Some((
            Vec3::new(min.x, center.y, center.z),
            Vec3::new(max.x, center.y, center.z),
        ))
    }
}

fn active_drag(
    pose: &meteor_asset::pos::Pose,
    frame: f32,
) -> Option<&meteor_asset::pos::DragBlock> {
    pose.drags.iter().fold(None, |active, drag| {
        let inside = drag
            .start
            .zip(drag.end)
            .is_some_and(|(start, end)| frame >= start as f32 && frame <= end as f32);
        if inside {
            Some(drag)
        } else {
            active
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn update_weapon_trail(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    settings: Res<crate::app_state::Settings>,
    real: Res<Time<Real>>,
    eligibility: Query<(
        &crate::components::fighter::Health,
        &crate::components::fighter::DeathRespawn,
        &crate::components::fighter::FighterHitReaction,
        &crate::components::fighter::EntryBlend,
    )>,
    assets: Option<Res<CharacterAssetsSet>>,
    mut runtime: ResMut<WeaponTrailRuntime>,
    fighters: Query<(Entity, &Fighter, &PoseRuntime, &WeaponForm)>,
    endpoints: Query<(&WeaponEndpoint, &GlobalTransform)>,
    collision: Query<(&WeaponCollisionGeometry, &GlobalTransform)>,
    mut ribbons: Query<&mut Visibility, With<WeaponTrailRibbon>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    if dt <= 0.0 {
        return;
    }
    runtime.now += dt;
    let now = runtime.now;
    let cutoff = now - DRAG_HISTORY_SECONDS;
    runtime.histories.retain(|_, history| {
        history.retain(|sample| sample.time >= cutoff);
        !history.is_empty()
    });

    let Some(assets) = assets else {
        hide_ribbon(&runtime, &mut ribbons);
        runtime.wakes.clear();
        return;
    };
    let mut fighter_state = HashMap::new();
    let mut drags = HashMap::new();
    let mut attack_windows = HashMap::new();
    for (entity, fighter, pose, form) in &fighters {
        fighter_state.insert(
            entity,
            (
                fighter,
                meteor_combat::combo_engine::weapon_visual_form(
                    fighter.weapon,
                    form.current,
                    pose.pose_id,
                ),
            ),
        );
        let Some(character) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        let Some(definition) = character.poses.get(&pose.pose_id) else {
            continue;
        };
        if settings.hit_effects
            && eligibility
                .get(entity)
                .is_ok_and(|(health, death, reaction, blend)| {
                    health.current > 0.0
                        && !death.active
                        && reaction.current.is_none()
                        && !crate::plugins::combat_plugin::entry_blend_is_active(
                            blend.duration,
                            blend.pose_id,
                            pose.pose_id,
                        )
                })
        {
            if let Some((index, _)) =
                crate::plugins::combat_plugin::active_attack_window(pose.frame, &definition.attacks)
            {
                attack_windows.insert(entity, (pose.pose_instance_id, index));
            }
        }
        if let Some(drag) = active_drag(definition, pose.frame).filter(|drag| drag.time > 0.0) {
            drags.insert(entity, drag);
        }
    }

    let mut parts: HashMap<TrailKey, PartEndpoints> = HashMap::new();
    for (endpoint, world) in &endpoints {
        let Some((fighter, form)) = fighter_state.get(&endpoint.fighter) else {
            continue;
        };
        if !part_is_current(
            fighter,
            *form,
            endpoint.weapon,
            endpoint.slot,
            endpoint.variant,
            endpoint.forms.as_deref(),
        ) {
            continue;
        }
        let key = TrailKey {
            fighter: endpoint.fighter,
            slot: endpoint.slot,
            part: endpoint.part.clone(),
        };
        let part = parts.entry(key).or_default();
        part.left = endpoint.left;
        endpoint_slot(part, &endpoint.name, world.translation());
    }

    let mut edges: HashMap<TrailKey, (Vec3, Vec3)> = parts
        .into_iter()
        .filter_map(|(key, part)| part.edge().map(|edge| (key, edge)))
        .collect();
    for (geometry, world) in &collision {
        let Some((fighter, form)) = fighter_state.get(&geometry.fighter) else {
            continue;
        };
        if !part_is_current(
            fighter,
            *form,
            geometry.weapon,
            geometry.slot,
            geometry.variant,
            geometry.forms.as_deref(),
        ) {
            continue;
        }
        let key = TrailKey {
            fighter: geometry.fighter,
            slot: geometry.slot,
            part: geometry.part.clone(),
        };
        if let std::collections::hash_map::Entry::Vacant(entry) = edges.entry(key) {
            if let Some(edge) = fallback_edge(geometry, world) {
                entry.insert(edge);
            }
        }
    }

    let mut active = Vec::new();
    for (key, (base, tip)) in edges {
        if base.distance_squared(tip) < 1e-6 {
            continue;
        }
        if !runtime.histories.contains_key(&key) && runtime.histories.len() >= MAX_TRAIL_STREAMS {
            continue;
        }
        let history = runtime.histories.entry(key.clone()).or_default();
        let previous = history.last().copied();
        let sample = EdgeSample {
            time: now,
            base,
            tip,
        };
        if history.last().is_some_and(|latest| latest.time == now) {
            if let Some(latest) = history.last_mut() {
                *latest = sample;
            }
        } else {
            history.push(sample);
        }
        history.retain(|sample| sample.time >= cutoff);
        record_ripple(
            &mut runtime.wakes,
            key.clone(),
            attack_windows.get(&key.fighter).copied(),
            previous,
            sample,
            real.elapsed_secs(),
        );
        if let Some(drag) = drags.get(&key.fighter) {
            active.push((key, *drag));
        }
    }

    let (positions, colors, indices) = build_ribbons(&runtime.histories, &active, now);
    let Some(mesh_handle) = runtime.mesh.as_ref() else {
        return;
    };

    if !indices.is_empty() {
        if let Some(mut mesh) = meshes.get_mut(mesh_handle) {
            let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
            let uvs = vec![[0.0, 0.0]; positions.len()];
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
            mesh.insert_indices(Indices::U32(indices.clone()));
        }
    }
    if let Some(entity) = runtime.entity {
        if let Ok(mut visibility) = ribbons.get_mut(entity) {
            *visibility = if indices.is_empty() {
                Visibility::Hidden
            } else {
                Visibility::Visible
            };
        }
    }
}

fn update_blade_ripples(
    real: Res<Time<Real>>,
    settings: Res<crate::app_state::Settings>,
    impact: Option<Res<crate::plugins::impact_fx_plugin::ImpactSimulationScale>>,
    mut runtime: ResMut<WeaponTrailRuntime>,
    mut ripples: Query<&mut Visibility, (With<BladeRippleRibbon>, Without<WeaponTrailRibbon>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut ripple_materials: ResMut<Assets<BladeRippleMaterial>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
) {
    let wall_now = real.elapsed_secs();
    if !settings.hit_effects {
        runtime.wakes.clear();
    }
    let (ripple_positions, ripple_uvs, ripple_colors, ripple_indices) =
        build_wakes(&mut runtime.wakes, wall_now);
    if !ripple_indices.is_empty() {
        if let Some(handle) = runtime.ripple_mesh.as_ref() {
            if let Some(mut mesh) = meshes.get_mut(handle) {
                let normals = vec![[0.0, 1.0, 0.0]; ripple_positions.len()];
                mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, ripple_positions);
                mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
                mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, ripple_uvs);
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, ripple_colors);
                mesh.insert_indices(Indices::U32(ripple_indices.clone()));
            }
        }
    }
    if let Some(handle) = runtime.ripple_material.as_ref() {
        if let Some(mut material) = ripple_materials.get_mut(handle) {
            material.params.time = impact.as_deref().map_or(0.0, |impact| impact.wall_time);
            material.params.impact = impact.as_deref().map_or(0.0, |impact| impact.ripple_impact);
        }
    }
    let ripple_active = settings.hit_effects && !ripple_indices.is_empty();
    if let Some(mut material) = runtime
        .material
        .as_ref()
        .and_then(|handle| materials.get_mut(handle))
    {
        if material.weapon_refraction_source != ripple_active {
            material.weapon_refraction_source = ripple_active;
        }
    }
    if let Some(entity) = runtime.ripple_entity {
        if let Ok(mut visibility) = ripples.get_mut(entity) {
            *visibility = if ripple_active {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
        }
    }
}

fn hide_ribbon(
    runtime: &WeaponTrailRuntime,
    ribbons: &mut Query<&mut Visibility, With<WeaponTrailRibbon>>,
) {
    if let Some(entity) = runtime.entity {
        if let Ok(mut visibility) = ribbons.get_mut(entity) {
            *visibility = Visibility::Hidden;
        }
    }
}

fn hide_ripple(
    runtime: &WeaponTrailRuntime,
    ripples: &mut Query<&mut Visibility, (With<BladeRippleRibbon>, Without<WeaponTrailRibbon>)>,
) {
    if let Some(entity) = runtime.ripple_entity {
        if let Ok(mut visibility) = ripples.get_mut(entity) {
            *visibility = Visibility::Hidden;
        }
    }
}

fn sample_edge_history(history: &[EdgeSample], time: f32) -> Option<(Vec3, Vec3)> {
    let first = history.first()?;
    if time <= first.time {
        return Some((first.base, first.tip));
    }
    let last = history.last()?;
    if time >= last.time {
        return Some((last.base, last.tip));
    }
    let high = history.partition_point(|sample| sample.time <= time);
    let low = high - 1;
    let span = history[high].time - history[low].time;
    let t = if span > 0.0 {
        (time - history[low].time) / span
    } else {
        0.0
    };
    Some((
        history[low].base.lerp(history[high].base, t),
        history[low].tip.lerp(history[high].tip, t),
    ))
}

fn ripple_strength(sections: &[(Vec3, Vec3)], lookback: f32) -> f32 {
    if sections.len() < 2 || lookback <= 0.0 {
        return 0.0;
    }
    let blade_length = sections[0].0.distance(sections[0].1);
    if blade_length <= 1e-4 {
        return 0.0;
    }
    let path: f32 = sections
        .windows(2)
        .map(|pair| pair[1].1.distance(pair[0].1))
        .sum();
    let speed = path / lookback / blade_length;
    if speed <= RIPPLE_SPEED_MIN {
        return 0.0;
    }
    if speed >= RIPPLE_SPEED_MAX {
        return 1.0;
    }
    let t = (speed - RIPPLE_SPEED_MIN) / (RIPPLE_SPEED_MAX - RIPPLE_SPEED_MIN);
    t * t * (3.0 - 2.0 * t)
}

type RippleBuffers = (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 4]>, Vec<u32>);

#[cfg(test)]
fn build_ripple(histories: &HashMap<TrailKey, Vec<EdgeSample>>, now: f32) -> RippleBuffers {
    build_ripple_sections(
        histories
            .values()
            .map(|history| (history.as_slice(), now, 1.0)),
    )
}

fn build_ripple_sections<'a>(
    histories: impl Iterator<Item = (&'a [EdgeSample], f32, f32)>,
) -> RippleBuffers {
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut colors = Vec::new();
    let mut indices = Vec::new();
    let mut streams = 0;
    for (history, now, opacity) in histories {
        if streams >= RIPPLE_MAX_STREAMS {
            break;
        }
        let Some(first) = history.first() else {
            continue;
        };
        let lookback = (now - first.time).min(RIPPLE_LOOKBACK);
        if history.len() < 2 || lookback <= 0.0 {
            continue;
        }
        let sections: Vec<(Vec3, Vec3)> = (0..=RIPPLE_SEGMENTS)
            .filter_map(|index| {
                let sample_time = now - lookback * index as f32 / RIPPLE_SEGMENTS as f32;
                sample_edge_history(history, sample_time)
            })
            .collect();
        if sections.len() != RIPPLE_SEGMENTS + 1 {
            continue;
        }
        let speed_strength = ripple_strength(&sections, lookback);

        let strength = if speed_strength > 0.0 {
            (0.55 + speed_strength * 0.45) * opacity
        } else {
            0.0
        };
        if strength <= 0.0 {
            continue;
        }
        let first_vertex = positions.len() as u32;
        for (index, (base, tip)) in sections.iter().enumerate() {
            let overhang = (*tip - *base) * RIPPLE_EDGE_OVERSHOOT;
            let age = index as f32 / RIPPLE_SEGMENTS as f32;
            positions.extend([
                (*base - (*tip - *base) * RIPPLE_BASE_OVERSHOOT).to_array(),
                (*tip + overhang).to_array(),
            ]);
            uvs.extend([[age, 0.0], [age, 1.0]]);

            colors.extend([[strength, opacity, 0.0, 1.0]; 2]);
        }
        for index in 0..RIPPLE_SEGMENTS as u32 {
            let quad = first_vertex + index * 2;
            indices.extend([quad, quad + 1, quad + 3, quad, quad + 3, quad + 2]);
        }
        streams += 1;
    }
    (positions, uvs, colors, indices)
}

fn select_control_points(history: &[EdgeSample]) -> Option<[EdgeSample; DRAG_CONTROL_POINTS]> {
    let newest = *history.last()?;
    let mut newest_to_oldest = vec![newest];
    let mut selected_time = newest.time;
    for sample in history.iter().rev().skip(1) {
        if selected_time - sample.time > DRAG_CONTROL_SECONDS {
            newest_to_oldest.push(*sample);
            selected_time = sample.time;
            if newest_to_oldest.len() == DRAG_CONTROL_POINTS {
                break;
            }
        }
    }
    if newest_to_oldest.len() != DRAG_CONTROL_POINTS {
        return None;
    }
    newest_to_oldest.reverse();
    newest_to_oldest.try_into().ok()
}

fn spline_second(
    xs: &[f32; DRAG_CONTROL_POINTS],
    ys: &[f32; DRAG_CONTROL_POINTS],
) -> [f32; DRAG_CONTROL_POINTS] {
    let mut second = [0.0; DRAG_CONTROL_POINTS];
    let mut work = [0.0; DRAG_CONTROL_POINTS - 1];
    for i in 1..DRAG_CONTROL_POINTS - 1 {
        let span = xs[i + 1] - xs[i - 1];
        let sigma = (xs[i] - xs[i - 1]) / span;
        let pivot = sigma * second[i - 1] + 2.0;
        second[i] = (sigma - 1.0) / pivot;
        let right_slope = (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]);
        let left_slope = (ys[i] - ys[i - 1]) / (xs[i] - xs[i - 1]);
        work[i] = (6.0 * (right_slope - left_slope) / span - sigma * work[i - 1]) / pivot;
    }
    for i in (0..DRAG_CONTROL_POINTS - 1).rev() {
        second[i] = second[i] * second[i + 1] + work[i];
    }
    second
}

fn spline_value(
    xs: &[f32; DRAG_CONTROL_POINTS],
    ys: &[f32; DRAG_CONTROL_POINTS],
    second: &[f32; DRAG_CONTROL_POINTS],
    query: f32,
) -> f32 {
    if query <= xs[0] {
        return ys[0];
    }
    if query >= xs[DRAG_CONTROL_POINTS - 1] {
        return ys[DRAG_CONTROL_POINTS - 1];
    }
    let high = xs.partition_point(|value| *value <= query);
    let low = high - 1;
    let width = xs[high] - xs[low];
    let a = (xs[high] - query) / width;
    let b = (query - xs[low]) / width;
    a * ys[low]
        + b * ys[high]
        + ((a * a * a - a) * second[low] + (b * b * b - b) * second[high]) * width * width / 6.0
}

fn vector_spline(controls: &[EdgeSample; DRAG_CONTROL_POINTS], tip: bool, query: f32) -> Vec3 {
    let xs = controls.map(|sample| sample.time);
    let values = controls.map(|sample| if tip { sample.tip } else { sample.base });
    let x = values.map(|value| value.x);
    let y = values.map(|value| value.y);
    let z = values.map(|value| value.z);
    Vec3::new(
        spline_value(&xs, &x, &spline_second(&xs, &x), query),
        spline_value(&xs, &y, &spline_second(&xs, &y), query),
        spline_value(&xs, &z, &spline_second(&xs, &z), query),
    )
}

type RibbonBuffers = (Vec<[f32; 3]>, Vec<[f32; 4]>, Vec<u32>);

fn build_ribbons(
    histories: &HashMap<TrailKey, Vec<EdgeSample>>,
    active: &[(TrailKey, &meteor_asset::pos::DragBlock)],
    now: f32,
) -> RibbonBuffers {
    let mut positions = Vec::new();
    let mut colors = Vec::new();
    let mut indices = Vec::new();
    for (key, drag) in active {
        let Some(history) = histories.get(key) else {
            continue;
        };
        let Some(controls) = select_control_points(history) else {
            continue;
        };
        for segment in 0..DRAG_SEGMENTS {
            let front_time = now - drag.time * segment as f32 / DRAG_SEGMENTS as f32;
            let back_time = now - drag.time * (segment + 1) as f32 / DRAG_SEGMENTS as f32;
            let front_base = vector_spline(&controls, false, front_time);
            let back_base = vector_spline(&controls, false, back_time);
            let back_tip = vector_spline(&controls, true, back_time);
            let front_tip = vector_spline(&controls, true, front_time);
            let front_alpha =
                (255 * (DRAG_SEGMENTS - segment) / (2 * DRAG_SEGMENTS)) as f32 / 255.0;
            let back_alpha =
                (255 * (DRAG_SEGMENTS - segment - 1) / (2 * DRAG_SEGMENTS)) as f32 / 255.0;
            let rgb = drag.color.map(|channel| channel as f32 / 255.0);
            let first = positions.len() as u32;
            positions.extend([
                front_base.to_array(),
                back_base.to_array(),
                back_tip.to_array(),
                front_tip.to_array(),
            ]);
            colors.extend([
                [rgb[0], rgb[1], rgb[2], front_alpha],
                [rgb[0], rgb[1], rgb[2], back_alpha],
                [rgb[0], rgb[1], rgb[2], back_alpha],
                [rgb[0], rgb[1], rgb[2], front_alpha],
            ]);
            indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
        }
    }
    (positions, colors, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inactive_ribbon_keeps_nonzero_gpu_ranges() {
        let mesh = inactive_trail_mesh();
        assert!(mesh.get_vertex_buffer_size() > 0);
        assert!(mesh
            .get_index_buffer_bytes()
            .is_some_and(|bytes| !bytes.is_empty()));
    }

    fn fighter() -> Fighter {
        Fighter {
            character_id: 0,
            player_slot: 0,
            weapon: 6,
            weapon_slots: [6, 6],
            weapon_slot: 1,
            weapon_slot_variants: [0, 2],
            weapon_variant: 2,
        }
    }

    fn sample(time: f32) -> EdgeSample {
        EdgeSample {
            time,
            base: Vec3::new(time, 0.0, 0.0),
            tip: Vec3::new(time, 2.0, 0.0),
        }
    }

    #[test]
    fn controls_require_strict_spacing_and_return_oldest_first() {
        let history = [
            sample(0.0),
            sample(0.03),
            sample(0.031),
            sample(0.062),
            sample(0.093),
            sample(0.124),
        ];
        let selected = select_control_points(&history).expect("five spaced controls");
        assert_eq!(
            selected.map(|sample| sample.time),
            [0.0, 0.031, 0.062, 0.093, 0.124]
        );
    }

    #[test]
    fn trail_uses_only_the_active_preheated_slot() {
        let fighter = fighter();
        let form = WeaponForm { current: 1 };
        assert!(part_is_current(&fighter, form.current, 6, 1, 2, Some(&[1])));
        assert!(!part_is_current(
            &fighter,
            form.current,
            6,
            0,
            0,
            Some(&[1])
        ));
        assert!(!part_is_current(
            &fighter,
            form.current,
            6,
            1,
            0,
            Some(&[1])
        ));
        assert!(!part_is_current(
            &fighter,
            form.current,
            6,
            1,
            2,
            Some(&[0])
        ));
    }

    #[test]
    fn natural_spline_clamps_and_preserves_linear_motion() {
        let controls = [
            sample(0.0),
            sample(0.04),
            sample(0.08),
            sample(0.12),
            sample(0.16),
        ];
        assert_eq!(vector_spline(&controls, false, -1.0), controls[0].base);
        assert_eq!(vector_spline(&controls, true, 1.0), controls[4].tip);
        let middle = vector_spline(&controls, false, 0.1);
        assert!((middle.x - 0.1).abs() < 1e-5);
    }

    #[test]
    fn ribbon_matches_web_vertex_index_and_alpha_layout() {
        let key = TrailKey {
            fighter: Entity::from_raw_u32(7).expect("valid test entity index"),
            slot: 0,
            part: "w0_0".to_string(),
        };
        let history = vec![
            sample(0.0),
            sample(0.04),
            sample(0.08),
            sample(0.12),
            sample(0.16),
        ];
        let histories = HashMap::from([(key.clone(), history)]);
        let drag = meteor_asset::pos::DragBlock {
            start: Some(1),
            end: Some(2),
            time: 0.1,
            color: [255, 128, 0],
        };
        let (positions, colors, indices) = build_ribbons(&histories, &[(key, &drag)], 0.16);
        assert_eq!(positions.len(), DRAG_SEGMENTS * 4);
        assert_eq!(colors.len(), DRAG_SEGMENTS * 4);
        assert_eq!(indices.len(), DRAG_SEGMENTS * 6);
        assert_eq!(&indices[..6], &[0, 1, 2, 0, 2, 3]);
        assert_eq!(colors[0][..3], [1.0, 128.0 / 255.0, 0.0]);
        assert_eq!(colors[0][3], 127.0 / 255.0);
        assert_eq!(colors[1][3], 121.0 / 255.0);
    }

    #[test]
    fn left_part_prefers_left_endpoints_then_falls_back_to_right() {
        let part = PartEndpoints {
            left: true,
            right_start: Some(Vec3::X),
            right_end: Some(Vec3::Y),
            left_start: Some(Vec3::Z),
            left_end: Some(Vec3::ONE),
        };
        assert_eq!(part.edge(), Some((Vec3::Z, Vec3::ONE)));
    }

    #[test]
    fn ripple_history_interpolates_and_clamps_like_web() {
        let history = [sample(0.0), sample(0.1), sample(0.2)];
        assert_eq!(
            sample_edge_history(&history, -1.0),
            Some((history[0].base, history[0].tip))
        );
        assert_eq!(
            sample_edge_history(&history, 1.0),
            Some((history[2].base, history[2].tip))
        );
        let (base, tip) = sample_edge_history(&history, 0.15).expect("interpolated edge");
        assert!((base.x - 0.15).abs() < 1e-6);
        assert!((tip.x - 0.15).abs() < 1e-6);
    }

    #[test]
    fn ripple_strength_uses_blade_lengths_per_second_gate() {
        let slow = vec![(Vec3::ZERO, Vec3::Y), (Vec3::ZERO, Vec3::Y + Vec3::X * 0.1)];
        assert_eq!(ripple_strength(&slow, RIPPLE_LOOKBACK), 0.0);
        let fast = vec![(Vec3::ZERO, Vec3::Y), (Vec3::ZERO, Vec3::Y + Vec3::X * 2.0)];
        assert_eq!(ripple_strength(&fast, RIPPLE_LOOKBACK), 1.0);
    }

    #[test]
    fn empty_histories_produce_no_ripple() {
        let empty: HashMap<TrailKey, Vec<EdgeSample>> = HashMap::new();
        let (_, _, _, indices) = build_ripple(&empty, 0.14);
        assert!(indices.is_empty());
    }

    #[test]
    fn fast_blade_history_produces_ripple_geometry() {
        let key = TrailKey {
            fighter: Entity::from_raw_u32(2).expect("valid test entity index"),
            slot: 0,
            part: "blade".into(),
        };
        let history: Vec<EdgeSample> = (0..=14)
            .map(|index| EdgeSample {
                time: index as f32 * 0.01,
                base: Vec3::ZERO,
                tip: Vec3::new(index as f32 * 0.2, 1.0, 0.0),
            })
            .collect();
        let (_, _, _, indices) = build_ripple(&HashMap::from([(key, history)]), 0.14);
        assert!(!indices.is_empty());
    }

    #[test]
    fn ripple_builds_web_quad_strip_layout() {
        let key = TrailKey {
            fighter: Entity::from_raw_u32(1).expect("valid test entity index"),
            slot: 0,
            part: "blade".into(),
        };
        let history: Vec<EdgeSample> = (0..=14)
            .map(|index| {
                let time = index as f32 * 0.01;
                EdgeSample {
                    time,
                    base: Vec3::ZERO,
                    tip: Vec3::new(index as f32 * 0.1, 1.0, 0.0),
                }
            })
            .collect();
        let (positions, uvs, colors, indices) =
            build_ripple(&HashMap::from([(key, history)]), 0.14);
        assert_eq!(positions.len(), (RIPPLE_SEGMENTS + 1) * 2);
        assert_eq!(uvs.len(), positions.len());
        assert_eq!(colors.len(), positions.len());
        assert_eq!(indices.len(), RIPPLE_SEGMENTS * 6);
        assert_eq!(&indices[..6], &[0, 1, 3, 0, 3, 2]);

        let tip = Vec3::new(1.4, 1.0, 0.0);
        assert!(Vec3::from_array(positions[0]).distance(-tip * 0.05) < 1e-5);
        assert!(Vec3::from_array(positions[1]).distance(tip * 1.6) < 1e-5);
    }
}
