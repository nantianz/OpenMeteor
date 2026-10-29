use meteor_core::MODEL_SCALE;

pub const GROUND_SPEED_LIMIT: f32 = 180.0 * MODEL_SCALE;

pub const AIR_SPEED_LIMIT: f32 = 10000.0 * MODEL_SCALE;

pub const SIDE_LIMIT_FACTOR: f32 = 0.24;

pub const DRIVE_ACCEL: f32 = 1000.0 * MODEL_SCALE;

pub const NO_SUPPORT_ACCEL_SCALE: f32 = 0.1;

pub const FRICTION_BASE: f32 = 200.0 * MODEL_SCALE;

pub const GROUND_RESIST_SCALE: f32 = 10.0;

pub const AIR_RESIST_SCALE: f32 = 0.1;

pub const DRAG_COEFF: f32 = 1.0 / 600.0;

#[derive(Debug, Clone, Copy)]
pub struct DriveParams {
    pub dt: f32,

    pub speed_factor: f32,

    pub forward: bool,

    pub ground_support: bool,

    pub air_mode0: bool,

    pub action_vetoed: bool,
}

impl Default for DriveParams {
    fn default() -> Self {
        Self {
            dt: 0.0,
            speed_factor: 1.0,
            forward: false,
            ground_support: false,
            air_mode0: false,
            action_vetoed: false,
        }
    }
}

impl DriveParams {
    pub fn speed_limit(&self) -> f32 {
        let base = if self.air_mode0 {
            AIR_SPEED_LIMIT
        } else {
            GROUND_SPEED_LIMIT
        };
        let side = if self.forward { 1.0 } else { SIDE_LIMIT_FACTOR };
        base * self.speed_factor.max(0.0) * side
    }

    pub fn drive_accel(&self) -> f32 {
        let scale = if self.ground_support {
            1.0
        } else {
            NO_SUPPORT_ACCEL_SCALE
        };
        DRIVE_ACCEL * self.speed_factor.max(0.0) * scale
    }

    pub fn resist_scale(&self) -> f32 {
        if self.air_mode0 {
            AIR_RESIST_SCALE
        } else {
            GROUND_RESIST_SCALE
        }
    }
}

fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn normalized(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = length(v);
    if len <= 1e-6 {
        return None;
    }
    Some([v[0] / len, v[1] / len, v[2] / len])
}

pub fn input_accel(speed: f32, dir: [f32; 3], params: &DriveParams) -> [f32; 3] {
    if params.action_vetoed {
        return [0.0; 3];
    }
    let Some(unit) = normalized(dir) else {
        return [0.0; 3];
    };
    let limit = params.speed_limit();
    if limit <= 1e-6 {
        return [0.0; 3];
    }
    let r = speed / limit;
    if r >= 1.0 {
        return [0.0; 3];
    }
    let taper = 1.0 - r * r * r * r;
    let accel = params.drive_accel() * taper;
    [unit[0] * accel, unit[1] * accel, unit[2] * accel]
}

pub fn apply_friction(velocity: [f32; 3], dir: [f32; 3], params: &DriveParams) -> [f32; 3] {
    let speed = length(velocity);
    if speed <= 0.0 || params.dt <= 0.0 {
        return velocity;
    }
    let Some(vhat) = normalized(velocity) else {
        return velocity;
    };
    let magnitude = params.resist_scale() * FRICTION_BASE * params.dt;
    let mut friction = [
        -vhat[0] * magnitude,
        -vhat[1] * magnitude,
        -vhat[2] * magnitude,
    ];
    let cancel_dir = if params.action_vetoed { [0.0; 3] } else { dir };
    if let Some(ihat) = normalized(cancel_dir) {
        let dot = ihat[0] * vhat[0] + ihat[1] * vhat[1] + ihat[2] * vhat[2];
        if dot > 0.0 {
            let cancel = length(friction) * dot;
            friction[0] += ihat[0] * cancel;
            friction[1] += ihat[1] * cancel;
            friction[2] += ihat[2] * cancel;
        }
    }
    if length(friction) >= speed {
        return [0.0; 3];
    }
    [
        velocity[0] + friction[0],
        velocity[1] + friction[1],
        velocity[2] + friction[2],
    ]
}

