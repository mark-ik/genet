/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacosCALayerPresentSmokeOutcome {
    pub width: u32,
    pub height: u32,
    pub frames_presented: u32,
    pub created_window: bool,
}

/// Configuration for [`run_macos_calayer_present_smoke`]. `frames` is
/// the number of redraw-presents to fire before exiting the loop;
/// `1` is enough to validate the construction + present path.
/// `0` is the "run until the user closes the window" sentinel,
/// useful for visual inspection.
///
/// Setting `declare_subsurface = true` makes the smoke also exercise
/// the per-`SurfaceKey` `declare`/`destroy`/`present` paths by
/// declaring one `CompositorSurface` covering the top-left quarter
/// of the viewport at 50% opacity.
#[cfg(feature = "macos-present")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacosCALayerPresentSmokeConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub declare_subsurface: bool,
}

#[cfg(feature = "macos-present")]
impl Default for MacosCALayerPresentSmokeConfig {
    fn default() -> Self {
        Self {
            title: "pelt — macos-calayer present smoke".into(),
            width: 800,
            height: 600,
            // ~1s at 60Hz; long enough to actually see the window
            // before auto-exit. Bump higher on ProMotion (120Hz)
            // displays where 60 frames is ~0.5s. Path validation is
            // satisfied at frames=1 — this default trades smoke
            // speed for visual confirmability.
            frames: 60,
            declare_subsurface: false,
        }
    }
}

/// Headed presentation smoke test on macOS — opens a winit window,
/// extracts the NSView's `CALayer`, constructs a
/// [`paint::MacosCALayerBackend`] over netrender's wgpu Metal device,
/// and renders + presents `frames` frames of a synthetic red scene
/// through a `CAMetalLayer` attached to the view.
///
/// On non-Apple targets this returns
/// `Err("macos-present requires target_vendor = \"apple\"")` without
/// touching winit so the feature still type-checks portably.
///
/// Runtime path on macOS:
///   1. winit `EventLoop::new` + window create
///   2. `RawWindowHandle::AppKit` → NSView pointer
///   3. NSView `setWantsLayer:YES` → `[ns_view layer]` for the
///      embedder root `CALayer`
///   4. `wgpu::Instance` forced to `Backends::METAL`
///   5. `paint::HostWgpuContext::new(device, queue)`
///   6. `paint::MacosCALayerBackend::new(&host, layer_ptr)` builds
///      the `CAMetalLayer` sublayer + per-backend `MTLCommandQueue`
///   7. `paint::ServoCompositor::new(host, backend)`
///   8. Per `RedrawRequested`:
///        a. build a `netrender::Scene` directly (a single coloured
///           rect for now)
///        b. `renderer.render_with_compositor(scene, format, &mut
///           servo_compositor, base)` — netrender renders the master,
///           the backend blits it into the drawable + presents
///   9. Loop exits after `config.frames` redraws or window close.
#[cfg(feature = "macos-present")]
pub fn run_macos_calayer_present_smoke(
    config: MacosCALayerPresentSmokeConfig,
) -> Result<MacosCALayerPresentSmokeOutcome, String> {
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = config;
        return Err("macos-present requires target_vendor = \"apple\"".into());
    }

    #[cfg(target_vendor = "apple")]
    {
        let event_loop = winit::event_loop::EventLoop::new()
            .map_err(|error| format!("could not create event loop: {error}"))?;
        let mut app = MacosCALayerPresentApp::new(config);
        event_loop
            .run_app(&mut app)
            .map_err(|error| format!("present-smoke event loop failed: {error}"))?;
        if let Some(error) = app.error {
            return Err(error);
        }
        app.outcome
            .ok_or_else(|| "present smoke ended without an outcome".into())
    }
}

#[cfg(all(feature = "macos-present", target_vendor = "apple"))]
struct MacosCALayerPresentApp {
    config: MacosCALayerPresentSmokeConfig,
    window: Option<winit::window::Window>,
    window_id: Option<winit::window::WindowId>,
    state: Option<MacosPresentState>,
    frames_presented: u32,
    outcome: Option<MacosCALayerPresentSmokeOutcome>,
    error: Option<String>,
}

