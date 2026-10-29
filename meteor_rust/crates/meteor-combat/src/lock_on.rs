pub const LOCK_ACQUIRE_RANGE: f32 = 200.0 * 0.12;

pub const MIN_PICK_DISTANCE: f32 = 0.001 * 0.12;

pub const TRACK_REFRESH_SECONDS: f32 = 1.0 / 3.0;

pub const LOCK_ATTACK_FACE_RANGE: f32 = 240.0 * 0.12;

pub const LOCK_ATTACK_FACE_HEIGHT: f32 = 100.0 * 0.12;

pub const TRACK_TURN_LIMIT: f32 = 0.7;

pub const TRACK_CURVE_EXIT_TURN_LIMIT: f32 = 0.3;

pub const TRACK_TURN_RESPONSE: f32 = 20.0;

pub const AUTO_CAMERA_LOCK_DISABLED_STRATEGIES: [u8; 3] = [2, 3, 6];

pub fn weapon_equip_clears_lock(strategy_id: u8) -> bool {
    AUTO_CAMERA_LOCK_DISABLED_STRATEGIES.contains(&strategy_id)
}

pub fn shortest_angle(mut angle: f32) -> f32 {
    while angle > std::f32::consts::PI {
        angle -= std::f32::consts::PI * 2.0;
    }
    while angle < -std::f32::consts::PI {
        angle += std::f32::consts::PI * 2.0;
    }
    angle
}

pub fn tracked_head_yaw(
    player_x: f32,
    player_z: f32,
    player_yaw: f32,
    target: Option<(f32, f32)>,
    current: f32,
    dt: f32,
    turn_limit: f32,
) -> Option<f32> {
    if !dt.is_finite() || dt <= 0.0 || !turn_limit.is_finite() || turn_limit < 0.0 {
        return None;
    }
    let mut desired = 0.0;
    if let Some((tx, tz)) = target {
        let dx = tx - player_x;
        let dz = tz - player_z;
        if (dx * dx + dz * dz).sqrt() < 1e-6 {
            return Some(current);
        }
        desired = shortest_angle(dx.atan2(dz) - player_yaw).clamp(-turn_limit, turn_limit);
    }
    Some(current + (desired - current) * TRACK_TURN_RESPONSE * dt)
}

pub fn locked_attack_facing_yaw(player: (f32, f32, f32), target: (f32, f32, f32)) -> Option<f32> {
    let dx = target.0 - player.0;
    let dy = target.1 - player.1;
    let dz = target.2 - player.2;
    let dist3 = (dx * dx + dy * dy + dz * dz).sqrt();
    let planar = (dx * dx + dz * dz).sqrt();
    if dist3 >= LOCK_ATTACK_FACE_RANGE {
        return None;
    }
    if dy.abs() >= LOCK_ATTACK_FACE_HEIGHT {
        return None;
    }
    if planar < 1e-6 {
        return None;
    }
    Some(dx.atan2(dz))
}

pub fn locked_attack_gate_failed(player: (f32, f32, f32), target: (f32, f32, f32)) -> bool {
    let dx = target.0 - player.0;
    let dy = target.1 - player.1;
    let dz = target.2 - player.2;
    let dist3 = (dx * dx + dy * dy + dz * dz).sqrt();
    dist3 >= LOCK_ATTACK_FACE_RANGE || dy.abs() >= LOCK_ATTACK_FACE_HEIGHT
}

#[derive(Debug, Clone, Copy)]
pub struct LockCandidate {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub team: i32,
}

#[derive(Debug, Clone)]
pub struct LockOnSystem {
    pub lock_target: Option<u64>,

    pub track_target: Option<u64>,

    track_timer: f32,
}

impl Default for LockOnSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl LockOnSystem {
    pub fn new() -> Self {
        Self {
            lock_target: None,
            track_target: None,
            track_timer: 0.0,
        }
    }

    pub fn is_locked(&self) -> bool {
        self.lock_target.is_some()
    }

    pub fn clear_lock(&mut self) -> bool {
        let had_lock = self.lock_target.is_some();
        self.lock_target = None;
        had_lock
    }

    pub fn acquire_on_attack(
        &mut self,
        enabled: bool,
        strategy_id: u8,
        player: (f32, f32, f32),
        player_yaw: f32,
        candidates: &[LockCandidate],
    ) -> bool {
        if !enabled
            || self.lock_target.is_some()
            || AUTO_CAMERA_LOCK_DISABLED_STRATEGIES.contains(&strategy_id)
        {
            return false;
        }
        let Some(pick) = pick_lock_target(player, player_yaw, candidates) else {
            return false;
        };
        self.lock_target = Some(pick.id);
        true
    }

