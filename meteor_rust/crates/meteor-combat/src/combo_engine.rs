use std::collections::HashMap;

pub const ANGER_FULL: u16 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeaponCommand {
    pub input: &'static [Dir],
    pub pose_id: u16,
    pub action_id: u16,
    pub anger_cost: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct ResolvedCommand {
    pub pose_id: u16,
    pub action_id: u16,
    pub anger_cost: u16,

    pub matched_length: usize,
}

impl ResolvedCommand {
    pub fn with_act_entry(mut self, act: &ActData) -> Option<Self> {
        self.pose_id = *act.group_members.get(&self.action_id)?.first()?;
        Some(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WeaponAttackEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub ground_light: u16,
    pub air_light: u16,
}

pub const WEAPON_ATTACK_TABLE: [WeaponAttackEntry; 12] = [
    WeaponAttackEntry {
        id: "dart",
        name: "鏢",
        ground_light: 200,
        air_light: 202,
    },
    WeaponAttackEntry {
        id: "wheel",
        name: "輪",
        ground_light: 218,
        air_light: 338,
    },
    WeaponAttackEntry {
        id: "gun",
        name: "銃",
        ground_light: 206,
        air_light: 338,
    },
    WeaponAttackEntry {
        id: "stinger",
        name: "刺",
        ground_light: 230,
        air_light: 226,
    },
    WeaponAttackEntry {
        id: "dagger",
        name: "匕",
        ground_light: 248,
        air_light: 245,
    },
    WeaponAttackEntry {
        id: "sword",
        name: "劍",
        ground_light: 262,
        air_light: 260,
    },
    WeaponAttackEntry {
        id: "spear",
        name: "槍",
        ground_light: 279,
        air_light: 338,
    },
    WeaponAttackEntry {
        id: "blade",
        name: "刀",
        ground_light: 295,
        air_light: 338,
    },
    WeaponAttackEntry {
        id: "hammer",
        name: "錘",
        ground_light: 315,
        air_light: 338,
    },
    WeaponAttackEntry {
        id: "staff-blade",
        name: "杖刀",
        ground_light: 432,
        air_light: 458,
    },
    WeaponAttackEntry {
        id: "fist",
        name: "指虎",
        ground_light: 407,
        air_light: 405,
    },
    WeaponAttackEntry {
        id: "ninja-blade",
        name: "忍刀",
        ground_light: 461,
        air_light: 478,
    },
];

macro_rules! cmd {
    ($input:expr, $pose:expr, $action:expr $(, $cost:expr)?) => {
        WeaponCommand {
            input: $input,
            pose_id: $pose,
            action_id: $action,
            anger_cost: cmd!(@cost $($cost)?),
        }
    };
    (@cost) => { None };
    (@cost $cost:expr) => { Some($cost) };
}

#[derive(Debug, Clone)]
pub struct WeaponCommandTable {
    pub ground: &'static [WeaponCommand],
    pub air: &'static [WeaponCommand],

    pub forms: Option<[(&'static [WeaponCommand], &'static [WeaponCommand]); 3]>,
}

pub fn weapon_command_table(weapon_id: u8) -> Option<&'static WeaponCommandTable> {
    use weapon_tables::*;
    match weapon_id {
        1 => Some(&DART),
        2 => Some(&WHEEL),
        3 => Some(&GUN),
        4 => Some(&STINGER),
        5 => Some(&DAGGER),
        6 => Some(&SWORD),
        7 => Some(&SPEAR),
        8 => Some(&BLADE),
        9 => Some(&HAMMER),
        10 => Some(&STAFF_BLADE),
        11 => Some(&FIST),
        12 => Some(&NINJA_BLADE),
        _ => None,
    }
}

pub fn staff_form_for_action(action_id: u16) -> Option<u8> {
    match action_id {
        92 | 98 => Some(0),
        88 | 99 => Some(1),
        89 | 93 => Some(2),
        _ => None,
    }
}

pub fn weapon_visual_form(weapon_id: u8, command_form: u8, pose_id: u16) -> u8 {
    if weapon_id != 10 {
        return command_form;
    }

    if pose_id == 180 {
        return command_form;
    }
    let common = matches!(pose_id,
        0 | 10 | 24 | 32 | 35 | 36 | 90..=120 | 140..=148 | 150..=163
        | 176..=179 | 367 | 401..=403 | 481 | 494..=500 | 505..=508);
    let attack = matches!(
        pose_id,
        367 | 446 | 451 | 457..=459 | 486 | 487 | 520 | 521 | 524
    );
    if common || attack {
        command_form
    } else {
        0
    }
}

pub fn is_command_suffix(history: &[Dir], command: &[Dir]) -> bool {
    if command.len() > history.len() {
        return false;
    }
    let offset = history.len() - command.len();
    history[offset..] == *command
}

pub fn match_longest_command_suffix<'a>(
    directions: &[Dir],
    commands: &'a [WeaponCommand],
) -> Option<&'a WeaponCommand> {
    let mut best: Option<&WeaponCommand> = None;
    for command in commands {
        if !is_command_suffix(directions, command.input) {
            continue;
        }
        if best.is_none() || command.input.len() > best.unwrap().input.len() {
            best = Some(command);
        }
    }
    best
}

