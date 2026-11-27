use super::*;
use crate::app_state::{InputBinding, Keymap};
use crate::plugins::{
    camera_plugin::{
        bot_look_degrees, BotLookDelta, CameraBasis, GameplayCamera, LocalTurnState,
        ThirdPersonState,
    },
    character_plugin::{load_character_asset_for_test, spawn_fighter_model_for_test, FighterModel},
    fighter_frame::{ActiveFighter, FighterUpdate},
    impact_fx_plugin::{CombatImpactEvent, CombatImpactKind},
    input_plugin::{capture_local_command_input, capture_local_hold_inputs},
    FighterFrameSet,
};

#[derive(Default)]
struct Controller {
    keys: ButtonInput<KeyCode>,
    mouse: ButtonInput<MouseButton>,
    command: LocalCommandInput,
    combat: LocalCombatInput,
    crouch: LocalCrouchInput,
    guard: LocalGuardInput,
    jump: LocalJumpInput,
    dodge: DesktopDodgeInput,
    lock: LockOnController,
    turn: LocalTurnState,
    basis: CameraBasis,
    camera: ThirdPersonState,
    pending: Vec<(InputBinding, bool)>,
    pending_look: f32,
}

impl Controller {
    fn exchange(&mut self, world: &mut World, camera: Entity) {
        fn swap<R: Resource<Mutability = bevy::ecs::component::Mutable>>(
            world: &mut World,
            parked: &mut R,
        ) {
            std::mem::swap(&mut *world.resource_mut::<R>(), parked);
        }
        swap(world, &mut self.keys);
        swap(world, &mut self.mouse);
        swap(world, &mut self.command);
        swap(world, &mut self.combat);
        swap(world, &mut self.crouch);
        swap(world, &mut self.guard);
        swap(world, &mut self.jump);
        swap(world, &mut self.dodge);
        swap(world, &mut self.lock);
        swap(world, &mut self.turn);
        swap(world, &mut self.basis);
        std::mem::swap(
            &mut *world.get_mut::<ThirdPersonState>(camera).unwrap(),
            &mut self.camera,
        );
    }
}

#[derive(Resource)]
struct PeerControls {
    fighters: [Entity; 2],
    controllers: [Controller; 2],
    camera: Entity,
}

impl PeerControls {
    fn active_side(&self, world: &World) -> usize {
        let active = world.resource::<ActiveFighter>().0.unwrap();
        self.fighters
            .iter()
            .position(|&fighter| fighter == active)
            .unwrap()
    }
}

fn set_local_side(world: &mut World, fighters: [Entity; 2], side: usize) {
    world
        .entity_mut(fighters[1 - side])
        .remove::<LocalFighter>();
    world.entity_mut(fighters[side]).insert(LocalFighter);

    world
        .get_mut::<Fighter>(fighters[side])
        .unwrap()
        .player_slot = 0;
    world
        .get_mut::<Fighter>(fighters[1 - side])
        .unwrap()
        .player_slot = 1;
}

fn begin_input(world: &mut World) {
    world.resource_scope(|world, mut peers: Mut<PeerControls>| {
        let side = peers.active_side(world);
        let camera = peers.camera;
        if side == 1 {
            set_local_side(world, peers.fighters, 1);
        }
        let controller = &mut peers.controllers[side];
        controller.exchange(world, camera);
        world.resource_mut::<BotLookDelta>().0 = std::mem::take(&mut controller.pending_look);
        for (binding, pressed) in controller.pending.drain(..) {
            match (binding, pressed) {
                (InputBinding::Key(key), true) => {
                    world.resource_mut::<ButtonInput<KeyCode>>().press(key)
                }
                (InputBinding::Key(key), false) => {
                    world.resource_mut::<ButtonInput<KeyCode>>().release(key)
                }
                (InputBinding::Mouse(button), true) => world
                    .resource_mut::<ButtonInput<MouseButton>>()
                    .press(button),
                (InputBinding::Mouse(button), false) => world
                    .resource_mut::<ButtonInput<MouseButton>>()
                    .release(button),
                (InputBinding::Unbound, _) => continue,
            }
            world
                .resource_mut::<SyntheticInputStream>()
                .push_live_binding(binding, pressed);
        }
    });
}

fn finish_input(world: &mut World) {
    world.resource_scope(|world, mut peers: Mut<PeerControls>| {
        let side = peers.active_side(world);
        let camera = peers.camera;
        world.resource_mut::<ButtonInput<KeyCode>>().clear();
        world.resource_mut::<ButtonInput<MouseButton>>().clear();
        peers.controllers[side].exchange(world, camera);
        if side == 1 {
            set_local_side(world, peers.fighters, 0);
        }
    });
}

