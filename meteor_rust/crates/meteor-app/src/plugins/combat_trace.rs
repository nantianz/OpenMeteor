use super::{
    fighter_frame::{ActiveFighter, FighterUpdate},
    FighterFrameSet,
};
use crate::components::fighter::{
    EntryBlend, Fighter, FighterHitReaction, Health, LockTarget, MotionState, PoseRuntime, SmState,
};
use bevy::prelude::*;
use std::io::{BufWriter, Write};
use std::time::Instant;

#[derive(Resource)]
pub(super) struct CombatTrace {
    output: BufWriter<std::fs::File>,
    start: Option<Instant>,
    done: bool,
    rows: u64,
}

impl CombatTrace {
    pub(super) fn event(&mut self, mut row: serde_json::Value) {
        let Some(start) = self.start else {
            return;
        };
        if self.done || start.elapsed().as_secs_f64() >= 120.0 || self.rows >= 250_000 {
            return;
        }
        row["qpcMs"] = serde_json::json!(qpc_ms());
        row["ms"] = serde_json::json!(start.elapsed().as_secs_f64() * 1000.0);
        if let Err(error) = writeln!(self.output, "{row}") {
            self.done = true;
            eprintln!("COMBAT TRACE event write failed: {error}");
        }
        self.rows += 1;
    }
}

#[cfg(windows)]
fn qpc_ms() -> f64 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn QueryPerformanceCounter(value: *mut i64) -> i32;
        fn QueryPerformanceFrequency(value: *mut i64) -> i32;
    }
    let (mut value, mut frequency) = (0, 0);
    unsafe {
        QueryPerformanceCounter(&mut value);
        QueryPerformanceFrequency(&mut frequency);
    }
    value as f64 * 1000.0 / frequency as f64
}
#[cfg(not(windows))]
fn qpc_ms() -> f64 {
    0.0
}

pub(super) fn install(app: &mut App) {
    let Some(path) = std::env::var_os("METEOR_COMBAT_TRACE") else {
        return;
    };
    let file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) => {
            eprintln!("COMBAT TRACE cannot create {path:?}: {error}");
            return;
        }
    };
    let mut trace = CombatTrace {
        output: BufWriter::new(file),
        start: None,
        done: false,
        rows: 0,
    };
    let _ = writeln!(
        trace.output,
        "{}",
        serde_json::json!({"kind":"capture", "schemaVersion":2,"maxRows":250000, "pid":std::process::id(), "clock":"QueryPerformanceCounter", "units":"world (source * 0.12), Y up", "seconds":120, "trigger":"first pose 417 or 419", "exe":std::env::current_exe().ok()})
    );
    let _ = trace.output.flush();
    app.insert_resource(trace)
        .add_systems(
            FighterUpdate,
            before_hit
                .after(FighterFrameSet::SkeletonSample)
                .before(FighterFrameSet::HitJudge),
        )
        .add_systems(
            FighterUpdate,
            after_hit
                .after(FighterFrameSet::HitJudge)
                .before(FighterFrameSet::RootMotion),
        )
        .add_systems(
            FighterUpdate,
            before_contact
                .after(FighterFrameSet::RootMotion)
                .after(super::character_contact_plugin::apply_authored_repulsion)
                .before(super::character_contact_plugin::finish_character_contact_frame),
        )
        .add_systems(
            FighterUpdate,
            after_contact
                .after(super::character_contact_plugin::finish_character_contact_frame)
                .before(FighterFrameSet::AttackFacing),
        );
    eprintln!("COMBAT TRACE armed: {path:?}; records 120s after first 417/419");
}

