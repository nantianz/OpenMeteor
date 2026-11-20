use super::*;
use crate::plugins::network_plugin::{
    NetState, NetStatus, StockSceneItemGrounded, StockSceneItemPickup,
};
use crate::plugins::stage_plugin::{tick_stage_script, StageScriptRuntime};
use meteor_scene::scene_script_runtime::{PstValue, SceneItemInstance, SceneScriptRuntime};

#[test]
fn stock_pickup_batches_stay_hidden_until_authoritative_respawn() {
    for player_id in ["stock-1", "stock-2"] {
        let instance = SceneItemInstance {
            name: "D_Item01".into(),
            index: 0,
            props: ["active", "visible", "interactive"]
                .into_iter()
                .map(|key| (key.into(), PstValue::Num(1.0)))
                .collect(),
        };
        let runtime =
            SceneScriptRuntime::load("Scene_OnLoad() {}", vec![instance], 10_000).unwrap();
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<ItemBuffs>()
            .init_resource::<crate::plugins::stage_collider::StageCollider>()
            .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>()
            .insert_resource(StageScriptRuntime::from_script(runtime))
            .insert_resource(NetState {
                status: NetStatus::InRoom,
                address: "udp://127.0.0.1:5230".into(),
                player_id: player_id.into(),
                ..Default::default()
            })
            .add_systems(
                Update,
                (apply_stock_scene_item_pickups, tick_stage_script).chain(),
            );
        for _ in 0..3 {
            app.world_mut().spawn((
                StageProp {
                    name: "D_Item01".into(),
                    model: "item01".into(),
                    center_offset: Vec3::ZERO,
                    half_extents: Vec3::ONE,
                    hit_count: 0,
                    respawn_timer: 0.0,
                    interactive: false,
                    collision_enabled: false,
                    breakable: false,
                    broken: false,
                },
                Transform::default(),
                GlobalTransform::IDENTITY,
                Visibility::Hidden,
            ));
        }
        let grounded = |record_id| StockSceneItemGrounded {
            record_id,
            category: 2,
            item_id: 1,
            position: [0.0; 3],
            spawned: true,
        };
        let taken = |record_id| StockSceneItemPickup {
            actor_id: 1,
            record_id,
            category: 2,
            item_id: 1,
            scene_reference: None,
            position: None,
        };
        let assert_state = |app: &mut App, record_id, available| {
            let mut query = app
                .world_mut()
                .query::<(&StageProp, &Visibility, &StockSceneItemView)>();
            assert_eq!(query.iter(app.world()).count(), 3);
            for (prop, visibility, view) in query.iter(app.world()) {
                assert_eq!(view.record_id, record_id);
                assert_eq!(view.available, available);
                assert_eq!(prop.interactive, available);
                assert_eq!(*visibility == Visibility::Visible, available);
            }
        };
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_stock_scene_grounded
            .push(grounded(44));
        app.update();
        assert_state(&mut app, 44, true);
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_stock_scene_items
            .push(taken(44));
        app.update();
        assert_state(&mut app, 44, false);

        for field in ["active", "interactive"] {
            app.world_mut()
                .resource_mut::<NetState>()
                .pending_scene_ops
                .push(meteor_network::protocol::SceneOp {
                    kind: "set".into(),
                    reference: serde_json::json!("D_Item01"),
                    field: field.into(),
                    args: vec![serde_json::json!(1)],
                });
        }
        app.update();
        assert_state(&mut app, 44, false);

        {
            let mut net = app.world_mut().resource_mut::<NetState>();
            net.pending_stock_scene_items.push(StockSceneItemPickup {
                position: Some([0.0; 3]),
                ..taken(44)
            });
            net.pending_stock_scene_grounded.push(grounded(45));
        }
        app.update();
        assert_state(&mut app, 45, true);
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_stock_scene_items
            .push(taken(45));
        app.update();
        assert_state(&mut app, 45, false);
    }
}

