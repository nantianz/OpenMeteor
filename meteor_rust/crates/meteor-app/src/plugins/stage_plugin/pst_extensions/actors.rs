use std::collections::BTreeMap;

use bevy::prelude::*;
use meteor_scene::pst_game::{ActorAction, ScriptActor};

use super::{source_to_world, world_to_source, PstSession};
use crate::app_state::{AppState, LaunchRequest, LoadedStageInfo};
use crate::components::fighter::{
    DeathRespawn, Fighter, FighterStatusEffects, Health, MotionState, PoseRuntime, Qi,
    WeaponVariantChanged,
};
use crate::plugins::{
    battle_result, camera_plugin, character_plugin, combat_plugin, item_plugin, movement_plugin,
    network_plugin::NetState,
    render_plugin,
    stage_plugin::{
        drop_script_escort, resource_roots, LocalFighter, MissionNpc, NpcPerformance,
        StageMissionRuntime, StageScriptRuntime,
    },
};

#[derive(Component)]
pub(in crate::plugins) struct ScriptFighterOptions {
    pub allow_switch: bool,
    pub name: Option<String>,
    pub aggress_pose: Option<u16>,
}

impl Default for ScriptFighterOptions {
    fn default() -> Self {
        Self {
            allow_switch: true,
            name: None,
            aggress_pose: None,
        }
    }
}

#[derive(Component)]
pub(super) struct AttackSequence {
    commands: std::collections::VecDeque<String>,
    next: f64,
}

pub(super) fn actor_id(local: Option<&LocalFighter>, npc: Option<&MissionNpc>) -> Option<i64> {
    if local.is_some() {
        Some(0)
    } else {
        npc.filter(|npc| npc.in_scene_roster).map(|npc| npc.id)
    }
}

pub(super) fn actor_entity(world: &mut World, id: i64) -> Option<Entity> {
    world
        .query::<(Entity, Option<&LocalFighter>, Option<&MissionNpc>)>()
        .iter(world)
        .find_map(|(entity, local, npc)| (actor_id(local, npc) == Some(id)).then_some(entity))
}

