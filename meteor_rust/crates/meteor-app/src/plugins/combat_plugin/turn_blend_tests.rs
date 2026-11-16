use super::*;
use bevy::ecs::system::RunSystemOnce;

fn set_source(h: &mut GameplayHarness, id: u16) -> f32 {
    h.app
        .world_mut()
        .resource_scope(|world, assets: Mut<CharacterAssetsSet>| {
            let mut pose = world.get_mut::<PoseRuntime>(h.fighter).unwrap();
            assets.0["p1"].apply_pose(&mut pose, id, true);
            pose.frame = (pose.start + 9).min(pose.end) as f32;
        });
    *h.app.world_mut().get_mut::<EntryBlend>(h.fighter).unwrap() = EntryBlend::default();
    h.app.world().get::<PoseRuntime>(h.fighter).unwrap().frame
}

#[test]
fn production_turn_blend_matches_stock_for_both_writers_and_npcs() {
    let cases = [
        (0, 141, Vec2::X, false),
        (141, 142, Vec2::NEG_X, false),
        (141, 144, Vec2::Y, false),
        (144, 141, Vec2::X, false),
        (142, 143, Vec2::NEG_Y, false),
        (145, 146, Vec2::X, true),
        (146, 147, Vec2::NEG_X, true),
        (147, 145, Vec2::Y, true),
        (141, 146, Vec2::X, true),
        (146, 141, Vec2::X, false),
        (141, 0, Vec2::ZERO, false),
        (146, 10, Vec2::ZERO, true),
    ];
    for (npc, preadvance) in [(false, true), (false, false), (true, false)] {
        let mut h = spawn_gameplay_harness(6).unwrap();
        if npc {
            h.app
                .world_mut()
                .entity_mut(h.fighter)
                .remove::<LocalFighter>();
        }
        for (from, to, input, crouching) in cases {
            let source_frame = set_source(&mut h, from);
            let world = h.app.world_mut();
            world
                .get_mut::<FighterAir>(h.fighter)
                .unwrap()
                .recovery_ground_mode = None;
            world
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .movement_mode = if crouching {
                meteor_core::MovementMode::Crouch
            } else {
                meteor_core::MovementMode::Idle
            };
            let keymap = world.resource::<crate::app_state::Keymap>().clone();
            *world.resource_mut::<ButtonInput<KeyCode>>() = default();
            for (value, binding) in [
                (input.x > 0.0, keymap.move_right),
                (input.x < 0.0, keymap.move_left),
                (input.y > 0.0, keymap.move_up),
                (input.y < 0.0, keymap.move_down),
            ] {
                if value {
                    let crate::app_state::InputBinding::Key(key) = binding else {
                        panic!()
                    };
                    world.resource_mut::<ButtonInput<KeyCode>>().press(key);
                }
            }
            world
                .get_mut::<AiIntent>(h.fighter)
                .unwrap()
                .locomotion_input = input;
            if preadvance {
                world
                    .run_system_once(preadvance_local_ground_locomotion_pose)
                    .unwrap();
            } else {
                world.run_system_once(default_poses).unwrap();
            }
            let pose = world.get::<PoseRuntime>(h.fighter).unwrap();
            let blend = world.get::<EntryBlend>(h.fighter).unwrap();
            assert_eq!(pose.pose_id, to, "{from}->{to} npc={npc} pre={preadvance}");
            assert_eq!(
                pose.frame, pose.start as f32,
                "stock blends to target Start"
            );
            assert_eq!(blend.motion_source, Some((from, source_frame as u32)));
            assert!((blend.duration - 0.12).abs() < 1e-6);
            let generation = pose.pose_instance_id;

            world.get_mut::<PoseRuntime>(h.fighter).unwrap().frame += 2.0;
            world.get_mut::<EntryBlend>(h.fighter).unwrap().elapsed = 0.06;
            if preadvance {
                world
                    .run_system_once(preadvance_local_ground_locomotion_pose)
                    .unwrap();
            }
            world.run_system_once(default_poses).unwrap();
            let pose = world.get::<PoseRuntime>(h.fighter).unwrap();
            assert_eq!(pose.frame, pose.start as f32 + 2.0);
            assert_eq!(pose.pose_instance_id, generation);
            assert_eq!(world.get::<EntryBlend>(h.fighter).unwrap().elapsed, 0.06);
        }
    }
}

