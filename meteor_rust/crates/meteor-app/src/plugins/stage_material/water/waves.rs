use crate::app_state::{AppState, Settings};
use crate::plugins::graphics_pack::{config::Wind, wind_at};
use crate::plugins::stage_material::FrameUniforms;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use meteor_core::constants::MODEL_SCALE;
use std::collections::BTreeMap;

use super::config::{self, WindWave as Wave, MAX_WAVES};

#[derive(Clone, Copy, Default)]
struct Basis {
    sine: f32,
    cosine: f32,
    gradient: Vec2,
}

#[derive(Clone, Copy)]
struct VertexWave {
    position: [f32; 3],
    normal: [f32; 3],
    shore: f32,
    basis: [Basis; MAX_WAVES],
}

#[derive(Clone, Copy, Default)]
struct WaveSample {
    height: f32,
    gradient: Vec2,
}

#[derive(Component)]
pub(crate) struct WaterWaveMesh {
    vertices: Vec<VertexWave>,
    shoreline: Vec<(Vec2, Vec2)>,
    scratch: Vec<WaveSample>,
    last_seconds: Option<f64>,
    active: bool,
    physics: super::physics::WaveField,
    dynamics: Option<Handle<Image>>,
    weather: Option<WeatherWaves>,
}

struct WeatherWaves {
    spectrum: Vec<Wave>,
    wind: Wind,
    response: f32,
    sample_position: Vec3,
}

fn orient_spectrum(waves: &[Wave], target: Vec2) -> Vec<Wave> {
    let Some(dominant) = waves
        .iter()
        .max_by(|a, b| a.amplitude.total_cmp(&b.amplitude))
    else {
        return Vec::new();
    };

    let reference = Vec2::from_array(dominant.direction).normalize_or_zero()
        * if dominant.speed >= 0.0 { -1.0 } else { 1.0 };
    let rotation = Vec2::new(reference.dot(target), reference.perp_dot(target));
    waves
        .iter()
        .map(|wave| Wave {
            direction: rotation.rotate(Vec2::from_array(wave.direction)).to_array(),
            warp: rotation.rotate(Vec2::from_array(wave.warp)).to_array(),
            ..*wave
        })
        .collect()
}

fn basis_at(point: Vec2, wave: Wave) -> Basis {
    let direction = Vec2::from_array(wave.direction).normalize_or_zero()
        * (std::f32::consts::TAU / wave.wavelength);
    let warp_phase = point.dot(Vec2::from_array(wave.warp)) + wave.phase;
    let phase = point.dot(direction) + 0.55 * warp_phase.sin() + wave.phase;
    let (sine, cosine) = phase.sin_cos();
    Basis {
        sine,
        cosine,
        gradient: direction + Vec2::from_array(wave.warp) * (0.55 * warp_phase.cos()),
    }
}

fn bases_at(point: Vec2, spectrum: &[Wave]) -> [Basis; MAX_WAVES] {
    std::array::from_fn(|i| {
        spectrum
            .get(i)
            .map_or(Basis::default(), |&wave| basis_at(point, wave))
    })
}

fn coefficients(seconds: f64, spectrum: &[Wave], amplitude: f32) -> [(f32, f32); MAX_WAVES] {
    std::array::from_fn(|i| {
        spectrum.get(i).map_or((0.0, 0.0), |wave| {
            let (sine, cosine) = (seconds * f64::from(wave.speed)).sin_cos();
            let gain = wave.amplitude * amplitude;
            (sine as f32 * gain, cosine as f32 * gain)
        })
    })
}

fn sample_basis(basis: &[Basis; MAX_WAVES], shore: f32, time: &[(f32, f32)]) -> WaveSample {
    let mut result = WaveSample::default();
    for (basis, &(time_sin, time_cos)) in basis.iter().zip(time) {
        let sin_phase = basis.sine * time_cos + basis.cosine * time_sin;
        let cos_phase = basis.cosine * time_cos - basis.sine * time_sin;
        result.height += sin_phase;
        result.gradient += basis.gradient * cos_phase;
    }
    result.height *= shore;
    result.gradient *= shore;
    result
}