pub(super) fn update_script_actors(world: &mut World) {
    if *world.resource::<State<AppState>>().get() != AppState::Playing {
        return;
    }
    let game = world.resource::<PstSession>().0.clone();
    if !game.active() {
        return;
    }
    let center = world.resource::<LoadedStageInfo>().center;
    let mut query = world.query::<(
        &Fighter,
        &Transform,
        &MotionState,
        &Health,
        Option<&Qi>,
        Option<&PoseRuntime>,
        Option<&FighterStatusEffects>,
        Option<&LocalFighter>,
        Option<&MissionNpc>,
        Option<&ScriptFighterOptions>,
    )>();
    let scores = world.get_resource::<battle_result::OfflineScores>();
    let local_display = world.resource::<LaunchRequest>().player_name.clone();
    let actors: BTreeMap<_, _> = query
        .iter(world)
        .filter_map(
            |(fighter, transform, motion, health, qi, pose, status, local, npc, options)| {
                let id = actor_id(local, npc)?;
                let active = usize::from(fighter.weapon_slot.min(1));
                let spare = 1 - active;
                let status_max = status.map_or(0, |s| s.applied_max_health_bonus);
                let items = (id == 0)
                    .then(|| world.get_resource::<item_plugin::ItemBuffs>())
                    .flatten();
                let local_max = items.map_or(0, |buffs| buffs.max_hp_bonus);
                let resources = world.get_resource::<combat_plugin::CombatResourceData>();
                let weapon =
                    resources.and_then(|res| combat_plugin::combat_weapon_stats(&res.0, fighter));
                let [attack, defense, speed] = status.map_or([0; 3], |status| status.pst_modifiers);
                let effective = [
                    (weapon.map_or_else(
                        || meteor_combat::hit_resolve::weapon_base_atk(fighter.weapon),
                        |weapon| weapon.attack,
                    ) + status.map_or(0, |status| status.effects.attack_bonus())
                        + items.map_or(0, |items| items.attack_bonus)
                        + attack) as f32,
                    (weapon.map_or(15, |weapon| weapon.defense)
                        + status.map_or(0, |status| status.effects.defense_bonus())
                        + items.map_or(0, |items| items.defense_bonus)
                        + defense) as f32,
                    combat_plugin::fighter_speed_factor(
                        resources,
                        fighter,
                        items.map_or(1.0, |items| items.speed_factor),
                        status,
                    ) * 100.0,
                ];
                let score_id = battle_result::offline_actor_id(
                    id == 0,
                    npc.map(|npc| npc.id),
                    fighter.player_slot,
                );
                let (kills, deaths) = scores.map_or((0, 0), |scores| scores.totals_for(&score_id));
                let name = options
                    .and_then(|options| options.name.as_deref())
                    .map_or_else(
                        || npc.map_or_else(|| local_display.clone(), |npc| npc.name.clone()),
                        str::to_owned,
                    );
                let mut buffs = status.map_or_else(Vec::new, |status| {
                    status
                        .effects
                        .active
                        .iter()
                        .map(|effect| i64::from(effect.definition.id))
                        .collect()
                });
                if let Some(items) = items {
                    buffs.extend(items.pst_buff_ids(resources.map(|resources| &resources.0)));
                    buffs.sort_unstable();
                    buffs.dedup();
                }
                Some((
                    id,
                    ScriptActor {
                        name,
                        position: world_to_source(transform.translation, center),
                        yaw: motion.facing.to_degrees(),
                        alive: health.current > 0.0,
                        team: i64::from(combat_plugin::fighter_combat_team(
                            world.resource::<NetState>(),
                            local.is_some(),
                            None,
                            npc,
                        )),
                        pose: pose.map_or(-1, |pose| i64::from(pose.pose_id)),
                        weapon: weapon_item(fighter.weapon, fighter.weapon_variant),
                        weapon_type: i64::from(fighter.weapon),
                        spare_weapon: weapon_item(
                            fighter.weapon_slots[spare],
                            fighter.weapon_slot_variants[spare],
                        ),
                        spare_type: i64::from(fighter.weapon_slots[spare]),
                        anger: qi.map_or(0.0, |qi| qi.current),
                        base: [
                            (health.max - (status_max + local_max) as f32).max(1.0),
                            health.current,
                            attack as f32,
                            defense as f32,
                            (100 + speed) as f32,
                        ],
                        effective,
                        kills: i64::from(kills),
                        deaths: i64::from(deaths),
                        buffs,
                        can_switch: options.is_none_or(|options| options.allow_switch),
                    },
                ))
            },
        )
        .collect();
    game.set_actors(actors);
}

fn weapon_item(weapon: u8, variant: u8) -> i64 {
    meteor_network::stock_protocol::stock_weapon_item_id(weapon, variant).map_or(0, i64::from)
}

