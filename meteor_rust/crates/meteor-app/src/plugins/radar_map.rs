use super::stage_collider::StageMeshCollider;
use bevy::prelude::*;
use meteor_asset::texture::DecodedImage;

pub(super) fn terrain_image(
    mesh: &StageMeshCollider,
    bounds: (f32, f32, f32, f32),
) -> DecodedImage {
    let (min_x, max_x, min_z, max_z) = bounds;
    let scale = 1024.0 / (max_x - min_x).max(max_z - min_z);
    let width = ((max_x - min_x) * scale).round().max(2.0) as u32;
    let height = ((max_z - min_z) * scale).round().max(2.0) as u32;
    let project = |p: Vec3| {
        Vec2::new(
            (p.x - min_x) / (max_x - min_x) * (width - 1) as f32,
            (max_z - p.z) / (max_z - min_z) * (height - 1) as f32,
        )
    };
    let mut elevations = vec![f32::NEG_INFINITY; (width * height) as usize];
    let triangle = |id: u32| -> Option<[Vec3; 3]> {
        let indices = mesh.indices.get(id as usize * 3..id as usize * 3 + 3)?;
        let mut result = [Vec3::ZERO; 3];
        for (point, index) in result.iter_mut().zip(indices) {
            *point = Vec3::from_array(*mesh.positions.get(*index as usize)?);
            if !point.is_finite() {
                return None;
            }
        }
        Some(result)
    };

    for &id in &mesh.floor_tris {
        let Some(vertices) = triangle(id) else {
            continue;
        };
        let [a, b, c] = vertices.map(project);
        let area = (b - a).perp_dot(c - a);
        if area.abs() < 1e-6 {
            continue;
        }
        let lower = a.min(b).min(c).floor().max(Vec2::ZERO);
        let upper = a
            .max(b)
            .max(c)
            .ceil()
            .min(Vec2::new((width - 1) as f32, (height - 1) as f32));
        for y in lower.y as u32..=upper.y as u32 {
            for x in lower.x as u32..=upper.x as u32 {
                let p = Vec2::new(x as f32, y as f32);
                let u = (b - p).perp_dot(c - p) / area;
                let v = (c - p).perp_dot(a - p) / area;
                let w = 1.0 - u - v;
                if u.min(v).min(w) >= -1e-5 {
                    let h = u * vertices[0].y + v * vertices[1].y + w * vertices[2].y;
                    let pixel = &mut elevations[(y * width + x) as usize];
                    *pixel = pixel.max(h);
                }
            }
        }
    }
    let low = elevations
        .iter()
        .copied()
        .filter(|h| h.is_finite())
        .fold(f32::INFINITY, f32::min);
    let high = elevations
        .iter()
        .copied()
        .filter(|h| h.is_finite())
        .fold(f32::NEG_INFINITY, f32::max);
    let mut rgba = vec![0; (width * height * 4) as usize];
    for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
        let h = elevations[index];
        let shade = if h.is_finite() {
            ((h - low) / (high - low).max(1.0) * 28.0) as u8
        } else {
            0
        };
        let color = if h.is_finite() {
            [26 + shade, 30 + shade, 37 + shade, 235]
        } else {
            [10, 17, 23, 215]
        };
        pixel.copy_from_slice(&color);
    }

    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let i = (y * width + x) as usize;
            if elevations[i].is_finite()
                && [i - 1, i + 1, i - width as usize, i + width as usize]
                    .iter()
                    .any(|&n| !elevations[n].is_finite())
            {
                rgba[i * 4..i * 4 + 4].copy_from_slice(&[90, 102, 109, 255]);
            }
        }
    }
    for &id in &mesh.wall_tris {
        let Some(vertices) = triangle(id) else {
            continue;
        };
        let points = vertices.map(project);
        for (a, b) in [
            (points[0], points[1]),
            (points[1], points[2]),
            (points[2], points[0]),
        ] {
            let steps = (b - a).abs().max_element().ceil().max(1.0) as u32;
            for step in 0..=steps {
                let p = a.lerp(b, step as f32 / steps as f32).round();
                if p.x >= 0.0 && p.y >= 0.0 && p.x < width as f32 && p.y < height as f32 {
                    let i = (p.y as u32 * width + p.x as u32) as usize * 4;
                    rgba[i..i + 4].copy_from_slice(&[105, 115, 120, 255]);
                }
            }
        }
    }
    DecodedImage {
        width,
        height,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimap_floor_orientation_empty_space_and_invalid_faces() {
        let mut mesh = StageMeshCollider::default();
        mesh.positions = vec![[0.0, 0.0, 10.0], [5.0, 0.0, 10.0], [0.0, 0.0, 5.0]];
        mesh.indices = vec![0, 1, 2, 999, 0, 1];
        mesh.floor_tris = vec![0, 1, 999];
        let image = terrain_image(&mesh, (0.0, 20.0, 0.0, 10.0));
        assert_eq!((image.width, image.height), (1024, 512));
        let pixel = |x: usize, y: usize| &image.rgba[(y * 1024 + x) * 4..(y * 1024 + x) * 4 + 4];
        assert_eq!(pixel(10, 10), &[26, 30, 37, 235]);
        assert_eq!(pixel(1000, 490), &[10, 17, 23, 215]);
    }
}

pub(super) fn threat_glow_image() -> DecodedImage {
    let size = 128u32;
    let mut rgba = vec![0; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let p =
                (Vec2::new(x as f32, y as f32) + Vec2::splat(0.5)) / size as f32 - Vec2::splat(0.5);
            let radius = p.length();
            let angle = p.x.atan2(-p.y);
            let alpha = if radius < 0.5 {
                (-0.5 * ((radius - 0.465) / 0.021).powi(2) - 0.5 * (angle / 0.19).powi(2)).exp()
                    * ((0.5 - radius) / 0.012).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let i = (y * size + x) as usize * 4;
            rgba[i..i + 4].copy_from_slice(&[255, 255, 255, (alpha * 255.0) as u8]);
        }
    }
    DecodedImage {
        width: size,
        height: size,
        rgba,
    }
}

#[cfg(test)]
#[test]
fn radar_threat_glow_has_soft_edges_and_no_pixels_outside_circle() {
    let image = threat_glow_image();
    let mut soft_pixels = 0;
    for y in 0..image.height {
        for x in 0..image.width {
            let alpha = image.rgba[((y * image.width + x) * 4 + 3) as usize];
            let p = (Vec2::new(x as f32, y as f32) + Vec2::splat(0.5)) / image.width as f32
                - Vec2::splat(0.5);
            if p.length() >= 0.5 || p.y > 0.0 {
                assert_eq!(alpha, 0);
            }
            if alpha > 0 && alpha < 200 {
                soft_pixels += 1;
            }
        }
    }
    assert!(soft_pixels > 100);
}
