use meteor_core::MODEL_SCALE;

pub const LAND_SPEED_GATE: f32 = 1e-4 * MODEL_SCALE;

pub const LAND_EPS: f32 = 0.005;

pub const MAX_AIR_TIME: f32 = 2.5;

pub const SELF_LAUNCH_ATTACK_POSES: [u16; 9] = [203, 244, 253, 314, 325, 425, 471, 558, 562];

pub fn is_self_launch_attack_pose(pose_id: u16) -> bool {
    SELF_LAUNCH_ATTACK_POSES.contains(&pose_id)
}

pub const GRAVITY: f32 = -882.0 * MODEL_SCALE;

pub const JUMP_VERTICAL_SPEED: f32 = 282.0 * MODEL_SCALE;

pub const JUMP_VERTICAL_SPEED_STRONG: f32 = 375.0 * MODEL_SCALE;

pub const FALL_POSE_SPEED: f32 = -40.0 * MODEL_SCALE;

pub const AIR_INPUT_ACCEL: f32 =
    crate::planar_drive::DRIVE_ACCEL * crate::planar_drive::NO_SUPPORT_ACCEL_SCALE;

pub const AIR_INPUT_SPEED_LIMIT: f32 = crate::planar_drive::AIR_SPEED_LIMIT;

pub const AIR_INPUT_SIDE_LIMIT_FACTOR: f32 = crate::planar_drive::SIDE_LIMIT_FACTOR;

#[derive(Debug, Clone, Copy)]
pub struct AirInputDrive {
    pub speed_factor: f32,

    pub forward: bool,

    pub ground_contact: bool,

    pub action_vetoed: bool,
}

impl Default for AirInputDrive {
    fn default() -> Self {
        Self {
            speed_factor: 1.0,
            forward: false,
            ground_contact: false,
            action_vetoed: false,
        }
    }
}

pub const AIR_DRAG_COEFF: f32 = 0.1 / 600.0;

pub const WALL_KICK_SPEED: f32 = 180.0 * MODEL_SCALE;

pub const WALL_KICK_SPEED_STRONG: f32 = 260.0 * MODEL_SCALE;

pub const WALL_CLING_MAX_TIME: f32 = 1.8;

pub const WALL_VELOCITY_DAMPING: f32 = 1.2;

pub const WALL_GRAVITY_SCALE: f32 = 0.1;

pub const WALK_SPEED: f32 =
    crate::planar_drive::GROUND_SPEED_LIMIT * crate::planar_drive::SIDE_LIMIT_FACTOR;

pub const RUN_SPEED: f32 = 18.7;

pub const STEP_DOWN: f32 = 1.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirMotionMode {
    Ballistic,

    Wall,

    Curve,

    CurveExit,
}

pub fn fall_pose_keep(pose_id: u16) -> bool {
    matches!(pose_id, 152 | 154 | 156 | 158 | 180)
}

pub fn fall_pose_by_rise(pose_id: u16) -> Option<u16> {
    match pose_id {
        153 => Some(154),
        155 => Some(156),
        157 => Some(158),
        _ => None,
    }
}

pub fn fall_pose_for(pose_id: u16) -> u16 {
    if fall_pose_keep(pose_id) {
        return pose_id;
    }
    if let Some(mapped) = fall_pose_by_rise(pose_id) {
        return mapped;
    }

    152
}

