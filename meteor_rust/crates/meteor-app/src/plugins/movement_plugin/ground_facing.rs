use super::*;

#[allow(clippy::type_complexity)]
pub(crate) fn finalize_ground_lock_facing(
    active: Option<Res<super::super::fighter_frame::ActiveFighter>>,
    playback: Option<Res<crate::plugins::replay_plugin::MrcPlayback>>,
    mut lock: ResMut<LockOnController>,
    mut basis: ResMut<CameraBasis>,
    mut players: Query<(Entity, &Transform, &mut MotionState, &mut FighterAir), With<LocalFighter>>,
    targets: Query<(&Transform, Option<&MotionState>), Without<LocalFighter>>,
) {
    for (entity, position, mut motion, mut air) in &mut players {
        if !super::super::fighter_frame::accepts(active.as_deref(), entity) {
            continue;
        }
        if !std::mem::take(&mut air.ground_lock_facing_pending)
            || playback.as_ref().is_some_and(|p| p.active)
        {
            continue;
        }
        let Some((target, target_motion)) = lock.lock_entity.and_then(|e| targets.get(e).ok())
        else {
            continue;
        };
        let player = position.translation.to_array().into();
        let target = target.translation.to_array().into();
        if meteor_combat::lock_on::locked_attack_gate_failed(player, target) {
            lock.system.clear_lock();
            lock.lock_entity = None;
            basis.yaw = motion.facing;
        } else if let Some(yaw) = meteor_combat::lock_on::locked_facing_yaw(
            player,
            target,
            target_motion.map(|m| m.facing),
        ) {
            motion.facing = yaw;
        }
    }
}
