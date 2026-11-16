use bevy::prelude::*;
use std::io::{BufWriter, Write};
use std::time::Instant;

use super::character_plugin::{FighterBone, FighterDummy, FighterModel};
use super::fighter_frame::{ActiveFighter, FighterUpdate};
use super::movement_plugin::{FighterAir, FighterWall};
use super::stage_plugin::LocalFighter;
use super::FighterFrameSet;
use crate::components::fighter::{
    EntryBlend, Fighter, MotionState, PoseRuntime, SmState, VisualRootPin,
};

const MAX_ROWS: u64 = 180_000;
const MAX_SECONDS: f64 = 900.0;

#[derive(Resource)]
struct JumpTrace {
    output: BufWriter<std::fs::File>,
    start: Instant,
    last_flush: Instant,
    rows: u64,
    tick: u64,
    done: bool,
}

impl JumpTrace {
    fn write(&mut self, row: serde_json::Value) {
        if self.done {
            return;
        }
        let result = writeln!(self.output, "{row}");
        self.rows += 1;
        let result = result.and_then(|_| {
            if self.last_flush.elapsed().as_secs_f32() >= 0.5 {
                self.last_flush = Instant::now();
                self.output.flush()
            } else {
                Ok(())
            }
        });
        if let Err(error) = result {
            self.done = true;
            eprintln!("JUMP TRACE stopped: {error}");
        }
    }

    fn finish(&mut self, reason: &str) {
        if self.done {
            return;
        }
        self.write(serde_json::json!({"kind":"capture-end","reason":reason,"rows":self.rows,"seconds":self.start.elapsed().as_secs_f64()}));
        let _ = self.output.flush();
        self.done = true;
        eprintln!("JUMP TRACE ended: {reason}");
    }
}

pub(super) fn install(app: &mut App) {
    let Some(path) = std::env::var_os("METEOR_JUMP_TRACE") else {
        return;
    };
    let file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) => {
            eprintln!("JUMP TRACE cannot create {path:?}: {error}");
            return;
        }
    };
    let now = Instant::now();
    let mut trace = JumpTrace {
        output: BufWriter::new(file),
        start: now,
        last_flush: now,
        rows: 0,
        tick: 0,
        done: false,
    };
    trace.write(serde_json::json!({"kind":"capture","schemaVersion":1,"exe":std::env::current_exe().ok(),"pid":std::process::id(),"maxRows":MAX_ROWS,"maxSeconds":MAX_SECONDS,"units":"actor/global: world Y-up; bone/dummy local and root pins: model Z-up; seconds; absolute animation frames","markerKey":"F8","nonFinite":"JSON null plus finite=false"}));
    let _ = trace.output.flush();
    eprintln!("JUMP TRACE enabled: {path:?}; press F8 after the symptom to mark it");
    app.insert_resource(trace);
    install_systems(app);
}

fn install_systems(app: &mut App) {
    app.add_systems(FighterUpdate, before.before(FighterFrameSet::Movement))
        .add_systems(
            FighterUpdate,
            moved
                .after(FighterFrameSet::Movement)
                .before(FighterFrameSet::PoseDecision),
        )
        .add_systems(
            FighterUpdate,
            root_before
                .after(FighterFrameSet::HitJudge)
                .before(FighterFrameSet::RootMotion),
        )
        .add_systems(FighterUpdate, resolved.after(FighterFrameSet::PoseResolve))
        .add_systems(
            PostUpdate,
            rendered.after(bevy::transform::TransformSystems::Propagate),
        )
        .add_systems(Last, finish_on_exit);
}

fn transform_row(world: &World, entity: Entity, global: bool) -> serde_json::Value {
    let local = world.get::<Transform>(entity).map(|t| serde_json::json!({
        "position":t.translation.to_array(),"rotation":t.rotation.to_array(),"scale":t.scale.to_array(),
        "finite":t.translation.is_finite() && t.rotation.is_finite() && t.scale.is_finite()
    }));
    let propagated = global.then(|| world.get::<GlobalTransform>(entity)).flatten().map(|t| {
        let matrix = t.to_matrix();
        serde_json::json!({"position":t.translation().to_array(),"matrix":matrix.to_cols_array(),"finite":matrix.is_finite()})
    });
    serde_json::json!({"entity":entity.to_bits(),"local":local,"global":propagated})
}

