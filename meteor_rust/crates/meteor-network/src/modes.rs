pub const MODE_RULES: &[(&str, i32)] = &[
    ("leader", 1),
    ("escort", 2),
    ("siege", 3),
    ("assassin", 4),
    ("deathmatch", 5),
];

pub const TEAM_MODE_IDS: &[&str] = &["escort", "siege", "assassin", "deathmatch"];

pub const ESCORT_STAGE_IDS: &[&str] = &["sn03"];

pub const SIEGE_STAGE_IDS: &[&str] = &["sn04", "sn11", "sn12"];

pub fn mode_label(mode: &str) -> &'static str {
    match mode {
        "single" => "单人",
        "leader" => "盟主",
        "escort" => "劫镖",
        "siege" => "护城",
        "assassin" => "暗杀",
        "deathmatch" => "死斗（阵营）",
        _ => "未知模式",
    }
}

pub fn mode_objective(mode: &str) -> &'static str {
    match mode {
        "leader" => "不分阵营；限时内取得尽可能多的个人击杀，死亡 3 秒后复活",
        "escort" => "夺取镖物，并送达本阵营的原版安全区",
        "siege" => "摧毁对方机关目标并保护己方目标；时间到为平局",
        "assassin" => "保护己方随机选出的 VIP 玩家；击败对方 VIP 即获胜",
        "deathmatch" => "回合内消灭对方全部成员；时间到为平局",
        _ => "",
    }
}

pub fn is_original_mode(mode: &str) -> bool {
    MODE_RULES.iter().any(|(id, _)| *id == mode)
}

pub fn is_team_mode(mode: &str) -> bool {
    TEAM_MODE_IDS.contains(&mode)
}

pub fn is_siege_stage(stage_id: &str) -> bool {
    SIEGE_STAGE_IDS.contains(&stage_id)
}

pub fn is_escort_stage(stage_id: &str) -> bool {
    ESCORT_STAGE_IDS.contains(&stage_id)
}

pub const RESPAWN_DELAY_SECONDS: f32 = 3.0;

pub const NO_RESPAWN_RULES: [i32; 3] = [4, 5, 6];

pub const DEFAULT_LEVEL_RULE: i32 = 5;

pub fn can_respawn(rule: Option<i32>) -> bool {
    match rule {
        None => true,
        Some(value) => !NO_RESPAWN_RULES.contains(&value),
    }
}

pub const ROUND_RESULT_TIMEOUT: i32 = 1;
pub const ROUND_RESULT_TEAM1_WIPED: i32 = 2;
pub const ROUND_RESULT_TEAM2_WIPED: i32 = 4;

pub const DEFAULT_ROUND_TIME_MINUTES: i32 = 15;

pub fn round_time_limit_seconds(round_time: Option<i32>) -> i32 {
    let minutes = match round_time {
        Some(m) if m > 0 => m,
        _ => DEFAULT_ROUND_TIME_MINUTES,
    };
    minutes * 60
}

#[derive(Debug, Clone, Copy)]
pub struct RoundMember {
    pub team: i32,
    pub dead: bool,
}

pub fn evaluate_round_result(
    elapsed_seconds: f32,
    round_time: Option<i32>,
    members: &[RoundMember],
) -> i32 {
    if elapsed_seconds >= round_time_limit_seconds(round_time) as f32 {
        return ROUND_RESULT_TIMEOUT;
    }
    let mut result = 0;
    for team_id in [1, 2] {
        let team: Vec<&RoundMember> = members.iter().filter(|m| m.team == team_id).collect();
        let dead = team.iter().filter(|m| m.dead).count();
        if dead > 0 && dead == team.len() {
            result |= if team_id == 1 {
                ROUND_RESULT_TEAM1_WIPED
            } else {
                ROUND_RESULT_TEAM2_WIPED
            };
        }
    }
    result
}

pub fn mode_rule_id(mode: &str) -> Option<i32> {
    MODE_RULES
        .iter()
        .find(|(id, _)| *id == mode)
        .map(|(_, r)| *r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_rule_table() {
        assert_eq!(mode_rule_id("leader"), Some(1));
        assert_eq!(mode_rule_id("deathmatch"), Some(5));
        assert_eq!(mode_rule_id("custom"), None);
    }

    #[test]
    fn mode_detection() {
        assert!(is_original_mode("leader"));
        assert!(is_original_mode("deathmatch"));
        assert!(!is_original_mode("custom"));

        assert!(!is_team_mode("leader"));
        assert!(is_team_mode("escort"));
        assert!(is_team_mode("deathmatch"));
        assert!(is_escort_stage("sn03"));
        assert!(!is_escort_stage("sn01"));
        assert!(is_siege_stage("sn04"));
        assert!(is_siege_stage("sn11"));
        assert!(is_siege_stage("sn12"));
        assert!(!is_siege_stage("sn01"));
    }

    #[test]
    fn labels_and_objectives() {
        assert_eq!(mode_label("leader"), "盟主");
        assert_eq!(mode_label("deathmatch"), "死斗（阵营）");
        assert_eq!(mode_label("bogus"), "未知模式");
        assert!(!mode_objective("leader").is_empty());
        assert!(mode_objective("bogus").is_empty());
    }

    #[test]
    fn respawn_rules() {
        assert!(can_respawn(None));
        assert!(can_respawn(Some(1)));
        assert!(can_respawn(Some(2)));
        assert!(can_respawn(Some(3)));
        assert!(can_respawn(Some(10)));

        assert!(!can_respawn(Some(4)));
        assert!(!can_respawn(Some(5)));
        assert!(!can_respawn(Some(6)));
    }

    #[test]
    fn round_time_conversion() {
        assert_eq!(round_time_limit_seconds(None), 900);
        assert_eq!(round_time_limit_seconds(Some(10)), 600);
        assert_eq!(round_time_limit_seconds(Some(0)), 900);
    }

    #[test]
    fn round_result_timeout() {
        let members = [RoundMember {
            team: 1,
            dead: false,
        }];
        assert_eq!(
            evaluate_round_result(901.0, None, &members),
            ROUND_RESULT_TIMEOUT
        );

        assert_eq!(evaluate_round_result(100.0, None, &members), 0);
    }

    #[test]
    fn round_result_team_wiped() {
        let members = [
            RoundMember {
                team: 1,
                dead: true,
            },
            RoundMember {
                team: 2,
                dead: false,
            },
        ];
        assert_eq!(
            evaluate_round_result(100.0, None, &members),
            ROUND_RESULT_TEAM1_WIPED
        );

        let members = [
            RoundMember {
                team: 2,
                dead: true,
            },
            RoundMember {
                team: 2,
                dead: true,
            },
        ];
        assert_eq!(
            evaluate_round_result(100.0, None, &members),
            ROUND_RESULT_TEAM2_WIPED
        );

        let members = [
            RoundMember {
                team: 1,
                dead: true,
            },
            RoundMember {
                team: 2,
                dead: true,
            },
        ];
        assert_eq!(evaluate_round_result(100.0, None, &members), 6);

        assert_eq!(evaluate_round_result(100.0, None, &[]), 0);
    }
}
