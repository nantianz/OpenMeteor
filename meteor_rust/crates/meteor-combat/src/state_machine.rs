use meteor_core::FighterState;

pub const ATTACK_STATES: [FighterState; 3] = [
    FighterState::Attack1,
    FighterState::Attack2,
    FighterState::Attack3,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Idle,
    Attack,
    Jump,
    Guard,
    GuardRelease,
    Crouch,
    CrouchRelease,
    Dodge,
    Fake,
    CameraUp,
    CameraDown,
    UnlockCamera,
    Move,
    GetItem,
    SwitchWeapon,
    DropWeapon,
    DropItem,
    Burst,
    Cure,
    QiBurst,
}

impl Action {
    pub fn is_attack(&self) -> bool {
        matches!(self, Action::Attack | Action::QiBurst)
    }
}

pub fn interrupt_rules(state: FighterState) -> Option<&'static [Action]> {
    match state {
        FighterState::Attack1
        | FighterState::Attack2
        | FighterState::Attack3
        | FighterState::Dodge
        | FighterState::Burst
        | FighterState::Cure
        | FighterState::Jump => Some(&[]),
        FighterState::Fake => Some(&[Action::Dodge, Action::Burst, Action::Guard, Action::Jump]),
        FighterState::Down => Some(&[Action::Dodge, Action::Burst, Action::Jump]),
        FighterState::Dead => Some(&[]),

        _ => None,
    }
}

pub fn can_interrupt(state: FighterState, action: Action) -> bool {
    interrupt_rules(state).is_none_or(|allowed| allowed.contains(&action))
}

pub trait FighterView {
    fn guard_held(&self) -> bool;

    fn in_guard_state(&self) -> bool;

    fn pose_id(&self) -> u16;

    fn is_pose_finished(&self) -> bool;

    fn is_airborne(&self) -> bool;

    fn ground_contact(&self) -> Option<bool>;

    fn play_pose(&mut self, pose_id: u16, hard: bool);

    fn play_pose_hard_cut(&mut self, pose_id: u16) {
        self.play_pose(pose_id, true);
    }

    fn set_in_guard(&mut self, v: bool);

    fn has_pose(&self, pose_id: u16) -> bool;

    fn pose_link(&self, pose_id: u16) -> Option<u16>;

    fn pose_group(&self, pose_id: u16) -> Option<u16>;

    fn weapon_action_vetoed(&self) -> bool;

    fn can_qi_burst(&self) -> bool;

    fn start_qi_burst(&mut self) -> bool;

    fn queue_qi_burst(&mut self) -> Option<QueuedAttack>;

    fn start_neutral_attack(&mut self, airborne: bool) -> bool;

    fn queue_attack(&mut self, airborne: bool) -> Option<QueuedAttack>;

