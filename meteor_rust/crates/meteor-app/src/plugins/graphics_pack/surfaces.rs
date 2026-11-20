use super::*;
use crate::plugins::stage_material::{GmdlMaterial, GmdlMode};
use bevy::{
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    mesh::VertexAttributeValues,
    render::render_resource::TextureFormat,
};
use std::collections::HashMap;

pub struct SceneGraphics<'a> {
    packs: Option<&'a GraphicsPacks>,
    rules: Vec<(usize, config::Surface)>,
    environment: Option<(usize, config::Environment)>,
    textures: HashMap<(usize, String, bool, bool), Handle<Image>>,
    wind_vertices: usize,
    specular: Option<Handle<Image>>,
}
impl<'a> SceneGraphics<'a> {
    pub fn new(packs: Option<&'a GraphicsPacks>, stage: &str) -> Self {
        let mut out = Self {
            packs,
            rules: vec![],
            environment: None,
            textures: HashMap::new(),
            wind_vertices: 0,
            specular: None,
        };
        if let Some(packs) = packs {
            for (i, p) in packs.packs.iter().enumerate() {
                for s in p
                    .config
                    .scenes
                    .iter()
                    .filter(|s| s.stage == "*" || s.stage.eq_ignore_ascii_case(stage))
                {
                    if let Some(e) = &s.environment {
                        out.environment = Some((i, e.clone()));
                    }
                    out.rules.extend(s.surfaces.iter().map(|r| (i, r.clone())));
                }
            }
        }
        out
    }
    fn texture(
        &mut self,
        pack: usize,
        id: &str,
        srgb: bool,
        environment: bool,
        images: &mut Assets<Image>,
    ) -> Handle<Image> {
        let key = (pack, id.to_string(), srgb, environment);
        self.textures
            .entry(key)
            .or_insert_with(|| {
                let data = &self.packs.unwrap().packs[pack].images[id];
                let handle = crate::plugins::pak_textures::image_from_decoded(images, data);
                let mut image = images.get_mut(&handle).unwrap();
                if srgb {
                    image.texture_descriptor.format = TextureFormat::Rgba8UnormSrgb;
                }
                let (bytes, count) = mip_chain(data, srgb);
                image.data = Some(bytes);
                image.texture_descriptor.mip_level_count = count;
                if environment {
                    if let ImageSampler::Descriptor(sampler) = &mut image.sampler {
                        sampler.address_mode_v = ImageAddressMode::ClampToEdge;
                    }
                }
                handle
            })
            .clone()
    }
    pub fn configure(
        &mut self,
        object: &str,
        texture: Option<&str>,
        material: &mut GmdlMaterial,
        mesh: &mut Mesh,
        allow_pbr: bool,
        allow_wind: bool,
        images: &mut Assets<Image>,
    ) -> Option<WindMesh> {
        let matches: Vec<_> = self
            .rules
            .iter()
            .filter(|(_, r)| r.matches(object, texture))
            .cloned()
            .collect();
        let mut selected_material = None;
        let mut selected_wind = None;
        for (i, r) in matches {
            if let Some(m) = r.material {
                selected_material = Some((i, m));
            }
            if r.wind.is_some() {
                selected_wind = r.wind;
            }
        }
        if let Some((i, id)) =
            selected_material.filter(|_| allow_pbr && material.mode == GmdlMode::Opaque)
        {
            self.apply_pbr(i, &id, object, material, mesh, images);
        }
        let w = selected_wind.filter(|_| allow_wind)?;
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            return None;
        };
        if positions.is_empty() {
            return None;
        }
        if positions.len() > 8192 || self.wind_vertices + positions.len() > 65536 {
            warn!("graphics wind: {object} exceeds display vertex budget");
            return None;
        }
        self.wind_vertices += positions.len();
        let normals = match mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
            Some(VertexAttributeValues::Float32x3(n)) => n.clone(),
            _ => vec![],
        };
        let tangents = match mesh.attribute(Mesh::ATTRIBUTE_TANGENT) {
            Some(VertexAttributeValues::Float32x4(t)) => t.clone(),
            _ => vec![],
        };
        let min = positions
            .iter()
            .map(|p| Vec3::from_array(*p))
            .fold(Vec3::splat(f32::INFINITY), Vec3::min);
        let max = positions
            .iter()
            .map(|p| Vec3::from_array(*p))
            .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
        Some(WindMesh {
            center: (min + max) * 0.5,
            positions: positions.clone(),
            normals,
            tangents,
            profile: w,
            last_wind: None,
        })
    }
    pub fn configure_additive_texture(
        &mut self,
        object: &str,
        texture: Option<&str>,
        material: &mut GmdlMaterial,
        images: &mut Assets<Image>,
    ) -> Option<(Handle<Image>, Option<f32>)> {
        if material.mode != GmdlMode::SrcColorOne {
            return None;
        }
        if self
            .rules
            .iter()
            .filter(|(_, rule)| rule.matches(object, texture))
            .any(|(_, rule)| rule.waterfall_lighting)
        {
            material.water.x = 7.0;
        }
        let selected = self
            .rules
            .iter()
            .filter(|(_, rule)| rule.matches(object, texture))
            .filter_map(|(index, rule)| {
                rule.additive_texture.as_ref().map(|id| {
                    (
                        *index,
                        id.clone(),
                        rule.waterfall_layers,
                        rule.waterfall_floor,
                    )
                })
            })
            .last();
        if let Some((index, id, layers, floor)) = selected {
            let texture = self.texture(index, &id, false, false, images);
            material.texture = Some(texture.clone());
            material.params.has_texture = 1;
            return layers.then_some((texture, floor));
        }
        None
    }
    pub(in crate::plugins) fn apply_pbr(
        &mut self,
        i: usize,
        id: &str,
        object: &str,
        material: &mut GmdlMaterial,
        mesh: &mut Mesh,
        images: &mut Assets<Image>,
    ) {
        let p = self.packs.unwrap().packs[i].config.materials[id].clone();
        material.pbr = Vec4::new(1., p.metallic, p.roughness, p.vertex_color);
        material.pbr_tint = Vec3::from_array(p.tint).extend(0.);
        material.pbr_lighting = Vec3::new(p.direct_light, p.ambient_light, p.fog_strength);
        if let Some(id) = p.base_color {
            material.texture = Some(self.texture(i, &id, true, false, images));
            material.params.has_texture = 1;
            material.pbr_textures.y = 1.;
        }
        if let Some(id) = p.normal {
            if mesh.attribute(Mesh::ATTRIBUTE_TANGENT).is_some() || mesh.generate_tangents().is_ok()
            {
                material.normal_texture = Some(self.texture(i, &id, false, false, images));
                material.normal_strength = p.normal_strength;
                material.normal_contrast = 1.0;
            } else {
                warn!("graphics normal map: {object} has no valid tangent basis");
            }
        }
        if let Some(id) = p.orm {
            material.orm_texture = Some(self.texture(i, &id, false, false, images));
            material.pbr_textures.x = 1.;
        }
        if let Some((ei, e)) = self.environment.clone() {
            if let Some(id) = e.diffuse {
                material.env_diffuse = Some(self.texture(ei, &id, true, true, images));
                material.pbr_tint.w = 1.;
            }
            if let Some(id) = e.specular_mips.first() {
                let h = self
                    .specular
                    .get_or_insert_with(|| {
                        let levels = &self.packs.unwrap().packs[ei].images;
                        let base = &levels[id];
                        let h = crate::plugins::pak_textures::image_from_decoded(images, base);
                        let mut image = images.get_mut(&h).unwrap();
                        image.texture_descriptor.format = TextureFormat::Rgba8UnormSrgb;
                        image.texture_descriptor.mip_level_count = e.specular_mips.len() as u32;
                        image.data = Some(
                            e.specular_mips
                                .iter()
                                .flat_map(|id| levels[id].rgba.iter().copied())
                                .collect(),
                        );
                        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                            address_mode_u: ImageAddressMode::Repeat,
                            address_mode_v: ImageAddressMode::ClampToEdge,
                            mag_filter: ImageFilterMode::Linear,
                            min_filter: ImageFilterMode::Linear,
                            mipmap_filter: ImageFilterMode::Linear,
                            ..default()
                        });
                        h
                    })
                    .clone();
                material.env_specular = Some(h);
                material.pbr_textures.z = 1.;
            }
        }
    }

    pub(super) fn has_wind(&self, object: &str, texture: Option<&str>) -> bool {
        self.rules
            .iter()
            .any(|(_, rule)| rule.wind.is_some() && rule.matches(object, texture))
    }
}
#[derive(Component)]
pub struct WindMesh {
    center: Vec3,
    pub positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    tangents: Vec<[f32; 4]>,
    pub profile: config::WindSurface,
    last_wind: Option<(Vec2, GlobalTransform)>,
}