fn record(world: &mut World, stage: &str) {
    if world.resource::<CombatTrace>().done {
        return;
    }
    let triggered = world
        .query::<&PoseRuntime>()
        .iter(world)
        .any(|pose| matches!(pose.pose_id, 417 | 419));
    if world.resource::<CombatTrace>().start.is_none() {
        if !triggered {
            return;
        }
        world.resource_mut::<CombatTrace>().start = Some(Instant::now());
    }
    let elapsed = world
        .resource::<CombatTrace>()
        .start
        .unwrap()
        .elapsed()
        .as_secs_f64();
    if elapsed >= 120.0 || world.resource::<CombatTrace>().rows >= 250_000 {
        let mut trace = world.resource_mut::<CombatTrace>();
        let row = serde_json::json!({"kind":"capture-end", "rows":trace.rows, "qpcMs":qpc_ms(), "limitReached":trace.rows >= 250_000});
        let result = writeln!(trace.output, "{row}").and_then(|_| trace.output.flush());
        if let Err(error) = result {
            eprintln!("COMBAT TRACE write failed: {error}");
        }
        trace.done = true;
        return;
    }
    let active = world
        .get_resource::<ActiveFighter>()
        .and_then(|active| active.0)
        .map(|entity| entity.to_bits());
    let entities: Vec<_> = world
        .query_filtered::<Entity, With<Fighter>>()
        .iter(world)
        .collect();
    let mut pairs = Vec::new();
    for (index, a) in entities.iter().enumerate() {
        for b in &entities[index + 1..] {
            if let (Some(ta), Some(tb)) = (world.get::<Transform>(*a), world.get::<Transform>(*b)) {
                let delta = tb.translation - ta.translation;
                pairs.push(serde_json::json!({"a":a.to_bits(),"b":b.to_bits(),"bMinusA":delta.to_array(),"planarDistance":delta.xz().length(),"heightDifference":delta.y}));
            }
        }
    }
    let actors: Vec<_> = entities.into_iter().map(|entity| {
        let fighter = world.get::<Fighter>(entity).unwrap();
        let pose = world.get::<PoseRuntime>(entity);
        let transform = world.get::<Transform>(entity);
        let motion = world.get::<MotionState>(entity);
        serde_json::json!({
            "entity":entity.to_bits(), "character":fighter.character_id, "weapon":fighter.weapon,
            "local":world.get::<super::stage_plugin::LocalFighter>(entity).is_some(),
            "position":transform.map(|t| t.translation.to_array()),
            "pose":pose.map(|p| p.pose_id), "frame":pose.map(|p| p.frame),
            "sequence":pose.map(|p| p.pose_instance_id),
            "velocity":motion.map(|m| m.velocity.to_array()), "yaw":motion.map(|m| m.facing),
            "movement":motion.map(|m| format!("{:?}",m.movement_mode)),
            "hp":world.get::<Health>(entity).map(|h| h.current),
            "blend":world.get::<EntryBlend>(entity).map(|b| serde_json::json!({"duration":b.duration,"elapsed":b.elapsed,"target":b.to_frame,"kind":format!("{:?}",b.kind)})),
            "state":world.get::<SmState>(entity).map(|s| format!("{s:?}")),
            "reaction":world.get::<FighterHitReaction>(entity).map(|s| format!("{s:?}")),
            "air":world.get::<super::movement_plugin::FighterAir>(entity).map(|s| serde_json::json!({"air":format!("{:?}", s.air), "groundContact":s.ground_contact,"externalVelocity":s.ground_external_velocity.to_array(),"steep":s.carry_decay_steep,"noSupport":s.carry_no_support})),
            "lock":world.get::<LockTarget>(entity).map(|s| format!("{s:?}")),
            "remoteTarget":world.get::<super::network_plugin::RemoteTransformTarget>(entity).map(|s| serde_json::json!({"position":s.translation.to_array(),"yaw":s.yaw})),
            "remoteContact":world.get::<super::network_plugin::RemoteContactSegment>(entity).map(|s| serde_json::json!({"from":s.from.to_array(),"to":s.to.to_array(),"valid":s.valid,"roleDriven":s.role_driven,"stateAge":s.state_age,"hitAge":s.hit_age})),
            "authority":world.get::<super::network_plugin::RemoteAuthorityGeneration>(entity).map(|s| serde_json::json!({"roleTick":s.role_state_tick,"hitTick":s.last_hit_tick})),
            "remoteHitMotionRemaining":world.get::<super::network_plugin::RemoteHitMotionWindow>(entity).map(|s| s.remaining),
        })
    }).collect();
    let row = serde_json::json!({"kind":"actors", "stage":stage, "qpcMs":qpc_ms(), "ms":elapsed*1000.0, "active":active, "actors":actors, "pairs":pairs,
        "keys":world.get_resource::<ButtonInput<KeyCode>>().map(|keys| serde_json::json!({"held":keys.get_pressed().map(|key| format!("{key:?}")).collect::<Vec<_>>(),"pressed":keys.get_just_pressed().map(|key| format!("{key:?}")).collect::<Vec<_>>(),"released":keys.get_just_released().map(|key| format!("{key:?}")).collect::<Vec<_>>()})),
        "mouse":world.get_resource::<ButtonInput<MouseButton>>().map(|keys| serde_json::json!({"held":keys.get_pressed().map(|key| format!("{key:?}")).collect::<Vec<_>>(),"pressed":keys.get_just_pressed().map(|key| format!("{key:?}")).collect::<Vec<_>>()}))});
    let mut trace = world.resource_mut::<CombatTrace>();
    if let Err(error) = writeln!(trace.output, "{row}") {
        trace.done = true;
        eprintln!("COMBAT TRACE stopped after write error: {error}");
    }
    trace.rows += 1;
    if trace.rows % 120 == 0 {
        if let Err(error) = trace.output.flush() {
            trace.done = true;
            eprintln!("COMBAT TRACE flush failed: {error}");
        }
    }
}
fn before_hit(world: &mut World) {
    record(world, "before-hit");
}
fn after_hit(world: &mut World) {
    record(world, "after-hit");
}
fn before_contact(world: &mut World) {
    record(world, "before-contact");
}
fn after_contact(world: &mut World) {
    record(world, "after-contact");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trace_keeps_enemy_state_and_writes_a_bounded_end_marker() {
        let path = std::env::temp_dir().join(format!(
            "meteor-combat-trace-{}-{}.jsonl",
            std::process::id(),
            qpc_ms()
        ));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let mut world = World::new();
        world.insert_resource(CombatTrace {
            output: BufWriter::new(file),
            start: None,
            done: false,
            rows: 0,
        });
        for (slot, pose, x) in [(0, 417, 0.0), (1, 100, 1.0)] {
            world.spawn((
                Fighter {
                    character_id: 4,
                    player_slot: slot,
                    weapon: 9,
                    weapon_slots: [9, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                PoseRuntime {
                    pose_id: pose,
                    ..Default::default()
                },
                Transform::from_xyz(x, 0.0, 0.0),
                Health::new(),
            ));
        }
        record(&mut world, "before-contact");
        world
            .resource_mut::<CombatTrace>()
            .event(serde_json::json!({"kind":"test-network-event","position":[1,2,3]}));
        world.resource_mut::<CombatTrace>().start =
            Some(Instant::now() - std::time::Duration::from_secs(121));
        record(&mut world, "after-contact");
        assert!(world.resource::<CombatTrace>().done);
        let text = std::fs::read_to_string(&path).unwrap();
        let rows: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows[0]["actors"].as_array().unwrap().len(), 2);
        assert!(rows[0]["actors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|actor| actor["pose"] == 100));
        assert_eq!(rows[0]["pairs"][0]["planarDistance"], 1.0);
        assert_eq!(rows[1]["kind"], "test-network-event");
        assert!(rows[1]["qpcMs"].is_number());
        assert_eq!(rows[2]["kind"], "capture-end");
        assert_eq!(rows[2]["rows"], 2);
        drop(world);
        std::fs::remove_file(path).unwrap();
    }
}
