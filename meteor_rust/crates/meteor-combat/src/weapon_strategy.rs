pub fn attack_uses_all_candidates(strategy_id: u8, action_id: Option<u16>) -> bool {
    let Some(action) = action_id else {
        return false;
    };
    match strategy_id {
        0 => [44].contains(&action),
        1 => [30].contains(&action),
        2 => [6, 7, 146].contains(&action),
        3 => [3, 142, 143].contains(&action),
        4 => [81, 156, 157].contains(&action),
        5 => [70, 154, 155].contains(&action),
        6 => [11, 144, 145].contains(&action),
        7 => [57, 152, 153].contains(&action),
        8 => [20, 148].contains(&action),
        9 => [103, 158, 159].contains(&action),
        10 => [114, 119, 120].contains(&action),
        11 => [127, 135, 140, 141].contains(&action),
        _ => false,
    }
}

pub const WHEEL_STRATEGY_ID: u8 = 6;

pub const FIST_STRATEGY_ID: u8 = 10;

pub const FIST_SWAP_POSES: [u16; 2] = [412, 413];

pub const FIST_SWAP_ACTIONS: [u8; 2] = [110, 111];

pub const WEAPON_SWITCH_POSE_GROUND: u16 = 24;
pub const WEAPON_SWITCH_POSE_AIR: u16 = 36;

pub fn weapon_can_guard(strategy_id: u8) -> bool {
    !matches!(strategy_id, 2 | 3 | 6)
}

pub fn is_weapon_taken_from_hand(detached_weapon_parts: &[u8]) -> bool {
    !detached_weapon_parts.is_empty()
}

pub fn weapon_action_vetoed(strategy_id: u8, detached_weapon_parts: &[u8]) -> bool {
    if strategy_id != WHEEL_STRATEGY_ID {
        return false;
    }
    is_weapon_taken_from_hand(detached_weapon_parts)
}

pub fn is_fist_weapon_swap_pose(strategy_id: u8, pose_id: u16) -> bool {
    if strategy_id != FIST_STRATEGY_ID {
        return false;
    }
    FIST_SWAP_POSES.contains(&pose_id)
}

pub fn weapon_to_strategy(weapon_id: u8) -> u8 {
    match weapon_id {
        1 => 3,
        2 => 6,
        3 => 2,
        4 => 8,
        5 => 1,
        6 => 0,
        7 => 7,
        8 => 5,
        9 => 4,
        10 => 9,
        11 => 10,
        12 => 11,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_wheel_never_vetoed() {
        for strategy in [0u8, 1, 2, 3, 4, 5, 7, 8, 9, 10, 11] {
            assert!(
                !weapon_action_vetoed(strategy, &[1, 2]),
                "strategy {strategy} must not veto"
            );
        }
    }

    #[test]
    fn wheel_vetoes_when_taken() {
        assert!(weapon_action_vetoed(WHEEL_STRATEGY_ID, &[1]));

        assert!(!weapon_action_vetoed(WHEEL_STRATEGY_ID, &[]));
    }

    #[test]
    fn fist_swap_poses_only_for_fist() {
        assert!(is_fist_weapon_swap_pose(FIST_STRATEGY_ID, 412));
        assert!(is_fist_weapon_swap_pose(FIST_STRATEGY_ID, 413));

        assert!(!is_fist_weapon_swap_pose(FIST_STRATEGY_ID, 411));

        assert!(!is_fist_weapon_swap_pose(6, 412));
    }

    #[test]
    fn constants_alignment() {
        assert_eq!(WHEEL_STRATEGY_ID, 6);
        assert_eq!(FIST_STRATEGY_ID, 10);
        assert_eq!(FIST_SWAP_POSES, [412, 413]);
        assert_eq!(FIST_SWAP_ACTIONS, [110, 111]);
    }

    #[test]
    fn weapon_to_strategy_table_matches_web() {
        assert_eq!(weapon_to_strategy(1), 3);
        assert_eq!(weapon_to_strategy(2), 6);
        assert_eq!(weapon_to_strategy(3), 2);
        assert_eq!(weapon_to_strategy(4), 8);
        assert_eq!(weapon_to_strategy(5), 1);
        assert_eq!(weapon_to_strategy(6), 0);
        assert_eq!(weapon_to_strategy(7), 7);
        assert_eq!(weapon_to_strategy(8), 5);
        assert_eq!(weapon_to_strategy(9), 4);
        assert_eq!(weapon_to_strategy(10), 9);
        assert_eq!(weapon_to_strategy(11), 10);
        assert_eq!(weapon_to_strategy(12), 11);

        assert_eq!(weapon_to_strategy(3), 2);
        assert_eq!(weapon_to_strategy(1), 3);
        assert_eq!(weapon_to_strategy(2), 6);
    }
}

#[cfg(all(test, feature = "original-assets"))]
mod original_candidate_policy_tests {
    use super::*;
    #[test]
    fn all_strategy_candidate_policies_match_original_factory_execution() {
        let reference: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../analysis/mrc-moves-20260928/probe-strategy-candidate-policy.json"
        )))
        .unwrap();
        for row in reference["samples"].as_array().unwrap() {
            let strategy = row["strategy"].as_u64().unwrap() as u8;
            for action in 0..256 {
                let expected = row["all_candidates_actions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v.as_u64() == Some(action as u64));
                assert_eq!(
                    attack_uses_all_candidates(strategy, Some(action)),
                    expected,
                    "strategy={strategy} action={action}"
                );
            }
            assert!(!attack_uses_all_candidates(strategy, None));
        }
    }
}
