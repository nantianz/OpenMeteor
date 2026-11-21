use super::*;
use crate::app_state::{LaunchRequest, Settings};
use crate::components::fighter::{
    EntryBlend, Fighter, FighterHitReaction, Health, LockOnController, PoseRuntime, Qi, SmState,
};
use crate::plugins::character_plugin::CharacterAssetsSet;
use crate::plugins::combat_plugin::sampled_attack_window;
use crate::plugins::network_plugin::RemoteTransformTarget;
use crate::plugins::{fighter_frame, FighterFrameSet, FrameClock};
use bevy::ecs::message::MessageCursor;
use serde_json::{json, Value};
use std::io::{BufWriter, Write};

const MAX_ROWS: u32 = 300_000;
const DEFAULT_WINDOW: (f32, f32) = (0.0, 120.0);

const MAX_WINDOW_SECONDS: f32 = 180.0;

#[derive(Resource)]
pub(super) struct Trace {
    output: BufWriter<std::fs::File>,
    window: (f32, f32),
    last_pump: Option<f32>,
    cursors: [usize; 5],
    sought: bool,
    rows: u32,
    done: bool,
}

impl Trace {
    fn write(&mut self, row: Value) {
        if self.done {
            return;
        }
        let result = writeln!(self.output, "{row}");
        self.rows += 1;
        let capped = self.rows >= MAX_ROWS;
        let result = result.and_then(|()| {
            if capped {
                writeln!(
                    self.output,
                    "{}",
                    json!({"kind":"capture-end","reason":"row-limit"})
                )?;
            }
            if capped || self.rows % 120 == 0 {
                self.output.flush()?;
            }
            Ok(())
        });
        if let Err(error) = result {
            eprintln!("MRC TRACE write failed: {error}");
            self.done = true;
        } else if capped {
            self.done = true;
        }
    }

    pub(super) fn tracks(&self, time: f32) -> bool {
        !self.done && (self.window.0..=self.window.1).contains(&time)
    }

    pub(super) fn hit(
        &mut self,
        replay_time: f32,
        event_time: f32,
        guarded: bool,
        event: &meteor_network::stock_protocol::StockHitReactionEvent,
        outcome: &str,
        poses: Option<(u16, u16)>,
    ) {
        if self.tracks(replay_time) {
            self.write(json!({"kind":"hit-packet", "replayTime":replay_time, "eventTime":event_time,
                "actorId":event.target_actor_id, "guarded":guarded, "context":event.context,
                "poseWord":event.pose_id, "moveHundred":event.move_hundred, "valueHundred":event.value_hundred,
                "outcome":outcome, "reactionPose":poses.map(|p| p.0), "feedbackPose":poses.map(|p| p.1)}));
        }
    }
}

fn parse_window(value: &str) -> Option<(f32, f32)> {
    let (start, end) = value.split_once(',')?;
    let start = start.parse::<f32>().ok()?;
    let end = end.parse::<f32>().ok()?;
    (start.is_finite()
        && end.is_finite()
        && 0.0 <= start
        && start < end
        && end - start <= MAX_WINDOW_SECONDS)
        .then_some((start, end))
}

pub(super) fn install(app: &mut App) {
    let Some(path) = std::env::var_os("METEOR_MRC_TRACE") else {
        return;
    };
    let window = match std::env::var("METEOR_MRC_TRACE_WINDOW") {
        Ok(value) => match parse_window(&value) {
            Some(window) => window,
            None => {
                eprintln!("MRC TRACE expects METEOR_MRC_TRACE_WINDOW=FROM,TO (up to 180 seconds)");
                return;
            }
        },
        Err(_) => DEFAULT_WINDOW,
    };
    let file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) => {
            eprintln!("MRC TRACE cannot create {path:?}: {error}");
            return;
        }
    };
    let mut trace = Trace {
        output: BufWriter::new(file),
        window,
        last_pump: None,
        cursors: [0; 5],
        sought: false,
        rows: 0,
        done: false,
    };
    trace.write(json!({"kind":"capture","schema":1,"replayWindow":window,"maxRows":MAX_ROWS,
        "pid":std::process::id(),"exe":std::env::current_exe().ok(),"units":"world (source * 0.12), Y up"}));
    let _ = trace.output.flush();
    eprintln!(
        "MRC TRACE armed: {path:?}; replay seconds {}..{}",
        window.0, window.1
    );
    install_systems(app, trace);
}

fn replay_running(playback: Res<MrcPlayback>) -> bool {
    playback.active
}