pub fn resolve_weapon_command(
    weapon_id: u8,
    directions: &[Dir],
    airborne: bool,
    weapon_form: u8,
) -> Option<ResolvedCommand> {
    let table = weapon_command_table(weapon_id)?;
    let form = weapon_form.min(2);
    let commands: &'static [WeaponCommand] = match &table.forms {
        Some(forms) => {
            if airborne {
                forms[form as usize].1
            } else {
                forms[form as usize].0
            }
        }
        None => {
            if airborne {
                table.air
            } else {
                table.ground
            }
        }
    };

    let command = match_longest_command_suffix(directions, commands)?;
    Some(ResolvedCommand {
        pose_id: command.pose_id,
        action_id: command.action_id,
        anger_cost: command.anger_cost.unwrap_or(0),
        matched_length: command.input.len(),
    })
}

pub fn can_pay_command(cost: u16, can_spend_anger: impl Fn(u16) -> bool) -> bool {
    if cost == 0 {
        return true;
    }
    can_spend_anger(cost)
}

pub fn resolve_affordable_weapon_command(
    weapon_id: u8,
    directions: &[Dir],
    airborne: bool,
    weapon_form: u8,
    can_spend_anger: impl Fn(u16) -> bool,
) -> Option<ResolvedCommand> {
    let command = resolve_weapon_command(weapon_id, directions, airborne, weapon_form);
    if let Some(c) = &command {
        if !can_pay_command(c.anger_cost, &can_spend_anger) {
            return resolve_weapon_command(weapon_id, &[], airborne, weapon_form);
        }
    }
    command
}

pub fn anger_cost_for_weapon_action(weapon_id: u8, action_id: u16) -> Option<u16> {
    let table = weapon_command_table(weapon_id)?;
    let mut sides: Vec<&[WeaponCommand]> = Vec::new();
    if let Some(forms) = &table.forms {
        for (ground, air) in forms {
            sides.push(ground);
            sides.push(air);
        }
    } else {
        sides.push(table.ground);
        sides.push(table.air);
    }
    for commands in sides {
        for entry in commands {
            if entry.action_id == action_id {
                let cost = entry.anger_cost.unwrap_or(0);
                return if cost > 0 { Some(cost) } else { None };
            }
        }
    }
    None
}

pub use meteor_asset::act::ActData;

pub fn direction_from_input(x: f32, z: f32) -> Option<u8> {
    if x * x + z * z < 0.01 {
        return None;
    }
    if z > 0.01 {
        return Some(0);
    }
    if z < -0.01 {
        return Some(2);
    }
    if x > 0.01 {
        return Some(1);
    }
    if x < -0.01 {
        return Some(3);
    }
    None
}

pub fn pick_act_successor(
    requested_action: u16,
    candidates: &[u16],
    poses_by_id: &[u16],
    groups: &HashMap<u16, u16>,
) -> Option<u16> {
    for &candidate in candidates {
        if groups.get(&candidate).copied() != Some(requested_action) {
            continue;
        }

        return poses_by_id.contains(&candidate).then_some(candidate);
    }
    None
}

#[derive(Debug, Clone)]
pub struct ComboProfile {
    pub entry_pose_id: u16,

    pub command_recognized: bool,

    pub command: Option<ResolvedCommand>,

    pub transitions: HashMap<u16, Vec<u16>>,

    pub groups: HashMap<u16, u16>,
}

