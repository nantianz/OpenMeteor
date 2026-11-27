use super::*;
use crate::plugins::stock_bot::binding_for;
use meteor_scene::hazards::{build_volumes, build_world_bounds, StageHazards};

const ARENA: &str = "dagger_lab";
const SN06: &str = "sn06";
const FALL_Y: f32 = -38.0;

const SN06_STARTS: [[[f32; 2]; 2]; 5] = [
    [[-1.051, 48.211], [3.673, -16.503]],
    [[-46.585, -27.724], [-1.891, 10.207]],
    [[24.686, -50.656], [-5.232, 5.616]],
    [[-46.171, 47.223], [3.909, 7.260]],
    [[-23.736, 47.635], [33.669, -2.040]],
];
const SN06_YAWS: [f32; 5] = [-2.311, -0.583, -0.684, -1.404, -1.354];

struct Sn06Scene {
    mesh: StageMeshCollider,
    hazards: StageHazards,
    pads: Vec<Vec3>,
}

fn sn06_scene() -> Result<&'static Sn06Scene, String> {
    static SCENE: std::sync::OnceLock<Result<Sn06Scene, String>> = std::sync::OnceLock::new();
    SCENE
        .get_or_init(|| {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
            let bytes = |path| std::fs::read(root.join(path)).map_err(|err| err.to_string());
            let model = meteor_asset::gmdl::decode_gmdl(&bytes("sn06/sn06.gmb")?)
                .map_err(|err| err.to_string())?;
            let desc = meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(
                &bytes("sn06/sn06.des")?,
            ));
            let cob = meteor_asset::cob::parse_cob(&bytes("sn06/sn06.cob")?)
                .map_err(|err| err.to_string())?;
            let wp = crate::text_codec::decode_text(&bytes("Level/sn06.wp")?);
            let center = meteor_scene::stage_loader::get_stage_center(SN06, Some(&wp));
            let mesh = StageMeshCollider::from_gmdl(&model, &desc, Some(&cob.face_ids), center);
            let mut hazards = StageHazards::default();
            hazards.volumes = build_volumes(&model, &desc, center);
            hazards.world_bounds = build_world_bounds(&model, center);
            let pads = meteor_network::spawn::pads(&desc, 0)
                .map(|object| {
                    let p = object.position;
                    Vec3::new(p.x - center[0], p.z - center[2], center[1] - p.y)
                        * meteor_core::MODEL_SCALE
                })
                .collect();
            Ok(Sn06Scene {
                mesh,
                hazards,
                pads,
            })
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn sn06_support_grid() -> Result<Value, String> {
    use meteor_physics::collision::MeshCollider;
    let mesh = &sn06_scene()?.mesh;
    let rows: Vec<Vec<Option<f32>>> = (-100..=100)
        .step_by(2)
        .map(|z| {
            (-100..=100)
                .step_by(2)
                .map(|x| {
                    mesh.sweep_foot_segment(x as f32, 60.0, -(z as f32), FALL_Y)
                        .map(|hit| (hit.y * 100.0).round() / 100.0)
                })
                .collect()
        })
        .collect();
    Ok(json!({"stage": SN06, "origin": -100, "pitch": 2, "rows": rows}))
}

fn place_sn06(
    harness: &mut GameplayHarness,
    fighters: [Entity; 2],
    spec: &ResetSpec,
    distance: f32,
    lateral: f32,
) -> Result<(), String> {
    use meteor_physics::collision::MeshCollider;
    let scene = sn06_scene()?;
    let mesh = &scene.mesh;
    let spawn = spec.spawn.unwrap_or_else(|| {
        if seeded_fraction(spec.seed ^ 0x6623) < 0.18 {
            (seeded_fraction(spec.seed ^ 0x2923) * SN06_STARTS.len() as f32) as u8
        } else {
            5
        }
    });
    let near = spawn == 5;
    let mut pair = if near {
        [[8.0, -18.0], [8.0 + lateral, -18.0 - distance]]
    } else {
        *SN06_STARTS
            .get(spawn as usize)
            .ok_or("expected sn06 spawn in [0, 5]")?
    };

    if !near {
        if let Some(pad) = scene
            .pads
            .iter()
            .find(|p| (p.x - pair[0][0]).hypot(-p.z - pair[0][1]) < 0.5)
        {
            pair[0] = [pad.x, -pad.z];
        }
    }
    let facing = (pair[1][0] - pair[0][0]).atan2(pair[0][1] - pair[1][1]);
    let mut yaws = if near {
        [facing, facing + std::f32::consts::PI]
    } else {
        [0.0, SN06_YAWS[spawn as usize]]
    };
    if !near && seeded_fraction(spec.seed ^ 0x1739) < 0.5 {
        pair.swap(0, 1);
        yaws.swap(0, 1);
    }
    harness.replace_stage_mesh(mesh.clone());
    let world = harness.app.world_mut();
    for ((fighter, pos), facing) in fighters.into_iter().zip(pair).zip(yaws) {
        let [x, z] = pos;
        let support = mesh
            .sample_ground_hit(x, -z, FALL_Y, 2.159)
            .ok_or_else(|| format!("sn06 spawn {spawn} has no ground at {pos:?}"))?;
        world.get_mut::<Transform>(fighter).unwrap().translation = Vec3::new(x, support.y, -z);
        world.get_mut::<MotionState>(fighter).unwrap().facing = facing;
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct ResetSpec {
    seed: u64,
    stage: String,
    loadouts: [u8; 2],
    max_steps: u32,
    slots: usize,
    physics_hz: u32,
    distance: Option<f32>,
    lateral: Option<f32>,
    starting_hp: Option<[f32; 2]>,
    spawn: Option<u8>,
    #[serde(default)]
    timing: bool,
}

impl ResetSpec {
    fn placement(&self) -> Result<(f32, f32), String> {
        if !matches!(self.stage.as_str(), ARENA | SN06) || self.loadouts != [DAGGER; 2] {
            return Err("expected dagger_lab or sn06 with two dagger loadouts".into());
        }
        if self.max_steps == 0
            || !(1..=3).contains(&self.slots)
            || !matches!(self.physics_hz, 30 | 60 | 120)
        {
            return Err(
                "expected positive max_steps, 1–3 slots and physics_hz of 30, 60 or 120".into(),
            );
        }
        let distance = self
            .distance
            .unwrap_or_else(|| 2.8 + 2.0 * seeded_fraction(self.seed));
        let lateral = self
            .lateral
            .unwrap_or_else(|| 1.4 * seeded_fraction(!self.seed) - 0.7);
        if !(2.0..=12.0).contains(&distance) || !(-3.0..=3.0).contains(&lateral) {
            return Err("expected distance in [2, 12] and lateral in [-3, 3]".into());
        }
        Ok((distance, lateral))
    }
}

fn seeded_fraction(seed: u64) -> f32 {
    let mut value = seed.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    (value >> 40) as f32 / 16_777_216.0
}

fn combat_timing(world: &World, fighter: Entity, contact: u8) -> Value {
    let pose = world.get::<PoseRuntime>(fighter).unwrap();
    let blend = world.get::<EntryBlend>(fighter).unwrap();
    let sm = world.get::<SmState>(fighter).unwrap();
    let reaction = world
        .get::<FighterHitReaction>(fighter)
        .and_then(|state| state.current.as_ref());
    let assets = &world.resource::<CharacterAssetsSet>().0["p0"];
    let authored = assets.poses.get(&pose.pose_id);
    let fighting = matches!(
        sm.current,
        FighterState::Attack1 | FighterState::Attack2 | FighterState::Attack3
    );
    let blending = blend.is_active_for(pose.pose_id);
    let frame = pose.frame.floor() as u32;
    let windows: Vec<_> = authored
        .into_iter()
        .flat_map(|def| &def.attacks)
        .filter(|_| fighting)
        .map(|attack| {
            [
                attack.start as f32 - pose.frame,
                attack.end as f32 - pose.frame,
            ]
        })
        .collect();
    let buffer = authored
        .filter(|_| fighting)
        .and_then(|def| def.next_pose.as_ref())
        .and_then(|window| Some((window.start?, window.end?)));
    let buffer_open = buffer.is_some_and(|(start, end)| (start..=end).contains(&frame));
    let successors: Vec<_> = assets
        .act
        .edges
        .get(&pose.pose_id)
        .filter(|_| buffer.is_some())
        .into_iter()
        .flatten()
        .filter(|&&id| assets.poses.contains_key(&id))
        .collect();
    let neutral_air_successor = if !buffer_open
        || blending
        || sm.attack_queued.is_some()
        || !world.get::<FighterAir>(fighter).unwrap().air.airborne
    {
        None
    } else {
        use meteor_combat::combo_engine::{pick_act_successor, resolve_weapon_command};
        resolve_weapon_command(DAGGER, &[], true, 0).and_then(|command| {
            let action = assets.act.groups.get(&command.pose_id)?;
            let candidates = assets.act.edges.get(&pose.pose_id)?;
            let poses: Vec<_> = assets.poses.keys().copied().collect();
            pick_act_successor(*action, candidates, &poses, &assets.act.groups)
        })
    };
    json!({
        "pose": {
            "frame": (pose.frame - pose.start as f32).max(0.0),
            "frames_to_end": pose.plays_to_end().then(|| (pose.end as f32 - pose.frame).max(0.0)),
            "blending": blending,
            "attack_windows": windows,
            "natural_link": authored.and_then(|def| def.link),
            "buffer_window": buffer.map(|(start, end)| [start as f32 - pose.frame, end as f32 - pose.frame]),
            "buffer_open": buffer_open && !blending,
            "successors": successors,
        },
        "control": {
            "locked": sm.locked,
            "reaction": reaction.map(|r| format!("{:?}", r.kind)),
            "lock_seconds": reaction.and_then(|r| r.lock_remaining.is_finite().then_some(r.lock_remaining)),
            "minimum_seconds": reaction.map(|r| r.minimum_remaining),
            "down_seconds": reaction.map(|r| r.down_remaining),
            "queued_pose": sm.attack_queued.as_ref().map(|a| a.next_pose_id),
            "neutral_air_successor": neutral_air_successor,
            "contact": contact,
        },
    })
}

struct DuelEnv {
    harness: GameplayHarness,
    fighters: [Entity; 2],
    keymap: Keymap,
    spec: ResetSpec,
    tick: u32,
    hp: [f32; 2],
    events: Vec<Value>,
    fallen: [bool; 2],

    contacts_by_pose: [((u16, u32), u8); 2],
}

impl DuelEnv {
    fn reset(spec: ResetSpec) -> Result<Self, String> {
        let (distance, lateral) = spec.placement()?;
        let (mut harness, fighters) = peer_harness(distance, lateral);
        if serial_executor() {
            use bevy::ecs::schedule::{Schedules, SingleThreadedExecutor};
            let mut schedules = harness.app.world_mut().resource_mut::<Schedules>();
            for (_, schedule) in schedules.iter_mut() {
                schedule.set_executor(SingleThreadedExecutor::new());
            }
        }
        if spec.stage == SN06 {
            place_sn06(&mut harness, fighters, &spec, distance, lateral)?;
        }
        let world = harness.app.world_mut();
        world.resource_mut::<PeerContacts>().include_instance = spec.timing;
        if let Some(hp) = spec.starting_hp {
            for (fighter, current) in fighters.into_iter().zip(hp) {
                let mut health = world.get_mut::<Health>(fighter).unwrap();
                if !(current > 0.0 && current <= health.max) {
                    return Err(format!(
                        "starting HP must be positive and at most {}",
                        health.max
                    ));
                }
                health.current = current;
            }
        }
        let hp = fighters.map(|fighter| world.get::<Health>(fighter).unwrap().current);
        let keymap = world.resource::<Keymap>().clone();
        Ok(Self {
            harness,
            fighters,
            keymap,
            spec,
            tick: 0,
            hp,
            events: Vec::new(),
            fallen: [false; 2],
            contacts_by_pose: [((0, 0), 0); 2],
        })
    }

    fn terminated(&self) -> bool {
        self.hp.iter().any(|&hp| hp <= 0.0)
    }

    fn snapshot(&self, net: f32) -> Value {
        let world = self.harness.app.world();
        let fighters = std::array::from_fn::<_, 2, _>(|side| {
            let fighter = self.fighters[side];
            let mut fields = body(world, fighter);
            fields.as_object_mut().unwrap().remove("attack_recovery");
            if self.spec.timing {
                fields["timing"] = combat_timing(world, fighter, self.contacts_by_pose[side].1);
            }
            fields
        });
        let terminated = self.terminated();
        let truncated = !terminated && self.tick == self.spec.max_steps;
        let winner = if (terminated || truncated) && self.hp[0] != self.hp[1] {
            Some(usize::from(self.hp[1] > self.hp[0]))
        } else {
            None
        };
        let mut snapshot = json!({
            "tick": self.tick, "fighters": fighters, "cameras": peer_cameras(world),
            "reward": [net, -net], "terminated": terminated, "truncated": truncated,
            "info": {"seed": self.spec.seed, "slots": self.spec.slots, "physics_hz": self.spec.physics_hz,
                     "winner": winner, "contacts": self.events},
        });
        if self.spec.stage == SN06 {
            snapshot["info"]["fallen"] = json!(self.fallen);
        }
        snapshot
    }

    fn step(&mut self, inputs: &PeerInputs) -> Result<Value, String> {
        if self.terminated() || self.tick == self.spec.max_steps {
            return Err("episode ended; reset before stepping".into());
        }
        for edges in inputs {
            if edges.len() > 8 {
                return Err("at most eight input edges per side and step".into());
            }
            for (name, pressed) in edges {
                if matches!(
                    name.as_str(),
                    "switch_weapon" | "drop_weapon" | "drop_item" | "cure"
                ) || (binding_for(name, &self.keymap).is_none()
                    && bot_look_degrees(name, *pressed).is_none())
                {
                    return Err(format!("unsupported dagger input: {name}"));
                }
            }
        }
        self.events.clear();
        step_peers(
            &mut self.harness,
            &self.keymap,
            inputs,
            self.spec.slots,
            self.spec.physics_hz,
            &mut self.events,
        );
        self.tick += 1;
        let world = self.harness.app.world_mut();
        if self.spec.timing {
            for (side, fighter) in self.fighters.into_iter().enumerate() {
                let pose = world.get::<PoseRuntime>(fighter).unwrap();
                let key = (pose.pose_id, pose.pose_instance_id);
                let contact = &mut self.contacts_by_pose[side];
                if key != contact.0 {
                    *contact = (key, 0);
                }
                for event in &self.events {
                    if event["side"].as_u64() == Some(side as u64)
                        && event["attack_pose"].as_u64() == Some(key.0.into())
                        && event["attack_instance"].as_u64() == Some(key.1.into())
                    {
                        contact.1 |= if event["blocked"].as_bool() == Some(true) {
                            2
                        } else {
                            1
                        };
                    }
                }
            }
        }
        let hp = self
            .fighters
            .map(|fighter| world.get::<Health>(fighter).unwrap().current);
        let net = (self.hp[1] - hp[1]) - (self.hp[0] - hp[0]);
        self.hp = hp;
        if self.spec.stage == SN06 {
            let hazards = &sn06_scene()?.hazards;
            for (side, fighter) in self.fighters.into_iter().enumerate() {
                let p = world
                    .get::<Transform>(fighter)
                    .unwrap()
                    .translation
                    .to_array();
                let hell = hazards
                    .volumes
                    .iter()
                    .any(|vol| vol.damage >= 100000 && StageHazards::point_in_aabb(p, &vol.box_));
                let outside = hazards
                    .world_bounds
                    .as_ref()
                    .is_some_and(|bounds| StageHazards::outside_world_bounds(p, bounds));
                if hp[side] > 0.0 && (hell || outside) {
                    self.fallen[side] = true;
                    self.hp[side] = 0.0;
                    world.get_mut::<Health>(fighter).unwrap().current = 0.0;
                }
            }
        }
        Ok(self.snapshot(net))
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Sn06Support,
    Reset { spec: ResetSpec },
    Step { inputs: PeerInputs },
}

fn handle(env: &mut Option<DuelEnv>, request: Request) -> Result<Value, String> {
    match request {
        Request::Sn06Support => sn06_support_grid(),
        Request::Reset { spec } => {
            let next = DuelEnv::reset(spec)?;
            let observation = next.snapshot(0.0);
            *env = Some(next);
            Ok(observation)
        }
        Request::Step { inputs } => env.as_mut().ok_or("reset before stepping")?.step(&inputs),
    }
}

fn serial_executor() -> bool {
    std::env::var("METEOR_DAGGER_ENV_EXECUTOR").map_or(true, |value| value != "multi")
}

#[test]
#[ignore = "persistent JSONL training driver: tools/mrc_duel_env.py"]
fn dagger_env_stdio() -> io::Result<()> {
    use bevy::tasks::{ComputeTaskPool, TaskPoolBuilder};
    let threads = std::env::var("METEOR_DAGGER_ENV_THREADS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|&threads| threads > 0)
        .unwrap_or(1);
    ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(threads).build());

    let mut output = io::BufWriter::new(io::stdout());
    writeln!(output, "ENV {{\"ready\":1}}")?;
    output.flush()?;
    let mut env = None;
    for line in io::stdin().lock().lines() {
        let result = serde_json::from_str(&line?)
            .map_err(|error| error.to_string())
            .and_then(|request| handle(&mut env, request));
        let reply = match result {
            Ok(reply) => reply,
            Err(error) => json!({"error": error}),
        };
        writeln!(output, "ENV {reply}")?;
        output.flush()?;
    }
    Ok(())
}
