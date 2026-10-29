use super::*;

const LINGER_SECONDS: f32 = 0.34;
const HOLD_SECONDS: f32 = 0.02;
const MAX_WAKE_SAMPLES: usize = 64;

pub(super) struct RippleWake {
    key: TrailKey,
    window: (u32, u32),
    open: bool,
    samples: Vec<EdgeSample>,
    last_emission: f32,
}

pub(super) fn record_ripple(
    wakes: &mut Vec<RippleWake>,
    key: TrailKey,
    window: Option<(u32, u32)>,
    previous: Option<EdgeSample>,
    sample: EdgeSample,
    wall_now: f32,
) {
    let existing = wakes.iter().position(|wake| {
        wake.key == key
            && wake.open
            && Some(wake.window) == window
            && previous.is_some_and(|previous| {
                wake.samples
                    .last()
                    .is_some_and(|last| last.time == previous.time)
            })
    });
    for wake in wakes.iter_mut().filter(|wake| wake.key == key) {
        wake.open = false;
    }
    let Some(window) = window else {
        return;
    };
    let index = if let Some(index) = existing {
        index
    } else {
        if wakes.len() >= RIPPLE_MAX_STREAMS {
            let oldest = wakes
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.last_emission.total_cmp(&b.last_emission))
                .map(|(index, _)| index)
                .unwrap();
            wakes.remove(oldest);
        }

        let samples = previous
            .filter(|previous| {
                let dt = sample.time - previous.time;
                dt > 0.0 && dt <= meteor_core::MAX_FRAME_DT + 1e-5
            })
            .into_iter()
            .collect();
        wakes.push(RippleWake {
            key,
            window,
            open: true,
            samples,
            last_emission: wall_now,
        });
        wakes.len() - 1
    };
    let wake = &mut wakes[index];
    wake.open = true;
    wake.last_emission = wall_now;
    wake.samples.push(sample);

    while wake.samples.len() > 2 && wake.samples[1].time < sample.time - RIPPLE_LOOKBACK {
        wake.samples.remove(0);
    }
    if wake.samples.len() > MAX_WAKE_SAMPLES {
        wake.samples.drain(..wake.samples.len() - MAX_WAKE_SAMPLES);
    }
}