    pub fn acquire_hit_target(
        &mut self,
        enabled: bool,
        strategy_id: u8,
        target_id: u64,
        candidates: &[LockCandidate],
    ) -> bool {
        if !enabled
            || self.lock_target.is_some()
            || AUTO_CAMERA_LOCK_DISABLED_STRATEGIES.contains(&strategy_id)
            || !candidates.iter().any(|candidate| candidate.id == target_id)
        {
            return false;
        }
        self.lock_target = Some(target_id);
        true
    }

    pub fn toggle(
        &mut self,
        strategy_id: u8,
        player: (f32, f32, f32),
        player_yaw: f32,
        candidates: &[LockCandidate],
    ) -> bool {
        if AUTO_CAMERA_LOCK_DISABLED_STRATEGIES.contains(&strategy_id) {
            return true;
        }
        if self.lock_target.is_some() {
            self.clear_lock();
            return true;
        }
        if let Some(pick) = pick_lock_target(player, player_yaw, candidates) {
            self.lock_target = Some(pick.id);
        }
        true
    }

    pub fn clear(&mut self) {
        self.lock_target = None;
        self.track_target = None;
        self.track_timer = 0.0;
    }

    pub fn update(
        &mut self,
        dt: f32,
        player: (f32, f32, f32),
        player_yaw: f32,
        candidates: &[LockCandidate],
        is_attacking: bool,
        entry_blend: bool,
    ) {
        if let Some(lock) = self.lock_target {
            if !candidates.iter().any(|c| c.id == lock) {
                self.lock_target = None;
            }
        }

        self.track_timer -= dt;
        if self.lock_target.is_none() {
            let track_invalid = self
                .track_target
                .map(|t| !candidates.iter().any(|c| c.id == t))
                .unwrap_or(true);
            if self.track_timer <= 0.0 || track_invalid {
                self.track_timer = TRACK_REFRESH_SECONDS;
                self.track_target = pick_lock_target(player, player_yaw, candidates).map(|c| c.id);
            }
        }

        if self.lock_target.is_some() && entry_blend && is_attacking {
            if let Some(target_id) = self.lock_target {
                if let Some(t) = candidates.iter().find(|c| c.id == target_id) {
                    let target3 = (t.x, t.y, t.z);
                    if locked_attack_gate_failed(player, target3) {
                        self.lock_target = None;
                    }
                }
            }
        }
    }
}

