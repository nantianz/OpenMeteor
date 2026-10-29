pub use meteor_asset::amb::BlendedFrame;

use meteor_asset::amb::AmbFrame;
use meteor_core::Vec3;

pub fn root_pivot_delta(from: &AmbFrame, to: &AmbFrame) -> Vec3 {
    Vec3::new(
        to.root_pivot[0] - from.root_pivot[0],
        to.root_pivot[1] - from.root_pivot[1],
        to.root_pivot[2] - from.root_pivot[2],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use meteor_asset::amb::{parse_amb, AmbAnimation, BANIM_MAGIC};

    fn make_banim() -> Vec<u8> {
        let num_bones: i32 = 2;
        let num_dummy: i32 = 0;
        let num_frames: i32 = 3;
        let mut data = Vec::new();
        data.extend_from_slice(BANIM_MAGIC);
        data.extend_from_slice(&num_bones.to_le_bytes());
        data.extend_from_slice(&num_dummy.to_le_bytes());
        data.extend_from_slice(&num_frames.to_le_bytes());
        data.extend_from_slice(&num_bones.to_le_bytes());
        data.extend_from_slice(&(-1i32).to_le_bytes());
        data.extend_from_slice(&(0i32).to_le_bytes());
        for frame in 0..num_frames {
            let f = frame as f32;
            data.extend_from_slice(&f.to_le_bytes());
            data.extend_from_slice(&(f * 2.0).to_le_bytes());
            data.extend_from_slice(&(f * 3.0).to_le_bytes());

            data.extend_from_slice(&1.0f32.to_le_bytes());
            data.extend_from_slice(&0.0f32.to_le_bytes());
            data.extend_from_slice(&0.0f32.to_le_bytes());
            data.extend_from_slice(&0.0f32.to_le_bytes());

            let angle = f * std::f32::consts::FRAC_PI_4;
            let wq = (angle * 0.5).cos();
            let zq = (angle * 0.5).sin();
            data.extend_from_slice(&wq.to_le_bytes());
            data.extend_from_slice(&0.0f32.to_le_bytes());
            data.extend_from_slice(&0.0f32.to_le_bytes());
            data.extend_from_slice(&zq.to_le_bytes());
            data.extend_from_slice(&[0u8; 8]);
        }
        data
    }

    fn make_single_frame() -> Vec<u8> {
        let num_bones: i32 = 1;
        let num_dummy: i32 = 0;
        let num_frames: i32 = 1;
        let mut data = Vec::new();
        data.extend_from_slice(BANIM_MAGIC);
        data.extend_from_slice(&num_bones.to_le_bytes());
        data.extend_from_slice(&num_dummy.to_le_bytes());
        data.extend_from_slice(&num_frames.to_le_bytes());
        data.extend_from_slice(&num_bones.to_le_bytes());
        data.extend_from_slice(&(-1i32).to_le_bytes());
        data.extend_from_slice(&(0i32).to_le_bytes());
        data.extend_from_slice(&7.0f32.to_le_bytes());
        data.extend_from_slice(&8.0f32.to_le_bytes());
        data.extend_from_slice(&9.0f32.to_le_bytes());
        data.extend_from_slice(&1.0f32.to_le_bytes());
        data.extend_from_slice(&0.0f32.to_le_bytes());
        data.extend_from_slice(&0.0f32.to_le_bytes());
        data.extend_from_slice(&0.0f32.to_le_bytes());
        data.extend_from_slice(&[0u8; 8]);
        data
    }

    fn amb() -> AmbAnimation {
        parse_amb(&make_banim()).unwrap()
    }

    #[test]
    fn sample_at_integer_frame_returns_exact_frame() {
        let f = amb().sample_at(1.0);
        assert!(f.weight.abs() < 1e-6);
        assert!((f.root_pivot[0] - 1.0).abs() < 1e-5);
        assert!((f.root_pivot[1] - 2.0).abs() < 1e-5);
        assert!((f.root_pivot[2] - 3.0).abs() < 1e-5);
    }

    #[test]
    fn sample_at_clamps_below_zero_and_above_max() {
        let a = amb();
        let neg = a.sample_at(-5.0);
        assert!((neg.root_pivot[0] - 0.0).abs() < 1e-5);
        let over = a.sample_at(100.0);
        assert!((over.root_pivot[0] - 2.0).abs() < 1e-5);
        assert!((over.root_pivot[1] - 4.0).abs() < 1e-5);
    }

    #[test]
    fn sample_at_fractional_root_lerps() {
        let f = amb().sample_at(0.5);
        assert!((f.root_pivot[0] - 0.5).abs() < 1e-5);
        assert!((f.root_pivot[1] - 1.0).abs() < 1e-5);
        assert!((f.root_pivot[2] - 1.5).abs() < 1e-5);
        assert!((f.weight - 0.5).abs() < 1e-5);
    }

    #[test]
    fn sample_at_fractional_bone_slerps_shortest_path() {
        let f = amb().sample_at(0.5);

        let b1 = f.bone_quats[1];
        let expected_w = (std::f32::consts::FRAC_PI_8 * 0.5).cos();
        let expected_z = (std::f32::consts::FRAC_PI_8 * 0.5).sin();
        assert!((b1[3] - expected_w).abs() < 1e-4, "w {}", b1[3]);
        assert!((b1[2] - expected_z).abs() < 1e-4, "z {}", b1[2]);
    }

    #[test]
    fn sample_at_tiny_weight_returns_integer_frame() {
        let f = amb().sample_at(0.00001);
        assert!(f.weight.abs() < 1e-6);
        assert!((f.root_pivot[0] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn sample_at_single_frame_returns_exact_even_with_weight() {
        let a = parse_amb(&make_single_frame()).unwrap();
        let f = a.sample_at(0.7);
        assert!(f.weight.abs() < 1e-6);
        assert!((f.root_pivot[0] - 7.0).abs() < 1e-5);
    }

    #[test]
    fn root_pivot_delta_subtracts() {
        let a = amb();
        let f0 = a.sample(0);
        let f1 = a.sample(1);
        let d = root_pivot_delta(&f0, &f1);
        assert!((d.z - 3.0).abs() < 1e-5);
    }
}
