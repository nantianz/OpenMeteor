use meteor_core::FighterState;

pub mod pose {

    pub const IDLE: u16 = 0;

    pub const CROUCH_IDLE: u16 = 10;

    pub const WALK_F: u16 = 140;
    pub const WALK_R: u16 = 141;
    pub const WALK_L: u16 = 142;
    pub const WALK_B: u16 = 143;

    pub const RUN_F: u16 = 144;

    pub const CROUCH_F: u16 = 145;
    pub const CROUCH_R: u16 = 146;
    pub const CROUCH_L: u16 = 147;
    pub const CROUCH_B: u16 = 148;

    pub const SLOW_WALK: u16 = 150;

    pub const JUMP_F: u16 = 151;
    pub const JUMP_R: u16 = 153;
    pub const JUMP_L: u16 = 155;
    pub const JUMP_B: u16 = 157;

    pub const WALL_KICK_R: u16 = 159;
    pub const WALL_KICK_L: u16 = 160;

    pub const WALL_CLING_F: u16 = 161;
    pub const WALL_CLING_R: u16 = 162;
    pub const WALL_CLING_L: u16 = 163;

    pub const DODGE_F: u16 = 164;
    pub const DODGE_R: u16 = 167;
    pub const DODGE_L: u16 = 170;
    pub const DODGE_B: u16 = 173;

    pub const BURST_F: u16 = 176;
    pub const BURST_R: u16 = 177;
    pub const BURST_L: u16 = 178;
    pub const BURST_B: u16 = 179;

    pub const JUMP_ARC: u16 = 180;
}

const WALK4: [u16; 4] = [pose::WALK_F, pose::WALK_R, pose::WALK_B, pose::WALK_L];
const CROUCH4: [u16; 4] = [
    pose::CROUCH_F,
    pose::CROUCH_R,
    pose::CROUCH_B,
    pose::CROUCH_L,
];
const JUMP4: [u16; 4] = [pose::JUMP_F, pose::JUMP_R, pose::JUMP_B, pose::JUMP_L];
const DODGE4: [u16; 4] = [pose::DODGE_F, pose::DODGE_R, pose::DODGE_B, pose::DODGE_L];
const BURST4: [u16; 4] = [pose::BURST_F, pose::BURST_R, pose::BURST_B, pose::BURST_L];

const DODGE4_PLUS1: [u16; 4] = [165, 168, 174, 171];

const DODGE4_PLUS2: [u16; 4] = [166, 169, 175, 172];

pub fn dodge_table(strategy_id: Option<u8>) -> &'static [u16; 4] {
    match strategy_id {
        Some(0) | Some(5) => &DODGE4_PLUS1,
        Some(1) | Some(6) | Some(7) => &DODGE4_PLUS2,

        Some(9) => &[505, 506, 508, 507],
        Some(10) => &[509, 510, 512, 511],
        Some(11) => &[501, 502, 504, 503],
        _ => &DODGE4,
    }
}

pub const FIREARM_STRATEGY: u8 = 2;

pub const STAFF_BLADE_STRATEGY: u8 = 9;

pub const GUN_AIM_HOLD_POSE: u16 = 213;

const LOCKED_IDLE_BY_STRATEGY: [(u8, u16); 10] = [
    (0, 6),
    (1, 5),
    (3, 1),
    (4, 7),
    (5, 8),
    (6, 3),
    (7, 7),
    (8, 4),
    (10, 400),
    (11, 404),
];

const STAFF_BLADE_LOCKED_IDLE: [u16; 3] = [401, 402, 403];

