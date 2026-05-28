/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `pelt-live-counter`: Stage 1b-window of
//! `docs/2026-05-27_serval_as_host_xilem_serval_plan.md`.
//!
//! The visible payoff of the headless Stages 1a/1b/2a/2b: a real on-screen
//! winit window running an [`xilem_serval`] counter, rendered by serval and
//! presented through netrender. The window shows a big count number plus a
//! clickable `[ + ]` button. A background timer bumps the count ~1/s so the
//! number climbs on its own; clicking `[ + ]` bumps it too, proving the full
//! input loop on screen.
//!
//! The spine (the same one the headless probe asserts on, now driven by a
//! window):
//!
//! ```text
//! app state --(ServalAppRunner)--> ScriptedDom diff
//!           --(scene_from_scripted_dom: cascade -> layout -> paint emit)--> netrender::Scene
//!           --(Renderer::render_vello)--> Rgba8Unorm texture
//!           --(Renderer::compose_external_texture)--> wgpu::Surface backbuffer --> present
//! ```
//!
//! # The present path
//!
//! netrender's vello rasterizer writes into an `Rgba8Unorm` texture (it binds
//! the target as a storage texture), but a winit surface backbuffer is
//! typically `Bgra8UnormSrgb`. A raw `copy_texture_to_texture` requires
//! matching formats, so present is *not* a copy: it is a blit. netrender
//! already ships exactly that blit — [`Renderer::compose_external_texture`]
//! samples a source texture and draws it into a target view of any
//! `target_format` (the same zero-copy pass pelt-viewer uses for `<img>`
//! overlays). We point it at the surface's backbuffer view, so the bin adds no
//! GPU code of its own.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use netrender::external_texture::ExternalTexturePlacement;
use netrender::{ColorLoad, NetrenderOptions, Renderer, Scene};
use serval_scripted_dom::ScriptedDom;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};
use xilem_serval::{El, OnClick, PointerClick, ServalAppRunner, el, on_click};

use pelt_live::{hit_test_node, scene_from_scripted_dom};

// ── App state + view ───────────────────────────────────────────────────────

/// The app state: a single counter, mirroring the lib's Stage 1b probe.
struct Counter {
    count: u32,
}

/// The concrete button-counter view type, copied from the lib's test module:
/// `<div><p>{count}</p><button>+</button></div>`, the `<button>` carrying an
/// `on_click` that increments the count. The handler is a non-capturing
/// closure, so it coerces to a `fn` pointer and the view type is nameable
/// (no boxing). `<p>` carries the count text on its own line; `<button>` is
/// the click target.
type ButtonView = El<
    (
        El<String, Counter, ()>,
        OnClick<El<&'static str, Counter, ()>, Counter, (), fn(&mut Counter, PointerClick)>,
    ),
    Counter,
    (),
>;

fn button_counter_view(s: &Counter) -> ButtonView {
    let increment: fn(&mut Counter, PointerClick) = |s: &mut Counter, _ev| s.count += 1;
    el::<_, Counter, ()>(
        "div",
        (
            el::<_, Counter, ()>("p", s.count.to_string()),
            on_click(el::<_, Counter, ()>("button", "+"), increment),
        ),
    )
}

/// The author stylesheet. Block boxes so layout reaches every element; a large
/// font on the `<p>` makes the count visibly big; the `<button>` gets a little
/// padding/colour so the `[ + ]` target reads as a button. Kept minimal and
/// within what serval's cascade supports. The page background is the white
/// clear in [`App::render`] (the runner attaches the `<div>` directly under the
/// document root — there is no `<body>` element to style).
const SHEET: &[&str] = &[
    "div, p, button { display: block; }",
    "p { font-size: 96px; color: rgb(30, 30, 50); }",
    "button { font-size: 48px; color: rgb(255, 255, 255); \
        background-color: rgb(60, 120, 220); padding: 12px; }",
];

// ── winit user event ───────────────────────────────────────────────────────

