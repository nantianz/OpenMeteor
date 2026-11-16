use super::*;
use crate::components::fighter::*;
use crate::plugins::camera_plugin::CameraBasis;
use crate::plugins::item_plugin::{tick_item_buffs, ItemBuffs};
use crate::plugins::movement_plugin::{
    move_local_fighter, FighterAir, FighterAttackMotion, FighterDodge, FighterWall,
};
use crate::plugins::stage_collider::{StageCollider, StageMeshCollider};

#[test]
fn stock_environment_respawn_survives_late_zero_vitals_and_can_move() {
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<crate::app_state::Keymap>()
        .init_resource::<crate::plugins::input_plugin::LocalCrouchInput>()
        .init_resource::<LockOnController>()
        .init_resource::<CameraBasis>()
        .init_resource::<ItemBuffs>()
        .insert_resource(StageCollider {
            mesh: Some(StageMeshCollider::default()),
        })
        .insert_resource(NetState {
            status: NetStatus::InRoom,
            wire_format: NetWireFormat::V4,
            address: "udp://127.0.0.1:5230".into(),
            player_id: "stock-1".into(),
            round_active: true,
            ..Default::default()
        })
        .add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>()
        .add_message::<crate::plugins::render_plugin::OfflineWeaponDeathDrop>()
        .add_systems(
            Update,
            (
                net_apply_stock_vitals,
                net_handle_hit,
                net_apply_authority_respawns,
                tick_item_buffs,
                move_local_fighter,
            )
                .chain(),
        );
    let actor = app
        .world_mut()
        .spawn((
            (
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 6,
                    weapon_slots: [6, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::default(),
                Health::default(),
                Qi::default(),
                DeathRespawn::default(),
                PoseRuntime::idle(),
                EntryBlend::default(),
                CombatState::default(),
                SmState::default(),
                TargetHitClock::default(),
            ),
            (
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                FighterAir::default(),
                FighterDodge::default(),
                FighterWall::default(),
                FighterAttackMotion::default(),
            ),
        ))
        .id();
    let mut request = LaunchRequest {
        stock_rule: 1,
        ..Default::default()
    };
    let mut end = MatchEnd::default();
    let mut tx = NetTx::default();
    let mut next = NextState::default();

    handle_stock_game_event(
        &mut app.world_mut().resource_mut::<NetState>(),
        &mut request,
        &mut end,
        &mut tx,
        &mut next,
        AppState::Playing,
        StockGameEvent::EnvironmentDeath(1),
    );
    app.update();
    assert!(app.world().get::<DeathRespawn>(actor).unwrap().active);
    handle_stock_game_event(
        &mut app.world_mut().resource_mut::<NetState>(),
        &mut request,
        &mut end,
        &mut tx,
        &mut next,
        AppState::Playing,
        StockGameEvent::LocalRespawn,
    );
    app.update();
    assert!(!app.world().get::<DeathRespawn>(actor).unwrap().active);

    let max = app.world().get::<Health>(actor).unwrap().max;
    app.world_mut()
        .resource_mut::<NetState>()
        .pending_stock_vitals = Some([0.0, max, 0.0]);
    app.update();
    assert!(
        !app.world().get::<DeathRespawn>(actor).unwrap().active,
        "a blood sample must not create a second local death after server revival"
    );
    app.world_mut()
        .resource_mut::<NetState>()
        .pending_stock_vitals = Some([max, max, 0.0]);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);
    let before = app.world().get::<Transform>(actor).unwrap().translation;
    for _ in 0..10 {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(20));
        app.update();
    }
    let after = app.world().get::<Transform>(actor).unwrap().translation;
    assert!(
        (after - before).with_y(0.0).length() > 0.01,
        "direction input must move the revived actor"
    );
    assert!(!app.world().get::<DeathRespawn>(actor).unwrap().active);
    assert!(app
        .world()
        .get::<FighterHitReaction>(actor)
        .unwrap()
        .current
        .is_none());
}

#[test]
fn stock_environment_death_clears_remote_death_so_observers_can_attack_again() {
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<crate::plugins::item_plugin::ItemBuffs>()
        .insert_resource(NetState {
            status: NetStatus::InRoom,
            wire_format: NetWireFormat::V4,
            address: "udp://127.0.0.1:5230".into(),
            player_id: "stock-1".into(),
            round_active: true,
            ..Default::default()
        })
        .add_systems(Update, net_apply_remote_vitals);
    let remote = app
        .world_mut()
        .spawn((
            RemoteFighter {
                player_id: "stock-2".into(),
            },
            RemoteVitalsStatus {
                health_known: true,
                anger_known: false,
            },
            Health {
                current: 0.0,
                max: 100.0,
            },
            Qi::default(),
            DeathRespawn {
                active: true,
                timer: 0.0,
                spawn_pos: Vec3::ZERO,
            },
            TargetHitClock::default(),
        ))
        .id();
    let mut request = LaunchRequest {
        stock_rule: 3,
        ..Default::default()
    };
    let mut end = MatchEnd::default();
    let mut tx = NetTx::default();
    let mut next = NextState::default();

    handle_stock_game_event(
        &mut app.world_mut().resource_mut::<NetState>(),
        &mut request,
        &mut end,
        &mut tx,
        &mut next,
        AppState::Playing,
        StockGameEvent::Respawn(meteor_network::stock_protocol::StockRespawnEvent { actor_id: 2 }),
    );
    app.update();
    assert!(
        !app.world().get::<DeathRespawn>(remote).unwrap().active,
        "event 46 must clear observer DeathRespawn so the revived host is attackable"
    );

    app.world_mut()
        .resource_mut::<NetState>()
        .pending_remote_vitals
        .insert("stock-2".into(), [0.0, 100.0, 0.0]);
    app.update();
    assert!(!app.world().get::<DeathRespawn>(remote).unwrap().active);
    assert_eq!(app.world().get::<Health>(remote).unwrap().current, 0.0);

    app.world_mut()
        .resource_mut::<NetState>()
        .pending_remote_vitals
        .insert("stock-2".into(), [100.0, 100.0, 0.0]);
    app.world_mut()
        .get_mut::<DeathRespawn>(remote)
        .unwrap()
        .active = true;
    app.update();
    assert!(
        !app.world().get::<DeathRespawn>(remote).unwrap().active,
        "full-health vitals must also clear a stuck remote death flag"
    );
    assert_eq!(app.world().get::<Health>(remote).unwrap().current, 100.0);
}