pub(super) fn update_wind(
    packs: Res<GraphicsPacks>,
    loaded: Option<Res<crate::app_state::LoadedStageInfo>>,
    state: Option<Res<State<crate::app_state::AppState>>>,
    clock: Option<Res<crate::plugins::FrameClock>>,
    frame: Option<Res<crate::plugins::stage_material::FrameUniforms>>,
    time: Res<Time>,
    mut elapsed: Local<f64>,
    mut accumulated: Local<f32>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut query: Query<(&Mesh3d, &GlobalTransform, &mut WindMesh)>,
) {
    if state.is_none_or(|s| *s.get() != crate::app_state::AppState::Playing) {
        return;
    }
    let dt = crate::plugins::simulation_dt(clock.as_deref(), time.delta_secs());
    if dt <= 0. {
        return;
    }
    *elapsed += dt as f64;
    *accumulated += dt;
    if *accumulated < 1. / 30. {
        return;
    }
    *accumulated = 0.;
    let Some(loaded) = loaded else {
        return;
    };
    let wind = packs.wind(&loaded.stage_id);
    for (handle, transform, mut w) in &mut query {
        let breeze = wind.map_or(Vec2::ZERO, |wind| {
            wind_at(
                wind,
                transform.transform_point(w.center),
                frame.as_ref().map_or(*elapsed, |f| f.seconds),
            ) * wind.response.map_or(1.0, |response| response.cloth)
        });
        let wind_pose = (breeze, *transform);
        if w.last_wind == Some(wind_pose) {
            continue;
        }
        let Some(mut mesh) = meshes.get_mut(&handle.0) else {
            continue;
        };
        w.last_wind = Some(wind_pose);
        let matrix = transform.affine().matrix3;
        let inverse = matrix.inverse();

        let height_axis = matrix.transpose() * Vec3::Y;
        let direction = inverse * Vec3::new(breeze.x, 0., breeze.y) * w.profile.amplitude;
        let span = w.profile.end_height - w.profile.start_height;
        let bend = |u: f32| if w.profile.anchor_top { 1.0 - u } else { u };
        let slope = |p: [f32; 3]| {
            let y = transform.transform_point(Vec3::from_array(p)).y;
            let u = (y - w.profile.start_height) / span;
            if u > 0. && u < 1. {
                2. * bend(u) / span * if w.profile.anchor_top { -1.0 } else { 1.0 }
            } else {
                0.
            }
        };
        let positions: Vec<_> = w
            .positions
            .iter()
            .map(|p| {
                let p = Vec3::from_array(*p);
                let y = transform.transform_point(p).y;
                let u = ((y - w.profile.start_height) / span).clamp(0., 1.);
                (p + direction * bend(u).powi(2)).to_array()
            })
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        if w.normals.len() == w.positions.len() {
            mesh.insert_attribute(
                Mesh::ATTRIBUTE_NORMAL,
                w.normals
                    .iter()
                    .zip(&w.positions)
                    .map(|(n, p)| {
                        let mut n = Vec3::from_array(*n);
                        let gradient = height_axis * slope(*p);
                        n -= gradient * direction.dot(n) / (1. + gradient.dot(direction)).max(0.01);
                        n.normalize_or_zero().to_array()
                    })
                    .collect::<Vec<_>>(),
            );
        }
        if w.tangents.len() == w.positions.len() {
            mesh.insert_attribute(
                Mesh::ATTRIBUTE_TANGENT,
                w.tangents
                    .iter()
                    .zip(&w.positions)
                    .map(|(t, p)| {
                        let t = Vec4::from_array(*t);
                        (t.truncate() + direction * slope(*p) * height_axis.dot(t.truncate()))
                            .normalize_or_zero()
                            .extend(t.w)
                            .to_array()
                    })
                    .collect::<Vec<_>>(),
            );
        }
    }
}

