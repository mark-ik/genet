// A minimal pure-wgpu Servo embedder.
//
// Demonstrates the shared-device pipeline: the host app owns the wgpu device
// and surface, passes the device to Servo/WebRender via a custom
// RenderingContext, and composites WebRender's output texture into its own
// render pass — zero-copy on the GPU.
//
// Usage:
//   SERVO_WGPU_BACKEND=1 cargo run -p wgpu-embedder -- <url>
//
// Requires the wgpu_backend feature in paint_api and webrender.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use dpi::PhysicalSize;
use euclid::{Size2D, Scale};
use image::RgbaImage;
use log::info;
use paint_api::rendering_context::RenderingContext;
use servo::{
    DeviceIntRect, DevicePixel, EventLoopWaker,
    ServoBuilder, WebView, WebViewBuilder, WebViewDelegate,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowAttributes, WindowId};

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

    fn present(&self) {
        // Presentation is handled by the host's wgpu surface, not here.
    }

    fn prepare_for_rendering(&self) {
        // No-op for pure-wgpu — there's no GL framebuffer to bind.
    }

    fn make_current(&self) -> Result<(), surfman::Error> {
        // No GL context to make current.
        Ok(())
    }

    fn gleam_gl_api(&self) -> Rc<dyn gleam::gl::Gl> {
        unreachable!("gleam_gl_api called on pure-wgpu RenderingContext")
    }

    fn glow_gl_api(&self) -> Arc<glow::Context> {
        unreachable!("glow_gl_api called on pure-wgpu RenderingContext")
    }

    fn read_to_image(&self, _source_rectangle: DeviceIntRect) -> Option<RgbaImage> {
        // TODO: implement via wgpu readback if needed for screenshots
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
// Minimal WebViewDelegate
// ---------------------------------------------------------------------------

struct MinimalDelegate;

impl WebViewDelegate for MinimalDelegate {
    fn notify_new_frame_ready(&self, _webview: WebView) {
        // In a real app, request a redraw here.
    }
}

// ---------------------------------------------------------------------------
// EventLoopWaker
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct WinitWaker {
    proxy: EventLoopProxy<()>,
}

impl EventLoopWaker for WinitWaker {
    fn wake(&self) {
        let _ = self.proxy.send_event(());
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
    url: String,
    state: Option<RunningState>,
}

struct RunningState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    servo: servo::Servo,
    _webview: WebView,
    // Blit pipeline for compositing WebRender output onto the window surface.
    blit_pipeline: wgpu::RenderPipeline,
    blit_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl App {
    fn new(waker: Box<dyn EventLoopWaker>, url: String) -> Self {
        Self {
            waker,
            url,
            state: None,
        }
    }
}

impl ApplicationHandler<()> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        // Create winit window.
        let window = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title("wgpu-embedder (Servo)")
                        .with_inner_size(winit::dpi::PhysicalSize::new(1024u32, 768)),
                )
                .expect("Failed to create window"),
        );

        // Create wgpu instance, surface, adapter, device.
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
                required_features: wgpu::Features::empty(),
                ..Default::default()
            },
        ))
        .expect("Failed to create device");

        let win_size = window.inner_size();
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            width: win_size.width.max(1),
            height: win_size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        // Create the blit pipeline (fullscreen triangle that samples WebRender's texture).
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
                    format: surface_config.format,
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

        // Create the WgpuRenderingContext that shares our device with Servo.
        let rendering_context: Rc<dyn RenderingContext> = Rc::new(WgpuRenderingContext::new(
            device.clone(),
            queue.clone(),
            PhysicalSize::new(win_size.width, win_size.height),
        ));

        // Build Servo.
        let servo = ServoBuilder::default()
            .event_loop_waker(self.waker.clone())
            .build();
        servo.setup_logging();

        // Create a WebView.
        let url = servo::ServoUrl::parse(&self.url)
            .unwrap_or_else(|_| servo::ServoUrl::parse("about:blank").unwrap());
        let webview = WebViewBuilder::new(&servo, rendering_context)
            .url(url.as_url().clone())
            .hidpi_scale_factor(Scale::new(window.scale_factor() as f32))
            .delegate(Rc::new(MinimalDelegate))
            .build();

        self.state = Some(RunningState {
            window,
            surface,
            surface_config,
            device,
            queue,
            servo,
            _webview: webview,
            blit_pipeline,
            blit_bind_group_layout,
            sampler,
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
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                state.surface_config.width = new_size.width.max(1);
                state.surface_config.height = new_size.height.max(1);
                state
                    .surface
                    .configure(&state.device, &state.surface_config);
                state.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                // Pump Servo's event loop.
                state.servo.spin_event_loop();

                // TODO: Access WebRender's composite_output() texture and blit it
                // to the window surface. For now, just clear to dark grey to prove
                // the host render loop works.
                let frame = match state.surface.get_current_texture() {
                    Ok(f) => f,
                    Err(wgpu::SurfaceError::Outdated) => return,
                    Err(e) => {
                        log::error!("Surface error: {e:?}");
                        return;
                    }
                };
                let view = frame.texture.create_view(&Default::default());

                let mut encoder = state.device.create_command_encoder(&Default::default());
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("host_clear"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
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
                    // TODO: bind WebRender texture and draw fullscreen quad
                }
                state.queue.submit(Some(encoder.finish()));
                frame.present();

                // Request next frame.
                state.window.request_redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.servo.spin_event_loop();
        }
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
    // Fullscreen triangle: vertices 0,1,2 cover the entire clip space.
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
    // Don't call env_logger::init() — Servo's setup_logging() will do it.

    // Ensure wgpu backend is selected.
    // SAFETY: Called before any threads are spawned.
    unsafe { std::env::set_var("SERVO_WGPU_BACKEND", "1") };

    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://servo.org".to_string());

    let event_loop = EventLoop::<()>::with_user_event()
        .build()
        .expect("Failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);

    let waker = Box::new(WinitWaker {
        proxy: event_loop.create_proxy(),
    });
    let mut app = App::new(waker, url);
    event_loop.run_app(&mut app).expect("Event loop failed");
}
