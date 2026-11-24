use super::{
    inactive_trail_mesh, select_control_points, vector_spline, EdgeSample, TrailKey, DRAG_SEGMENTS,
};
use crate::plugins::graphics_pack::config::WeaponTrail;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::material::AlphaMode;
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::mesh::{Indices, MeshVertexBufferLayoutRef};
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, RenderPipelineDescriptor,
    ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::{Shader, ShaderDefVal, ShaderRef};
use std::collections::HashMap;

pub(super) const ENHANCED_TRAIL_SHADER_HANDLE: Handle<Shader> =
    bevy::asset::uuid_handle!("5b11ad3e-71aa-4001-8c61-115e77aa0002");

#[derive(ShaderType, Clone, Default)]
pub(super) struct TrailParams {
    pub misc: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub(super) struct EnhancedTrailMaterial {
    #[uniform(0)]
    pub params: TrailParams,
    #[texture(1)]
    #[sampler(2)]
    pub texture: Option<Handle<Image>>,
}

impl Material for EnhancedTrailMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(ENHANCED_TRAIL_SHADER_HANDLE)
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
        _key: MaterialPipelineKey<Self>,
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
            let additive = BlendComponent {
                src_factor: BlendFactor::One,
                dst_factor: BlendFactor::One,
                operation: BlendOperation::Add,
            };
            for target in fragment.targets.iter_mut().flatten() {
                target.blend = Some(BlendState {
                    color: additive,
                    alpha: additive,
                });
            }
        }
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

#[derive(Component)]
pub(super) struct EnhancedTrailRibbon;

pub(super) struct StyledRibbon {
    pub entity: Entity,
    pub mesh: Handle<Mesh>,
    pub material: Handle<EnhancedTrailMaterial>,
    pub texture: Option<Handle<Image>>,
}

#[derive(Default)]
pub(super) struct EnhancedRibbons {
    pub by_style: HashMap<u8, StyledRibbon>,
    pub generation: u64,
}

impl EnhancedRibbons {
    pub fn despawn_all(&mut self, commands: &mut Commands) {
        for ribbon in self.by_style.drain().map(|(_, ribbon)| ribbon) {
            commands.entity(ribbon.entity).despawn();
        }
    }

    pub fn ensure(
        &mut self,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<EnhancedTrailMaterial>,
        style_key: u8,
        texture: Option<Handle<Image>>,
    ) -> &StyledRibbon {
        let needs_new = self
            .by_style
            .get(&style_key)
            .is_none_or(|ribbon| ribbon.texture != texture);
        if needs_new {
            if let Some(old) = self.by_style.remove(&style_key) {
                commands.entity(old.entity).despawn();
            }
            let mesh = meshes.add(inactive_trail_mesh());
            let material = materials.add(EnhancedTrailMaterial {
                params: TrailParams {
                    misc: Vec4::new(0.0, f32::from(texture.is_some()), 0.0, 0.0),
                },
                texture: texture.clone(),
            });
            let entity = commands
                .spawn((
                    EnhancedTrailRibbon,
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    Visibility::Hidden,
                    Transform::IDENTITY,
                    NoFrustumCulling,
                    NotShadowCaster,
                    NotShadowReceiver,
                ))
                .id();
            self.by_style.insert(
                style_key,
                StyledRibbon {
                    entity,
                    mesh,
                    material,
                    texture,
                },
            );
        }
        &self.by_style[&style_key]
    }
}

