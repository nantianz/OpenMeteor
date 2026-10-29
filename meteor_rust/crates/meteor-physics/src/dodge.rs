pub const DODGE_ROOT_MOTION_SCALE: f32 = 1.0;

pub fn local_axis_for_pose(pose_id: u16) -> Option<(f32, f32)> {
    match pose_id {
        164..=166 => Some((0.0, 1.0)),
        167..=169 => Some((-1.0, 0.0)),
        170..=172 => Some((1.0, 0.0)),
        173..=175 => Some((0.0, -1.0)),

        176 => Some((0.0, 1.0)),
        177 => Some((-1.0, 0.0)),
        178 => Some((1.0, 0.0)),
        179 => Some((0.0, -1.0)),

        501..=512 => {
            let dir = (pose_id - 501) % 4;
            match dir {
                0 => Some((0.0, 1.0)),
                1 => Some((-1.0, 0.0)),
                2 => Some((1.0, 0.0)),
                _ => Some((0.0, -1.0)),
            }
        }
        _ => None,
    }
}

pub fn world_axis_for_pose(pose_id: u16, yaw: f32) -> Option<(f32, f32)> {
    let (lx, lz) = local_axis_for_pose(pose_id)?;
    let cos = yaw.cos();
    let sin = yaw.sin();
    Some((lx * cos + lz * sin, -lx * sin + lz * cos))
}

#[derive(Debug, Clone)]
pub struct DodgeState {
    pub active: bool,
    pub burst: bool,

    pub motion_axis: Option<(f32, f32)>,

    pub progress: f32,

    pub raw_progress: f32,

    pub prev_root_x: f32,
    pub prev_root_y: f32,

    pub root_started: bool,

    pub residual_x: f32,
    pub residual_z: f32,
}

impl DodgeState {
    pub fn new() -> Self {
        Self {
            active: false,
            burst: false,
            motion_axis: None,
            progress: 0.0,
            raw_progress: 0.0,
            prev_root_x: 0.0,
            prev_root_y: 0.0,
            root_started: false,
            residual_x: 0.0,
            residual_z: 0.0,
        }
    }

    pub fn start(&mut self, pose_id: u16, yaw: f32, burst: bool, planar_x: f32, planar_z: f32) {
        self.active = true;
        self.burst = burst;
        self.motion_axis = world_axis_for_pose(pose_id, yaw);
        self.progress = 0.0;
        self.raw_progress = 0.0;
        self.root_started = false;

        self.residual_x = planar_x;
        self.residual_z = planar_z;
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.residual_x = 0.0;
        self.residual_z = 0.0;
    }

    pub fn decay_residual(&mut self, dt: f32, damping: f32) {
        let scale = (1.0 - damping * dt).max(0.0);
        self.residual_x *= scale;
        self.residual_z *= scale;
    }
}

impl Default for DodgeState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_axes_base_dodge() {
        assert_eq!(local_axis_for_pose(164), Some((0.0, 1.0)));
        assert_eq!(local_axis_for_pose(165), Some((0.0, 1.0)));

        assert_eq!(local_axis_for_pose(167), Some((-1.0, 0.0)));

        assert_eq!(local_axis_for_pose(170), Some((1.0, 0.0)));

        assert_eq!(local_axis_for_pose(173), Some((0.0, -1.0)));
    }

    #[test]
    fn local_axes_burst() {
        assert_eq!(local_axis_for_pose(176), Some((0.0, 1.0)));
        assert_eq!(local_axis_for_pose(177), Some((-1.0, 0.0)));
        assert_eq!(local_axis_for_pose(178), Some((1.0, 0.0)));
        assert_eq!(local_axis_for_pose(179), Some((0.0, -1.0)));
    }

    #[test]
    fn local_axes_dedicated_remaps() {
        assert_eq!(local_axis_for_pose(501), Some((0.0, 1.0)));
        assert_eq!(local_axis_for_pose(502), Some((-1.0, 0.0)));
        assert_eq!(local_axis_for_pose(503), Some((1.0, 0.0)));
        assert_eq!(local_axis_for_pose(504), Some((0.0, -1.0)));

        assert_eq!(local_axis_for_pose(505), Some((0.0, 1.0)));
        assert_eq!(local_axis_for_pose(508), Some((0.0, -1.0)));

        assert_eq!(local_axis_for_pose(509), Some((0.0, 1.0)));
        assert_eq!(local_axis_for_pose(512), Some((0.0, -1.0)));
    }

    #[test]
    fn local_axis_unknown() {
        assert_eq!(local_axis_for_pose(0), None);
        assert_eq!(local_axis_for_pose(144), None);
        assert_eq!(local_axis_for_pose(513), None);
    }

    #[test]
    fn world_axis_forward_at_zero_yaw() {
        let (x, z) = world_axis_for_pose(164, 0.0).unwrap();
        assert!((x - 0.0).abs() < 1e-5);
        assert!((z - 1.0).abs() < 1e-5);
    }

    #[test]
    fn world_axis_rotates_with_yaw() {
        let (x, z) = world_axis_for_pose(164, std::f32::consts::FRAC_PI_2).unwrap();
        assert!((x - 1.0).abs() < 1e-5);
        assert!((z - 0.0).abs() < 1e-5);

        let (x, z) = world_axis_for_pose(167, std::f32::consts::FRAC_PI_2).unwrap();
        assert!((x - 0.0).abs() < 1e-5);
        assert!((z - 1.0).abs() < 1e-5);
    }

    #[test]
    fn start_banks_residual() {
        let mut d = DodgeState::new();
        d.start(164, 0.0, false, 12.0, 0.0);
        assert!(d.active);
        assert!(!d.burst);
        assert_eq!(d.motion_axis, Some((0.0, 1.0)));

        assert_eq!(d.residual_x, 12.0);
        assert_eq!(d.residual_z, 0.0);
    }

    #[test]
    fn residual_decays() {
        let mut d = DodgeState::new();
        d.start(164, 0.0, false, 12.0, 0.0);

        d.decay_residual(1.0 / 30.0, 9.0);
        assert!(d.residual_x < 12.0);
        assert!(d.residual_x > 0.0);

        for _ in 0..300 {
            d.decay_residual(1.0 / 30.0, 9.0);
        }

        assert!(d.residual_x.abs() < 1e-3);
    }

    #[test]
    fn stop_clears() {
        let mut d = DodgeState::new();
        d.start(164, 0.0, true, 5.0, 0.0);
        d.stop();
        assert!(!d.active);

        assert!(d.residual_x.abs() < 1e-3);
    }

    #[test]
    fn root_motion_scale_is_one() {
        assert_eq!(DODGE_ROOT_MOTION_SCALE, 1.0);
    }
}
