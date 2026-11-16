use super::*;

pub(super) fn recover(
    patrol: &mut NpcPatrol,
    intent: &mut AiIntent,
    facing: f32,
    target: Vec3,
    position: Vec3,
    look: f32,
    think: f32,
) -> Option<f32> {
    let roll = (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
    if roll % 4 == 0 {
        patrol.think_remaining = meteor_ai::combat::decision_interval(think, 0.8);
        let yaw = yaw_towards(target - position);
        if yaw.is_none_or(|yaw| npc_facing_error(facing, yaw).abs() <= 5.0_f32.to_radians()) {
            intent.want_taunt = true;
        }
        return yaw;
    }

    let mut heading = patrol.idle_facing.unwrap_or(facing);
    patrol.stop_navigation();
    patrol.navigation_reset_pending = false;
    patrol.combat_spacing = None;
    patrol
        .perception
        .return_from_failed_pursuit(intent.enemy_id);
    intent.lock_target = None;
    intent.ranged_aim = None;
    let look_roll = (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
    if ((look_roll % 100) as f32) < look {
        let angle_roll = (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32;
        heading = (angle_roll % 360) as f32 * std::f32::consts::PI / 180.0;
    }
    patrol.idle_facing = Some(heading);
    patrol.think_remaining = meteor_ai::combat::decision_interval(think, 0.4);
    Some(heading)
}
