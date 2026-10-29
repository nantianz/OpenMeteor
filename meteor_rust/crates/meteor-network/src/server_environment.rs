use super::*;
use meteor_scene::hazards::{build_volumes, StageHazards};

pub(super) fn load_hazards(root: Option<&Path>, stage: &str) -> StageHazards {
    let mut hazards = StageHazards::new();
    let Some(root) = root else { return hazards };

    if !meteor_scene::stage_registry::is_official_stage_id(stage) {
        return hazards;
    }
    let paths = meteor_scene::stage_loader::stage_asset_paths(stage);
    let loaded = (|| -> Result<_, String> {
        let des = read_resource(root, &paths.des).map_err(|e| e.to_string())?;
        let text = std::str::from_utf8(&des)
            .map(std::borrow::Cow::Borrowed)
            .unwrap_or_else(|_| encoding_rs::GB18030.decode(&des).0);
        let desc = meteor_asset::des::parse_stage_description(&text);
        let gmb = read_resource(root, &paths.gmb).map_err(|e| e.to_string())?;
        let mesh = meteor_asset::gmdl::decode_gmdl(&gmb).map_err(|e| e.to_string())?;
        Ok(build_volumes(
            &mesh,
            &desc,
            meteor_scene::stage_loader::get_stage_center(stage, None),
        ))
    })();
    match loaded {
        Ok(volumes) => hazards.build_from_stage(volumes, Vec::new()),
        Err(error) => log::warn!("WS environment resources for {stage}: {error}"),
    }
    hazards
}

pub(super) fn update_hazards(room: &mut Room, dt: f32) -> Vec<Message> {
    if !room
        .round
        .as_ref()
        .is_some_and(|round| round.phase == RoundPhase::Active)
    {
        return Vec::new();
    }
    let actors = room
        .combatants
        .iter()
        .filter(|(id, _)| {
            room.members.get(*id).is_some_and(|member| member.connected)
                && room.role_states.contains_key(*id)
        })
        .map(|(id, actor)| {
            let p = actor.foot_position();
            (
                id.clone(),
                [p.x, p.y, p.z],
                !actor.alive(),
                actor.target_hit_clock.spawn_protected(),
            )
        })
        .collect::<Vec<_>>();
    let mut hits = Vec::new();
    room.hazards.update(dt, &actors, &mut |id, _, damage, _| {
        hits.push((id.to_owned(), damage))
    });
    let mut events = Vec::new();
    for (id, damage) in hits {
        let Some(actor) = room.combatants.get_mut(&id) else {
            continue;
        };
        if !actor.alive() || damage <= 0 || actor.target_hit_clock.spawn_protected() {
            continue;
        }
        let damage = (damage as f32).min(actor.health);
        actor.health -= damage;
        actor.target_hit_clock.alive = actor.alive();
        events.push(Message::CombatHit {
            target: id.clone(),
            damage,
            attacker_id: None,
            guarded: Some(false),
            health: Some(actor.health),
            anger: Some(actor.anger),
            motion_scale: None,
            reaction: None,
            tick: Some(room.authority_tick),
        });
        if actor.alive() {
            continue;
        }
        actor.deaths = actor.deaths.saturating_add(1);
        actor.respawn_at =
            can_respawn(mode_rule_id(&room.mode)).then_some(room.elapsed + RESPAWN_DELAY_SECONDS);
        let respawn_seconds = actor.respawn_at.map(|_| RESPAWN_DELAY_SECONDS);
        let vip_winner = (mode_rule_id(&room.mode) == Some(4) && actor.is_leader)
            .then_some(actor.team)
            .flatten()
            .and_then(|team| match team {
                1 => Some(2),
                2 => Some(1),
                _ => None,
            });
        let weapon = actor.active_weapon().cloned();
        actor.clear_item_effects();
        if let Some(winner_team) = vip_winner {
            room.pending_round_end.get_or_insert(PendingRoundEnd {
                winner_team: Some(winner_team),
                reason: "vip",
            });
        }
        if let Some(weapon) = weapon {
            if let Some(drop) =
                spawn_dynamic_weapon_drop(room, &id, weapon, DynamicWeaponDropSource::Death)
            {
                events.push(drop);
            }
        }
        events.push(Message::CombatDeath {
            attacker_id: None,
            target_id: id.clone(),
            respawn_seconds,
            tick: Some(room.authority_tick),
        });
        if room
            .escort_flag
            .as_ref()
            .is_some_and(|flag| flag.carrier_id.as_deref() == Some(&id))
        {
            if let Some(drop) = RoomServer::drop_escort_flag(room) {
                events.push(drop);
            }
        }
    }
    events
}

pub(super) fn read_resource(root: &Path, path: &str) -> std::io::Result<Vec<u8>> {
    #[cfg(test)]
    if root.is_absolute() {
        return std::fs::read(root.join(path));
    }
    let _ = root;
    meteor_asset::runtime_assets::read_file(path)
}
