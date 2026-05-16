/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::{DesktopHostProfile, WindowingMode};
use pelt_core::EngineProfile;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

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
