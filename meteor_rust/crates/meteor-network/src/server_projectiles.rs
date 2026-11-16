use super::*;

pub(super) fn submit(
    room: &mut Room,
    player: &str,
    catalog: Option<&AttackCatalog>,
    mut state: crate::runtime_sync::ProjectileState,
) -> Option<Message> {
    let actor = room.combatants.get(player)?;
    if !state.valid() {
        return None;
    }
    state.player_id = player.to_owned();
    if state.active && !room.projectiles.known(player, state.id) {
        let member = room.members.get(player)?;
        let weapon = actor.active_weapon();
        if !actor.alive()
            || state.weapon != weapon.map_or(member.weapon, |weapon| weapon.weapon_id)
            || state.variant != weapon.map_or(0, |weapon| weapon.variant)
            || !crate::runtime_sync::valid_projectile_birth(catalog?, member.character_id, &state)
        {
            return None;
        }
    }
    if !room
        .projectiles
        .accept(state.clone(), (room.elapsed * 1000.0) as u64)
    {
        return None;
    }
    Some(message(room, state))
}
fn message(room: &Room, state: crate::runtime_sync::ProjectileState) -> Message {
    Message::ProjectileState {
        room_id: room.id.clone(),
        match_epoch: room.match_epoch,
        round_epoch: room.round_epoch,
        state,
    }
}
pub(super) fn bootstrap(room: &Room, player: &str) -> Vec<Message> {
    if !room.projectile_subscribers.contains(player)
        || !room
            .round
            .as_ref()
            .is_some_and(|round| round.phase == RoundPhase::Active)
    {
        return Vec::new();
    }
    room.projectiles
        .active()
        .filter(|state| state.player_id != player)
        .cloned()
        .map(|state| message(room, state))
        .collect()
}
pub(super) fn update(room: &mut Room) -> Vec<Message> {
    let now = (room.elapsed * 1000.0) as u64;
    let gone = room
        .projectiles
        .active()
        .filter(|state| {
            !room
                .members
                .get(&state.player_id)
                .is_some_and(|m| m.connected)
                || !room.combatants.contains_key(&state.player_id)
        })
        .map(|state| state.player_id.clone())
        .collect::<HashSet<_>>();
    let mut states = room.projectiles.expire(now);
    for player in gone {
        states.extend(room.projectiles.retire_owner(&player, now));
    }
    states
        .into_iter()
        .map(|state| message(room, state))
        .collect()
}