#[derive(Debug, Clone, Copy)]
pub struct JumpStartInput {
    pub carry_x: f32,
    pub carry_z: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Impulse {
    pub x: f32,
    pub z: f32,
}

pub fn jump_impulse(input: JumpStartInput) -> Impulse {
    Impulse {
        x: input.carry_x,
        z: input.carry_z,
    }
}

pub fn integrate_vertical(
    vertical_speed: f32,
    planar_x: f32,
    planar_z: f32,
    dt: f32,
    gravity_scale: f32,
    apply_drag: bool,
) -> (f32, f32, f32, f32) {
    let mut vy = vertical_speed + GRAVITY * gravity_scale * dt;
    let mut px = planar_x;
    let mut pz = planar_z;

    if apply_drag {
        let vx_src = px / MODEL_SCALE;
        let vy_src = vy / MODEL_SCALE;
        let vz_src = pz / MODEL_SCALE;
        let speed = (vx_src * vx_src + vy_src * vy_src + vz_src * vz_src).sqrt();
        if speed > 1e-6 {
            let drag = speed * speed * AIR_DRAG_COEFF * dt;
            let factor = if drag < speed {
                1.0 - drag / speed
            } else {
                0.0
            };
            px *= factor;
            pz *= factor;
            vy *= factor;
        }
    }

    let displacement = vy * dt;
    (vy, displacement, px, pz)
}

pub fn lands_on_contact(saved_vertical_speed: f32) -> bool {
    saved_vertical_speed < LAND_SPEED_GATE
}

pub const AIR_WEAPON_SWITCH_DAMPING: f32 = 0.2;

pub fn apply_air_weapon_switch_damping(planar_x: f32, planar_z: f32, dt: f32) -> (f32, f32) {
    let factor = (1.0 - AIR_WEAPON_SWITCH_DAMPING * dt.max(0.0)).max(0.0);
    (planar_x * factor, planar_z * factor)
}

#[derive(Debug, Clone)]
pub struct AirState {
    pub airborne: bool,
    pub air_time: f32,

    pub vertical_speed: f32,

    pub planar_x: f32,
    pub planar_z: f32,
    pub motion_mode: AirMotionMode,

    pub wall_cling_remaining: f32,
    pub wall_collapsed: bool,

    pub cling_armed: bool,

    pub vertical_pose_id: u16,

    pub prev_root_z: Option<f32>,

    pub launch_lifted: bool,

    pub launch_pose_id: u16,
}

impl AirState {
    pub fn new() -> Self {
        Self {
            airborne: false,
            air_time: 0.0,
            vertical_speed: 0.0,
            planar_x: 0.0,
            planar_z: 0.0,
            motion_mode: AirMotionMode::Ballistic,
            wall_cling_remaining: 0.0,
            wall_collapsed: false,
            cling_armed: false,
            vertical_pose_id: 0,
            prev_root_z: None,
            launch_lifted: false,
            launch_pose_id: 0,
        }
    }

    pub fn start_jump(&mut self, strong: bool, impulse: Impulse) {
        self.airborne = true;
        self.air_time = 0.0;
        self.vertical_speed = if strong {
            JUMP_VERTICAL_SPEED_STRONG
        } else {
            JUMP_VERTICAL_SPEED
        };
        self.planar_x = impulse.x;
        self.planar_z = impulse.z;
        self.motion_mode = AirMotionMode::Ballistic;
        self.cling_armed = true;

        self.vertical_pose_id = 0;
        self.prev_root_z = None;
        self.launch_lifted = false;
        self.launch_pose_id = 0;
    }

    pub fn start_fall(&mut self, planar_x: f32, planar_z: f32) {
        self.airborne = true;
        self.air_time = 0.0;
        self.vertical_speed = 0.0;
        self.planar_x = planar_x;
        self.planar_z = planar_z;
        self.motion_mode = AirMotionMode::Ballistic;

        self.vertical_pose_id = 0;
        self.prev_root_z = None;
        self.launch_lifted = false;
        self.launch_pose_id = 0;
    }

    pub fn start_ground_launch(&mut self, pose_id: u16) {
        self.airborne = true;
        self.air_time = 0.0;
        self.vertical_speed = 0.0;
        self.planar_x = 0.0;
        self.planar_z = 0.0;
        self.motion_mode = AirMotionMode::Curve;
        self.wall_cling_remaining = 0.0;
        self.wall_collapsed = false;
        self.cling_armed = false;
        self.vertical_pose_id = pose_id;
        self.prev_root_z = None;
        self.launch_lifted = false;
        self.launch_pose_id = pose_id;
    }

    pub fn start_curve(&mut self, launching: bool, pose_id: u16) {
        self.motion_mode = AirMotionMode::Curve;
        if launching {
            self.launch_pose_id = pose_id;
            self.launch_lifted = false;
        }
    }

    pub fn land(&mut self) {
        self.airborne = false;
        self.air_time = 0.0;
        self.vertical_speed = 0.0;
        self.planar_x = 0.0;
        self.planar_z = 0.0;
        self.motion_mode = AirMotionMode::Ballistic;
        self.wall_cling_remaining = 0.0;
        self.wall_collapsed = false;
        self.cling_armed = false;
        self.vertical_pose_id = 0;
        self.prev_root_z = None;
        self.launch_lifted = false;
        self.launch_pose_id = 0;
    }