pub fn apply_drag(velocity: [f32; 3], params: &DriveParams) -> [f32; 3] {
    if params.dt <= 0.0 {
        return velocity;
    }
    let src = [
        velocity[0] / MODEL_SCALE,
        velocity[1] / MODEL_SCALE,
        velocity[2] / MODEL_SCALE,
    ];
    let speed = length(src);
    if speed <= 1e-6 {
        return velocity;
    }
    let drag = speed * speed * params.resist_scale() * params.dt * DRAG_COEFF;
    if drag >= speed {
        return [0.0; 3];
    }
    let factor = 1.0 - drag / speed;
    [
        velocity[0] * factor,
        velocity[1] * factor,
        velocity[2] * factor,
    ]
}

pub fn tick_ground(velocity: [f32; 3], dir: [f32; 3], params: &DriveParams) -> [f32; 3] {
    let speed = length(velocity);
    let accel = input_accel(speed, dir, params);
    let mut v = apply_friction(velocity, dir, params);
    v[0] += accel[0] * params.dt;
    v[1] += accel[1] * params.dt;
    v[2] += accel[2] * params.dt;
    apply_drag(v, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ground(dt: f32) -> DriveParams {
        DriveParams {
            dt,
            forward: true,
            ground_support: true,
            ..Default::default()
        }
    }

    #[test]
    fn constants_match_source_units() {
        assert!((GROUND_SPEED_LIMIT - 21.6).abs() < 1e-4);
        assert!((GROUND_SPEED_LIMIT * SIDE_LIMIT_FACTOR - 5.184).abs() < 1e-4);
        assert!((AIR_SPEED_LIMIT - 1200.0).abs() < 1e-2);

        assert!((ground(0.0).drive_accel() - 120.0).abs() < 1e-4);

        let airborne = DriveParams {
            ground_support: false,
            ..ground(0.0)
        };
        assert!((airborne.drive_accel() - 12.0).abs() < 1e-4);
    }

    #[test]
    fn forward_ground_run_converges_below_the_180_limit() {
        let dt = 1.0 / 60.0;
        let mut v = [0.0f32; 3];
        for _ in 0..600 {
            v = tick_ground(v, [0.0, 0.0, 1.0], &ground(dt));
        }
        assert!(v[2] > 18.0 && v[2] < 19.5, "v={v:?}");
        assert!(v[2] < GROUND_SPEED_LIMIT, "v={v:?}");
    }

    #[test]
    fn side_limit_is_a_quarter_of_forward() {
        let dt = 1.0 / 60.0;
        let params = DriveParams {
            forward: false,
            ..ground(dt)
        };
        let mut v = [0.0f32; 3];
        for _ in 0..600 {
            v = tick_ground(v, [1.0, 0.0, 0.0], &params);
        }
        let limit = GROUND_SPEED_LIMIT * SIDE_LIMIT_FACTOR;

        assert!(v[0] > limit * 0.97 && v[0] <= limit + 1e-3, "v={v:?}");
    }

    #[test]
    fn releasing_input_brakes_hard_on_ground() {
        let dt = 1.0 / 60.0;
        let params = DriveParams {
            forward: false,
            ..ground(dt)
        };

        let mut v = [0.0, 0.0, GROUND_SPEED_LIMIT];
        for _ in 0..6 {
            v = tick_ground(v, [0.0; 3], &params);
        }
        assert_eq!(v, [0.0; 3]);
    }

    #[test]
    fn holding_input_is_not_braked_by_friction() {
        let params = ground(1.0 / 60.0);
        let v = apply_friction([0.0, 0.0, 10.0], [0.0, 0.0, 1.0], &params);
        assert!((v[2] - 10.0).abs() < 1e-5, "v={v:?}");
    }

    #[test]
    fn air_mode0_keeps_accelerating_past_the_ground_limit() {
        let params = DriveParams {
            dt: 1.0 / 60.0,
            forward: true,
            ground_support: false,
            air_mode0: true,
            ..Default::default()
        };
        let accel = input_accel(GROUND_SPEED_LIMIT, [0.0, 0.0, 1.0], &params);
        assert!(accel[2] > 11.9, "accel={accel:?}");
    }
}
