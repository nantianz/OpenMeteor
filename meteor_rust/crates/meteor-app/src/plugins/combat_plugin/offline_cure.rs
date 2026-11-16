use super::*;

const CURE_RANGE: f32 = 80.0 * meteor_core::constants::MODEL_SCALE;

#[derive(Message, Clone, Copy)]
pub(super) struct CureCompleted {
    pub(super) healer: Entity,
    pub(super) position: Vec3,
}

pub(super) fn clear_cure_completed(mut completed: ResMut<Messages<CureCompleted>>) {
    completed.clear();
}

type CureTargets<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static MissionNpc,
        &'static Transform,
        &'static mut DeathRespawn,
        &'static mut Health,
        &'static mut Qi,
        &'static mut CombatState,
        &'static mut PoseRuntime,
        &'static mut SmState,
        &'static mut TargetHitClock,
        (
            &'static mut EntryBlend,
            &'static mut MotionState,
            &'static mut FighterAir,
            &'static mut FighterDodge,
            &'static mut FighterAttackMotion,
            &'static mut FighterHitReaction,
            &'static mut GunAimState,
            Option<&'static mut AiIntent>,
            Option<&'static mut FighterWall>,
        ),
    ),
    (With<MissionNpc>, Without<LocalFighter>),
>;

#[allow(clippy::too_many_arguments)]
pub(super) fn cure_mission_allies(
    mut commands: Commands,
    mut effects: MessageWriter<crate::plugins::effect_plugin::SceneEffectEvent>,
    request: Res<crate::app_state::LaunchRequest>,
    net: Res<NetState>,
    end: Res<MatchEnd>,
    mission: Res<crate::plugins::stage_plugin::StageMissionRuntime>,
    assets: Option<Res<CharacterAssetsSet>>,
    mut completed: MessageReader<CureCompleted>,
    local: Query<(&Health, &DeathRespawn), With<LocalFighter>>,
    mut targets: CureTargets<'_, '_>,
    mut statuses: Query<(
        &mut crate::components::fighter::FighterStatusEffects,
        &mut crate::components::fighter::WornEffect,
    )>,
) {
    let eligible = request.mode == "single"
        && !request.spawn_room_npcs
        && net.status != NetStatus::InRoom
        && !end.settled
        && mission.vip_team_for_actor(true, None) == Some(1);
    for completion in completed.read() {
        if !eligible {
            continue;
        }
        let Ok((health, death)) = local.get(completion.healer) else {
            continue;
        };
        if health.current <= 0.0 || death.active {
            continue;
        }
        let radius_squared = CURE_RANGE * CURE_RANGE;
        for (
            entity,
            fighter,
            npc,
            transform,
            mut death,
            mut health,
            mut qi,
            mut combat,
            mut pose,
            mut sm,
            mut hit_clock,
            (
                mut blend,
                mut motion,
                mut air,
                mut dodge,
                mut attack_motion,
                mut reaction,
                mut gun_aim,
                mut ai_intent,
                mut wall,
            ),
        ) in &mut targets
        {
            if !npc.in_scene_roster
                || npc.team != 1
                || !death.active
                || health.current > 0.0
                || transform.translation.distance_squared(completion.position) > radius_squared
            {
                continue;
            }
            if let Ok((mut status, mut worn)) = statuses.get_mut(entity) {
                status.clear(&mut worn);
            }
            death.active = false;
            death.timer = 0.0;
            health.current = health.max;
            qi.current = 0.0;
            let character_assets = assets
                .as_deref()
                .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)));
            reset_fighter_runtime_for_respawn(
                character_assets,
                &mut combat,
                &mut pose,
                &mut blend,
                &mut sm,
                &mut motion,
                &mut air,
                &mut dodge,
                &mut attack_motion,
                &mut reaction,
                &mut gun_aim,
                ai_intent.as_deref_mut(),
                wall.as_deref_mut(),
            );
            hit_clock.reset_spawn();
            hit_clock.clear_hit();
            commands.entity(entity).remove::<WeaponSwitchActive>();
            effects.write(crate::plugins::effect_plugin::SceneEffectEvent {
                effect: "ReBorn".to_string(),
                position: transform.translation,
            });
            info!("solo Cure revived {} (mission id {})", npc.name, npc.id);
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use meteor_scene::mission_runtime::{
        parse_mission_definition, parse_npc_definition, MissionScriptRuntime,
    };

    pub(crate) fn dead_npc(
        app: &mut App,
        id: i64,
        team: u8,
        position: Vec3,
        roster: bool,
    ) -> Entity {
        app.world_mut()
            .spawn((
                Fighter {
                    character_id: 1,
                    player_slot: 1,
                    weapon: 1,
                    weapon_slots: [1, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                MissionNpc {
                    id,
                    name: format!("npc{id}"),
                    team,
                    in_scene_roster: roster,
                    view: 500.0,
                    think: 100.0,
                    forced_behavior: None,
                    is_vip: false,
                    attack_target_slot: None,
                    behavior_args: Vec::new(),
                    attack_target_remaining: 0,
                },
                Transform::from_translation(position),
                DeathRespawn {
                    active: true,
                    timer: 3.0,
                    ..default()
                },
                Health {
                    current: 0.0,
                    max: 2000.0,
                },
                Qi::default(),
                CombatState {
                    state: meteor_core::FighterState::Dead,
                },
                PoseRuntime::idle(),
                SmState::default(),
                TargetHitClock::default(),
                (
                    EntryBlend::default(),
                    MotionState {
                        movement_mode: meteor_core::MovementMode::Idle,
                        velocity: Vec3::ZERO,
                        facing: 0.0,
                    },
                    FighterAir::default(),
                    FighterDodge::default(),
                    FighterAttackMotion::default(),
                    FighterHitReaction::default(),
                    GunAimState::default(),
                    AiIntent::default(),
                ),
            ))
            .id()
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn real_sn13_rule4_cure_revives_only_nearby_dead_ally_once() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
        let mission_text =
            crate::text_codec::decode_text(&std::fs::read(root.join("sn13_.pst")).unwrap());
        let ally_text =
            crate::text_codec::decode_text(&std::fs::read(root.join("npc13_01.pst")).unwrap());
        let enemy_text =
            crate::text_codec::decode_text(&std::fs::read(root.join("npc13_02.pst")).unwrap());
        let ally_team = parse_npc_definition(&ally_text, "npc13_01").team as u8;
        let enemy_team = parse_npc_definition(&enemy_text, "npc13_02").team as u8;
        assert_eq!((ally_team, enemy_team), (1, 2));
        let mission = MissionScriptRuntime::new(
            &mission_text,
            parse_mission_definition(&mission_text).values,
        );
        assert_eq!(mission.host().vip(1), Some(0));
        let mut app = App::new();
        app.insert_resource(crate::app_state::LaunchRequest {
            mode: "single".into(),
            ..default()
        })
        .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>()
        .init_resource::<NetState>()
        .init_resource::<MatchEnd>()
        .add_message::<CureCompleted>()
        .insert_resource(crate::plugins::stage_plugin::StageMissionRuntime::from_script(mission))
        .add_systems(Update, cure_mission_allies);
        let mut cure_pose = PoseRuntime::idle();
        cure_pose.pose_id = 22;
        cure_pose.pose_instance_id = 7;
        let healer = app
            .world_mut()
            .spawn((
                LocalFighter,
                Health {
                    current: 2000.0,
                    max: 2000.0,
                },
                DeathRespawn::default(),
                SmState {
                    current: meteor_core::FighterState::Cure,
                    ..default()
                },
                cure_pose,
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();
        let ally = dead_npc(&mut app, 1, ally_team, Vec3::new(9.5, 0.0, 0.0), true);
        let enemy = dead_npc(&mut app, 2, enemy_team, Vec3::new(1.0, 0.0, 0.0), true);
        let far_ally = dead_npc(&mut app, 3, ally_team, Vec3::new(9.7, 0.0, 0.0), true);
        let removed = dead_npc(&mut app, 4, ally_team, Vec3::new(1.0, 0.0, 0.0), false);
        app.update();
        assert!(
            app.world().get::<DeathRespawn>(ally).unwrap().active,
            "entering Pose22 does not commit Cure"
        );
        app.world_mut().write_message(CureCompleted {
            healer,
            position: Vec3::ZERO,
        });
        app.update();
        assert_eq!(app.world().get::<Health>(ally).unwrap().current, 2000.0);
        assert!(!app.world().get::<DeathRespawn>(ally).unwrap().active);
        assert_eq!(
            app.world().get::<SmState>(ally).unwrap().current,
            meteor_core::FighterState::Name
        );
        assert_eq!(
            app.world()
                .resource::<Messages<crate::plugins::effect_plugin::SceneEffectEvent>>()
                .len(),
            1
        );
        for untouched in [enemy, far_ally, removed] {
            assert!(app.world().get::<DeathRespawn>(untouched).unwrap().active);
        }
        app.world_mut()
            .get_mut::<DeathRespawn>(ally)
            .unwrap()
            .active = true;
        app.world_mut().get_mut::<Health>(ally).unwrap().current = 0.0;
        app.update();
        assert!(app.world().get::<DeathRespawn>(ally).unwrap().active);
    }
}
