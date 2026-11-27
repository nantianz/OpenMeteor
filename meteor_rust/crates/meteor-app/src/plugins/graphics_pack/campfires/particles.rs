use super::super::fx::{self, FxMaterial, FxParams, SpriteBuffers};
use bevy::{
    camera::visibility::NoFrustumCulling,
    light::{NotShadowCaster, NotShadowReceiver},
    prelude::*,
};
use std::f32::consts::TAU;

#[derive(Clone, Copy)]
enum Layer {
    Flame,
    Smoke,
    Ember,
}

const LAYERS: [Layer; 3] = [Layer::Flame, Layer::Smoke, Layer::Ember];

pub(super) fn materials(materials: &mut Assets<FxMaterial>) -> [Handle<FxMaterial>; 3] {
    LAYERS.map(|layer| {
        let (additive, shape) = match layer {
            Layer::Flame => (true, 1.0),
            Layer::Smoke => (false, 2.0),
            Layer::Ember => (true, 0.0),
        };
        materials.add(FxMaterial {
            params: FxParams {
                misc: Vec4::new(0.0, f32::from(additive), shape, 0.0),
            },
            texture: None,
            additive,
        })
    })
}

#[derive(Component)]
pub(super) struct Plume {
    layer: Layer,
    seed: u32,
    scale: f32,
    pub(super) elapsed: f32,
}

pub(super) fn spawn(
    root: &mut ChildSpawnerCommands,
    meshes: &mut Assets<Mesh>,
    materials: &[Handle<FxMaterial>; 3],
    hearth: Vec3,
    scale: f32,
    seed: u32,
) {
    for (layer, material) in LAYERS.into_iter().zip(materials) {
        let sort_height = match layer {
            Layer::Flame => 0.65,
            Layer::Smoke => 2.65,
            Layer::Ember => 1.1,
        };
        root.spawn((
            Plume {
                layer,
                scale,
                seed,
                elapsed: 0.0,
            },
            Mesh3d(meshes.add(fx::placeholder_mesh())),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(hearth + Vec3::Y * sort_height * scale),
            Visibility::Hidden,
            NoFrustumCulling,
            NotShadowCaster,
            NotShadowReceiver,
        ));
    }
}

impl Plume {
    pub(super) fn sprites(
        &self,
        camera: &GlobalTransform,
        wind: Vec2,
        wake: Vec2,
    ) -> SpriteBuffers {
        let (count, life, sort_height) = match self.layer {
            Layer::Flame => (24, 0.91, 0.65),
            Layer::Smoke => (14, 3.8, 2.65),
            Layer::Ember => (26, 2.3, 1.1),
        };
        let right = camera.right().as_vec3();
        let up = camera.up().as_vec3();
        let forward = camera.forward().as_vec3();
        let wind = Vec3::new(wind.x, 0.0, wind.y);
        let wake = Vec3::new(wake.x, 0.0, wake.y);
        let mut sprites = SpriteBuffers::with_capacity(count);
        for i in 0..count {
            let phase = (i as f32 + self.seed as f32 * 0.49) / count as f32;
            let time = self.elapsed / life + phase;
            let t = time.fract();
            let mut rng = (i as u32 + 1).wrapping_mul(0x9e37_79b9)
                ^ self.seed.wrapping_mul(0x85eb_ca6b)
                ^ (time.floor() as u32).wrapping_mul(0xc2b2_ae35);
            rng |= 1;
            let angle = fx::next_unit(&mut rng) * TAU;
            let radial = Vec3::new(angle.cos(), 0.0, angle.sin());
            let variation = fx::next_unit(&mut rng);
            let twist = fx::next_unit(&mut rng);
            let seed = fx::next_unit(&mut rng) * 32.0 + t * 2.5;
            let fade = (t * 11.0).min(1.0) * (1.0 - t).powf(1.7);
            let start = -Vec3::Y * sort_height;
            let (position, right, up, soft, rgba) = match self.layer {
                Layer::Flame => {
                    let position = start
                        + radial * ((0.09 + variation * 0.5) * (1.0 - 0.54 * t))
                        + Vec3::Y * (0.33 + t * (1.14 + twist * 0.92))
                        + wind * (0.22 * t * t)
                        + wake * (0.44 * t * t);
                    let half = (0.24 + variation * 0.38) * (1.0 - t * 0.67);
                    let color = Vec3::new(1.0, 0.68, 0.20).lerp(Vec3::new(0.92, 0.095, 0.008), t);
                    let skew = right * (0.17 * (angle + t * TAU).sin());
                    (
                        position,
                        right * half,
                        (up + skew) * half * (1.85 + twist),
                        0.43,
                        (color * (1.0 + variation)).extend(fade * 0.63),
                    )
                }
                Layer::Smoke => {
                    let position = start
                        + radial * (0.24 + t * (0.3 + twist))
                        + Vec3::Y * (1.3 + t * (3.2 + twist * 1.6))
                        + wind * (t * (0.9 + t * 1.8))
                        + wake * (0.50 * t);
                    let half = 0.52 + t * (1.2 + variation * 0.8);
                    let spin = angle + t * (twist - 0.5) * 1.8;
                    let (sin, cos) = spin.sin_cos();
                    let color = Vec3::new(0.20, 0.13, 0.095).lerp(Vec3::new(0.20, 0.21, 0.225), t);
                    (
                        position,
                        (right * cos + up * sin) * half,
                        (up * cos - right * sin) * half,
                        0.85,
                        color.extend(fade * (0.18 + variation * 0.14)),
                    )
                }
                Layer::Ember => {
                    let rise = 1.7 + variation * 4.6;
                    let position = start
                        + radial * (0.30 + t * (0.6 + twist * 1.55))
                        + Vec3::Y * (0.28 + t * (rise - 0.80 * t))
                        + wind * (0.95 * t * t)
                        + wake * (0.75 * t * t);
                    let velocity = Vec3::Y * (rise - 1.60 * t)
                        + radial * (0.6 + twist * 1.55)
                        + wind * (1.9 * t)
                        + wake * (1.50 * t);
                    let axis = (velocity - forward * velocity.dot(forward)).normalize_or(up);
                    let half = (0.021 + twist * 0.036) * (1.0 - 0.64 * t);
                    let blink = 0.52 + 0.48 * (t * 32.0 + angle).sin().powi(2);
                    let color = Vec3::new(5.0, 2.2, 0.48).lerp(Vec3::new(2.5, 0.51, 0.06), t);
                    (
                        position,
                        forward.cross(axis) * half,
                        axis * half * (2.4 + variation * 3.7),
                        0.17,
                        color.extend(fade * blink),
                    )
                }
            };
            sprites.push_quad(
                position * self.scale,
                right * self.scale,
                up * self.scale,
                Vec2::ZERO,
                Vec2::ONE,
                [soft * self.scale, seed],
                rgba.to_array(),
            );
        }
        sprites
    }
}