fn install_systems(app: &mut App, trace: Trace) {
    let actor_schedule = fighter_frame::schedule(app);
    app.insert_resource(trace)
        .add_systems(
            Update,
            pump.in_set(FighterFrameSet::Movement)
                .after(tick_mrc_playback)
                .before(apply_replay_seek)
                .run_if(replay_running.or_else(|seek: Res<ReplaySeekRequest>| seek.0.is_some())),
        )
        .add_systems(
            Update,
            after_session
                .in_set(FighterFrameSet::Movement)
                .after(smooth_mrc_remote_fighters)
                .before(crate::plugins::input_plugin::capture_local_command_input)
                .run_if(replay_running),
        )
        .add_systems(
            Update,
            after_input
                .in_set(FighterFrameSet::Movement)
                .after(crate::plugins::input_plugin::capture_local_command_input)
                .before(FighterFrameSet::PoseDecision)
                .run_if(replay_running),
        )
        .add_systems(
            actor_schedule,
            before_hit
                .after(FighterFrameSet::SkeletonSample)
                .before(FighterFrameSet::HitJudge),
        )
        .add_systems(
            actor_schedule,
            after_hit
                .after(FighterFrameSet::HitJudge)
                .before(FighterFrameSet::RootMotion),
        )
        .add_systems(
            Update,
            (after_camera, impacts)
                .chain()
                .after(FighterFrameSet::CameraFinalize),
        );
}

fn pump(
    mut trace: ResMut<Trace>,
    playback: Res<MrcPlayback>,
    seek: Res<ReplaySeekRequest>,
    frame_override: Res<MrcFrameOverride>,
    settings: Option<Res<Settings>>,
    launch: Option<Res<LaunchRequest>>,
    waypoints: Option<Res<meteor_scene::stage_loader::WaypointGraph>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keymap: Res<Keymap>,
) {
    let Some(doc) = playback
        .doc
        .as_ref()
        .filter(|_| playback.active && !trace.done)
    else {
        return;
    };
    let t = playback.elapsed;
    let now = [
        playback.index_a,
        playback.index_b,
        playback.index_c,
        playback.index_d,
        playback.index_e,
    ];
    let old = std::mem::replace(&mut trace.cursors, now);
    let previous = trace.last_pump.replace(t);
    if let Some(target) = seek.0 {
        trace.sought = true;
        trace.write(json!({"kind":"seek", "replayTime":t, "target":target}));
        return;
    }
    if std::mem::take(&mut trace.sought) {
        trace.write(json!({"kind":"seek-cursors", "replayTime":t, "indices":now}));
        return;
    }
    if previous.is_some_and(|before| before <= trace.window.1 && t > trace.window.1) {
        trace.write(json!({"kind":"capture-end", "replayTime":t, "reason":"window-end"}));
        let _ = trace.output.flush();
        trace.done = true;
        return;
    }
    if !trace.tracks(t) || previous == Some(t) {
        return;
    }
    if previous.is_none_or(|p| p < trace.window.0 || p > t) {
        trace.write(json!({"kind":"replay", "file":playback.file_name, "localActorId":playback.local_slot(),
            "fileStage":playback.stage_id(), "loadedStage":launch.as_ref().map(|r| r.stage_id.as_str()),
            "sourceCenter":mrc_scene_frame(&playback, waypoints.as_deref()).map(|s|s.0),
            "targetFps":settings.as_ref().map(|s|s.target_fps), "autoCamera":settings.as_ref().map(|s|s.auto_camera),
            "duration":doc.duration(), "state":doc.state, "meta":doc.meta}));
    }

    let a = doc
        .queue_a
        .get(old[0]..now[0])
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            let raw: String = entry
                .blob
                .iter()
                .take(128)
                .map(|b| format!("{b:02x}"))
                .collect();
            json!({"eventTime":entry.time,"id":entry.id,"size":entry.blob.len(),"raw":raw})
        })
        .collect::<Vec<_>>();
    let b = doc
        .queue_b
        .get(old[1]..now[1])
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            json!({"eventTime":entry.time,"code":entry.code,"value":entry.value,
            "binding":mrc_code_to_key(entry.code, &keymap).map(|binding| binding.name()),
            "action":mrc_code_to_synthetic_action(entry.code).map(|action|format!("{action:?}"))})
        })
        .collect::<Vec<_>>();
    let c = doc
        .queue_c
        .get(old[2]..now[2])
        .unwrap_or_default()
        .iter()
        .map(|e| json!({"eventTime":e.time,"words":[e.packed]}))
        .collect::<Vec<_>>();
    let d = doc
        .queue_d
        .get(old[3]..now[3])
        .unwrap_or_default()
        .iter()
        .map(|e| json!({"eventTime":e.time,"words":[e.a,e.b,e.c]}))
        .collect::<Vec<_>>();
    let e = doc
        .queue_e
        .get(old[4]..now[4])
        .unwrap_or_default()
        .iter()
        .map(|e| json!({"eventTime":e.time,"words":[e.a,e.b]}))
        .collect::<Vec<_>>();
    trace.write(json!({"kind":"pump","replayTime":t,"indices":now,"A":a,"B":b,"C":c,"D":d,"E":e,
        "positionOverride":frame_override.position_applied,"facingOverride":frame_override.facing_applied,
        "keys":keys.get_pressed().map(|key| format!("{key:?}")).collect::<Vec<_>>(),
        "mouse":mouse.get_pressed().map(|key| format!("{key:?}")).collect::<Vec<_>>() }));
}