fn edge_distance(point: Vec2, (a, b): (Vec2, Vec2)) -> f32 {
    let edge = b - a;
    let along = ((point - a).dot(edge) / edge.length_squared().max(1e-8)).clamp(0.0, 1.0);
    point.distance(a + edge * along)
}

fn shore_weight(point: Vec2, edges: &[(Vec2, Vec2)]) -> f32 {
    let distance = edges
        .iter()
        .map(|&edge| edge_distance(point, edge))
        .fold(f32::INFINITY, f32::min);
    let t = (distance / config::get().geometry.shore_fade).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn boundary_edges(positions: &[[f32; 3]], faces: &[u32]) -> Vec<(Vec2, Vec2)> {
    type Key = (i32, i32);
    let point = |index: u32| {
        let p = positions[index as usize];
        Vec2::new(p[0] * MODEL_SCALE, -p[1] * MODEL_SCALE)
    };
    let key = |p: Vec2| -> Key {
        (
            (p.x * 10_000.0).round() as i32,
            (p.y * 10_000.0).round() as i32,
        )
    };
    let mut edges = BTreeMap::<(Key, Key), (usize, Vec2, Vec2)>::new();
    for face in faces.chunks_exact(3) {
        for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
            let (a, b) = (point(a), point(b));
            let (ka, kb) = (key(a), key(b));
            let sorted = if ka <= kb { (ka, kb) } else { (kb, ka) };
            edges
                .entry(sorted)
                .and_modify(|edge| edge.0 += 1)
                .or_insert((1, a, b));
        }
    }
    edges
        .into_values()
        .filter_map(|(count, a, b)| (count == 1).then_some((a, b)))
        .collect()
}

fn grid_index(row: usize, col: usize, subdivisions: usize) -> usize {
    row * (subdivisions + 1) - row * row.saturating_sub(1) / 2 + col
}

impl WaterWaveMesh {
    pub(crate) fn configure_shallow(
        &mut self,
        material: &super::GmdlMaterial,
        images: &Assets<Image>,
        transform: &Transform,
    ) {
        if material.water_optics.control.w != 2.0 {
            return;
        }
        let bed = material
            .water_depth
            .as_ref()
            .and_then(|handle| images.get(handle));
        let bounds = material.water_bounds;
        self.physics.use_shallow(|point| {
            let bed = bed?;
            let data = bed.data.as_deref()?;
            let world = transform.transform_point(Vec3::new(
                point.x / MODEL_SCALE,
                -point.y / MODEL_SCALE,
                0.0,
            ));
            let uv = (world.xz() - bounds.xy()) / Vec2::new(bounds.z, bounds.w);
            if !uv.cmpge(Vec2::ZERO).all() || !uv.cmple(Vec2::ONE).all() {
                return None;
            }
            let size = bed.texture_descriptor.size;
            let x = ((uv.x * size.width as f32) as u32).min(size.width.saturating_sub(1));
            let y = ((uv.y * size.height as f32) as u32).min(size.height.saturating_sub(1));
            let offset = (y as usize * size.width as usize + x as usize) * 4;
            let pixel = data.get(offset..offset + 4)?;
            if pixel[2] == 0 {
                return Some(0.0);
            }
            (pixel[1] != 0).then(|| f32::from(pixel[0].max(1)) * material.water.y / 255.0)
        });
    }

    #[cfg(test)]
    pub(crate) fn refine(mesh: &Mesh, faces: &[u32]) -> Option<(Mesh, Self)> {
        Self::refine_with_wind(mesh, faces, None, &Transform::IDENTITY)
    }

