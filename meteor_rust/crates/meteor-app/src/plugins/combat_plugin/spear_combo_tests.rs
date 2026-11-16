use super::*;
use crate::components::fighter::{FighterStatusEffects, VisualRootPin};
use crate::plugins::camera_plugin::CameraBasis;
use crate::plugins::character_plugin::{
    drive_fighter_animation_for_test, spawn_fighter_model_for_test,
};
use crate::plugins::fighter_frame::FighterUpdate;
use std::fmt::Write;

fn output(name: &str, text: String) {
    let variant = std::env::var("METEOR_SPEAR_AUDIT_VARIANT").unwrap_or_else(|_| "current".into());
    assert!(variant
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-'));
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../analysis/spear-combo-20261111")
        .join(format!("{variant}-{name}"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
#[ignore = "Spear audit: emits current production timeline, not original-runtime acceptance"]
fn diagnose_spear_combo_timeline() {
    let mut rows = String::from("fps,contacts,source_degrees,target_degrees,distance,tick,phase,source_pose,source_frame,source_instance,source_x,source_y,source_z,source_yaw,source_blend,source_queue,target_pose,target_frame,target_x,target_y,target_z,target_yaw,target_state,target_locked,target_blend,target_blend_elapsed,target_minimum,hp,lock\n");
    for fps in [30, 60, 120] {
        for contact in [false, true] {
            for (source_degrees, target_degrees) in [
                (45.0_f32, 180.0_f32),
                (0.0, 135.0),
                (0.0, 180.0),
                (-45.0, 180.0),
            ] {
                for distance in [3.0, 4.0] {
                    let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(7, true);
                    h.app.insert_resource(NetState::default());
                    h.app.insert_resource(State::new(AppState::Playing));
                    h.app
                        .world_mut()
                        .entity_mut(h.fighter)
                        .insert(HeadTrackState::default());
                    h.app
                        .world_mut()
                        .entity_mut(target)
                        .insert(WeaponForm::default());
                    for entity in [h.fighter, target] {
                        h.app
                            .world_mut()
                            .entity_mut(entity)
                            .insert((VisualRootPin::default(), FighterStatusEffects::default()));
                        h.app.world_mut().get_mut::<Health>(entity).unwrap().current = 10000.0;
                    }
                    configure_lock_updates(&mut h.app);
                    if contact {
                        h.app.add_plugins(crate::plugins::CharacterContactPlugin);
                    }
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(target)
                        .unwrap()
                        .translation = Vec3::new(0.0, 0.0, distance);
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(target)
                        .unwrap()
                        .facing = target_degrees.to_radians();
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(h.fighter)
                        .unwrap()
                        .facing = source_degrees.to_radians();
                    h.app.world_mut().resource_mut::<CameraBasis>().yaw =
                        source_degrees.to_radians();
                    h.app.add_systems(
                        Update,
                        tick_target_hit_clocks
                            .in_set(crate::plugins::FighterFrameSet::PoseDecision),
                    );
                    h.app.add_systems(
                        FighterUpdate,
                        drive_fighter_animation_for_test
                            .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
                    );
                    h.app.add_systems(
                        FighterUpdate,
                        attack_windows.in_set(crate::plugins::FighterFrameSet::HitJudge),
                    );
                    h.app.add_systems(
                        FighterUpdate,
                        hit_reaction_root_motion
                            .after(crate::plugins::movement_plugin::attack_root_motion)
                            .in_set(crate::plugins::FighterFrameSet::RootMotion),
                    );
                    let mut phase = 0;
                    let mut hp_before = 10000.0;
                    let mut hits = Vec::new();
                    for tick in 0..fps * 4 {
                        for key in [KeyCode::KeyW, KeyCode::KeyS, KeyCode::KeyQ] {
                            h.release_key(key);
                        }
                        h.release_mouse(MouseButton::Left);
                        let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                        match phase {
                            0 => {
                                h.press_key(KeyCode::KeyW);
                                h.press_mouse(MouseButton::Left);
                                phase = 1;
                            }
                            1 if pose.pose_id == 285 && hp_before < 10000.0 => {
                                h.press_key(KeyCode::KeyQ);
                                h.press_key(KeyCode::KeyS);
                                phase = 2;
                            }
                            2 => {
                                phase = 3;
                            }
                            3 => {
                                h.press_key(KeyCode::KeyS);
                                h.press_mouse(MouseButton::Left);
                                phase = 4;
                            }
                            4 if pose.pose_id == 287 && pose.frame >= 2978.0 => {
                                h.press_key(KeyCode::KeyW);
                                h.press_mouse(MouseButton::Left);
                                phase = 5;
                            }
                            _ => {}
                        }
                        h.step_dt(1.0 / fps as f32);
                        let world = h.app.world();
                        let sp = world.get::<PoseRuntime>(h.fighter).unwrap();
                        let tp = world.get::<PoseRuntime>(target).unwrap();
                        let s = h.position();
                        let t = world.get::<Transform>(target).unwrap().translation;
                        let syaw = world.get::<MotionState>(h.fighter).unwrap().facing;
                        let tyaw = world.get::<MotionState>(target).unwrap().facing;
                        let sm = world.get::<SmState>(h.fighter).unwrap();
                        let tm = world.get::<SmState>(target).unwrap();
                        let sb = world.get::<EntryBlend>(h.fighter).unwrap();
                        let tb = world.get::<EntryBlend>(target).unwrap();
                        let hr = world.get::<FighterHitReaction>(target).unwrap();
                        let hp = world.get::<Health>(target).unwrap().current;
                        writeln!(rows, "{fps},{contact},{source_degrees},{target_degrees},{distance},{tick},{phase},{},{},{},{},{},{},{syaw},{},{:?},{},{},{},{},{},{tyaw},{:?},{},{},{},{},{hp},{}", sp.pose_id,sp.frame,sp.pose_instance_id,s.x,s.y,s.z,sb.duration,sm.attack_queued.as_ref().map(|q|q.next_pose_id),tp.pose_id,tp.frame,t.x,t.y,t.z,tm.current,tm.locked,tb.duration,tb.elapsed,hr.current.as_ref().map_or(-1.0,|r|r.minimum_remaining),world.resource::<LockOnController>().lock_entity.is_some()).unwrap();
                        if hp < hp_before {
                            hits.push((tick, sp.pose_id, tp.pose_id, s.distance(t), tm.current));
                        }
                        hp_before = hp;
                    }
                    eprintln!("spear fps={fps} contacts={contact} source={source_degrees} target={target_degrees} distance={distance} phase={phase} hits={hits:?}");
                    if hits.first().is_some_and(|hit| hit.1 == 285) {
                        assert_eq!(phase, 5, "the complete command sequence must be exercised after the first real hit");
                    }
                }
            }
        }
    }
    output("combo-timeline.csv", rows);
}

#[derive(Resource, Default, Debug)]
struct BeforeReaction {
    pose: u16,
    blend_duration: f32,
    blend_elapsed: f32,
}

fn sample_before_reaction(
    mut sample: ResMut<BeforeReaction>,
    fighter: Query<(&PoseRuntime, &EntryBlend), With<LocalFighter>>,
) {
    let (pose, blend) = fighter.single().unwrap();
    *sample = BeforeReaction {
        pose: pose.pose_id,
        blend_duration: if blend.pose_id == pose.pose_id {
            blend.duration
        } else {
            0.0
        },
        blend_elapsed: blend.elapsed,
    };
}

#[test]
#[ignore = "Known discrepancy audit: mode 4 must retain input through linked down blending"]
fn diagnose_spear_linked_down_blend() {
    let mut rows = String::from(
        "fps,reaction_pose,tick,pose,frame,state,blend,elapsed,minimum,pre_blend,pre_elapsed\n",
    );
    let mut early = Vec::new();
    for fps in [30, 60, 120] {
        for reaction_pose in [95, 97, 98, 99] {
            let mut h = spawn_gameplay_harness_with_order(7, true).unwrap();
            h.app.insert_resource(State::new(AppState::Playing));
            h.app.world_mut().entity_mut(h.fighter).insert((
                HeadTrackState::default(),
                VisualRootPin::default(),
                FighterStatusEffects::default(),
            ));
            spawn_fighter_model_for_test(h.app.world_mut(), h.fighter);
            drive_fighter_animation_for_test(h.app.world_mut());
            h.app.add_systems(
                FighterUpdate,
                drive_fighter_animation_for_test
                    .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
            );
            let mut pose = PoseRuntime::idle();
            h.app.init_resource::<BeforeReaction>().add_systems(
                FighterUpdate,
                sample_before_reaction
                    .after(sync_attacks_after_animation)
                    .before(tick_hit_reactions)
                    .in_set(crate::plugins::FighterFrameSet::PoseResolve),
            );
            let mut blend = EntryBlend::default();
            let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
            assert!(assets.poses[&287]
                .attacks
                .iter()
                .all(|a| a.target_value == 0.0));
            assets.apply_pose_with_blend(
                &mut pose,
                &mut blend,
                reaction_pose,
                true,
                None,
                EntryBlendKind::Generic,
                false,
                false,
                None,
            );
            let mut reaction = getup_reaction_for_pose(reaction_pose, pose.start, pose.end);
            reaction.kind = HitReactKind::BeHit;
            let mut runtime = FighterHitReaction::default();
            runtime.begin(reaction, false);
            h.app.world_mut().entity_mut(h.fighter).insert((
                pose,
                blend,
                runtime,
                SmState {
                    current: FighterState::BeHit,
                    locked: true,
                    ..default()
                },
                CombatState {
                    state: FighterState::BeHit,
                    ..default()
                },
            ));
            for tick in 0..fps * 2 {
                h.step_dt(1.0 / fps as f32);
                let world = h.app.world();
                let pose = world.get::<PoseRuntime>(h.fighter).unwrap();
                let blend = world.get::<EntryBlend>(h.fighter).unwrap();
                let reaction = world.get::<FighterHitReaction>(h.fighter).unwrap();
                let sample = world.resource::<BeforeReaction>();
                writeln!(
                    rows,
                    "{fps},{reaction_pose},{tick},{},{},{:?},{},{},{},{},{}",
                    pose.pose_id,
                    pose.frame,
                    h.state(),
                    blend.duration,
                    blend.elapsed,
                    reaction
                        .current
                        .as_ref()
                        .map_or(-1.0, |r| r.minimum_remaining),
                    sample.blend_duration,
                    sample.blend_elapsed
                )
                .unwrap();
                if h.state() == FighterState::Down {
                    if sample.pose == pose.pose_id && sample.blend_duration > 0.0 {
                        early.push((
                            fps,
                            reaction_pose,
                            tick,
                            sample.blend_duration,
                            sample.blend_elapsed,
                        ));
                    }

                    h.press_binding(crate::app_state::Keymap::default().guard);
                    h.step_dt(1.0 / fps as f32);
                    eprintln!("linked-down fps={fps} reaction={reaction_pose} down_tick={tick} after_guard={:?} pose={}",h.state(),h.pose_id());
                    break;
                }
            }
        }
    }
    output("linked-down-blend.csv", rows);
    assert!(
        early.is_empty(),
        "Down accepts input before its linked blend finishes: {early:?}"
    );
}
