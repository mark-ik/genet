// A hardened pure-wgpu Servo embedder.
//
// Demonstrates the shared-device pipeline: the host app owns the wgpu device
// and surface, passes the device to Servo/WebRender via a custom
// RenderingContext, and composites WebRender's output texture into its own
// render pass — zero-copy on the GPU.
//
// Features over the minimal seed:
//   - sRGB colour fix: surface viewed as Bgra8Unorm to avoid double-encoding
//   - Proper resize: webview.resize() propagates through Servo/WebRender
//   - HiDPI: scale factor propagated on ScaleFactorChanged
//   - Mouse: move, click, wheel, leave-viewport all routed to Servo
//   - Keyboard: full key translation via keyutils module
//   - Cursor: window cursor icon updated from Servo's cursor change callbacks
//   - Title: window title tracks the loaded page title
//   - Shutdown: clean servo.deinit() on window close
//
// Usage:
//   cargo run -p wgpu-embedder -- <url>

mod keyutils;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dpi::PhysicalSize;
use euclid::{Scale, Size2D};
use image::RgbaImage;
use log::info;
use paint_api::rendering_context::RenderingContext;
use servo::{
    Cursor, DeviceIntRect, DevicePixel, DevicePoint, EventLoopWaker, InputEvent,
    MouseButton as ServoMouseButton, MouseButtonAction, MouseButtonEvent, MouseLeftViewportEvent,
    MouseMoveEvent, ServoBuilder, WebView, WebViewBuilder, WebViewDelegate, WheelDelta, WheelEvent,
    WheelMode,
};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Window, WindowAttributes, WindowId};

// Pixels per "line" for line-delta scroll events.
const LINE_HEIGHT: f32 = 76.0;
const LINE_WIDTH: f32 = 76.0;

// ---------------------------------------------------------------------------
// WgpuRenderingContext: a pure-wgpu RenderingContext for Servo
// ---------------------------------------------------------------------------

struct WgpuRenderingContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    size: Cell<PhysicalSize<u32>>,
}

impl WgpuRenderingContext {
    fn new(device: wgpu::Device, queue: wgpu::Queue, size: PhysicalSize<u32>) -> Self {
        Self {
            device,
            queue,
            size: Cell::new(size),
        }
    }
}

impl RenderingContext for WgpuRenderingContext {
    fn size(&self) -> PhysicalSize<u32> {
        self.size.get()
    }

    fn size2d(&self) -> Size2D<u32, DevicePixel> {
        let s = self.size.get();
        Size2D::new(s.width, s.height)
    }

    fn resize(&self, size: PhysicalSize<u32>) {
        self.size.set(size);
    }

    fn present(&self) {}

    fn prepare_for_rendering(&self) {}

    fn make_current(&self) -> Result<(), surfman::Error> {
        Ok(())
    }

    fn gleam_gl_api(&self) -> Rc<dyn gleam::gl::Gl> {
        unreachable!("gleam_gl_api called on pure-wgpu RenderingContext")
    }

    fn glow_gl_api(&self) -> Arc<glow::Context> {
        unreachable!("glow_gl_api called on pure-wgpu RenderingContext")
    }

    fn read_to_image(&self, _source_rectangle: DeviceIntRect) -> Option<RgbaImage> {
        None
    }

    fn wgpu_device(&self) -> Option<wgpu::Device> {
        Some(self.device.clone())
    }

    fn wgpu_queue(&self) -> Option<wgpu::Queue> {
        Some(self.queue.clone())
    }
}

// ---------------------------------------------------------------------------
// Delegate
// ---------------------------------------------------------------------------

struct EmbedderDelegate {
    frame_ready: Arc<AtomicBool>,
    event_proxy: EventLoopProxy<AppEvent>,
}

#[derive(Debug)]
enum AppEvent {
    WakeUp,
    CursorChanged(CursorIcon),
    TitleChanged(Option<String>),
}

impl WebViewDelegate for EmbedderDelegate {
    fn notify_new_frame_ready(&self, webview: WebView) {
        webview.render();
        self.frame_ready.store(true, Ordering::Relaxed);
    }

    fn notify_cursor_changed(&self, _webview: WebView, cursor: Cursor) {
        let icon = servo_cursor_to_winit(cursor);
        let _ = self.event_proxy.send_event(AppEvent::CursorChanged(icon));
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        let _ = self.event_proxy.send_event(AppEvent::TitleChanged(title));
    }
}

