

extern crate wgpu_hal as hal;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "ios",
    target_os = "visionos",
    target_env = "ohos"
)))]
fn main() {
    use std::ffi::CString;

    use glutin::{
        config::GlConfig as _,
        context::{NotCurrentGlContext as _, PossiblyCurrentGlContext as _, Version},
        display::{GetGlDisplay as _, GlDisplay as _},
        surface::GlSurface as _,
    };
    use glutin_winit::GlWindow as _;
    use raw_window_handle::HasWindowHandle as _;
    use winit::{
        application::ApplicationHandler,
        event::{KeyEvent, WindowEvent},
        event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
        keyboard::{Key, NamedKey},
        window::Window,
    };

    env_logger::init();
    println!("Initializing external GL context");

    fn gl_config_picker(
        configs: Box<dyn Iterator<Item = glutin::config::Config> + '_>,
    ) -> glutin::config::Config {
        configs
            .reduce(|accum, config| {
                if config.num_samples() > accum.num_samples() {
                    config
                } else {
                    accum
                }
            })
            .expect("Failed to find a matching config")
    }

    struct App {
        gl_config: Option<glutin::config::Config>,
        not_current_gl_context: Option<glutin::context::NotCurrentContext>,
        state: Option<(
            glutin::context::PossiblyCurrentContext,
            glutin::surface::Surface<glutin::surface::WindowSurface>,
            Window,
        )>,
        exposed: Option<hal::ExposedAdapter<hal::api::Gles>>,
    }

    impl App {
        fn new() -> Self {
            Self {
                gl_config: None,
                not_current_gl_context: None,
                state: None,
                exposed: None,
            }
        }
    }

    impl ApplicationHandler for App {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {

            if self.gl_config.is_none() {

                let window_attributes = cfg!(windows).then(|| {
                    Window::default_attributes()
                        .with_title("wgpu raw GLES example (press Escape to exit)")
                });

                let template = glutin::config::ConfigTemplateBuilder::new();

                let display_builder =
                    glutin_winit::DisplayBuilder::new().with_window_attributes(window_attributes);

                let (window, gl_config) = display_builder
                    .build(event_loop, template, gl_config_picker)
                    .expect("Failed to build window and config from display");

                println!("Picked a config with {} samples", gl_config.num_samples());

                let raw_window_handle = window
                    .as_ref()
                    .and_then(|window| window.window_handle().ok())
                    .map(|handle| handle.as_raw());

                let gl_display = gl_config.display();

                let context_attributes = glutin::context::ContextAttributesBuilder::new()

                    .with_context_api(glutin::context::ContextApi::Gles(Some(Version::new(3, 0))))
                    .build(raw_window_handle);

                let gl_context = unsafe {
                    gl_display
                        .create_context(&gl_config, &context_attributes)
                        .expect("failed to create context")
                };

                self.not_current_gl_context = Some(gl_context);
                self.gl_config = Some(gl_config);

                if let Some(window) = window {
                    self.create_surface(event_loop, window);
                    return;
                }
            }

            if self.state.is_none() {
                let gl_config = self.gl_config.as_ref().unwrap();
                let window = glutin_winit::finalize_window(
                    event_loop,
                    Window::default_attributes()
                        .with_title("wgpu raw GLES example (press Escape to exit)"),
                    gl_config,
                )
                .unwrap();
                self.create_surface(event_loop, window);
            }
        }

        fn suspended(&mut self, _event_loop: &ActiveEventLoop) {

            println!("Android window removed");

            if let Some((gl_context, ..)) = self.state.take() {
                assert!(self
                    .not_current_gl_context
                    .replace(gl_context.make_not_current().unwrap())
                    .is_none());
            }
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            _window_id: winit::window::WindowId,
            event: WindowEvent,
        ) {
            event_loop.set_control_flow(ControlFlow::Wait);

            match event {
                WindowEvent::CloseRequested
                | WindowEvent::KeyboardInput {
                    event:
                        KeyEvent {
                            logical_key: Key::Named(NamedKey::Escape),
                            ..
                        },
                    ..
                } => event_loop.exit(),
                WindowEvent::Resized(size) => {
                    if size.width != 0 && size.height != 0 {

                        if let Some((gl_context, gl_surface, window)) = &self.state {
                            window.resize_surface(gl_surface, gl_context);
                        }
                    }
                }
                WindowEvent::RedrawRequested => {
                    if let (Some(exposed), Some((gl_context, gl_surface, window))) =
                        (&self.exposed, &self.state)
                    {
                        let inner_size = window.inner_size();

                        fill_screen(exposed, inner_size.width, inner_size.height);

                        println!("Showing the window");
                        gl_surface
                            .swap_buffers(gl_context)
                            .expect("Failed to swap buffers");
                    }
                }
                _ => (),
            }
        }
    }

    impl App {
        fn create_surface(&mut self, _event_loop: &ActiveEventLoop, window: Window) {
            let gl_config = self.gl_config.as_ref().unwrap();

            let attrs = window
                .build_surface_attributes(Default::default())
                .expect("Failed to build surface attributes");
            let gl_surface = unsafe {
                gl_config
                    .display()
                    .create_window_surface(gl_config, &attrs)
                    .expect("Cannot create GL WindowSurface")
            };

            let gl_context = self
                .not_current_gl_context
                .take()
                .unwrap()
                .make_current(&gl_surface)
                .expect("GL context cannot be made current with WindowSurface");

            println!("Hooking up to wgpu-hal");
            self.exposed.get_or_insert_with(|| {
                unsafe {
                    <hal::api::Gles as hal::Api>::Adapter::new_external(
                        |name| {

                            gl_config
                                .display()
                                .get_proc_address(&CString::new(name).expect(name))
                        },
                        wgpu_types::GlBackendOptions::default(),
                    )
                }
                .expect("GL adapter can't be initialized")
            });

            window.request_redraw();
            self.state = Some((gl_context, gl_surface, window));
        }
    }

    let event_loop = EventLoop::new().unwrap();
    let mut app = App::new();
    event_loop
        .run_app(&mut app)
        .expect("Couldn't run event loop");
}

