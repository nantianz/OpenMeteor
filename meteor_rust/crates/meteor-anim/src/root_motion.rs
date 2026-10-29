use meteor_core::constants::MODEL_SCALE;

pub const ROOT_MOTION_SCALE: f32 = 1.0;

pub fn root_delta_to_world_xz(
    d_model_x: f32,
    d_model_y: f32,
    yaw: f32,
    motion_scale: f32,
) -> (f32, f32) {
    let local_x = d_model_x * MODEL_SCALE * motion_scale;
    let local_z = -d_model_y * MODEL_SCALE * motion_scale;
    let cos = yaw.cos();
    let sin = yaw.sin();
    let x = local_x * cos + local_z * sin;
    let z = -local_x * sin + local_z * cos;
    (x, z)
}

pub fn root_delta_to_world_xz_default(d_model_x: f32, d_model_y: f32, yaw: f32) -> (f32, f32) {
    root_delta_to_world_xz(d_model_x, d_model_y, yaw, ROOT_MOTION_SCALE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_zero_maps_axis_aligned() {
        let (x, z) = root_delta_to_world_xz_default(10.0, 20.0, 0.0);

        assert!((x - 1.2).abs() < 1e-4);
        assert!((z - -2.4).abs() < 1e-4);
    }

    #[test]
    fn motion_scale_one_matches_combat_paths() {
        let (x, z) = root_delta_to_world_xz(10.0, 20.0, 0.0, 1.0);

        assert!((x - 1.2).abs() < 1e-4);
        assert!((z - -2.4).abs() < 1e-4);
    }

    #[test]
    fn yaw_quarter_turn_rotates_vector() {
        let (x, z) = root_delta_to_world_xz_default(10.0, 0.0, std::f32::consts::FRAC_PI_2);

        assert!(x.abs() < 1e-4);
        assert!((z - -1.2).abs() < 1e-4);
    }

    #[test]
    fn zero_delta_is_zero_world() {
        let (x, z) = root_delta_to_world_xz_default(0.0, 0.0, 0.5);
        assert!(x.abs() < 1e-6);
        assert!(z.abs() < 1e-6);
    }
}