    pub const CURVE_EXIT_SPEED: f32 = -5.0 * MODEL_SCALE;

    pub fn tick_vertical(&mut self, dt: f32, grounded: bool) -> f32 {
        if !self.airborne {
            return 0.0;
        }
        self.air_time += dt;
        let saved_v = self.vertical_speed;
        let gravity_scale = if self.motion_mode == AirMotionMode::Wall {
            WALL_GRAVITY_SCALE
        } else {
            1.0
        };
        let apply_drag = self.motion_mode == AirMotionMode::Ballistic;
        let (new_v, disp, px, pz) = integrate_vertical(
            saved_v,
            self.planar_x,
            self.planar_z,
            dt,
            gravity_scale,
            apply_drag,
        );
        self.vertical_speed = new_v;
        if apply_drag {
            self.planar_x = px;
            self.planar_z = pz;
        }

        if grounded && lands_on_contact(saved_v) {
            self.land();
            return 0.0;
        }
        disp
    }

    pub fn tick_planar(
        &mut self,
        dt: f32,
        input_x: f32,
        input_z: f32,
        drive: AirInputDrive,
    ) -> (f32, f32) {
        if !self.airborne || self.motion_mode != AirMotionMode::Ballistic {
            return (self.planar_x, self.planar_z);
        }
        let params = crate::planar_drive::DriveParams {
            dt,
            speed_factor: drive.speed_factor.max(0.0),
            forward: drive.forward,
            ground_support: drive.ground_contact,
            air_mode0: true,
            action_vetoed: drive.action_vetoed,
        };
        let dir = [input_x, 0.0, input_z];

        let speed = self
            .planar_x
            .hypot(self.planar_z)
            .hypot(self.vertical_speed);
        let accel = crate::planar_drive::input_accel(speed, dir, &params);

        let braked = crate::planar_drive::apply_friction(
            [self.planar_x, self.vertical_speed, self.planar_z],
            dir,
            &params,
        );
        self.planar_x = braked[0] + accel[0] * dt;
        self.vertical_speed = braked[1];
        self.planar_z = braked[2] + accel[2] * dt;

        (self.planar_x, self.planar_z)
    }

    pub fn start_wall_cling(&mut self) {
        self.motion_mode = AirMotionMode::Wall;
        self.wall_cling_remaining = WALL_CLING_MAX_TIME;
        self.wall_collapsed = false;
    }

    pub fn tick_wall_cling(&mut self, dt: f32) -> bool {
        if self.motion_mode != AirMotionMode::Wall {
            return false;
        }
        self.wall_cling_remaining -= dt;
        if self.wall_cling_remaining <= 0.0 {
            self.wall_collapsed = true;
            self.motion_mode = AirMotionMode::Ballistic;
            return true;
        }
        false
    }
}

impl Default for AirState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::planar_drive;
    use meteor_core::MODEL_SCALE;

    #[test]
    fn gravity_matches_exe() {
        assert!((GRAVITY - (-882.0 * MODEL_SCALE)).abs() < 1e-5);
        assert!((JUMP_VERTICAL_SPEED - 282.0 * MODEL_SCALE).abs() < 1e-5);
        assert!((JUMP_VERTICAL_SPEED_STRONG - 375.0 * MODEL_SCALE).abs() < 1e-5);
        assert!((LAND_SPEED_GATE - 1e-4 * MODEL_SCALE).abs() < 1e-9);
    }

    #[test]
    fn self_launch_pose_set_matches_web_profile() {
        assert_eq!(
            SELF_LAUNCH_ATTACK_POSES,
            [203, 244, 253, 314, 325, 425, 471, 558, 562]
        );
        assert!(is_self_launch_attack_pose(253));
        assert!(is_self_launch_attack_pose(558));
        assert!(!is_self_launch_attack_pose(252));
    }

    #[test]
    fn ground_launch_starts_curve_without_jump_impulse() {
        let mut air = AirState::new();
        air.planar_x = 12.0;
        air.vertical_speed = -4.0;
        air.start_ground_launch(253);
        assert!(air.airborne);
        assert_eq!(air.motion_mode, AirMotionMode::Curve);
        assert_eq!(air.launch_pose_id, 253);
        assert!(!air.launch_lifted);
        assert_eq!(air.vertical_speed, 0.0);
        assert_eq!(air.planar_x, 0.0);
        assert!(!air.cling_armed);
    }

