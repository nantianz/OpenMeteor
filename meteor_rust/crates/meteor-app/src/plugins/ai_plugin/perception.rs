use super::{AiActorSnapshot, BehaviorMode, Vec3};
use meteor_core::MODEL_SCALE as S;

#[derive(Clone, Copy)]
pub(super) struct Scan {
    pub peace: i32,
    pub self_id: i64,
    pub team: u8,
    pub origin: Vec3,
    pub forward: Vec3,
    pub center: Vec3,
    pub radius: f32,
}

pub(super) fn select_hostile(
    scan: Scan,
    actors: &[AiActorSnapshot],
    mut roll: impl FnMut() -> u32,
    mut visible: impl FnMut(AiActorSnapshot) -> bool,
) -> Option<AiActorSnapshot> {
    if scan.peace > 1 {
        return None;
    }
    let mut selected = None;
    for actor in actors.iter().copied() {
        let distance_squared = actor.position.distance_squared(scan.center);
        if actor.id == scan.self_id
            || !actor.alive
            || meteor_combat::hit_resolve::team_mask(i32::from(actor.team))
                & meteor_combat::hit_resolve::team_mask(i32::from(scan.team))
                != 0
            || distance_squared > scan.radius * scan.radius
            || (distance_squared > (80.0 * S).powi(2)
                && (actor.position - scan.origin).dot(scan.forward) <= 0.0)
            || !visible(actor)
        {
            continue;
        }
        selected = Some(actor);

        if roll() % 2 == 0 {
            break;
        }
    }
    selected
}

#[derive(Clone, Copy)]
pub(super) struct Context {
    pub peace: i32,
    pub self_id: i64,
    pub team: u8,
    pub position: Vec3,
    pub forward: Vec3,
    pub weapon: u8,
    pub mode: Option<BehaviorMode>,
    pub configured: Option<(i64, Vec3)>,
    pub navigating: bool,
    pub look: f32,
}

impl Context {
    fn scan(self, searching: bool) -> Scan {
        let following = !searching && self.mode == Some(BehaviorMode::Follow);
        let (center, radius) = if following {
            self.configured
                .map_or((self.position, 3000.0), |(_, p)| (p, 200.0))
        } else {
            (self.position, 3000.0)
        };
        Scan {
            peace: self.peace,
            self_id: self.self_id,
            team: self.team,
            origin: self.position,
            forward: self.forward,
            center,
            radius: (radius + meteor_ai::combat::weapon_tactics(self.weapon).attack_range * 0.5)
                * S,
        }
    }