pub fn pick_lock_target(
    player: (f32, f32, f32),
    player_yaw: f32,
    candidates: &[LockCandidate],
) -> Option<LockCandidate> {
    let fx = player_yaw.sin();
    let fz = player_yaw.cos();
    let mut best: Option<LockCandidate> = None;
    let mut best_dot = 0.0f32;
    for f in candidates {
        let dx = f.x - player.0;
        let dz = f.z - player.2;
        let dist = (dx * dx + dz * dz).sqrt();
        if dist > LOCK_ACQUIRE_RANGE {
            continue;
        }
        if dist < MIN_PICK_DISTANCE {
            continue;
        }
        let dot = (dx / dist) * fx + (dz / dist) * fz;

        if dot <= 0.0 {
            continue;
        }
        if dot > best_dot {
            best_dot = dot;
            best = Some(*f);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: u64, x: f32, z: f32) -> LockCandidate {
        LockCandidate {
            id,
            x,
            y: 0.0,
            z,
            team: 2,
        }
    }

    #[test]
    fn shortest_angle_wraps() {
        assert!((shortest_angle(3.5) - (3.5 - 2.0 * std::f32::consts::PI)).abs() < 1e-5);
        assert!((shortest_angle(-3.5) - (-3.5 + 2.0 * std::f32::consts::PI)).abs() < 1e-5);
        assert!((shortest_angle(0.5) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn tracked_head_no_target_returns_to_zero() {
        let v = tracked_head_yaw(0.0, 0.0, 0.0, None, 0.5, 0.1, TRACK_TURN_LIMIT).unwrap();

        assert!((v - 0.5 * (1.0 - TRACK_TURN_RESPONSE * 0.1)).abs() < 1e-4);
    }

    #[test]
    fn tracked_head_faces_target() {
        let v =
            tracked_head_yaw(0.0, 0.0, 0.0, Some((0.0, 5.0)), 0.0, 0.1, TRACK_TURN_LIMIT).unwrap();
        assert!(v.abs() < 1e-4);

        let v =
            tracked_head_yaw(0.0, 0.0, 0.0, Some((5.0, 0.0)), 0.0, 0.1, TRACK_TURN_LIMIT).unwrap();
        assert!(v > 0.0);
        assert!((v - 0.7 * TRACK_TURN_RESPONSE * 0.1).abs() < 1e-3);
    }

    #[test]
    fn attack_facing_gate() {
        let yaw = locked_attack_facing_yaw((0.0, 0.0, 0.0), (1.0, 0.0, 0.0));
        assert!(yaw.is_some());

        assert!(locked_attack_facing_yaw((0.0, 0.0, 0.0), (500.0, 0.0, 0.0)).is_none());

        assert!(locked_attack_facing_yaw((0.0, 0.0, 0.0), (0.0, 500.0, 0.0)).is_none());

        assert!(locked_attack_gate_failed(
            (0.0, 0.0, 0.0),
            (500.0, 0.0, 0.0)
        ));
        assert!(!locked_attack_gate_failed((0.0, 0.0, 0.0), (1.0, 0.0, 0.0)));
    }

    #[test]
    fn pick_prefers_front_angle_only() {
        let player = (0.0f32, 0.0f32, 0.0f32);

        let cands = vec![
            candidate(1, 0.0, 3.0),
            candidate(2, 3.0, 3.0),
            candidate(3, -3.0, 0.0),
            candidate(4, 0.0, -3.0),
        ];
        let pick = pick_lock_target(player, 0.0, &cands).unwrap();
        assert_eq!(pick.id, 1);
    }

    #[test]
    fn pick_culls_behind() {
        let player = (0.0f32, 0.0f32, 0.0f32);
        let cands = vec![candidate(9, 0.0, -3.0)];
        assert!(pick_lock_target(player, 0.0, &cands).is_none());
    }

    #[test]
    fn toggle_releases_then_acquires() {
        let mut sys = LockOnSystem::new();
        let cands = vec![candidate(7, 0.0, 3.0)];

        sys.toggle(1, (0.0, 0.0, 0.0), 0.0, &cands);
        assert_eq!(sys.lock_target, Some(7));

        sys.toggle(1, (0.0, 0.0, 0.0), 0.0, &cands);
        assert!(sys.lock_target.is_none());
    }

    #[test]
    fn attack_auto_lock_obeys_setting_and_strategy_gate() {
        let candidates = vec![candidate(7, 0.0, 3.0)];
        let mut sys = LockOnSystem::new();
        assert!(!sys.acquire_on_attack(false, 1, (0.0, 0.0, 0.0), 0.0, &candidates));
        assert!(!sys.acquire_on_attack(true, 2, (0.0, 0.0, 0.0), 0.0, &candidates));
        assert!(sys.acquire_on_attack(true, 1, (0.0, 0.0, 0.0), 0.0, &candidates));
        assert_eq!(sys.lock_target, Some(7));
    }

    #[test]
    fn hit_auto_lock_requires_actual_candidate_and_never_overwrites() {
        let candidates = vec![candidate(9, -2.0, -2.0)];
        let mut sys = LockOnSystem::new();
        assert!(!sys.acquire_hit_target(true, 1, 8, &candidates));
        assert!(sys.acquire_hit_target(true, 1, 9, &candidates));
        assert!(!sys.acquire_hit_target(true, 1, 8, &[candidate(8, 0.0, 2.0)]));
        assert_eq!(sys.lock_target, Some(9));
    }

    #[test]
    fn toggle_disabled_for_ranged() {
        let mut sys = LockOnSystem::new();
        let cands = vec![candidate(7, 0.0, 3.0)];

        sys.toggle(3, (0.0, 0.0, 0.0), 0.0, &cands);
        assert!(sys.lock_target.is_none());
    }

    #[test]
    fn ranged_equipment_hook_clears_only_lock() {
        assert!(weapon_equip_clears_lock(2));
        assert!(weapon_equip_clears_lock(3));
        assert!(weapon_equip_clears_lock(6));
        assert!(!weapon_equip_clears_lock(0));
        let mut sys = LockOnSystem::new();
        sys.lock_target = Some(7);
        sys.track_target = Some(8);
        assert!(sys.clear_lock());
        assert_eq!(sys.lock_target, None);
        assert_eq!(sys.track_target, Some(8));
    }

    #[test]
    fn update_drops_dead_lock() {
        let mut sys = LockOnSystem::new();
        sys.lock_target = Some(7);

        sys.update(
            0.1,
            (0.0, 0.0, 0.0),
            0.0,
            &[candidate(8, 0.0, 3.0)],
            false,
            false,
        );
        assert!(sys.lock_target.is_none());
    }

    #[test]
    fn update_attack_gate_clears_lock() {
        let mut sys = LockOnSystem::new();
        sys.lock_target = Some(7);
        let far = candidate(7, 500.0, 0.0);
        sys.update(0.1, (0.0, 0.0, 0.0), 0.0, &[far], true, true);
        assert!(sys.lock_target.is_none());

        sys.lock_target = Some(8);
        let high = LockCandidate {
            id: 8,
            x: 0.0,
            y: LOCK_ATTACK_FACE_HEIGHT,
            z: 2.0,
            team: 2,
        };
        sys.update(0.1, (0.0, 0.0, 0.0), 0.0, &[high], true, true);
        assert!(sys.lock_target.is_none());
    }
}
