#[test]
#[ignore = "prints dagger dash-to-attack ledge trajectories for manual analysis"]
fn diagnose_dagger_dash_attack_at_ledge() {
    for fps in [30, 60, 120] {
        for delay_ms in [0, 80, 160, 240] {
            let mut h =
                spawn_gameplay_harness_with_order(DAGGER, true).expect("original P1 resources");
            install_guard_test_animation(&mut h);
            let mut mesh = harness_ground();
            mesh.append_world_triangles(
                &[
                    [-20.0, 8.0, -20.0],
                    [-20.0, 8.0, 0.0],
                    [20.0, 8.0, 0.0],
                    [20.0, 8.0, -20.0],
                ],
                &[0, 1, 2, 0, 2, 3],
            );
            h.app.world_mut().resource_mut::<StageCollider>().mesh = Some(mesh);
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = Vec3::new(0.0, 8.0, -2.0);
            h.app
                .world_mut()
                .get_mut::<FighterAir>(h.fighter)
                .unwrap()
                .ground_y = 8.0;
            let dt = 1.0 / fps as f32;
            h.step_dt(dt);
            h.press_key(KeyCode::KeyW);
            h.step_dt(dt);
            h.release_key(KeyCode::KeyW);
            h.step_dt(dt);
            h.press_key(KeyCode::KeyW);
            h.step_dt(dt);
            h.release_key(KeyCode::KeyW);
            let wait_ticks = (delay_ms as f32 * fps as f32 / 1000.0).round() as usize;
            for tick in 0..wait_ticks + fps as usize {
                let attack = tick == wait_ticks;
                if attack {
                    h.press_mouse(MouseButton::Left);
                }
                let before = h.position();
                h.step_dt(dt);
                if attack {
                    h.release_mouse(MouseButton::Left);
                }
                let w = h.app.world();
                let air = w.get::<FighterAir>(h.fighter).unwrap();
                let pose = w.get::<PoseRuntime>(h.fighter).unwrap();
                let root = w.resource::<CharacterAssetsSet>().0["p1"].authored_root_step(
                    &w.resource::<CommonAmb>().0,
                    pose,
                    dt,
                );
                let pos = h.position();
                eprintln!("LEDGE,{fps},{delay_ms},{tick},{attack},{:?},{},{:.4},{:.6},{:.6},{:.6},{:.6},{},{},{},{},{:.6},{:.6}",
                    h.state(), pose.pose_id, pose.frame, pos.y, pos.z,
                    pos.y-before.y, pos.z-before.z, air.air.airborne, air.ground_contact,
                    air.air.launch_pose_id, air.air.launch_lifted, air.air.vertical_speed, root[2]);
            }
        }
    }
}
