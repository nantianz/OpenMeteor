use crate::app_state::Settings;
use crate::plugins::camera_plugin::GameplayCamera;
use bevy::camera::CameraOutputMode;
use bevy::core_pipeline::{schedule::Core3d, Core3dSystems, FullscreenShader};
use bevy::prelude::*;
use bevy::render::{
    camera::ExtractedCamera,
    extract_component::{
        ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
        UniformComponentPlugin,
    },
    render_resource::{
        binding_types::{sampler, texture_2d, uniform_buffer},
        *,
    },
    renderer::{RenderContext, RenderDevice, ViewQuery},
    view::{ExtractedView, ViewTarget},
    Render, RenderApp, RenderStartup, RenderSystems,
};
use bevy::window::PrimaryWindow;

const DISPLAY_POST_PROCESS_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("ea3bd76c-8c12-4e9f-909f-f72547991a01");

#[derive(Debug, Clone, Copy, PartialEq)]
struct DisplayProjection {
    brightness: f32,
    contrast: f32,
    resolution_override: Option<UVec2>,
}

fn display_projection(
    settings: &Settings,
    physical_size: UVec2,
    window_scale_factor: f32,
) -> DisplayProjection {
    let requested_scale = if settings.render_scale.is_finite() {
        settings.render_scale.clamp(0.5, 1.0)
    } else {
        1.0
    };

    let dpr_cap = if window_scale_factor.is_finite() && window_scale_factor > 0.0 {
        (2.0 / window_scale_factor).min(1.0)
    } else {
        1.0
    };

    let effective_scale = (requested_scale * dpr_cap).clamp(0.01, 1.0);
    let override_size = UVec2::new(
        ((physical_size.x as f32 * effective_scale).round() as u32)
            .clamp(1, physical_size.x.max(1)),
        ((physical_size.y as f32 * effective_scale).round() as u32)
            .clamp(1, physical_size.y.max(1)),
    );
    let resolution_override =
        (physical_size.x > 0 && physical_size.y > 0 && override_size != physical_size)
            .then_some(override_size);
    let brightness_value = if settings.brightness.is_finite() {
        settings.brightness
    } else {
        94.0
    };
    let contrast_value = if settings.contrast.is_finite() {
        settings.contrast
    } else {
        94.0
    };
    DisplayProjection {
        brightness: (brightness_value / 94.0).max(0.45),
        contrast: (contrast_value / 94.0).max(0.55),
        resolution_override,
    }
}

#[derive(Component, Clone, Copy, ExtractComponent, ShaderType, PartialEq)]
struct ViewportPostProcessSettings {
    brightness: f32,
    contrast: f32,
    padding: Vec2,
}

#[derive(Component, Clone, Copy, PartialEq, ExtractComponent)]
pub(crate) struct RenderScaleResolution(pub(crate) UVec2);

fn resize_scene_attachments(
    mut views: Query<(
        &RenderScaleResolution,
        &mut ExtractedCamera,
        &mut ExtractedView,
    )>,
) {
    for (resolution, mut camera, mut view) in &mut views {
        camera.physical_target_size = Some(resolution.0);
        camera.physical_viewport_size = Some(resolution.0);
        camera.viewport = None;
        view.viewport = UVec4::new(0, 0, resolution.0.x, resolution.0.y);
    }
}

#[derive(Resource, Clone, Default)]
pub(crate) struct ActualRenderResolution(
    std::sync::Arc<std::sync::Mutex<Option<SceneAttachmentSizes>>>,
);

#[derive(Clone, Copy, Debug, PartialEq)]
struct SceneAttachmentSizes {
    color: UVec2,
    msaa: Option<UVec2>,
    depth: Option<UVec2>,
    samples: u32,
}

