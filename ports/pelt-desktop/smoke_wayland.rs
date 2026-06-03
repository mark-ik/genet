/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Headed presentation smoke on Linux Wayland.
//!
//! Mirrors smoke_macos / smoke_windows in shape: winit window →
//! raw handles → forced wgpu Vulkan backend → netrender Renderer
//! → default_compositor_for_window → render_with_compositor per
//! frame, with optional CompositorSurface declared at 50% opacity
//! for the visual receipt.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WaylandPresentSmokeOutcome {
    pub width: u32,
    pub height: u32,
    pub frames_presented: u32,
    pub created_window: bool,
    pub declared_subsurface: bool,
}

#[cfg(feature = "linux-present")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WaylandPresentSmokeConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub declare_subsurface: bool,
}

#[cfg(feature = "linux-present")]
impl Default for WaylandPresentSmokeConfig {
    fn default() -> Self {
        Self {
            title: "pelt — wayland-subsurface present smoke".into(),
            width: 800,
            height: 600,
            // ~1s at 60Hz; long enough to confirm the basic smoke is
            // doing real work before auto-exit.
            frames: 60,
            declare_subsurface: false,
        }
    }
}

#[cfg(feature = "linux-present")]
pub fn run_wayland_subsurface_present_smoke(
    config: WaylandPresentSmokeConfig,
) -> Result<WaylandPresentSmokeOutcome, String> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        return Err("linux-present requires target_os = \"linux\"".into());
    }

    #[cfg(target_os = "linux")]
    {
        let event_loop = winit::event_loop::EventLoop::new()
            .map_err(|error| format!("could not create event loop: {error}"))?;
        let mut app = linux_impl::WaylandPresentApp::new(config);
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

#[cfg(all(feature = "linux-present", target_os = "linux"))]
mod linux_impl {
    use super::*;

    pub struct WaylandPresentApp {
        pub config: WaylandPresentSmokeConfig,
        window: Option<winit::window::Window>,
        window_id: Option<winit::window::WindowId>,
        state: Option<WaylandPresentState>,
        frames_presented: u32,
        pub outcome: Option<WaylandPresentSmokeOutcome>,
        pub error: Option<String>,
    }

    struct WaylandPresentState {
        renderer: netrender::Renderer,
        compositor: Box<dyn paint::PaintCompositor>,
    }

    impl WaylandPresentApp {
        pub fn new(config: WaylandPresentSmokeConfig) -> Self {
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

    fn build_vulkan_handles() -> Result<netrender::WgpuHandles, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
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
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("pelt wayland-present device"),
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

    impl winit::application::ApplicationHandler for WaylandPresentApp {
        fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }

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

            use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
            let display_handle = match window.display_handle() {
                Ok(h) => h.as_raw(),
                Err(err) => return self.fail(event_loop, format!("display_handle: {err}")),
            };
            let window_handle = match window.window_handle() {
                Ok(h) => h.as_raw(),
                Err(err) => return self.fail(event_loop, format!("window_handle: {err}")),
            };

            let handles = match build_vulkan_handles() {
                Ok(h) => h,
                Err(err) => return self.fail(event_loop, format!("wgpu Vulkan boot: {err}")),
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
                    return self
                        .fail(event_loop, format!("create_netrender_instance: {err:?}"));
                },
            };

            let host = paint::HostWgpuContext::new(device, queue);
            let compositor =
                match paint::default_compositor_for_window(host, display_handle, window_handle) {
                    Ok(c) => c,
                    Err(err) => {
                        return self
                            .fail(event_loop, format!("default_compositor_for_window: {err}"));
                    },
                };

            self.state = Some(WaylandPresentState {
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
                        // Top-left quarter green; per-surface composes at 50% opacity.
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

                    let pc: &mut dyn paint::PaintCompositor = &mut *state.compositor;
                    state.renderer.render_with_compositor(
                        &scene,
                        wgpu::TextureFormat::Rgba8Unorm,
                        pc,
                        netrender::peniko::Color::new([0.0, 0.0, 0.0, 0.0]),
                    );

                    self.frames_presented += 1;

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
            self.outcome = Some(WaylandPresentSmokeOutcome {
                width: self.config.width,
                height: self.config.height,
                frames_presented: self.frames_presented,
                created_window: self.window_id.is_some(),
                declared_subsurface: self.config.declare_subsurface,
            });
        }
    }
}
