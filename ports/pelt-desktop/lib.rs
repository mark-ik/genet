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
