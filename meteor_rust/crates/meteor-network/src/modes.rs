pub const MODE_RULES: &[(&str, i32)] = &[
    ("leader", 1),
    ("escort", 2),
    ("siege", 3),
    ("assassin", 4),
    ("deathmatch", 5),
];

pub const TEAM_MODE_IDS: &[&str] = &["escort", "siege", "assassin", "deathmatch"];

pub const ESCORT_STAGE_IDS: &[&str] = &["sn02", "sn03"];

const ORIGINAL_STAGE_RULE_MASKS: [u32; 22] = [
    57, 27, 59, 61, 57, 25, 25, 25, 56, 57, 4, 29, 57, 57, 57, 25, 25, 25, 25, 25, 25, 17,
];

pub fn original_stage_supports_rule(stage_id: &str, rule: i32) -> bool {
    if !(1..=6).contains(&rule) {
        return false;
    }
    stage_id
        .strip_prefix("sn")
        .and_then(|number| number.parse::<usize>().ok())
        .and_then(|number| number.checked_sub(1))
        .and_then(|index| ORIGINAL_STAGE_RULE_MASKS.get(index))
        .is_some_and(|mask| mask & (1 << (rule - 1)) != 0)
}

pub fn original_stage_supports_mode(stage_id: &str, mode: &str) -> bool {
    mode_rule_id(mode).is_some_and(|rule| original_stage_supports_rule(stage_id, rule))
}

pub fn is_stage_id(stage: &str) -> bool {
    let bytes = stage.as_bytes();
    bytes.len() == 4
        && bytes.starts_with(b"sn")
        && bytes[2..].iter().all(u8::is_ascii_digit)
        && &bytes[2..] != b"00"
}

pub fn custom_stage_supports_rule(stage: &str, rule: i32) -> bool {
    is_stage_id(stage)
        && !meteor_scene::stage_registry::is_official_stage_id(stage)
        && matches!(rule, 1 | 4 | 5)
}

pub(crate) fn stage_rule_masks(
    bytes: &[u8],
) -> Result<[Option<u32>; 100], meteor_asset::res::MeteorResError> {
    let mut masks = [None; 100];
    for record in meteor_asset::res::parse_meteor_res(bytes)?.records {
        if record.category == 5 {
            if let (Some(slot), Some(mask)) = (
                masks.get_mut(record.id as usize),
                record.prop_seed_value(11),
            ) {
                *slot = Some(mask as u32);
            }
        }
    }
    Ok(masks)
}

pub fn runtime_stage_rule_mask(stage: &str) -> Option<u32> {
    static MASKS: std::sync::OnceLock<[Option<u32>; 100]> = std::sync::OnceLock::new();
    let index: usize = stage.strip_prefix("sn")?.parse().ok()?;
    if let Some(masks) = MASKS.get() {
        return *masks.get(index)?;
    }
    let bytes = meteor_asset::runtime_assets::read_entry("common/meteor.res")?;
    let masks = stage_rule_masks(&bytes).ok()?;
    *MASKS.get_or_init(|| masks).get(index)?
}

pub fn stage_supports_room_rule(stage: &str, rule: i32, mask: Option<u32>) -> bool {
    is_stage_id(stage)
        && (1..=5).contains(&rule)
        && mask.map_or_else(
            || original_stage_supports_rule(stage, rule) || custom_stage_supports_rule(stage, rule),
            |mask| mask & (1 << (rule - 1)) != 0,
        )
}

pub fn validate_siege_assets(
    stage: &str,
    mut read: impl FnMut(&str) -> Option<Vec<u8>>,
) -> Result<(), String> {
    use meteor_scene::pst_runtime::{parse_program, tokenize, Token};
    let decode = |bytes: Vec<u8>| {
        String::from_utf8(bytes)
            .unwrap_or_else(|error| encoding_rs::GB18030.decode(error.as_bytes()).0.into_owned())
    };
    let des_path = format!("{stage}/{stage}.des");
    let des = read(&des_path).ok_or_else(|| format!("护城地图缺少 {des_path}"))?;
    let desc = meteor_asset::des::parse_stage_description(&decode(des));
    for goal in ["GoalBlue", "GoalRed"] {
        if !desc
            .scene_objects
            .iter()
            .chain(&desc.dummies)
            .any(|object| {
                object.admits_ticket(1 << 3)
                    && object
                        .effect
                        .as_deref()
                        .is_some_and(|effect| effect.eq_ignore_ascii_case(goal))
            })
        {
            return Err(format!("护城地图 {des_path} 缺少可用机关 {goal}"));
        }
    }
    let pst_path = format!("{stage}/{stage}.pst");
    let pst = read(&pst_path).ok_or_else(|| format!("护城地图缺少 {pst_path}"))?;
    let (script, _, missing) = meteor_scene::scene_script_runtime::expand_scene_pst_includes(
        &pst_path,
        &decode(pst),
        &mut read,
    );
    if !missing.is_empty() {
        return Err(format!(
            "护城脚本 {pst_path} 缺少引用文件 {}",
            missing.join(", ")
        ));
    }
    parse_program(&script).map_err(|error| format!("护城脚本 {pst_path} 无法解析：{error}"))?;
    if !tokenize(&script).windows(3).any(|tokens| {
        matches!(tokens,
            [Token::Ident(name), Token::Op(open), Token::String(command)]
                if name == "GameCallBack" && open == "(" && command == "end"
        )
    }) {
        return Err(format!(
            "护城脚本 {pst_path} 缺少 GameCallBack(\"end\", team)"
        ));
    }
    Ok(())
}

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
        "escort" => "夺取镖物，送达公共安全区或本阵营安全区",
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
        assert!(is_escort_stage("sn02"));
        assert!(is_escort_stage("sn03"));
        assert!(is_escort_stage("sn02"));
        assert!(!is_escort_stage("sn01"));
        assert!(is_siege_stage("sn04"));
        assert!(is_siege_stage("sn11"));
        assert!(is_siege_stage("sn12"));
        assert!(!is_siege_stage("sn01"));
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn stage_rule_masks_match_original_resource_records() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let registry =
            meteor_asset::res::parse_meteor_res(&std::fs::read(root.join("Meteor.res")).unwrap())
                .unwrap();
        for record in registry
            .records
            .iter()
            .filter(|record| record.category == 5)
        {
            let stage = format!("sn{:02}", record.id);
            let mask = record.prop_seed_value(11).unwrap() as u32;
            for rule in 1..=6 {
                assert_eq!(
                    original_stage_supports_rule(&stage, rule),
                    mask & (1 << (rule - 1)) != 0,
                    "{stage}, rule {rule}"
                );
            }
        }
        assert!(!original_stage_supports_mode("sn09", "leader"));
        assert!(!original_stage_supports_mode("sn11", "deathmatch"));
        assert!(!original_stage_supports_mode("sn22", "assassin"));
        assert!(original_stage_supports_mode("sn02", "escort"));
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