pub fn combo_profile(
    weapon_id: u8,
    airborne: bool,
    command_sequence: &[Dir],
    weapon_form: u8,
    poses: &[u16],
    act: &ActData,
    can_spend_anger: impl Fn(u16) -> bool,
) -> Option<ComboProfile> {
    let entry = |directions: &[Dir]| {
        let command = resolve_affordable_weapon_command(
            weapon_id,
            directions,
            airborne,
            weapon_form,
            &can_spend_anger,
        )?;
        let command = if act.group_members.is_empty() {
            command
        } else {
            command.with_act_entry(act)?
        };
        poses.contains(&command.pose_id).then_some(command)
    };
    let command = entry(command_sequence).or_else(|| entry(&[]))?;
    let entry_pose_id = command.pose_id;

    let mut transitions = HashMap::new();
    for (from, list) in &act.edges {
        if !poses.contains(from) {
            continue;
        }
        transitions.insert(*from, list.clone());
    }

    Some(ComboProfile {
        entry_pose_id,
        command_recognized: true,
        command: Some(command),
        transitions,
        groups: act.groups.clone(),
    })
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;
    use Dir::{Down as D, Left as L, Right as R, Up as U};

    fn append_command_snapshot(output: &mut String, label: &str, commands: &[WeaponCommand]) {
        use std::fmt::Write;

        write!(output, "{label}=").unwrap();
        for (index, command) in commands.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            if command.input.is_empty() {
                output.push('-');
            } else {
                for direction in command.input {
                    output.push(match direction {
                        Dir::Up => 'U',
                        Dir::Down => 'D',
                        Dir::Left => 'L',
                        Dir::Right => 'R',
                    });
                }
            }
            write!(output, ">{}", command.pose_id).unwrap();
            write!(output, ":{}", command.action_id).unwrap();
            match command.anger_cost {
                Some(cost) => write!(output, ":{cost}").unwrap(),
                None => output.push_str(":-"),
            }
        }
        output.push('\n');
    }

    fn all_weapon_command_snapshot() -> String {
        let mut output = String::new();
        for weapon_id in 1..=12 {
            let table = weapon_command_table(weapon_id).unwrap();
            if let Some(forms) = &table.forms {
                for (form, (ground, air)) in forms.iter().enumerate() {
                    append_command_snapshot(&mut output, &format!("{weapon_id}.{form}.g"), ground);
                    append_command_snapshot(&mut output, &format!("{weapon_id}.{form}.a"), air);
                }
            } else {
                append_command_snapshot(&mut output, &format!("{weapon_id}.g"), table.ground);
                append_command_snapshot(&mut output, &format!("{weapon_id}.a"), table.air);
            }
        }
        output
    }

    #[test]
    fn all_weapon_command_tables_match_web_contract() {
        let expected = r#"1.g=DUU>203:3:100,DU>201:142:60,U>209:1:-,D>210:2:-,->200:0:-
1.a=DU>551:143:60,U>214:86:-,->202:84:-
2.g=DUU>224:11:100,DU>552:144:60,U>220:9:-,D>222:10:-,->218:8:-
2.a=DU>554:145:60,->338:58:-
3.g=DUU>216:7:100,DU>215:146:60,U>207:5:-,D>208:6:20,->206:4:-
3.a=->338:58:-
4.g=DDU>557:148:100,LRU>556:147:60,DLR>244:20:100,UU>243:19:-,DU>240:17:-,LR>239:16:-,U>233:13:-,D>241:18:-,L>238:15:-,R>237:14:-,->230:12:-
4.a=U>228:22:-,D>235:23:-,->226:21:-
5.g=DUU>259:30:100,DU>257:29:-,UU>253:26:-,DD>558:87:60,U>250:25:-,D>343:83:-,L>370:27:-,R>255:28:-,->248:24:-
5.a=DD>559:149:60,UD>256:33:-,D>252:32:-,->245:31:-
6.g=DUU>277:43:-,LRU>368:44:100,RL>561:151:60,DD>271:39:-,DU>272:40:-,UD>274:41:-,UU>275:42:-,LR>560:150:60,U>266:35:-,D>267:36:-,L>269:37:-,R>270:38:-,->262:34:-
6.a=D>265:46:-,->260:45:-
7.g=UUU>571:152:60,LRD>562:153:60,DUU>293:57:100,DDU>291:55:-,LRU>292:56:-,RL>286:50:-,DD>287:51:-,DU>288:52:-,UU>289:53:-,LR>290:54:-,U>285:49:-,D>284:48:-,->279:47:-
7.a=D>282:59:-,->338:58:-
8.g=DDU>565:155:60,LRD>310:70:100,LRU>309:69:-,RL>572:154:60,UU>303:65:-,DU>305:66:-,DD>306:67:-,LR>374:68:-,U>299:61:-,D>300:62:-,L>301:63:-,R>302:64:-,->295:60:-
8.a=D>298:71:-,->338:58:-
9.g=LRU>566:156:60,DUU>567:157:100,DDU>325:81:100,LRD>372:80:-,UU>320:76:-,DD>321:77:-,DU>319:75:-,LR>322:78:-,RL>323:79:-,U>316:73:-,D>317:74:-,->315:72:-
9.a=D>318:82:-,->338:58:-
10.0.g=DD>431:89:-,UU>568:159:60,L>430:88:-,D>438:91:-,->432:90:-
10.0.a=D>446:105:-,->458:104:-
10.1.g=DD>441:93:-,DU>570:158:60,UU>444:96:-,R>440:92:-,U>443:95:-,D>445:97:-,->442:94:-
10.1.a=D>446:105:-,->458:104:-
10.2.g=DDU>451:103:100,DU>449:101:-,UD>450:102:-,L>448:99:-,R>447:98:-,->520:100:-
10.2.a=D>446:105:-,->458:104:-
11.g=DDU>489:120:100,LRD>421:119:100,DUU>420:118:-,LRU>416:114:60,UD>419:117:-,UU>418:116:-,DU>417:115:-,LL>412:110:-,RR>413:111:-,LR>411:109:-,DD>415:113:-,D>414:112:-,->407:107:-
11.a=D>410:108:-,->405:106:-
12.g=LRUD>468:135:50,DUU>473:140:50,DDU>470:128:-,LRU>472:134:-,LRD>535:127:50,RR>527:137:-,LL>528:138:-,DD>529:139:-,UU>526:136:-,LR>540:141:10,UD>466:126:-,DU>471:133:-,U>463:123:-,D>464:124:-,->461:122:-
12.a=RR>531:130:-,LL>532:131:-,DD>533:132:-,UU>530:129:-,D>465:125:-,->478:121:-"#;

        assert_eq!(all_weapon_command_snapshot().trim(), expected);
        let staff = weapon_command_table(10).unwrap();
        let forms = staff.forms.as_ref().unwrap();
        assert_eq!(
            staff.ground, forms[0].0,
            "Web top-level staff ground = form 0"
        );
        assert_eq!(
            staff.air, forms[0].1,
            "Web top-level staff air = shared air table"
        );
    }

    #[test]
    fn all_weapon_attack_entries_match_web_contract() {
        let actual: Vec<_> = WEAPON_ATTACK_TABLE
            .iter()
            .map(|entry| (entry.id, entry.ground_light, entry.air_light))
            .collect();
        assert_eq!(
            actual,
            vec![
                ("dart", 200, 202),
                ("wheel", 218, 338),
                ("gun", 206, 338),
                ("stinger", 230, 226),
                ("dagger", 248, 245),
                ("sword", 262, 260),
                ("spear", 279, 338),
                ("blade", 295, 338),
                ("hammer", 315, 338),
                ("staff-blade", 432, 458),
                ("fist", 407, 405),
                ("ninja-blade", 461, 478),
            ]
        );
    }

    #[test]
    fn every_registered_command_points_to_its_authored_act_action() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../origin_res/Common/character.act");
        let bytes = std::fs::read(path).expect("origin_res/Common/character.act");
        let act = meteor_asset::act::parse_character_act(&String::from_utf8_lossy(&bytes));

        for weapon_id in 1..=12 {
            let table = weapon_command_table(weapon_id).unwrap();
            let mut sides = Vec::new();
            if let Some(forms) = &table.forms {
                for (ground, air) in forms {
                    sides.push(*ground);
                    sides.push(*air);
                }
            } else {
                sides.push(table.ground);
                sides.push(table.air);
            }
            for command in sides.into_iter().flatten() {
                let authored_action = act.groups.get(&command.pose_id).copied();
                assert_eq!(
                    authored_action,
                    Some(command.action_id),
                    "weapon {weapon_id} pose {} actionId",
                    command.pose_id,
                );
            }
        }
    }

    #[test]
    fn suffix_matching() {
        let res = resolve_weapon_command(6, &[], false, 0).unwrap();
        assert_eq!(res.pose_id, 262);

        let res = resolve_weapon_command(6, &[U], false, 0).unwrap();
        assert_eq!(res.pose_id, 266);
    }

    #[test]
    fn longest_suffix_wins() {
        let res = resolve_weapon_command(6, &[U, U], false, 0).unwrap();
        assert_eq!(res.pose_id, 275);
    }

    #[test]
    fn double_tap_roll_prefix_is_suffix_ok() {
        let res = resolve_weapon_command(6, &[D, U], false, 0).unwrap();
        assert_eq!(res.pose_id, 272);
    }

    #[test]
    fn airborne_uses_air_table() {
        let res = resolve_weapon_command(6, &[], true, 0).unwrap();
        assert_eq!(res.pose_id, 260);
    }

    #[test]
    fn anger_gate_degrades_to_plain() {
        let res = resolve_affordable_weapon_command(6, &[L, R, U], false, 0, |_| false);
        assert_eq!(res.unwrap().pose_id, 262);

        let res = resolve_affordable_weapon_command(6, &[L, R, U], false, 0, |_| true);
        assert_eq!(res.unwrap().pose_id, 368);
    }

    #[test]
    fn staff_blade_jump_chains_keep_all_three_forms_until_landing() {
        for form in 0..=2 {
            for (rise, fall) in [
                (151, 152),
                (153, 154),
                (155, 156),
                (157, 158),
                (159, 156),
                (160, 154),
            ] {
                for pose in [rise, fall, 180, 180, 0] {
                    assert_eq!(
                        weapon_visual_form(10, form, pose),
                        form,
                        "form {form}, pose {pose}"
                    );
                }
            }

            assert_eq!(weapon_visual_form(10, form, 454), 0);
        }
    }

    #[test]
    fn staff_blade_sheathing_keeps_command_form_and_animates_separate_parts() {
        for entry in [431, 441, 450, 523] {
            let command_form = 2;
            for (pose, visible) in [(entry, 0), (485, 0), (486, 2), (403, 2)] {
                assert_eq!(weapon_visual_form(10, command_form, pose), visible);
                assert_eq!(command_form, 2, "visual fallback must not change commands");
            }
        }
    }

    #[test]
    fn staff_blade_other_attacks_restore_parts_and_common_poses_keep_form() {
        for form in [1, 2] {
            for pose in [
                0, 32, 90, 120, 140, 148, 150, 163, 176, 179, 367, 402, 403, 481, 494, 500, 505,
                508,
            ] {
                assert_eq!(weapon_visual_form(10, form, pose), form, "pose {pose}");
            }

            for pose in [
                149, 430, 431, 440, 441, 442, 447, 448, 449, 450, 485, 522, 523,
            ] {
                assert_eq!(weapon_visual_form(10, form, pose), 0, "pose {pose}");
            }
            for (pose, combined) in [
                (451, true),
                (452, false),
                (453, false),
                (454, false),
                (455, false),
                (456, false),
                (487, true),
                (520, true),
                (521, true),
                (522, false),
                (524, true),
            ] {
                assert_eq!(
                    weapon_visual_form(10, form, pose),
                    if combined { form } else { 0 }
                );
            }
        }
        for weapon in 0..=12 {
            assert_eq!(weapon_visual_form(weapon, 0, 485), 0);
            if weapon != 10 {
                assert_eq!(weapon_visual_form(weapon, 2, 485), 2);
            }
        }
    }

    #[test]
    fn staff_blade_forms() {
        let res = resolve_weapon_command(10, &[], false, 0).unwrap();
        assert_eq!(res.pose_id, 432);

        let res = resolve_weapon_command(10, &[], false, 2).unwrap();
        assert_eq!(res.pose_id, 520);
    }

    #[test]
    fn anger_cost_lookup() {
        let cost = anger_cost_for_weapon_action(6, 150).unwrap();
        assert_eq!(cost, 60);

        let cost = anger_cost_for_weapon_action(6, 44).unwrap();
        assert_eq!(cost, ANGER_FULL);
    }

    #[test]
    fn act_successor_is_first_authored_candidate_in_requested_action() {
        let groups = HashMap::from([(263u16, 34u16), (266u16, 35u16), (267u16, 34u16)]);
        let next = pick_act_successor(34, &[266, 267, 263], &[263, 266, 267], &groups);
        assert_eq!(next, Some(267));
    }

    #[test]
    fn act_successor_has_no_source_group_or_numeric_fallback() {
        let groups = HashMap::from([(262u16, 34u16), (263u16, 34u16), (266u16, 35u16)]);

        let next = pick_act_successor(36, &[263, 266], &[262, 263, 266], &groups);
        assert_eq!(next, None);
    }

    #[test]
    fn act_successor_missing_authored_candidate_aborts_scan() {
        let groups = HashMap::from([(999u16, 34u16), (263u16, 34u16), (266u16, 35u16)]);

        let next = pick_act_successor(34, &[999, 263], &[263, 266], &groups);
        assert_eq!(next, None);
    }

    #[test]
    fn act_successor_skips_missing_candidates_from_other_actions() {
        let groups = HashMap::from([(263u16, 34u16), (999u16, 35u16)]);
        let next = pick_act_successor(34, &[999, 263], &[263], &groups);
        assert_eq!(next, Some(263));
    }

    #[test]
    fn custom_act_ground_left_selects_all_three_authored_stages() {
        let mut text = "group(0)\n".repeat(63);
        text.push_str("group(600-602)\n600(602)\n602(601)");
        let act = meteor_asset::act::parse_character_act(&text);
        let poses = [301, 600, 601, 602];
        let profile = combo_profile(8, false, &[Dir::Left], 0, &poses, &act, |_| true).unwrap();
        assert_eq!(profile.entry_pose_id, 600);
        let action = profile.command.unwrap().action_id;
        for (source, target) in [(600, 602), (602, 601)] {
            assert_eq!(
                pick_act_successor(action, &act.edges[&source], &poses, &act.groups),
                Some(target)
            );
        }
    }

    #[test]
    fn staff_switch_targets_follow_all_six_registered_actions() {
        for (action, target) in [(88, 1), (89, 2), (92, 0), (93, 2), (98, 0), (99, 1)] {
            assert_eq!(staff_form_for_action(action), Some(target));
        }
        assert_eq!(staff_form_for_action(90), None);
    }

    #[test]
    fn combo_profile_neutral_entry() {
        let act = ActData {
            groups: HashMap::new(),
            edges: HashMap::new(),
            ..Default::default()
        };

        let profile = combo_profile(
            6,
            false,
            &[],
            0,
            &[200, 260, 262, 263, 266, 272, 275, 277],
            &act,
            |_| true,
        )
        .unwrap();
        assert_eq!(profile.entry_pose_id, 262);
        assert!(profile.command_recognized);
    }

    #[test]
    fn combo_profile_air_entry() {
        let act = ActData::default();

        let profile = combo_profile(6, true, &[], 0, &[260, 262, 265], &act, |_| true).unwrap();
        assert_eq!(profile.entry_pose_id, 260);
    }
}