#[cfg(all(feature = "macos-present", target_vendor = "apple"))]
struct MacosPresentState {
    renderer: netrender::Renderer,
    /// `Box<dyn paint::PaintCompositor>` rather than the concrete
    /// `ServoCompositor<MacosCALayerBackend>` because we construct
    /// it via `default_compositor_for_window`, which returns the
    /// erased shape so the same factory call works on every
    /// platform.
    compositor: Box<dyn paint::PaintCompositor>,
}

/// Build a Metal-forced [`netrender::WgpuHandles`].
///
/// `netrender::boot()` lets wgpu pick a backend; on macOS hosts that
/// is reliably Metal, but [`paint::MacosCALayerBackend`] requires
/// Metal explicitly. Force the choice here for symmetry with
/// [`build_dx12_handles`].
#[cfg(all(feature = "macos-present", target_vendor = "apple"))]
fn build_metal_handles() -> Result<netrender::WgpuHandles, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .map_err(|err| format!("request_adapter: {err}"))?;
    // No optional features: vello's compute pipeline binds its
    // storage texture as `Rgba8Unorm`-format-expected (vello 0.8's
    // bind-group descriptor hardcodes the storage texture format),
    // so the master must be `Rgba8Unorm` regardless of what the OS
    // compositor wants downstream. See the BGRA-vs-RGBA note in
    // [compositor_calayer.rs](../../components/paint/compositor_calayer.rs)
    // for the macOS path's accepted contract violation.
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pelt macos-present device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits {
            max_inter_stage_shader_variables: 28,
            ..Default::default()
        },
        ..Default::default()
    }))
    .map_err(|err| format!("request_device: {err}"))?;
    Ok(netrender::WgpuHandles {
        instance,
        adapter,
        device,
        queue,
    })
}

#[cfg(all(feature = "macos-present", target_vendor = "apple"))]
impl MacosCALayerPresentApp {
    fn new(config: MacosCALayerPresentSmokeConfig) -> Self {
        Self {
            config,
            window: None,
            window_id: None,
            state: None,
            frames_presented: 0,
            outcome: None,
            error: None,
        }
    }

    fn fail(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, message: String) {
        self.error = Some(message);
        event_loop.exit();
    }
}