const FAST_TARGET_POSES_BY_STRATEGY: [(u8, [u16; 7]); 12] = [
    (0, [60, 61, 62, 63, 80, 81, 82]),
    (1, [56, 57, 58, 59, 83, 84, 85]),
    (2, [44, 45, 46, 47, 77, 78, 79]),
    (3, [40, 41, 42, 43, 77, 78, 79]),
    (4, [72, 73, 74, 75, 77, 78, 79]),
    (5, [68, 69, 70, 71, 80, 81, 82]),
    (6, [48, 49, 50, 51, 83, 84, 85]),
    (7, [64, 65, 66, 67, 86, 87, 88]),
    (8, [52, 53, 54, 55, 77, 78, 79]),
    (9, [494, 495, 496, 497, 498, 499, 500]),
    (10, [490, 491, 492, 493, 513, 514, 515]),
    (11, [516, 517, 518, 519, 475, 476, 477]),
];

pub fn fast_blend_seconds(strategy_id: u8, pose_id: u16) -> Option<f32> {
    let row = FAST_TARGET_POSES_BY_STRATEGY
        .iter()
        .find(|(sid, _)| *sid == strategy_id)?;
    if row.1.contains(&pose_id) {
        return Some(0.01);
    }

    if strategy_id == 9 && pose_id == 481 {
        return Some(0.01);
    }
    None
}

const GUARD_STANCE_BY_STRATEGY: [(u8, u16); 12] = [
    (0, 16),
    (1, 15),
    (2, 12),
    (3, 11),
    (4, 19),
    (5, 18),
    (6, 13),
    (7, 17),
    (8, 14),
    (9, 481),
    (10, 480),
    (11, 482),
];

pub fn guard_pose_for_weapon(strategy_id: u8) -> Option<u16> {
    GUARD_STANCE_BY_STRATEGY
        .iter()
        .find(|(s, _)| *s == strategy_id)
        .map(|(_, p)| *p)
}

pub fn locked_idle_pose_for_weapon(strategy_id: u8, weapon_form: u8) -> Option<u16> {
    if strategy_id == STAFF_BLADE_STRATEGY {
        return STAFF_BLADE_LOCKED_IDLE
            .get(weapon_form as usize)
            .copied()
            .or(Some(STAFF_BLADE_LOCKED_IDLE[0]));
    }
    LOCKED_IDLE_BY_STRATEGY
        .iter()
        .find(|(s, _)| *s == strategy_id)
        .map(|(_, p)| *p)
}

pub fn remap_pose_by_strategy(
    pose_id: u16,
    strategy_id: Option<u8>,
    locked: bool,
    aiming: bool,
    weapon_form: u8,
) -> u16 {
    if strategy_id == Some(FIREARM_STRATEGY) {
        if aiming && pose_id == pose::IDLE {
            return GUN_AIM_HOLD_POSE;
        }

        return dodge_remap(pose_id, strategy_id).unwrap_or(pose_id);
    }
    if pose_id == pose::IDLE {
        if !locked {
            return pose_id;
        }
        return locked_idle_pose_for_weapon(strategy_id.unwrap_or(0), weapon_form)
            .unwrap_or(pose_id);
    }
    dodge_remap(pose_id, strategy_id).unwrap_or(pose_id)
}

fn dodge_remap(pose_id: u16, strategy_id: Option<u8>) -> Option<u16> {
    let table = dodge_table(strategy_id);
    match pose_id {
        pose::DODGE_F => Some(table[0]),
        pose::DODGE_R => Some(table[1]),
        pose::DODGE_B => Some(table[2]),
        pose::DODGE_L => Some(table[3]),
        _ => None,
    }
}

pub fn is_walk_pose_id(id: u16) -> bool {
    WALK4.contains(&id) || CROUCH4.contains(&id) || id == pose::RUN_F || id == pose::SLOW_WALK
}

pub fn is_crouch_walk_pose_id(id: u16) -> bool {
    CROUCH4.contains(&id)
}

pub fn can_keep_locomotion_phase(from_id: u16, to_id: u16) -> bool {
    if !is_walk_pose_id(from_id) || !is_walk_pose_id(to_id) {
        return false;
    }
    is_crouch_walk_pose_id(from_id) == is_crouch_walk_pose_id(to_id)
}

