use bevy::prelude::*;
use bevy::render::{
    view::screenshot::{Capturing, Screenshot},
    Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
};

use crate::app_state::Settings;

#[derive(Resource)]
struct RenderFrame(bool);

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct DrawFrame;

#[derive(Default)]
struct RenderPacer {
    cap: Option<u32>,
    phase: f64,
}

impl RenderPacer {
    fn advance(&mut self, dt: f64, cap: u32) -> bool {
        if self.cap != Some(cap) {
            self.cap = Some(cap);
            self.phase = 0.0;
            return true;
        }
        if cap == 0 {
            return true;
        }
        let interval = 1.0 / f64::from(cap);
        self.phase += dt;
        if self.phase + 1e-9 < interval {
            return false;
        }
        self.phase = self.phase.rem_euclid(interval);
        if interval - self.phase <= 1e-9 {
            self.phase = 0.0;
        }
        true
    }
}

pub(super) fn install(app: &mut App) {
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };

    render
        .insert_resource(RenderFrame(true))
        .add_systems(ExtractSchedule, extract_render_frame)
        .configure_sets(
            Render,
            (
                DrawFrame.run_if(|frame: Res<RenderFrame>| frame.0),
                RenderSystems::Render.in_set(DrawFrame),
            ),
        );

    app.world_mut()
        .remove_resource::<bevy::time::TimeReceiver>();
    app.insert_resource(RenderFrame(true))
        .add_systems(Last, select_render_frame);
}

fn select_render_frame(
    time: Res<Time<bevy::time::Real>>,
    settings: Res<Settings>,
    screenshots: Query<(), (With<Screenshot>, Without<Capturing>)>,
    mut pacing: Local<RenderPacer>,
    mut frame: ResMut<RenderFrame>,
) {
    frame.0 = pacing.advance(time.delta_secs_f64(), settings.target_fps.clamp(0, 240))
        || !screenshots.is_empty();
}

