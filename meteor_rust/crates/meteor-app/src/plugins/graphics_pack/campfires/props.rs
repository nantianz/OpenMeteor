use super::BRAZIER;
use crate::plugins::{
    character_plugin::{SceneCloth, SceneClothCapsule},
    graphics_pack::{
        config::Clothing,
        scene_actors::{live_actors, SceneActors, BODY_HEIGHT, BODY_RADIUS},
    },
};
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::NoFrustumCulling,
    image::{ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::{
        mesh::{Indices, PrimitiveTopology},
        render_resource::{Extent3d, TextureDimension, TextureFormat},
    },
};
use meteor_asset::texture::DecodedImage;
use std::f32::consts::{FRAC_PI_2, TAU};

const BANNER_HEIGHT: f32 = 7.24;
const BANNER_WIDTH: f32 = 2.48;
const BANNER_LENGTH: f32 = 3.62;
pub(super) const COLUMNS: usize = 13;
const ROWS: usize = 16;

pub(super) struct Kit {
    column: Handle<Mesh>,
    tapered: Handle<Mesh>,
    rock: Handle<Mesh>,
    wood: Handle<StandardMaterial>,
    brass: Handle<StandardMaterial>,
    iron: Handle<StandardMaterial>,
    stone: [Handle<StandardMaterial>; 3],
    coal: Handle<StandardMaterial>,
}

impl Kit {
    pub(super) fn new(meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) -> Self {
        let mut material = |rgb: [f32; 3], metallic, roughness| {
            materials.add(StandardMaterial {
                base_color: Color::srgb_from_array(rgb),
                metallic,
                perceptual_roughness: roughness,
                ..default()
            })
        };
        let wood = material([0.23, 0.117, 0.074], 0.0, 0.91);
        let brass = material([0.48, 0.31, 0.15], 0.59, 0.52);
        let iron = material([0.19, 0.16, 0.14], 0.45, 0.77);
        let stone = [
            material([0.27, 0.255, 0.235], 0.0, 0.93),
            material([0.34, 0.32, 0.275], 0.0, 0.93),
            material([0.23, 0.23, 0.21], 0.0, 0.93),
        ];
        let coal = materials.add(StandardMaterial {
            base_color: Color::srgb(0.14, 0.046, 0.026),
            emissive: LinearRgba::rgb(0.45, 0.045, 0.004),
            perceptual_roughness: 1.0,
            ..default()
        });
        Self {
            column: meshes.add(Cylinder::new(1.0, 1.0).mesh().resolution(12)),
            tapered: meshes.add(ConicalFrustum {
                radius_top: 0.82,
                radius_bottom: 1.0,
                height: 1.0,
            }),
            rock: meshes.add(Sphere::new(1.0).mesh().ico(0).unwrap()),
            wood,
            brass,
            iron,
            stone,
            coal,
        }
    }