mod weapon_tables {
    use super::*;
    use Dir::{Down as D, Left as L, Right as R, Up as U};
    pub static DART: WeaponCommandTable = WeaponCommandTable {
        ground: &[
            cmd!(&[D, U, U], 203, 3, ANGER_FULL),
            cmd!(&[D, U], 201, 142, 60),
            cmd!(&[U], 209, 1),
            cmd!(&[D], 210, 2),
            cmd!(&[], 200, 0),
        ],
        air: &[
            cmd!(&[D, U], 551, 143, 60),
            cmd!(&[U], 214, 86),
            cmd!(&[], 202, 84),
        ],
        forms: None,
    };

    pub static WHEEL: WeaponCommandTable = WeaponCommandTable {
        ground: &[
            cmd!(&[D, U, U], 224, 11, ANGER_FULL),
            cmd!(&[D, U], 552, 144, 60),
            cmd!(&[U], 220, 9),
            cmd!(&[D], 222, 10),
            cmd!(&[], 218, 8),
        ],
        air: &[cmd!(&[D, U], 554, 145, 60), cmd!(&[], 338, 58)],
        forms: None,
    };

    pub static GUN: WeaponCommandTable = WeaponCommandTable {
        ground: &[
            cmd!(&[D, U, U], 216, 7, ANGER_FULL),
            cmd!(&[D, U], 215, 146, 60),
            cmd!(&[U], 207, 5),
            cmd!(&[D], 208, 6, 20),
            cmd!(&[], 206, 4),
        ],
        air: &[cmd!(&[], 338, 58)],
        forms: None,
    };