    pub(crate) fn refine_with_wind(
        mesh: &Mesh,
        faces: &[u32],
        wind: Option<&Wind>,
        transform: &Transform,
    ) -> Option<(Mesh, Self)> {
        let VertexAttributeValues::Float32x3(source_positions) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
        else {
            return None;
        };
        let VertexAttributeValues::Float32x3(source_normals) =
            mesh.attribute(Mesh::ATTRIBUTE_NORMAL)?
        else {
            return None;
        };
        let VertexAttributeValues::Float32x2(source_uvs) = mesh.attribute(Mesh::ATTRIBUTE_UV_0)?
        else {
            return None;
        };
        let VertexAttributeValues::Float32x4(source_colors) =
            mesh.attribute(Mesh::ATTRIBUTE_COLOR)?
        else {
            return None;
        };
        let tangent = match mesh.attribute(Mesh::ATTRIBUTE_TANGENT) {
            Some(VertexAttributeValues::Float32x4(values)) => Some(values),
            None => None,
            _ => return None,
        };
        let face_count = faces.len() / 3;
        let subdivisions = (2..=config::get().geometry.subdivisions).rev().find(|n| {
            face_count > 0
                && face_count * (n + 1) * (n + 2) / 2 <= config::get().geometry.max_vertices
        })?;
        let vertices_per_face = (subdivisions + 1) * (subdivisions + 2) / 2;
        let triangles: Vec<_> = faces
            .chunks_exact(3)
            .map(|f| {
                std::array::from_fn(|i| {
                    let p = source_positions[f[i] as usize];
                    Vec2::new(p[0], -p[1]) * MODEL_SCALE
                })
            })
            .collect();
        let weather = wind.and_then(|wind| {
            let response = wind.response?.water;
            let target = Vec2::from_array(wind.direction);
            let local_direction = (Self::local_point(target, Some(transform))
                - Self::local_point(Vec2::ZERO, Some(transform)))
            .normalize_or_zero();
            let (min, max) = triangles.iter().flatten().copied().fold(
                (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)),
                |(min, max), point| (min.min(point), max.max(point)),
            );
            let midpoint = (min + max) * (0.5 / MODEL_SCALE);
            Some(WeatherWaves {
                spectrum: orient_spectrum(&config::get().wind_waves, local_direction),
                wind: wind.clone(),
                response,
                sample_position: transform.transform_point(Vec3::new(midpoint.x, -midpoint.y, 0.0)),
            })
        });
        let spectrum = weather
            .as_ref()
            .map_or(config::get().wind_waves.as_slice(), |weather| {
                &weather.spectrum
            });
        let physics = super::physics::WaveField::new(&triangles);
        let shoreline = boundary_edges(source_positions, faces);
        if shoreline.is_empty() {
            return None;
        }
        let mut positions = Vec::with_capacity(face_count * vertices_per_face);
        let mut normals = Vec::with_capacity(positions.capacity());
        let mut uvs = Vec::with_capacity(positions.capacity());
        let mut colors = Vec::with_capacity(positions.capacity());
        let mut tangents = tangent.map(|_| Vec::with_capacity(positions.capacity()));
        let mut indices = Vec::with_capacity(face_count * subdivisions * subdivisions * 3);
        let mut vertices = Vec::with_capacity(positions.capacity());
        for face in faces.chunks_exact(3) {
            let [a, b, c] = [face[0] as usize, face[1] as usize, face[2] as usize];
            let base = positions.len();
            for row in 0..=subdivisions {
                for col in 0..=subdivisions - row {
                    let u = col as f32 / subdivisions as f32;
                    let v = row as f32 / subdivisions as f32;
                    let w = 1.0 - u - v;
                    let mix3 = |values: &Vec<[f32; 3]>| {
                        (Vec3::from_array(values[a]) * w
                            + Vec3::from_array(values[b]) * u
                            + Vec3::from_array(values[c]) * v)
                            .to_array()
                    };
                    let mix2 = |values: &Vec<[f32; 2]>| {
                        (Vec2::from_array(values[a]) * w
                            + Vec2::from_array(values[b]) * u
                            + Vec2::from_array(values[c]) * v)
                            .to_array()
                    };
                    let mix4 = |values: &Vec<[f32; 4]>| {
                        (Vec4::from_array(values[a]) * w
                            + Vec4::from_array(values[b]) * u
                            + Vec4::from_array(values[c]) * v)
                            .to_array()
                    };
                    let position = mix3(source_positions);
                    let normal = Vec3::from_array(mix3(source_normals))
                        .normalize_or_zero()
                        .to_array();
                    let world = Vec2::new(position[0] * MODEL_SCALE, -position[1] * MODEL_SCALE);
                    let seed = VertexWave {
                        position,
                        normal,
                        shore: shore_weight(world, &shoreline),
                        basis: bases_at(world, spectrum),
                    };
                    positions.push(position);
                    normals.push(normal);
                    uvs.push(mix2(source_uvs));
                    colors.push(mix4(source_colors));
                    if let (Some(source), Some(output)) = (tangent, tangents.as_mut()) {
                        output.push(mix4(source));
                    }
                    vertices.push(seed);
                }
            }
            for row in 0..subdivisions {
                for col in 0..subdivisions - row {
                    let [a, b, c] = [
                        base + grid_index(row, col, subdivisions),
                        base + grid_index(row, col + 1, subdivisions),
                        base + grid_index(row + 1, col, subdivisions),
                    ];
                    indices.extend([a as u32, b as u32, c as u32]);
                    if col + row + 1 < subdivisions {
                        let d = base + grid_index(row + 1, col + 1, subdivisions);
                        indices.extend([b as u32, d as u32, c as u32]);
                    }
                }
            }
        }
        let mut refined = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        refined.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        refined.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        refined.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        refined.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        if let Some(tangents) = tangents {
            refined.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
        }
        refined.insert_indices(Indices::U32(indices));
        let scratch = vec![WaveSample::default(); vertices.len()];
        Some((
            refined,
            Self {
                vertices,
                shoreline,
                scratch,
                last_seconds: None,
                active: false,
                physics,
                dynamics: None,
                weather,
            },
        ))
    }

    pub(crate) fn height_at(&self, point: Vec2, seconds: f64) -> f32 {
        let spectrum = self.spectrum();
        let basis = bases_at(point, spectrum);
        let shore = shore_weight(point, &self.shoreline);
        sample_basis(
            &basis,
            shore,
            &self.wind_coefficients(seconds)[..spectrum.len()],
        )
        .height
            + self.physics.sample(point).0 * shore
    }

    fn spectrum(&self) -> &[Wave] {
        self.weather
            .as_ref()
            .map_or(config::get().wind_waves.as_slice(), |weather| {
                &weather.spectrum
            })
    }

    fn wind_coefficients(&self, seconds: f64) -> [(f32, f32); MAX_WAVES] {
        let amplitude = self.weather.as_ref().map_or(1.0, |weather| {
            (wind_at(&weather.wind, weather.sample_position, seconds).length() * weather.response)
                .min(1.0)
        });
        coefficients(seconds, self.spectrum(), amplitude)
    }

    fn local_point(point: Vec2, transform: Option<&Transform>) -> Vec2 {
        let Some(transform) = transform else {
            return point;
        };
        let p = transform
            .to_matrix()
            .inverse()
            .transform_point3(Vec3::new(point.x, 0.0, point.y));
        Vec2::new(p.x, -p.y) * MODEL_SCALE
    }

    pub(super) fn world_height_at(
        &self,
        point: Vec2,
        seconds: f64,
        transform: Option<&Transform>,
    ) -> f32 {
        self.height_at(Self::local_point(point, transform), seconds)
    }
}

