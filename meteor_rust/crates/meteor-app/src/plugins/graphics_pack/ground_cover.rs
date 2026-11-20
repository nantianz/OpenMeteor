use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

pub(super) const SPACING: f32 = 0.25;
const MAX_AXIS: u32 = 4096;
const MAX_PIXELS: u32 = 4_194_304;
const MAX_RASTER_TESTS: u64 = 12_000_000;

pub(super) struct GroundCover {
    origin: Vec2,
    pub size: UVec2,
    spacing: f32,
    pub depths: Vec<u8>,
}

impl GroundCover {
    pub fn for_triangles(
        triangles: &[[Vec3; 3]],
        bounds: Option<[[f32; 2]; 2]>,
        edge_width: f32,
    ) -> Option<Self> {
        let mut min = Vec2::splat(f32::INFINITY);
        let mut max = Vec2::splat(f32::NEG_INFINITY);
        for p in triangles.iter().flatten().map(|point| point.xz()) {
            min = min.min(p);
            max = max.max(p);
        }
        let bounds = bounds.map(|v| v.map(Vec2::from_array));
        if let Some([lo, hi]) = bounds {
            min = min.max(lo);
            max = max.min(hi);
        }
        if !min.cmplt(max).all() || !min.is_finite() || !max.is_finite() {
            return None;
        }

        for power in 0..12 {
            let spacing = SPACING * (1u32 << power) as f32;
            let origin = (min / spacing).floor() * spacing - spacing;
            let size = ((max - origin) / spacing)
                .ceil()
                .as_uvec2()
                .saturating_add(UVec2::ONE);
            if let Some(mask) = Self::raster(triangles, origin, size, bounds, spacing, edge_width) {
                return Some(mask);
            }
        }
        None
    }

    pub fn raster(
        triangles: &[[Vec3; 3]],
        origin: Vec2,
        size: UVec2,
        bounds: Option<[Vec2; 2]>,
        spacing: f32,
        edge_width: f32,
    ) -> Option<Self> {
        if size.min_element() < 3 || size.max_element() > MAX_AXIS || size.x * size.y > MAX_PIXELS {
            return None;
        }
        let mut depths = vec![0; (size.x * size.y) as usize];
        let [min, max] = bounds.map_or([UVec2::ZERO, size], |bounds| {
            bounds.map(|p| ((p - origin) / spacing - 0.5).ceil().as_uvec2().min(size))
        });
        let mut work = 0;
        for triangle in triangles {
            let [a, b, c] = triangle.map(|p| (p.xz() - origin) / spacing);
            let lo = a.min(b).min(c).floor().as_uvec2().max(min);
            let hi = a.max(b).max(c).ceil().as_uvec2().min(max);
            if lo.cmpge(hi).any() {
                continue;
            }
            work += u64::from(hi.x - lo.x) * u64::from(hi.y - lo.y);
            if work > MAX_RASTER_TESTS {
                return None;
            }
            let sign = (b - a).perp_dot(c - a).signum();
            let ab = (b - a) * sign;
            let bc = (c - b) * sign;
            let ca = (a - c) * sign;
            for z in lo.y..hi.y {
                let row = (z * size.x) as usize;
                let point = Vec2::new(lo.x as f32 + 0.5, z as f32 + 0.5);
                let mut edges = Vec3::new(
                    ab.perp_dot(point - a),
                    bc.perp_dot(point - b),
                    ca.perp_dot(point - c),
                );
                let dx = Vec3::new(-ab.y, -bc.y, -ca.y);
                for x in lo.x..hi.x {
                    if edges.min_element() >= -0.0001 {
                        depths[row + x as usize] = 255;
                    }
                    edges += dx;
                }
            }
        }
        if !depths.contains(&255) {
            return None;
        }
        taper_edges(&mut depths, size, spacing, edge_width);
        Some(Self {
            origin,
            size,
            spacing,
            depths,
        })
    }

    pub fn bounds(&self) -> Vec4 {
        let span = self.size.as_vec2() * self.spacing;
        self.origin.extend(span.x).extend(span.y)
    }

    pub fn at_world(&self, point: Vec2) -> f32 {
        let p = (point - self.origin) / self.spacing - 0.5;
        let base = p.floor();
        let frac = p - base;
        let base = base.as_ivec2();
        let sample = |offset: IVec2| {
            let p = base + offset;
            if p.cmplt(IVec2::ZERO).any() || p.cmpge(self.size.as_ivec2()).any() {
                return 0.0;
            }
            f32::from(self.depths[(p.y as u32 * self.size.x + p.x as u32) as usize]) / 255.0
        };
        let a = sample(IVec2::ZERO).lerp(sample(IVec2::X), frac.x);
        let b = sample(IVec2::Y).lerp(sample(IVec2::ONE), frac.x);
        a.lerp(b, frac.y)
    }

    pub fn into_image(self) -> Image {
        let mut image = Image::new(
            Extent3d {
                width: self.size.x,
                height: self.size.y,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            self.depths,
            TextureFormat::R8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        );
        image.sampler = ImageSampler::linear();
        image
    }
}

fn taper_edges(depths: &mut [u8], size: UVec2, spacing: f32, width: f32) {
    let edge = (255.0 * spacing / width).round().clamp(1.0, 255.0) as u8;
    let diagonal = (edge as f32 * std::f32::consts::SQRT_2).round() as u8;
    let row = size.x as usize;
    for z in 1..size.y as usize - 1 {
        for x in 1..row - 1 {
            let i = z * row + x;
            depths[i] = depths[i]
                .min(depths[i - 1].saturating_add(edge))
                .min(depths[i - row].saturating_add(edge))
                .min(depths[i - row - 1].saturating_add(diagonal))
                .min(depths[i - row + 1].saturating_add(diagonal));
        }
    }
    for z in (1..size.y as usize - 1).rev() {
        for x in (1..row - 1).rev() {
            let i = z * row + x;
            depths[i] = depths[i]
                .min(depths[i + 1].saturating_add(edge))
                .min(depths[i + row].saturating_add(edge))
                .min(depths[i + row - 1].saturating_add(diagonal))
                .min(depths[i + row + 1].saturating_add(diagonal));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_follows_outer_surface_and_authored_bounds_across_triangle_seams() {
        let a = Vec3::ZERO;
        let b = Vec3::X * 12.0;
        let c = Vec3::Z * 12.0;
        let cover = GroundCover::for_triangles(
            &[[a, b, c], [b, c, b + c]],
            Some([[0.0, 2.0], [10.0, 12.0]]),
            2.4,
        )
        .unwrap();
        assert_eq!(cover.at_world(Vec2::new(5.0, 1.0)), 0.0);
        let edge = cover.at_world(Vec2::new(5.0, 2.3));
        assert!(edge > 0.0 && edge < 0.35);
        assert!(cover.at_world(Vec2::new(5.0, 3.1)) > edge);
        assert_eq!(cover.at_world(Vec2::new(6.0, 6.0)), 1.0);
        assert_eq!(cover.at_world(Vec2::new(11.0, 6.0)), 0.0);
    }
}
