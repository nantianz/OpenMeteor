use super::*;
use crate::plugins::{impact_fx_plugin::CombatImpactEvent, FighterFrameSet};

#[derive(Resource)]
struct HitJudgeSample {
    player: Entity,
    npc: Entity,
    before: Option<Value>,
}

fn sample(world: &World, player: Entity, npc: Entity) -> Value {
    let mut own = body(world, player);
    own["combat_state"] = json!(format!(
        "{:?}",
        world.get::<CombatState>(player).unwrap().state
    ));
    json!({"self": own, "opponent": body(world, npc)})
}

fn sample_before_hit_judge(world: &mut World) {
    let probe = world.resource::<HitJudgeSample>();
    let active = world.get_resource::<crate::plugins::fighter_frame::ActiveFighter>();
    if !crate::plugins::fighter_frame::accepts(active, probe.npc) {
        return;
    }
    let before = sample(world, probe.player, probe.npc);
    world.resource_mut::<HitJudgeSample>().before = Some(before);
}

#[test]
#[ignore = "local opening trace replay; does not change the duel driver protocol"]
fn headless_opening_substeps() {
    #[derive(serde::Deserialize)]
    struct Replay {
        distance: f32,
        edges: Vec<Vec<(String, bool)>>,
        early_guard_tick: Option<usize>,
    }
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).unwrap();
    let replay: Replay = serde_json::from_str(&line).unwrap();
    assert!((2.0..=12.0).contains(&replay.distance));
    assert!((2..=64).contains(&replay.edges.len()));
    assert!(replay.edges[0].is_empty());
    if let Some(tick) = replay.early_guard_tick {
        assert!(tick > 0);
        assert_eq!(replay.edges[tick], [(String::from("guard"), true)]);
    }

    let (mut h, npc) = duel_harness(replay.distance, DAGGER);
    let player = h.fighter;
    let keymap = h.app.world().resource::<crate::app_state::Keymap>().clone();
    let schedule = crate::plugins::fighter_frame::schedule(&h.app);
    h.app.insert_resource(HitJudgeSample {
        player,
        npc,
        before: None,
    });
    h.app.add_systems(
        schedule,
        sample_before_hit_judge
            .in_set(FighterFrameSet::HitJudge)
            .before(attack_windows),
    );
    let mut cursor = MessageCursor::<CombatImpactEvent>::default();
    let mut out = io::stdout().lock();
    for (tick, edges) in replay.edges.iter().enumerate() {
        assert!(edges.len() <= 8);
        for substep in 0..3 {
            let begin = substep * edges.len() / 3;
            let end = (substep + 1) * edges.len() / 3;
            let applied = if replay.early_guard_tick == Some(tick) {
                if substep == 0 {
                    &edges[..]
                } else {
                    &edges[..0]
                }
            } else {
                &edges[begin..end]
            };
            let before_input = sample(h.app.world(), player, npc);
            for (name, pressed) in applied {
                if *pressed {
                    h.press_binding(binding(&keymap, name));
                } else {
                    h.release_binding(binding(&keymap, name));
                }
            }
            let guard_held = keymap.guard.pressed(
                h.app.world().resource::<ButtonInput<KeyCode>>(),
                h.app.world().resource::<ButtonInput<MouseButton>>(),
            );
            h.step();
            let before_hit_judge = h
                .app
                .world_mut()
                .resource_mut::<HitJudgeSample>()
                .before
                .take()
                .expect("fixture must reach HitJudge every step");
            let world = h.app.world();
            let impacts: Vec<_> = cursor
                .read(world.resource::<Messages<CombatImpactEvent>>())
                .map(|impact| {
                    json!({
                        "kind": format!("{:?}", impact.kind),
                        "npc_to_player": impact.attacker == npc && impact.defender == player,
                        "player_to_npc": impact.attacker == player && impact.defender == npc,
                        "reaction_pose": impact.reaction_pose,
                    })
                })
                .collect();
            writeln!(out, "SUBSTEP {}", json!({
                "tick": tick, "substep": substep, "input_edges": applied, "guard_held": guard_held,
                "before_input": before_input, "before_hit_judge": before_hit_judge,
                "after": sample(world, player, npc), "impacts": impacts,
            })).unwrap();
        }
    }
}
