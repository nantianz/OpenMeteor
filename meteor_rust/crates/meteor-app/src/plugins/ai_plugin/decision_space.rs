use super::*;

#[derive(Clone, Copy)]
pub(super) struct DecisionSpace<'a> {
    pub position: Vec3,
    pub facing: f32,
    pub ground_controllable: bool,
    pub mesh: Option<&'a StageMeshCollider>,
}

pub(super) fn world_input(input: Vec2, facing: f32) -> Vec2 {
    let (sin, cos) = facing.sin_cos();
    Vec2::new(
        -input.x * cos + input.y * sin,
        input.x * sin + input.y * cos,
    )
}

impl DecisionSpace<'_> {
    pub fn supported(self, input: Vec2, distance: f32) -> bool {
        if !self.ground_controllable {
            return false;
        }
        let direction = world_input(input, self.facing);
        let at = self.position
            + Vec3::new(direction.x, 0.0, direction.y) * (distance * meteor_core::MODEL_SCALE);

        self.mesh.is_none_or(|mesh| {
            mesh.raycast_hit(
                Point3::new(at.x, at.y + 70.0 * meteor_core::MODEL_SCALE, at.z),
                Point3::new(0.0, -1.0, 0.0),
                90.0 * meteor_core::MODEL_SCALE,
            )
            .is_some()
        })
    }

    pub fn jump(
        self,
        patrol: &mut NpcPatrol,
        intent: &mut AiIntent,
        performance: &mut NpcPerformance,
        target_yaw: f32,
        forward: bool,
    ) -> bool {
        let input = if forward { Vec2::Y } else { Vec2::NEG_Y };
        if npc_facing_error(self.facing, target_yaw).abs() > 5.0_f32.to_radians()
            || !self.supported(input, if forward { 200.0 } else { 40.0 })
        {
            return false;
        }
        let strong = npc_roll(patrol) % 2 != 0;
        performance.ai_jump = Some(strong);
        patrol.airborne_heading = Some(self.facing);
        patrol.airborne_input = input;
        intent.move_dir = world_input(input, self.facing);
        intent.locomotion_input = input;
        intent.want_attack = false;
        intent.want_guard = false;
        intent.lock_target = None;
        intent.dodge_burst = None;
        true
    }
}

pub(super) fn npc_roll(patrol: &mut NpcPatrol) -> u32 {
    (next_npc_rand(&mut patrol.rng) * 0x0100_0000 as f32) as u32
}

pub(super) fn queue_command(
    intent: &mut AiIntent,
    patrol: &mut NpcPatrol,
    weapon: u8,
    category: usize,
    airborne: bool,
    allowed: bool,
) {
    intent.want_attack = allowed;
    intent.attack_directions.clear();
    if !allowed {
        return;
    }
    let command = if airborne {
        meteor_ai::combat::AIR_COMMANDS[npc_roll(patrol) as usize % 17]
    } else {
        let reuse = npc_roll(patrol) % 2 == 0;
        let roll = if reuse { 0 } else { npc_roll(patrol) };
        patrol.command_memory.choose(
            meteor_ai::combat::weapon_tactics(weapon),
            category,
            reuse,
            roll,
        )
    };
    intent.attack_directions = meteor_ai::combat::directions(command);
}