fn extract_render_frame(main: Extract<Res<RenderFrame>>, mut render: ResMut<RenderFrame>) {
    render.0 = main.0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{self, fighter_frame::FighterStepStart, FrameClock};
    use bevy::app::SubApp;
    use bevy::ecs::schedule::ScheduleLabel;
    use bevy::time::Real;
    use std::time::Duration;

    #[derive(Resource, Default)]
    struct FrameCounts {
        assets: usize,
        prepared: usize,
        draws: usize,
        steps: Vec<f32>,
        reported_steps: usize,
    }

    #[test]
    fn render_caps_gate_draws_without_gating_simulation_or_asset_delivery() {
        for hz in [0, 30, 60, 120, 135, 150, 173, 200] {
            for cap in [0, 30, 60, 120] {
                let mut app = App::new();
                let mut render = SubApp::new();
                render.add_schedule(Render::base_schedule());
                render.update_schedule = Some(Render.intern());
                render.set_extract(|main, render| {
                    render.resource_mut::<RenderFrame>().0 = main.resource::<RenderFrame>().0;
                });
                render.init_resource::<FrameCounts>().add_systems(
                    Render,
                    (
                        (|mut counts: ResMut<FrameCounts>| counts.assets += 1)
                            .in_set(RenderSystems::PrepareAssets),
                        (|mut counts: ResMut<FrameCounts>| counts.prepared += 1)
                            .in_set(RenderSystems::Prepare),
                        (|mut counts: ResMut<FrameCounts>| counts.draws += 1)
                            .in_set(RenderSystems::Render),
                    ),
                );
                app.insert_sub_app(RenderApp, render);
                app.insert_resource(Settings {
                    target_fps: cap,
                    physics_hz: if hz == 0 {
                        Settings::PHYSICS_HZ_DEFAULT
                    } else {
                        hz
                    },
                    variable_physics: hz == 0,
                    ..default()
                })
                .init_resource::<Time<Real>>()
                .init_resource::<FrameCounts>();
                install(&mut app);
                plugins::install_frame_clock(&mut app);
                plugins::fighter_frame::install(&mut app);
                app.add_systems(
                    FighterStepStart,
                    |clock: Res<FrameClock>, mut counts: ResMut<FrameCounts>| {
                        if clock.sim_dt > 0.0 {
                            counts.steps.push(clock.sim_dt);
                        }
                    },
                );
                app.add_systems(
                    Update,
                    (|clock: Res<FrameClock>, mut counts: ResMut<FrameCounts>| {
                        counts.reported_steps += clock.completed_controller_steps as usize;
                    })
                    .after(plugins::fighter_frame::SerialFighterSet),
                );
                app.update();
                let mut previous = 0;
                for update in 1..=144 {
                    let elapsed = update * 1_000_000_000 / 144;
                    app.world_mut()
                        .resource_mut::<Time<Real>>()
                        .advance_by(Duration::from_nanos(elapsed - previous));
                    previous = elapsed;
                    app.update();
                }
                let sim = app.world().resource::<FrameCounts>();
                assert_eq!(sim.reported_steps, sim.steps.len(), "cap={cap}, hz={hz}");
                if hz == 0 {
                    assert_eq!(sim.steps.len(), 144, "cap={cap}");
                    for (index, step) in sim.steps.iter().copied().enumerate() {
                        let end = (index + 1) * 1_000_000_000 / 144;
                        let start = index * 1_000_000_000 / 144;
                        assert_eq!(
                            step,
                            Duration::from_nanos((end - start) as u64).as_secs_f32(),
                            "cap={cap}, host update {index}"
                        );
                    }
                } else {
                    assert_eq!(sim.steps, vec![1.0 / hz as f32; hz as usize], "cap={cap}");
                }
                let render = app.sub_app(RenderApp).world().resource::<FrameCounts>();
                assert_eq!(render.assets, 145);
                assert_eq!(render.prepared, 145);
                assert_eq!(render.draws, 1 + if cap == 0 { 144 } else { cap as usize });
            }
        }
    }

    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    #[test]
    fn capped_frames_keep_offscreen_menu_images_and_text_visible() {
        use bevy::app::PluginsState;
        use bevy::asset::RenderAssetUsages;
        use bevy::camera::RenderTarget;
        use bevy::render::{
            gpu_readback::{Readback, ReadbackComplete},
            render_resource::{Extent3d, PollType, TextureDimension, TextureFormat, TextureUsages},
            renderer::RenderDevice,
            RenderPlugin,
        };
        use bevy::time::TimeUpdateStrategy;
        use bevy::ui::IsDefaultUiCamera;
        use bevy::window::{ExitCondition, PrimaryWindow};
        use std::sync::{Arc, Mutex};

        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    ..default()
                })
                .set(RenderPlugin {
                    synchronous_pipeline_compilation: true,
                    ..default()
                })
                .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>()
                .disable::<bevy::winit::WinitPlugin>()
                .disable::<bevy::audio::AudioPlugin>(),
        )
        .insert_resource(Settings {
            target_fps: 60,
            ..default()
        })
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_nanos(
            4_166_667,
        )));
        install(&mut app);
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while app.plugins_state() != PluginsState::Ready {
            assert!(
                std::time::Instant::now() < deadline,
                "renderer setup timed out"
            );
            bevy::tasks::tick_global_task_pools_on_main_thread();
            std::thread::sleep(Duration::from_millis(2));
        }
        app.finish();
        app.cleanup();

        let mut target = Image::new_target_texture(64, 64, TextureFormat::Rgba8UnormSrgb, None);
        target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        let target = images.add(target);
        let thumbnail = images.add(Image::new_fill(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[0, 255, 0, 255],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        ));
        let font_bytes = bevy::text::DEFAULT_FONT_DATA.to_vec();
        let font = app
            .world_mut()
            .resource_mut::<Assets<Font>>()
            .add(Font::from_bytes(font_bytes));
        app.world_mut().spawn((
            PrimaryWindow,
            Window {
                resolution: (64, 64).into(),
                ..default()
            },
        ));
        app.world_mut().spawn((
            Camera2d,
            Camera {
                clear_color: ClearColorConfig::Custom(Color::BLACK),
                ..default()
            },
            RenderTarget::Image(target.clone().into()),
            IsDefaultUiCamera,
        ));
        app.world_mut().spawn((
            ImageNode::new(thumbnail),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(4.0),
                top: Val::Px(4.0),
                width: Val::Px(12.0),
                height: Val::Px(12.0),
                ..default()
            },
        ));
        app.world_mut().spawn((
            Text::new("流星"),
            TextFont {
                font: font.into(),
                font_size: 22.0.into(),
                ..default()
            },
            TextColor(Color::WHITE),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(4.0),
                top: Val::Px(24.0),
                ..default()
            },
        ));

        let captured = Arc::new(Mutex::new(Vec::new()));
        let pixels = captured.clone();
        app.world_mut().spawn(Readback::texture(target)).observe(
            move |event: On<ReadbackComplete>| {
                *pixels.lock().unwrap() = event.data.clone();
            },
        );
        for _ in 0..72 {
            app.update();
            app.world()
                .resource::<RenderDevice>()
                .poll(PollType::Poll)
                .unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        let pixels = captured.lock().unwrap();
        assert_eq!(pixels.len(), 64 * 64 * 4);
        let image_pixel = (8 * 64 + 8) * 4;
        assert_eq!(&pixels[image_pixel..image_pixel + 3], &[0, 255, 0]);
        assert!(
            pixels[24 * 64 * 4..]
                .chunks_exact(4)
                .any(|rgba| rgba[..3].iter().all(|&value| value >= 200)),
            "menu glyphs must reach the render target"
        );
    }
}