#[cfg(all(feature = "macos-present", target_vendor = "apple"))]
impl winit::application::ApplicationHandler for MacosCALayerPresentApp {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        // 1. winit window
        let attributes = winit::window::WindowAttributes::default()
            .with_title(self.config.title.clone())
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.config.width as f64,
                self.config.height as f64,
            ));
        let window = match event_loop.create_window(attributes) {
            Ok(w) => w,
            Err(err) => return self.fail(event_loop, format!("create_window: {err}")),
        };
        self.window_id = Some(window.id());

        // 2. Display + window handles for the factory.
        use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
        let display_handle = match window.display_handle() {
            Ok(h) => h.as_raw(),
            Err(err) => return self.fail(event_loop, format!("display_handle: {err}")),
        };
        let window_handle = match window.window_handle() {
            Ok(h) => h.as_raw(),
            Err(err) => return self.fail(event_loop, format!("window_handle: {err}")),
        };

        // 3. wgpu instance/adapter/device/queue, forced to Metal.
        let handles = match build_metal_handles() {
            Ok(h) => h,
            Err(err) => return self.fail(event_loop, format!("wgpu Metal boot: {err}")),
        };
        let device = handles.device.clone();
        let queue = handles.queue.clone();
        let renderer = match netrender::create_netrender_instance(
            handles,
            netrender::NetrenderOptions {
                tile_cache_size: Some(64),
                enable_vello: true,
                ..Default::default()
            },
        ) {
            Ok(r) => r,
            Err(err) => {
                return self.fail(event_loop, format!("create_netrender_instance: {err:?}"));
            },
        };

        // 4. Host context + factory-built compositor. The factory
        // owns the per-platform AppKit/UiKit -> CALayer extraction
        // (see `paint::compositor_factory`); pelt-desktop just hands
        // it the raw-window-handle pieces and gets back a
        // `Box<dyn PaintCompositor>` already wrapping the right
        // backend for the host OS.
        let host = paint::HostWgpuContext::new(device, queue);
        let compositor =
            match paint::default_compositor_for_window(host, display_handle, window_handle) {
                Ok(c) => c,
                Err(err) => {
                    return self.fail(event_loop, format!("default_compositor_for_window: {err}"));
                },
            };

        self.state = Some(MacosPresentState {
            renderer,
            compositor,
        });

        window.request_redraw();
        self.window = Some(window);
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        if Some(window_id) != self.window_id {
            return;
        }

        match event {
            winit::event::WindowEvent::CloseRequested => event_loop.exit(),
            winit::event::WindowEvent::Resized(_) => {
                // Force a redraw so the master + per-surface
                // CALayer pick up the new `inner_size` immediately.
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            },
            winit::event::WindowEvent::RedrawRequested => {
                let Some(state) = self.state.as_mut() else {
                    return;
                };
                let Some(window) = self.window.as_ref() else {
                    return;
                };

                // winit on macOS sometimes fires one more
                // RedrawRequested after `event_loop.exit()` is
                // called, before the loop actually unwinds. Without
                // this guard, `frames_presented` ticks past
                // `config.frames` and the outcome reports e.g.
                // `frames=61` for a configured 60. Cheap to ignore.
                // `config.frames == 0` is the "run until window
                // close" sentinel — never auto-exit.
                if self.config.frames > 0 && self.frames_presented >= self.config.frames {
                    return;
                }

                // 8a. synthetic scene at backing-pixel resolution
                // of the *current* window. Pulling from
                // `window.inner_size()` (already in physical/backing
                // pixels per winit's docs) instead of fixing
                // `config.width × scale_factor` lets the master
                // texture grow/shrink with window resize, which in
                // turn changes the per-`SurfaceKey` source rect, so
                // `ServoCompositor` calls `backend.declare` again
                // with the new dims and the per-surface CALayer
                // gets a fresh frame.
                let inner = window.inner_size();
                let backing_w = inner.width.max(1);
                let backing_h = inner.height.max(1);
                let mut scene = netrender::Scene::new(backing_w, backing_h);
                // Background red over the full viewport.
                scene.push_rect(
                    0.0,
                    0.0,
                    backing_w as f32,
                    backing_h as f32,
                    [1.0, 0.0, 0.0, 1.0],
                );
                if self.config.declare_subsurface {
                    // Top-left quarter as a declared compositor
                    // surface. The compositor blits master[rect]
                    // into an IOSurface-backed destination texture;
                    // the OS composites that as a separate CALayer
                    // sublayer. To visually distinguish the
                    // per-surface CALayer from the master CALayer
                    // beneath it, paint a green stripe inside the
                    // surface bounds + drop opacity to 0.5 so the
                    // result is a yellow-ish stripe (master red +
                    // per-surface green at 50% blend) where the
                    // per-surface CALayer composites.
                    let half_w = backing_w as f32 / 2.0;
                    let half_h = backing_h as f32 / 2.0;
                    scene.push_rect(0.0, 0.0, half_w, half_h, [0.0, 1.0, 0.0, 1.0]);
                    let mut surface = netrender::CompositorSurface::new(
                        netrender::SurfaceKey(1),
                        [0.0, 0.0, half_w, half_h],
                    );
                    surface.opacity = 0.5;
                    scene.declare_compositor_surface(surface);
                }

                // 8b. render through the CAMetalLayer.
                // `&mut *state.compositor` is `&mut dyn
                // PaintCompositor`; rustc 1.86+ trait upcasting (via
                // `PaintCompositor: Compositor`) lets it coerce to
                // the `&mut dyn Compositor` `render_with_compositor`
                // wants. Same pattern Paint::render uses internally
                // (see `components/paint/netrender_painter.rs:468`).
                let pc: &mut dyn paint::PaintCompositor = &mut *state.compositor;
                state.renderer.render_with_compositor(
                    &scene,
                    wgpu::TextureFormat::Rgba8Unorm,
                    pc,
                    netrender::peniko::Color::new([0.0, 0.0, 0.0, 0.0]),
                );

                self.frames_presented += 1;

                // `frames == 0` means run until the user closes the
                // window — used by the surfaces smoke so it sticks
                // around for visual inspection / screenshots.
                if self.config.frames > 0 && self.frames_presented >= self.config.frames {
                    event_loop.exit();
                } else if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            },
            _ => {},
        }
    }

    fn exiting(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.outcome.is_some() {
            return;
        }
        self.outcome = Some(MacosCALayerPresentSmokeOutcome {
            width: self.config.width,
            height: self.config.height,
            frames_presented: self.frames_presented,
            created_window: self.window_id.is_some(),
        });
    }
}