pub(super) fn build_wakes(wakes: &mut Vec<RippleWake>, wall_now: f32) -> RippleBuffers {
    wakes.retain(|wake| wall_now - wake.last_emission < LINGER_SECONDS);
    build_ripple_sections(wakes.iter().filter_map(|wake| {
        let last = wake.samples.last()?;
        let t = ((wall_now - wake.last_emission - HOLD_SECONDS) / (LINGER_SECONDS - HOLD_SECONDS))
            .clamp(0.0, 1.0);
        let opacity = 1.0 - t * t * (3.0 - 2.0 * t);

        Some((wake.samples.as_slice(), last.time, opacity))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> TrailKey {
        TrailKey {
            fighter: Entity::from_raw_u32(1).unwrap(),
            slot: 0,
            part: "blade".into(),
        }
    }

    fn sample(time: f32, x: f32) -> EdgeSample {
        EdgeSample {
            time,
            base: Vec3::new(x, 0.0, 0.0),
            tip: Vec3::new(x, 1.0, 0.0),
        }
    }

    #[test]
    fn windup_and_recovery_do_not_emit_and_one_judgment_frame_has_a_wake() {
        let mut wakes = Vec::new();
        record_ripple(
            &mut wakes,
            key(),
            None,
            Some(sample(0.0, 0.0)),
            sample(0.03, 0.4),
            1.0,
        );
        assert!(wakes.is_empty());
        record_ripple(
            &mut wakes,
            key(),
            Some((1, 0)),
            Some(sample(0.03, 0.4)),
            sample(0.06, 0.8),
            1.03,
        );
        let live = build_wakes(&mut wakes, 1.03);
        assert!(
            !live.3.is_empty(),
            "single-frame attacks must not need 84ms of history"
        );
        record_ripple(
            &mut wakes,
            key(),
            None,
            Some(sample(0.06, 0.8)),
            sample(0.09, 5.0),
            1.06,
        );
        let lingering = build_wakes(&mut wakes, 1.16);
        assert_eq!(
            live.0, lingering.0,
            "recovery motion cannot pull the retained arc"
        );
        assert!(lingering.2[0][0] > 0.0 && lingering.2[0][0] < live.2[0][0]);
        assert_eq!(live.2[0][1], 1.0);
        assert!(lingering.2[0][1] > 0.0 && lingering.2[0][1] < 1.0);
        assert!(build_wakes(&mut wakes, 1.40).3.is_empty());
        assert!(wakes.is_empty());
    }

    #[test]
    fn new_pose_and_separated_windows_do_not_bridge_old_arcs() {
        let mut wakes = Vec::new();
        record_ripple(
            &mut wakes,
            key(),
            Some((1, 0)),
            Some(sample(0.0, 0.0)),
            sample(0.03, 0.4),
            1.0,
        );
        record_ripple(
            &mut wakes,
            key(),
            None,
            Some(sample(0.03, 0.4)),
            sample(0.06, 2.0),
            1.03,
        );
        record_ripple(
            &mut wakes,
            key(),
            Some((1, 0)),
            Some(sample(0.06, 2.0)),
            sample(0.09, 2.4),
            1.06,
        );
        record_ripple(
            &mut wakes,
            key(),
            Some((2, 0)),
            Some(sample(0.09, 2.4)),
            sample(0.12, 2.8),
            1.09,
        );
        assert_eq!(wakes.len(), 3);
        assert_eq!(wakes[0].samples.last().unwrap().tip.x, 0.4);
        assert_eq!(wakes[1].samples.first().unwrap().tip.x, 2.0);
        assert_eq!(wakes[2].samples.len(), 2);
    }

    #[test]
    fn linger_expires_without_simulation_advancing_and_pool_is_bounded() {
        let mut wakes = Vec::new();
        for index in 0..40 {
            record_ripple(
                &mut wakes,
                key(),
                Some((index, 0)),
                Some(sample(0.0, 0.0)),
                sample(0.01, 0.2),
                index as f32 * 0.001,
            );
        }
        assert_eq!(wakes.len(), RIPPLE_MAX_STREAMS);
        assert!(!build_wakes(&mut wakes, 0.1).3.is_empty());
        assert!(build_wakes(&mut wakes, 0.4).3.is_empty());
    }

    #[test]
    fn disabling_effects_clears_linger_even_when_simulation_is_stopped() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let mut runtime = WeaponTrailRuntime::default();
        record_ripple(
            &mut runtime.wakes,
            key(),
            Some((1, 0)),
            Some(sample(0.0, 0.0)),
            sample(0.03, 0.4),
            0.0,
        );
        world.insert_resource(runtime);
        world.insert_resource(crate::app_state::Settings {
            hit_effects: false,
            ..default()
        });
        world.init_resource::<Time<Real>>();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<GmdlMaterial>>();
        world.init_resource::<Assets<BladeRippleMaterial>>();
        world.init_resource::<crate::plugins::FrameClock>();
        world.run_system_once(update_blade_ripples).unwrap();
        assert!(world.resource::<WeaponTrailRuntime>().wakes.is_empty());
    }

    #[test]
    fn reused_combat_window_gate_includes_end_fraction_but_not_recovery() {
        let poses = meteor_asset::pos::parse_poses(
            "Pose 1\n{\nStart 10\nEnd 20\nAttack\n{\nStart 12\nEnd 14\n}\n}\n",
        );
        assert_eq!(poses.len(), 1);
        let attacks = &poses[0].attacks;
        assert_eq!(attacks.len(), 1);
        for (frame, active) in [(11.99, false), (12.0, true), (14.99, true), (15.0, false)] {
            assert_eq!(
                crate::plugins::combat_plugin::active_attack_window(frame, attacks).is_some(),
                active
            );
        }
    }
}