    fn consume_queued_attack(&mut self, queued: &mut QueuedAttack) -> QueuedAttackCommit;
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueuedAttack {
    pub source_pose_id: u16,
    pub next_pose_id: u16,
    pub last_evaluated_frame: u32,
    pub transition_time: f32,

    pub anger_cost: u16,

    pub action_id: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuedAttackCommit {
    Pending,

    Committed,

    Rejected,
}

pub struct StateMachine<F: FighterView> {
    fighter: F,

    pub current: FighterState,

    pub previous: FighterState,

    pub time_in_state: f32,

    pub locked: bool,

    pub attack_stage: u8,

    pub attack_queued: Option<QueuedAttack>,

    pub linked_recovery_pose_id: Option<u16>,

    pub exe_ground_mode_ticks: u8,
}

impl<F: FighterView> StateMachine<F> {
    pub fn new(fighter: F) -> Self {
        Self {
            fighter,
            current: FighterState::Name,
            previous: FighterState::Name,
            time_in_state: 0.0,
            locked: false,
            attack_stage: 0,
            attack_queued: None,
            linked_recovery_pose_id: None,
            exe_ground_mode_ticks: 0,
        }
    }

    pub fn is_attacking(&self) -> bool {
        ATTACK_STATES.contains(&self.current)
    }

    pub fn is_dead(&self) -> bool {
        self.current == FighterState::Dead
    }

    pub fn is_hit_reacting(&self) -> bool {
        matches!(self.current, FighterState::BeHit | FighterState::Down)
    }

    pub fn fighter_mut(&mut self) -> &mut F {
        &mut self.fighter
    }

    fn set_state(&mut self, state: FighterState) {
        self.previous = self.current;
        self.current = state;
        self.time_in_state = 0.0;
        self.locked = false;
    }

    pub fn transition(&mut self, state: FighterState) {
        if self.current == FighterState::Dead {
            return;
        }
        let crossed_name_boundary = state == FighterState::Name;
        let mut state = state;
        if state == FighterState::Name && self.fighter.guard_held() {
            state = FighterState::Guard;
        }
        if state == self.current {
            return;
        }
        if state != FighterState::Name {
            self.exe_ground_mode_ticks = 0;
        }
        self.attack_queued = None;
        self.linked_recovery_pose_id = None;
        self.set_state(state);
        if state != FighterState::BeHit && state != FighterState::Down {
            let pose = self.pose_for_state(state);
            if pose != 0 {
                self.fighter.play_pose(pose, true);
            }
        }
        self.fighter.set_in_guard(state == FighterState::Guard);
        if crossed_name_boundary {
            self.attack_queued = None;
            self.locked = false;
        }
    }

    fn pose_for_state(&self, state: FighterState) -> u16 {
        let preferred = match state {
            FighterState::Cure => 22,
            FighterState::Fake => 117,
            _ => 0,
        };
        if preferred != 0 && self.fighter.has_pose(preferred) {
            preferred
        } else {
            0
        }
    }

    pub fn enter_be_hit(&mut self, pose_id: u16) {
        self.exe_ground_mode_ticks = 0;
        self.attack_queued = None;
        self.linked_recovery_pose_id = None;
        self.attack_stage = 0;
        self.set_state(FighterState::BeHit);
        self.locked = true;
        self.fighter.set_in_guard(false);
        if pose_id != 0 && self.fighter.has_pose(pose_id) {
            self.fighter.play_pose_hard_cut(pose_id);
        }
    }

    pub fn enter_dead(&mut self, pose_id: u16) {
        self.exe_ground_mode_ticks = 0;
        self.attack_queued = None;
        self.linked_recovery_pose_id = None;
        self.attack_stage = 0;
        self.set_state(FighterState::Dead);
        self.locked = true;
        self.fighter.set_in_guard(false);
        if pose_id != 0 && self.fighter.has_pose(pose_id) {
            self.fighter.play_pose(pose_id, true);
        }
    }

    pub fn revive(&mut self) {
        if self.current != FighterState::Dead {
            return;
        }
        self.set_state(FighterState::Name);
        self.locked = false;
        self.fighter.play_pose(0, true);
    }

    pub fn enter_down(&mut self, pose_id: u16) {
        self.exe_ground_mode_ticks = 0;
        self.attack_queued = None;
        self.linked_recovery_pose_id = None;
        self.set_state(FighterState::Down);
        self.locked = true;
        self.fighter.set_in_guard(false);
        let id = if pose_id != 0 && self.fighter.has_pose(pose_id) {
            pose_id
        } else if self.fighter.has_pose(112) {
            112
        } else {
            113
        };
        if self.fighter.has_pose(id) {
            self.fighter.play_pose(id, true);
        }
    }

    fn command_airborne(&self) -> bool {
        if self.current == FighterState::Jump {
            return true;
        }
        if !self.fighter.is_airborne() {
            return false;
        }

        match self.fighter.ground_contact() {
            Some(contact) => !contact,
            None => true,
        }
    }

    fn movement_airborne(&self) -> bool {
        self.current == FighterState::Jump || self.fighter.is_airborne()
    }

    fn start_neutral_attack(&mut self, airborne: bool) -> bool {
        if !self.fighter.start_neutral_attack(airborne) {
            return false;
        }
        self.attack_queued = None;
        self.linked_recovery_pose_id = None;
        self.exe_ground_mode_ticks = 0;
        self.attack_stage = 1;
        self.set_state(FighterState::Attack1);
        true
    }

    fn queue_attack(&mut self, airborne: bool) -> bool {
        if self.attack_queued.is_some() {
            return false;
        }
        let Some(queued) = self.fighter.queue_attack(airborne) else {
            return false;
        };
        self.attack_queued = Some(queued);
        true
    }

    pub fn handle_input(&mut self, action: Action) -> bool {
        if self.current == FighterState::Dead {
            return false;
        }

        if action == Action::Crouch || action == Action::CrouchRelease {
            return true;
        }

        if action == Action::Guard || action == Action::GuardRelease {
            let held = action == Action::Guard;
            if held {
                if (self.current == FighterState::Fake || self.current == FighterState::Down)
                    && self.play_getup()
                {
                    return true;
                }
                if self.current == FighterState::Name {
                    self.transition(FighterState::Guard);
                }
                return true;
            }
            self.fighter.set_in_guard(false);
            if self.current == FighterState::Guard {
                self.transition(FighterState::Name);
            }
            return true;
        }

        if action == Action::QiBurst {
            return self.qi_burst();
        }

        if self.current == FighterState::BeHit {
            return false;
        }

        if action == Action::Attack
            && matches!(self.current, FighterState::Down | FighterState::Fake)
        {
            self.play_getup();
            return false;
        }

        if self.current == FighterState::Down
            && !matches!(action, Action::Dodge | Action::Burst | Action::Jump)
        {
            return false;
        }

        let airborne = self.command_airborne();

        if action == Action::Attack {
            if self.is_attacking() {
                return self.queue_attack(airborne);
            }
            if matches!(self.current, FighterState::Dodge | FighterState::Burst) {
                return self.start_neutral_attack(airborne);
            }
            if self.current != FighterState::Name && self.current != FighterState::Jump {
                return false;
            }
            return self.start_neutral_attack(airborne);
        }

        if !can_interrupt(self.current, action) {
            return false;
        }

        match action {
            Action::Idle => {
                self.transition(FighterState::Name);
                true
            }
            Action::Jump => {
                if self.is_attacking() {
                    return false;
                }
                if self.fighter.weapon_action_vetoed() {
                    return false;
                }

                if self.fighter.is_airborne() {
                    return false;
                }
                self.transition(FighterState::Jump);
                true
            }
            Action::Dodge => {
                if self.movement_airborne()
                    && self.current != FighterState::Down
                    && self.current != FighterState::Fake
                {
                    return false;
                }
                if self.fighter.weapon_action_vetoed() {
                    return false;
                }
                self.transition(FighterState::Dodge);
                true
            }
            Action::Burst => {
                if self.movement_airborne()
                    && self.current != FighterState::Down
                    && self.current != FighterState::Fake
                {
                    return false;
                }
                self.transition(FighterState::Burst);
                true
            }
            Action::Fake => {
                if self.current != FighterState::Name {
                    return false;
                }
                self.transition(FighterState::Fake);
                self.locked = true;
                true
            }
            Action::Cure => {
                if self.current != FighterState::Name || self.fighter.is_airborne() {
                    return false;
                }
                if self.fighter.weapon_action_vetoed() {
                    return false;
                }
                self.transition(FighterState::Cure);
                self.locked = true;
                true
            }
            Action::GetItem => {
                self.transition(FighterState::GetItem);
                true
            }
            _ => false,
        }
    }

    pub fn qi_burst(&mut self) -> bool {
        if !self.fighter.can_qi_burst() {
            return false;
        }
        if self.is_attacking() {
            if self.attack_queued.is_some() {
                return false;
            }
            let Some(queued) = self.fighter.queue_qi_burst() else {
                return false;
            };
            self.attack_queued = Some(queued);
            return true;
        }

        if !self.fighter.start_qi_burst() {
            return false;
        }
        self.attack_queued = None;
        self.linked_recovery_pose_id = None;
        self.exe_ground_mode_ticks = 0;
        self.attack_stage = 1;
        self.set_state(FighterState::Attack1);
        true
    }

    pub fn taunt(&mut self) -> bool {
        if self.current != FighterState::Name
            || self.fighter.is_airborne()
            || !self.fighter.has_pose(32)
        {
            return false;
        }
        self.attack_queued = None;
        self.linked_recovery_pose_id = None;
        self.set_state(FighterState::Attack1);
        self.attack_stage = 1;
        self.locked = true;
        self.fighter.play_pose(32, true);
        true
    }

    pub fn play_getup(&mut self) -> bool {
        let pose = self.fighter.pose_id();
        let preferred = if pose == 113 {
            120
        } else if pose == 117 {
            35
        } else {
            119
        };
        if !self.fighter.has_pose(preferred) {
            return false;
        }
        self.attack_queued = None;
        self.exe_ground_mode_ticks = 0;

        self.set_state(FighterState::BeHit);
        self.locked = true;
        self.linked_recovery_pose_id = Some(preferred);
        self.fighter.play_pose(preferred, true);
        true
    }

    pub fn consume_queued_attack(&mut self) -> bool {
        let Some(mut queued) = self.attack_queued.take() else {
            return false;
        };
        if self.fighter.pose_id() != queued.source_pose_id {
            return false;
        }
        match self.fighter.consume_queued_attack(&mut queued) {
            QueuedAttackCommit::Pending => {
                self.attack_queued = Some(queued);
                return false;
            }
            QueuedAttackCommit::Rejected => return false,
            QueuedAttackCommit::Committed => {}
        }

        self.attack_stage = self.attack_stage.saturating_add(1).max(2);
        let state_index = usize::from(self.attack_stage.saturating_sub(1))
            .min(ATTACK_STATES.len().saturating_sub(1));
        self.set_state(ATTACK_STATES[state_index]);
        true
    }

    pub fn update(&mut self, dt: f32) {
        self.time_in_state += dt;

        if self.current == FighterState::Name
            && self.linked_recovery_pose_id.is_some()
            && self.fighter.pose_id() == self.linked_recovery_pose_id.unwrap()
            && self.fighter.is_pose_finished()
        {
            self.linked_recovery_pose_id = None;
        }

        if self.current == FighterState::Cure {
            let linked_finished = self.linked_recovery_pose_id.is_some()
                && self.fighter.pose_id() == self.linked_recovery_pose_id.unwrap()
                && self.fighter.is_pose_finished();
            let unlinked_finished = self.linked_recovery_pose_id.is_none()
                && self.fighter.pose_id() == 22
                && self.fighter.is_pose_finished();
            if linked_finished || unlinked_finished {
                self.transition(FighterState::Name);
            }
        }

        if matches!(self.current, FighterState::GetItem | FighterState::Spawn)
            && self.time_in_state > 0.8
        {
            self.transition(FighterState::Name);
        }
    }

    pub fn sync_after_animation(&mut self) {
        if !self.is_attacking() {
            if self.current == FighterState::Name
                && self.linked_recovery_pose_id.is_some()
                && self.fighter.pose_id() == self.linked_recovery_pose_id.unwrap()
                && self.attack_queued.is_some()
            {
                self.consume_queued_attack();
            }
            return;
        }

        if let Some(recovery) = self.linked_recovery_pose_id {
            if self.fighter.pose_id() != recovery {
                self.linked_recovery_pose_id = None;
            } else {
                if self.consume_queued_attack() {
                    return;
                }
                if self.fighter.is_pose_finished()
                    && self.fighter.pose_link(self.fighter.pose_id()).is_none()
                {
                    self.linked_recovery_pose_id = None;
                    self.exe_ground_mode_ticks = 1;
                    self.transition(FighterState::Name);
                }
                return;
            }
        }

        if self.consume_queued_attack() {
            return;
        }

        if self.fighter.is_pose_finished()
            && self.fighter.pose_link(self.fighter.pose_id()).is_none()
        {
            self.exe_ground_mode_ticks = 1;
            self.transition(FighterState::Name);
        }
    }

    pub fn land_air_attack(&mut self) -> Option<u16> {
        if !self.is_attacking() {
            return None;
        }
        let current = self.fighter.pose_id();
        let recovery = self
            .fighter
            .pose_link(current)
            .filter(|&id| self.fighter.has_pose(id));
        self.attack_stage = self.attack_stage.max(1);
        if recovery.is_some_and(|id| id != current) {
            self.attack_queued = None;
        }
        self.linked_recovery_pose_id = recovery;
        Some(recovery.unwrap_or(current))
    }

    pub fn on_pose_linked(&mut self, from_pose_id: u16, to_pose_id: u16) {
        if self.fighter.pose_id() != from_pose_id {
            return;
        }

        if self.current == FighterState::Cure {
            self.linked_recovery_pose_id = Some(to_pose_id);
            return;
        }
        if !self.is_attacking() {
            return;
        }
        self.attack_queued = None;
        self.linked_recovery_pose_id = if self.fighter.pose_group(from_pose_id)
            == self.fighter.pose_group(to_pose_id)
            && self.fighter.pose_group(from_pose_id).is_some()
        {
            None
        } else {
            Some(to_pose_id)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_taunt_uses_pose32_and_returns_control_when_finished() {
        let mut fighter = StubFighter::new();
        fighter.poses.push(32);
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::BeHit;
        assert!(!sm.taunt());
        sm.current = FighterState::Name;
        sm.fighter.airborne = true;
        assert!(!sm.taunt());
        sm.fighter.airborne = false;
        assert!(sm.taunt());
        assert_eq!(sm.fighter.pose, 32);
        assert!(sm.locked);
        assert!(!sm.taunt());
        sm.fighter.finished = true;
        sm.sync_after_animation();
        assert_eq!(sm.current, FighterState::Name);
    }

    struct StubFighter {
        guard_held: bool,
        in_guard: bool,
        pose: u16,
        finished: bool,
        airborne: bool,
        ground_contact: Option<bool>,
        poses: Vec<u16>,
        links: Vec<(u16, u16)>,
        groups: Vec<(u16, u16)>,
        qi_ok: bool,
        veto: bool,
        neutral_ok: bool,
        neutral_start_calls: u32,
        qi_start_calls: u32,
        attack_queue_calls: u32,
        qi_queue_calls: u32,
        queued: Option<QueuedAttack>,
        consume_result: QueuedAttackCommit,
        committed: Vec<QueuedAttack>,
        last_play_hard: Option<bool>,
        last_play_hard_cut: bool,
    }

    impl StubFighter {
        fn new() -> Self {
            Self {
                guard_held: false,
                in_guard: false,
                pose: 0,
                finished: false,
                airborne: false,
                ground_contact: Some(true),
                poses: vec![0, 22, 35, 93, 112, 113, 117, 119, 120],
                links: Vec::new(),
                groups: Vec::new(),
                qi_ok: true,
                veto: false,
                neutral_ok: true,
                neutral_start_calls: 0,
                qi_start_calls: 0,
                attack_queue_calls: 0,
                qi_queue_calls: 0,
                queued: Some(QueuedAttack {
                    source_pose_id: 200,
                    next_pose_id: 201,
                    last_evaluated_frame: 10,
                    transition_time: 0.1,
                    anger_cost: 0,
                    action_id: 0,
                }),
                consume_result: QueuedAttackCommit::Committed,
                committed: Vec::new(),
                last_play_hard: None,
                last_play_hard_cut: false,
            }
        }
    }

    impl FighterView for StubFighter {
        fn guard_held(&self) -> bool {
            self.guard_held
        }
        fn in_guard_state(&self) -> bool {
            self.in_guard
        }
        fn pose_id(&self) -> u16 {
            self.pose
        }
        fn is_pose_finished(&self) -> bool {
            self.finished
        }
        fn is_airborne(&self) -> bool {
            self.airborne
        }
        fn ground_contact(&self) -> Option<bool> {
            self.ground_contact
        }
        fn play_pose(&mut self, pose_id: u16, hard: bool) {
            self.pose = pose_id;
            self.last_play_hard = Some(hard);
            self.last_play_hard_cut = false;
        }
        fn play_pose_hard_cut(&mut self, pose_id: u16) {
            self.pose = pose_id;
            self.last_play_hard = Some(true);
            self.last_play_hard_cut = true;
        }
        fn set_in_guard(&mut self, v: bool) {
            self.in_guard = v;
        }
        fn has_pose(&self, pose_id: u16) -> bool {
            self.poses.contains(&pose_id)
        }
        fn pose_link(&self, pose_id: u16) -> Option<u16> {
            self.links
                .iter()
                .find_map(|&(from, to)| (from == pose_id).then_some(to))
        }
        fn pose_group(&self, pose_id: u16) -> Option<u16> {
            self.groups
                .iter()
                .find_map(|&(pose, group)| (pose == pose_id).then_some(group))
        }
        fn weapon_action_vetoed(&self) -> bool {
            self.veto
        }
        fn can_qi_burst(&self) -> bool {
            self.qi_ok
        }
        fn start_qi_burst(&mut self) -> bool {
            self.qi_start_calls += 1;
            self.neutral_ok
        }
        fn queue_qi_burst(&mut self) -> Option<QueuedAttack> {
            self.qi_queue_calls += 1;
            self.queued.clone()
        }
        fn start_neutral_attack(&mut self, _airborne: bool) -> bool {
            self.neutral_start_calls += 1;
            self.neutral_ok
        }
        fn queue_attack(&mut self, _airborne: bool) -> Option<QueuedAttack> {
            self.attack_queue_calls += 1;
            self.queued.clone()
        }
        fn consume_queued_attack(&mut self, queued: &mut QueuedAttack) -> QueuedAttackCommit {
            match self.consume_result {
                QueuedAttackCommit::Pending => queued.last_evaluated_frame += 1,
                QueuedAttackCommit::Committed => self.committed.push(queued.clone()),
                QueuedAttackCommit::Rejected => {}
            }
            self.consume_result
        }
    }

    #[test]
    fn initial_state_is_name() {
        let sm = StateMachine::new(StubFighter::new());
        assert_eq!(sm.current, FighterState::Name);
        assert!(!sm.is_attacking());
        assert!(!sm.is_dead());
    }

    #[test]
    fn jump_rejects_generic_idle_interrupt() {
        let mut sm = StateMachine::new(StubFighter::new());
        sm.transition(FighterState::Jump);
        assert_eq!(sm.current, FighterState::Jump);
        assert!(!sm.handle_input(Action::Idle));
        assert_eq!(sm.current, FighterState::Jump);
    }

    #[test]
    fn guard_held_redirects_name_to_guard() {
        let mut f = StubFighter::new();
        f.guard_held = true;
        let mut sm = StateMachine::new(f);
        sm.transition(FighterState::Name);
        assert_eq!(sm.current, FighterState::Guard);
        assert!(sm.fighter.in_guard_state());
    }

    #[test]
    fn guard_release_leaves_guard() {
        let mut sm = StateMachine::new(StubFighter::new());
        sm.handle_input(Action::Guard);
        assert_eq!(sm.current, FighterState::Guard);
        sm.handle_input(Action::GuardRelease);
        assert_eq!(sm.current, FighterState::Name);
    }

    #[test]
    fn guard_attack_order_changes_the_committed_state() {
        let mut guard_first = StateMachine::new(StubFighter::new());
        assert!(guard_first.handle_input(Action::Guard));
        assert!(!guard_first.handle_input(Action::Attack));
        assert_eq!(guard_first.current, FighterState::Guard);
        assert_eq!(guard_first.fighter.neutral_start_calls, 0);

        let mut attack_first = StateMachine::new(StubFighter::new());
        assert!(attack_first.handle_input(Action::Attack));
        assert!(attack_first.handle_input(Action::Guard));
        assert_eq!(attack_first.current, FighterState::Attack1);
        assert_eq!(attack_first.fighter.neutral_start_calls, 1);
    }

    #[test]
    fn guard_release_before_attack_reopens_name_but_reverse_order_does_not() {
        let mut release_first = StateMachine::new(StubFighter::new());
        release_first.transition(FighterState::Guard);
        assert!(release_first.handle_input(Action::GuardRelease));
        assert!(release_first.handle_input(Action::Attack));
        assert_eq!(release_first.current, FighterState::Attack1);

        let mut attack_first = StateMachine::new(StubFighter::new());
        attack_first.transition(FighterState::Guard);
        assert!(!attack_first.handle_input(Action::Attack));
        assert!(attack_first.handle_input(Action::GuardRelease));
        assert_eq!(attack_first.current, FighterState::Name);
    }

    #[test]
    fn attack_states_are_fully_locked() {
        for state in ATTACK_STATES {
            let allowed = interrupt_rules(state).unwrap();
            assert!(
                allowed.is_empty(),
                "attack state {state:?} must not interrupt"
            );
        }
    }

    #[test]
    fn unlocked_action_states_still_apply_interrupt_rules() {
        for state in [
            FighterState::Attack1,
            FighterState::Dodge,
            FighterState::Burst,
        ] {
            let mut sm = StateMachine::new(StubFighter::new());
            sm.transition(state);
            assert!(!sm.locked, "{state:?} intentionally stores locked=false");
            for action in [Action::Idle, Action::Jump, Action::Dodge, Action::GetItem] {
                assert!(!sm.handle_input(action), "{state:?} accepted {action:?}");
                assert_eq!(sm.current, state);
            }
        }
        assert!(can_interrupt(FighterState::Name, Action::Dodge));
        assert!(can_interrupt(FighterState::Guard, Action::Dodge));
    }

    #[test]
    fn dodge_rejects_free_air_and_weapon_action_veto() {
        let mut airborne = StubFighter::new();
        airborne.airborne = true;
        airborne.ground_contact = Some(false);
        let mut sm = StateMachine::new(airborne);
        assert!(!sm.handle_input(Action::Dodge));
        assert_eq!(sm.current, FighterState::Name);

        let mut vetoed = StubFighter::new();
        vetoed.veto = true;
        let mut sm = StateMachine::new(vetoed);
        assert!(!sm.handle_input(Action::Dodge));
        assert_eq!(sm.current, FighterState::Name);
    }

    #[test]
    fn dead_accepts_nothing() {
        let mut sm = StateMachine::new(StubFighter::new());
        sm.enter_dead(0);
        assert!(sm.is_dead());
        for action in [
            Action::Idle,
            Action::Attack,
            Action::Guard,
            Action::Jump,
            Action::Dodge,
            Action::QiBurst,
        ] {
            assert!(!sm.handle_input(action), "dead must reject {action:?}");
        }

        sm.revive();
        assert_eq!(sm.current, FighterState::Name);
    }

    #[test]
    fn behit_is_fully_locked() {
        let mut sm = StateMachine::new(StubFighter::new());
        sm.enter_be_hit(93);
        assert!(sm.locked);
        assert!(sm.is_hit_reacting());
        assert!(!sm.handle_input(Action::Jump));
        assert!(!sm.handle_input(Action::Attack));
        assert_eq!(sm.current, FighterState::BeHit);
        assert!(sm.fighter.last_play_hard_cut);
    }

    #[test]
    fn down_allows_only_roll_actions() {
        let mut sm = StateMachine::new(StubFighter::new());
        sm.enter_down(112);
        assert_eq!(sm.current, FighterState::Down);

        assert!(!sm.handle_input(Action::Attack));
        assert_eq!(sm.current, FighterState::BeHit);
        assert!(sm.locked);
        assert_eq!(sm.linked_recovery_pose_id, Some(119));
        assert_eq!(sm.fighter.pose, 119);
        assert!(!sm.handle_input(Action::Jump));
        assert!(!sm.handle_input(Action::Attack));
        assert_eq!(sm.fighter.pose, 119);
    }

    #[test]
    fn fake_accepts_getup_via_guard() {
        let mut sm = StateMachine::new(StubFighter::new());
        sm.transition(FighterState::Fake);
        assert_eq!(sm.current, FighterState::Fake);
        assert_eq!(sm.fighter.pose, 117);
        assert_eq!(sm.fighter.last_play_hard, Some(true));

        assert!(sm.handle_input(Action::Guard));
        assert_eq!(sm.current, FighterState::BeHit);
        assert!(sm.locked);
        assert_eq!(sm.linked_recovery_pose_id, Some(35));
        assert_eq!(sm.fighter.pose, 35);
    }

    #[test]
    fn cure_and_fake_reset_the_target_pose_cursor_like_web_play_pose() {
        let mut cure = StateMachine::new(StubFighter::new());
        assert!(cure.handle_input(Action::Cure));
        assert_eq!(cure.current, FighterState::Cure);
        assert_eq!(cure.fighter.pose, 22);
        assert_eq!(cure.fighter.last_play_hard, Some(true));

        let mut fake = StateMachine::new(StubFighter::new());
        assert!(fake.handle_input(Action::Fake));
        assert_eq!(fake.current, FighterState::Fake);
        assert_eq!(fake.fighter.pose, 117);
        assert_eq!(fake.fighter.last_play_hard, Some(true));
    }

    #[test]
    fn fake_accepts_ground_jump_interrupt() {
        let mut sm = StateMachine::new(StubFighter::new());
        assert!(sm.handle_input(Action::Fake));
        assert_eq!(sm.current, FighterState::Fake);
        assert!(sm.handle_input(Action::Jump));
        assert_eq!(sm.current, FighterState::Jump);
    }

    #[test]
    fn guard_accepts_ground_jump_interrupt() {
        let mut sm = StateMachine::new(StubFighter::new());
        assert!(sm.handle_input(Action::Guard));
        assert_eq!(sm.current, FighterState::Guard);

        assert!(sm.handle_input(Action::Jump));
        assert_eq!(sm.current, FighterState::Jump);
        assert_eq!(sm.previous, FighterState::Guard);
    }

    #[test]
    fn crouch_is_hold_and_ignored_by_sm() {
        let mut sm = StateMachine::new(StubFighter::new());
        assert!(sm.handle_input(Action::Crouch));
        assert!(sm.handle_input(Action::CrouchRelease));
        assert_eq!(sm.current, FighterState::Name);
    }

    #[test]
    fn successful_neutral_attack_enters_attack1_stage1() {
        let mut sm = StateMachine::new(StubFighter::new());

        assert!(sm.handle_input(Action::Attack));
        assert_eq!(sm.current, FighterState::Attack1);
        assert_eq!(sm.previous, FighterState::Name);
        assert_eq!(sm.attack_stage, 1);
    }

    #[test]
    fn failed_neutral_attack_leaves_state_unchanged() {
        let mut fighter = StubFighter::new();
        fighter.neutral_ok = false;
        let mut sm = StateMachine::new(fighter);
        sm.transition(FighterState::Dodge);
        sm.time_in_state = 0.25;
        sm.locked = true;
        sm.linked_recovery_pose_id = Some(119);

        assert!(!sm.handle_input(Action::Attack));
        assert_eq!(sm.current, FighterState::Dodge);
        assert_eq!(sm.previous, FighterState::Name);
        assert_eq!(sm.time_in_state, 0.25);
        assert_eq!(sm.attack_stage, 0);
        assert!(sm.locked);
        assert_eq!(sm.linked_recovery_pose_id, Some(119));
    }

    #[test]
    fn queue_is_persisted_once_and_passed_to_commit() {
        let mut fighter = StubFighter::new();
        fighter.pose = 200;
        let expected = fighter.queued.clone().unwrap();
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::Attack1;
        sm.attack_stage = 1;

        assert!(sm.handle_input(Action::Attack));
        assert_eq!(sm.attack_queued, Some(expected.clone()));
        assert!(
            !sm.handle_input(Action::Attack),
            "registration is a one-shot latch"
        );

        assert!(sm.consume_queued_attack());
        assert_eq!(sm.attack_queued, None);
        assert_eq!(sm.attack_stage, 2);
        assert_eq!(sm.current, FighterState::Attack2);
        let committed = expected;
        assert_eq!(sm.fighter.committed, vec![committed]);
    }

    #[test]
    fn unready_queue_remains_persisted_with_commit_updates() {
        let mut fighter = StubFighter::new();
        fighter.pose = 200;
        fighter.consume_result = QueuedAttackCommit::Pending;
        let mut expected = fighter.queued.clone().unwrap();
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::Attack1;
        sm.attack_stage = 1;
        assert!(sm.handle_input(Action::Attack));

        assert!(!sm.consume_queued_attack());
        expected.last_evaluated_frame += 1;
        assert_eq!(sm.attack_queued, Some(expected));
        assert_eq!(sm.attack_stage, 1);
        assert_eq!(sm.current, FighterState::Attack1);
    }

    #[test]
    fn terminally_rejected_queue_is_not_retried() {
        let mut fighter = StubFighter::new();
        fighter.pose = 200;
        fighter.consume_result = QueuedAttackCommit::Rejected;
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::Attack1;
        sm.attack_stage = 1;
        assert!(sm.handle_input(Action::Attack));

        assert!(!sm.consume_queued_attack());
        assert_eq!(sm.attack_queued, None);
        assert_eq!(sm.attack_stage, 1);
        assert_eq!(sm.current, FighterState::Attack1);
        assert!(sm.fighter.committed.is_empty());
    }

    #[test]
    fn long_combo_keeps_real_stage_while_state_saturates_at_attack3() {
        let mut fighter = StubFighter::new();
        fighter.pose = 200;
        let queued = fighter.queued.clone().unwrap();
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::Attack3;
        sm.attack_stage = 3;

        for expected_stage in [4, 5] {
            sm.attack_queued = Some(queued.clone());
            assert!(sm.consume_queued_attack());
            assert_eq!(sm.attack_stage, expected_stage);
            assert_eq!(sm.current, FighterState::Attack3);
        }
    }

    #[test]
    fn queue_from_stale_source_is_discarded_without_commit() {
        let mut fighter = StubFighter::new();
        fighter.pose = 200;
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::Attack1;
        sm.attack_stage = 1;
        assert!(sm.handle_input(Action::Attack));

        sm.fighter.pose = 202;
        assert!(!sm.consume_queued_attack());
        assert_eq!(sm.attack_queued, None);
        assert_eq!(sm.attack_stage, 1);
        assert!(sm.fighter.committed.is_empty());
    }

    #[test]
    fn air_attack_landing_uses_authored_link_and_keeps_attack_state() {
        let mut fighter = StubFighter::new();
        fighter.pose = 245;
        fighter.links.push((245, 332));
        fighter.poses.push(332);
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::Attack1;
        sm.attack_stage = 1;

        assert_eq!(sm.land_air_attack(), Some(332));
        assert_eq!(sm.current, FighterState::Attack1);
        assert_eq!(sm.linked_recovery_pose_id, Some(332));
        assert_eq!(sm.fighter.pose, 245);
    }

    #[test]
    fn air_attack_landing_keeps_self_contained_pose_without_link() {
        let mut fighter = StubFighter::new();
        fighter.pose = 252;
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::Attack1;
        sm.attack_stage = 1;

        assert_eq!(sm.land_air_attack(), Some(252));
        assert_eq!(sm.current, FighterState::Attack1);
        assert_eq!(sm.linked_recovery_pose_id, None);
    }

    #[test]
    fn landing_keeps_source_queue_until_the_pose_actually_changes() {
        for link in [None, Some(332)] {
            let mut fighter = StubFighter::new();
            fighter.pose = 245;
            fighter.poses.push(332);
            if let Some(id) = link {
                fighter.links.push((245, id));
            }
            let mut sm = StateMachine::new(fighter);
            sm.current = FighterState::Attack1;
            let queued = QueuedAttack {
                source_pose_id: 245,
                next_pose_id: 246,
                last_evaluated_frame: 10,
                transition_time: 0.1,
                anger_cost: 0,
                action_id: 0,
            };
            sm.attack_queued = Some(queued.clone());
            sm.land_air_attack();
            assert_eq!(
                sm.attack_queued,
                if link.is_none() { Some(queued) } else { None }
            );
        }
    }

    #[test]
    fn terminal_attack_with_authored_link_waits_for_host_resolution() {
        let mut fighter = StubFighter::new();
        fighter.pose = 200;
        fighter.finished = true;
        fighter.links.push((200, 210));
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::Attack1;
        sm.attack_stage = 1;

        sm.sync_after_animation();

        assert_eq!(sm.current, FighterState::Attack1);
        assert_eq!(sm.linked_recovery_pose_id, None);
        assert_eq!(sm.exe_ground_mode_ticks, 0);
    }

    #[test]
    fn terminal_attack_without_link_opens_buffered_jump_ground_tick() {
        let mut fighter = StubFighter::new();
        fighter.pose = 200;
        fighter.finished = true;
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::Attack1;
        sm.attack_stage = 1;

        sm.sync_after_animation();

        assert_eq!(sm.current, FighterState::Name);
        assert_eq!(sm.exe_ground_mode_ticks, 1);
    }

    #[test]
    fn finished_linked_recovery_opens_buffered_jump_ground_tick() {
        let mut fighter = StubFighter::new();
        fighter.pose = 210;
        fighter.finished = true;
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::Attack2;
        sm.attack_stage = 2;
        sm.linked_recovery_pose_id = Some(210);

        sm.sync_after_animation();

        assert_eq!(sm.current, FighterState::Name);
        assert_eq!(sm.linked_recovery_pose_id, None);
        assert_eq!(sm.exe_ground_mode_ticks, 1);
    }

    #[test]
    fn authored_link_registers_recovery_without_advancing_attack_stage() {
        let mut fighter = StubFighter::new();
        fighter.pose = 200;
        fighter.links.push((200, 210));
        fighter.groups.extend([(200, 10), (210, 11)]);
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::Attack2;
        sm.attack_stage = 2;

        sm.on_pose_linked(200, 210);

        assert_eq!(sm.current, FighterState::Attack2);
        assert_eq!(sm.attack_stage, 2);
        assert_eq!(sm.linked_recovery_pose_id, Some(210));
    }

    #[test]
    fn same_action_link_is_continuation_not_recovery() {
        let mut fighter = StubFighter::new();
        fighter.pose = 200;
        fighter.groups.extend([(200, 10), (201, 10)]);
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::Attack1;
        sm.attack_stage = 1;

        sm.on_pose_linked(200, 201);

        assert_eq!(sm.linked_recovery_pose_id, None);
    }

    #[test]
    fn cure_link_stays_locked_until_authored_recovery_finishes() {
        let mut fighter = StubFighter::new();
        fighter.pose = 22;
        fighter.finished = false;
        let mut sm = StateMachine::new(fighter);
        sm.current = FighterState::Cure;

        sm.on_pose_linked(22, 34);

        assert_eq!(sm.current, FighterState::Cure);
        assert_eq!(sm.linked_recovery_pose_id, Some(34));

        sm.fighter.pose = 34;
        sm.fighter.finished = true;
        sm.update(0.0);

        assert_eq!(sm.current, FighterState::Name);
        assert_eq!(sm.linked_recovery_pose_id, None);
    }

    #[test]
    fn qi_burst_from_name_commits() {
        let mut sm = StateMachine::new(StubFighter::new());
        assert!(sm.handle_input(Action::QiBurst));
        assert_eq!(sm.current, FighterState::Attack1);
        assert_eq!(sm.attack_stage, 1);
        assert_eq!(sm.fighter.qi_start_calls, 1);
        assert_eq!(sm.fighter.neutral_start_calls, 0);
    }

    #[test]
    fn attack_and_qi_burst_use_distinct_fighter_paths() {
        let mut attack = StateMachine::new(StubFighter::new());
        assert!(attack.handle_input(Action::Attack));
        assert_eq!(attack.fighter.neutral_start_calls, 1);
        assert_eq!(attack.fighter.qi_start_calls, 0);

        let mut burst = StateMachine::new(StubFighter::new());
        burst.current = FighterState::Attack1;
        burst.attack_stage = 1;
        assert!(burst.handle_input(Action::QiBurst));
        assert_eq!(burst.fighter.qi_queue_calls, 1);
        assert_eq!(burst.fighter.attack_queue_calls, 0);
    }

    #[test]
    fn qi_burst_blocked_when_unaffordable() {
        let mut f = StubFighter::new();
        f.qi_ok = false;
        let mut sm = StateMachine::new(f);
        assert!(!sm.handle_input(Action::QiBurst));
    }

    #[test]
    fn transient_states_expire() {
        let mut sm = StateMachine::new(StubFighter::new());
        sm.transition(FighterState::GetItem);
        sm.update(0.9);
        assert_eq!(sm.current, FighterState::Name);
    }

    #[test]
    fn interrupt_rules_table_coverage() {
        let all = [
            FighterState::Name,
            FighterState::View,
            FighterState::Think,
            FighterState::Attack1,
            FighterState::Attack2,
            FighterState::Attack3,
            FighterState::Guard,
            FighterState::Dodge,
            FighterState::Jump,
            FighterState::Look,
            FighterState::Burst,
            FighterState::Cure,
            FighterState::GetItem,
            FighterState::Spawn,
            FighterState::BeHit,
            FighterState::Down,
            FighterState::Fake,
            FighterState::Dead,
        ];
        for state in all {
            let _ = interrupt_rules(state);
        }
    }
}