fn air_row(a: &FighterAir) -> serde_json::Value {
    let mut row = serde_json::json!({
        "airborne":a.air.airborne,"time":a.air.air_time,"verticalSpeed":a.air.vertical_speed,"planar":[a.air.planar_x,a.air.planar_z],"mode":format!("{:?}",a.air.motion_mode),
        "groundY":a.ground_y,"groundContact":a.ground_contact,"verticalContact":a.mover_vertical_contact,"noSupport":a.carry_no_support,"steep":a.carry_decay_steep,"external":a.ground_external_velocity.to_array(),
        "groundBlendPending":a.ground_entry_blend_pending,"recoveryMode":a.recovery_ground_mode,"ledgeFall":a.ledge_fall,"pendingLedgeFall":a.pending_ledge_fall,"jumpHold":a.jump_hold,"jumpCharge":a.jump_charge_active
    });
    row["launch"] = serde_json::json!({"clingArmed":a.air.cling_armed,"pose":a.air.launch_pose_id,"lifted":a.air.launch_lifted,"verticalPose":a.air.vertical_pose_id,"prevRootZ":a.air.prev_root_z,"peakLift":a.peak_lift});
    row["root"] = serde_json::json!({"pin":a.motion_root_pin,"pinPose":a.motion_root_pin_pose_id,"lateVisualSnap":a.late_visual_pose_snap,"jumpStarted":a.jump_root_xy_started,"jumpPrevPose":a.jump_prev_pose_id,"jumpPrev":[a.jump_prev_root_x,a.jump_prev_root_y],"stand":[a.stand_motion_root_x,a.stand_motion_root_y],"pinZ":a.pin_root_z});
    row
}