fn servo_cursor_to_winit(cursor: Cursor) -> CursorIcon {
    match cursor {
        Cursor::Default => CursorIcon::Default,
        Cursor::Pointer => CursorIcon::Pointer,
        Cursor::Text => CursorIcon::Text,
        Cursor::VerticalText => CursorIcon::VerticalText,
        Cursor::Move => CursorIcon::Move,
        Cursor::EResize => CursorIcon::EResize,
        Cursor::NResize => CursorIcon::NResize,
        Cursor::NeResize => CursorIcon::NeResize,
        Cursor::NwResize => CursorIcon::NwResize,
        Cursor::SResize => CursorIcon::SResize,
        Cursor::SeResize => CursorIcon::SeResize,
        Cursor::SwResize => CursorIcon::SwResize,
        Cursor::WResize => CursorIcon::WResize,
        Cursor::EwResize => CursorIcon::EwResize,
        Cursor::NsResize => CursorIcon::NsResize,
        Cursor::NwseResize => CursorIcon::NwseResize,
        Cursor::NeswResize => CursorIcon::NeswResize,
        Cursor::ColResize => CursorIcon::ColResize,
        Cursor::RowResize => CursorIcon::RowResize,
        Cursor::AllScroll => CursorIcon::AllScroll,
        Cursor::ZoomIn => CursorIcon::ZoomIn,
        Cursor::ZoomOut => CursorIcon::ZoomOut,
        Cursor::Grab => CursorIcon::Grab,
        Cursor::Grabbing => CursorIcon::Grabbing,
        Cursor::Crosshair => CursorIcon::Crosshair,
        Cursor::Copy => CursorIcon::Copy,
        Cursor::Alias => CursorIcon::Alias,
        Cursor::ContextMenu => CursorIcon::ContextMenu,
        Cursor::Help => CursorIcon::Help,
        Cursor::Progress => CursorIcon::Progress,
        Cursor::Wait => CursorIcon::Wait,
        Cursor::Cell => CursorIcon::Cell,
        Cursor::NoDrop => CursorIcon::NoDrop,
        Cursor::NotAllowed => CursorIcon::NotAllowed,
        Cursor::None => CursorIcon::Default,
    }
}

// ---------------------------------------------------------------------------
// EventLoopWaker
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct WinitWaker {
    proxy: EventLoopProxy<AppEvent>,
}

impl EventLoopWaker for WinitWaker {
    fn wake(&self) {
        let _ = self.proxy.send_event(AppEvent::WakeUp);
    }

    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

struct App {
    waker: Box<dyn EventLoopWaker>,
    proxy: EventLoopProxy<AppEvent>,
    url: String,
    state: Option<RunningState>,
    frame_ready: Arc<AtomicBool>,
}

struct RunningState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    servo: servo::Servo,
    webview: WebView,
    blit_pipeline: wgpu::RenderPipeline,
    blit_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    // Input tracking
    cursor_pos: Cell<DevicePoint>,
    cursor_in_window: Cell<bool>,
    modifiers: Cell<ModifiersState>,
    hidpi_scale: f64,
}

impl App {
    fn new(proxy: EventLoopProxy<AppEvent>, waker: Box<dyn EventLoopWaker>, url: String) -> Self {
        Self {
            waker,
            proxy,
            url,
            state: None,
            frame_ready: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title("wgpu-embedder (Servo)")
                        .with_inner_size(winit::dpi::PhysicalSize::new(1024u32, 768)),
                )
                .expect("Failed to create window"),
        );

        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .expect("Failed to create surface");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("No suitable GPU adapter found");