/// The only user-injected event: the ~1Hz timer tick. A background thread
/// sleeps 1s and sends this through an [`EventLoopProxy`], so the timer lives
/// off the event loop without a busy-poll.
#[derive(Debug, Clone, Copy)]
enum UserEvent {
    Tick,
}

// ── GPU state (created on resume) ────────────────────────────────────────────

/// wgpu/netrender state, built once a window exists. Held together so the
/// surface, its config, and the renderer share one lifetime.
struct Gpu {
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

// ── The application ─────────────────────────────────────────────────────────

/// Logic alias: `button_counter_view` as the runner's logic closure type.
type Logic = fn(&Counter) -> ButtonView;

struct App {
    /// The shared document the runner mutates and the render path reads.
    dom: Rc<RefCell<ScriptedDom>>,
    runner: ServalAppRunner<Counter, Logic, ButtonView>,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    /// Last cursor position in physical pixels (window space == content space:
    /// the surface fills the window, so window coords are layout coords).
    cursor: (f32, f32),
    width: u32,
    height: u32,
}

impl App {
    fn new() -> Self {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = ServalAppRunner::new(
            dom.clone(),
            button_counter_view as Logic,
            Counter { count: 0 },
        );
        Self {
            dom,
            runner,
            window: None,
            gpu: None,
            cursor: (0.0, 0.0),
            width: 800,
            height: 600,
        }
    }

    /// Render the current DOM and present it to the surface backbuffer.
    ///
    /// 1. `scene_from_scripted_dom` runs the serval engine (cascade → layout →
    ///    paint emit) over the live `ScriptedDom` into a `netrender::Scene`.
    /// 2. `render_vello` rasterizes the scene into an `Rgba8Unorm` texture.
    /// 3. `compose_external_texture` blits that texture onto the surface's
    ///    (sRGB BGRA) backbuffer — the format-bridging present.
    fn render(&mut self) {
        let Some(gpu) = self.gpu.as_ref() else { return };
        let (w, h) = (self.width.max(1), self.height.max(1));

        // 1. Engine pipeline → Scene.
        let scene: Scene = scene_from_scripted_dom(&self.dom.borrow(), SHEET, w, h);

        // 2. Render the scene into a fresh Rgba8Unorm target. vello binds this
        //    as a storage texture (STORAGE_BINDING) and also reads it back via
        //    sampling for the present blit (TEXTURE_BINDING).
        let device = &gpu.renderer.wgpu_device.core.device;
        let content = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pelt-live-counter content"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
        });
        let content_view = content.create_view(&wgpu::TextureViewDescriptor {
            label: Some("pelt-live-counter content view"),
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        });
        gpu.renderer.render_vello(
            &scene,
            &content_view,
            ColorLoad::Clear(wgpu::Color::WHITE),
        );