#[derive(Resource, Default)]
struct PeerContacts {
    poses: [(u16, u32); 2],
    events: Vec<Value>,
    include_instance: bool,
}

fn sample_contact_poses(
    peers: Res<PeerControls>,
    poses: Query<&PoseRuntime>,
    mut contacts: ResMut<PeerContacts>,
) {
    contacts.poses = peers.fighters.map(|fighter| {
        let pose = poses.get(fighter).unwrap();
        (pose.pose_id, pose.pose_instance_id)
    });
}

fn collect_contacts(
    peers: Res<PeerControls>,
    mut impacts: MessageReader<CombatImpactEvent>,
    mut contacts: ResMut<PeerContacts>,
) {
    let poses = contacts.poses;
    for e in impacts.read() {
        let Some(side) = peers
            .fighters
            .iter()
            .position(|&fighter| fighter == e.attacker)
        else {
            continue;
        };
        if e.defender != peers.fighters[1 - side] {
            continue;
        }
        let mut contact = json!({"side": side, "blocked": e.kind == CombatImpactKind::Guard,
            "attack_pose": poses[side].0, "target_pose": poses[1 - side].0,
            "reaction_pose": e.reaction_pose});
        if contacts.include_instance {
            contact["attack_instance"] = json!(poses[side].1);
        }
        contacts.events.push(contact);
    }
}

fn peer_harness(distance: f32, lateral: f32) -> (GameplayHarness, [Entity; 2]) {
    let (mut h, target) = duel_harness_with_camera_schedule(distance, DAGGER, FighterUpdate);
    let fighters = [h.fighter, target];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let asset = load_character_asset_for_test(&[root], "p0").expect("original p0 duel resources");
    let mut pose = PoseRuntime::idle();
    asset.apply_pose(&mut pose, 0, true);
    let world = h.app.world_mut();
    world
        .resource_mut::<CharacterAssetsSet>()
        .0
        .insert("p0".into(), asset);
    let camera = world
        .query_filtered::<Entity, With<GameplayCamera>>()
        .single(world)
        .unwrap();
    let models: Vec<_> = world
        .query::<(Entity, &FighterModel)>()
        .iter(world)
        .filter_map(|(entity, model)| fighters.contains(&model.fighter).then_some(entity))
        .collect();
    for model in models {
        world.despawn(model);
    }
    world.get_mut::<Transform>(target).unwrap().translation.x = lateral;
    let angle = lateral.atan2(distance);
    for (side, fighter) in fighters.into_iter().enumerate() {
        world.get_mut::<Fighter>(fighter).unwrap().character_id = 0;
        world.get_mut::<MotionState>(fighter).unwrap().facing =
            angle + side as f32 * std::f32::consts::PI;
        world
            .entity_mut(fighter)
            .remove::<NpcFighter>()
            .insert((pose.clone(), HeadTrackState::default()));
        spawn_fighter_model_for_test(world, fighter);
    }
    world.insert_resource(PeerControls {
        fighters,
        controllers: Default::default(),
        camera,
    });

    use crate::plugins::effect_plugin as fx;
    h.app
        .init_resource::<Assets<fx::PonEfMaterial>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<Assets<crate::plugins::stage_material::GmdlMaterial>>()
        .init_resource::<fx::EfTextureCache>()
        .add_message::<fx::SceneEffectEvent>();
    h.app
        .world_mut()
        .run_system_once(fx::setup_effect_assets)
        .unwrap();
    fx::install_effect_playback_systems(&mut h.app);
    h.app
        .init_resource::<PeerContacts>()
        .init_resource::<BotLookDelta>()
        .add_systems(
            FighterUpdate,
            (
                sample_contact_poses.before(attack_windows),
                collect_contacts.after(attack_windows),
            )
                .in_set(FighterFrameSet::HitJudge),
        )
        .add_systems(FighterUpdate, begin_input.before(FighterFrameSet::Movement))
        .add_systems(
            FighterUpdate,
            (capture_local_command_input, capture_local_hold_inputs)
                .chain()
                .in_set(FighterFrameSet::Movement),
        )
        .add_systems(
            FighterUpdate,
            finish_input.after(FighterFrameSet::PoseResolve),
        );
    (h, fighters)
}

fn peer_body(world: &World, fighter: Entity) -> Value {
    let mut observation = body(world, fighter);
    let pose = world.get::<PoseRuntime>(fighter).unwrap();
    observation["pose_instance"] = json!(pose.pose_instance_id);
    observation["character_id"] = json!(world.get::<Fighter>(fighter).unwrap().character_id);
    observation
}

mod training {
    include!("duel_lab_env.rs");
}

type PeerInputs = [Vec<(String, bool)>; 2];