pub(super) fn mip_chain(
    source: &meteor_asset::texture::DecodedImage,
    srgb: bool,
) -> (Vec<u8>, u32) {
    let linear: [f32; 256] = std::array::from_fn(|i| {
        let x = i as f32 / 255.;
        if srgb && x > 0.04045 {
            ((x + 0.055) / 1.055).powf(2.4)
        } else if srgb {
            x / 12.92
        } else {
            x
        }
    });
    let mut bytes = source.rgba.clone();
    let mut previous = source.rgba.clone();
    let (mut width, mut height) = (source.width, source.height);
    let mut count = 1;
    while width > 1 || height > 1 {
        let (nw, nh) = ((width / 2).max(1), (height / 2).max(1));
        let mut next = Vec::with_capacity((nw * nh * 4) as usize);
        for y in 0..nh {
            for x in 0..nw {
                let mut sum = [0.; 4];
                let mut n = 0.;
                for sy in y * height / nh..(y + 1) * height / nh {
                    for sx in x * width / nw..(x + 1) * width / nw {
                        let i = ((sy * width + sx) * 4) as usize;
                        for c in 0..4 {
                            sum[c] += if c < 3 {
                                linear[previous[i + c] as usize]
                            } else {
                                previous[i + c] as f32 / 255.
                            };
                        }
                        n += 1.;
                    }
                }
                for (c, value) in sum.into_iter().enumerate() {
                    let v = value / n;
                    let encoded = if srgb && c < 3 {
                        if v > 0.0031308 {
                            1.055 * v.powf(1. / 2.4) - 0.055
                        } else {
                            12.92 * v
                        }
                    } else {
                        v
                    };
                    next.push((encoded * 255.).round().clamp(0., 255.) as u8);
                }
            }
        }
        bytes.extend_from_slice(&next);
        previous = next;
        width = nw;
        height = nh;
        count += 1;
    }
    (bytes, count)
}

#[cfg(test)]
pub(super) fn test_mips() {
    let data = meteor_asset::texture::DecodedImage {
        width: 2,
        height: 1,
        rgba: vec![0, 0, 0, 255, 255, 255, 255, 255],
    };
    let (linear, n) = mip_chain(&data, false);
    let (srgb, _) = mip_chain(&data, true);
    assert_eq!(n, 2);
    assert_eq!(linear[8], 128);
    assert_eq!(srgb[8], 188);
}
