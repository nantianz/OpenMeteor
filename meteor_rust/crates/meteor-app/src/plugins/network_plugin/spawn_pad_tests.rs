use super::*;
use crate::app_state::{LoadedStageAssets, LoadedStageInfo};
use crate::plugins::camera_plugin::CameraBasis;
use crate::plugins::item_plugin::ItemBuffs;
use meteor_network::spawn::SpawnPad;

fn actor(app: &mut App, local: bool) -> Entity {
    let mut e = app.world_mut().spawn((
        (
            Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 6,
                weapon_slots: [6, 0],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Health {
                current: 0.0,
                max: 1000.0,
            },
            Qi::default(),
            CombatState::default(),
            PoseRuntime::idle(),
            SmState::default(),
            DeathRespawn {
                active: true,
                ..Default::default()
            },
            TargetHitClock::default(),
            Transform::from_xyz(99.0, 99.0, 99.0),
        ),
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
        ),
    ));
    if local {
        e.insert(LocalFighter);
    }
    e.id()
}

#[cfg(feature = "original-assets")]
#[test]
fn stock_spawn_pad_owner_ecs_honors_index_waits_for_assets_and_preserves_empty() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/sn03/sn03.des"),
    )
    .unwrap();
    let desc = meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(&bytes));
    let mut app = App::new();
    app.init_resource::<LockOnController>()
        .init_resource::<CameraBasis>()
        .init_resource::<ItemBuffs>()
        .insert_resource(NetState {
            status: NetStatus::InRoom,
            wire_format: NetWireFormat::V4,
            address: "udp://127.0.0.1:5230".into(),
            player_id: "stock-1".into(),
            ..Default::default()
        })
        .add_systems(Update, net_apply_authority_respawns);
    let player = actor(&mut app, true);
    let npc = actor(&mut app, false);
    app.world_mut()
        .resource_mut::<NetState>()
        .stock_npc_actors
        .insert(npc, 2);
    for (id, group, index) in [("stock-1", 1, 5), ("stock-2", 2, 3)] {
        let mut net = app.world_mut().resource_mut::<NetState>();
        net.pending_respawns.push(id.into());
        net.pending_spawn_pads
            .insert(id.into(), Some(SpawnPad { group, index }));
    }
    app.update();
    assert_eq!(
        app.world().resource::<NetState>().pending_spawn_pads.len(),
        2
    );
    assert_eq!(app.world().get::<Health>(player).unwrap().current, 0.0);
    app.insert_resource(LoadedStageAssets {
        des: Some(bytes),
        ..Default::default()
    })
    .insert_resource(LoadedStageInfo::default());
    app.insert_resource(NextState::Pending(AppState::Loading));
    app.update();
    assert_eq!(
        app.world().resource::<NetState>().pending_spawn_pads.len(),
        2,
        "Rule 3 loading must retain owner pads even while old scene assets are present"
    );
    app.insert_resource(NextState::<AppState>::Unchanged);
    for (entity, group, index) in [(player, 1, 5), (npc, 2, 3)] {
        app.update();
        let p = meteor_network::spawn::pads(&desc, group)
            .nth(index)
            .unwrap()
            .position;
        let actual = app.world().get::<Transform>(entity).unwrap().translation;
        assert!(
            actual.distance(Vec3::new(p.x, p.z, -p.y) * meteor_core::constants::MODEL_SCALE) < 1e-4
        );
        assert_eq!(app.world().get::<Health>(entity).unwrap().current, 1000.0);
        assert!(!app.world().get::<DeathRespawn>(entity).unwrap().active);
    }
    assert!(app
        .world()
        .resource::<NetState>()
        .pending_spawn_pads
        .is_empty());
    let before = app.world().get::<Transform>(player).unwrap().translation;
    let mut request = LaunchRequest::default();
    let mut end = MatchEnd::default();
    let mut tx = NetTx::default();
    let mut next = NextState::default();
    for event in [StockGameEvent::RoundRestart, StockGameEvent::SpawnPad(None)] {
        handle_stock_game_event(
            &mut app.world_mut().resource_mut::<NetState>(),
            &mut request,
            &mut end,
            &mut tx,
            &mut next,
            AppState::Playing,
            event,
        );
        app.update();
        assert_eq!(
            app.world().get::<Transform>(player).unwrap().translation,
            before
        );
    }
}
