use super::*;
use bevy::render::{render_resource::Extent3d, renderer::RenderAdapterInfo};
use std::time::{Duration, Instant};

pub(super) fn wait(device: &RenderDevice) {
    device
        .poll(PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(5)),
        })
        .unwrap();
}

pub(super) fn summarize(samples: &mut [f64]) -> String {
    if samples.is_empty() {
        return "unavailable".to_owned();
    }
    samples.sort_by(f64::total_cmp);
    format!(
        "median {:.3} ms, p95 {:.3} ms (n={})",
        samples[samples.len() / 2],
        samples[(samples.len() * 95).div_ceil(100) - 1],
        samples.len()
    )
}

pub(super) fn run(
    app: &mut App,
    camera: Entity,
    target: &Handle<Image>,
    to_world: impl Fn(Vec3) -> Vec3,
) {
    let original_size = app
        .world()
        .resource::<Assets<Image>>()
        .get(target)
        .unwrap()
        .texture_descriptor
        .size;
    let adapter = app.world().resource::<RenderAdapterInfo>();
    eprintln!(
        "sn13 sky benchmark: adapter {:?}, {}",
        adapter.0.backend, adapter.0.name
    );
    let eye = Vec3::new(-794., -178., 63.);
    let views = [
        ("level", Vec3::new(-0.58, 0.81, 0.08)),
        ("look_up", Vec3::new(-0.58, 0.81, 0.50)),
    ];
    let only_1440 = std::env::var("METEOR_SKY_BENCH").as_deref() == Ok("1440");
    for (width, height) in [(1920, 1080), (2560, 1440)] {
        if only_1440 && height != 1440 {
            continue;
        }
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .get_mut(target)
            .unwrap()
            .resize(Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            });
        for (name, direction) in views {
            *app.world_mut().get_mut::<Transform>(camera).unwrap() =
                Transform::from_translation(to_world(eye))
                    .looking_at(to_world(eye + direction), Vec3::Y);
            let mut results = [Vec::new(), Vec::new()];
            for enabled in [false, true, true, false] {
                load_sn13_variant(app, enabled);

                for frame in 0..56 {
                    app.world_mut().resource_mut::<FrameUniforms>().seconds =
                        f64::from(frame) / 60.0;
                    let start = Instant::now();
                    app.update();
                    let device = app.world().resource::<RenderDevice>();
                    wait(device);
                    if frame >= 24 {
                        assert_eq!(sky::cache_ready(app), enabled);
                        results[usize::from(enabled)].push(start.elapsed().as_secs_f64() * 1000.0);
                    }
                }
            }
            for (enabled, samples) in results.iter_mut().enumerate() {
                eprintln!(
                    "{width}x{height} {name} sky={enabled}: CPU+GPU {}",
                    summarize(samples),
                );
            }
        }
    }
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .get_mut(target)
        .unwrap()
        .resize(original_size);
    app.world_mut().resource_mut::<FrameUniforms>().seconds = 0.0;
}