fn step_peers(
    h: &mut GameplayHarness,
    keymap: &Keymap,
    inputs: &PeerInputs,
    slots: usize,
    fps: u32,
    events: &mut Vec<Value>,
) {
    for slot in 0..slots {
        {
            let mut peers = h.app.world_mut().resource_mut::<PeerControls>();
            for (controller, edges) in peers.controllers.iter_mut().zip(inputs) {
                let first = slot * edges.len() / slots;
                let last = (slot + 1) * edges.len() / slots;
                for (name, pressed) in &edges[first..last] {
                    if let Some(delta) = bot_look_degrees(name, *pressed) {
                        controller.pending_look =
                            (controller.pending_look + delta).clamp(-120.0, 120.0);
                    } else {
                        controller.pending.push((binding(keymap, name), *pressed));
                    }
                }
            }
        }
        for frame in 0..fps / 30 {
            h.step_dt(1.0 / fps as f32);
            let mut contacts = h.app.world_mut().resource_mut::<PeerContacts>();
            for mut event in contacts.events.drain(..) {
                event["slot"] = json!(slot);
                event["frame"] = json!(frame);
                events.push(event);
            }
        }
    }
}

fn peer_cameras(world: &World) -> [Value; 2] {
    world.resource::<PeerControls>().controllers.each_ref().map(|controller| {
        json!({"basis_yaw": controller.basis.yaw, "locked": controller.lock.lock_entity.is_some()})
    })
}

#[test]
#[ignore = "local JSONL driver: tools/mrc_duel_peers.py"]
fn headless_duel_peers_stdio() {
    #[derive(serde::Deserialize)]
    struct Request {
        distance: f32,
        lateral: f32,
        windows: u32,
        fps: u32,
        starting_hp: Option<[f32; 2]>,
        followup_spacing: Option<(u32, usize, f32)>,
    }
    let mut lines = io::stdin().lock().lines();
    let request: Request = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert!((2.0..=12.0).contains(&request.distance) && request.lateral.abs() <= 3.0);
    assert!((1..=600).contains(&request.windows) && matches!(request.fps, 30 | 60 | 120));
    if let Some((tick, side, spacing)) = request.followup_spacing {
        assert!(tick < request.windows && side < 2 && (0.65..=4.1).contains(&spacing));
    }
    let (mut h, fighters) = peer_harness(request.distance, request.lateral);
    if let Some(hp) = request.starting_hp {
        for (fighter, current) in fighters.into_iter().zip(hp) {
            let mut health = h.app.world_mut().get_mut::<Health>(fighter).unwrap();
            assert!(current > 0.0 && current <= health.max);
            health.current = current;
        }
    }
    let keymap = h.app.world().resource::<Keymap>().clone();
    let mut output = io::stdout().lock();
    let mut hits = [0u32; 2];
    let mut guards = [0u32; 2];
    let mut events = Vec::new();
    for tick in 0..=request.windows {
        if let Some((at, side, spacing)) = request.followup_spacing {
            if at == tick {
                let world = h.app.world_mut();
                let [own, other] = [fighters[side], fighters[1 - side]];
                assert_eq!(world.get::<PoseRuntime>(own).unwrap().pose_id, 252);
                assert_eq!(world.get::<PoseRuntime>(other).unwrap().pose_id, 113);
                let origin = world.get::<Transform>(own).unwrap().translation;
                let mut target = world.get_mut::<Transform>(other).unwrap();
                let delta = target.translation - origin;
                let offset = Vec3::new(delta.x, 0.0, delta.z).normalize() * spacing;
                target.translation.x = origin.x + offset.x;
                target.translation.z = origin.z + offset.z;
            }
        }
        let world = h.app.world();
        let fighters_json = fighters.map(|entity| peer_body(world, entity));
        let cameras = peer_cameras(world);
        let done = tick == request.windows
            || fighters
                .iter()
                .any(|&fighter| world.get::<Health>(fighter).unwrap().current <= 0.0);
        writeln!(
            output,
            "DUEL {}",
            json!({"tick":tick, "done":done, "fighters":fighters_json,
            "cameras":cameras, "hits":hits, "guards":guards, "contacts":events})
        )
        .unwrap();
        output.flush().unwrap();
        if done {
            return;
        }
        events.clear();
        let line = lines.next().expect("missing peer inputs").unwrap();
        let inputs: PeerInputs = serde_json::from_str(&line).unwrap();
        assert!(inputs.iter().all(|edges| edges.len() <= 8));
        step_peers(&mut h, &keymap, &inputs, 3, request.fps, &mut events);
        for event in &events {
            let side = event["side"].as_u64().unwrap() as usize;
            if event["blocked"].as_bool().unwrap() {
                guards[side] += 1;
            } else {
                hits[side] += 1;
            }
        }
    }
}