pub(super) struct EnhancedBuffers {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub uvs_b: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl EnhancedBuffers {
    pub fn new() -> Self {
        Self {
            positions: Vec::new(),
            uvs: Vec::new(),
            uvs_b: Vec::new(),
            colors: Vec::new(),
            indices: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn write(&self, mesh: &mut Mesh) {
        let normals = vec![[0.0, 1.0, 0.0]; self.positions.len()];
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, self.uvs_b.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, self.colors.clone());
        mesh.insert_indices(Indices::U32(self.indices.clone()));
    }
}

pub(super) fn gradient(style: &WeaponTrail, age: f32) -> Vec3 {
    let [head, mid, tail] = style.gradient.map(Vec3::from_array);
    if age < 0.5 {
        head.lerp(mid, age * 2.0)
    } else {
        mid.lerp(tail, (age - 0.5) * 2.0)
    }
}

pub(super) fn append_ribbon(
    out: &mut EnhancedBuffers,
    history: &[EdgeSample],
    drag: &meteor_asset::pos::DragBlock,
    style: &WeaponTrail,
    now: f32,
) {
    let Some(controls) = select_control_points(history) else {
        return;
    };
    let window = drag.time * style.length_scale;
    let pose = if style.use_pose_color {
        Vec3::from_array(drag.color.map(|channel| channel as f32 / 255.0))
    } else {
        Vec3::ONE
    };
    for segment in 0..DRAG_SEGMENTS {
        let front_age = segment as f32 / DRAG_SEGMENTS as f32;
        let back_age = (segment + 1) as f32 / DRAG_SEGMENTS as f32;
        let front_time = now - window * front_age;
        let back_time = now - window * back_age;
        let mut front_base = vector_spline(&controls, false, front_time);
        let mut back_base = vector_spline(&controls, false, back_time);
        let mut back_tip = vector_spline(&controls, true, back_time);
        let mut front_tip = vector_spline(&controls, true, front_time);
        if style.tip_overshoot > 0.0 {
            front_tip += (front_tip - front_base) * style.tip_overshoot;
            back_tip += (back_tip - back_base) * style.tip_overshoot;
        }

        let front_alpha = (255 * (DRAG_SEGMENTS - segment) / (2 * DRAG_SEGMENTS)) as f32 / 255.0;
        let back_alpha = (255 * (DRAG_SEGMENTS - segment - 1) / (2 * DRAG_SEGMENTS)) as f32 / 255.0;
        let shade = |age: f32| -> Vec3 {
            gradient(style, age) * pose * style.intensity * (1.0 - age).powf(style.tail_power)
        };
        let front_rgb = shade(front_age).to_array();
        let back_rgb = shade(back_age).to_array();
        let first = out.positions.len() as u32;

        let shrink = 0.02;
        front_base += (front_tip - front_base) * shrink;
        back_base += (back_tip - back_base) * shrink;
        out.positions.extend([
            front_base.to_array(),
            back_base.to_array(),
            back_tip.to_array(),
            front_tip.to_array(),
        ]);
        out.uvs.extend([
            [front_age, 0.0],
            [back_age, 0.0],
            [back_age, 1.0],
            [front_age, 1.0],
        ]);
        let extra = [style.edge_softness, style.core];
        out.uvs_b.extend([extra; 4]);
        out.colors.extend([
            [front_rgb[0], front_rgb[1], front_rgb[2], front_alpha],
            [back_rgb[0], back_rgb[1], back_rgb[2], back_alpha],
            [back_rgb[0], back_rgb[1], back_rgb[2], back_alpha],
            [front_rgb[0], front_rgb[1], front_rgb[2], front_alpha],
        ]);
        out.indices
            .extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }
}

pub(super) fn build_styled_ribbons<'a>(
    histories: &HashMap<TrailKey, Vec<EdgeSample>>,
    active: impl Iterator<
        Item = (
            &'a TrailKey,
            &'a meteor_asset::pos::DragBlock,
            u8,
            &'a WeaponTrail,
        ),
    >,
    now: f32,
) -> HashMap<u8, EnhancedBuffers> {
    let mut out: HashMap<u8, EnhancedBuffers> = HashMap::new();
    for (key, drag, style_key, style) in active {
        let Some(history) = histories.get(key) else {
            continue;
        };
        let buffers = out.entry(style_key).or_insert_with(EnhancedBuffers::new);
        append_ribbon(buffers, history, drag, style, now);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(time: f32) -> EdgeSample {
        EdgeSample {
            time,
            base: Vec3::new(time, 0.0, 0.0),
            tip: Vec3::new(time, 2.0, 0.0),
        }
    }

    #[test]
    fn styled_ribbon_writes_hdr_colours_and_overshoots_the_tip() {
        let history: Vec<_> = (0..8).map(|i| sample(i as f32 * 0.04)).collect();
        let drag = meteor_asset::pos::DragBlock {
            start: Some(0),
            end: Some(10),
            time: 0.2,
            color: [255, 128, 0],
        };
        let style = WeaponTrail {
            intensity: 4.0,
            tip_overshoot: 0.5,
            gradient: [[1.0; 3]; 3],
            tail_power: 1.0,
            ..default()
        };
        let mut out = EnhancedBuffers::new();
        append_ribbon(&mut out, &history, &drag, &style, 0.28);
        assert_eq!(out.indices.len(), DRAG_SEGMENTS * 6);
        assert_eq!(out.uvs.len(), out.positions.len());
        assert_eq!(out.uvs_b.len(), out.positions.len());

        let head = out.colors[0];
        assert!((head[0] - 4.0).abs() < 1e-4 && (head[1] - 4.0 * 128.0 / 255.0).abs() < 1e-3);
        assert!((head[3] - 127.0 / 255.0).abs() < 1e-6);

        let tip_y = out.positions[3][1];
        assert!((tip_y - 3.0).abs() < 1e-3, "tip y {tip_y}");

        let tail = out.colors[out.colors.len() - 2];
        assert!(tail[0].abs() < 1e-5 && tail[3] == 0.0);
        assert_eq!(out.uvs_b[0], [style.edge_softness, style.core]);
    }

    #[test]
    fn gradient_interpolates_three_stops() {
        let style = WeaponTrail {
            gradient: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            ..default()
        };
        assert_eq!(gradient(&style, 0.0), Vec3::X);
        assert_eq!(gradient(&style, 0.5), Vec3::Y);
        assert_eq!(gradient(&style, 1.0), Vec3::Z);
        assert!((gradient(&style, 0.25) - Vec3::new(0.5, 0.5, 0.0)).length() < 1e-6);
    }

    #[test]
    fn enhanced_shader_validates() {
        let source = include_str!("../weapon_trail_enhanced.wgsl")
            .replace(
                "#import bevy_pbr::forward_io::VertexOutput",
                "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(2) uv: vec2<f32>, @location(3) uv_b: vec2<f32>, @location(5) color: vec4<f32>, };",
            )
            .replace("#{MATERIAL_BIND_GROUP}", "3");
        let module = naga::front::wgsl::parse_str(&source).expect("enhanced trail shader parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("enhanced trail shader validates");
    }
}
