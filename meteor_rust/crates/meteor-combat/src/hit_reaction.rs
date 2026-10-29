use meteor_asset::pos::AttackBlock;
use meteor_core::FighterState;

pub const HIT_REACTION_GRAVITY: f32 = -882.0 * 0.12;

pub const HIT_REACTION_ANIM_MIN_SPEED: f32 = 1.0;

pub const DOWN_MIN_SECONDS: f32 = 0.0;

pub const ENVIRONMENT_HIT_POSE: u16 = 90;

pub const DEATH_POSES: [u16; 5] = [117, 114, 115, 112, 113];

pub const DOWN_HOLD_POSES: [u16; 2] = [112, 113];

pub const GETUP_FALLBACK_POSES: [u16; 7] = [119, 120, 116, 114, 115, 118, 0];

pub const LIGHT_HIT_FALLBACK_POSES: [u16; 10] = [93, 90, 91, 92, 94, 95, 96, 97, 98, 99];

pub const FAST_TARGET_POSES_BY_STRATEGY: [[u16; 7]; 12] = [
    [60, 61, 62, 63, 80, 81, 82],
    [56, 57, 58, 59, 83, 84, 85],
    [44, 45, 46, 47, 77, 78, 79],
    [40, 41, 42, 43, 77, 78, 79],
    [72, 73, 74, 75, 77, 78, 79],
    [68, 69, 70, 71, 80, 81, 82],
    [48, 49, 50, 51, 83, 84, 85],
    [64, 65, 66, 67, 86, 87, 88],
    [52, 53, 54, 55, 77, 78, 79],
    [494, 495, 496, 497, 498, 499, 500],
    [490, 491, 492, 493, 513, 514, 515],
    [516, 517, 518, 519, 475, 476, 477],
];

