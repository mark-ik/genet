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

    // Real impl lands in 8.2.
    pub struct WaylandPresentApp {
        pub config: WaylandPresentSmokeConfig,
        pub outcome: Option<WaylandPresentSmokeOutcome>,
        pub error: Option<String>,
    }

    impl WaylandPresentApp {
        pub fn new(config: WaylandPresentSmokeConfig) -> Self {
            Self {
                config,
                outcome: None,
                error: None,
            }
        }
    }

    impl winit::application::ApplicationHandler for WaylandPresentApp {
        fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
            // Placeholder — Task 8.2.
            let _ = event_loop;
        }
        fn window_event(
            &mut self,
            _: &winit::event_loop::ActiveEventLoop,
            _: winit::window::WindowId,
            _: winit::event::WindowEvent,
        ) {
        }
    }
}
