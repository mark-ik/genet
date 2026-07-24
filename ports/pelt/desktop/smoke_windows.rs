/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

/// Outcome of [`run_windows_dxgi_present_smoke`]. Captures observable
/// state from the headed Windows-DXGI present path so callers can
/// assert on it without actually inspecting the on-screen pixels.
#[cfg(feature = "windows-present")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsDxgiPresentSmokeOutcome {
    pub width: u32,
    pub height: u32,
    pub frames_presented: u32,
    pub created_window: bool,
    pub declared_subsurface: bool,
}

/// Configuration for [`run_windows_dxgi_present_smoke`]. `frames` is
/// the number of redraw-presents to fire before exiting the loop;
/// `1` is enough to validate the construction + present path.
/// `frames == 0` keeps the window open until close.
#[cfg(feature = "windows-present")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsDxgiPresentSmokeConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub declare_subsurface: bool,
}

#[cfg(feature = "windows-present")]
impl Default for WindowsDxgiPresentSmokeConfig {
    fn default() -> Self {
        Self {
            title: "pelt — windows-dxgi present smoke".into(),
            width: 800,
            height: 600,
            frames: 1,
            declare_subsurface: false,
        }
    }
}

/// Headed presentation smoke test — opens a winit window, extracts
/// the HWND, constructs a [`paint::WindowsDxgiBackend`] over
/// netrender's wgpu device, and renders + presents `frames` frames
/// of a synthetic red-on-transparent scene through the DCOMP
/// composition swapchain. With `declare_subsurface`, the scene also
/// declares a top-left compositor surface so the per-`SurfaceKey`
/// DCOMP child visual path is exercised.
///
/// On non-Windows targets this returns
/// `Err("windows-present requires target_os = \"windows\"")` without
/// touching winit so the feature still type-checks portably.
///
/// Runtime path on Windows:
///   1. winit `EventLoop::new` + window create
///   2. raw-window-handle pulls the `HWND` from the window
///   3. `netrender::boot()` returns wgpu instance/adapter/device/queue
///   4. `paint::HostWgpuContext::new(device, queue)` bundles those for
///      the backend
///   5. `paint::WindowsDxgiBackend::new(&host, hwnd)` builds the DCOMP
///      visual tree + composition swapchain
///   6. `paint::ServoCompositor::new(host, backend)` wraps the
///      backend into the netrender `Compositor` shape
///   7. Per RedrawRequested:
///        a. translate-equivalent: build a `netrender::Scene` directly
///           (a single coloured rect for now)
///        b. `renderer.render_with_compositor(scene, format, &mut
///           servo_compositor, base)` — netrender renders the master,
///           the backend copies it into the swapchain backbuffer +
///           presents + commits DCOMP
///   8. Loop exits after `config.frames` redraws or window close.
#[cfg(feature = "windows-present")]
pub fn run_windows_dxgi_present_smoke(
    config: WindowsDxgiPresentSmokeConfig,
) -> Result<WindowsDxgiPresentSmokeOutcome, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = config;
        return Err("windows-present requires target_os = \"windows\"".into());
    }

    #[cfg(target_os = "windows")]
    {
        let event_loop = winit::event_loop::EventLoop::new()
            .map_err(|error| format!("could not create event loop: {error}"))?;
        let mut app = WindowsDxgiPresentApp::new(config);
        event_loop
            .run_app(&mut app)
            .map_err(|error| format!("present-smoke event loop failed: {error}"))?;
        // App-level errors take precedence over the outcome — even if
        // exiting() set an outcome, surfacing the underlying failure
        // is what the caller wants. (Previously the outcome's
        // `created_window=true` was silently masking a real
        // construction failure later in resumed().)
        if let Some(error) = app.error {
            return Err(error);
        }
        app.outcome
            .ok_or_else(|| "present smoke ended without an outcome".into())
    }
}

#[cfg(all(feature = "windows-present", target_os = "windows"))]
struct WindowsDxgiPresentApp {
    config: WindowsDxgiPresentSmokeConfig,
    window: Option<winit::window::Window>,
    window_id: Option<winit::window::WindowId>,
    state: Option<PresentState>,
    frames_presented: u32,
    outcome: Option<WindowsDxgiPresentSmokeOutcome>,
    error: Option<String>,
}

#[cfg(all(feature = "windows-present", target_os = "windows"))]
struct PresentState {
    renderer: netrender::Renderer,
    compositor: paint::ServoCompositor<paint::WindowsDxgiBackend>,
}

