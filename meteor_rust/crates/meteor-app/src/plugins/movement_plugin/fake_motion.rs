use super::*;

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(crate) fn fake_root_motion(
    active: Option<Res<super::super::fighter_frame::ActiveFighter>>,
    time: Res<Time>,
    clock: Option<Res<crate::plugins::FrameClock>>,
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    common: Option<Res<crate::plugins::character_plugin::CommonAmb>>,
    collider: Option<Res<StageCollider>>,
    mut contacts: Option<ResMut<CharacterContactFrame>>,
    mut actors: Query<
        (
            Entity,
            &Fighter,
            &SmState,
            &PoseRuntime,
            Option<&EntryBlend>,
            &mut FighterAir,
            &mut Transform,
            &mut MotionState,
            Option<&LocalFighter>,
        ),
        (With<Fighter>, Without<RemoteFighter>),
    >,
) {
    let dt = crate::plugins::simulation_dt(clock.as_deref(), time.delta_secs());
    if !dt.is_finite() || dt <= 0.0 {
        return;
    }
    let mut stage = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    if let Some(mesh) = collider.as_deref().and_then(|c| c.mesh.as_ref()) {
        stage.set_mesh_collider(mesh);
    }
    for (entity, fighter, state, pose, blend, mut air, mut transform, mut motion, local) in
        &mut actors
    {
        if !super::super::fighter_frame::accepts(active.as_deref(), entity) {
            continue;
        }
        if state.current != FighterState::Fake || air.air.airborne {
            air.fake_carry = None;
            continue;
        }

        let mut carry = match air.fake_carry {
            Some((instance, carry)) if instance == pose.pose_instance_id => carry,
            _ if local.is_some() => Vec2::new(motion.velocity.x, motion.velocity.z),
            _ => Vec2::ZERO,
        };
        carry *= (1.0 - action_carry_decay(&air) * dt).max(0.0);

        let (anim_x, anim_z) =
            if blend.is_some_and(|b| b.duration > 0.0 && b.pose_id == pose.pose_id) {
                (0.0, 0.0)
            } else {
                assets
                    .as_deref()
                    .and_then(|a| a.0.get(&format!("p{}", fighter.character_id)))
                    .zip(common.as_deref())
                    .map_or((0.0, 0.0), |(asset, common)| {
                        let [x, y, _] = asset.authored_root_step(&common.0, pose, dt);
                        meteor_anim::root_motion::root_delta_to_world_xz(x, y, motion.facing, 1.0)
                    })
            };
        let (dx, _, dz, _) = clamp_composed_delta_3d(
            anim_x + (carry.x + air.ground_external_velocity.x) * dt,
            0.0,
            anim_z + (carry.y + air.ground_external_velocity.z) * dt,
            dt,
            MOVER_MAX_SPEED,
        );
        let p = transform.translation;
        let (rx, rz, ry) = resolve_fighter_xz(
            contacts.as_deref_mut(),
            entity,
            &stage,
            Point3::new(p.x, p.y, p.z),
            dx,
            dz,
            false,
        );
        if rx == 0.0 && dx != 0.0 {
            carry.x = 0.0;
        }
        if rz == 0.0 && dz != 0.0 {
            carry.y = 0.0;
        }
        air.fake_carry = Some((pose.pose_instance_id, carry));
        let accepted = Vec3::new(rx, ry, rz);
        transform.translation += accepted;

        motion.velocity = accepted / dt;
        commit_fighter_position(contacts.as_deref_mut(), entity, transform.translation);
    }
}