pub fn is_jump_pose_id(id: u16) -> bool {
    JUMP4.contains(&id)
        || id == 152
        || id == 154
        || id == 156
        || id == 158
        || is_wall_kick_pose_id(id)
        || is_wall_cling_pose_id(id)
        || id == pose::JUMP_ARC
}

pub fn is_wall_kick_pose_id(id: u16) -> bool {
    id == pose::WALL_KICK_L || id == pose::WALL_KICK_R
}

pub fn is_wall_cling_pose_id(id: u16) -> bool {
    id == pose::WALL_CLING_F || id == pose::WALL_CLING_L || id == pose::WALL_CLING_R
}

pub fn wall_kick_pose_for_cling(id: u16) -> u16 {
    match id {
        pose::WALL_CLING_F => pose::JUMP_B,
        pose::WALL_CLING_L => pose::WALL_KICK_L,
        pose::WALL_CLING_R => pose::WALL_KICK_R,
        _ => pose::JUMP_F,
    }
}

pub fn is_landing_pose_id(id: u16) -> bool {
    id == 112 || id == 113 || id == 117
}

pub fn is_dodge_pose_id(id: u16) -> bool {
    (164..=179).contains(&id) || (501..=512).contains(&id)
}

pub fn cardinal_from_input(x: f32, z: f32) -> Option<usize> {
    if x * x + z * z < 0.01 {
        return None;
    }
    if z > 0.01 {
        Some(0)
    } else if z < -0.01 {
        Some(2)
    } else if x > 0.01 {
        Some(1)
    } else if x < -0.01 {
        Some(3)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocoAxes {
    pub fb: i8,
    pub lr: i8,
    pub local_x: f32,
    pub local_z: f32,
    pub running: bool,
}

pub fn compose_loco_axes(x: f32, z: f32, forward_strafe_scale: f32) -> LocoAxes {
    let fb = if z > 0.01 {
        1
    } else if z < -0.01 {
        -1
    } else {
        0
    };
    let lr = if x > 0.01 {
        1
    } else if x < -0.01 {
        -1
    } else {
        0
    };
    let side_scale = if fb > 0 { forward_strafe_scale } else { 1.0 };
    LocoAxes {
        fb,
        lr,
        local_x: lr as f32 * side_scale,
        local_z: fb as f32,
        running: fb > 0,
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WalkOptions {
    pub crouching: bool,
    pub slow: bool,
    pub strategy_id: Option<u8>,
    pub locked: bool,
    pub aiming: bool,
    pub weapon_form: u8,
}

pub fn walk_pose_for_input(x: f32, z: f32, opts: WalkOptions) -> u16 {
    let card = cardinal_from_input(x, z);

    if card.is_none() {
        if opts.crouching {
            return pose::CROUCH_IDLE;
        }
        if opts.aiming && opts.strategy_id == Some(FIREARM_STRATEGY) {
            return GUN_AIM_HOLD_POSE;
        }
        return remap_pose_by_strategy(
            pose::IDLE,
            opts.strategy_id,
            opts.locked,
            opts.aiming,
            opts.weapon_form,
        );
    }
    let card = card.unwrap();
    if opts.crouching {
        return CROUCH4[card];
    }
    if opts.aiming && opts.strategy_id == Some(FIREARM_STRATEGY) {
        return CROUCH4[card];
    }

    let stand = if opts.slow && card == 0 {
        pose::SLOW_WALK
    } else if card == 0 {
        pose::RUN_F
    } else {
        WALK4[card]
    };
    remap_pose_by_strategy(
        stand,
        opts.strategy_id,
        opts.locked,
        opts.aiming,
        opts.weapon_form,
    )
}

pub fn jump_pose_for_input(x: f32, z: f32) -> u16 {
    match cardinal_from_input(x, z) {
        Some(card) => JUMP4[card],
        None => pose::JUMP_F,
    }
}

pub fn dodge_pose_for_input(
    x: f32,
    z: f32,
    burst: bool,
    roll: bool,
    strategy_id: Option<u8>,
) -> u16 {
    let card = cardinal_from_input(x, z);
    let table = if burst || roll {
        &BURST4
    } else {
        dodge_table(strategy_id)
    };
    match card {
        Some(c) => table[c],
        None => table[0],
    }
}

pub fn is_grounded_state(state: FighterState) -> bool {
    !matches!(state, FighterState::Jump | FighterState::Burst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cardinal_matches_exe_priority() {
        assert_eq!(cardinal_from_input(1.0, 1.0), Some(0));
        assert_eq!(cardinal_from_input(-1.0, -1.0), Some(2));
        assert_eq!(cardinal_from_input(1.0, 0.0), Some(1));
        assert_eq!(cardinal_from_input(-1.0, 0.0), Some(3));
        assert_eq!(cardinal_from_input(0.0, 0.0), None);

        assert_eq!(cardinal_from_input(0.005, 0.005), None);
    }

    #[test]
    fn walk_pose_forward_is_run() {
        assert_eq!(
            walk_pose_for_input(
                0.0,
                1.0,
                WalkOptions {
                    crouching: false,
                    slow: false,
                    strategy_id: None,
                    locked: false,
                    aiming: false,
                    weapon_form: 0
                }
            ),
            pose::RUN_F
        );

        assert_eq!(
            walk_pose_for_input(
                0.0,
                1.0,
                WalkOptions {
                    crouching: true,
                    slow: false,
                    strategy_id: None,
                    locked: false,
                    aiming: false,
                    weapon_form: 0
                }
            ),
            pose::CROUCH_F
        );

        assert_eq!(
            walk_pose_for_input(
                0.0,
                1.0,
                WalkOptions {
                    crouching: false,
                    slow: true,
                    strategy_id: None,
                    locked: false,
                    aiming: false,
                    weapon_form: 0
                }
            ),
            pose::SLOW_WALK
        );
    }

    #[test]
    fn walk_pose_strafe_and_back() {
        assert_eq!(
            walk_pose_for_input(
                1.0,
                0.0,
                WalkOptions {
                    crouching: false,
                    slow: false,
                    strategy_id: None,
                    locked: false,
                    aiming: false,
                    weapon_form: 0
                }
            ),
            pose::WALK_R
        );

        assert_eq!(
            walk_pose_for_input(
                0.0,
                -1.0,
                WalkOptions {
                    crouching: false,
                    slow: false,
                    strategy_id: None,
                    locked: false,
                    aiming: false,
                    weapon_form: 0
                }
            ),
            pose::WALK_B
        );

        assert_eq!(
            walk_pose_for_input(
                1.0,
                0.0,
                WalkOptions {
                    crouching: true,
                    slow: false,
                    strategy_id: None,
                    locked: false,
                    aiming: false,
                    weapon_form: 0
                }
            ),
            pose::CROUCH_R
        );
    }

    #[test]
    fn idle_remap_crouch_and_lock() {
        assert_eq!(
            walk_pose_for_input(
                0.0,
                0.0,
                WalkOptions {
                    crouching: true,
                    slow: false,
                    strategy_id: None,
                    locked: false,
                    aiming: false,
                    weapon_form: 0
                }
            ),
            pose::CROUCH_IDLE
        );

        assert_eq!(
            walk_pose_for_input(
                0.0,
                0.0,
                WalkOptions {
                    crouching: false,
                    slow: false,
                    strategy_id: Some(0),
                    locked: true,
                    aiming: false,
                    weapon_form: 0
                }
            ),
            6
        );

        assert_eq!(
            walk_pose_for_input(
                0.0,
                0.0,
                WalkOptions {
                    crouching: false,
                    slow: false,
                    strategy_id: Some(0),
                    locked: false,
                    aiming: false,
                    weapon_form: 0
                }
            ),
            0
        );
    }

    #[test]
    fn firearm_aim_uses_crouch_poses() {
        assert_eq!(
            walk_pose_for_input(
                0.0,
                0.0,
                WalkOptions {
                    crouching: false,
                    slow: false,
                    strategy_id: Some(2),
                    locked: false,
                    aiming: true,
                    weapon_form: 0
                }
            ),
            GUN_AIM_HOLD_POSE
        );

        assert_eq!(
            walk_pose_for_input(
                0.0,
                1.0,
                WalkOptions {
                    crouching: false,
                    slow: false,
                    strategy_id: Some(2),
                    locked: false,
                    aiming: true,
                    weapon_form: 0
                }
            ),
            pose::CROUCH_F
        );
    }

    #[test]
    fn dodge_table_by_strategy() {
        assert_eq!(dodge_table(Some(0))[0], 165);

        assert_eq!(dodge_table(Some(1))[0], 166);

        assert_eq!(dodge_table(Some(2))[0], 164);

        assert_eq!(dodge_table(Some(9)), &[505, 506, 508, 507]);
    }

    #[test]
    fn dodge_pose_uses_weapon_table() {
        assert_eq!(dodge_pose_for_input(0.0, 1.0, false, false, Some(0)), 165);

        assert_eq!(
            dodge_pose_for_input(0.0, 1.0, true, false, Some(0)),
            pose::BURST_F
        );

        assert_eq!(
            dodge_pose_for_input(0.0, 0.0, false, false, None),
            pose::DODGE_F
        );
    }

    #[test]
    fn jump_pose_defaults_forward() {
        assert_eq!(jump_pose_for_input(0.0, 1.0), pose::JUMP_F);
        assert_eq!(jump_pose_for_input(1.0, 0.0), pose::JUMP_R);
        assert_eq!(jump_pose_for_input(0.0, 0.0), pose::JUMP_F);
    }

    #[test]
    fn wall_kick_mapping() {
        assert_eq!(wall_kick_pose_for_cling(pose::WALL_CLING_F), pose::JUMP_B);
        assert_eq!(
            wall_kick_pose_for_cling(pose::WALL_CLING_L),
            pose::WALL_KICK_L
        );
        assert_eq!(
            wall_kick_pose_for_cling(pose::WALL_CLING_R),
            pose::WALL_KICK_R
        );
        assert_eq!(wall_kick_pose_for_cling(999), pose::JUMP_F);
    }

    #[test]
    fn pose_family_detection() {
        assert!(is_walk_pose_id(140));
        assert!(is_walk_pose_id(145));
        assert!(!is_walk_pose_id(10));
        assert!(is_crouch_walk_pose_id(145));
        assert!(!is_crouch_walk_pose_id(10));
        assert!(is_jump_pose_id(151));
        assert!(is_jump_pose_id(180));
        assert!(is_wall_cling_pose_id(162));
        assert!(is_wall_kick_pose_id(159));
        assert!(is_landing_pose_id(112));
        assert!(is_dodge_pose_id(164));
        assert!(is_dodge_pose_id(501));
        assert!(!is_dodge_pose_id(200));
    }

    #[test]
    fn locomotion_phase_preserved_only_same_family() {
        assert!(can_keep_locomotion_phase(140, 141));

        assert!(!can_keep_locomotion_phase(144, 145));

        assert!(!can_keep_locomotion_phase(0, 141));
    }

    #[test]
    fn compose_axes_lateral_scale() {
        let a = compose_loco_axes(1.0, 1.0, 0.2);
        assert_eq!(a.fb, 1);
        assert_eq!(a.lr, 1);
        assert!((a.local_x - 0.2).abs() < 1e-5);
        assert!(a.running);

        let b = compose_loco_axes(1.0, 0.0, 0.2);
        assert_eq!(b.fb, 0);
        assert!((b.local_x - 1.0).abs() < 1e-5);
        assert!(!b.running);
    }

    #[test]
    fn guard_stance_lookup() {
        assert_eq!(guard_pose_for_weapon(0), Some(16));
        assert_eq!(guard_pose_for_weapon(9), Some(481));
        assert_eq!(guard_pose_for_weapon(99), None);
    }
}
