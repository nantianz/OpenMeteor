use super::*;
use crate::plugins::character_plugin::{
    drive_fighter_animation_for_test, load_character_asset_for_test, spawn_fighter_model_for_test,
};
use crate::plugins::fighter_frame::FighterUpdate;

#[derive(Resource, Default, Clone, Copy, Debug)]
struct GetupSample {
    pose: u16,
    frame: f32,
    end: u32,
    finished: bool,
}

fn sample_getup_before_reaction(
    mut sample: ResMut<GetupSample>,
    fighter: Query<&PoseRuntime, With<LocalFighter>>,
) {
    let pose = fighter.single().unwrap();
    *sample = GetupSample {
        pose: pose.pose_id,
        frame: pose.frame,
        end: pose.end,
        finished: pose.finished(),
    };
}

fn getup_harness(character_id: u8, source_pose: u16) -> GameplayHarness {
    let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
    h.app.insert_resource(State::new(AppState::Playing));
    if character_id != 1 {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let name = format!("p{character_id}");
        let character = load_character_asset_for_test(&[root], &name).unwrap();
        h.app
            .world_mut()
            .resource_mut::<CharacterAssetsSet>()
            .0
            .insert(name, character);
        h.app
            .world_mut()
            .get_mut::<Fighter>(h.fighter)
            .unwrap()
            .character_id = character_id;
    }
    h.app.world_mut().entity_mut(h.fighter).insert((
        HeadTrackState::default(),
        crate::components::fighter::VisualRootPin::default(),
        crate::components::fighter::FighterStatusEffects::default(),
    ));
    spawn_fighter_model_for_test(h.app.world_mut(), h.fighter);
    h.app
        .init_resource::<GetupSample>()
        .add_systems(
            FighterUpdate,
            drive_fighter_animation_for_test
                .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
        )
        .add_systems(
            FighterUpdate,
            sample_getup_before_reaction
                .after(sync_attacks_after_animation)
                .before(tick_hit_reactions)
                .in_set(crate::plugins::FighterFrameSet::PoseResolve),
        );
    let mut pose = PoseRuntime::idle();
    h.app.world().resource::<CharacterAssetsSet>().0[&format!("p{character_id}")].apply_pose(
        &mut pose,
        source_pose,
        true,
    );
    pose.frame = pose.end as f32;
    let state = if source_pose == 117 {
        FighterState::Fake
    } else {
        FighterState::Down
    };
    h.app.world_mut().entity_mut(h.fighter).insert((
        pose,
        SmState {
            current: state,
            locked: true,
            ..default()
        },
        CombatState { state, ..default() },
    ));
    drive_fighter_animation_for_test(h.app.world_mut());
    h
}

#[test]
fn production_getup_waits_for_authored_end_across_frame_rates() {
    let mut premature = Vec::new();

    for character in [0, 1, 2] {
        for fps in [30, 60, 120] {
            for (source, expected) in [(112, 119), (113, 120), (117, 35)] {
                let mut h = getup_harness(character, source);
                let dt = 1.0 / fps as f32;
                let attack = crate::app_state::Keymap::default().attack;
                h.press_binding(attack);
                h.step_dt(dt);
                h.release_binding(attack);
                assert_eq!(h.pose_id(), expected);
                assert_eq!(h.state(), FighterState::BeHit);
                let mut released = false;
                for step in 1..=fps * 2 {
                    h.step_dt(dt);
                    let sample = *h.app.world().resource::<GetupSample>();
                    assert_eq!(sample.pose, expected);
                    if h.state() != FighterState::BeHit {
                        eprintln!("getup p{character} {source}->{expected} {fps}fps released={:.6}s frame={:.6}/{} finished={}",
                            step as f32 * dt, sample.frame, sample.end, sample.finished);
                        if !sample.finished {
                            premature.push((character, fps, source, step, sample));
                        }
                        released = true;
                        break;
                    }
                    assert!(h.app.world().get::<SmState>(h.fighter).unwrap().locked);
                    assert_eq!(
                        h.app
                            .world()
                            .get::<FighterHitReaction>(h.fighter)
                            .unwrap()
                            .current
                            .as_ref()
                            .unwrap()
                            .kind,
                        HitReactKind::GetUp
                    );
                    assert!(
                        !sample.finished,
                        "finished getup must release without extra timer delay"
                    );
                }
                assert!(released, "p{character} {source} {fps}fps stuck in getup");
            }
        }
    }
    assert!(
        premature.is_empty(),
        "getups released before authored End: {premature:?}"
    );
}

#[test]
fn production_getup_rejects_movement_jump_and_attack_until_end() {
    for character in [0, 1, 2] {
        for fps in [30, 60, 120] {
            for input in ["move", "jump", "attack"] {
                let mut h = getup_harness(character, 117);
                let keys = crate::app_state::Keymap::default();
                let dt = 1.0 / fps as f32;
                h.press_binding(keys.attack);
                h.step_dt(dt);
                h.release_binding(keys.attack);
                assert_eq!(h.pose_id(), 35);
                let origin = h.position();
                let binding = match input {
                    "move" => keys.move_up,
                    "jump" => keys.jump,
                    _ => keys.attack,
                };
                if input == "move" {
                    h.press_binding(binding);
                }
                let mut completed = false;
                for _ in 0..fps * 2 {
                    if input != "move" {
                        h.press_binding(binding);
                    }
                    h.step_dt(dt);
                    if input != "move" {
                        h.release_binding(binding);
                    }
                    let sample = *h.app.world().resource::<GetupSample>();
                    assert_eq!(
                        sample.pose, 35,
                        "p{character}/{fps}/{input}: input replaced getup"
                    );
                    assert!(
                        !h.airborne(),
                        "p{character}/{fps}/{input}: jumped before recovery"
                    );
                    assert!(
                        (h.position().xz() - origin.xz()).length() < 1e-4,
                        "p{character}/{fps}/{input}: moved during getup"
                    );
                    if sample.finished {
                        assert_eq!(h.state(), FighterState::Name);
                        completed = true;
                        break;
                    }
                    assert_eq!(
                        h.state(),
                        FighterState::BeHit,
                        "p{character}/{fps}/{input}: released at {sample:?}"
                    );
                }
                assert!(completed);
                if input != "move" {
                    h.press_binding(binding);
                }
                h.step_dt(dt);
                if input == "jump" {
                    h.release_binding(binding);
                }
                h.step_dt(dt);
                h.trace(&format!("resumed p{character}/{fps}/{input}"));
                match input {
                    "move" => assert!(
                        (h.position().xz() - origin.xz()).length() > 1e-3,
                        "p{character}/{fps}/{input}: failed to resume from {origin:?}"
                    ),
                    "jump" => assert!(h.airborne()),
                    _ => assert!(is_attacking_state(h.state())),
                }
            }
        }
    }
}