pub fn guard_reaction_pose_id(
    strategy_id: u8,
    selector: u32,
    poses: &[u16],
    front_random_index: u32,
) -> Option<u16> {
    let table = FAST_TARGET_POSES_BY_STRATEGY.get(strategy_id as usize)?;
    let index = match selector {
        0 => (front_random_index % 4) as usize,
        1..=3 => selector as usize + 3,
        _ => return None,
    };
    let selected = table[index];
    poses
        .contains(&selected)
        .then_some(selected)
        .or_else(|| table[..4].iter().copied().find(|id| poses.contains(id)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacingBucket {
    Front,
    Back,
    Left,
    Right,
    Default,
}

pub fn hit_direction_bits(bucket: FacingBucket) -> u32 {
    match bucket {
        FacingBucket::Front => 0x0000,
        FacingBucket::Left => 0x2000,
        FacingBucket::Right => 0x1000,
        FacingBucket::Back => 0x3000,
        FacingBucket::Default => 0x0000,
    }
}

pub fn facing_bucket(
    attacker_x: f32,
    attacker_z: f32,
    defender_x: f32,
    defender_z: f32,
    defender_yaw: f32,
) -> FacingBucket {
    let dx = attacker_x - defender_x;
    let dz = attacker_z - defender_z;
    let to_attacker = dx.atan2(dz);
    let mut rel = to_attacker - defender_yaw;
    while rel > std::f32::consts::PI {
        rel -= std::f32::consts::PI * 2.0;
    }
    while rel < -std::f32::consts::PI {
        rel += std::f32::consts::PI * 2.0;
    }

    let rel = f64::from(rel);
    let quarter = 0.7853975_f64;
    let three_quarters = 2.3561924999999997_f64;
    if rel > -quarter && rel <= quarter {
        FacingBucket::Front
    } else if rel > quarter && rel <= three_quarters {
        FacingBucket::Left
    } else if rel >= -three_quarters && rel <= -quarter {
        FacingBucket::Right
    } else {
        FacingBucket::Back
    }
}

pub fn pack_target_pose_with_direction(pose_id: u16, bucket: FacingBucket) -> u32 {
    (pose_id as u32) | hit_direction_bits(bucket)
}

pub fn pick_reaction_pose_id(
    attack: &AttackBlock,
    bucket: FacingBucket,
    poses: &[u16],
) -> Option<u16> {
    let directed = match bucket {
        FacingBucket::Front => attack.target_pose_front,
        FacingBucket::Back => attack.target_pose_back,
        FacingBucket::Left => attack.target_pose_left,
        FacingBucket::Right => attack.target_pose_right,
        FacingBucket::Default => attack.target_pose,
    };
    for id in [directed, attack.target_pose].into_iter().flatten() {
        if poses.contains(&id) {
            return Some(id);
        }
    }
    LIGHT_HIT_FALLBACK_POSES
        .into_iter()
        .find(|&id| poses.contains(&id))
}

pub fn pose_duration_seconds(start: u32, end: u32) -> f32 {
    let frames = (end.saturating_sub(start) + 1).max(1);
    if frames <= 1 {
        0.0
    } else {
        frames as f32 / 30.0
    }
}

#[derive(Debug, Clone)]
pub struct HitReact {
    pub kind: HitReactKind,
    pub pose_id: Option<u16>,

    pub head_pose_id: Option<u16>,
    pub lock_remaining: f32,
    pub minimum_remaining: f32,

    pub authored_minimum: f32,

    pub motion_scale: f32,
    pub down_remaining: f32,

    pub bucket: FacingBucket,

    pub environmental: bool,

    pub selector: u32,

    pub rewind_count: u32,

    pub play_to_end_pose_id: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitReactKind {
    BeHit,
    Guard,
    Roll,
    Down,
    GetUp,
    Dead,
}

impl HitReact {
    fn new(kind: HitReactKind, pose_id: Option<u16>) -> Self {
        Self {
            kind,
            pose_id,
            head_pose_id: None,
            lock_remaining: 0.0,
            minimum_remaining: 0.0,
            authored_minimum: 0.0,
            motion_scale: 1.0,
            down_remaining: 0.0,
            bucket: FacingBucket::Default,
            environmental: false,
            selector: 0,
            rewind_count: 0,
            play_to_end_pose_id: None,
        }
    }
}

pub trait ReactionFighter {
    fn pose_id(&self) -> u16;
    fn is_pose_finished(&self) -> bool;
    fn has_pose(&self, pose_id: u16) -> bool;
    fn play_pose(&mut self, pose_id: u16, hard: bool);

    fn play_pose_hard_cut(&mut self, pose_id: u16) {
        self.play_pose(pose_id, true);
    }
    fn set_in_guard(&mut self, v: bool);
    fn current_state(&self) -> FighterState;
    fn set_state(&mut self, state: FighterState, locked: bool);
    fn alive(&self) -> bool;
    fn guard_held(&self) -> bool;

    fn pose_link(&self, pose_id: u16) -> Option<u16>;

    fn is_airborne(&self) -> bool;

    fn pose_range(&self, pose_id: u16) -> Option<(u32, u32)>;

    fn release_pose_loop_to_end(&mut self, pose_id: u16);

    fn rewind_reaction_pose(&mut self, pose_id: u16);
}

pub fn begin_be_hit(
    fighter: &mut impl ReactionFighter,
    attack: Option<&AttackBlock>,
    bucket: FacingBucket,
    poses: &[u16],
) -> Option<HitReact> {
    let pose_id = attack
        .and_then(|a| pick_reaction_pose_id(a, bucket, poses))
        .or_else(|| {
            poses
                .iter()
                .copied()
                .find(|p| LIGHT_HIT_FALLBACK_POSES.contains(p))
        });

    let (move_val, target_value) = attack
        .map(|a| (a.target_move, a.target_value.max(0.0)))
        .unwrap_or((0.0, 0.0));

    let lock_seconds = if let Some(id) = pose_id {
        let (start, end) = pose_range(fighter, id);
        target_value.max(pose_duration_seconds(start, end))
    } else {
        target_value.max(0.25)
    };

    let mut hr = HitReact::new(HitReactKind::BeHit, pose_id);
    hr.lock_remaining = lock_seconds;
    hr.minimum_remaining = target_value;
    hr.authored_minimum = target_value;
    hr.motion_scale = move_val;
    hr.bucket = bucket;
    fighter.set_in_guard(false);
    if let Some(id) = pose_id {
        fighter.play_pose(id, true);
    }
    Some(hr)
}

pub fn begin_environmental_hit(
    fighter: &mut impl ReactionFighter,
    poses: &[u16],
) -> Option<HitReact> {
    if !fighter.alive() || !poses.contains(&ENVIRONMENT_HIT_POSE) {
        return None;
    }
    let (start, end) = pose_range(fighter, ENVIRONMENT_HIT_POSE);
    let lock_seconds = pose_duration_seconds(start, end).max(0.25);
    let mut hr = HitReact::new(HitReactKind::BeHit, Some(ENVIRONMENT_HIT_POSE));
    hr.lock_remaining = lock_seconds;
    hr.minimum_remaining = 0.0;
    hr.authored_minimum = 0.0;
    hr.motion_scale = 1.0;
    hr.bucket = FacingBucket::Default;
    hr.environmental = true;
    fighter.set_in_guard(false);
    fighter.play_pose(ENVIRONMENT_HIT_POSE, true);
    Some(hr)
}

pub fn begin_death(fighter: &mut impl ReactionFighter, attack: Option<&AttackBlock>) -> HitReact {
    let pose_id = DEATH_POSES.iter().copied().find(|id| fighter.has_pose(*id));
    let move_val = attack.map(|a| a.target_move).unwrap_or(0.0);
    let mut hr = HitReact::new(HitReactKind::Dead, pose_id);
    hr.lock_remaining = f32::INFINITY;
    hr.minimum_remaining = 0.0;
    hr.authored_minimum = 0.0;
    hr.motion_scale = move_val;
    fighter.set_in_guard(false);
    if let Some(id) = pose_id {
        fighter.play_pose(id, true);
    }
    hr
}

pub fn begin_guard_hit(
    fighter: &mut impl ReactionFighter,
    attack: Option<&AttackBlock>,
    bucket: FacingBucket,
    poses: &[u16],
    strategy_id: u8,
    head_pose_id: Option<u16>,
    random_selector: u32,
) -> Option<HitReact> {
    let (move_val, defense_value) = attack
        .map(|a| (a.defense_move, a.defense_value.max(0.0)))
        .unwrap_or((0.0, 0.0));

    let directed = attack.and_then(|a| pick_reaction_pose_id(a, bucket, poses));
    let packed = pack_target_pose_with_direction(directed.unwrap_or(0), bucket);
    let selector = (packed >> 12) & 0xf;

    let react_pose = guard_reaction_pose_id(strategy_id, selector, poses, random_selector)
        .or(head_pose_id.filter(|id| poses.contains(id)));
    let lock_seconds = if let Some(id) = react_pose {
        let (start, end) = pose_range(fighter, id);
        defense_value.max(pose_duration_seconds(start, end))
    } else {
        defense_value.max(0.25)
    };

    let mut hr = HitReact::new(HitReactKind::Guard, react_pose);
    hr.head_pose_id = head_pose_id;
    hr.lock_remaining = lock_seconds;
    hr.minimum_remaining = defense_value;
    hr.authored_minimum = defense_value;
    hr.motion_scale = move_val;
    hr.bucket = bucket;
    hr.selector = selector;
    if let Some(id) = react_pose {
        if Some(id) != head_pose_id {
            fighter.play_pose(id, true);
        }
    }
    Some(hr)
}

pub fn apply_hit_reaction(
    outcome: &crate::hit_resolve::HitOutcome,
    fighter: &mut impl ReactionFighter,
    attack: Option<&AttackBlock>,
    bucket: FacingBucket,
    poses: &[u16],
    strategy_id: u8,
    head_pose_id: Option<u16>,
) -> Option<HitReact> {
    match outcome {
        crate::hit_resolve::HitOutcome::Guard => {
            begin_guard_hit(fighter, attack, bucket, poses, strategy_id, head_pose_id, 0)
        }
        _ => begin_be_hit(fighter, attack, bucket, poses),
    }
}

pub fn tick_hit_react(
    fighter: &mut impl ReactionFighter,
    mut hr: HitReact,
    dt: f32,
) -> Option<HitReact> {
    if !fighter.alive() {
        if hr.kind != HitReactKind::Dead {
            return Some(begin_death(fighter, None));
        }
        return Some(hr);
    }

    let t = dt.max(0.0);

    match hr.kind {
        HitReactKind::Guard => {
            hr.lock_remaining -= t;
            hr.minimum_remaining = (hr.minimum_remaining - t).max(0.0);
            let link = hr.pose_id.and_then(|id| fighter.pose_link(id));
            if hr.minimum_remaining <= 0.0 && link.is_none() && hr.play_to_end_pose_id.is_none() {
                if let Some(id) = hr.pose_id {
                    fighter.release_pose_loop_to_end(id);
                    hr.play_to_end_pose_id = Some(id);
                }
            } else if hr.minimum_remaining > 0.0 && link.is_none() && fighter.is_pose_finished() {
                if let Some(id) = hr.pose_id {
                    fighter.rewind_reaction_pose(id);
                    hr.rewind_count = hr.rewind_count.saturating_add(1);
                    return Some(hr);
                }
            }
            let finished =
                fighter.is_pose_finished() || fighter.pose_id() != hr.pose_id.unwrap_or(u16::MAX);
            if finished && hr.minimum_remaining <= 0.0 && !fighter.is_airborne() {
                if fighter.guard_held() && fighter.current_state() == FighterState::Guard {
                    fighter.set_in_guard(true);
                    if let Some(head) = hr.head_pose_id {
                        if fighter.pose_id() != head && fighter.has_pose(head) {
                            fighter.play_pose(head, true);
                        }
                    }
                } else if fighter.current_state() == FighterState::Guard {
                    fighter.set_state(FighterState::Name, false);
                }
                return None;
            }
            Some(hr)
        }
        HitReactKind::BeHit => {
            hr.lock_remaining -= t;
            hr.minimum_remaining = (hr.minimum_remaining - t).max(0.0);
            let link = hr.pose_id.and_then(|id| fighter.pose_link(id));
            if hr.minimum_remaining <= 0.0 && link.is_none() && hr.play_to_end_pose_id.is_none() {
                if let Some(id) = hr.pose_id {
                    fighter.release_pose_loop_to_end(id);
                    hr.play_to_end_pose_id = Some(id);
                }
            } else if hr.minimum_remaining > 0.0 && link.is_none() && fighter.is_pose_finished() {
                if let Some(id) = hr.pose_id {
                    fighter.rewind_reaction_pose(id);
                    hr.rewind_count = hr.rewind_count.saturating_add(1);
                    return Some(hr);
                }
            }
            let finished = fighter.is_pose_finished();

            if finished {
                if let Some(to) = link {
                    if fighter.pose_id() != to {
                        fighter.play_pose(to, true);
                        hr.pose_id = Some(to);
                        hr.minimum_remaining = carried_minimum(attack_target_value(&hr));
                        hr.play_to_end_pose_id = None;

                        return Some(hr);
                    }
                }
            }

            if let Some(id) = hr.pose_id {
                if DOWN_HOLD_POSES.contains(&id)
                    && finished
                    && hr.minimum_remaining <= 0.0
                    && !fighter.is_airborne()
                {
                    return Some(enter_down_state(
                        fighter,
                        id,
                        carried_minimum(attack_target_value(&hr)),
                    ));
                }
            }

            if finished
                && link.is_none()
                && hr.minimum_remaining <= 0.0
                && !hr.pose_id.is_some_and(|id| DOWN_HOLD_POSES.contains(&id))
            {
                return None;
            }
            Some(hr)
        }
        HitReactKind::Roll => {
            hr.lock_remaining -= t;
            if fighter.is_pose_finished() || hr.lock_remaining <= 0.0 {
                return Some(start_getup(fighter));
            }
            Some(hr)
        }
        HitReactKind::Down => {
            hr.down_remaining -= t;

            hr.down_remaining = hr.down_remaining.max(0.0);
            if let Some(id) = hr.pose_id {
                if fighter.pose_id() != id && fighter.has_pose(id) {
                    fighter.play_pose(id, true);
                }
            }
            Some(hr)
        }
        HitReactKind::GetUp => {
            hr.lock_remaining -= t;
            if fighter.is_pose_finished() || hr.lock_remaining <= 0.0 {
                return None;
            }
            Some(hr)
        }
        HitReactKind::Dead => Some(hr),
    }
}

fn enter_down_state(
    fighter: &mut impl ReactionFighter,
    pose_id: u16,
    minimum_seconds: f32,
) -> HitReact {
    let down_remaining = DOWN_MIN_SECONDS.max(minimum_seconds);
    let mut hr = HitReact::new(HitReactKind::Down, Some(pose_id));
    hr.lock_remaining = 0.0;
    hr.down_remaining = down_remaining;
    hr.authored_minimum = minimum_seconds.max(0.0);
    fighter.play_pose(pose_id, true);
    hr
}

fn start_getup(fighter: &mut impl ReactionFighter) -> HitReact {
    let preferred = if fighter.pose_id() == 113 {
        120
    } else if fighter.pose_id() == 117 {
        35
    } else {
        119
    };
    let getup = if fighter.has_pose(preferred) {
        Some(preferred)
    } else {
        GETUP_FALLBACK_POSES
            .iter()
            .copied()
            .find(|id| fighter.has_pose(*id))
    };
    match getup {
        Some(id) if id != 0 => {
            let (start, end) = pose_range(fighter, id);
            let hr = getup_reaction_for_pose(id, start, end);
            fighter.play_pose(id, true);
            hr
        }
        _ => {
            let mut hr = HitReact::new(HitReactKind::GetUp, None);
            hr.lock_remaining = 0.0;
            hr
        }
    }
}

pub fn getup_reaction_for_pose(pose_id: u16, start: u32, end: u32) -> HitReact {
    let mut hr = HitReact::new(HitReactKind::GetUp, Some(pose_id));
    hr.lock_remaining = pose_duration_seconds(start, end).max(0.35);
    hr
}

fn attack_target_value(hr: &HitReact) -> f32 {
    hr.authored_minimum
}

fn carried_minimum(v: f32) -> f32 {
    v.max(0.0)
}

fn pose_range(fighter: &impl ReactionFighter, pose_id: u16) -> (u32, u32) {
    fighter.pose_range(pose_id).unwrap_or((0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hit_resolve::{resolve_hit, CombatStats, HitOptions};

    struct StubFighter {
        pose: u16,
        finished: bool,
        poses: Vec<u16>,
        in_guard: bool,
        state: FighterState,
        alive: bool,
        guard_held: bool,
        airborne: bool,
        links: std::collections::HashMap<u16, u16>,
        released_to_end: bool,
        rewound: bool,
        last_entry_hard_cut: bool,
    }

    impl StubFighter {
        fn new() -> Self {
            Self {
                pose: 0,
                finished: false,
                poses: vec![22, 35, 90, 93, 112, 113, 117, 119, 120, 176, 177, 178, 179],
                in_guard: false,
                state: FighterState::Name,
                alive: true,
                guard_held: false,
                airborne: false,
                links: std::collections::HashMap::new(),
                released_to_end: false,
                rewound: false,
                last_entry_hard_cut: false,
            }
        }
    }

    impl ReactionFighter for StubFighter {
        fn pose_id(&self) -> u16 {
            self.pose
        }
        fn is_pose_finished(&self) -> bool {
            self.finished
        }
        fn has_pose(&self, pose_id: u16) -> bool {
            self.poses.contains(&pose_id)
        }
        fn play_pose(&mut self, pose_id: u16, _hard: bool) {
            self.pose = pose_id;
            self.last_entry_hard_cut = false;
        }
        fn play_pose_hard_cut(&mut self, pose_id: u16) {
            self.pose = pose_id;
            self.last_entry_hard_cut = true;
        }
        fn set_in_guard(&mut self, v: bool) {
            self.in_guard = v;
        }
        fn current_state(&self) -> FighterState {
            self.state
        }
        fn set_state(&mut self, state: FighterState, locked: bool) {
            self.state = state;
            let _ = locked;
        }
        fn alive(&self) -> bool {
            self.alive
        }
        fn guard_held(&self) -> bool {
            self.guard_held
        }
        fn pose_link(&self, pose_id: u16) -> Option<u16> {
            self.links.get(&pose_id).copied()
        }
        fn is_airborne(&self) -> bool {
            self.airborne
        }
        fn pose_range(&self, pose_id: u16) -> Option<(u32, u32)> {
            self.has_pose(pose_id).then_some((0, 14))
        }
        fn release_pose_loop_to_end(&mut self, pose_id: u16) {
            if self.pose == pose_id {
                self.released_to_end = true;
            }
        }
        fn rewind_reaction_pose(&mut self, pose_id: u16) {
            if self.pose == pose_id {
                self.finished = false;
                self.rewound = true;
            }
        }
    }

    fn attack_block() -> AttackBlock {
        AttackBlock {
            start: 0,
            end: 30,
            attack_type: 0,
            check_friend: 0,
            defense_value: 0.3,
            defense_move: 2.0,
            target_value: 0.5,
            target_move: 3.0,
            target_pose: Some(93),
            target_pose_front: Some(95),
            target_pose_back: Some(96),
            target_pose_left: Some(97),
            target_pose_right: Some(98),
            bones: Vec::new(),
        }
    }

    #[test]
    fn finished_fast_reaction_does_not_wait_for_unscaled_clip_duration() {
        let mut fighter = StubFighter::new();
        fighter.pose = 93;
        let mut hr = HitReact::new(HitReactKind::BeHit, Some(93));
        hr.lock_remaining = 1.2;
        hr.minimum_remaining = 0.0;
        fighter.finished = false;
        assert!(
            tick_hit_react(&mut fighter, hr.clone(), 0.0).is_some(),
            "Value expiry must not skip the tail"
        );
        fighter.finished = true;
        assert!(
            tick_hit_react(&mut fighter, hr.clone(), 0.0).is_none(),
            "animation completion owns recovery"
        );
        hr.minimum_remaining = 0.5;
        assert!(
            tick_hit_react(&mut fighter, hr, 0.0).is_some(),
            "authored minimum still holds"
        );
    }

    #[test]
    fn facing_bucket_front_back() {
        assert_eq!(facing_bucket(0.0, 5.0, 0.0, 0.0, 0.0), FacingBucket::Front);

        assert_eq!(facing_bucket(0.0, -5.0, 0.0, 0.0, 0.0), FacingBucket::Back);
    }

    #[test]
    fn facing_bucket_left_right() {
        assert_eq!(facing_bucket(5.0, 0.0, 0.0, 0.0, 0.0), FacingBucket::Left);

        assert_eq!(facing_bucket(-5.0, 0.0, 0.0, 0.0, 0.0), FacingBucket::Right);
    }

    #[test]
    fn facing_boundaries_match_original_signed_angle_branches() {
        for (x, z, bucket) in [
            (1.0, 1.0, FacingBucket::Left),
            (-1.0, 1.0, FacingBucket::Right),
            (1.0, -1.0, FacingBucket::Back),
            (-1.0, -1.0, FacingBucket::Back),
        ] {
            assert_eq!(facing_bucket(x, z, 0.0, 0.0, 0.0), bucket);
        }
        for (angle, bucket) in [
            (0.7853970_f32, FacingBucket::Front),
            (0.7853980, FacingBucket::Left),
            (-0.7853970, FacingBucket::Front),
            (-0.7853980, FacingBucket::Right),
            (2.3561920, FacingBucket::Left),
            (2.3561930, FacingBucket::Back),
            (-2.3561920, FacingBucket::Right),
            (-2.3561930, FacingBucket::Back),
        ] {
            assert_eq!(
                facing_bucket(angle.sin(), angle.cos(), 0.0, 0.0, 0.0),
                bucket
            );
        }
    }

    #[test]
    fn pick_reaction_pose_uses_original_directional_fields() {
        let atk = attack_block();
        let poses = [90, 93, 95, 96, 97, 98];
        for (bucket, expected) in [
            (FacingBucket::Front, 95),
            (FacingBucket::Back, 96),
            (FacingBucket::Left, 97),
            (FacingBucket::Right, 98),
            (FacingBucket::Default, 93),
        ] {
            assert_eq!(pick_reaction_pose_id(&atk, bucket, &poses), Some(expected));
        }
    }

    #[test]
    fn pick_reaction_pose_falls_back_to_generic() {
        let atk = attack_block();

        let poses = [90, 93];
        assert_eq!(
            pick_reaction_pose_id(&atk, FacingBucket::Front, &poses),
            Some(93)
        );
    }

    #[test]
    fn pose_duration_frames() {
        let d = pose_duration_seconds(0, 10);
        assert!((d - 0.3666).abs() < 0.01);

        assert_eq!(pose_duration_seconds(100, 100), 0.0);
    }

    #[test]
    fn pack_direction_bits() {
        assert_eq!(
            pack_target_pose_with_direction(93, FacingBucket::Left),
            93 | 0x2000
        );
        assert_eq!(
            pack_target_pose_with_direction(93, FacingBucket::Back),
            93 | 0x3000
        );
    }

    #[test]
    fn guard_reaction_uses_strategy_fast_target_table() {
        let poses = FAST_TARGET_POSES_BY_STRATEGY[0];
        assert_eq!(guard_reaction_pose_id(0, 0, &poses, 2), Some(62));
        assert_eq!(guard_reaction_pose_id(0, 1, &poses, 0), Some(80));
        assert_eq!(guard_reaction_pose_id(0, 2, &poses, 0), Some(81));
        assert_eq!(guard_reaction_pose_id(0, 3, &poses, 0), Some(82));
    }

    #[test]
    fn begin_be_hit_sets_lock() {
        let mut f = StubFighter::new();

        let poses = vec![90, 93, 95, 96, 97, 98];
        let hr = begin_be_hit(&mut f, Some(&attack_block()), FacingBucket::Front, &poses);
        let hr = hr.unwrap();
        assert_eq!(hr.kind, HitReactKind::BeHit);
        assert_eq!(hr.pose_id, Some(95));

        assert!(hr.lock_remaining >= 0.5);
        assert_eq!(hr.motion_scale, 3.0);
        assert!(!f.last_entry_hard_cut);
    }

    #[test]
    fn begin_death_is_terminal() {
        let mut f = StubFighter::new();
        let hr = begin_death(&mut f, None);
        assert_eq!(hr.kind, HitReactKind::Dead);
        assert_eq!(hr.pose_id, Some(117));
        assert!(hr.lock_remaining.is_infinite());
        assert!(!f.last_entry_hard_cut, "death keeps the normal entry blend");
    }

    #[test]
    fn environmental_hit_uses_the_same_blended_entry_as_weapon_behit() {
        let mut f = StubFighter::new();
        let poses = f.poses.clone();

        let hr = begin_environmental_hit(&mut f, &poses).unwrap();

        assert_eq!(hr.pose_id, Some(ENVIRONMENT_HIT_POSE));
        assert_eq!(f.pose, ENVIRONMENT_HIT_POSE);
        assert!(!f.last_entry_hard_cut);
    }

    #[test]
    fn airborne_environmental_hit_releases_when_animation_finishes() {
        let mut f = StubFighter::new();
        f.airborne = true;
        let poses = f.poses.clone();
        let hr = begin_environmental_hit(&mut f, &poses).unwrap();
        let hr = tick_hit_react(&mut f, hr, 0.016).expect("animation still playing");
        f.finished = true;
        assert!(tick_hit_react(&mut f, hr, 0.016).is_none());
    }

    #[test]
    fn tick_behit_completes_to_name() {
        let mut f = StubFighter::new();
        let poses = f.poses.clone();
        let mut hr =
            begin_be_hit(&mut f, Some(&attack_block()), FacingBucket::Front, &poses).unwrap();
        hr.lock_remaining = 0.1;
        hr.minimum_remaining = 0.0;
        hr.pose_id = Some(93);
        f.pose = 93;
        f.finished = true;

        let next = tick_hit_react(&mut f, hr, 0.2);
        assert!(next.is_none());
    }

    #[test]
    fn tick_behit_down_link_retains_lock_until_target_minimum_and_playback_end() {
        let mut f = StubFighter::new();
        f.links.insert(95, 112);
        f.finished = true;
        f.airborne = false;
        let mut hr = HitReact::new(HitReactKind::BeHit, Some(95));
        hr.authored_minimum = 0.8;
        hr.minimum_remaining = 0.1;
        let next = tick_hit_react(&mut f, hr, 0.016);
        let linked = next.unwrap();
        assert_eq!(linked.kind, HitReactKind::BeHit);
        assert_eq!(linked.pose_id, Some(112));
        assert_eq!(linked.minimum_remaining, 0.8);
        let waiting = tick_hit_react(&mut f, linked, 0.4).unwrap();
        assert_eq!(waiting.kind, HitReactKind::BeHit);
        let waiting = tick_hit_react(&mut f, waiting, 0.4).unwrap();
        assert_eq!(waiting.kind, HitReactKind::BeHit);
        f.finished = true;
        let down = tick_hit_react(&mut f, waiting, 0.0).unwrap();
        assert_eq!(down.kind, HitReactKind::Down);
        assert_eq!(down.down_remaining, 0.8);
        assert_eq!(down.authored_minimum, 0.8);
    }

    #[test]
    fn tick_behit_reinstalls_authored_minimum_on_non_down_link() {
        let mut f = StubFighter::new();
        f.pose = 93;
        f.links.insert(93, 90);
        f.finished = true;
        let mut hr = HitReact::new(HitReactKind::BeHit, Some(93));
        hr.authored_minimum = 0.8;
        hr.minimum_remaining = 0.1;

        let linked = tick_hit_react(&mut f, hr, 0.016).unwrap();

        assert_eq!(linked.kind, HitReactKind::BeHit);
        assert_eq!(linked.pose_id, Some(90));
        assert_eq!(linked.minimum_remaining, 0.8);
        assert_eq!(linked.authored_minimum, 0.8);
    }

    #[test]
    fn tick_down_waits_for_explicit_getup_request() {
        let mut f = StubFighter::new();
        f.pose = 112;
        let mut hr = HitReact::new(HitReactKind::Down, Some(112));
        hr.down_remaining = 0.1;
        let next = tick_hit_react(&mut f, hr, 0.2);
        let down = next.unwrap();
        assert_eq!(down.kind, HitReactKind::Down);
        assert_eq!(down.pose_id, Some(112));
        assert_eq!(down.down_remaining, 0.0);
    }

    #[test]
    fn minimum_expiry_releases_loop_and_waits_for_tail_end() {
        let mut f = StubFighter::new();
        f.pose = 93;
        let mut hr = HitReact::new(HitReactKind::BeHit, Some(93));
        hr.minimum_remaining = 0.05;
        hr.lock_remaining = 0.05;

        let hr = tick_hit_react(&mut f, hr, 0.1).unwrap();
        assert!(f.released_to_end);
        assert_eq!(hr.play_to_end_pose_id, Some(93));

        f.finished = true;
        assert!(tick_hit_react(&mut f, hr, 0.0).is_none());
    }

    #[test]
    fn unmet_minimum_rewinds_linkless_finished_pose() {
        let mut f = StubFighter::new();
        f.pose = 93;
        f.finished = true;
        let mut hr = HitReact::new(HitReactKind::BeHit, Some(93));
        hr.minimum_remaining = 0.5;
        hr.lock_remaining = 0.5;

        let hr = tick_hit_react(&mut f, hr, 0.1).unwrap();
        assert!(f.rewound);
        assert_eq!(hr.rewind_count, 1);
    }

    #[test]
    fn tick_dead_fighter_never_recovers() {
        let mut f = StubFighter::new();
        f.alive = false;
        let hr = HitReact::new(HitReactKind::BeHit, Some(93));
        let next = tick_hit_react(&mut f, hr, 0.1);
        let dead = next.unwrap();
        assert_eq!(dead.kind, HitReactKind::Dead);
    }

    #[test]
    fn tick_guard_held_returns_to_head() {
        let mut f = StubFighter::new();
        f.state = FighterState::Guard;
        f.guard_held = true;
        f.pose = 90;
        f.finished = true;
        let mut hr = HitReact::new(HitReactKind::Guard, Some(90));
        hr.head_pose_id = Some(35);
        hr.minimum_remaining = 0.0;
        hr.lock_remaining = 0.0;
        let next = tick_hit_react(&mut f, hr, 0.1);
        assert!(next.is_none());
        assert_eq!(f.pose, 35);
    }

    #[test]
    fn hit_resolve_feeds_reaction() {
        struct Stats {
            hp: f32,
            alive: bool,
            anger: f32,
            def: f32,
        }
        impl CombatStats for Stats {
            fn hp(&self) -> f32 {
                self.hp
            }
            fn alive(&self) -> bool {
                self.alive
            }
            fn invincible(&self) -> bool {
                false
            }
            fn anger(&self) -> f32 {
                self.anger
            }
            fn anger_max(&self) -> f32 {
                100.0
            }
            fn def(&self) -> f32 {
                self.def
            }
            fn attack_bonus(&self) -> f32 {
                0.0
            }
            fn take_damage(&mut self, amount: i32) -> i32 {
                let dmg = amount.max(0);
                self.hp = (self.hp - dmg as f32).max(0.0);
                if self.hp <= 0.0 {
                    self.alive = false;
                }
                dmg
            }
            fn gain_anger_from_damage(&mut self, damage: i32) -> i32 {
                self.anger = (self.anger + damage as f32).min(100.0);
                damage
            }
            fn gain_anger_on_guard(&mut self, _raw_atk: i32) -> i32 {
                0
            }
            fn reset_spawn_clock(&mut self) {}
        }

        fn sword_percent(_pose_id: u16) -> Option<f32> {
            Some(35.0)
        }

        let mut stats = Stats {
            hp: 1000.0,
            alive: true,
            anger: 0.0,
            def: 0.0,
        };
        let opts = HitOptions {
            attacker_team: 1,
            defender_team: 2,
            defender_in_guard: false,
            defender_weapon_def: None,
            weapon_id: 6,
            pose_id: 262,
            attack_power: None,
            attack_bonus: 0,
            hit_key: None,
            force_hit_flags: None,
            attack_type: 0,
            dword1: None,
            attack: Some(&attack_block()),
            pose_attack_percent: &sword_percent,
        };
        let result = resolve_hit(&mut stats, &opts);
        assert_eq!(result.outcome, crate::hit_resolve::HitOutcome::BeHit);

        let mut f = StubFighter::new();
        let poses = f.poses.clone();
        let hr = apply_hit_reaction(
            &result.outcome,
            &mut f,
            Some(&attack_block()),
            FacingBucket::Front,
            &poses,
            0,
            None,
        );
        assert!(hr.is_some());
        assert_eq!(hr.unwrap().kind, HitReactKind::BeHit);
    }
}
