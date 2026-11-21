#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "diagnostic; set METEOR_MRC_PATH (optionally METEOR_MRC_WINDOW=from,to)"]
fn diagnose_mrc_attack_queue_decision() {
    use crate::plugins::input_plugin::{LocalCommandInput, SyntheticAction, SyntheticInputStream};
    let Ok(path) = std::env::var("METEOR_MRC_PATH") else {
        eprintln!("METEOR_MRC_PATH not set");
        return;
    };
    let window = std::env::var("METEOR_MRC_WINDOW")
        .ok()
        .and_then(|raw| {
            let (a, b) = raw.split_once(',')?;
            Some((a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?))
        })
        .unwrap_or((40.60_f32, 42.00_f32));
    let doc = meteor_asset::mrc::decode_mrc(&std::fs::read(&path).unwrap()).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut h = spawn_gameplay_harness_with_order(5, true).expect("origin_res required");
    let character = std::env::var("METEOR_MRC_CHARACTER").unwrap_or_else(|_| "p9".to_string());
    let character_id: u8 = character.trim_start_matches('p').parse().unwrap();
    let asset = crate::plugins::character_plugin::load_character_asset_for_test(
        std::slice::from_ref(&root.join("origin_res")),
        &character,
    )
    .unwrap();
    h.app
        .world_mut()
        .resource_mut::<CharacterAssetsSet>()
        .0
        .insert(character.clone(), asset);
    h.app
        .world_mut()
        .get_mut::<Fighter>(h.fighter)
        .unwrap()
        .character_id = character_id;
    let keymap = h.app.world().resource::<crate::app_state::Keymap>().clone();
    let entries: Vec<_> = doc
        .queue_b
        .iter()
        .filter(|entry| entry.time >= window.0 && entry.time <= window.1)
        .map(|entry| (entry.code, entry.value, entry.time))
        .collect();
    eprintln!("window {window:?} entries {entries:?}");
    let mut next = 0usize;
    let dt = std::time::Duration::from_nanos(1_000_000_000 / 120);
    let mut t = window.0;
    let mut last = String::new();
    while t < window.1 {
        t += 1.0 / 120.0;
        let mut edges = Vec::new();
        while next < entries.len() && entries[next].2 <= t {
            let (code, value, _) = entries[next];
            next += 1;
            let pressed = value != 0;
            let binding = match code {
                0x57 => keymap.move_up,
                0x53 => keymap.move_down,
                0x41 => keymap.move_left,
                0x44 => keymap.move_right,
                0x1000 => keymap.attack,
                0x1200 => keymap.jump,
                _ => continue,
            };
            queue_probe_set_binding(&mut h.app, binding, pressed);
            let mut stream = h.app.world_mut().resource_mut::<SyntheticInputStream>();
            match code {
                0x1000 => stream.push_action(
                    binding,
                    SyntheticAction::Attack,
                    pressed,
                    Some(f64::from(t)),
                ),
                0x1200 => {
                    stream.push_action(binding, SyntheticAction::Jump, pressed, Some(f64::from(t)))
                }
                _ => stream.push_mrc_direction(binding, pressed, Some(f64::from(t))),
            }
            edges.push(format!("{code:#x}={value}"));
        }
        h.app
            .world_mut()
            .resource_mut::<SyntheticInputStream>()
            .set_frame_clock_seconds(f64::from(t));
        let before = queue_probe_snapshot(&h);
        h.step_duration(dt);
        let after = queue_probe_snapshot(&h);
        let queued_changed = before.queued != after.queued;
        if !edges.is_empty() || before.pose != after.pose || queued_changed {
            let attack = edges.iter().any(|edge| edge.starts_with("0x1000=1"));
            eprintln!(
                "{t:8.4} edge={:?} | {} | after pose {}@{:.2} state {} queued {:?}{}",
                edges,
                before.describe(&h),
                after.pose,
                after.frame,
                after.state,
                after.queued,
                if attack {
                    format!(" | attack-pre {}", before.describe(&h))
                } else {
                    String::new()
                },
            );
        }
        last = after.describe(&h);
    }
    eprintln!("final {last}");
}

#[cfg(not(target_arch = "wasm32"))]
fn queue_probe_set_binding(app: &mut App, binding: crate::app_state::InputBinding, pressed: bool) {
    match binding {
        crate::app_state::InputBinding::Key(key) => {
            let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            if pressed {
                input.press(key);
            } else {
                input.release(key);
            }
        }
        crate::app_state::InputBinding::Mouse(button) => {
            let mut input = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
            if pressed {
                input.press(button);
            } else {
                input.release(button);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct QueueProbe {
    state: String,
    pose: u16,
    frame: f32,
    airborne: bool,
    contact: bool,
    history: Vec<meteor_input::command_parser::Dir>,
    queued: Option<u16>,
}

#[cfg(not(target_arch = "wasm32"))]
fn queue_probe_snapshot(h: &GameplayHarness) -> QueueProbe {
    use crate::plugins::input_plugin::LocalCommandInput;
    let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
    let sm = h.app.world().get::<SmState>(h.fighter).unwrap();
    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
    let history = h.app.world().resource::<LocalCommandInput>().0.peek();
    QueueProbe {
        state: format!("{:?}", sm.current),
        pose: pose.pose_id,
        frame: pose.frame,
        airborne: air.air.airborne,
        contact: air.ground_contact,
        history,
        queued: sm.attack_queued.as_ref().map(|queued| queued.next_pose_id),
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl QueueProbe {
    fn describe(&self, h: &GameplayHarness) -> String {
        let history: String = self
            .history
            .iter()
            .map(|dir| match dir {
                meteor_input::command_parser::Dir::Up => 'u',
                meteor_input::command_parser::Dir::Down => 'd',
                meteor_input::command_parser::Dir::Left => 'l',
                meteor_input::command_parser::Dir::Right => 'r',
            })
            .collect();
        let dirs: Vec<CombatDir> = self.history.iter().copied().map(combat_dir).collect();
        let fighter = h.app.world().get::<Fighter>(h.fighter).unwrap();
        let assets = h.app.world().resource::<CharacterAssetsSet>();
        let Some(asset) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            return format!("history={history:?} (no p{} assets)", fighter.character_id);
        };
        let form = h
            .app
            .world()
            .get::<WeaponForm>(h.fighter)
            .map_or(0, |form| form.current);
        let window = asset
            .poses
            .get(&self.pose)
            .and_then(|pose| pose.next_pose.as_ref())
            .map(|next| (next.start, next.end));
        let poses: Vec<u16> = asset.poses.keys().copied().collect();
        let resolution = |airborne: bool| {
            let resolved =
                resolve_affordable_weapon_command(fighter.weapon, &dirs, airborne, form, |_| true)?;
            let group = asset.act.groups.get(&resolved.pose_id).copied();
            let candidates = asset.act.edges.get(&self.pose).cloned();
            let picked = group.and_then(|group| {
                pick_act_successor(
                    group,
                    candidates.as_deref().unwrap_or_default(),
                    &poses,
                    &asset.act.groups,
                )
            });
            Some(format!(
                "pose {} group {group:?} edges {} picked {:?}",
                resolved.pose_id,
                candidates.map_or(0, |list| list.len()),
                picked,
            ))
        };
        let command_airborne = self.airborne && !self.contact;
        format!(
            "history={history:?} cmd_air={command_airborne} (air={} contact={}) window={window:?} ground[{}] air[{}]",
            self.airborne,
            self.contact,
            resolution(false).unwrap_or_else(|| "none".into()),
            resolution(true).unwrap_or_else(|| "none".into()),
        )
    }
}