    pub static STINGER: WeaponCommandTable = WeaponCommandTable {
        ground: &[
            cmd!(&[D, D, U], 557, 148, ANGER_FULL),
            cmd!(&[L, R, U], 556, 147, 60),
            cmd!(&[D, L, R], 244, 20, ANGER_FULL),
            cmd!(&[U, U], 243, 19),
            cmd!(&[D, U], 240, 17),
            cmd!(&[L, R], 239, 16),
            cmd!(&[U], 233, 13),
            cmd!(&[D], 241, 18),
            cmd!(&[L], 238, 15),
            cmd!(&[R], 237, 14),
            cmd!(&[], 230, 12),
        ],
        air: &[cmd!(&[U], 228, 22), cmd!(&[D], 235, 23), cmd!(&[], 226, 21)],
        forms: None,
    };

    pub static DAGGER: WeaponCommandTable = WeaponCommandTable {
        ground: &[
            cmd!(&[D, U, U], 259, 30, ANGER_FULL),
            cmd!(&[D, U], 257, 29),
            cmd!(&[U, U], 253, 26),
            cmd!(&[D, D], 558, 87, 60),
            cmd!(&[U], 250, 25),
            cmd!(&[D], 343, 83),
            cmd!(&[L], 370, 27),
            cmd!(&[R], 255, 28),
            cmd!(&[], 248, 24),
        ],
        air: &[
            cmd!(&[D, D], 559, 149, 60),
            cmd!(&[U, D], 256, 33),
            cmd!(&[D], 252, 32),
            cmd!(&[], 245, 31),
        ],
        forms: None,
    };