fn actor_rows(world: &mut World, stage: &str) {
    let playback = world.resource::<MrcPlayback>();
    let t = playback.elapsed;
    if !playback.active || playback.paused || !world.resource::<Trace>().tracks(t) {
        return;
    }
    let slot = playback.local_slot();
    let pending: Vec<Value> = Vec::new();
    let active = world
        .get_resource::<fighter_frame::ActiveFighter>()
        .and_then(|active| active.0);
    let actors: Vec<_> = world
        .query_filtered::<Entity, With<Fighter>>()
        .iter(world)
        .collect();
    let actors = actors.into_iter().map(|entity| {
        let fighter = world.get::<Fighter>(entity).unwrap();
        let local = world.get::<LocalFighter>(entity).is_some();
        let remote = world.get::<MrcRemoteFighter>(entity);
        let pose = world.get::<PoseRuntime>(entity);
        let blend = world.get::<EntryBlend>(entity);
        let motion = world.get::<MotionState>(entity);
        let attack = world.get_resource::<CharacterAssetsSet>()
            .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
            .and_then(|assets| sampled_attack_window(assets, pose?, blend?))
            .map(|(index, hit)|json!({"index":index,"bones":hit.bones,"reactionPose":hit.target_pose}));
        json!({"entity":entity.to_bits(),"actorId":if local { slot } else { remote.map(|r|u32::from(r.actor_id)) },
            "local":local,"character":fighter.character_id,"weapon":fighter.weapon,"weaponVariant":fighter.weapon_variant,
            "position":world.get::<Transform>(entity).map(|t|t.translation.to_array()),
            "velocity":motion.map(|m|m.velocity.to_array()),"yaw":motion.map(|m|m.facing),
            "movement":motion.map(|m|format!("{:?}",m.movement_mode)),"pose":pose.map(|p|p.pose_id),
            "frame":pose.map(|p|p.frame),"poseInstance":pose.map(|p|p.pose_instance_id),
            "blend":blend.map(|b|json!({"kind":format!("{:?}",b.kind),"elapsed":b.elapsed,"duration":b.duration,"toFrame":b.to_frame})),
            "sm":world.get::<SmState>(entity).map(|sm|format!("{sm:?}")),"hp":world.get::<Health>(entity).map(|h|h.current),
            "anger":world.get::<Qi>(entity).map(|q|q.current), "attackWindow":attack,
            "reaction":world.get::<FighterHitReaction>(entity).and_then(|reaction|reaction.current.as_ref()).map(|r|
                json!({"kind":format!("{:?}",r.kind),"pose":r.pose_id,"selector":r.selector,"remaining":r.lock_remaining})),
            "remoteTarget":world.get::<RemoteTransformTarget>(entity).map(|r|json!({"position":r.translation.to_array(),"yaw":r.yaw})),
            "remoteState":remote.map(|r|json!({"desiredPose":r.desired_pose_id,"posePending":r.pose_pending,
                "poseSequence":r.pose_sequence,"actionSequence":r.action_sequence,"stateAge":r.state_age,"hitAge":r.hit_age}))})
    }).collect::<Vec<_>>();
    let row = json!({"kind":"actors","stage":stage,"replayTime":t, "active":active.map(Entity::to_bits),
        "actors":actors, "pendingHitLocks":pending,
        "lock":world.get_resource::<LockOnController>().and_then(|l|l.lock_entity).map(Entity::to_bits),
        "cameraYaw":world.get_resource::<CameraBasis>().map(|c|c.yaw),
        "clock":world.get_resource::<FrameClock>().map(|c|json!({"simDt":c.sim_dt,"wallDt":c.wall_dt,
            "substepNow":c.substep.map(|s|s.now_seconds)}))});
    world.resource_mut::<Trace>().write(row);
}

fn after_session(world: &mut World) {
    actor_rows(world, "after-session");
}

fn after_input(
    mut trace: ResMut<Trace>,
    playback: Res<MrcPlayback>,
    combat: Option<Res<crate::plugins::input_plugin::LocalCombatInput>>,
    crouch: Option<Res<crate::plugins::input_plugin::LocalCrouchInput>>,
    guard: Option<Res<crate::plugins::input_plugin::LocalGuardInput>>,
) {
    if !playback.active || playback.paused || !trace.tracks(playback.elapsed) {
        return;
    }
    if let Some(combat) = combat {
        trace.write(
            json!({"kind":"input-consumed","replayTime":playback.elapsed,
            "edges":combat.edges.iter().map(|edge|format!("{edge:?}")).collect::<Vec<_>>(),
            "guardHeld":guard.as_ref().map(|g|g.held),"crouchHeld":crouch.as_ref().map(|c|c.held),
            "commandUpdateMs":combat.command_update_ms}),
        );
    }
}