    pub(super) fn spawn(&self, parent: &mut ChildSpawnerCommands) {
        let mut draw = |mesh: &Handle<Mesh>,
                        material: &Handle<StandardMaterial>,
                        position,
                        scale,
                        rotation| {
            parent.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                Transform {
                    translation: position,
                    rotation,
                    scale,
                },
            ));
        };
        let pole = |y| Vec3::Y * y;
        let round = |radius, height| Vec3::new(radius, height, radius);
        draw(
            &self.tapered,
            &self.stone[0],
            pole(0.21),
            round(0.59, 0.46),
            Quat::IDENTITY,
        );
        draw(
            &self.tapered,
            &self.wood,
            pole(4.03),
            round(0.145, 7.47),
            Quat::IDENTITY,
        );
        for (height, radius, width) in [
            (0.48, 0.29, 0.17),
            (0.75, 0.19, 0.12),
            (1.05, 0.17, 0.08),
            (BANNER_HEIGHT, 0.16, 0.22),
            (7.75, 0.17, 0.12),
        ] {
            draw(
                &self.column,
                &self.brass,
                pole(height),
                round(radius, width),
                Quat::IDENTITY,
            );
        }
        draw(
            &self.rock,
            &self.brass,
            pole(7.93),
            Vec3::new(0.16, 0.25, 0.16),
            Quat::IDENTITY,
        );
        let bar = Quat::from_rotation_z(FRAC_PI_2);
        draw(
            &self.column,
            &self.wood,
            Vec3::new(1.40, BANNER_HEIGHT, 0.0),
            round(0.066, 3.28),
            bar,
        );
        for x in [-0.21, 0.41, 2.80, 3.00] {
            draw(
                &self.column,
                &self.brass,
                Vec3::new(x, BANNER_HEIGHT, 0.0),
                round(0.084, 0.095),
                bar,
            );
        }

        draw(
            &self.column,
            &self.stone[2],
            BRAZIER + pole(0.10),
            round(1.20, 0.28),
            Quat::IDENTITY,
        );
        draw(
            &self.tapered,
            &self.iron,
            BRAZIER + pole(0.46),
            round(0.88, 0.38),
            Quat::from_rotation_x(std::f32::consts::PI),
        );
        draw(
            &self.column,
            &self.brass,
            BRAZIER + pole(0.63),
            round(0.90, 0.075),
            Quat::IDENTITY,
        );
        draw(
            &self.column,
            &self.coal,
            BRAZIER + pole(0.67),
            round(0.74, 0.07),
            Quat::IDENTITY,
        );
        for i in 0..9 {
            let angle = i as f32 * TAU / 9.0;
            let radial = Vec3::new(angle.cos(), 0.0, angle.sin());
            let rotation = Quat::from_rotation_y(-angle);
            draw(
                &self.rock,
                &self.stone[i % 3],
                BRAZIER + radial * 1.11 + pole(0.18),
                Vec3::new(0.32, 0.24 + 0.05 * (i % 3) as f32, 0.44),
                rotation,
            );
            draw(
                &self.rock,
                &self.coal,
                BRAZIER + radial * 0.60 + pole(0.72),
                Vec3::new(0.13, 0.07, 0.10),
                rotation,
            );
        }
        for (i, (x, z, yaw)) in [(-0.25, -0.06, 0.3), (0.20, 0.02, 0.8), (0.0, 0.04, -0.65)]
            .into_iter()
            .enumerate()
        {
            let rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_z(FRAC_PI_2 - 0.16);
            draw(
                &self.column,
                &self.wood,
                BRAZIER + Vec3::new(x, 0.79 + i as f32 * 0.08, z),
                round(0.132, 1.20),
                rotation,
            );
        }
    }
}

#[derive(Component)]
pub(super) struct Banner {
    cloth: SceneCloth,
    pub(super) elapsed: f32,
}

impl Banner {
    pub(super) fn advance(
        &mut self,
        dt: f32,
        wind: Vec3,
        transform: &GlobalTransform,
        actors: &SceneActors,
        mesh: &mut Mesh,
    ) {
        self.elapsed += dt;
        if dt == 0.0 {
            return;
        }
        let cloth = Clothing {
            density: 0.14,
            wind_drag: 0.87,
            motion_limit: 1.45,
            gravity_scale: 0.78,
            bend_compliance: 0.025,
            collision_friction: 0.25,
            damping: 1.8,
            iterations: 5,
            ..default()
        };

        let eddy = (self.elapsed * 3.4).sin() + 0.44 * (self.elapsed * 6.9).sin();
        let wind = wind * 3.35 + Vec3::new(0.0, 0.21, eddy * 0.88) * wind.length();
        let inverse = transform.affine().inverse();
        let radius = BODY_RADIUS / transform.affine().matrix3.x_axis.length();
        let padding = Vec3::splat(radius + cloth.motion_limit);
        let min = Vec3::new(0.0, -BANNER_LENGTH, 0.0) - padding;
        let max = Vec3::new(BANNER_WIDTH, 0.0, 0.0) + padding;
        let mut contacts: Vec<_> = live_actors(actors)
            .filter_map(|actor| {
                let a = inverse.transform_point3(actor.feet + Vec3::Y * BODY_RADIUS);
                let b =
                    inverse.transform_point3(actor.feet + Vec3::Y * (BODY_HEIGHT - BODY_RADIUS));
                if a.max(b).cmplt(min).any() || a.min(b).cmpgt(max).any() {
                    return None;
                }
                Some((actor.entity, SceneClothCapsule { a, b, radius }))
            })
            .collect();
        contacts.sort_unstable_by_key(|(entity, _)| entity.to_bits());
        let positions = self.cloth.advance(dt, wind, &contacts, &cloth);
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            positions.iter().map(|p| p.to_array()).collect::<Vec<_>>(),
        );
        mesh.compute_smooth_normals();
    }
}