fn record(world: &mut World, entity: Entity, phase: &str) {
    if world.resource::<JumpTrace>().done {
        return;
    }
    if world.get::<LocalFighter>(entity).is_none() {
        return;
    }
    let elapsed = world.resource::<JumpTrace>().start.elapsed().as_secs_f64();
    if elapsed >= MAX_SECONDS || world.resource::<JumpTrace>().rows >= MAX_ROWS {
        world.resource_mut::<JumpTrace>().finish("limit");
        return;
    }
    if phase == "before-movement" {
        world.resource_mut::<JumpTrace>().tick += 1;
    }
    let pose = world.get::<PoseRuntime>(entity);
    let visual = phase == "after-transform-propagation";
    let marker = visual
        && world
            .get_resource::<ButtonInput<KeyCode>>()
            .is_some_and(|k| k.just_pressed(KeyCode::F8));
    let mut row = serde_json::json!({
        "kind":"sample","phase":phase,"seconds":elapsed,"tick":world.resource::<JumpTrace>().tick,
        "frame":world.get_resource::<bevy::diagnostic::FrameCount>().map(|f| f.0),
        "stage":world.get_resource::<crate::app_state::LoadedStageInfo>().map(|s| &s.stage_id),
        "appState":world.get_resource::<State<crate::app_state::AppState>>().map(|s| format!("{:?}",s.get())),
        "fighter":world.get::<Fighter>(entity).map(|f| serde_json::json!({"character":f.character_id,"weapon":f.weapon})),
        "actor":transform_row(world,entity,visual),
        "state":world.get::<SmState>(entity).map(|s| format!("{:?}",s.current)),
        "pose":pose.map(|p| serde_json::json!({"id":p.pose_id,"frame":p.frame,"start":p.start,"end":p.end,"sequence":p.pose_instance_id,"loops":p.loops,"fallHold":p.is_fall_hold()})),
        "blend":world.get::<EntryBlend>(entity).map(|b| serde_json::json!({"pose":b.pose_id,"active":pose.is_some_and(|p| b.is_active_for(p.pose_id)),"duration":b.duration,"elapsed":b.elapsed,"target":b.to_frame,"fromRoot":b.from_root,"zeroPending":b.zero_duration_pending,"preserveSample":b.preserve_sample_once,"kind":format!("{:?}",b.kind)})),
        "motion":world.get::<MotionState>(entity).map(|m| serde_json::json!({"mode":format!("{:?}",m.movement_mode),"velocity":m.velocity.to_array(),"facing":m.facing,"finite":m.velocity.is_finite() && m.facing.is_finite()})),
        "air":world.get::<FighterAir>(entity).map(air_row),
        "visualPin":world.get::<VisualRootPin>(entity).map(|p| serde_json::json!({"active":p.active,"pose":p.pose_id,"baseline":p.baseline,"xy":p.lock_xy,"z":p.lock_z,"release":p.release_after_sample})),
        "wall":world.get::<FighterWall>(entity).map(|w| serde_json::json!({"active":w.cling_active,"pose":w.cling_pose,"kickPose":w.kick_pose,"normal":w.wall_normal.to_array(),"contact":w.contact_seen,"remaining":w.remaining,"external":w.external_accel.to_array()})),
        "clock":world.get_resource::<super::FrameClock>().map(|c| serde_json::json!({"simDt":c.sim_dt,"wallDt":c.wall_dt,"wallStep":c.wall_step})),
        "realTime":world.get_resource::<Time<bevy::time::Real>>().map(|t| t.elapsed_secs_f64()),
        "keys":world.get_resource::<ButtonInput<KeyCode>>().map(|k| k.get_pressed().map(|k| format!("{k:?}")).collect::<Vec<_>>()),
        "mouse":world.get_resource::<ButtonInput<MouseButton>>().map(|k| k.get_pressed().map(|k| format!("{k:?}")).collect::<Vec<_>>()),
        "marker":marker
    });
    row["jumpInput"] = world.get_resource::<super::input_plugin::LocalJumpInput>().map(|j| {
        serde_json::json!({"pressed":j.pressed,"released":j.released,"accepted":j.action_accepted,"held":j.held,"holdSeconds":j.hold_seconds})
    }).into();

    let models: Vec<Entity> = world
        .query::<(Entity, &FighterModel)>()
        .iter(world)
        .filter_map(|(e, m)| (m.fighter == entity).then_some(e))
        .collect();
    let mut nodes = Vec::new();
    for model in &models {
        let bones: Vec<(Entity, String, usize)> = world
            .query::<(Entity, &FighterBone)>()
            .iter(world)
            .filter(|(_, b)| b.model == *model && (visual || b.skin_index == 0))
            .map(|(e, b)| (e, b.name.clone(), b.skin_index))
            .collect();
        let dummies: Vec<(Entity, String)> = world
            .query::<(Entity, &FighterDummy)>()
            .iter(world)
            .filter(|(_, d)| d.model == *model && d.name.eq_ignore_ascii_case("d_base"))
            .map(|(e, d)| (e, d.name.clone()))
            .collect();
        for (bone, name, index) in bones {
            nodes.push(serde_json::json!({"kind":"bone","name":name,"skinIndex":index,"model":model.to_bits(),"transform":transform_row(world,bone,visual)}));
        }
        for (dummy, name) in dummies {
            nodes.push(serde_json::json!({"kind":"dummy","name":name,"model":model.to_bits(),"transform":transform_row(world,dummy,visual)}));
        }
    }
    row["models"] = models
        .iter()
        .map(|e| transform_row(world, *e, visual))
        .collect();
    row["nodes"] = nodes.into();
    if visual {
        let cameras: Vec<Entity> = world
            .query_filtered::<Entity, With<super::camera_plugin::GameplayCamera>>()
            .iter(world)
            .collect();
        row["cameras"] = cameras
            .iter()
            .map(|e| {
                serde_json::json!({
                    "transform":transform_row(world,*e,true),
                    "projection":world.get::<Projection>(*e).map(|p| format!("{p:?}")),
                    "active":world.get::<Camera>(*e).map(|c| c.is_active)
                })
            })
            .collect();

        if let (Some(position), Some(stage)) = (
            world.get::<Transform>(entity),
            world.get_resource::<super::stage_collider::StageCollider>(),
        ) {
            if let Some(mesh) = &stage.mesh {
                use meteor_physics::collision::MeshCollider;
                let p = position.translation;
                if p.is_finite() {
                    row["groundProbe"] = serde_json::json!({"position":p.to_array(),"hit":mesh.sample_ground_hit(p.x,p.z,0.0,p.y).map(|h| serde_json::json!({"y":h.y,"normal":[h.nx,h.ny,h.nz]}))});
                }
            }
        }
    }
    let mut trace = world.resource_mut::<JumpTrace>();
    trace.write(row);
    if marker {
        let _ = trace.output.flush();
        eprintln!(
            "JUMP TRACE F8 marker: tick={} seconds={elapsed:.3}",
            trace.tick
        );
    }
}