        // 3. Acquire the surface backbuffer and blit the content onto it. The
        //    blit pass uses `LoadOp::Load`, so it draws over whatever is in the
        //    backbuffer; the full-viewport draw covers it entirely (the scene's
        //    body background paints the whole viewport), so no separate clear
        //    is needed.
        let frame = match gpu.surface.get_current_texture() {
            // Both Success and Suboptimal carry a usable frame; present it.
            // (Suboptimal just means a reconfigure would be more optimal, which
            // the next Resized handles.)
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                // The surface needs reconfiguring (e.g. a resize raced us).
                gpu.surface.configure(device, &gpu.surface_config);
                return;
            },
            // Timeout / Occluded / Validation: skip this frame, try again.
            other => {
                eprintln!("[pelt-live-counter] surface acquire skipped: {other:?}");
                return;
            },
        };
        let target_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        gpu.renderer.compose_external_texture(
            &content_view,
            &target_view,
            gpu.surface_config.format,
            w,
            h,
            ExternalTexturePlacement::new([0.0, 0.0, w as f32, h as f32]),
        );

        // `compose_external_texture` already submitted its encoder (it owns the
        // device + queue internally), so the blit is queued; present the frame.
        frame.present();
    }

    /// Reconfigure the surface for `(width, height)` and request a redraw.
    fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.surface_config.width = self.width;
            gpu.surface_config.height = self.height;
            gpu.surface
                .configure(&gpu.renderer.wgpu_device.core.device, &gpu.surface_config);
        }
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        // 1. Window.
        let attributes = Window::default_attributes()
            .with_title("Pelt Live — xilem-serval counter")
            .with_inner_size(PhysicalSize::new(self.width, self.height));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .expect("failed to create pelt-live-counter window"),
        );
        let size = window.inner_size();
        self.width = size.width.max(1);
        self.height = size.height.max(1);

        // 2. wgpu handles via netrender::boot (standalone instance/adapter/
        //    device/queue), then the netrender renderer over them.
        let handles = match netrender::boot() {
            Ok(handles) => handles,
            Err(err) => {
                eprintln!("[pelt-live-counter] netrender wgpu boot failed: {err}");
                event_loop.exit();
                return;
            },
        };

        // 3. Surface over the window, on the booted instance. The window is
        //    Arc-held so the surface can be `'static`.
        let surface = match handles.instance.create_surface(window.clone()) {
            Ok(surface) => surface,
            Err(err) => {
                eprintln!("[pelt-live-counter] create_surface failed: {err}");
                event_loop.exit();
                return;
            },
        };

        // 4. Surface configuration. Prefer an sRGB format from the adapter's
        //    supported set (the typical desktop backbuffer is Bgra8UnormSrgb);
        //    `compose_external_texture` builds its blit pipeline for whatever
        //    format we pick, so any supported format works.
        let caps = surface.get_capabilities(&handles.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: self.width,
            height: self.height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };

        let renderer = match netrender::create_netrender_instance(
            handles,
            NetrenderOptions {
                tile_cache_size: Some(64),
                enable_vello: true,
                ..Default::default()
            },
        ) {
            Ok(renderer) => renderer,
            Err(err) => {
                eprintln!("[pelt-live-counter] netrender init failed: {err:?}");
                event_loop.exit();
                return;
            },
        };
        surface.configure(&renderer.wgpu_device.core.device, &surface_config);

        self.gpu = Some(Gpu {
            surface,
            surface_config,
            renderer,
        });
        window.request_redraw();
        self.window = Some(window);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Tick => {
                // Timer tick: bump the count through the runner (state → DOM
                // diff), then redraw so the new number shows.
                self.runner.update(|s| s.count += 1);
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            },
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.window.as_ref().map(|w| w.id()) != Some(window_id) {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => self.resize(size.width, size.height),

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
            },

            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // Click input: hit-test the cursor through serval's existing
                // query, then dispatch a PointerClick to the hit node. If the
                // hit lands on (or under) the `[ + ]` button, its handler bumps
                // the count and the runner rebuilds.
                let (x, y) = self.cursor;
                let hit = hit_test_node(&self.dom.borrow(), SHEET, self.width, self.height, x, y);
                if let Some(node) = hit {
                    self.runner
                        .dispatch_click(node, PointerClick { local: (x, y) });
                    if let Some(window) = self.window.as_ref() {
                        window.request_redraw();
                    }
                }
            },

            WindowEvent::RedrawRequested => {
                self.render();
                // Continuous loop: keep redrawing so the timer-driven climb is
                // always reflected promptly (and the window stays responsive).
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            },

            _ => {},
        }
    }
}

fn main() {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to build event loop");

    // The ~1Hz timer: a background thread sleeps 1s and sends a Tick through
    // the proxy. It runs for the program's lifetime; send errors mean the loop
    // has exited, at which point the thread ends.
    let proxy: EventLoopProxy<UserEvent> = event_loop.create_proxy();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if proxy.send_event(UserEvent::Tick).is_err() {
                break;
            }
        }
    });

    let mut app = App::new();
    event_loop
        .run_app(&mut app)
        .expect("pelt-live-counter event loop failed");
}