impl ActualRenderResolution {
    pub(crate) fn label(&self) -> String {
        let Some(sizes) = *self.0.lock().unwrap() else {
            return "3D 实际渲染：等待渲染器".into();
        };
        let size = |value: Option<UVec2>| {
            value.map_or_else(|| "无".into(), |v| format!("{}×{}", v.x, v.y))
        };
        format!(
            "3D 实际渲染：{}×{}\nMSAA {}×：{} / 深度：{}",
            sizes.color.x,
            sizes.color.y,
            sizes.samples,
            size(sizes.msaa),
            size(sizes.depth),
        )
    }
}

fn capture_scene_attachment_sizes(
    shared: Res<ActualRenderResolution>,
    views: Query<
        (&ViewTarget, Option<&bevy::render::view::ViewDepthTexture>),
        With<GameplayCamera>,
    >,
) {
    let sizes = views.single().ok().map(|(target, depth)| {
        let color = target.main_texture();
        let msaa = target.sampled_main_texture();
        SceneAttachmentSizes {
            color: UVec2::new(color.width(), color.height()),
            msaa: msaa.map(|t| UVec2::new(t.width(), t.height())),
            depth: depth.map(|d| UVec2::new(d.texture.width(), d.texture.height())),
            samples: msaa.map_or(1, |t| t.sample_count()),
        }
    });
    *shared.0.lock().unwrap() = sizes;
}

pub struct DisplaySettingsPlugin;

impl Plugin for DisplaySettingsPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::load_internal_asset!(
            app,
            DISPLAY_POST_PROCESS_SHADER,
            "display_post_process.wgsl",
            Shader::from_wgsl
        );
        app.add_plugins((
            ExtractComponentPlugin::<ViewportPostProcessSettings>::default(),
            UniformComponentPlugin::<ViewportPostProcessSettings>::default(),
            ExtractComponentPlugin::<RenderScaleResolution>::default(),
            ExtractComponentPlugin::<GameplayCamera>::default(),
        ))
        .add_systems(Update, sync_display_settings);

        let actual_resolution = ActualRenderResolution::default();
        app.insert_resource(actual_resolution.clone());
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(actual_resolution)
            .init_resource::<SpecializedRenderPipelines<DisplayPostProcessPipeline>>()
            .add_systems(RenderStartup, init_display_post_process_pipeline)
            .add_systems(
                Render,
                (
                    resize_scene_attachments
                        .after(RenderSystems::ExtractCommands)
                        .before(RenderSystems::PrepareViews),
                    prepare_display_output.in_set(RenderSystems::PrepareResources),
                    capture_scene_attachment_sizes
                        .after(RenderSystems::Prepare)
                        .before(RenderSystems::Render),
                ),
            );
        render_app.add_systems(
            Core3d,
            display_post_process
                .after(Core3dSystems::PostProcess)
                .after(bevy::core_pipeline::upscaling::upscaling),
        );
    }
}

type DisplayCameraQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static ViewportPostProcessSettings>,
        Option<&'static RenderScaleResolution>,
    ),
    With<GameplayCamera>,
>;

fn sync_display_settings(
    settings: Res<Settings>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut commands: Commands,
    cameras: DisplayCameraQuery<'_, '_>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let physical = UVec2::new(window.physical_width(), window.physical_height());
    let projection = display_projection(&settings, physical, window.scale_factor());
    let post = ViewportPostProcessSettings {
        brightness: projection.brightness,
        contrast: projection.contrast,
        padding: Vec2::ZERO,
    };
    let needs_post = projection.resolution_override.is_some()
        || (projection.brightness - 1.0).abs() > f32::EPSILON
        || (projection.contrast - 1.0).abs() > f32::EPSILON;

    for (entity, current_post, current_resolution) in &cameras {
        let mut entity_commands = commands.entity(entity);
        if needs_post {
            if current_post.is_none_or(|current| *current != post) {
                entity_commands.insert(post);
            }
        } else if current_post.is_some() {
            entity_commands.remove::<ViewportPostProcessSettings>();
        }
        match projection.resolution_override {
            Some(size) if current_resolution.is_none_or(|current| current.0 != size) => {
                entity_commands.insert(RenderScaleResolution(size));
            }
            None if current_resolution.is_some() => {
                entity_commands.remove::<RenderScaleResolution>();
            }
            _ => {}
        }
    }
}