    pub static SWORD: WeaponCommandTable = WeaponCommandTable {
        ground: &[
            cmd!(&[D, U, U], 277, 43),
            cmd!(&[L, R, U], 368, 44, ANGER_FULL),
            cmd!(&[R, L], 561, 151, 60),
            cmd!(&[D, D], 271, 39),
            cmd!(&[D, U], 272, 40),
            cmd!(&[U, D], 274, 41),
            cmd!(&[U, U], 275, 42),
            cmd!(&[L, R], 560, 150, 60),
            cmd!(&[U], 266, 35),
            cmd!(&[D], 267, 36),
            cmd!(&[L], 269, 37),
            cmd!(&[R], 270, 38),
            cmd!(&[], 262, 34),
        ],
        air: &[cmd!(&[D], 265, 46), cmd!(&[], 260, 45)],
        forms: None,
    };

    pub static SPEAR: WeaponCommandTable = WeaponCommandTable {
        ground: &[
            cmd!(&[U, U, U], 571, 152, 60),
            cmd!(&[L, R, D], 562, 153, 60),
            cmd!(&[D, U, U], 293, 57, ANGER_FULL),
            cmd!(&[D, D, U], 291, 55),
            cmd!(&[L, R, U], 292, 56),
            cmd!(&[R, L], 286, 50),
            cmd!(&[D, D], 287, 51),
            cmd!(&[D, U], 288, 52),
            cmd!(&[U, U], 289, 53),
            cmd!(&[L, R], 290, 54),
            cmd!(&[U], 285, 49),
            cmd!(&[D], 284, 48),
            cmd!(&[], 279, 47),
        ],
        air: &[cmd!(&[D], 282, 59), cmd!(&[], 338, 58)],
        forms: None,
    };

