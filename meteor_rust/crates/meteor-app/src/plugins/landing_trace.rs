use bevy::prelude::*;
use std::io::{BufWriter, Write};
use std::time::Instant;

use super::{
    fighter_frame::{ActiveFighter, FighterUpdate},
    movement_plugin::{FighterAir, FighterWall},
    stage_plugin::LocalFighter,
    FighterFrameSet,
};
use crate::components::fighter::{EntryBlend, Fighter, MotionState, PoseRuntime, SmState};

#[derive(Resource)]
struct LandingTrace {
    output: BufWriter<std::fs::File>,
    start: Instant,
    last_flush: Instant,
    rows: u64,
    done: bool,
}

impl LandingTrace {
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
            eprintln!("LANDING TRACE stopped: {error}");
        }
    }
}

pub(super) fn install(app: &mut App) {
    let Some(path) = std::env::var_os("METEOR_LANDING_TRACE") else {
        return;
    };
    let file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) => {
            eprintln!("LANDING TRACE cannot create {path:?}: {error}");
            return;
        }
    };
    let now = Instant::now();
    let mut trace = LandingTrace {
        output: BufWriter::new(file),
        start: now,
        last_flush: now,
        rows: 0,
        done: false,
    };
    trace.write(serde_json::json!({"kind":"capture","schemaVersion":1,"exe":std::env::current_exe().ok(),"pid":std::process::id(),"maxRows":180000,"maxSeconds":900,"units":"world, Y up; seconds; absolute animation frames"}));
    let _ = trace.output.flush();
    eprintln!("LANDING TRACE enabled: {path:?}");
    app.insert_resource(trace)
        .add_systems(FighterUpdate, before.before(FighterFrameSet::Movement))
        .add_systems(FighterUpdate, after.after(FighterFrameSet::PoseResolve));
}

fn record(world: &mut World, phase: &str) {
    if world.resource::<LandingTrace>().done {
        return;
    }
    let Some(entity) = world.get_resource::<ActiveFighter>().and_then(|a| a.0) else {
        return;
    };
    if world.get::<LocalFighter>(entity).is_none() {
        return;
    }
    let trace = world.resource::<LandingTrace>();
    let elapsed = trace.start.elapsed().as_secs_f64();
    if elapsed >= 900.0 || trace.rows >= 180000 {
        let mut trace = world.resource_mut::<LandingTrace>();
        let rows = trace.rows;
        trace.write(serde_json::json!({"kind":"capture-end","rows":rows,"seconds":elapsed,"reason":"limit"}));
        let _ = trace.output.flush();
        trace.done = true;
        eprintln!("LANDING TRACE limit reached; restart capture for another reproduction");
        return;
    }
    let pose = world.get::<PoseRuntime>(entity);
    let row = serde_json::json!({
        "kind":"sample","phase":phase,"seconds":elapsed,"entity":entity.to_bits(),
        "stage":world.get_resource::<crate::app_state::LoadedStageInfo>().map(|s| &s.stage_id),
        "fighter":world.get::<Fighter>(entity).map(|f| serde_json::json!({"character":f.character_id,"weapon":f.weapon})),
        "position":world.get::<Transform>(entity).map(|t| t.translation.to_array()),
        "state":world.get::<SmState>(entity).map(|s| format!("{:?}",s.current)),
        "pose":pose.map(|p| serde_json::json!({"id":p.pose_id,"frame":p.frame,"start":p.start,"end":p.end,"sequence":p.pose_instance_id,"loops":p.loops,"loopStart":p.loop_start,"loopEnd":p.loop_end,"fallHold":p.is_fall_hold()})),
        "blend":world.get::<EntryBlend>(entity).map(|b| serde_json::json!({"pose":b.pose_id,"active":pose.is_some_and(|p| b.is_active_for(p.pose_id)),"duration":b.duration,"elapsed":b.elapsed,"startedAt":b.started_at_seconds,"target":b.to_frame,"zeroPending":b.zero_duration_pending,"kind":format!("{:?}",b.kind)})),
        "motion":world.get::<MotionState>(entity).map(|m| serde_json::json!({"mode":format!("{:?}",m.movement_mode),"velocity":m.velocity.to_array(),"facing":m.facing})),
        "air":world.get::<FighterAir>(entity).map(|a| serde_json::json!({"airborne":a.air.airborne,"time":a.air.air_time,"verticalSpeed":a.air.vertical_speed,"mode":format!("{:?}",a.air.motion_mode),"groundY":a.ground_y,"groundContact":a.ground_contact,"noSupport":a.carry_no_support,"steep":a.carry_decay_steep,"groundBlendPending":a.ground_entry_blend_pending,"ledgeFall":a.ledge_fall,"pendingLedgeFall":a.pending_ledge_fall,"launchPose":a.air.launch_pose_id,"launchLifted":a.air.launch_lifted,"lateVisualSnap":a.late_visual_pose_snap})),
        "wall":world.get::<FighterWall>(entity).map(|w| serde_json::json!({"active":w.cling_active,"pose":w.cling_pose,"normal":w.wall_normal.to_array(),"contact":w.contact_seen})),
        "clock":world.get_resource::<super::FrameClock>().map(|c| serde_json::json!({"simDt":c.sim_dt,"wallDt":c.wall_dt,"wallStep":c.wall_step})),
        "realTime":world.get_resource::<Time<bevy::time::Real>>().map(|t| t.elapsed_secs_f64()),
        "keys":world.get_resource::<ButtonInput<KeyCode>>().map(|k| k.get_pressed().map(|k| format!("{k:?}")).collect::<Vec<_>>()),
        "mouse":world.get_resource::<ButtonInput<MouseButton>>().map(|k| k.get_pressed().map(|k| format!("{k:?}")).collect::<Vec<_>>()),
    });
    world.resource_mut::<LandingTrace>().write(row);
}
fn before(world: &mut World) {
    record(world, "before-movement");
}
fn after(world: &mut World) {
    record(world, "after-pose-resolve");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_records_landing_and_ignores_other_actors() {
        let path = std::env::temp_dir().join(format!(
            "landing-trace-{}-{}.jsonl",
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
        let mut world = World::new();
        let now = Instant::now();
        world.insert_resource(LandingTrace {
            output: BufWriter::new(file),
            start: now,
            last_flush: now,
            rows: 0,
            done: false,
        });
        let entity = world
            .spawn((
                LocalFighter,
                FighterAir::default(),
                PoseRuntime::default(),
                EntryBlend::default(),
            ))
            .id();
        world.insert_resource(ActiveFighter(Some(entity)));
        world.get_mut::<FighterAir>(entity).unwrap().air.airborne = true;
        before(&mut world);
        world.get_mut::<FighterAir>(entity).unwrap().air.airborne = false;
        world
            .get_mut::<FighterAir>(entity)
            .unwrap()
            .ground_entry_blend_pending = true;
        after(&mut world);
        let npc = world.spawn_empty().id();
        world.resource_mut::<ActiveFighter>().0 = Some(npc);
        after(&mut world);
        assert_eq!(world.resource::<LandingTrace>().rows, 2);
        world.resource_mut::<LandingTrace>().output.flush().unwrap();
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(rows[0]["air"]["airborne"], true);
        assert_eq!(rows[1]["air"]["airborne"], false);
        assert_eq!(rows[1]["air"]["groundBlendPending"], true);
        drop(world);
        std::fs::remove_file(path).unwrap();
    }
}
