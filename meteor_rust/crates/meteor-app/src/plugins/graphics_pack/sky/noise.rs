use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

const SIZE: u32 = 64;
const WEATHER_SIZE: u32 = 128;
const WEATHER_CELLS: i32 = 5;

pub(super) fn image() -> Image {
    let cells = CloudCells::new();
    let mut data = Vec::with_capacity((SIZE * SIZE * SIZE * 4) as usize);
    for z in 0..SIZE {
        for y in 0..SIZE {
            for x in 0..SIZE {
                let p = (Vec3::new(x as f32, y as f32, z as f32) + 0.5) / SIZE as f32;
                let shape = 0.76 * value(p, 8, 127) + 0.24 * value(p, 17, 611);
                let erosion = 0.65 * value(p, 21, 389) + 0.35 * value(p, 31, 173);
                let fine = value(p, 13, 281);
                let billows = cells.density(p);
                data.extend([shape, erosion, fine, billows].map(quantize));
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: SIZE,
        },
        TextureDimension::D3,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = repeating_sampler();
    image
}

pub(super) fn weather() -> Image {
    let mut data = Vec::with_capacity((WEATHER_SIZE * WEATHER_SIZE * 4) as usize);
    for z in 0..WEATHER_SIZE {
        for x in 0..WEATHER_SIZE {
            let uv = (Vec2::new(x as f32, z as f32) + 0.5) / WEATHER_SIZE as f32;
            data.extend(weather_at(uv).to_array().map(quantize));
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: WEATHER_SIZE,
            height: WEATHER_SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = repeating_sampler();
    image
}

fn weather_at(uv: Vec2) -> Vec4 {
    let p = Vec3::new(uv.x, 0.0, uv.y);
    let distortion = Vec2::new(value(p, 7, 191), value(p, 7, 345)) - 0.5;
    let point = uv * WEATHER_CELLS as f32 + distortion * 0.25;
    let origin = point.floor().as_ivec2();
    let mut union = 0.0_f32;
    let mut total = 0.0_f32;
    let mut tower_height = 0.0_f32;
    for z in -1..=1 {
        for x in -1..=1 {
            let cell = origin + IVec2::new(x, z);
            let wrapped = cell.rem_euclid(IVec2::splat(WEATHER_CELLS)).as_uvec2();
            let key = UVec3::new(wrapped.x, 0, wrapped.y);
            let random = |seed| lattice_random(key, seed);
            let angle = random(25) * std::f32::consts::PI;
            let axis = Vec2::from_angle(angle);
            let center = cell.as_vec2() + Vec2::new(random(153), random(417)) * 0.47 + 0.265;
            let d = point - center;
            let patch = Vec2::new(d.dot(axis), d.perp_dot(axis));
            let radius = Vec2::new(0.48 + 0.25 * random(614), 0.30 + 0.22 * random(90));
            let body = (1.0 - (patch / radius).length_squared()).max(0.0);
            let satellite = (1.0
                - ((patch - Vec2::new(0.29, 0.10)) / (radius * 0.53)).length_squared())
            .max(0.0);
            let cover = (body + 0.48 * satellite * (1.0 - body)).powf(0.70);
            let gain = cover * (0.52 + 0.48 * random(91));
            union += gain * (1.0 - union);
            total += gain;
            tower_height += gain * (0.4 + 0.6 * random(821));
        }
    }
    Vec4::new(
        union,
        tower_height / total.max(0.001),
        value(p, 3, 649),
        1.0,
    )
}

fn quantize(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn repeating_sampler() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    })
}

struct CloudCells(Vec<Vec3>);

impl CloudCells {
    const PERIOD: i32 = 12;

    fn new() -> Self {
        let mut features = Vec::with_capacity(Self::PERIOD.pow(3) as usize);
        for z in 0..Self::PERIOD {
            for y in 0..Self::PERIOD {
                for x in 0..Self::PERIOD {
                    let cell = UVec3::new(x as u32, y as u32, z as u32);
                    let offset = [143, 761, 211].map(|seed| lattice_random(cell, seed));
                    features.push(Vec3::from_array(offset) * 0.60 + 0.20);
                }
            }
        }
        Self(features)
    }

    fn density(&self, p: Vec3) -> f32 {
        let point = p * Self::PERIOD as f32;
        let origin = point.floor();
        let cell = origin.as_ivec3();
        let local = point - origin;
        let mut nearest = 2.0f32;
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    let delta = IVec3::new(x, y, z);
                    let tile = (cell + delta).rem_euclid(IVec3::splat(Self::PERIOD));
                    let index = (tile.z * Self::PERIOD + tile.y) * Self::PERIOD + tile.x;
                    let feature = self.0[index as usize] + delta.as_vec3();
                    nearest = nearest.min(local.distance_squared(feature));
                }
            }
        }
        (1.0 - nearest.sqrt()).max(0.0)
    }
}

fn lattice_random(v: UVec3, seed: u32) -> f32 {
    let mut h = seed
        ^ v.x.wrapping_mul(1597334677)
        ^ v.y.wrapping_mul(3812015801)
        ^ v.z.wrapping_mul(2798796415);
    h = (h ^ (h >> 16)).wrapping_mul(2246822519);
    ((h ^ (h >> 13)) & 0x00ff_ffff) as f32 / 16777216.0
}

fn value(p: Vec3, period: i32, seed: u32) -> f32 {
    let point = p * period as f32;
    let cell = point.floor().as_ivec3();
    let frac = point - cell.as_vec3();
    let t = frac * frac * (3.0 - 2.0 * frac);
    let sample = |x, y, z| {
        let p = cell + IVec3::new(x, y, z);
        let v = p.rem_euclid(IVec3::splat(period)).as_uvec3();
        lattice_random(v, seed)
    };
    let row = |y, z| sample(0, y, z).lerp(sample(1, y, z), t.x);
    let plane = |z| row(0, z).lerp(row(1, z), t.y);
    plane(0).lerp(plane(1), t.z)
}