#[test]
fn stock_remote_food_and_concealment_apply_once_and_visual_effects_expire() {
    use crate::components::fighter::{Concealment, WornEffect};
    use crate::plugins::network_plugin::RemoteFighter;
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<ItemBuffs>()
        .init_resource::<StageScriptRuntime>()
        .init_resource::<crate::plugins::FrameClock>()
        .insert_resource(NetState::default())
        .add_systems(
            Update,
            (
                apply_stock_scene_item_pickups,
                tick_stock_remote_item_visuals,
            )
                .chain(),
        );
    let remote = app
        .world_mut()
        .spawn((
            RemoteFighter {
                player_id: "stock-2".into(),
            },
            Fighter {
                character_id: 0,
                player_slot: 1,
                weapon: 6,
                weapon_slots: [6, 0],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Health {
                current: 1000.0,
                max: 2000.0,
            },
            Qi::default(),
            Transform::default(),
        ))
        .id();
    for item_id in [1, 4, 7] {
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_stock_scene_items
            .push(StockSceneItemPickup {
                actor_id: 2,
                record_id: item_id,
                category: 2,
                item_id,
                scene_reference: None,
                position: None,
            });
    }
    app.update();
    assert_eq!(app.world().get::<Health>(remote).unwrap().current, 1300.0);
    assert_eq!(
        app.world().get::<Concealment>(remote).unwrap().remaining,
        60.0
    );
    assert!(app
        .world()
        .get::<WornEffect>(remote)
        .unwrap()
        .effects
        .iter()
        .any(|e| e.name == "PowUP"));
    app.world_mut()
        .resource_mut::<crate::plugins::FrameClock>()
        .sim_dt = 30.0;
    app.update();
    assert_eq!(app.world().get::<Health>(remote).unwrap().current, 1300.0);
    assert_eq!(
        app.world().get::<Concealment>(remote).unwrap().remaining,
        30.0
    );
    app.update();
    assert_eq!(
        app.world().get::<Concealment>(remote).unwrap().remaining,
        0.0
    );
    assert!(app
        .world()
        .get::<WornEffect>(remote)
        .unwrap()
        .effects
        .is_empty());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn stock_room_rule_filtered_generators_stay_hidden_after_script_sync() {
    let instance = SceneItemInstance {
        name: "D_Item04".into(),
        index: 0,
        props: ["active", "visible", "interactive"]
            .into_iter()
            .map(|key| (key.into(), PstValue::Num(1.0)))
            .collect(),
    };
    let runtime = SceneScriptRuntime::load("Scene_OnLoad() {}", vec![instance], 10_000).unwrap();
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<ItemBuffs>()
        .init_resource::<StockSceneManifestState>()
        .init_resource::<crate::plugins::stage_collider::StageCollider>()
        .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>()
        .insert_resource(StageScriptRuntime::from_script(runtime))
        .insert_resource(NetState {
            status: NetStatus::InRoom,
            address: "udp://127.0.0.1:5230".into(),
            ..Default::default()
        })
        .add_systems(
            Update,
            (initialize_stock_scene_items, tick_stage_script).chain(),
        );
    let prop = app
        .world_mut()
        .spawn((
            StageProp {
                name: "D_Item04".into(),
                model: "item04".into(),
                center_offset: Vec3::ZERO,
                half_extents: Vec3::ONE,
                hit_count: 0,
                respawn_timer: 0.0,
                interactive: true,
                collision_enabled: false,
                breakable: false,
                broken: false,
            },
            Transform::default(),
            GlobalTransform::IDENTITY,
            Visibility::Visible,
        ))
        .id();

    app.update();

    assert!(!app.world().get::<StageProp>(prop).unwrap().interactive);
    assert_eq!(
        *app.world().get::<Visibility>(prop).unwrap(),
        Visibility::Hidden
    );
    assert!(app
        .world()
        .get::<StockSceneItemView>(prop)
        .is_some_and(|view| !view.available));
}

#[test]
fn stock_event45_disables_only_the_authored_object_with_the_matching_index() {
    let mut app = App::new();
    let instances = ["D_Item01", "D_Item02"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| SceneItemInstance {
            name: name.into(),
            index,
            props: [("active".into(), PstValue::Num(1.0))].into(),
        })
        .collect();
    let script = SceneScriptRuntime::load("Scene_OnLoad() {}", instances, 10_000).unwrap();
    app.init_resource::<ItemBuffs>()
        .insert_resource(StageScriptRuntime::from_script(script))
        .insert_resource(NetState::default())
        .add_systems(Update, apply_stock_scene_item_pickups);
    let mut spawn = |name: &str| {
        app.world_mut()
            .spawn((
                StageProp {
                    name: name.into(),
                    model: "item01".into(),
                    center_offset: Vec3::ZERO,
                    half_extents: Vec3::ONE,
                    hit_count: 0,
                    respawn_timer: 0.0,
                    interactive: true,
                    collision_enabled: false,
                    breakable: false,
                    broken: false,
                },
                Transform::default(),
                Visibility::Visible,
            ))
            .id()
    };
    let first = spawn("D_Item01");
    let second = spawn("D_Item02");
    app.world_mut()
        .resource_mut::<NetState>()
        .pending_stock_scene_items
        .push(StockSceneItemPickup {
            actor_id: 1,
            record_id: u32::MAX,
            category: 2,
            item_id: 1,
            scene_reference: Some(1),
            position: None,
        });
    app.update();
    assert_eq!(
        app.world().get::<Visibility>(first),
        Some(&Visibility::Visible)
    );
    assert_eq!(
        app.world().get::<Visibility>(second),
        Some(&Visibility::Hidden)
    );
    assert!(app.world().get::<StageProp>(first).unwrap().interactive);
    assert!(!app.world().get::<StageProp>(second).unwrap().interactive);
}