    #[test]
    fn jump_impulse_carries_velocity() {
        let i = jump_impulse(JumpStartInput {
            carry_x: 12.0,
            carry_z: -3.0,
        });
        assert!((i.x - 12.0).abs() < 1e-5);
        assert!((i.z + 3.0).abs() < 1e-5);
    }

    #[test]
    fn jump_impulse_never_seeds_speed_from_facing() {
        let i = jump_impulse(JumpStartInput {
            carry_x: 0.0,
            carry_z: 0.0,
        });
        assert_eq!(i.x, 0.0);
        assert_eq!(i.z, 0.0);
    }

    fn chained_forward_jump_count(strong: bool) -> f32 {
        use crate::planar_drive::{self, DriveParams};

        const PLATFORM: f32 = 1000.0 * MODEL_SCALE;
        let dt = 1.0 / 60.0;
        let ground = DriveParams {
            dt,
            forward: true,
            ground_support: true,
            ..Default::default()
        };
        let air_params = DriveParams {
            dt,
            forward: true,
            ground_support: false,
            air_mode0: true,
            ..Default::default()
        };
        let dir = [0.0, 0.0, 1.0];

        let mut v = [0.0f32; 3];
        for _ in 0..300 {
            v = planar_drive::tick_ground(v, dir, &ground);
        }

        let mut travelled = 0.0f32;
        let mut jumps = 0.0f32;
        while travelled < PLATFORM && jumps < 20.0 {
            let mut air = AirState::new();
            air.start_jump(strong, Impulse { x: v[0], z: v[2] });
            let mut y = 0.0f32;
            loop {
                let accel = planar_drive::input_accel(
                    air.planar_x.hypot(air.planar_z).hypot(air.vertical_speed),
                    dir,
                    &air_params,
                );
                air.planar_x += accel[0] * dt;
                air.planar_z += accel[2] * dt;
                let saved = air.vertical_speed;
                let disp = air.tick_vertical(dt, false);
                travelled += air.planar_z * dt;
                y += disp;
                if y <= 0.0 && lands_on_contact(saved) {
                    break;
                }
            }
            v = [air.planar_x, 0.0, air.planar_z];
            jumps += 1.0;

            for _ in 0..2 {
                v = planar_drive::tick_ground(v, dir, &ground);
            }
        }

        jumps
    }

    #[test]
    fn chained_forward_jumps_cross_the_sn06_center_platform_in_about_four_and_a_half() {
        let strong = chained_forward_jump_count(true);
        assert!(
            (4.0..=5.0).contains(&strong),
            "高跳连跳跨过 sn06 中央平台用了 {strong} 跳（实测基准 ≈4.5）"
        );

        let light = chained_forward_jump_count(false);
        assert!(light > strong, "light={light} strong={strong}");
    }

    #[test]
    fn vertical_integration_ascends_then_descends() {
        let mut air = AirState::new();
        air.start_jump(false, Impulse { x: 0.0, z: 0.0 });

        let d0 = air.tick_vertical(1.0 / 30.0, false);
        assert!(d0 > 0.0);

        assert!(air.vertical_speed < JUMP_VERTICAL_SPEED);
    }

    #[test]
    fn integrate_vertical_uses_semi_implicit_euler() {
        let (vy, disp_x, _px, _pz) = integrate_vertical(100.0, 0.0, 0.0, 1.0 / 30.0, 1.0, false);

        let expected = (100.0 + GRAVITY * (1.0 / 30.0)) * (1.0 / 30.0);
        assert!(
            (disp_x - expected).abs() < 1e-5,
            "disp={} expected={}",
            disp_x,
            expected
        );
        assert!((vy - (100.0 + GRAVITY * (1.0 / 30.0))).abs() < 1e-5);

        let explicit = 100.0 * (1.0 / 30.0) + 0.5 * GRAVITY * (1.0 / 30.0) * (1.0 / 30.0);
        assert!(
            (disp_x - explicit).abs() > 1e-4,
            "半隐式与显式 Euler 位移重合，回归"
        );

        assert_eq!(_px, 0.0);
        assert_eq!(_pz, 0.0);
    }

