use super::*;
use crate::plugins::network_plugin::{NetState, NetStatus};
use bevy::ecs::system::RunSystemOnce;
use meteor_network::protocol::RoomMemberWire;

#[cfg(feature = "original-assets")]
#[test]
fn stock_spawn_pad_initial_placement_randomizes_within_authored_group() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/sn03/sn03.des"),
    )
    .unwrap();
    let desc = meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(&bytes));
    for camp in 0..=2 {
        let mut app = App::new();
        app.insert_resource(LaunchRequest {
            mode: "leader".into(),
            ..Default::default()
        })
        .insert_resource(LoadedStageAssets {
            des: Some(bytes.clone()),
            ..Default::default()
        })
        .insert_resource(LoadedStageInfo::default())
        .init_resource::<StageMissionRuntime>()
        .insert_resource(NetState {
            status: NetStatus::InRoom,
            address: "udp://127.0.0.1:5230".into(),
            player_id: "stock-1".into(),
            members: vec![RoomMemberWire {
                player_id: "stock-1".into(),
                name: "test".into(),
                character_id: 0,
                weapon: 6,
                team: (camp != 0).then_some(camp),
                ready: true,
                connected: true,
            }],
            ..Default::default()
        });
        for _ in 0..64 {
            app.world_mut()
                .run_system_once(spawn_local_fighter)
                .unwrap();
        }
        let pads = stock_spawn_pads(&desc, camp, [0.0; 3]);
        let positions: Vec<_> = app
            .world_mut()
            .query_filtered::<&Transform, With<LocalFighter>>()
            .iter(app.world())
            .map(|t| t.translation)
            .collect();
        assert_eq!(positions.len(), 64);
        assert!(positions
            .iter()
            .all(|p| pads.iter().any(|(pad, _)| p.distance(*pad) < 1e-4)));
        assert!(
            positions.iter().any(|p| p.distance(positions[0]) > 1.0),
            "roster slot must not fix the pad"
        );
    }
}

#[test]
fn stock_spawn_pad_keeps_resource_order_even_when_names_are_out_of_order() {
    let desc = meteor_asset::des::parse_stage_description("SceneObjects 0 DummeyObjects 3\nObject D_A\n{\nPosition: 9 0 0\nCustom:\n{\nname=\"teamA09\"\n}\n}\nObject D_B\n{\nPosition: 1 0 0\nCustom:\n{\nname=\"teamA01\"\n}\n}\nObject D_C\n{\nPosition: 2 0 0\nCustom:\n{\nname=\"user01\"\n}\n}\n");
    let pads = stock_spawn_pads(&desc, 1, [0.0; 3]);
    assert_eq!(pads.len(), 2);
    assert!(pads[0].0.x > pads[1].0.x);
    assert_eq!(meteor_network::spawn::counts(&desc), [1, 2, 0]);
}