#[derive(Resource)]
struct DisplayPostProcessPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    vertex: VertexState,
}

fn init_display_post_process_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    fullscreen_shader: Res<FullscreenShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "meteor_display_post_process_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<ViewportPostProcessSettings>(true),
            ),
        ),
    );
    let sampler = render_device.create_sampler(&SamplerDescriptor {
        label: Some("meteor display upscale sampler"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    commands.insert_resource(DisplayPostProcessPipeline {
        layout,
        sampler,
        vertex: fullscreen_shader.to_vertex_state(),
    });
}

impl SpecializedRenderPipeline for DisplayPostProcessPipeline {
    type Key = TextureFormat;

    fn specialize(&self, format: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("meteor display output".into()),
            layout: vec![self.layout.clone()],
            vertex: self.vertex.clone(),
            fragment: Some(FragmentState {
                shader: DISPLAY_POST_PROCESS_SHADER,
                entry_point: Some("fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }
    }
}

#[derive(Component)]
struct DisplayOutputPipeline(CachedRenderPipelineId);

fn prepare_display_output(
    mut commands: Commands,
    mut pipelines: ResMut<SpecializedRenderPipelines<DisplayPostProcessPipeline>>,
    pipeline: Res<DisplayPostProcessPipeline>,
    mut pipeline_cache: ResMut<PipelineCache>,
    mut views: Query<
        (
            Entity,
            &ViewTarget,
            &mut ExtractedCamera,
            Option<&DisplayOutputPipeline>,
        ),
        With<ViewportPostProcessSettings>,
    >,
) {
    for (entity, target, mut camera, current) in &mut views {
        if matches!(camera.output_mode, CameraOutputMode::Skip) {
            commands.entity(entity).remove::<DisplayOutputPipeline>();
            continue;
        }
        let Some(format) = target.out_texture_view_format() else {
            continue;
        };
        let id = pipelines.specialize(&pipeline_cache, &pipeline, format);

        if current.is_none_or(|current| current.0 != id) {
            pipeline_cache.block_on_render_pipeline(id);
            commands.entity(entity).insert(DisplayOutputPipeline(id));
        }
        camera.output_mode = CameraOutputMode::Skip;
    }
}

#[derive(Default)]
struct DisplayBindGroupCache(Option<(TextureViewId, BufferId, BindGroup)>);

fn display_post_process(
    view: ViewQuery<(
        &ViewTarget,
        &DisplayOutputPipeline,
        &ViewportPostProcessSettings,
        &DynamicUniformIndex<ViewportPostProcessSettings>,
    )>,
    pipeline: Option<Res<DisplayPostProcessPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    settings_uniforms: Res<ComponentUniforms<ViewportPostProcessSettings>>,
    mut cache: Local<DisplayBindGroupCache>,
    mut context: RenderContext,
) {
    let Some(pipeline) = pipeline else {
        return;
    };
    let (view_target, output_pipeline, _settings, settings_index) = view.into_inner();
    let Some(render_pipeline) = pipeline_cache.get_render_pipeline(output_pipeline.0) else {
        return;
    };
    let Some(settings_binding) = settings_uniforms.uniforms().binding() else {
        return;
    };
    let Some(uniform_buffer) = settings_uniforms.uniforms().buffer() else {
        return;
    };
    let source = view_target.main_texture_view();
    let Some(output) = view_target.out_texture_color_attachment(Some(default())) else {
        return;
    };
    let bind_group = match &mut cache.0 {
        Some((texture_id, buffer_id, bind_group))
            if *texture_id == source.id() && *buffer_id == uniform_buffer.id() =>
        {
            bind_group
        }
        cached => {
            let bind_group = context.render_device().create_bind_group(
                "meteor_display_post_process_bind_group",
                &pipeline_cache.get_bind_group_layout(&pipeline.layout),
                &BindGroupEntries::sequential((
                    source,
                    &pipeline.sampler,
                    settings_binding.clone(),
                )),
            );
            &cached
                .insert((source.id(), uniform_buffer.id(), bind_group))
                .2
        }
    };

    #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
    let diagnostics = context.diagnostic_recorder();
    #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
    let diagnostics = diagnostics.as_deref();
    #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
    use bevy::render::diagnostic::RecordDiagnostics;
    #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
    let span = diagnostics.time_span(context.command_encoder(), "meteor_display_post_process");
    let mut render_pass = context
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("meteor display post process pass"),
            color_attachments: &[Some(output)],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    render_pass.set_pipeline(render_pipeline);
    render_pass.set_bind_group(0, bind_group, &[settings_index.index()]);
    render_pass.draw(0..3, 0..1);
    drop(render_pass);
    #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
    span.end(context.command_encoder());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
    #[test]
    fn headless_render_reports_real_passes_and_async_capture_epoch() {
        const CHILD: &str = "METEOR_DISPLAY_GPU_TEST_FORMAT";
        if std::env::var_os(CHILD).is_none() {
            for format in ["rgba", "bgra"] {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "plugins::display_settings_plugin::tests::headless_render_reports_real_passes_and_async_capture_epoch", "--nocapture"])
                    .env(CHILD, format).output().unwrap();
                eprintln!(
                    "{format}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    output.status.success(),
                    "display GPU regression failed: {format}"
                );
            }
            return;
        }
        let bgra = std::env::var(CHILD).unwrap() == "bgra";
        use bevy::{
            app::PluginsState,
            diagnostic::{DiagnosticPath, DiagnosticsStore},
        };
        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                })
                .disable::<bevy::winit::WinitPlugin>()
                .disable::<bevy::audio::AudioPlugin>(),
        )
        .insert_state(crate::app_state::AppState::Playing)
        .init_resource::<crate::app_state::LoadedStageInfo>()
        .init_resource::<super::super::stage_collider::StageCollider>()
        .init_resource::<Settings>()
        .add_plugins((DisplaySettingsPlugin, super::super::perf_plugin::PerfPlugin));
        super::super::frame_work::install(&mut app);
        super::super::frame_work::serial_post_update(&mut app);

        app.world_mut()
            .run_system_cached(super::super::camera_plugin::setup_camera)
            .unwrap();
        let mut target_image = Image::new_target_texture(
            64,
            64,
            if bgra {
                TextureFormat::Bgra8UnormSrgb
            } else {
                TextureFormat::Rgba8UnormSrgb
            },
            None,
        );
        target_image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        let target = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(target_image);
        let game = app
            .world_mut()
            .query_filtered::<Entity, With<GameplayCamera>>()
            .single(app.world())
            .unwrap();
        let ui = app
            .world_mut()
            .query_filtered::<Entity, With<super::super::camera_plugin::UiCamera>>()
            .single(app.world())
            .unwrap();
        for camera in [game, ui] {
            app.world_mut()
                .entity_mut(camera)
                .insert(bevy::camera::RenderTarget::Image(target.clone().into()));
        }
        assert_eq!(*app.world().get::<Msaa>(game).unwrap(), Msaa::Sample4);
        assert_eq!(*app.world().get::<Msaa>(ui).unwrap(), Msaa::Off);
        app.world_mut().get_mut::<Camera>(game).unwrap().clear_color =
            ClearColorConfig::Custom(Color::linear_rgb(0.0, 0.0, 1.0));
        app.world_mut()
            .entity_mut(game)
            .insert(ViewportPostProcessSettings {
                brightness: 1.1,
                contrast: 0.9,
                padding: Vec2::ZERO,
            });
        let node = app
            .world_mut()
            .spawn((
                Node {
                    width: Val::Px(16.0),
                    height: Val::Px(16.0),
                    ..default()
                },
                BackgroundColor(Color::linear_rgba(1.0, 0.0, 0.0, 0.5)),
                UiTargetCamera(ui),
            ))
            .id();

        let window = app
            .world_mut()
            .spawn((
                PrimaryWindow,
                Window {
                    resolution: (64, 64).into(),
                    ..default()
                },
            ))
            .id();
        *app.world_mut().resource_mut::<Settings>() = Settings {
            render_scale: 0.5,
            brightness: 103.4,
            contrast: 84.6,
            ..default()
        };

        *app.world_mut().get_mut::<Transform>(game).unwrap() =
            Transform::from_xyz(32.0, 32.0, 100.0).looking_at(Vec3::new(32.0, 32.0, 0.0), Vec3::Y);
        *app.world_mut().get_mut::<Projection>(game).unwrap() =
            Projection::Orthographic(OrthographicProjection {
                scaling_mode: bevy::camera::ScalingMode::Fixed {
                    width: 64.0,
                    height: 64.0,
                },
                ..OrthographicProjection::default_3d()
            });
        let mesh = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(Rectangle::new(8.0, 8.0));
        let material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: Color::linear_rgb(0.0, 1.0, 0.0),
                unlit: true,
                ..default()
            });
        app.world_mut().spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_xyz(48.0, 16.0, 0.0),
        ));
        let dimensions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = dimensions.clone();
        app.sub_app_mut(RenderApp).add_systems(
            Render,
            (move |views: Query<(
                &bevy::render::sync_world::MainEntity,
                &ViewTarget,
                &ExtractedView,
                Option<&bevy::render::view::ViewDepthTexture>,
                &ExtractedCamera,
            )>| {
                *observed.lock().unwrap() = views
                    .iter()
                    .map(|(entity, target, view, depth, camera)| {
                        let color = target.main_texture();
                        (
                            entity.id(),
                            UVec2::new(color.width(), color.height()),
                            target
                                .sampled_main_texture()
                                .map(|t| UVec2::new(t.width(), t.height())),
                            depth.map(|d| UVec2::new(d.texture.width(), d.texture.height())),
                            target
                                .sampled_main_texture()
                                .map_or(1, |t| t.sample_count()),
                            view.viewport,
                            matches!(camera.output_mode, CameraOutputMode::Skip),
                        )
                    })
                    .collect();
            })
            .after(RenderSystems::Prepare)
            .before(RenderSystems::Render),
        );
        let pixels = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let captured = pixels.clone();
        app.world_mut()
            .spawn(bevy::render::gpu_readback::Readback::texture(
                target.clone(),
            ))
            .observe(
                move |event: On<bevy::render::gpu_readback::ReadbackComplete>| {
                    let mut data = event.data.clone();
                    if bgra {
                        for pixel in data.chunks_exact_mut(4) {
                            pixel.swap(0, 2);
                        }
                    }
                    *captured.lock().unwrap() = data;
                },
            );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.plugins_state() != PluginsState::Ready {
            assert!(
                std::time::Instant::now() < deadline,
                "headless renderer initialization timed out"
            );
            bevy::tasks::tick_global_task_pools_on_main_thread();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        app.finish();
        app.cleanup();
        let features = app.world().resource::<RenderDevice>().features();
        let expects_epoch = features
            .intersects(WgpuFeatures::TIMESTAMP_QUERY | WgpuFeatures::PIPELINE_STATISTICS_QUERY);
        let epoch_path = DiagnosticPath::const_new("render/meteor_capture_epoch");
        let cpu_paths = [
            "render/main_opaque_pass_3d/elapsed_cpu",
            "render/meteor_display_post_process/elapsed_cpu",
            "render/ui/elapsed_cpu",
        ];
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "expected real render-pass diagnostics were not received"
            );
            app.update();
            app.world()
                .resource::<RenderDevice>()
                .poll(PollType::Poll)
                .unwrap();
            let store = app.world().resource::<DiagnosticsStore>();
            let ready = cpu_paths.iter().all(|path| {
                store
                    .get_measurement(&DiagnosticPath::new(*path))
                    .is_some_and(|m| m.value.is_finite() && m.value > 0.0)
            });
            let pixels_ready = {
                let data = pixels.lock().unwrap();
                let background = (32 * 64 + 32) * 4;
                let overlay = (8 * 64 + 8) * 4;
                data.len() >= 64 * 64 * 4
                    && data[background] < 100
                    && data[background + 2] > 180
                    && data[overlay] > 130
                    && data[overlay + 1] < 100
                    && data[overlay + 2] > 130
            };
            if ready
                && pixels_ready
                && (!expects_epoch || store.get_measurement(&epoch_path).is_some())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        {
            let dimensions = dimensions.lock().unwrap();
            let scene = dimensions.iter().find(|d| d.0 == game).unwrap();
            assert_eq!(scene.1, UVec2::splat(32));
            assert_eq!(scene.2, Some(UVec2::splat(32)));
            assert_eq!(scene.3, Some(UVec2::splat(32)));
            assert_eq!(scene.4, 4);
            assert_eq!(scene.5, UVec4::new(0, 0, 32, 32));
            assert!(scene.6, "built-in scene upscaling must be skipped");
            let ui_target = dimensions.iter().find(|d| d.0 == ui).unwrap();
            assert_eq!(ui_target.1, UVec2::splat(64));
            assert_eq!(ui_target.2, None);
            assert!(!ui_target.6, "retain UI composition");
        }
        assert_eq!(
            app.world()
                .get::<Camera>(game)
                .unwrap()
                .physical_target_size(),
            Some(UVec2::splat(64))
        );
        assert!(!matches!(
            app.world().get::<Camera>(game).unwrap().output_mode,
            CameraOutputMode::Skip
        ));
        let store = app.world().resource::<DiagnosticsStore>();
        if expects_epoch {
            let epoch = store.get(&epoch_path).unwrap();
            assert!(epoch.measurements().all(|m| m.value >= 1.0));
            for path in cpu_paths {
                let measured = store.get(&DiagnosticPath::new(path)).unwrap();
                assert!(
                    measured
                        .measurements()
                        .any(|m| epoch.measurements().any(|e| e.time == m.time)),
                    "capture marker must arrive in the same asynchronous result as {path}"
                );
            }
        }
        if !cfg!(target_os = "macos")
            && features.contains(WgpuFeatures::TIMESTAMP_QUERY_INSIDE_ENCODERS)
        {
            let gpu_path =
                DiagnosticPath::const_new("render/meteor_display_post_process/elapsed_gpu");
            let gpu = store
                .get_measurement(&gpu_path)
                .expect("GPU timestamp for the production post-process pass");
            assert!(gpu.value.is_finite() && gpu.value >= 0.0);
        }
        for diagnostic in store.iter().filter(|d| {
            d.path().as_str().ends_with("/elapsed_cpu") || d.path().as_str() == epoch_path.as_str()
        }) {
            eprintln!("render-test {} {:?}", diagnostic.path(), diagnostic.value());
        }
        assert!(
            store
                .get(&DiagnosticPath::const_new(
                    "render/msaa_writeback/elapsed_cpu"
                ))
                .is_none(),
            "UI camera must not cause a full-screen MSAA writeback"
        );
        app.world_mut().despawn(node);

        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "removed UI left stale pixels"
            );
            app.update();
            app.world()
                .resource::<RenderDevice>()
                .poll(PollType::Poll)
                .unwrap();
            let data = pixels.lock().unwrap();
            let overlay = (8 * 64 + 8) * 4;
            if data.len() >= 64 * 64 * 4 && data[overlay] < 100 && data[overlay + 2] > 180 {
                break;
            }
            drop(data);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        for (scale, brightness, contrast, width, height) in [
            (1.0, 94.0, 94.0, 64, 64),
            (0.65, 47.0, 141.0, 64, 64),
            (0.5, 94.0, 94.0, 128, 64),
            (1.0, 94.0, 94.0, 128, 64),
            (0.8, 94.0, 94.0, 64, 64),
        ] {
            *app.world_mut().resource_mut::<Settings>() = Settings {
                render_scale: scale,
                brightness,
                contrast,
                ..default()
            };
            app.world_mut()
                .get_mut::<Window>(window)
                .unwrap()
                .resolution
                .set_physical_resolution(width, height);
            app.world_mut()
                .resource_mut::<Assets<Image>>()
                .get_mut(&target)
                .unwrap()
                .resize(Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                });
            app.world_mut().get_mut::<Camera>(game).unwrap().clear_color =
                ClearColorConfig::Custom(Color::linear_rgb(0.25, 0.5, 0.75));
            let new_overlay = app
                .world_mut()
                .spawn((
                    Node {
                        width: Val::Px(16.0),
                        height: Val::Px(16.0),
                        ..default()
                    },
                    BackgroundColor(Color::linear_rgb(1.0, 0.0, 0.0)),
                    UiTargetCamera(ui),
                ))
                .id();
            let expected: Vec<u8> = [0.25_f32, 0.5, 0.75]
                .into_iter()
                .map(|linear| {
                    let srgb = 1.055 * linear.powf(1.0 / 2.4) - 0.055;
                    let value = ((srgb * (brightness / 94.0) - 0.5) * (contrast / 94.0) + 0.5)
                        .clamp(0.0, 1.0);
                    (value * 255.0).round() as u8
                })
                .collect();
            let scaled = UVec2::new(
                (width as f32 * scale).round() as u32,
                (height as f32 * scale).round() as u32,
            );
            let phase_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                assert!(std::time::Instant::now() < phase_deadline,
                    "scale/resize/color output did not converge: {scale} {width}x{height}, expected {expected:?}, dimensions {:?}", dimensions.lock().unwrap());
                app.update();
                app.world()
                    .resource::<RenderDevice>()
                    .poll(PollType::Poll)
                    .unwrap();
                let dims = dimensions.lock().unwrap();
                let ready = dims.iter().any(|d| {
                    d.0 == game
                        && d.1 == scaled
                        && d.2 == Some(scaled)
                        && d.3 == Some(scaled)
                        && d.4 == 4
                }) && dims
                    .iter()
                    .any(|d| d.0 == ui && d.1 == UVec2::new(width, height) && d.2.is_none());
                let data = pixels.lock().unwrap();
                let bg = ((height / 2 * width + width / 2) * 4) as usize;
                let overlay = ((8 * width + 8) * 4) as usize;
                let mesh = ((height * 3 / 4 * width + width * 3 / 4) * 4) as usize;
                let pixels_ready = data.len() == (width * height * 4) as usize
                    && (0..3).all(|c| data[bg + c].abs_diff(expected[c]) <= 3)
                    && data[overlay] > 245
                    && data[overlay + 1] < 5
                    && data[overlay + 2] < 5
                    && data[mesh + 1] > data[mesh].saturating_add(40);
                let reported = *app
                    .world()
                    .resource::<ActualRenderResolution>()
                    .0
                    .lock()
                    .unwrap();
                let report_ready = reported
                    == Some(SceneAttachmentSizes {
                        color: scaled,
                        msaa: Some(scaled),
                        depth: Some(scaled),
                        samples: 4,
                    });
                if ready && pixels_ready && report_ready {
                    break;
                }
                drop(data);
                drop(dims);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let main_camera = app.world().get::<Camera>(game).unwrap();
            assert_eq!(
                main_camera.physical_target_size(),
                Some(UVec2::new(width, height))
            );
            assert_eq!(
                main_camera.physical_viewport_size(),
                Some(UVec2::new(width, height))
            );
            app.world_mut().despawn(new_overlay);
        }
    }

    #[test]
    fn default_projection_is_byte_preserving() {
        let projection = display_projection(&Settings::default(), UVec2::new(1920, 1080), 2.0);
        assert_eq!(projection.brightness, 1.0);
        assert_eq!(projection.contrast, 1.0);
        assert_eq!(projection.resolution_override, None);
    }

    #[test]
    fn render_scale_scales_the_high_dpi_capped_baseline() {
        let settings = Settings {
            render_scale: 0.65,
            ..default()
        };
        let projection = display_projection(&settings, UVec2::new(1920, 1080), 2.0);
        assert_eq!(projection.resolution_override, Some(UVec2::new(1248, 702)));

        let capped = display_projection(&Settings::default(), UVec2::new(3000, 1500), 3.0);
        assert_eq!(capped.resolution_override, Some(UVec2::new(2000, 1000)));
    }

    #[test]
    fn poco_high_dpi_presets_have_distinct_actual_sizes() {
        let sizes = [1.0, 0.8, 0.65].map(|scale| {
            display_projection(
                &Settings {
                    render_scale: scale,
                    ..default()
                },
                UVec2::new(3200, 1440),
                3.5,
            )
            .resolution_override
            .unwrap()
        });
        assert_eq!(
            sizes,
            [
                UVec2::new(1829, 823),
                UVec2::new(1463, 658),
                UVec2::new(1189, 535)
            ]
        );
    }

    #[test]
    fn brightness_and_contrast_use_web_css_ratios() {
        let settings = Settings {
            brightness: 47.0,
            contrast: 141.0,
            ..default()
        };
        let projection = display_projection(&settings, UVec2::new(1280, 720), 1.0);
        assert!((projection.brightness - 0.5).abs() < 1e-6);
        assert!((projection.contrast - 1.5).abs() < 1e-6);
    }

    #[test]
    fn shader_applies_css_order_in_srgb_space() {
        let shader = include_str!("display_post_process.wgsl");
        assert!(shader.contains("linear_to_srgb"));
        assert!(shader.contains("srgb_to_linear"));
        assert!(shader.contains("srgb * settings.brightness"));
        assert!(shader.contains("(srgb - vec3<f32>(0.5)) * settings.contrast"));
        assert!(shader.contains("        in.uv,"));
    }

    #[test]
    fn shader_passes_wgsl_parser_and_validator() {
        let source = include_str!("display_post_process.wgsl").replace(
            "#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput",
            "struct FullscreenVertexOutput {\n\
             @builtin(position) position: vec4<f32>,\n\
             @location(0) uv: vec2<f32>,\n\
             };",
        );
        let module = naga::front::wgsl::parse_str(&source).expect("display WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("display WGSL validates");
    }

    #[test]
    fn production_sync_installs_and_removes_camera_components() {
        let mut app = App::new();
        app.insert_resource(Settings {
            render_scale: 0.65,
            ..default()
        })
        .add_systems(Update, sync_display_settings);
        app.world_mut().spawn((PrimaryWindow, Window::default()));
        let camera = app.world_mut().spawn(GameplayCamera).id();

        app.update();
        assert!(app
            .world()
            .get::<ViewportPostProcessSettings>(camera)
            .is_some());
        assert!(app.world().get::<RenderScaleResolution>(camera).is_some());

        *app.world_mut().resource_mut::<Settings>() = Settings::default();
        app.update();
        assert!(app
            .world()
            .get::<ViewportPostProcessSettings>(camera)
            .is_none());
        assert!(app.world().get::<RenderScaleResolution>(camera).is_none());
    }
}
