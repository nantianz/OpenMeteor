#[test]
fn prop5_driver_defaults_zero_only_after_weapon_item_and_status_sum() {
    let resources = CombatResourceData(
        meteor_asset::res::parse_meteor_res(
            &std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../origin_res/Meteor.res"),
            )
            .unwrap(),
        )
        .unwrap()
        .combat_table(),
    );
    let mut fighter = Fighter {
        character_id: 0,
        player_slot: 0,
        weapon: 6,
        weapon_slots: [6, 5],
        weapon_slot: 0,
        weapon_slot_variants: [3, 0],
        weapon_variant: 3,
    };
    assert!((fighter_speed_factor(Some(&resources), &fighter, 1.0, None) - 1.19).abs() < 1e-6);
    let mut statuses = crate::components::fighter::FighterStatusEffects::default();
    let mut definition = resources.0.status_effect(10).unwrap();
    definition.speed_percent = Some(0);
    statuses.apply(definition);

    assert!(
        (fighter_speed_factor(Some(&resources), &fighter, 1.0, Some(&statuses)) - 0.19).abs()
            < 1e-6
    );
    assert_eq!(statuses.effects.speed_factor(1.0), 0.0);
    fighter.weapon_variant = 0;
    assert_eq!(
        fighter_speed_factor(Some(&resources), &fighter, 1.0, Some(&statuses)),
        1.0
    );
    assert_eq!(
        fighter_speed_factor(Some(&resources), &fighter, 0.5, Some(&statuses)),
        1.0
    );
    assert_eq!(fighter_speed_factor(None, &fighter, 0.0, None), 1.0);
    assert!((fighter_speed_factor(None, &fighter, 0.01, None) - 0.01).abs() < 1e-6);
}

#[test]
fn prop5_status_segments_truncate_independently_and_expire_without_requantizing() {
    let table = meteor_asset::res::parse_meteor_res(
        &std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Meteor.res"),
        )
        .unwrap(),
    )
    .unwrap()
    .combat_table();
    let mut statuses = meteor_combat::status_effects::HitStatusEffects::default();
    let mut definition = table.status_effect(10).unwrap();
    definition.speed_percent = Some(120);
    definition.duration = 1.0;
    statuses.apply(definition, 0.0);
    definition.id += 100;
    definition.duration = 2.0;
    statuses.apply(definition, 0.0);
    assert_eq!(statuses.speed_property(100), 138);
    statuses.tick(1.0);
    assert_eq!(statuses.speed_property(100), 119);
    statuses.tick(2.0);
    assert_eq!(statuses.speed_property(100), 100);
}
