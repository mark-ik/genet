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
    // Load the document before opening a window, so a bad URL fails fast (and the
    // caller reports the error) rather than flashing an empty window.
    let doc = crate::document::LoadedDocument::load(&crate::document::LocalFetcher, &config.url)?;
    run_headed_with(config, doc)
}

/// Open a window and present `content` (any [`ViewerContent`](windowed::ViewerContent))
/// through the shared winit shell until the window closes. The static viewer and the
/// scripted viewer ([`crate::scripted`]) are the two callers — same shell, different
/// document. Kept generic (not a trait object) so each content type monomorphizes and
/// the scripted profile can pick its JS engine at the call site.
#[cfg(feature = "viewer")]
pub(crate) fn run_headed_with<C: windowed::ViewerContent + 'static>(
    config: StaticViewerConfig,
    content: C,
) -> Result<StaticViewerOutcome, String> {
    use winit::event_loop::EventLoop;

    let event_loop =
        EventLoop::new().map_err(|error| format!("could not create event loop: {error}"))?;
    let mut app = windowed::ViewerApp::new(config, content);
    event_loop
        .run_app(&mut app)
        .map_err(|error| format!("viewer event loop failed: {error}"))?;
    Ok(app.outcome())
}

#[cfg(feature = "viewer")]
pub(crate) mod windowed {
    use std::sync::Arc;
    use std::time::Instant;

    use netrender::external_texture::ExternalTexturePlacement;
    use netrender::{ColorLoad, NetrenderOptions, Scene};
    use serval_layout::ScrollKey;
    use serval_winit_host::{wheel_delta_from_winit, SurfaceHost};
    use winit::application::ApplicationHandler;
    use winit::dpi::PhysicalSize;
    use winit::event::{ElementState, MouseButton, WindowEvent};
    use winit::event_loop::ActiveEventLoop;
    use winit::keyboard::{Key, NamedKey};
    use winit::window::{Window, WindowId};

    use super::{StaticViewerConfig, StaticViewerOutcome};
    use crate::document::LoadedDocument;

    /// A document the viewer can present: render at a size, scroll, click, and (for
    /// scripted content) advance time-based work. The static [`LoadedDocument`] and
    /// the scripted [`ScriptedDocument`](crate::scripted::ScriptedDocument) both
    /// implement it, so they share this one winit shell — the lib-first surface the
    /// pelt plan's V5/V6 grow from.
    pub(crate) trait ViewerContent {
        /// Render at `width`×`height` at the current scroll.
        fn frame(&mut self, width: u32, height: u32) -> Scene;
        /// Scroll by a device-px wheel delta; return whether the offset moved.
        fn scroll_by(&mut self, dx: f32, dy: f32) -> bool;
        /// Apply a keyboard scroll default; return whether the offset moved.
        fn scroll_for_key(&mut self, key: ScrollKey) -> bool;
        /// Handle a left click at a scene point; return whether the document scrolled.
        fn click_at(&mut self, x: f32, y: f32) -> bool;
        /// Advance time-based work (script timers + GC) to `now_ms`; return whether
        /// more is pending, so the shell keeps requesting frames. Static content has
        /// none — the default returns `false` and the shell redraws only on input.
        fn pump(&mut self, _now_ms: f64) -> bool {
            false
        }
    }