        info!("GPU: {}", adapter.get_info().name);

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("host"),
                ..Default::default()
            },
        ))
        .expect("Failed to create device");

        let win_size = window.inner_size();
        let hidpi_scale = window.scale_factor();

        // Use Bgra8UnormSrgb as the native preferred format, but expose
        // Bgra8Unorm as an additional view_format so we can create the surface
        // view without the automatic sRGB re-encode.  WebRender's composite
        // output is already display-encoded (sRGB); a second encode would
        // produce washed-out/wrong colours.
        let preferred_format = surface.get_capabilities(&adapter).formats[0];
        let non_srgb_format = preferred_format.remove_srgb_suffix();
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: preferred_format,
            width: win_size.width.max(1),
            height: win_size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            // Allow creating a non-sRGB view over the sRGB surface texture
            // so writes bypass the automatic gamma encoding.
            view_formats: if non_srgb_format != preferred_format {
                vec![non_srgb_format]
            } else {
                vec![]
            },
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        // Blit pipeline — fullscreen triangle that samples WebRender's texture.
        let blit_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("blit_bind_group_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let blit_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("blit_pipeline_layout"),
                bind_group_layouts: &[&blit_bind_group_layout],
                push_constant_ranges: &[],
            });

        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit_shader"),
            source: wgpu::ShaderSource::Wgsl(BLIT_SHADER_WGSL.into()),
        });

        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit_pipeline"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    // Write to the non-sRGB view to skip automatic gamma encoding.
                    format: non_srgb_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Rendering context wraps the wgpu device for Servo.
        let rendering_context: Rc<dyn RenderingContext> = Rc::new(WgpuRenderingContext::new(
            device.clone(),
            queue.clone(),
            PhysicalSize::new(win_size.width, win_size.height),
        ));

        let servo = ServoBuilder::default()
            .event_loop_waker(self.waker.clone())
            .build();
        servo.setup_logging();

        let url = servo::ServoUrl::parse(&self.url)
            .unwrap_or_else(|_| servo::ServoUrl::parse("about:blank").unwrap());
        let webview = WebViewBuilder::new(&servo, rendering_context)
            .url(url.as_url().clone())
            .hidpi_scale_factor(Scale::new(hidpi_scale as f32))
            .delegate(Rc::new(EmbedderDelegate {
                frame_ready: self.frame_ready.clone(),
                event_proxy: self.proxy.clone(),
            }))
            .build();

        self.state = Some(RunningState {
            window,
            surface,
            surface_config,
            device,
            queue,
            servo,
            webview,
            blit_pipeline,
            blit_bind_group_layout,
            sampler,
            cursor_pos: Cell::new(DevicePoint::zero()),
            cursor_in_window: Cell::new(false),
            modifiers: Cell::new(ModifiersState::empty()),
            hidpi_scale,
        });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.state else {
            return;
        };

        match event {
            // ---- Window lifecycle ----
            WindowEvent::CloseRequested => {
                // Drop the state (and servo) before exiting.
                self.state = None;
                event_loop.exit();
            }

            // ---- Resize ----
            WindowEvent::Resized(new_size) => {
                state.surface_config.width = new_size.width.max(1);
                state.surface_config.height = new_size.height.max(1);
                state.surface.configure(&state.device, &state.surface_config);
                // Propagate to Servo/WebRender through the webview handle.
                state.webview.resize(new_size);
                state.window.request_redraw();
            }

            // ---- HiDPI scale change ----
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                state.hidpi_scale = scale_factor;
                state.webview.set_hidpi_scale_factor(Scale::new(scale_factor as f32));
            }

            // ---- Redraw ----
            WindowEvent::RedrawRequested => {
                let frame = match state.surface.get_current_texture() {
                    Ok(f) => f,
                    Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                        state.surface.configure(&state.device, &state.surface_config);
                        return;
                    }
                    Err(e) => {
                        log::error!("Surface error: {e:?}");
                        return;
                    }
                };

                // Create a non-sRGB view to avoid double-encoding WR's output.
                let non_srgb_format = state.surface_config.format.remove_srgb_suffix();
                let surface_view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
                    format: Some(non_srgb_format),
                    ..Default::default()
                });
                let mut encoder = state.device.create_command_encoder(&Default::default());

                if let Some(wr_texture) = state.webview.composite_texture() {
                    let wr_view = wr_texture.create_view(&wgpu::TextureViewDescriptor {
                        format: Some(wgpu::TextureFormat::Bgra8Unorm),
                        ..Default::default()
                    });

                    let bind_group = state.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("blit_bind_group"),
                        layout: &state.blit_bind_group_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&wr_view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&state.sampler),
                            },
                        ],
                    });

                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("blit_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &surface_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                            depth_slice: None,
                        })],
                        depth_stencil_attachment: None,
                        ..Default::default()
                    });
                    pass.set_pipeline(&state.blit_pipeline);
                    pass.set_bind_group(0, &bind_group, &[]);
                    pass.draw(0..3, 0..1);
                } else {
                    // No composite texture yet — clear to loading colour.
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("host_clear"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &surface_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.15,
                                    g: 0.15,
                                    b: 0.15,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                            depth_slice: None,
                        })],
                        depth_stencil_attachment: None,
                        ..Default::default()
                    });
                }

                state.queue.submit(Some(encoder.finish()));
                frame.present();
                state.window.request_redraw();
            }

            // ---- Mouse ----
            WindowEvent::CursorMoved { position, .. } => {
                let pt = DevicePoint::new(position.x as f32, position.y as f32);
                state.cursor_pos.set(pt);
                state.cursor_in_window.set(true);
                state.webview.notify_input_event(InputEvent::MouseMove(
                    MouseMoveEvent::new(pt.into()),
                ));
            }
            WindowEvent::CursorLeft { .. } => {
                state.cursor_in_window.set(false);
                state
                    .webview
                    .notify_input_event(InputEvent::MouseLeftViewport(
                        MouseLeftViewportEvent::default(),
                    ));
            }
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
                let point = state.cursor_pos.get();
                let servo_button = winit_button_to_servo(button);
                let action = match btn_state {
                    ElementState::Pressed => MouseButtonAction::Down,
                    ElementState::Released => MouseButtonAction::Up,
                };
                state.webview.notify_input_event(InputEvent::MouseButton(
                    MouseButtonEvent::new(action, servo_button, point.into()),
                ));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let point = state.cursor_pos.get();
                let (dx, dy, mode) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        ((x * LINE_WIDTH) as f64, (y * LINE_HEIGHT) as f64, WheelMode::DeltaPixel)
                    }
                    MouseScrollDelta::PixelDelta(d) => (d.x, d.y, WheelMode::DeltaPixel),
                };
                state.webview.notify_input_event(InputEvent::Wheel(WheelEvent::new(
                    WheelDelta { x: dx, y: dy, z: 0.0, mode },
                    point.into(),
                )));
            }

            // ---- Keyboard ----
            WindowEvent::ModifiersChanged(mods) => {
                state.modifiers.set(mods.state());
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let mods = state.modifiers.get();
                let keyboard_event = keyutils::keyboard_event_from_winit(&event, mods);
                state
                    .webview
                    .notify_input_event(InputEvent::Keyboard(keyboard_event));
            }

            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        let Some(state) = &self.state else {
            return;
        };
        match event {
            AppEvent::WakeUp => {
                state.servo.spin_event_loop();
                if self.frame_ready.swap(false, Ordering::Relaxed) {
                    state.window.request_redraw();
                }
            }
            AppEvent::CursorChanged(icon) => {
                state.window.set_cursor(icon);
            }
            AppEvent::TitleChanged(title) => {
                let t = title.unwrap_or_else(|| "wgpu-embedder (Servo)".to_string());
                state.window.set_title(&t);
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.servo.spin_event_loop();
            if self.frame_ready.swap(false, Ordering::Relaxed) {
                state.window.request_redraw();
            }
        }
    }
}

fn winit_button_to_servo(button: MouseButton) -> ServoMouseButton {
    match button {
        MouseButton::Left => ServoMouseButton::Left,
        MouseButton::Right => ServoMouseButton::Right,
        MouseButton::Middle => ServoMouseButton::Middle,
        MouseButton::Back => ServoMouseButton::Back,
        MouseButton::Forward => ServoMouseButton::Forward,
        MouseButton::Other(v) => ServoMouseButton::Other(v),
    }
}

// Fullscreen-triangle blit shader (WGSL).
const BLIT_SHADER_WGSL: &str = r#"
@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var s_input: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let x = f32(i32(vi & 1u)) * 4.0 - 1.0;
    let y = f32(i32(vi >> 1u)) * 4.0 - 1.0;
    var out: VertexOutput;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(t_input, s_input, in.uv);
}
"#;

fn main() {
    // SAFETY: called before any threads are spawned.
    unsafe { std::env::set_var("SERVO_WGPU_BACKEND", "1") };

    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://servo.org".to_string());

    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .expect("Failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let waker = Box::new(WinitWaker { proxy: proxy.clone() });

    let mut app = App::new(proxy, waker, url);
    event_loop.run_app(&mut app).expect("Event loop failed");
}