    #[test]
    fn landing_velocity_gate() {
        assert!(lands_on_contact(1e-6));
        assert!(lands_on_contact(0.0));
        assert!(lands_on_contact(-20.0));

        assert!(!lands_on_contact(JUMP_VERTICAL_SPEED));
        assert!(!lands_on_contact(1.0));
    }

    #[test]
    fn grounded_low_speed_lands() {
        let mut air = AirState::new();
        air.start_jump(false, Impulse { x: 0.0, z: 0.0 });

        air.vertical_speed = -50.0;
        air.tick_vertical(1.0 / 30.0, true);
        assert!(!air.airborne);
        assert_eq!(air.vertical_speed, 0.0);
    }

    #[test]
    fn max_air_time_does_not_force_land_without_contact() {
        let mut air = AirState::new();
        air.start_jump(true, Impulse { x: 0.0, z: 0.0 });
        for _ in 0..80 {
            air.tick_vertical(1.0 / 30.0, false);
        }
        assert!(air.airborne);
        assert!(air.air_time >= MAX_AIR_TIME);
    }

    #[test]
    fn fall_pose_mapping() {
        assert_eq!(fall_pose_for(152), 152);
        assert_eq!(fall_pose_for(180), 180);

        assert_eq!(fall_pose_for(153), 154);
        assert_eq!(fall_pose_for(155), 156);
        assert_eq!(fall_pose_for(157), 158);

        assert_eq!(fall_pose_for(151), 152);
        assert_eq!(fall_pose_for(161), 152);
        assert_eq!(fall_pose_for(159), 152);
    }

    #[test]
    fn airborne_drive_is_one_tenth_of_ground_contact_drive() {
        for (contact, expected) in [(false, 0.4), (true, 4.0)] {
            let mut air = AirState::new();
            air.start_fall(0.0, 0.0);
            let (_, z) = air.tick_planar(
                1.0 / 30.0,
                0.0,
                1.0,
                AirInputDrive {
                    forward: true,
                    ground_contact: contact,
                    ..Default::default()
                },
            );
            assert!((z - expected).abs() < 1e-5);
        }
    }

    #[test]
    fn neutral_takeoff_can_receive_weak_air_input() {
        let mut air = AirState::new();
        air.start_jump(false, Impulse { x: 0.0, z: 0.0 });
        let (x, z) = air.tick_planar(
            1.0 / 30.0,
            0.0,
            1.0,
            AirInputDrive {
                forward: true,
                ..Default::default()
            },
        );
        assert_eq!(x, 0.0);
        assert!(z > 0.399 && z <= 0.4);
    }

    #[test]
    fn airborne_taper_uses_three_dimensional_speed_and_fourth_power() {
        for (forward, ceiling) in [(true, 1200.0), (false, 288.0)] {
            let mut air = AirState::new();
            air.start_fall(0.0, 0.0);
            air.vertical_speed = ceiling * 0.5;
            let (_, z) = air.tick_planar(
                1.0 / 30.0,
                0.0,
                1.0,
                AirInputDrive {
                    forward,
                    ..Default::default()
                },
            );

            assert!((z - 0.375).abs() < 1e-5);
            air.planar_z = 0.0;
            air.vertical_speed = ceiling;
            air.tick_planar(
                1.0 / 30.0,
                0.0,
                1.0,
                AirInputDrive {
                    forward,
                    ..Default::default()
                },
            );
            assert_eq!(air.planar_z, 0.0);
        }
    }

    #[test]
    fn speed_buff_scales_air_acceleration_and_taper_ceiling() {
        let mut air = AirState::new();
        air.start_fall(0.0, 0.0);
        air.vertical_speed = 900.0;
        air.tick_planar(
            1.0 / 30.0,
            0.0,
            1.0,
            AirInputDrive {
                forward: true,
                speed_factor: 1.5,
                ..Default::default()
            },
        );
        assert!((air.planar_z - 0.5625).abs() < 1e-5);
    }