#[test]
fn production_turn_blend_interpolates_real_bones_and_continues_interrupted_blend() {
    use crate::plugins::character_plugin::{
        drive_fighter_animation_for_test, spawn_fighter_model_for_test, FighterBone,
    };
    let mut h = spawn_gameplay_harness(6).unwrap();
    h.app
        .world_mut()
        .resource_mut::<Settings>()
        .new_character_models = false;
    spawn_fighter_model_for_test(h.app.world_mut(), h.fighter);
    set_source(&mut h, 141);
    drive_fighter_animation_for_test(h.app.world_mut());
    h.press_key(KeyCode::KeyW);
    h.app
        .world_mut()
        .run_system_once(preadvance_local_ground_locomotion_pose)
        .unwrap();
    drive_fighter_animation_for_test(h.app.world_mut());
    let blend = h.app.world().get::<EntryBlend>(h.fighter).unwrap();
    assert!(!blend.from_quats.is_empty());
    let sampled = h.app.world().resource::<CharacterAssetsSet>().0["p1"]
        .amb_for_pose(&h.app.world().resource::<CommonAmb>().0, 144)
        .sample_at(blend.to_frame);
    let (index, angle) = blend
        .from_quats
        .iter()
        .zip(&sampled.bone_quats)
        .enumerate()
        .map(|(i, (from, to))| {
            (
                i,
                Quat::from_array(*from).angle_between(Quat::from_array(*to)),
            )
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    assert!(
        angle > 0.01,
        "fixture must distinguish the source and target bones"
    );
    let source = Quat::from_array(blend.from_quats[index]);
    let target = Quat::from_array(sampled.bone_quats[index]);
    h.app
        .world_mut()
        .resource_mut::<Time<bevy::time::Real>>()
        .advance_by(std::time::Duration::from_secs_f64(0.06));
    drive_fighter_animation_for_test(h.app.world_mut());
    let world = h.app.world_mut();
    let actual = world
        .query::<(&FighterBone, &Transform)>()
        .iter(world)
        .find(|(bone, _)| bone.skin_index == index)
        .unwrap()
        .1
        .rotation;
    assert!(actual.angle_between(source.slerp(target, 0.5)) < 0.001);

    *world.resource_mut::<ButtonInput<KeyCode>>() = default();
    world
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);
    world
        .run_system_once(preadvance_local_ground_locomotion_pose)
        .unwrap();
    drive_fighter_animation_for_test(world);
    let blend = world.get::<EntryBlend>(h.fighter).unwrap();
    assert_eq!(blend.pose_id, 142);
    assert!(Quat::from_array(blend.from_quats[index]).angle_between(actual) < 0.001);
}

#[test]
fn production_turn_blend_keeps_other_pose_entry_rules() {
    let mut entries = Vec::new();
    for source in [0, 141] {
        for jumping in [false, true] {
            let mut h = spawn_gameplay_harness(6).unwrap();
            if jumping {
                h.press_key(KeyCode::Space);
                h.step_dt(1.0 / 60.0);
            }
            set_source(&mut h, source);
            if jumping {
                h.release_key(KeyCode::Space);
            } else {
                h.press_mouse(MouseButton::Left);
            }
            h.step_dt(1.0 / 60.0);
            let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
            let blend = h.app.world().get::<EntryBlend>(h.fighter).unwrap();
            assert_ne!(pose.pose_id, source);
            if jumping {
                assert!(h.airborne());
                assert!((blend.duration - 0.12).abs() < 1e-6);
            } else {
                assert!(is_attacking_state(h.state()));
            }
            entries.push((pose.pose_id, blend.duration, blend.kind));
        }
    }
    assert_eq!(
        entries[0], entries[2],
        "attack entry retains its original rules"
    );
    assert_eq!(
        entries[1], entries[3],
        "jump entry retains its original rules"
    );
}