    pub static BLADE: WeaponCommandTable = WeaponCommandTable {
        ground: &[
            cmd!(&[D, D, U], 565, 155, 60),
            cmd!(&[L, R, D], 310, 70, ANGER_FULL),
            cmd!(&[L, R, U], 309, 69),
            cmd!(&[R, L], 572, 154, 60),
            cmd!(&[U, U], 303, 65),
            cmd!(&[D, U], 305, 66),
            cmd!(&[D, D], 306, 67),
            cmd!(&[L, R], 374, 68),
            cmd!(&[U], 299, 61),
            cmd!(&[D], 300, 62),
            cmd!(&[L], 301, 63),
            cmd!(&[R], 302, 64),
            cmd!(&[], 295, 60),
        ],
        air: &[cmd!(&[D], 298, 71), cmd!(&[], 338, 58)],
        forms: None,
    };

    pub static HAMMER: WeaponCommandTable = WeaponCommandTable {
        ground: &[
            cmd!(&[L, R, U], 566, 156, 60),
            cmd!(&[D, U, U], 567, 157, ANGER_FULL),
            cmd!(&[D, D, U], 325, 81, ANGER_FULL),
            cmd!(&[L, R, D], 372, 80),
            cmd!(&[U, U], 320, 76),
            cmd!(&[D, D], 321, 77),
            cmd!(&[D, U], 319, 75),
            cmd!(&[L, R], 322, 78),
            cmd!(&[R, L], 323, 79),
            cmd!(&[U], 316, 73),
            cmd!(&[D], 317, 74),
            cmd!(&[], 315, 72),
        ],
        air: &[cmd!(&[D], 318, 82), cmd!(&[], 338, 58)],
        forms: None,
    };