    impl ViewerContent for LoadedDocument {
        fn frame(&mut self, width: u32, height: u32) -> Scene {
            LoadedDocument::frame(self, width, height)
        }
        fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
            LoadedDocument::scroll_by(self, dx, dy)
        }
        fn scroll_for_key(&mut self, key: ScrollKey) -> bool {
            LoadedDocument::scroll_for_key(self, key)
        }
        fn click_at(&mut self, x: f32, y: f32) -> bool {
            LoadedDocument::click_at(self, x, y)
        }
    }

    /// Map a winit key (with the shift state) to a [`ScrollKey`] default action, or
    /// `None` for keys that do not scroll. `Space` / `Shift+Space` are
    /// `PageDown` / `PageUp` (scope doc rule 5's key list). Pelt-inline for now; this
    /// lifts to `serval-winit-host` when meerkat shares the decode.
    fn scroll_key_from_winit(key: &Key, shift: bool) -> Option<ScrollKey> {
        Some(match key {
            Key::Named(NamedKey::ArrowUp) => ScrollKey::Up,
            Key::Named(NamedKey::ArrowDown) => ScrollKey::Down,
            Key::Named(NamedKey::ArrowLeft) => ScrollKey::Left,
            Key::Named(NamedKey::ArrowRight) => ScrollKey::Right,
            Key::Named(NamedKey::PageUp) => ScrollKey::PageUp,
            Key::Named(NamedKey::PageDown) => ScrollKey::PageDown,
            Key::Named(NamedKey::Home) => ScrollKey::Home,
            Key::Named(NamedKey::End) => ScrollKey::End,
            Key::Named(NamedKey::Space) => {
                if shift {
                    ScrollKey::PageUp
                } else {
                    ScrollKey::PageDown
                }
            },
            _ => return None,
        })
    }

    /// The viewer application: a [`ViewerContent`] document plus the window + shared
    /// present stack that drives it. Generic over the content so the static and
    /// scripted profiles share the shell.
    pub(crate) struct ViewerApp<C: ViewerContent> {
        config: StaticViewerConfig,
        doc: C,
        window: Option<Arc<Window>>,
        host: Option<SurfaceHost>,
        width: u32,
        height: u32,
        redraws: u32,
        /// Shift state, tracked from `ModifiersChanged`, so `Shift+Space` pages up.
        shift: bool,
        /// Last cursor position in physical px (winit's `MouseInput` carries none),
        /// so a click can hit-test the document for in-page link navigation.
        cursor: (f32, f32),
        /// Frame-loop clock origin, supplying the `now_ms` virtual clock that drives
        /// scripted content's timers (a no-op for static content).
        start: Instant,
    }

    impl<C: ViewerContent> ViewerApp<C> {
        pub(crate) fn new(config: StaticViewerConfig, doc: C) -> Self {
            Self {
                config,
                doc,
                window: None,
                host: None,
                width: 800,
                height: 600,
                redraws: 0,
                shift: false,
                cursor: (0.0, 0.0),
                start: Instant::now(),
            }
        }

        pub(crate) fn outcome(&self) -> StaticViewerOutcome {
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
            // Advance script time-based work (timers + GC) against the frame clock
            // before laying out; `more` is true while the content has pending work
            // (scripted timers), so the shell keeps the frame loop running.
            let now_ms = self.start.elapsed().as_secs_f64() * 1000.0;
            let more = self.doc.pump(now_ms);
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
                return;
            }
            if more {
                self.request_redraw();
            }
        }

        fn request_redraw(&self) {
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
        }
    }

    impl<C: ViewerContent> ApplicationHandler for ViewerApp<C> {
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
                WindowEvent::ModifiersChanged(mods) => {
                    self.shift = mods.state().shift_key();
                },
                WindowEvent::CursorMoved { position, .. } => {
                    self.cursor = (position.x as f32, position.y as f32);
                },
                WindowEvent::MouseInput { state, button, .. } => {
                    // A left click on an in-page link (`<a href="#id">`) scrolls its
                    // target into view (anchor-fragment navigation, scope doc rule 5).
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        let (x, y) = self.cursor;
                        if self.doc.click_at(x, y) {
                            self.request_redraw();
                        }
                    }
                },
                WindowEvent::KeyboardInput { event, .. } => {
                    // The keyboard scroll defaults (scope doc rule 5): map the key to
                    // a `ScrollKey` and scroll the document viewport. (No editable
                    // gate yet — pelt has no focusable fields in V1/V2; add the "focus
                    // not in an editable" check when it gains them.)
                    if event.state == ElementState::Pressed {
                        if let Some(key) = scroll_key_from_winit(&event.logical_key, self.shift) {
                            if self.doc.scroll_for_key(key) {
                                self.request_redraw();
                            }
                        }
                    }
                },
                WindowEvent::RedrawRequested => self.render(event_loop),
                _ => {},
            }
        }
    }
}