#[cfg(target_os = "emscripten")]
fn main() {
    env_logger::init();

    println!("Initializing external GL context");
    let egl = khronos_egl::Instance::new(khronos_egl::Static);
    let display = unsafe { egl.get_display(khronos_egl::DEFAULT_DISPLAY) }.unwrap();
    egl.initialize(display)
        .expect("unable to initialize display");

    let attributes = [
        khronos_egl::RED_SIZE,
        8,
        khronos_egl::GREEN_SIZE,
        8,
        khronos_egl::BLUE_SIZE,
        8,
        khronos_egl::NONE,
    ];

    let config = egl
        .choose_first_config(display, &attributes)
        .unwrap()
        .expect("unable to choose config");
    let surface = unsafe {
        let window = std::ptr::null_mut::<std::ffi::c_void>();
        egl.create_window_surface(display, config, window, None)
    }
    .expect("unable to create surface");

    let context_attributes = [khronos_egl::CONTEXT_CLIENT_VERSION, 3, khronos_egl::NONE];

    let gl_context = egl
        .create_context(display, config, None, &context_attributes)
        .expect("unable to create context");
    egl.make_current(display, Some(surface), Some(surface), Some(gl_context))
        .expect("can't make context current");

    println!("Hooking up to wgpu-hal");
    let exposed = unsafe {
        <hal::api::Gles as hal::Api>::Adapter::new_external(|name| {
            egl.get_proc_address(name)
                .map_or(std::ptr::null(), |p| p as *const _)
        })
    }
    .expect("GL adapter can't be initialized");

    fill_screen(&exposed, 640, 400);
}

#[cfg(any(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    target_os = "ios",
    target_os = "visionos",
    target_env = "ohos"
))]
fn main() {
    eprintln!("This example is not supported on this platform")
}

#[cfg(not(any(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    target_os = "ios",
    target_os = "visionos"
)))]
fn fill_screen(exposed: &hal::ExposedAdapter<hal::api::Gles>, width: u32, height: u32) {
    use hal::{Adapter as _, CommandEncoder as _, Device as _, Queue as _};

    let od = unsafe {
        exposed.adapter.open(
            wgpu_types::Features::empty(),
            &wgpu_types::Limits::downlevel_defaults(),
            &wgpu_types::MemoryHints::default(),
        )
    }
    .unwrap();

    let format = wgpu_types::TextureFormat::Rgba8UnormSrgb;
    let texture = <hal::api::Gles as hal::Api>::Texture::default_framebuffer(format);
    let view = unsafe {
        od.device
            .create_texture_view(
                &texture,
                &hal::TextureViewDescriptor {
                    label: None,
                    format,
                    dimension: wgpu_types::TextureViewDimension::D2,
                    usage: wgpu_types::TextureUses::COLOR_TARGET,
                    range: wgpu_types::ImageSubresourceRange::default(),
                },
            )
            .unwrap()
    };

    println!("Filling the screen");
    let mut encoder = unsafe {
        od.device
            .create_command_encoder(&hal::CommandEncoderDescriptor {
                label: None,
                queue: &od.queue,
            })
            .unwrap()
    };
    let mut fence = unsafe { od.device.create_fence().unwrap() };
    let rp_desc = hal::RenderPassDescriptor {
        label: None,
        extent: wgpu_types::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        sample_count: 1,
        color_attachments: &[Some(hal::ColorAttachment {
            target: hal::Attachment {
                view: &view,
                usage: wgpu_types::TextureUses::COLOR_TARGET,
            },
            depth_slice: None,
            resolve_target: None,
            ops: hal::AttachmentOps::STORE | hal::AttachmentOps::LOAD_CLEAR,
            clear_value: wgpu_types::Color::BLUE,
        })],
        depth_stencil_attachment: None,
        multiview_mask: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    };
    unsafe {
        encoder.begin_encoding(None).unwrap();
        encoder.begin_render_pass(&rp_desc).unwrap();
        encoder.end_render_pass();
        let cmd_buf = encoder.end_encoding().unwrap();
        od.queue.submit(&[&cmd_buf], &[], (&mut fence, 0)).unwrap();
    }
}