pub(super) fn apply_actor_action(
    world: &mut World,
    entity: Entity,
    id: i64,
    action: ActorAction,
    center: [f32; 3],
) {
    match action {
        ActorAction::Position(position) => {
            let position = source_to_world(position, center);
            if let Some(mut transform) = world.get_mut::<Transform>(entity) {
                transform.translation = position;
            }
            if let Some(mut motion) = world.get_mut::<MotionState>(entity) {
                motion.velocity = Vec3::ZERO;
            }
            if let Some(mut air) = world.get_mut::<movement_plugin::FighterAir>(entity) {
                air.air.vertical_speed = 0.0;
                air.air.planar_x = 0.0;
                air.air.planar_z = 0.0;
                air.ground_external_velocity = Vec3::ZERO;
                air.landed_planar = Vec2::ZERO;
                air.ground_y = position.y;
            }
            if id == 0 {
                world
                    .entity_mut(entity)
                    .insert(camera_plugin::CameraFollowReset);
            }
        }
        ActorAction::Yaw(yaw) => {
            if let Some(mut motion) = world.get_mut::<MotionState>(entity) {
                motion.facing = yaw.to_radians();
            }
        }
        ActorAction::Team(team) => {
            if let Some(mut npc) = world.get_mut::<MissionNpc>(entity) {
                npc.team = team as u8;
            }
            if id == 0 {
                world.resource_mut::<NetState>().offline_script_player_team = Some(team as i32);
            }
            if let Some(runtime) = world.resource_mut::<StageScriptRuntime>().script_mut() {
                runtime.host_mut().set_character_team(id, team);
            }
        }
        ActorAction::Name(name) => {
            world
                .entity_mut(entity)
                .entry::<ScriptFighterOptions>()
                .or_default();
            world.get_mut::<ScriptFighterOptions>(entity).unwrap().name = Some(name.clone());
            if let Some(mut npc) = world.get_mut::<MissionNpc>(entity) {
                npc.name = name.clone();
            }
            if id == 0 {
                world.resource_mut::<NetState>().offline_script_player_name = Some(name.clone());
            }
            if let Some(runtime) = world.resource_mut::<StageMissionRuntime>().script_mut() {
                if id != 0 {
                    runtime.host_mut().sync_actor_name(id, &name);
                }
            }
        }
        ActorAction::Base(index @ (0 | 1), value) => {
            let bonus = world
                .get::<FighterStatusEffects>(entity)
                .map_or(0, |s| s.applied_max_health_bonus)
                + if id == 0 {
                    world
                        .get_resource::<item_plugin::ItemBuffs>()
                        .map_or(0, |b| b.max_hp_bonus)
                } else {
                    0
                };
            if let Some(mut health) = world.get_mut::<Health>(entity) {
                if index == 0 {
                    health.max = (value + bonus as f32).max(1.0);
                    health.current = health.current.min(health.max);
                } else if health.current > 0.0 {
                    health.current = value.clamp(1.0, health.max);
                }
            }
        }
        ActorAction::Base(index, value) => {
            if let Some(mut status) = world.get_mut::<FighterStatusEffects>(entity) {
                status.pst_modifiers[index - 2] =
                    (value.clamp(-100_000.0, 100_000.0) as i32) - if index == 4 { 100 } else { 0 };
            }
        }
        ActorAction::RemoveBuff(buff) => {
            if let Some(mut status) = world.get_mut::<FighterStatusEffects>(entity) {
                status
                    .effects
                    .active
                    .retain(|effect| i64::from(effect.definition.id) != buff);
            }
            if id == 0 {
                if world.contains_resource::<item_plugin::ItemBuffs>() {
                    world.resource_scope(|world, mut items: Mut<item_plugin::ItemBuffs>| {
                        let mut actors = world.query::<&mut Health>();
                        if world.contains_resource::<combat_plugin::CombatResourceData>() {
                            world.resource_scope(
                                |world, table: Mut<combat_plugin::CombatResourceData>| {
                                    if let Ok(mut health) = actors.get_mut(world, entity) {
                                        items.pst_remove_buff(buff, &mut health, Some(&table.0));
                                    }
                                },
                            );
                        } else if let Ok(mut health) = actors.get_mut(world, entity) {
                            items.pst_remove_buff(buff, &mut health, None);
                        }
                    });
                }
            }
        }
        ActorAction::AllowSwitch(allow_switch) => {
            world
                .entity_mut(entity)
                .entry::<ScriptFighterOptions>()
                .or_default();
            world
                .get_mut::<ScriptFighterOptions>(entity)
                .unwrap()
                .allow_switch = allow_switch;
        }
        ActorAction::Equip(weapon) => {
            if let Some((weapon, variant)) = character_plugin::weapon_slot_from_item(weapon) {
                equip_weapon(world, entity, weapon, variant);
            }
        }
        ActorAction::Model(model) => {
            let key = format!("p{model}");
            let Some(assets) = world.get_resource::<character_plugin::CharacterAssetsSet>() else {
                return;
            };
            if !assets.0.contains_key(&key) {
                let roots = resource_roots(&std::env::current_dir().unwrap_or_else(|_| ".".into()));
                let Some(assets) = character_plugin::load_character_asset(&roots, &key) else {
                    return;
                };
                world
                    .resource_mut::<character_plugin::CharacterAssetsSet>()
                    .0
                    .insert(key, assets);
            }
            if let Some(mut fighter) = world.get_mut::<Fighter>(entity) {
                if fighter.character_id == model {
                    return;
                }
                fighter.character_id = model;
            }
            let visuals: Vec<_> = world
                .query::<(Entity, &character_plugin::FighterModel)>()
                .iter(world)
                .filter_map(|(model, owner)| (owner.fighter == entity).then_some(model))
                .collect();
            for model in visuals {
                world.despawn(model);
            }
            show_pose(world, entity, 0, false);
        }
        ActorAction::DropWeapon => {
            let Some((model, spare)) = world.get::<Fighter>(entity).and_then(|fighter| {
                let spare = 1 - usize::from(fighter.weapon_slot.min(1));
                Some((
                    character_plugin::weapon_model_stem(fighter.weapon, fighter.weapon_variant)?,
                    spare,
                ))
            }) else {
                return;
            };
            let Some(position) = world
                .get::<Transform>(entity)
                .map(|transform| transform.translation)
            else {
                return;
            };
            let yaw = world
                .get::<MotionState>(entity)
                .map_or(0.0, |motion| motion.facing);
            let (weapon, variant) = {
                let mut fighter = world.get_mut::<Fighter>(entity).unwrap();
                (
                    std::mem::take(&mut fighter.weapon_slots[spare]),
                    std::mem::take(&mut fighter.weapon_slot_variants[spare]),
                )
            };
            equip_weapon(world, entity, weapon, variant);
            if let Some(mut drops) =
                world.get_resource_mut::<Messages<render_plugin::OfflineWeaponDeathDrop>>()
            {
                drops.write(render_plugin::OfflineWeaponDeathDrop {
                    model,
                    position: (position + Vec3::Y * 1.5).to_array(),
                    velocity: [yaw.sin() * 12.0, 0.0, yaw.cos() * 12.0],
                });
            }
        }
        ActorAction::DropVip => drop_script_escort(world, entity),
        ActorAction::Reborn(position) => {
            if world
                .get::<DeathRespawn>(entity)
                .is_some_and(|death| death.active)
            {
                let position = position
                    .map(|position| source_to_world(position, center))
                    .or_else(|| {
                        world
                            .get::<Transform>(entity)
                            .map(|transform| transform.translation)
                    });
                if let Some(position) = position {
                    world
                        .entity_mut(entity)
                        .insert(combat_plugin::ScriptRebornAt(position));
                }
                world.entity_mut(entity).insert(combat_plugin::ScriptReborn);
            }
        }
        ActorAction::Pose(pose) => {
            if let Ok(pose) = u16::try_from(pose) {
                show_pose(world, entity, pose, true);
            }
        }
        ActorAction::Combo(sequence) => {
            let now = world.resource::<Time>().elapsed_secs_f64();
            let commands = sequence
                .split_inclusive([',', '.'])
                .flat_map(|command| {
                    let stripped = command.trim_end_matches([',', '.']);
                    let attack = (!stripped.is_empty()).then(|| stripped.to_owned());
                    let delay = (stripped.len() != command.len()).then(|| String::new());
                    [attack, delay].into_iter().flatten()
                })
                .collect();
            world.entity_mut(entity).insert(AttackSequence {
                commands,
                next: now,
            });
        }
        ActorAction::ImmediateCommand(command) => immediate(world, entity, command),
        ActorAction::LockTarget => lock_target(world, entity, id),
        ActorAction::AggressPose(pose) => {
            world
                .entity_mut(entity)
                .entry::<ScriptFighterOptions>()
                .or_default();
            world
                .get_mut::<ScriptFighterOptions>(entity)
                .unwrap()
                .aggress_pose = Some(pose);
        }
        ActorAction::InputKey(key, pressed) => {
            if id == 0 {
                input_game_key(world, &key, pressed);
            } else if pressed {
                match key.as_str() {
                    "attack" => immediate(world, entity, "n".into()),
                    "jump" => immediate(world, entity, "j".into()),
                    "burst" => immediate(world, entity, "burst".into()),
                    "lock" => lock_target(world, entity, id),
                    _ => {}
                }
            }
        }
        ActorAction::Kill => {
            world
                .entity_mut(entity)
                .insert(combat_plugin::ScriptForcedDeath);
        }
    }
}

