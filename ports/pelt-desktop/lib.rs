/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Desktop host contracts for Pelt.
//!
//! This crate is the destination for winit windows, input translation, native
//! dialogs, filesystem integration, and platform event-loop glue. It stays
//! above `pelt-core` and below the UI chrome crate.

use pelt_core::EngineProfile;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowingMode {
    Headed,
    Headless,
}

impl WindowingMode {
    pub fn from_headless_flag(headless: bool) -> Self {
        match headless {
            true => Self::Headless,
            false => Self::Headed,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopHostProfile {
    pub engine: EngineProfile,
    pub windowing: WindowingMode,
}

impl DesktopHostProfile {
    pub fn new(engine: EngineProfile, windowing: WindowingMode) -> Self {
        Self { engine, windowing }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticViewerConfig {
    pub profile: DesktopHostProfile,
    pub url: String,
    pub title: String,
    pub exit_after_first_redraw: bool,
}

impl StaticViewerConfig {
    pub fn new(engine: EngineProfile, windowing: WindowingMode, url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            profile: DesktopHostProfile::new(engine, windowing),
            title: format!("Pelt Viewer - {url}"),
            url,
            exit_after_first_redraw: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticViewerOutcome {
    pub url: String,
    pub created_window: bool,
    pub redraws: u32,
}

#[cfg(feature = "netrender")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetrenderSmokeOutcome {
    pub width: u32,
    pub height: u32,
    pub painted_pixels: usize,
}

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
}

#[cfg(feature = "netrender")]
pub fn run_netrender_smoke() -> Result<NetrenderSmokeOutcome, String> {
    const DIM: u32 = 64;

    let handles =
        netrender::boot().map_err(|error| format!("netrender wgpu boot failed: {error}"))?;
    let device = handles.device.clone();
    let renderer = netrender::create_netrender_instance(
        handles,
        netrender::NetrenderOptions {
            tile_cache_size: Some(32),
            enable_vello: true,
            ..Default::default()
        },
    )
    .map_err(|error| format!("netrender renderer init failed: {error:?}"))?;

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pelt netrender smoke target"),
        size: wgpu::Extent3d {
            width: DIM,
            height: DIM,
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
    let view = target.create_view(&wgpu::TextureViewDescriptor {
        label: Some("pelt netrender smoke view"),
        format: Some(wgpu::TextureFormat::Rgba8Unorm),
        ..Default::default()
    });

    let mut scene = netrender::Scene::new(DIM, DIM);
    scene.push_rect(0.0, 0.0, DIM as f32, DIM as f32, [1.0, 0.0, 0.0, 1.0]);
    renderer.render_vello(&scene, &view, netrender::ColorLoad::default());

    let bytes = renderer.wgpu_device.read_rgba8_texture(&target, DIM, DIM);
    let painted_pixels = bytes
        .chunks_exact(4)
        .filter(|rgba| rgba[0] != 0 || rgba[1] != 0 || rgba[2] != 0 || rgba[3] != 0)
        .count();

    Ok(NetrenderSmokeOutcome {
        width: DIM,
        height: DIM,
        painted_pixels,
    })
}

/// Configuration for [`run_windows_dxgi_present_smoke`]. `frames` is
/// the number of redraw-presents to fire before exiting the loop;
/// `1` is enough to validate the construction + present path.
#[cfg(feature = "windows-present")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsDxgiPresentSmokeConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
}

#[cfg(feature = "windows-present")]
impl Default for WindowsDxgiPresentSmokeConfig {
    fn default() -> Self {
        Self {
            title: "pelt — windows-dxgi present smoke".into(),
            width: 800,
            height: 600,
            frames: 1,
        }
    }
}

/// Headed presentation smoke test — opens a winit window, extracts
/// the HWND, constructs a [`paint::WindowsDxgiBackend`] over
/// netrender's wgpu device, and renders + presents `frames` frames
/// of a synthetic red-on-transparent scene through the DCOMP
/// composition swapchain.
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
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.config.width as f64,
                self.config.height as f64,
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
                return self.fail(event_loop, format!("create_netrender_instance: {err:?}"))
            },
        };

        // 4-5. host context + WindowsDxgiBackend
        let host = paint::HostWgpuContext::new(device, queue);
        let backend = match paint::WindowsDxgiBackend::new(&host, hwnd) {
            Ok(b) => b,
            Err(err) => {
                return self.fail(event_loop, format!("WindowsDxgiBackend::new: {err}"))
            },
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
            winit::event::WindowEvent::RedrawRequested => {
                let Some(state) = self.state.as_mut() else {
                    return;
                };

                // 7a. synthetic scene — single red rect filling the
                // viewport so we can visually confirm the present.
                let mut scene =
                    netrender::Scene::new(self.config.width, self.config.height);
                scene.push_rect(
                    0.0,
                    0.0,
                    self.config.width as f32,
                    self.config.height as f32,
                    [1.0, 0.0, 0.0, 1.0],
                );

                // 7b. render through the DCOMP composition swapchain
                state.renderer.render_with_compositor(
                    &scene,
                    wgpu::TextureFormat::Rgba8Unorm,
                    &mut state.compositor,
                    netrender::peniko::Color::new([0.0, 0.0, 0.0, 0.0]),
                );

                self.frames_presented += 1;

                if self.frames_presented >= self.config.frames {
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
        self.outcome = Some(WindowsDxgiPresentSmokeOutcome {
            width: self.config.width,
            height: self.config.height,
            frames_presented: self.frames_presented,
            created_window: self.window_id.is_some(),
        });
    }
}

/// Outcome of [`run_macos_calayer_present_smoke`]. Mirrors the
/// Windows-DXGI version — observable state from the macOS CAMetalLayer
/// present path so callers can assert on it without inspecting on-
/// screen pixels.
#[cfg(feature = "macos-present")]
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
#[cfg(feature = "macos-present")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacosCALayerPresentSmokeConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
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
    compositor: paint::ServoCompositor<paint::MacosCALayerBackend>,
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

        // 2. NSView from raw-window-handle's AppKit variant
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = match window.window_handle() {
            Ok(h) => h.as_raw(),
            Err(err) => return self.fail(event_loop, format!("window_handle: {err}")),
        };
        let RawWindowHandle::AppKit(appkit) = handle else {
            return self.fail(
                event_loop,
                format!("expected AppKit RawWindowHandle, got {handle:?}"),
            );
        };

        // 3. setWantsLayer + extract the NSView's backing CALayer.
        // winit's NSView is layer-backed by default on Big Sur+, but
        // setWantsLayer makes the contract explicit.
        use objc2::rc::Retained;
        use objc2_app_kit::NSView;
        let ns_view: Retained<NSView> = match unsafe {
            Retained::retain(appkit.ns_view.as_ptr() as *mut NSView)
        } {
            Some(v) => v,
            None => return self.fail(event_loop, "failed to retain NSView".into()),
        };
        ns_view.setWantsLayer(true);
        let layer = match ns_view.layer() {
            Some(l) => l,
            None => {
                return self.fail(
                    event_loop,
                    "NSView.layer returned nil after setWantsLayer; \
                     view is not layer-backed"
                        .into(),
                );
            },
        };
        let layer_ptr = Retained::as_ptr(&layer) as *mut std::ffi::c_void;

        // 4. wgpu instance/adapter/device/queue, forced to Metal.
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

        // 5-6. host context + MacosCALayerBackend
        let host = paint::HostWgpuContext::new(device, queue);
        // SAFETY: layer_ptr was obtained from a `Retained<CALayer>`
        // we hold on the stack via `ns_view.layer()`. The backend
        // retains its own reference internally, so the embedder's
        // reference (the `Retained<CALayer>` `layer` we drop at
        // function end) is independent of the backend's copy.
        let backend = match unsafe { paint::MacosCALayerBackend::new(&host, layer_ptr) } {
            Ok(b) => b,
            Err(err) => {
                return self.fail(event_loop, format!("MacosCALayerBackend::new: {err}"));
            },
        };

        // 7. ServoCompositor over the backend
        let compositor = paint::ServoCompositor::new(host, backend);

        self.state = Some(MacosPresentState {
            renderer,
            compositor,
        });

        window.request_redraw();
        self.window = Some(window);
        // Drop our local `layer` reference here — backend retained
        // its own copy in `MacosCALayerBackend::new`.
        drop(layer);
        drop(ns_view);
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
            winit::event::WindowEvent::RedrawRequested => {
                let Some(state) = self.state.as_mut() else {
                    return;
                };

                // 8a. synthetic scene — single red rect filling the
                // viewport so we can visually confirm the present.
                let mut scene =
                    netrender::Scene::new(self.config.width, self.config.height);
                scene.push_rect(
                    0.0,
                    0.0,
                    self.config.width as f32,
                    self.config.height as f32,
                    [1.0, 0.0, 0.0, 1.0],
                );

                // 8b. render through the CAMetalLayer
                state.renderer.render_with_compositor(
                    &scene,
                    wgpu::TextureFormat::Rgba8Unorm,
                    &mut state.compositor,
                    netrender::peniko::Color::new([0.0, 0.0, 0.0, 0.0]),
                );

                self.frames_presented += 1;

                if self.frames_presented >= self.config.frames {
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

pub fn run_static_viewer(config: StaticViewerConfig) -> Result<StaticViewerOutcome, String> {
    match config.profile.windowing {
        WindowingMode::Headless => Ok(StaticViewerOutcome {
            url: config.url,
            created_window: false,
            redraws: 0,
        }),
        WindowingMode::Headed => {
            let event_loop = EventLoop::new()
                .map_err(|error| format!("could not create event loop: {error}"))?;
            let mut app = StaticViewerApp::new(config);
            event_loop
                .run_app(&mut app)
                .map_err(|error| format!("viewer event loop failed: {error}"))?;
            Ok(app.outcome())
        },
    }
}

struct StaticViewerApp {
    config: StaticViewerConfig,
    window: Option<Window>,
    window_id: Option<WindowId>,
    redraws: u32,
}

impl StaticViewerApp {
    fn new(config: StaticViewerConfig) -> Self {
        Self {
            config,
            window: None,
            window_id: None,
            redraws: 0,
        }
    }

    fn outcome(&self) -> StaticViewerOutcome {
        StaticViewerOutcome {
            url: self.config.url.clone(),
            created_window: self.window_id.is_some(),
            redraws: self.redraws,
        }
    }
}

impl ApplicationHandler for StaticViewerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attributes = WindowAttributes::default()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(800.0, 600.0));
        let window = event_loop
            .create_window(attributes)
            .expect("failed to create Pelt viewer window");
        self.window_id = Some(window.id());
        window.request_redraw();
        self.window = Some(window);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if Some(window_id) != self.window_id {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                self.redraws += 1;
                if self.config.exit_after_first_redraw {
                    event_loop.exit();
                }
            },
            _ => {},
        }
    }
}