pub(super) fn update(
    state: Res<State<AppState>>,
    settings: Res<Settings>,
    frame: Res<FrameUniforms>,
    time: Res<Time>,
    mut interactions: ResMut<super::interaction::LiquidInteractions>,
    mut uploads: Option<ResMut<super::dynamics::WaterUploads>>,
    mut images: Option<ResMut<Assets<Image>>>,
    mut materials: Option<ResMut<Assets<super::GmdlMaterial>>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut waves: Query<(
        Entity,
        &Mesh3d,
        &Transform,
        Option<&MeshMaterial3d<super::GmdlMaterial>>,
        &mut WaterWaveMesh,
    )>,
) {
    let enabled =
        settings.water_enhance && matches!(state.get(), AppState::Playing | AppState::Paused);
    if enabled && *state.get() == AppState::Paused {
        return;
    }
    if let Some(uploads) = uploads.as_deref_mut() {
        uploads.textures.clear();
        uploads.epoch = uploads.epoch.wrapping_add(1);
    }

    let impulses = std::mem::take(&mut interactions.impulses);
    for (entity, handle, transform, material, mut wave) in &mut waves {
        let Some(mut mesh) = meshes.get_mut(&handle.0) else {
            continue;
        };
        if !enabled {
            if wave.active {
                if let Some(VertexAttributeValues::Float32x3(positions)) =
                    mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
                {
                    for (position, seed) in positions.iter_mut().zip(&wave.vertices) {
                        *position = seed.position;
                    }
                }
                if let Some(VertexAttributeValues::Float32x3(normals)) =
                    mesh.attribute_mut(Mesh::ATTRIBUTE_NORMAL)
                {
                    for (normal, seed) in normals.iter_mut().zip(&wave.vertices) {
                        *normal = seed.normal;
                    }
                }
                wave.physics.clear();
                wave.active = false;
                wave.last_seconds = None;
            }
            continue;
        }
        for impulse in impulses.iter().filter(|i| i.surface == entity) {
            wave.physics.impulse_at_radius(
                WaterWaveMesh::local_point(impulse.point.xz(), Some(transform)),
                impulse.strength,
                impulse.direction,
                impulse.radius,
            );
        }
        if wave.last_seconds == Some(frame.seconds) {
            continue;
        }
        let elapsed = wave.last_seconds.map_or(time.delta_secs(), |last| {
            (frame.seconds - last).max(0.0) as f32
        });
        wave.physics.advance(elapsed);
        if let (Some(images), Some(materials), Some(handle)) =
            (images.as_deref_mut(), materials.as_deref_mut(), material)
        {
            if wave.dynamics.is_none() {
                use bevy::image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
                use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
                let (width, height, bounds) = wave.physics.layout();
                let mut image = Image::new(
                    Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    TextureDimension::D2,
                    wave.physics.pixels(),
                    TextureFormat::Rgba8Unorm,
                    RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
                );
                image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                    mag_filter: ImageFilterMode::Linear,
                    min_filter: ImageFilterMode::Linear,
                    ..default()
                });
                let image = images.add(image);
                if let Some(mut material) = materials.get_mut(&handle.0) {
                    let origin = transform.transform_point(Vec3::new(
                        bounds.x / MODEL_SCALE,
                        -bounds.y / MODEL_SCALE,
                        0.0,
                    ));
                    material.water_dynamics_bounds =
                        Vec4::new(origin.x, origin.z, bounds.z, bounds.w);
                    material.water_dynamics = Some(image.clone());
                }
                wave.dynamics = Some(image);
                wave.physics.dirty = false;
            } else if wave.physics.dirty {
                if let Some(uploads) = uploads.as_deref_mut() {
                    let (width, height, _) = wave.physics.layout();
                    uploads.textures.push(super::dynamics::Upload {
                        image: wave.dynamics.as_ref().unwrap().clone(),
                        width,
                        height,
                        pixels: std::sync::Arc::new(wave.physics.pixels()),
                    });
                    wave.physics.dirty = false;
                }
            }
        }

        let time = wave.wind_coefficients(frame.seconds);
        let time = &time[..wave.spectrum().len()];
        for i in 0..wave.vertices.len() {
            let seed = wave.vertices[i];
            let mut sample = sample_basis(&seed.basis, seed.shore, time);
            let point = Vec2::new(seed.position[0], -seed.position[1]) * MODEL_SCALE;
            let (height, gradient) = wave.physics.sample(point);
            sample.height += height * seed.shore;
            if wave.dynamics.is_none() {
                sample.gradient += gradient * seed.shore;
            }
            wave.scratch[i] = sample;
        }
        if let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
        {
            for ((position, seed), sample) in
                positions.iter_mut().zip(&wave.vertices).zip(&wave.scratch)
            {
                *position = [
                    seed.position[0],
                    seed.position[1],
                    seed.position[2] + sample.height / MODEL_SCALE,
                ];
            }
        }
        if let Some(VertexAttributeValues::Float32x3(normals)) =
            mesh.attribute_mut(Mesh::ATTRIBUTE_NORMAL)
        {
            for (normal, sample) in normals.iter_mut().zip(&wave.scratch) {
                *normal = Vec3::new(-sample.gradient.x, sample.gradient.y, 1.0)
                    .normalize()
                    .to_array();
            }
        }
        wave.active = true;
        wave.last_seconds = Some(frame.seconds);
    }
}