fn immediate(world: &mut World, entity: Entity, command: String) {
    if command == "j" {
        if let Some(mut performance) = world.get_mut::<NpcPerformance>(entity) {
            performance.jump_requested = true;
            return;
        }
    }
    world
        .entity_mut(entity)
        .insert(combat_plugin::ScriptActorInput(command));
}

fn lock_target(world: &mut World, entity: Entity, id: i64) {
    if id == 0 {
        if world
            .get_resource::<crate::components::fighter::LockOnController>()
            .is_some_and(|lock| lock.lock_entity.is_none())
        {
            input_game_key(world, "lock", true);
            input_game_key(world, "lock", false);
        }
    } else if let Some(mut intent) = world.get_mut::<crate::components::fighter::AiIntent>(entity) {
        intent.lock_target = intent.enemy_entity;
    }
}

fn input_game_key(world: &mut World, key: &str, pressed: bool) {
    use crate::app_state::Keymap;
    use crate::plugins::input_plugin::{SyntheticAction as Action, SyntheticInputStream};
    let Some(keymap) = world.get_resource::<Keymap>() else {
        return;
    };
    let (binding, action) = match key {
        "attack" => (keymap.attack, Some(Action::Attack)),
        "jump" => (keymap.jump, Some(Action::Jump)),
        "guard" => (keymap.guard, Some(Action::Guard)),
        "crouch" => (keymap.crouch, Some(Action::Crouch)),
        "lock" => (keymap.lock_toggle, Some(Action::LockToggle)),
        "switch" => (keymap.weapon_switch, Some(Action::WeaponSwitch)),
        "burst" => (keymap.qi_burst, Some(Action::QiBurst)),
        "fake" => (keymap.fake, Some(Action::Fake)),
        "cure" => (keymap.cure, Some(Action::Cure)),
        "dropweapon" => (keymap.drop_weapon, Some(Action::DropWeapon)),
        "dropitem" => (keymap.drop_item, Some(Action::DropItem)),
        "forward" => (keymap.move_up, None),
        "back" => (keymap.move_down, None),
        "left" => (keymap.move_left, None),
        "right" => (keymap.move_right, None),
        _ => return,
    };
    let Some(mut input) = world.get_resource_mut::<SyntheticInputStream>() else {
        return;
    };
    if let Some(action) = action {
        input.push_action(binding, action, pressed, None);
    } else {
        input.push_live_binding(binding, pressed);
    }
}