pub(super) fn banner_mesh() -> (Banner, Mesh) {
    let mut points = Vec::with_capacity(COLUMNS * ROWS);
    let mut uv = Vec::with_capacity(COLUMNS * ROWS);
    let mut freedom = Vec::with_capacity(COLUMNS * ROWS);
    let mut faces = Vec::with_capacity((COLUMNS - 1) * (ROWS - 1) * 2);
    for row in 0..ROWS {
        for col in 0..COLUMNS {
            let u = col as f32 / (COLUMNS - 1) as f32;
            let v = row as f32 / (ROWS - 1) as f32;
            let notch = (1.0 - (u - 0.5).abs() * 2.0) * 0.48 * v.powi(4);
            let fold = (u * TAU * 2.0).sin() * 0.085 * v;
            points.push(Vec3::new(
                u * BANNER_WIDTH,
                -BANNER_LENGTH * v + notch,
                fold,
            ));
            uv.push([u, v]);
            freedom.push(v.sqrt());
            if col + 1 < COLUMNS && row + 1 < ROWS {
                let a = row * COLUMNS + col;
                faces.extend([
                    [a, a + COLUMNS, a + 1],
                    [a + 1, a + COLUMNS, a + COLUMNS + 1],
                ]);
            }
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        points.iter().map(|p| p.to_array()).collect::<Vec<_>>(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
    .with_inserted_indices(Indices::U16(
        faces.iter().flatten().map(|&i| i as u16).collect(),
    ));
    mesh.compute_smooth_normals();
    let cloth = SceneCloth::new(points, freedom, faces);
    (
        Banner {
            cloth,
            elapsed: 0.0,
        },
        mesh,
    )
}

fn weave(color: Vec3) -> Image {
    let (width, height) = (128, 192);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    let gold = Vec3::new(0.62, 0.45, 0.26);
    for y in 0..height {
        for x in 0..width {
            let u = (x as f32 + 0.5) / width as f32;
            let v = (y as f32 + 0.5) / height as f32;
            let edge = u.min(1.0 - u).min(v).min(1.0 - v);
            let frame = (0.025..0.05).contains(&edge) || (0.068..0.078).contains(&edge);
            let hem = (0.84..0.88).contains(&v) && (u * 28.0).fract() < 0.53;
            let seal_x = (u - 0.50).abs();
            let seal_y = (v - 0.43).abs();
            let seal = (0.237..0.254).contains(&(seal_x + seal_y * 1.4));
            let peak = ((1.0 - (u - 0.47).abs() / 0.14).max(0.0) * 0.115)
                .max((1.0 - (u - 0.59).abs() / 0.09).max(0.0) * 0.073);
            let mountains = v >= 0.47 - peak && v < 0.47 && peak > 0.0;
            let threads = if (x + y + (y / 3)) % 3 == 0 {
                0.89
            } else {
                1.04
            };
            let wear = 0.88 + 0.085 * (u * 19.0 + (v * 23.0).cos()).sin();
            let pigment = if frame || hem || seal || mountains {
                gold
            } else {
                color
            };
            rgba.extend(
                (pigment * threads * wear * 255.0)
                    .to_array()
                    .map(|c| c as u8),
            );
            rgba.push(255);
        }
    }
    let (bytes, count) = super::super::surfaces::mip_chain(
        &DecodedImage {
            width,
            height,
            rgba,
        },
        true,
    );
    let mut image = Image::new_uninit(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = count;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor::linear());
    image.data = Some(bytes);
    image
}

pub(super) fn spawn_banner(
    parent: &mut ChildSpawnerCommands,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    color: [f32; 3],
) {
    let texture = images.add(weave(Vec3::from_array(color)));
    let material = materials.add(StandardMaterial {
        base_color_texture: Some(texture.clone()),
        emissive_texture: Some(texture),
        emissive: LinearRgba::rgb(0.16, 0.16, 0.16),
        perceptual_roughness: 0.94,
        cull_mode: None,
        double_sided: true,
        ..default()
    });
    let (banner, mesh) = banner_mesh();
    parent.spawn((
        Name::new("Woven banner"),
        banner,
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(material),
        Transform::from_xyz(0.36, BANNER_HEIGHT, 0.0),
        NoFrustumCulling,
    ));
}