pub(super) fn install(app: &mut App) {
    super::dynamics::install(app);
    app.add_systems(
        Update,
        update.after(crate::plugins::render_plugin::update_uv_scroll),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reoriented_dominant_wave_travels_with_the_wind() {
        for wind in [Vec2::X, Vec2::NEG_X, Vec2::Y] {
            let spectrum = orient_spectrum(&config::get().wind_waves, wind);
            let dominant = spectrum
                .iter()
                .max_by(|a, b| a.amplitude.total_cmp(&b.amplitude))
                .unwrap();
            let flow = -dominant.speed.signum() * Vec2::from_array(dominant.direction).normalize();
            assert!((flow - wind).length() < 1e-5);
        }
    }

    #[test]
    fn sn13_tessellation_is_bounded_and_shore_stays_level() {
        let mesh = Triangle3d::new(Vec3::ZERO, Vec3::X * 100.0, Vec3::Y * 100.0)
            .mesh()
            .build();
        let mut mesh = mesh;
        let count = mesh.count_vertices();
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0; 4]; count]);
        let Some((refined, wave)) = WaterWaveMesh::refine(&mesh, &[0, 1, 2]) else {
            panic!("refined");
        };
        assert_eq!(
            refined.count_vertices(),
            (config::get().geometry.subdivisions + 1) * (config::get().geometry.subdivisions + 2)
                / 2
        );
        assert_eq!(
            refined.indices().unwrap().len(),
            config::get().geometry.subdivisions * config::get().geometry.subdivisions * 3
        );
        assert!(wave.height_at(Vec2::ZERO, 0.3).abs() < 1e-5);
        assert!(wave.height_at(Vec2::new(3.0, -3.0), 0.3).abs() > 0.01);
        let heights: Vec<_> = (0..20)
            .map(|i| wave.height_at(Vec2::new(3.0, -3.0), f64::from(i) * 0.37))
            .collect();
        let (lo, hi) = heights
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &h| {
                (lo.min(h), hi.max(h))
            });
        assert!(hi - lo > 0.02, "traveling waves must vary over time");
        assert!(WaterWaveMesh::refine(
            &mesh,
            &[0, 1, 2].repeat(config::get().geometry.max_vertices)
        )
        .is_none());
    }
    #[cfg(feature = "original-assets")]
    #[test]
    fn real_clear_water_meshes_fit_budget_and_share_contact_coordinates() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        for (stage, name) in [
            ("sn06", "Cylinder01"),
            ("sn09", "Plane37"),
            ("sn13", "Plane02"),
        ] {
            let gmdl = meteor_asset::gmdl::decode_gmdl(
                &std::fs::read(root.join(format!("{stage}/{stage}.gmb"))).unwrap(),
            )
            .unwrap();
            let object = gmdl.objects.iter().find(|o| o.name == name).unwrap();
            let indices = &object.indices;
            let (mesh, transform) = crate::plugins::render_plugin::build_scene_mesh(
                object,
                indices,
                [117.0, -42.0, 20.0],
                None,
            );
            let (refined, wave) = WaterWaveMesh::refine(&mesh, indices).expect(name);
            assert!(refined.count_vertices() <= config::get().geometry.max_vertices);
            for seed in wave.vertices.iter().step_by(41) {
                let world = transform.transform_point(Vec3::from_array(seed.position));
                let local = Vec2::new(seed.position[0], -seed.position[1]) * MODEL_SCALE;
                assert!(
                    (wave.world_height_at(world.xz(), 0.4, Some(&transform))
                        - wave.height_at(local, 0.4))
                    .abs()
                        < 1e-4
                );
            }
        }
    }

    #[test]
    fn contact_impulses_deform_only_their_pool_and_pause_toggle_restore_geometry() {
        use super::super::interaction::{LiquidInteractions, SurfaceImpulse};
        let mut mesh = Triangle3d::new(Vec3::ZERO, Vec3::X * 250.0, Vec3::Y * 250.0)
            .mesh()
            .build();
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0; 4]; mesh.count_vertices()]);
        let mut app = App::new();
        app.insert_resource(State::new(AppState::Playing))
            .insert_resource(Settings {
                water_enhance: true,
                ..default()
            })
            .insert_resource(Time::<()>::default())
            .init_resource::<FrameUniforms>()
            .init_resource::<LiquidInteractions>()
            .init_resource::<Assets<Mesh>>()
            .add_systems(Update, update);
        let transform =
            Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                .with_scale(Vec3::splat(MODEL_SCALE))
                .with_translation(Vec3::new(50.0, 3.0, 70.0));
        let mut entities = Vec::new();
        let calm = Wind {
            strength: 0.0,
            response: Some(Default::default()),
            ..default()
        };
        for _ in 0..2 {
            let (mesh, wave) =
                WaterWaveMesh::refine_with_wind(&mesh, &[0, 1, 2], Some(&calm), &transform)
                    .unwrap();
            assert_eq!(wave.height_at(Vec2::new(6.0, -6.0), 3.0), 0.0);
            let handle = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
            entities.push(
                app.world_mut()
                    .spawn((Mesh3d(handle), transform, wave))
                    .id(),
            );
        }
        app.world_mut()
            .resource_mut::<LiquidInteractions>()
            .impulses
            .push(SurfaceImpulse {
                surface: entities[0],
                point: Vec3::new(56.0, 3.0, 64.0),
                strength: 1.0,
                radius: 1.2,
                direction: Vec2::X,
            });
        for i in 0..10 {
            app.world_mut().resource_mut::<FrameUniforms>().seconds = i as f64 / 60.0;
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
            app.update();
        }
        let a = app.world().get::<WaterWaveMesh>(entities[0]).unwrap();
        let b = app.world().get::<WaterWaveMesh>(entities[1]).unwrap();
        assert!(a.physics.sample(Vec2::new(6.0, -6.0)).0.abs() > 0.01);
        assert_eq!(b.physics.sample(Vec2::new(6.0, -6.0)).0, 0.0);
        let snapshot = a.scratch.iter().map(|s| s.height).collect::<Vec<_>>();
        app.insert_resource(State::new(AppState::Paused));
        app.world_mut().resource_mut::<FrameUniforms>().seconds = 20.0;
        app.update();
        assert_eq!(
            snapshot,
            app.world()
                .get::<WaterWaveMesh>(entities[0])
                .unwrap()
                .scratch
                .iter()
                .map(|s| s.height)
                .collect::<Vec<_>>()
        );
        app.world_mut().resource_mut::<Settings>().water_enhance = false;
        app.update();
        for entity in entities {
            let wave = app.world().get::<WaterWaveMesh>(entity).unwrap();
            let mesh = app
                .world()
                .resource::<Assets<Mesh>>()
                .get(&app.world().get::<Mesh3d>(entity).unwrap().0)
                .unwrap();
            let Some(VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("positions");
            };
            assert!(positions
                .iter()
                .zip(&wave.vertices)
                .all(|(p, v)| *p == v.position));
            assert_eq!(wave.physics.sample(Vec2::new(6.0, -6.0)).0, 0.0);
        }
    }
}