fn equip_weapon(world: &mut World, entity: Entity, weapon: u8, variant: u8) {
    if let Some(mut fighter) = world.get_mut::<Fighter>(entity) {
        let slot = usize::from(fighter.weapon_slot.min(1));
        fighter.weapon_slots[slot] = weapon;
        fighter.weapon_slot_variants[slot] = variant;
        fighter.weapon = weapon;
        fighter.weapon_variant = variant;
    }
    world
        .entity_mut(entity)
        .insert(WeaponVariantChanged)
        .remove::<(
            crate::components::fighter::WeaponSwitchActive,
            combat_plugin::ScriptWeaponSwitch,
        )>();
    if let Some(mut form) = world.get_mut::<crate::components::fighter::WeaponForm>(entity) {
        *form = default();
    }
}

fn show_pose(world: &mut World, entity: Entity, pose_id: u16, once: bool) {
    use crate::components::fighter::{EntryBlend, EntryBlendKind};
    if !world.contains_resource::<character_plugin::CharacterAssetsSet>() {
        return;
    }
    world.resource_scope(|world, assets: Mut<character_plugin::CharacterAssetsSet>| {
        let mut query = world.query::<(
            &Fighter,
            &mut PoseRuntime,
            &mut EntryBlend,
            &mut FighterStatusEffects,
        )>();
        if let Ok((fighter, mut pose, mut blend, mut status)) = query.get_mut(world, entity) {
            if let Some(assets) = assets.0.get(&format!("p{}", fighter.character_id)) {
                if !assets.poses.contains_key(&pose_id) {
                    return;
                }
                assets.apply_pose_with_blend(
                    &mut pose,
                    &mut blend,
                    pose_id,
                    true,
                    None,
                    EntryBlendKind::Generic,
                    false,
                    false,
                    Some(meteor_combat::weapon_strategy::weapon_to_strategy(
                        fighter.weapon,
                    )),
                );
                if once && pose.pose_id == pose_id {
                    pose.loops = false;
                    status.pst_pose_instance = Some(pose.pose_instance_id);
                }
            }
        }
    });
}

pub(super) fn advance_attack_sequences(
    time: Res<Time>,
    mut commands: Commands,
    mut fighters: Query<(Entity, &mut AttackSequence, Option<&mut NpcPerformance>)>,
) {
    let now = time.elapsed_secs_f64();
    for (entity, mut sequence, mut performance) in &mut fighters {
        if sequence.next > now {
            continue;
        }
        match sequence.commands.pop_front() {
            Some(command) if command.is_empty() => sequence.next = now + 0.2,
            Some(command) => {
                sequence.next = now;
                if command == "j" {
                    if let Some(performance) = performance.as_deref_mut() {
                        performance.jump_requested = true;
                        continue;
                    }
                }
                commands
                    .entity(entity)
                    .insert(combat_plugin::ScriptActorInput(command));
            }
            None => {
                commands.entity(entity).remove::<AttackSequence>();
            }
        }
    }
}
