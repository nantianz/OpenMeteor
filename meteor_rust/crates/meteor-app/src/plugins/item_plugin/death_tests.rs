use super::*;
use crate::components::fighter::{Concealment, MotionState, WornEffect};
use crate::plugins::{
    network_plugin::{NetState, NetStatus},
    stage_plugin::LocalFighter,
};

#[test]
fn death_reverts_local_max_hp_once_and_preserves_authoritative_vitals() {
    for online in [false, true] {
        for tick_first in [false, true] {
            let mut app = App::new();
            app.init_resource::<Time>()
                .insert_resource(NetState {
                    status: if online {
                        NetStatus::InRoom
                    } else {
                        NetStatus::Disconnected
                    },
                    ..default()
                })
                .insert_resource(ItemBuffs {
                    max_hp_bonus: 500,
                    max_hp_remaining: 600.0,
                    speed_factor: 1.5,
                    ..default()
                })
                .add_message::<crate::plugins::render_plugin::OfflineWeaponDeathDrop>();
            if tick_first {
                app.add_systems(Update, (tick_item_buffs, clear_dead_item_effects).chain());
            } else {
                app.add_systems(Update, (clear_dead_item_effects, tick_item_buffs).chain());
            }
            let actor = app
                .world_mut()
                .spawn((
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
                    MotionState {
                        velocity: Vec3::ZERO,
                        facing: 0.0,
                        movement_mode: meteor_core::MovementMode::Idle,
                    },
                    Health {
                        current: 0.0,
                        max: 1500.0,
                    },
                    Qi::default(),
                    DeathRespawn {
                        active: true,
                        ..default()
                    },
                    SmState::default(),
                    CombatState::default(),
                    PoseRuntime::idle(),
                    Concealment { remaining: 30.0 },
                ))
                .id();
            app.world_mut()
                .get_mut::<WornEffect>(actor)
                .unwrap()
                .refresh("MaxLifUP", 600.0);
            for _ in 0..3 {
                app.update();
            }
            let expected = if online { 1500.0 } else { 1000.0 };
            assert_eq!(app.world().get::<Health>(actor).unwrap().max, expected);
            assert_eq!(app.world().resource::<ItemBuffs>().max_hp_bonus, 0);
            assert_eq!(app.world().resource::<ItemBuffs>().speed_factor, 1.0);
            assert!(app
                .world()
                .get::<WornEffect>(actor)
                .unwrap()
                .effects
                .is_empty());

            app.world_mut()
                .get_mut::<DeathRespawn>(actor)
                .unwrap()
                .active = false;
            app.world_mut().get_mut::<Health>(actor).unwrap().current = expected;
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs(601));
            app.update();
            assert_eq!(app.world().get::<Health>(actor).unwrap().max, expected);
            assert_eq!(app.world().get::<Health>(actor).unwrap().current, expected);
        }
    }
}
