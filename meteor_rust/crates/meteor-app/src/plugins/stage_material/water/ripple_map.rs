pub(super) fn generate() -> meteor_asset::texture::DecodedImage {
    let vectors: [(f64, f64); 12] = [
        (1.0, 2.0), (2.0, -3.0), (-3.0, 1.0), (4.0, 1.0),
        (-1.0, 4.0), (3.0, 3.0), (-4.0, 2.0), (2.0, 5.0),
        (-5.0, -2.0), (5.0, -1.0), (4.0, -4.0), (-3.0, -5.0),
    ];
    let mut rgba = Vec::with_capacity(128 * 128 * 4);
    for y in 0..128 {
        for x in 0..128 {
            let x = x as f64 * (std::f64::consts::TAU / 128.0);
            let y = y as f64 * (std::f64::consts::TAU / 128.0);
            let (mut dx, mut dy) = (0.0, 0.0);
            for (i, (kx, ky)) in vectors.iter().copied().enumerate() {
                let amplitude = 0.07 / kx.hypot(ky).powf(1.25);
                let i = i as f64;
                let phase = (i * i * 1.6180339 + i * 0.37 + 0.13) * std::f64::consts::TAU;
                let c = (kx * x + ky * y + phase).cos();
                dx += kx * amplitude * c;
                dy += ky * amplitude * c;
            }
            let length = (dx * dx + dy * dy + 1.0).sqrt();
            for value in [-dx, -dy, 1.0] {
                rgba.push(((value / length * 0.5 + 0.5) * 255.0).round_ties_even().clamp(0.0, 255.0) as u8);
            }
            rgba.push(255);
        }
    }
    meteor_asset::texture::DecodedImage { width: 128, height: 128, rgba }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    #[test]
    fn procedural_ripples_match_the_reference_texture() {
        let map = super::generate();
        assert_eq!((map.width, map.height), (128, 128));
        assert_eq!(format!("{:x}", Sha256::digest(&map.rgba)), "eaf18157f36f30f6f1d0fbab53e7e1b0f39e21d88b8aa4d4724230df93790f95");
    }
}