    fn follow_allows_combat(self, entering: bool) -> bool {
        if self.mode != Some(BehaviorMode::Follow) {
            return true;
        }
        self.configured.is_none_or(|(_, p)| {
            let distance_squared = p.distance_squared(self.position);
            if entering {
                distance_squared < (250.0 * S).powi(2)
            } else {
                distance_squared <= (250.0 * S).powi(2)
                    && (p.y - self.position.y).abs() <= 150.0 * S
            }
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct Perception {
    engaged: bool,
    pub parent_returned: bool,
    search_tick: Option<u8>,
    remembered: Option<i64>,
    view_multiplier: f32,
    pub passive: bool,
    pub clear_navigation: bool,
    pub search_yaw: Option<f32>,
}

impl Default for Perception {
    fn default() -> Self {
        Self {
            engaged: false,
            parent_returned: false,
            search_tick: None,
            remembered: None,
            view_multiplier: 1.0,
            passive: false,
            clear_navigation: false,
            search_yaw: None,
        }
    }
}

impl Perception {
    pub fn in_combat(&self) -> bool {
        self.engaged
    }
    pub fn return_from_failed_pursuit(&mut self, enemy: Option<i64>) {
        self.engaged = false;
        self.parent_returned = true;
        self.remembered = enemy;
        self.passive = false;
        self.search_tick = None;
        self.search_yaw = None;
        self.clear_navigation = true;
    }

    pub fn start_search(&mut self) {
        self.search_tick = Some(0);
        self.passive = true;
    }

    pub fn searching(&self) -> bool {
        self.search_tick.is_some()
    }

    pub fn callback_delay(&self) -> f32 {
        if self.passive && self.searching() {
            0.25
        } else {
            0.2
        }
    }

    pub fn decide(
        &mut self,
        context: Context,
        previous: Option<i64>,
        actors: &[AiActorSnapshot],
        mut roll: impl FnMut() -> u32,
        mut visible: impl FnMut(AiActorSnapshot, f32) -> bool,
    ) -> Option<AiActorSnapshot> {
        self.passive = false;
        self.clear_navigation = false;
        let parent_returned = std::mem::take(&mut self.parent_returned);
        if self.remembered.is_some_and(|id| {
            !actors.iter().any(|actor| {
                actor.id == id
                    && actor.alive
                    && (context.mode == Some(BehaviorMode::Kill)
                        || meteor_combat::hit_resolve::team_mask(i32::from(actor.team))
                            & meteor_combat::hit_resolve::team_mask(i32::from(context.team))
                            == 0)
            })
        }) {
            self.remembered = None;
        }
        if matches!(
            context.mode,
            Some(BehaviorMode::AttackTarget | BehaviorMode::Dodge)
        ) {
            self.engaged = false;
            self.search_tick = None;
            return None;
        }
        if let Some(tick) = self.search_tick {
            self.passive = true;

            if ((roll() % 100) as f32) < context.look * 2.0 {
                self.search_yaw = Some((roll() % 360) as f32 * std::f32::consts::PI / 180.0);
            }
            if tick == 0 {
                self.view_multiplier = 2.0;
            }
            if tick > 8 {
                self.clear_navigation = true;
                self.view_multiplier = 1.0;
                self.search_tick = None;
                self.search_yaw = None;
                return None;
            }
            let found = select_hostile(context.scan(true), actors, &mut roll, |actor| {
                visible(actor, self.view_multiplier)
            });
            if let Some(actor) = found {
                self.clear_navigation = true;
                self.remembered = Some(actor.id);
                self.search_tick = None;
                self.search_yaw = None;
            } else {
                self.search_tick = Some(tick + 1);
            }
            return None;
        }
        if !parent_returned
            && (self.engaged || (previous.is_some() && context.mode != Some(BehaviorMode::Idle)))
        {
            self.engaged = false;
            self.clear_navigation = true;
            if let Some(actor) = actors.iter().copied().find(|a| {
                Some(a.id) == previous
                    && a.alive
                    && (context.mode == Some(BehaviorMode::Kill)
                        || meteor_combat::hit_resolve::team_mask(i32::from(a.team))
                            & meteor_combat::hit_resolve::team_mask(i32::from(context.team))
                            == 0)
            }) {
                let return_to_parent = roll() % 30 == 0;
                if return_to_parent || !context.follow_allows_combat(false) {
                    self.return_from_failed_pursuit(Some(actor.id));
                    return Some(actor);
                }
                if !context.navigating
                    && actor.position.distance_squared(context.position) > (200.0 * S).powi(2)
                    && !visible(actor, self.view_multiplier)
                {
                    self.search_tick = Some(0);
                    self.passive = true;
                    self.remembered = Some(actor.id);
                    return None;
                }
                self.engaged = true;
                self.clear_navigation = false;
                return Some(actor);
            }
        }
        let found = if context.mode == Some(BehaviorMode::Kill) {
            context.configured.and_then(|(id, _)| {
                actors
                    .iter()
                    .copied()
                    .find(|a| a.id == id && a.alive && visible(*a, self.view_multiplier))
            })
        } else {
            let scanned = select_hostile(context.scan(false), actors, &mut roll, |actor| {
                visible(actor, self.view_multiplier)
            });
            scanned.or_else(|| {
                if !matches!(
                    context.mode,
                    None | Some(BehaviorMode::Wait | BehaviorMode::Run | BehaviorMode::Patrol)
                ) {
                    return None;
                }
                let remembered = previous.or(self.remembered)?;
                actors.iter().copied().find(|actor| {
                    actor.id == remembered
                        && actor.alive
                        && meteor_combat::hit_resolve::team_mask(i32::from(actor.team))
                            & meteor_combat::hit_resolve::team_mask(i32::from(context.team))
                            == 0
                        && actor.position.distance_squared(context.position) <= (200.0 * S).powi(2)
                })
            })
        };
        if let Some(actor) = found {
            self.remembered = Some(actor.id);
            if context.mode == Some(BehaviorMode::Idle) {
                self.engaged = false;
                return Some(actor);
            }
            if context.mode != Some(BehaviorMode::Idle) && context.follow_allows_combat(true) {
                self.engaged = true;
                self.clear_navigation = previous != Some(actor.id);
                return Some(actor);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(id: i64, x: f32, z: f32) -> AiActorSnapshot {
        AiActorSnapshot {
            id,
            team: 2,
            position: Vec3::new(x, 0.0, z),
            alive: true,
        }
    }
    fn context() -> Context {
        Context {
            peace: 0,
            self_id: 1,
            team: 1,
            position: Vec3::ZERO,
            forward: Vec3::Z,
            weapon: 6,
            mode: Some(BehaviorMode::Wait),
            configured: None,
            navigating: false,
            look: 0.0,
        }
    }

    #[test]
    fn acquisition_front_half_space_and_near_omnidirectional_boundary() {
        let scan = context().scan(false);
        for (target, expected) in [
            (actor(2, 0.0, 10.0), true),
            (actor(2, 0.0, -10.0), false),
            (actor(2, 0.0, -80.0 * S), true),
            (actor(2, 10.0, 0.0), false),
            (actor(2, 0.0, 80.0 * S + 0.001), true),
        ] {
            assert_eq!(
                select_hostile(scan, &[target], || 0, |_| true).is_some(),
                expected
            );
        }
        assert!(
            select_hostile(scan, &[actor(2, 0.0, 3.0)], || 0, |_| false).is_none(),
            "near range skips FOV only, never the wall test"
        );
    }

    #[test]
    fn peace_two_skips_scan_rng_but_preserves_parent_remembered_fallback() {
        let mut ctx = context();
        ctx.peace = 2;
        let target = actor(2, 0.0, 3.0);
        assert!(select_hostile(
            ctx.scan(false),
            &[target],
            || panic!("Peace scan must not consume RNG"),
            |_| panic!("Peace scan must not query visibility")
        )
        .is_none());
        let mut perception = Perception::default();
        perception.remembered = Some(2);
        let selected = perception.decide(
            ctx,
            None,
            &[target],
            || panic!("remembered fallback needs no random roll"),
            |_, _| panic!("remembered fallback bypasses acquisition visibility"),
        );
        assert_eq!(selected.unwrap().id, 2);
    }

    #[test]
    fn acquisition_uses_roster_coin_flip_instead_of_nearest_sort() {
        let actors = [actor(2, 0.0, 20.0), actor(3, 0.0, 5.0), actor(4, 0.0, 8.0)];
        assert_eq!(
            select_hostile(context().scan(false), &actors, || 0, |_| true)
                .unwrap()
                .id,
            2
        );
        assert_eq!(
            select_hostile(context().scan(false), &actors, || 1, |_| true)
                .unwrap()
                .id,
            4
        );
        let mut rolls = [1, 0].into_iter();
        assert_eq!(
            select_hostile(
                context().scan(false),
                &actors,
                || rolls.next().unwrap(),
                |_| true
            )
            .unwrap()
            .id,
            3
        );
    }

    #[test]
    fn follow_scans_around_escort_with_weapon_radius_and_own_visibility() {
        let mut ctx = context();
        ctx.mode = Some(BehaviorMode::Follow);
        ctx.configured = Some((10, Vec3::new(0.0, 0.0, 20.0)));
        let scan = ctx.scan(false);
        assert_eq!(scan.center.z, 20.0);
        assert!((scan.radius - 230.0 * S).abs() < 1e-5);

        let target = actor(2, 0.0, 40.0);
        let mut state = Perception::default();
        assert_eq!(
            state
                .decide(
                    ctx,
                    None,
                    &[target],
                    || 0,
                    |a, _| a.position.length() < 48.0
                )
                .unwrap()
                .id,
            2
        );
        let mut state = Perception::default();
        assert!(state
            .decide(ctx, None, &[target], || 0, |_, _| false)
            .is_none());

        ctx.configured = Some((10, Vec3::new(0.0, 0.0, -20.0)));
        assert!(select_hostile(ctx.scan(false), &[actor(2, 0.0, -21.0)], || 0, |_| true).is_some());
    }

    #[test]
    fn follow_engagement_and_leash_are_separate_from_scan_radius() {
        let mut ctx = context();
        ctx.mode = Some(BehaviorMode::Follow);
        ctx.configured = Some((10, Vec3::new(0.0, 0.0, 31.0)));
        let mut state = Perception::default();
        assert!(state
            .decide(ctx, None, &[actor(2, 0.0, 32.0)], || 0, |_, _| true)
            .is_none());
        ctx.configured = Some((10, Vec3::new(0.0, 0.0, 20.0)));
        assert!(state
            .decide(ctx, None, &[actor(2, 0.0, 32.0)], || 0, |_, _| true)
            .is_some());
        ctx.configured = Some((10, Vec3::new(0.0, 19.0, 0.0)));
        assert!(state
            .decide(ctx, Some(2), &[actor(2, 0.0, 32.0)], || 1, |_, _| true)
            .is_some());
        assert!(state.parent_returned && state.clear_navigation && !state.in_combat());
    }

    #[test]
    fn combat_retains_near_or_navigating_target_without_reacquisition_gates() {
        for (target, navigating) in [(actor(2, 0.0, -24.0), false), (actor(2, 0.0, -40.0), true)] {
            let mut state = Perception::default();
            let mut ctx = context();
            state
                .decide(ctx, None, &[actor(2, 0.0, 10.0)], || 0, |_, _| true)
                .unwrap();
            ctx.navigating = navigating;
            let retained = state.decide(
                ctx,
                Some(2),
                &[target],
                || 1,
                |_, _| panic!("near/in-path memory does not query visibility"),
            );
            assert_eq!(retained.unwrap().id, 2);
            assert!(!state.searching());
            assert!(
                !state.clear_navigation,
                "retention must preserve the active chase"
            );
        }
    }

    #[test]
    fn lost_distant_enemy_enters_passive_double_view_search_then_expires() {
        let mut state = Perception::default();
        let ctx = context();
        let actors = [actor(2, 0.0, 40.0)];
        state.decide(ctx, None, &actors, || 0, |_, _| true).unwrap();
        assert!(state
            .decide(ctx, Some(2), &actors, || 1, |_, _| false)
            .is_none());
        assert!(state.searching() && state.passive);
        assert!(
            state.clear_navigation,
            "search must discard the old chase path"
        );
        assert_eq!(state.callback_delay(), 0.25);
        for tick in 0..=8 {
            assert_eq!(state.search_tick, Some(tick));
            assert!(state
                .decide(
                    ctx,
                    None,
                    &actors,
                    || 1,
                    |_, multiplier| {
                        assert_eq!(multiplier, 2.0);
                        false
                    }
                )
                .is_none());
            assert!(state.passive);
        }
        state.decide(
            ctx,
            None,
            &actors,
            || 1,
            |_, _| panic!("expired search must not scan"),
        );
        assert!(!state.searching());
        assert_eq!(state.view_multiplier, 1.0);
    }

    #[test]
    fn search_success_returns_without_attacking_and_behavior_reset_restores_view() {
        let mut state = Perception::default();
        let ctx = context();
        let actors = [actor(2, 0.0, 40.0)];
        state.decide(ctx, None, &actors, || 0, |_, _| true);
        state.decide(ctx, Some(2), &actors, || 1, |_, _| false);
        assert!(state
            .decide(ctx, None, &actors, || 0, |_, multiplier| multiplier == 2.0)
            .is_none());
        assert!(state.passive && !state.searching());
        assert_eq!(state.remembered, Some(2));
        assert_eq!(
            state.view_multiplier, 2.0,
            "EXE success has no multiplier reset"
        );
        assert!(state
            .decide(ctx, None, &actors, || 0, |_, multiplier| multiplier == 2.0)
            .is_some());
        state = Perception::default();
        assert_eq!(state.view_multiplier, 1.0);
    }

    #[test]
    fn parent_callback_restores_remembered_enemy_within_original_200_unit_gate() {
        let mut state = Perception::default();
        let ctx = context();
        let target = actor(2, 0.0, 40.0);
        state
            .decide(ctx, None, &[target], || 0, |_, _| true)
            .unwrap();
        state.decide(ctx, Some(2), &[target], || 1, |_, _| false);
        assert!(state.searching());

        let behind = actor(2, 0.0, -20.0);
        for _ in 0..=9 {
            assert!(state
                .decide(ctx, None, &[behind], || 1, |_, _| true)
                .is_none());
        }
        assert!(!state.searching());
        let found = state.decide(ctx, None, &[behind], || 1, |_, _| false);
        assert_eq!(found.map(|actor| actor.id), Some(2));
        assert!(!state.passive);

        state = Perception::default();
        state.remembered = Some(2);
        for invalid in [
            actor(2, 0.0, -25.0),
            AiActorSnapshot {
                alive: false,
                ..behind
            },
            AiActorSnapshot { team: 1, ..behind },
        ] {
            assert!(state
                .decide(ctx, None, &[invalid], || 1, |_, _| false)
                .is_none());
        }
    }

    #[test]
    fn parent_callback_prefers_a_new_visible_scan_over_old_target_memory() {
        let mut state = Perception::default();
        state.remembered = Some(2);
        let actors = [actor(2, 0.0, -20.0), actor(3, 0.0, 10.0)];
        assert_eq!(
            state
                .decide(context(), None, &actors, || 0, |_, _| true)
                .map(|actor| actor.id),
            Some(3)
        );
    }

    #[test]
    fn dead_or_concealed_enemy_does_not_reappear_from_stale_memory() {
        for invalid in [
            vec![],
            vec![AiActorSnapshot {
                alive: false,
                ..actor(2, 0.0, -20.0)
            }],
            vec![AiActorSnapshot {
                team: 1,
                ..actor(2, 0.0, -20.0)
            }],
        ] {
            let mut state = Perception::default();
            state.remembered = Some(2);
            state.decide(context(), None, &invalid, || 1, |_, _| false);
            assert_eq!(state.remembered, None);
            assert!(state
                .decide(context(), None, &[actor(2, 0.0, -20.0)], || 1, |_, _| false)
                .is_none());
        }
    }

    #[test]
    fn idle_observes_without_combat_and_random_combat_exit_is_not_search() {
        let mut ctx = context();
        ctx.mode = Some(BehaviorMode::Idle);
        let actors = [actor(2, 0.0, 10.0)];
        let mut state = Perception::default();
        assert_eq!(
            state
                .decide(ctx, None, &actors, || 0, |_, _| true)
                .map(|actor| actor.id),
            Some(2)
        );
        assert!(!state.engaged);
        assert_eq!(state.remembered, Some(2));
        ctx.mode = Some(BehaviorMode::Wait);
        state.decide(ctx, None, &actors, || 0, |_, _| true).unwrap();
        assert_eq!(
            state
                .decide(ctx, Some(2), &actors, || 0, |_, _| true)
                .unwrap()
                .id,
            2
        );
        assert!(!state.engaged && !state.searching());
        assert!(
            state.clear_navigation && state.parent_returned,
            "pop keeps the enemy slot but stops the child route"
        );
        let others = [actor(3, 0.0, 5.0), actors[0]];
        assert_eq!(
            state
                .decide(ctx, Some(2), &others, || 0, |_, _| true)
                .unwrap()
                .id,
            3
        );
        assert!(state.engaged && state.clear_navigation);
    }

    #[test]
    fn active_combat_retains_route_until_its_random_parent_return() {
        let mut state = Perception::default();
        let mut ctx = context();
        let target = actor(2, 0.0, 40.0);
        state
            .decide(ctx, None, &[target], || 0, |_, _| true)
            .unwrap();
        ctx.navigating = true;
        ctx.forward = -Vec3::Z;
        for _ in 0..4 {
            let kept = state.decide(
                ctx,
                Some(2),
                &[target],
                || 1,
                |_, _| panic!("a committed route uses memory, not the initial FOV"),
            );
            assert_eq!(kept.unwrap().id, 2);
            assert!(!state.clear_navigation);
        }
        assert_eq!(
            state
                .decide(
                    ctx,
                    Some(2),
                    &[target],
                    || 0,
                    |_, _| panic!("the random pop precedes visibility")
                )
                .unwrap()
                .id,
            2
        );
        assert!(state.clear_navigation && state.parent_returned && !state.engaged);
    }

    #[test]
    fn invalidated_targets_do_not_survive_combat_memory() {
        for actors in [
            vec![],
            vec![AiActorSnapshot {
                alive: false,
                ..actor(2, 0.0, 5.0)
            }],
            vec![AiActorSnapshot {
                team: 1,
                ..actor(2, 0.0, 5.0)
            }],
        ] {
            let mut state = Perception::default();
            state.decide(context(), None, &[actor(2, 0.0, 5.0)], || 0, |_, _| true);
            assert!(state
                .decide(context(), Some(2), &actors, || 1, |_, _| true)
                .is_none());
        }
    }
}
