/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The on-screen static document viewer (`pelt --engine static <url>`).
//!
//! A thin winit shell over a [`LoadedDocument`](crate::document::LoadedDocument)
//! presented through the shared [`SurfaceHost`](serval_winit_host::SurfaceHost):
//! the second instance of the orrery-host pattern (a window-agnostic content lib
//! plus a thin shell that maps winit events onto the content's semantic input and
//! rasterizes + composites its scene per frame). The document is the content;
//! wheel scrolling is its only interaction in V1, fed through the shared
//! default-action helper into the document's viewport.

use crate::{DesktopHostProfile, WindowingMode};
use pelt_core::EngineProfile;

/// Configuration for one static-viewer run.
pub struct StaticViewerConfig {
    pub profile: DesktopHostProfile,
    pub url: String,
    pub title: String,
    /// Exit after the first presented frame (a one-shot render smoke). Interactive
    /// runs leave it `false` and stay open until the window is closed.
    pub exit_after_first_redraw: bool,
}

impl StaticViewerConfig {
    pub fn new(engine: EngineProfile, windowing: WindowingMode, url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            profile: DesktopHostProfile::new(engine, windowing),
            title: format!("Pelt — {url}"),
            url,
            exit_after_first_redraw: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticViewerOutcome {
    pub url: String,
    pub created_window: bool,
    pub redraws: u32,
}

/// Run the static viewer for `config`. Headless returns immediately with no window
/// (the CI smoke shape); headed opens a window, presents the document, and scrolls
/// it on the wheel until the window is closed.
pub fn run_static_viewer(config: StaticViewerConfig) -> Result<StaticViewerOutcome, String> {
    match config.profile.windowing {
        WindowingMode::Headless => Ok(StaticViewerOutcome {
            url: config.url,
            created_window: false,
            redraws: 0,
        }),
        WindowingMode::Headed => run_headed(config),
    }
}

/// Without the `viewer` feature there is no render / present stack, so a headed run
/// cannot proceed. (The light contracts + smoke build excludes it.)
#[cfg(not(feature = "viewer"))]
fn run_headed(_config: StaticViewerConfig) -> Result<StaticViewerOutcome, String> {
    Err("the static viewer needs the `viewer` feature (pelt's default build enables it)".to_string())
}

#[cfg(feature = "viewer")]
fn run_headed(config: StaticViewerConfig) -> Result<StaticViewerOutcome, String> {
    use winit::event_loop::EventLoop;

    // Load the document before opening a window, so a bad URL fails fast (and the
    // caller reports the error) rather than flashing an empty window.
    let doc = crate::document::LoadedDocument::load(&crate::document::LocalFetcher, &config.url)?;
    let event_loop =
        EventLoop::new().map_err(|error| format!("could not create event loop: {error}"))?;
    let mut app = windowed::ViewerApp::new(config, doc);
    event_loop
        .run_app(&mut app)
        .map_err(|error| format!("viewer event loop failed: {error}"))?;
    Ok(app.outcome())
}

#[cfg(feature = "viewer")]
mod windowed {
    use std::sync::Arc;

    use netrender::external_texture::ExternalTexturePlacement;
    use netrender::{ColorLoad, NetrenderOptions};
    use serval_winit_host::{wheel_delta_from_winit, SurfaceHost};
    use winit::application::ApplicationHandler;
    use winit::dpi::PhysicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::ActiveEventLoop;
    use winit::window::{Window, WindowId};

    use super::{StaticViewerConfig, StaticViewerOutcome};
    use crate::document::LoadedDocument;

    /// The static viewer application: the [`LoadedDocument`] content plus the window
    /// + shared present stack that drives it.
    pub(super) struct ViewerApp {
        config: StaticViewerConfig,
        doc: LoadedDocument,
        window: Option<Arc<Window>>,
        host: Option<SurfaceHost>,
        width: u32,
        height: u32,
        redraws: u32,
    }

    impl ViewerApp {
        pub(super) fn new(config: StaticViewerConfig, doc: LoadedDocument) -> Self {
            Self { config, doc, window: None, host: None, width: 800, height: 600, redraws: 0 }
        }

        pub(super) fn outcome(&self) -> StaticViewerOutcome {
            StaticViewerOutcome {
                url: self.config.url.clone(),
                created_window: self.window.is_some(),
                redraws: self.redraws,
            }
        }

        /// Render the document at the current size + scroll and present it. The
        /// per-frame shape `serval-winit-host` documents: rasterize the scene into a
        /// texture, acquire the backbuffer, composite the texture onto it, present.
        fn render(&mut self, event_loop: &ActiveEventLoop) {
            let Some(host) = self.host.as_ref() else { return };
            let (w, h) = (self.width.max(1), self.height.max(1));
            let scene = self.doc.frame(w, h);
            // White canvas: a document with no root/body background paints over white
            // (the page background), as a browser does.
            let (_tex, view) = host.rasterize(&scene, w, h, ColorLoad::Clear(wgpu::Color::WHITE));
            let Some(frame) = host.acquire() else { return };
            let target = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
            host.renderer().compose_external_texture(
                &view,
                &target,
                host.format(),
                w,
                h,
                ExternalTexturePlacement::new([0.0, 0.0, w as f32, h as f32]),
            );
            frame.present();
            self.redraws += 1;
            if self.config.exit_after_first_redraw {
                event_loop.exit();
            }
        }

        fn request_redraw(&self) {
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
        }
    }

    impl ApplicationHandler for ViewerApp {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let attributes = Window::default_attributes()
                .with_title(self.config.title.clone())
                .with_inner_size(PhysicalSize::new(self.width, self.height));
            let window = match event_loop.create_window(attributes) {
                Ok(window) => Arc::new(window),
                Err(err) => {
                    eprintln!("[pelt-viewer] could not create window: {err}");
                    event_loop.exit();
                    return;
                },
            };
            let size = window.inner_size();
            self.width = size.width.max(1);
            self.height = size.height.max(1);
            let options = NetrenderOptions {
                tile_cache_size: Some(64),
                enable_vello: true,
                ..Default::default()
            };
            match SurfaceHost::boot(window.clone(), self.width, self.height, options) {
                Ok(host) => self.host = Some(host),
                Err(err) => {
                    eprintln!("[pelt-viewer] {err}");
                    event_loop.exit();
                    return;
                },
            }
            window.request_redraw();
            self.window = Some(window);
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
                WindowEvent::Resized(size) => {
                    self.width = size.width.max(1);
                    self.height = size.height.max(1);
                    if let Some(host) = self.host.as_mut() {
                        host.resize(self.width, self.height);
                    }
                    // The session rebuilds at the new size on the next frame
                    // (re-resolving %-height + viewport units).
                    self.request_redraw();
                },
                WindowEvent::MouseWheel { delta, .. } => {
                    // The shared wheel default action (scope doc rule 5): map the
                    // wheel to a device-px delta and scroll the document's viewport.
                    // Redraw only when the offset actually moved (not at an edge).
                    let (dx, dy) = wheel_delta_from_winit(delta);
                    if self.doc.scroll_by(dx, dy) {
                        self.request_redraw();
                    }
                },
                WindowEvent::RedrawRequested => self.render(event_loop),
                _ => {},
            }
        }
    }
}