/// Build a D3D12-forced [`netrender::WgpuHandles`].
///
/// `netrender::boot()` uses `wgpu::Instance::default()` which lets
/// wgpu pick a backend; on Windows machines with both DX12 and
/// Vulkan drivers wgpu often picks Vulkan, but
/// [`paint::WindowsDxgiBackend`] requires Dx12. Force the choice
/// here by constructing the wgpu pieces explicitly.
#[cfg(all(feature = "windows-present", target_os = "windows"))]
fn build_dx12_handles() -> Result<netrender::WgpuHandles, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
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
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pelt windows-present device"),
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

#[cfg(all(feature = "windows-present", target_os = "windows"))]
impl WindowsDxgiPresentApp {
    fn new(config: WindowsDxgiPresentSmokeConfig) -> Self {
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

#[cfg(all(feature = "windows-present", target_os = "windows"))]
impl winit::application::ApplicationHandler for WindowsDxgiPresentApp {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        // 1. winit window
        let attributes = winit::window::WindowAttributes::default()
            .with_title(self.config.title.clone())
            .with_inner_size(winit::dpi::PhysicalSize::new(
                self.config.width,
                self.config.height,
            ));
        let window = match event_loop.create_window(attributes) {
            Ok(w) => w,
            Err(err) => return self.fail(event_loop, format!("create_window: {err}")),
        };
        self.window_id = Some(window.id());

        // 2. HWND from raw-window-handle
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = match window.window_handle() {
            Ok(h) => h.as_raw(),
            Err(err) => return self.fail(event_loop, format!("window_handle: {err}")),
        };
        let RawWindowHandle::Win32(win32) = handle else {
            return self.fail(
                event_loop,
                format!("expected Win32 RawWindowHandle, got {handle:?}"),
            );
        };
        let hwnd = windows::Win32::Foundation::HWND(win32.hwnd.get() as *mut _);

        // 3. wgpu instance/adapter/device/queue, **forced to D3D12**.
        // We can't use `netrender::boot()` here because it lets wgpu
        // pick the backend (frequently Vulkan on Windows machines
        // with Vulkan drivers), but `WindowsDxgiBackend` requires
        // D3D12. Construct the handles manually, then hand them to
        // `create_netrender_instance` directly.
        let handles = match build_dx12_handles() {
            Ok(h) => h,
            Err(err) => return self.fail(event_loop, format!("wgpu D3D12 boot: {err}")),
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

        // 4-5. host context + WindowsDxgiBackend
        let host = paint::HostWgpuContext::new(device, queue);
        let backend = match paint::WindowsDxgiBackend::new(&host, hwnd) {
            Ok(b) => b,
            Err(err) => return self.fail(event_loop, format!("WindowsDxgiBackend::new: {err}")),
        };

        // 6. ServoCompositor over the backend
        let compositor = paint::ServoCompositor::new(host, backend);

        self.state = Some(PresentState {
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

                if self.config.frames > 0 && self.frames_presented >= self.config.frames {
                    return;
                }

                // 7a. synthetic scene at current backing-pixel
                // resolution. The basic smoke is a red viewport; the
                // surfaces smoke adds a green top-left compositor
                // surface at 50% opacity so the child DCOMP visual is
                // visibly composited above the master.
                let backing_w = self.config.width.max(1);
                let backing_h = self.config.height.max(1);
                let mut scene = netrender::Scene::new(backing_w, backing_h);
                scene.push_rect(
                    0.0,
                    0.0,
                    backing_w as f32,
                    backing_h as f32,
                    [1.0, 0.0, 0.0, 1.0],
                );
                if self.config.declare_subsurface {
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

                // 7b. render through the DCOMP composition swapchain
                state.renderer.render_with_compositor(
                    &scene,
                    wgpu::TextureFormat::Rgba8Unorm,
                    &mut state.compositor,
                    netrender::peniko::Color::new([0.0, 0.0, 0.0, 0.0]),
                );

                self.frames_presented += 1;

                if self.config.frames > 0 && self.frames_presented >= self.config.frames {
                    event_loop.exit();
                } else if self.config.frames > 0 {
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
        self.outcome = Some(WindowsDxgiPresentSmokeOutcome {
            width: self.config.width,
            height: self.config.height,
            frames_presented: self.frames_presented,
            created_window: self.window_id.is_some(),
            declared_subsurface: self.config.declare_subsurface,
        });
    }
}
