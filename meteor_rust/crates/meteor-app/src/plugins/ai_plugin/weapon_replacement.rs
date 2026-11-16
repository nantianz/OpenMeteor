use super::*;

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct NpcWeaponDropRequest {
    pub spare: (u8, u8),
    pub position: Vec3,
    pub facing: f32,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn admit_weapon_getitem(
    patrol: &mut NpcPatrol,
    fighter: &Fighter,
    candidate: (u8, u8),
    position: Vec3,
    target: Vec3,
    graph: Option<&WaypointGraph>,
    mesh: Option<&StageMeshCollider>,
    _dt: f32,
    offline: bool,
) -> Result<Option<NpcWeaponDropRequest>, ()> {
    if fighter
        .weapon_slots
        .iter()
        .zip(fighter.weapon_slot_variants)
        .any(|(&weapon, variant)| (weapon, variant) == candidate)
    {
        return Err(());
    }

    let step = patrol.travel_step(graph, mesh, position, target, true);
    if step.failed
        || patrol.navigation.goal().is_none_or(|goal| {
            goal.distance_squared(target) > (50.0 * meteor_core::MODEL_SCALE).powi(2)
        })
    {
        return Err(());
    }
    let slot = 1 - usize::from(fighter.weapon_slot.min(1));
    let spare = (
        fighter.weapon_slots[slot],
        fighter.weapon_slot_variants[slot],
    );
    if spare.0 == 0 {
        return Ok(None);
    }

    if !offline {
        return Err(());
    }
    let roll = (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
    if roll % 2 != 0 {
        return Err(());
    }
    Ok(Some(NpcWeaponDropRequest {
        spare,
        position,
        facing: 0.0,
    }))
}
