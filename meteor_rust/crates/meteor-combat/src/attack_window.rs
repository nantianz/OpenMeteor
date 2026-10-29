use crate::hit_reaction::{
    begin_be_hit, begin_death, begin_guard_hit, FacingBucket, ReactionFighter,
};
use crate::hit_resolve::{
    force_hit_flags_from_attack_type, hit_key_from_pose, pack_hit_dword1, resolve_hit, CombatStats,
    HitOptions, HitOutcome,
};
use meteor_asset::pos::AttackBlock;

pub const MELEE_HIT_RANGE: f32 = 3.4;

pub const HIT_CONTACT_LERP: f32 = 1.0 / 6.0;

pub const HIT_CONTACT_RISE: f32 = 16.0 * 0.12;

pub const HIT_CONTACT_EFFECT: &str = "FrontHIT";

pub const TARGET_HIT_INTERVAL: f32 = 0.2;

pub const SPAWN_PROTECT_SECONDS: f32 = 2.0;

static FRIENDLY_FIRE_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_friendly_fire(enabled: bool) {
    FRIENDLY_FIRE_ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub fn friendly_fire_allowed() -> bool {
    FRIENDLY_FIRE_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn attack_can_reach_team(attacker_team: i32, defender_team: i32) -> bool {
    attack_can_reach_team_with_friendly_fire(attacker_team, defender_team, friendly_fire_allowed())
}

pub fn attack_can_reach_team_with_friendly_fire(
    attacker_team: i32,
    defender_team: i32,
    friendly_fire_enabled: bool,
) -> bool {
    if friendly_fire_enabled {
        return true;
    }
    (crate::hit_resolve::team_mask(attacker_team) & crate::hit_resolve::team_mask(defender_team))
        == 0
}

pub fn hit_contact_point(ax: f32, ay: f32, az: f32, dx: f32, dy: f32, dz: f32) -> [f32; 3] {
    [
        dx + (ax - dx) * HIT_CONTACT_LERP,
        dy + (ay - dy) * HIT_CONTACT_LERP + HIT_CONTACT_RISE,
        dz + (az - dz) * HIT_CONTACT_LERP,
    ]
}

#[derive(Debug, Clone)]
pub struct TargetHitClock {
    pub elapsed: f32,

    pub spawn_clock: f32,

    pub alive: bool,
}

impl Default for TargetHitClock {
    fn default() -> Self {
        Self::new()
    }
}

impl TargetHitClock {
    pub fn new() -> Self {
        Self {
            elapsed: f32::INFINITY,
            spawn_clock: 0.0,
            alive: true,
        }
    }

    pub fn accepts_target_hit(&self) -> bool {
        self.elapsed > TARGET_HIT_INTERVAL
    }

    pub fn spawn_protected(&self) -> bool {
        self.spawn_clock < SPAWN_PROTECT_SECONDS
    }

    pub fn reset_target_hit_clock(&mut self) {
        self.elapsed = 0.0;
    }

    pub fn advance(&mut self, dt: f32) {
        self.elapsed += dt.max(0.0);
        self.spawn_clock += dt.max(0.0);
    }
}

pub struct AttackHitContext<'a> {
    pub attacker_team: i32,
    pub defender_team: i32,

    pub weapon_id: u8,

    pub pose_id: u16,

    pub attack_power: Option<i32>,

    pub attack_bonus: i32,

    pub defender_weapon_def: Option<i32>,

    pub attack: &'a AttackBlock,

    pub pose_attack_percent: &'a dyn Fn(u16) -> Option<f32>,

    pub bucket: FacingBucket,
}

#[derive(Debug, Clone)]
pub struct AppliedHit {
    pub outcome: HitOutcome,
    pub damage: i32,
    pub raw: i32,
    pub killed: bool,
    pub guarded: bool,
}

pub fn apply_attack_hit<S: CombatStats, F: ReactionFighter>(
    defender_stats: &mut S,
    defender_clock: &mut TargetHitClock,
    defender_fighter: &mut F,
    poses: &[u16],
    ctx: &AttackHitContext,
) -> Option<AppliedHit> {
    if !attack_can_reach_team(ctx.attacker_team, ctx.defender_team) {
        return None;
    }

    if !defender_stats.alive() {
        return None;
    }

    if !defender_clock.accepts_target_hit() {
        return None;
    }
    defender_clock.reset_target_hit_clock();

    if defender_clock.spawn_protected() {
        return None;
    }

    let hit_key = hit_key_from_pose(ctx.pose_id);
    let force_hit_flags = force_hit_flags_from_attack_type(ctx.attack.attack_type as i32);
    let dword1 = pack_hit_dword1(hit_key, force_hit_flags, 0);

    let opts = HitOptions {
        attacker_team: ctx.attacker_team,
        defender_team: ctx.defender_team,
        defender_in_guard: false,
        defender_weapon_def: ctx.defender_weapon_def,
        weapon_id: ctx.weapon_id,
        pose_id: ctx.pose_id,
        attack_power: ctx.attack_power,
        attack_bonus: ctx.attack_bonus,
        hit_key: Some(hit_key),
        force_hit_flags: Some(force_hit_flags),
        attack_type: ctx.attack.attack_type as i32,
        dword1: Some(dword1),
        attack: Some(ctx.attack),
        pose_attack_percent: ctx.pose_attack_percent,
    };
    let result = resolve_hit(defender_stats, &opts);

    match result.outcome {
        HitOutcome::Guard => {
            let _ = begin_guard_hit(
                defender_fighter,
                Some(ctx.attack),
                ctx.bucket,
                poses,
                0,
                None,
                0,
            );
            Some(AppliedHit {
                outcome: HitOutcome::Guard,
                damage: result.damage,
                raw: result.raw,
                killed: false,
                guarded: true,
            })
        }
        HitOutcome::BeHit => {
            if result.killed {
                let _ = begin_death(defender_fighter, Some(ctx.attack));
            } else {
                let _ = begin_be_hit(defender_fighter, Some(ctx.attack), ctx.bucket, poses);
            }
            Some(AppliedHit {
                outcome: HitOutcome::BeHit,
                damage: result.damage,
                raw: result.raw,
                killed: result.killed,
                guarded: false,
            })
        }
        HitOutcome::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hit_reaction::ReactionFighter;
    use crate::hit_resolve::CombatStats;
    use meteor_core::FighterState;

    struct StubStats {
        hp: f32,
        alive: bool,
        anger: f32,
        def: f32,
    }
    impl CombatStats for StubStats {
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

    struct StubFighter {
        pose: u16,
        finished: bool,
        poses: Vec<u16>,
        in_guard: bool,
        state: FighterState,
        alive: bool,
        guard_held: bool,
        airborne: bool,
    }
    impl StubFighter {
        fn new() -> Self {
            Self {
                pose: 0,
                finished: false,
                poses: vec![90, 93, 95, 96, 97, 98, 112, 113, 117, 119, 120],
                in_guard: false,
                state: FighterState::Name,
                alive: true,
                guard_held: false,
                airborne: false,
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
        }
        fn set_in_guard(&mut self, v: bool) {
            self.in_guard = v;
        }
        fn current_state(&self) -> FighterState {
            self.state
        }
        fn set_state(&mut self, state: FighterState, _locked: bool) {
            self.state = state;
        }
        fn alive(&self) -> bool {
            self.alive
        }
        fn guard_held(&self) -> bool {
            self.guard_held
        }
        fn pose_link(&self, _pose_id: u16) -> Option<u16> {
            None
        }
        fn is_airborne(&self) -> bool {
            self.airborne
        }
        fn pose_range(&self, pose_id: u16) -> Option<(u32, u32)> {
            self.has_pose(pose_id).then_some((0, 14))
        }
        fn release_pose_loop_to_end(&mut self, _pose_id: u16) {}
        fn rewind_reaction_pose(&mut self, pose_id: u16) {
            if self.pose == pose_id {
                self.finished = false;
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

    fn sword_percent(_pose_id: u16) -> Option<f32> {
        Some(35.0)
    }

    fn ctx<'a>(attack: &'a AttackBlock) -> AttackHitContext<'a> {
        AttackHitContext {
            attacker_team: 1,
            defender_team: 2,
            weapon_id: 6,
            pose_id: 262,
            attack_power: None,
            attack_bonus: 0,
            defender_weapon_def: None,
            attack,
            pose_attack_percent: &sword_percent,
            bucket: FacingBucket::Front,
        }
    }

    #[test]
    fn friendly_fire_team_gate() {
        assert!(!attack_can_reach_team_with_friendly_fire(1, 1, false));
        assert!(attack_can_reach_team_with_friendly_fire(1, 1, true));
        set_friendly_fire(false);

        assert!(!attack_can_reach_team(1, 1));

        assert!(attack_can_reach_team(1, 2));
        set_friendly_fire(true);

        assert!(attack_can_reach_team(1, 1));
        set_friendly_fire(false);
    }

    #[test]
    fn target_hit_clock_interval() {
        let mut clock = TargetHitClock::new();

        assert!(clock.accepts_target_hit());
        clock.reset_target_hit_clock();

        assert!(!clock.accepts_target_hit());
        clock.advance(0.1);
        assert!(!clock.accepts_target_hit());
        clock.advance(0.15);
        assert!(clock.accepts_target_hit());
    }

    #[test]
    fn spawn_protection_blocks() {
        let mut clock = TargetHitClock::new();
        assert!(clock.spawn_protected());
        clock.advance(2.1);
        assert!(!clock.spawn_protected());
    }

    #[test]
    fn apply_hit_rejects_same_team() {
        set_friendly_fire(false);
        let mut stats = StubStats {
            hp: 1000.0,
            alive: true,
            anger: 0.0,
            def: 0.0,
        };
        let mut clock = TargetHitClock::new();
        let mut fighter = StubFighter::new();
        let atk = attack_block();
        let mut c = ctx(&atk);
        c.attacker_team = 2;
        c.defender_team = 2;
        let poses = fighter.poses.clone();
        let result = apply_attack_hit(&mut stats, &mut clock, &mut fighter, &poses, &c);
        assert!(result.is_none());
        assert_eq!(stats.hp, 1000.0);
    }

    #[test]
    fn apply_hit_rejects_hit_interval() {
        let mut stats = StubStats {
            hp: 1000.0,
            alive: true,
            anger: 0.0,
            def: 0.0,
        };
        let mut clock = TargetHitClock::new();
        clock.reset_target_hit_clock();
        let mut fighter = StubFighter::new();
        let poses = fighter.poses.clone();
        let atk = attack_block();
        let result = apply_attack_hit(&mut stats, &mut clock, &mut fighter, &poses, &ctx(&atk));
        assert!(result.is_none());
    }

    #[test]
    fn apply_hit_rejects_spawn_protected() {
        let mut stats = StubStats {
            hp: 1000.0,
            alive: true,
            anger: 0.0,
            def: 0.0,
        };
        let mut clock = TargetHitClock::new();
        let mut fighter = StubFighter::new();
        let poses = fighter.poses.clone();
        let atk = attack_block();
        let result = apply_attack_hit(&mut stats, &mut clock, &mut fighter, &poses, &ctx(&atk));
        assert!(result.is_none());
    }

    #[test]
    fn apply_hit_behit_and_damage() {
        let mut stats = StubStats {
            hp: 1000.0,
            alive: true,
            anger: 0.0,
            def: 0.0,
        };
        let mut clock = TargetHitClock::new();
        clock.advance(2.1);
        let mut fighter = StubFighter::new();
        let poses = fighter.poses.clone();
        let atk = attack_block();
        let result = apply_attack_hit(&mut stats, &mut clock, &mut fighter, &poses, &ctx(&atk));
        let hit = result.unwrap();
        assert_eq!(hit.outcome, HitOutcome::BeHit);
        assert_eq!(hit.damage, 39);
        assert!(!hit.killed);
        assert_eq!(stats.hp, 961.0);

        assert!(!clock.accepts_target_hit());
    }

    #[test]
    fn apply_hit_lethal_kills() {
        let mut stats = StubStats {
            hp: 10.0,
            alive: true,
            anger: 0.0,
            def: 0.0,
        };
        let mut clock = TargetHitClock::new();
        clock.advance(2.1);
        let mut fighter = StubFighter::new();
        let poses = fighter.poses.clone();
        let atk = attack_block();
        let result = apply_attack_hit(&mut stats, &mut clock, &mut fighter, &poses, &ctx(&atk));
        let hit = result.unwrap();
        assert!(hit.killed);
        assert!(!stats.alive);
    }

    #[test]
    fn hit_contact_point_lerp() {
        let p = hit_contact_point(10.0, 0.0, 0.0, 0.0, 0.0, 0.0);

        assert!((p[0] - 10.0 / 6.0).abs() < 0.001);

        assert!((p[1] - HIT_CONTACT_RISE).abs() < 0.001);
    }
}