    pub static STAFF_BLADE: WeaponCommandTable = WeaponCommandTable {
        ground: &[
            cmd!(&[D, D], 431, 89),
            cmd!(&[U, U], 568, 159, 60),
            cmd!(&[L], 430, 88),
            cmd!(&[D], 438, 91),
            cmd!(&[], 432, 90),
        ],
        air: &[cmd!(&[D], 446, 105), cmd!(&[], 458, 104)],
        forms: Some([
            (
                &[
                    cmd!(&[D, D], 431, 89),
                    cmd!(&[U, U], 568, 159, 60),
                    cmd!(&[L], 430, 88),
                    cmd!(&[D], 438, 91),
                    cmd!(&[], 432, 90),
                ],
                &[cmd!(&[D], 446, 105), cmd!(&[], 458, 104)],
            ),
            (
                &[
                    cmd!(&[D, D], 441, 93),
                    cmd!(&[D, U], 570, 158, 60),
                    cmd!(&[U, U], 444, 96),
                    cmd!(&[R], 440, 92),
                    cmd!(&[U], 443, 95),
                    cmd!(&[D], 445, 97),
                    cmd!(&[], 442, 94),
                ],
                &[cmd!(&[D], 446, 105), cmd!(&[], 458, 104)],
            ),
            (
                &[
                    cmd!(&[D, D, U], 451, 103, ANGER_FULL),
                    cmd!(&[D, U], 449, 101),
                    cmd!(&[U, D], 450, 102),
                    cmd!(&[L], 448, 99),
                    cmd!(&[R], 447, 98),
                    cmd!(&[], 520, 100),
                ],
                &[cmd!(&[D], 446, 105), cmd!(&[], 458, 104)],
            ),
        ]),
    };

    pub static FIST: WeaponCommandTable = WeaponCommandTable {
        ground: &[
            cmd!(&[D, D, U], 489, 120, ANGER_FULL),
            cmd!(&[L, R, D], 421, 119, ANGER_FULL),
            cmd!(&[D, U, U], 420, 118),
            cmd!(&[L, R, U], 416, 114, 60),
            cmd!(&[U, D], 419, 117),
            cmd!(&[U, U], 418, 116),
            cmd!(&[D, U], 417, 115),
            cmd!(&[L, L], 412, 110),
            cmd!(&[R, R], 413, 111),
            cmd!(&[L, R], 411, 109),
            cmd!(&[D, D], 415, 113),
            cmd!(&[D], 414, 112),
            cmd!(&[], 407, 107),
        ],
        air: &[cmd!(&[D], 410, 108), cmd!(&[], 405, 106)],
        forms: None,
    };

    pub static NINJA_BLADE: WeaponCommandTable = WeaponCommandTable {
        ground: &[
            cmd!(&[L, R, U, D], 468, 135, 50),
            cmd!(&[D, U, U], 473, 140, 50),
            cmd!(&[D, D, U], 470, 128),
            cmd!(&[L, R, U], 472, 134),
            cmd!(&[L, R, D], 535, 127, 50),
            cmd!(&[R, R], 527, 137),
            cmd!(&[L, L], 528, 138),
            cmd!(&[D, D], 529, 139),
            cmd!(&[U, U], 526, 136),
            cmd!(&[L, R], 540, 141, 10),
            cmd!(&[U, D], 466, 126),
            cmd!(&[D, U], 471, 133),
            cmd!(&[U], 463, 123),
            cmd!(&[D], 464, 124),
            cmd!(&[], 461, 122),
        ],
        air: &[
            cmd!(&[R, R], 531, 130),
            cmd!(&[L, L], 532, 131),
            cmd!(&[D, D], 533, 132),
            cmd!(&[U, U], 530, 129),
            cmd!(&[D], 465, 125),
            cmd!(&[], 478, 121),
        ],
        forms: None,
    };
}