    #[test]
    fn air_drive_preserves_inertia_and_bounds_reverse_braking_after_collision() {
        for initial in [0.0, 4.0, 12.0, 30.0] {
            for (ix, iz) in [(1.0, 0.0), (0.0, -1.0), (0.0, 0.0)] {
                let mut air = AirState::new();
                air.start_fall(0.0, initial);
                air.tick_planar(1.0 / 30.0, ix, iz, AirInputDrive::default());

                let friction =
                    planar_drive::AIR_RESIST_SCALE * planar_drive::FRICTION_BASE * (1.0 / 30.0);
                let step = 0.4 + friction;
                assert!(air.planar_x.abs() <= step + 1e-5);
                assert!((air.planar_z - initial).abs() <= step + 1e-5);
                if iz == 0.0 {
                    let expected = if initial > 0.0 {
                        initial - friction
                    } else {
                        0.0
                    };
                    assert!(
                        (air.planar_z - expected).abs() < 1e-5,
                        "initial={initial} got={} expected={expected}",
                        air.planar_z
                    );
                }
            }
        }
    }

    #[test]
    fn curve_wall_and_weapon_veto_do_not_receive_air_drive() {
        for mode in [
            AirMotionMode::Curve,
            AirMotionMode::CurveExit,
            AirMotionMode::Wall,
        ] {
            let mut air = AirState::new();
            air.start_fall(3.0, 4.0);
            air.motion_mode = mode;
            let velocity = air.tick_planar(1.0 / 30.0, 1.0, 0.0, AirInputDrive::default());
            assert_eq!(velocity, (3.0, 4.0));
        }

        let mut air = AirState::new();
        air.start_fall(3.0, 4.0);
        let (px, pz) = air.tick_planar(
            1.0 / 30.0,
            1.0,
            0.0,
            AirInputDrive {
                action_vetoed: true,
                ..Default::default()
            },
        );

        let friction = planar_drive::AIR_RESIST_SCALE * planar_drive::FRICTION_BASE * (1.0 / 30.0);
        let speed = 5.0f32;
        assert!(
            (px - (3.0 - friction * 3.0 / speed)).abs() < 1e-5,
            "px={px}"
        );
        assert!(
            (pz - (4.0 - friction * 4.0 / speed)).abs() < 1e-5,
            "pz={pz}"
        );
    }

    #[test]
    fn planar_no_input_drags() {
        let mut air = AirState::new();
        air.start_jump(true, Impulse { x: 12.0, z: 0.0 });
        let before = air.planar_x;

        let _ = air.tick_planar(1.0 / 30.0, 0.0, 0.0, AirInputDrive::default());
        let _ = air.tick_vertical(1.0 / 30.0, false);
        assert!(air.planar_x.abs() < before.abs());
    }

    #[test]
    fn wall_cling_times_out() {
        let mut air = AirState::new();
        air.start_jump(false, Impulse { x: 0.0, z: 0.0 });
        air.start_wall_cling();
        assert_eq!(air.motion_mode, AirMotionMode::Wall);

        let collapsed = air.tick_wall_cling(2.0);
        assert!(collapsed);
        assert!(air.wall_collapsed);
        assert_eq!(air.motion_mode, AirMotionMode::Ballistic);
    }

    #[test]
    fn landing_clears_every_air_track_for_the_next_jump() {
        let mut air = AirState {
            airborne: true,
            air_time: 1.2,
            vertical_speed: -8.0,
            planar_x: 3.0,
            planar_z: 4.0,
            motion_mode: AirMotionMode::Curve,
            wall_cling_remaining: 0.7,
            wall_collapsed: true,
            cling_armed: true,
            vertical_pose_id: 253,
            prev_root_z: Some(18.0),
            launch_lifted: true,
            launch_pose_id: 253,
        };
        air.land();
        assert!(!air.airborne);
        assert_eq!(air.air_time, 0.0);
        assert_eq!(air.vertical_speed, 0.0);
        assert_eq!(air.planar_x, 0.0);
        assert_eq!(air.planar_z, 0.0);
        assert_eq!(air.motion_mode, AirMotionMode::Ballistic);
        assert_eq!(air.wall_cling_remaining, 0.0);
        assert!(!air.wall_collapsed);
        assert!(!air.cling_armed);
        assert_eq!(air.vertical_pose_id, 0);
        assert_eq!(air.prev_root_z, None);
        assert!(!air.launch_lifted);
        assert_eq!(air.launch_pose_id, 0);
    }
}
