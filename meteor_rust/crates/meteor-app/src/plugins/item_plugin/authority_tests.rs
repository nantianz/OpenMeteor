use super::*;
use crate::components::fighter::{Concealment, MotionState};
use crate::plugins::network_plugin::{NetState, NetStatus};
use crate::plugins::stage_plugin::LocalFighter;

fn authority_test_buffs() -> ItemBuffs {
    let mut buffs = ItemBuffs::default();
    for (id, health, anger) in [(14, -10, 0), (39, 0, 3)] {
        buffs.periodic_effects.apply(
            meteor_asset::res::HitStatusEffect {
                id,
                duration: 1.0,
                tick_every: 0.015,
                defer_first_tick: true,
                health,
                anger,
                speed_percent: None,
                attack_bonus: 0,
                defense_bonus: 0,
                max_health_bonus: 0,
                concealed: false,
                worn_effect: None,
            },
            0.0,
        );
    }
    buffs.stat_buffs.push(TimedItemBuff {
        model: "fixture".into(),
        attack: 0,
        defense: 0,
        max_hp: 20,
        speed: 1.0,
        remaining: 0.01,
    });
    buffs.project_stats();
    buffs.project_periodic();
    buffs
}

#[test]
fn stock_item_timers_leave_vitals_and_death_to_authority_but_offline_poison_still_kills() {
    for online in [false, true] {
        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(NetState {
                status: if online {
                    NetStatus::InRoom
                } else {
                    NetStatus::Disconnected
                },
                address: "udp://127.0.0.1:5230".into(),
                ..Default::default()
            })
            .insert_resource(authority_test_buffs())
            .add_message::<crate::plugins::render_plugin::OfflineWeaponDeathDrop>()
            .add_systems(Update, tick_item_buffs);
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
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                Health {
                    current: 5.0,
                    max: 100.0,
                },
                Qi::default(),
                DeathRespawn::default(),
                SmState::default(),
                CombatState::default(),
                PoseRuntime::idle(),
                Concealment { remaining: 1.0 },
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(20));
        app.update();
        let health = app.world().get::<Health>(actor).unwrap();
        assert_eq!(health.current, if online { 5.0 } else { 0.0 });
        assert_eq!(health.max, if online { 100.0 } else { 80.0 });
        assert_eq!(
            app.world().get::<Qi>(actor).unwrap().current,
            if online { 0.0 } else { 3.0 }
        );
        assert_eq!(
            app.world().get::<DeathRespawn>(actor).unwrap().active,
            !online
        );
        assert!(app.world().get::<Concealment>(actor).unwrap().remaining < 1.0);
        assert!(app.world().resource::<ItemBuffs>().poison_total < 1.0);
    }
}