fn actor_phase(world: &mut World, phase: &str) {
    if let Some(entity) = world.get_resource::<ActiveFighter>().and_then(|a| a.0) {
        record(world, entity, phase);
    }
}
fn before(world: &mut World) {
    actor_phase(world, "before-movement");
}
fn moved(world: &mut World) {
    actor_phase(world, "after-movement");
}
fn root_before(world: &mut World) {
    actor_phase(world, "before-root-motion");
}
fn resolved(world: &mut World) {
    actor_phase(world, "after-pose-resolve");
}
fn rendered(world: &mut World) {
    if world.resource::<JumpTrace>().done {
        return;
    }
    let actors: Vec<Entity> = world
        .query_filtered::<Entity, With<LocalFighter>>()
        .iter(world)
        .collect();
    for entity in actors {
        record(world, entity, "after-transform-propagation");
    }
}
fn finish_on_exit(mut exit: MessageReader<AppExit>, mut trace: ResMut<JumpTrace>) {
    if exit.read().next().is_some() {
        trace.finish("app-exit");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_capture_brackets_movers_and_reads_propagated_hierarchy() {
        let path = std::env::temp_dir().join(format!(
            "jump-schedule-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let now = Instant::now();
        let mut app = App::new();
        app.add_plugins(bevy::transform::TransformPlugin)
            .init_schedule(FighterUpdate)
            .configure_sets(
                FighterUpdate,
                (
                    FighterFrameSet::Movement,
                    FighterFrameSet::PoseDecision,
                    FighterFrameSet::Locomotion,
                    FighterFrameSet::PoseAdvance,
                    FighterFrameSet::SkeletonSample,
                    FighterFrameSet::HitJudge,
                    FighterFrameSet::RootMotion,
                    FighterFrameSet::AttackFacing,
                    FighterFrameSet::PoseResolve,
                )
                    .chain(),
            )
            .insert_resource(JumpTrace {
                output: BufWriter::new(file),
                start: now,
                last_flush: now,
                rows: 0,
                tick: 0,
                done: false,
            });
        install_systems(&mut app);
        let actor = app
            .world_mut()
            .spawn((LocalFighter, Transform::default()))
            .id();
        let model = app
            .world_mut()
            .spawn((
                FighterModel {
                    fighter: actor,
                    character: "p0".into(),
                },
                Transform::default(),
                ChildOf(actor),
            ))
            .id();
        let dummy = app
            .world_mut()
            .spawn((
                FighterDummy {
                    model,
                    name: "d_base".into(),
                    dummy_index: 0,
                },
                Transform::default(),
                ChildOf(model),
            ))
            .id();
        app.insert_resource(ActiveFighter(Some(actor)))
            .add_systems(
                FighterUpdate,
                (move |world: &mut World| {
                    world.get_mut::<Transform>(actor).unwrap().translation.y = 10.0;
                })
                .in_set(FighterFrameSet::Movement),
            )
            .add_systems(
                FighterUpdate,
                (move |world: &mut World| {
                    world.get_mut::<Transform>(actor).unwrap().translation.y += 20.0;
                    world.get_mut::<Transform>(dummy).unwrap().translation.z = 3.0;
                })
                .in_set(FighterFrameSet::RootMotion),
            );
        app.world_mut().run_schedule(FighterUpdate);
        app.update();
        app.world_mut().resource_mut::<JumpTrace>().finish("test");
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            rows[..5]
                .iter()
                .map(|r| r["phase"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "before-movement",
                "after-movement",
                "before-root-motion",
                "after-pose-resolve",
                "after-transform-propagation"
            ]
        );
        assert_eq!(rows[0]["actor"]["local"]["position"][1], 0.0);
        assert_eq!(rows[1]["actor"]["local"]["position"][1], 10.0);
        assert_eq!(rows[2]["actor"]["local"]["position"][1], 10.0);
        assert_eq!(rows[3]["actor"]["local"]["position"][1], 30.0);
        assert_eq!(
            rows[4]["nodes"][0]["transform"]["global"]["position"],
            serde_json::json!([0.0, 30.0, 3.0])
        );
        drop(app);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn capture_locates_actor_and_visual_displacements_in_the_same_tick() {
        let path = std::env::temp_dir().join(format!(
            "jump-trace-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let now = Instant::now();
        let mut world = World::new();
        world.insert_resource(JumpTrace {
            output: BufWriter::new(file),
            start: now,
            last_flush: now,
            rows: 0,
            tick: 0,
            done: false,
        });
        let actor = world
            .spawn((
                LocalFighter,
                Transform::default(),
                GlobalTransform::default(),
                FighterAir::default(),
            ))
            .id();
        let model = world
            .spawn((
                FighterModel {
                    fighter: actor,
                    character: "p0".into(),
                },
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        let dummy = world
            .spawn((
                FighterDummy {
                    model,
                    name: "d_base".into(),
                    dummy_index: 0,
                },
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        world.insert_resource(ActiveFighter(Some(actor)));
        before(&mut world);
        world.get_mut::<Transform>(actor).unwrap().translation.y = 10.0;
        moved(&mut world);
        root_before(&mut world);
        world.get_mut::<Transform>(dummy).unwrap().translation.z = 1000.0;
        resolved(&mut world);
        world.get_mut::<GlobalTransform>(dummy).unwrap().clone_from(
            &GlobalTransform::from_translation(Vec3::new(0.0, 1010.0, 0.0)),
        );
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::F8);
        world.insert_resource(keys);
        rendered(&mut world);
        let npc = world.spawn(Transform::default()).id();
        world.resource_mut::<ActiveFighter>().0 = Some(npc);
        before(&mut world);
        world.resource_mut::<JumpTrace>().finish("test");
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(rows.len(), 6);
        assert!(rows[..5].iter().all(|r| r["tick"] == 1));
        assert_eq!(rows[0]["actor"]["local"]["position"][1], 0.0);
        assert_eq!(rows[1]["actor"]["local"]["position"][1], 10.0);
        assert_eq!(
            rows[3]["nodes"][0]["transform"]["local"]["position"][2],
            1000.0
        );
        assert!(rows[3]["nodes"][0]["transform"]["global"].is_null());
        assert_eq!(
            rows[4]["nodes"][0]["transform"]["global"]["position"][1],
            1010.0
        );
        assert_eq!(rows[4]["marker"], true);
        assert_eq!(world.get::<Transform>(actor).unwrap().translation.y, 10.0);
        world.get_mut::<Transform>(dummy).unwrap().translation.x = f32::NAN;
        assert_eq!(
            transform_row(&world, dummy, false)["local"]["finite"],
            false
        );
        drop(world);
        std::fs::remove_file(path).unwrap();
    }
}
