use meteor_core::FighterState;

pub const QI_BURST_POSE_ID: u16 = 367;

pub const QI_BURST_ACTION_ID: u16 = 85;

pub const QI_BURST_ANGER_COST: f32 = 60.0;

#[derive(Debug, Clone, Copy)]
pub struct QiBurstCommand {
    pub pose_id: u16,
    pub action_id: u16,
    pub anger_cost: f32,
}

pub const QI_BURST_COMMAND: QiBurstCommand = QiBurstCommand {
    pose_id: QI_BURST_POSE_ID,
    action_id: QI_BURST_ACTION_ID,
    anger_cost: QI_BURST_ANGER_COST,
};

pub fn qi_burst_state_allowed(state: FighterState, in_guard_hit_reaction: bool) -> bool {
    match state {
        FighterState::Dead => false,

        FighterState::Name
        | FighterState::Attack1
        | FighterState::Attack2
        | FighterState::Attack3
        | FighterState::BeHit
        | FighterState::Down
        | FighterState::Fake => true,

        FighterState::Guard => in_guard_hit_reaction,

        _ => false,
    }
}

pub fn can_qi_burst(
    state: FighterState,
    in_guard_hit_reaction: bool,
    airborne: bool,
    alive: bool,
    anger: f32,
    has_pose_367: bool,
) -> bool {
    if !qi_burst_state_allowed(state, in_guard_hit_reaction) {
        return false;
    }

    if airborne {
        return false;
    }
    if !has_pose_367 {
        return false;
    }
    if !alive {
        return false;
    }
    anger >= QI_BURST_ANGER_COST
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_cost_constants() {
        assert_eq!(QI_BURST_POSE_ID, 367);
        assert_eq!(QI_BURST_ACTION_ID, 85);
        assert_eq!(QI_BURST_ANGER_COST, 60.0);
    }

    #[test]
    fn state_gate_allows_expected() {
        assert!(qi_burst_state_allowed(FighterState::Name, false));
        assert!(qi_burst_state_allowed(FighterState::Attack1, false));
        assert!(qi_burst_state_allowed(FighterState::Attack2, false));
        assert!(qi_burst_state_allowed(FighterState::Attack3, false));
        assert!(qi_burst_state_allowed(FighterState::BeHit, false));
        assert!(qi_burst_state_allowed(FighterState::Down, false));
        assert!(qi_burst_state_allowed(FighterState::Fake, false));

        assert!(!qi_burst_state_allowed(FighterState::Dead, false));

        assert!(!qi_burst_state_allowed(FighterState::Guard, false));
        assert!(qi_burst_state_allowed(FighterState::Guard, true));

        assert!(!qi_burst_state_allowed(FighterState::Dodge, false));
    }

    #[test]
    fn can_burst_full_gate() {
        let ok = can_qi_burst(FighterState::Name, false, false, true, 60.0, true);
        assert!(ok);

        assert!(!can_qi_burst(
            FighterState::Name,
            false,
            false,
            true,
            59.9,
            true
        ));

        assert!(!can_qi_burst(
            FighterState::Name,
            false,
            true,
            true,
            100.0,
            true
        ));

        assert!(!can_qi_burst(
            FighterState::Name,
            false,
            false,
            true,
            100.0,
            false
        ));

        assert!(!can_qi_burst(
            FighterState::Dead,
            false,
            false,
            false,
            100.0,
            true
        ));

        assert!(!can_qi_burst(
            FighterState::Dodge,
            false,
            false,
            true,
            100.0,
            true
        ));
    }

    #[test]
    fn command_roundtrip() {
        assert_eq!(QI_BURST_COMMAND.pose_id, 367);
        assert_eq!(QI_BURST_COMMAND.action_id, 85);
        assert_eq!(QI_BURST_COMMAND.anger_cost, 60.0);
    }
}