fn before_hit(world: &mut World) {
    actor_rows(world, "before-hit");
}
fn after_hit(world: &mut World) {
    actor_rows(world, "after-hit");
}
fn after_camera(world: &mut World) {
    actor_rows(world, "after-camera");
}

fn impacts(
    mut trace: ResMut<Trace>,
    playback: Res<MrcPlayback>,
    events: Option<Res<Messages<CombatImpactEvent>>>,
    mut cursor: Local<MessageCursor<CombatImpactEvent>>,
) {
    let Some(events) = events else {
        return;
    };
    for event in cursor.read(&events) {
        if playback.active && trace.tracks(playback.elapsed) {
            trace.write(json!({"kind":"impact","replayTime":playback.elapsed,"impactKind":format!("{:?}",event.kind),
                "attacker":event.attacker.to_bits(),"defender":event.defender.to_bits(),"pose":event.reaction_pose,
                "strategy":event.effect_strategy,"contact":event.contact.to_array()}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mrc_trace_observes_the_production_pump_and_actor_snapshots() {
        let path = std::env::temp_dir().join(format!(
            "meteor-mrc-trace-{}-{}.jsonl",
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
        let mut app = App::new();
        app.insert_resource(State::new(AppState::Playing))
            .init_resource::<Time>()
            .init_resource::<Keymap>()
            .init_resource::<CameraBasis>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>();
        crate::plugins::configure_fighter_frame_sets(&mut app);
        app.add_plugins(ReplayPlugin);
        install_systems(
            &mut app,
            Trace {
                output: BufWriter::new(file),
                window: (0.0, 0.04),
                last_pump: None,
                cursors: [0; 5],
                sought: false,
                rows: 0,
                done: false,
            },
        );
        let mut doc = meteor_asset::mrc::MrcDocument {
            magic: meteor_asset::mrc::MRC_MAGIC,
            header_size: meteor_asset::mrc::MRC_HEADER_SIZE,
            mark0: 0,
            mark1: 0,
            tail: 0,
            state: [0; 20],
            meta: [0; 8],
            queue_a: Vec::new(),
            queue_b: Vec::new(),
            queue_c: Vec::new(),
            queue_d: Vec::new(),
            queue_e: Vec::new(),
        };
        doc.state[4..8].copy_from_slice(&1u32.to_le_bytes());
        doc.queue_b.push(meteor_asset::mrc::MrcQueueBEntry {
            code: 0x51,
            value: 1,
            time: 0.01,
        });
        doc.queue_d.push(meteor_asset::mrc::MrcQueueDEntry {
            a: 10.0f32.to_bits(),
            b: 20.0f32.to_bits(),
            c: 30.0f32.to_bits(),
            time: 0.01,
        });
        doc.queue_d.push(meteor_asset::mrc::MrcQueueDEntry {
            a: 0,
            b: 0,
            c: 0,
            time: 0.2,
        });
        app.insert_resource(MrcPlayback {
            active: true,
            doc: Some(doc),
            ..default()
        });
        let actor = app
            .world_mut()
            .spawn((
                LocalFighter,
                Transform::default(),
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 5,
                    weapon_slots: [5, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(20));
        app.update();
        assert!(app
            .world()
            .get::<Transform>(actor)
            .unwrap()
            .translation
            .abs_diff_eq(Vec3::new(1.2, 3.6, 2.4), 1e-5));
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(30));
        app.update();
        drop(app);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let rows: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let pump = rows.iter().find(|r| r["kind"] == "pump").unwrap();
        assert_eq!(pump["B"][0]["code"], 0x51);
        assert_eq!(pump["D"][0]["words"][0], 10.0f32.to_bits());
        assert_eq!(pump["positionOverride"], true);
        assert!(rows
            .iter()
            .any(|r| r["stage"] == "after-session" && r["actors"][0]["actorId"] == 1));
        assert_eq!(rows.last().unwrap()["reason"], "window-end");
    }

    #[test]
    fn mrc_trace_window_is_bounded() {
        assert_eq!(parse_window("0,120"), Some(DEFAULT_WINDOW));
        assert_eq!(parse_window("257,267"), Some((257.0, 267.0)));
        assert_eq!(parse_window("0,180"), Some((0.0, 180.0)));
        for value in ["NaN,50", "0,inf", "-1,20", "2,2", "0,181", "50", "a,b"] {
            assert_eq!(parse_window(value), None, "{value}");
        }
    }
}
