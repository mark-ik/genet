/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The on-screen browser shell (V2): a chrome strip beside a content document, as two
//! separate roots composited in one window.
//!
//! Two `ScriptedDom`s that never reference each other: the [`Chrome`] (omnibar +
//! back/forward, a xilem-serval view tree) and the content [`LoadedDocument`]. Each
//! renders to its own `netrender::Scene`; the shell rasterizes both and composites
//! them as layers (`compose_external_texture` per layer) onto the window backbuffer.
//! Input routes by region: a press/keystroke in the strip drives the chrome, elsewhere
//! the content. The chrome reaches the content root only through [`ChromeIntent`]s the
//! shell applies (reloading the content document on a navigation) — the seam that keeps
//! the two roots isolated.

use crate::chrome::StripSide;
use crate::{StaticViewerConfig, StaticViewerOutcome, WindowingMode};

/// Run the browser shell for `config`, with the chrome strip on `side` at `thickness`
/// px. Headless returns immediately (the smoke shape); headed opens the window.
pub fn run_chrome_viewer(
    config: StaticViewerConfig,
    side: StripSide,
    thickness: u32,
) -> Result<StaticViewerOutcome, String> {
    match config.profile.windowing {
        WindowingMode::Headless => Ok(StaticViewerOutcome {
            url: config.url,
            created_window: false,
            redraws: 0,
        }),
        WindowingMode::Headed => windowed::run(config, side, thickness),
    }
}

mod windowed {
    use std::sync::Arc;

    use netrender::external_texture::ExternalTexturePlacement;
    use netrender::{ColorLoad, NetrenderOptions};
    use serval_layout::ScrollKey;
    use serval_winit_host::{
        key_event_from_winit, modifiers_from_winit, wheel_delta_from_winit, SurfaceHost,
    };
    use winit::application::ApplicationHandler;
    use winit::dpi::PhysicalSize;
    use winit::event::{ElementState, MouseButton, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::keyboard::{Key, NamedKey};
    use winit::window::{Window, WindowId};
    use xilem_serval::{Modifiers, PointerClick};

    use super::{StaticViewerConfig, StaticViewerOutcome};
    use crate::chrome::{Chrome, ChromeIntent, StripSide};
    use crate::document::{ClickOutcome, LoadedDocument, LocalFetcher, resolve_href};

    /// A rect `(x, y, w, h)` in window pixels.
    type Rect = (u32, u32, u32, u32);

    pub(super) fn run(
        config: StaticViewerConfig,
        side: StripSide,
        thickness: u32,
    ) -> Result<StaticViewerOutcome, String> {
        // Load the content before opening a window, so a bad URL fails fast.
        let content = LoadedDocument::load(&LocalFetcher, &config.url)?;
        let chrome = Chrome::new(config.url.clone(), side, thickness);
        let event_loop =
            EventLoop::new().map_err(|error| format!("could not create event loop: {error}"))?;
        let mut app = BrowserApp::new(config, chrome, content);
        event_loop
            .run_app(&mut app)
            .map_err(|error| format!("browser event loop failed: {error}"))?;
        Ok(app.outcome())
    }

    /// The two-root browser: chrome + content + window + present stack.
    struct BrowserApp {
        config: StaticViewerConfig,
        chrome: Chrome,
        content: LoadedDocument,
        /// The URL currently loaded into `content`, so a back/forward to the same page
        /// (or an edge no-op) skips a reload.
        loaded_url: String,
        window: Option<Arc<Window>>,
        host: Option<SurfaceHost>,
        width: u32,
        height: u32,
        cursor: (f32, f32),
        mods: Modifiers,
        redraws: u32,
    }

    impl BrowserApp {
        fn new(config: StaticViewerConfig, chrome: Chrome, content: LoadedDocument) -> Self {
            let loaded_url = config.url.clone();
            Self {
                config,
                chrome,
                content,
                loaded_url,
                window: None,
                host: None,
                width: 1000,
                height: 700,
                cursor: (0.0, 0.0),
                mods: Modifiers::default(),
                redraws: 0,
            }
        }

        fn outcome(&self) -> StaticViewerOutcome {
            StaticViewerOutcome {
                url: self.loaded_url.clone(),
                created_window: self.window.is_some(),
                redraws: self.redraws,
            }
        }

        /// `(strip_rect, content_rect)` for the current side + thickness + window size.
        fn regions(&self) -> (Rect, Rect) {
            let (w, h) = (self.width.max(1), self.height.max(1));
            let side = self.chrome.side();
            let t = self
                .chrome
                .thickness()
                .min(if side.is_horizontal() { h } else { w });
            match side {
                StripSide::Top => ((0, 0, w, t), (0, t, w, h.saturating_sub(t))),
                StripSide::Bottom => ((0, h.saturating_sub(t), w, t), (0, 0, w, h.saturating_sub(t))),
                StripSide::Left => ((0, 0, t, h), (t, 0, w.saturating_sub(t), h)),
                StripSide::Right => ((w.saturating_sub(t), 0, t, h), (0, 0, w.saturating_sub(t), h)),
            }
        }

        /// Drain the chrome's queued intents; on a navigation, reload the content root
        /// with the chrome's now-current URL (skipping a reload if it didn't change).
        fn apply_chrome_intents(&mut self) {
            let intents = self.chrome.take_intents();
            let navigated = intents.iter().any(|i| {
                matches!(
                    i,
                    ChromeIntent::Navigate(_) | ChromeIntent::Back | ChromeIntent::Forward
                )
            });
            if !navigated {
                return;
            }
            let url = self.chrome.state().current().to_string();
            if url != self.loaded_url {
                match LoadedDocument::load(&LocalFetcher, &url) {
                    Ok(doc) => {
                        self.content = doc;
                        self.loaded_url = url;
                    }
                    Err(error) => eprintln!("[pelt] could not navigate to {url}: {error}"),
                }
            }
            self.request_redraw();
        }

        fn render(&mut self, event_loop: &ActiveEventLoop) {
            let Some(host) = self.host.as_ref() else { return };
            let (win_w, win_h) = (self.width.max(1), self.height.max(1));
            let (strip, content_rect) = self.regions();

            // Each root renders its own scene at its layer size.
            let chrome_scene = self.chrome.frame(strip.2.max(1), strip.3.max(1));
            let content_scene = self.content.frame(content_rect.2.max(1), content_rect.3.max(1));

            // Rasterize both layers (kept alive until present), then composite each into
            // its window rect over the backbuffer.
            let (_ct, chrome_view) = host.rasterize(
                &chrome_scene,
                strip.2.max(1),
                strip.3.max(1),
                ColorLoad::Clear(wgpu::Color { r: 0.17, g: 0.17, b: 0.2, a: 1.0 }),
            );
            let (_vt, content_view) = host.rasterize(
                &content_scene,
                content_rect.2.max(1),
                content_rect.3.max(1),
                ColorLoad::Clear(wgpu::Color::WHITE),
            );
            let Some(frame) = host.acquire() else { return };
            let target = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
            let renderer = host.renderer();
            renderer.compose_external_texture(
                &chrome_view,
                &target,
                host.format(),
                win_w,
                win_h,
                placement(strip),
            );
            renderer.compose_external_texture(
                &content_view,
                &target,
                host.format(),
                win_w,
                win_h,
                placement(content_rect),
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

    /// Map a window-pixel rect to an [`ExternalTexturePlacement`] (`[x0, y0, x1, y1]`).
    fn placement(r: Rect) -> ExternalTexturePlacement {
        ExternalTexturePlacement::new([
            r.0 as f32,
            r.1 as f32,
            (r.0 + r.2) as f32,
            (r.1 + r.3) as f32,
        ])
    }

    fn in_rect(p: (f32, f32), r: Rect) -> bool {
        p.0 >= r.0 as f32
            && p.0 < (r.0 + r.2) as f32
            && p.1 >= r.1 as f32
            && p.1 < (r.1 + r.3) as f32
    }

    /// Map a winit key (with shift) to a content [`ScrollKey`], or `None`. Mirrors the
    /// static viewer's decode (the content half of the shell shares the rule).
    fn scroll_key(key: &Key, shift: bool) -> Option<ScrollKey> {
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
            }
            _ => return None,
        })
    }

    impl ApplicationHandler for BrowserApp {
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
                    eprintln!("[pelt-browser] could not create window: {err}");
                    event_loop.exit();
                    return;
                }
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
                    eprintln!("[pelt-browser] {err}");
                    event_loop.exit();
                    return;
                }
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
                    self.request_redraw();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    self.cursor = (position.x as f32, position.y as f32);
                }
                WindowEvent::ModifiersChanged(mods) => {
                    self.mods = modifiers_from_winit(mods.state());
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    if state != ElementState::Pressed || button != MouseButton::Left {
                        return;
                    }
                    let (strip, content_rect) = self.regions();
                    if in_rect(self.cursor, strip) {
                        let local = (self.cursor.0 - strip.0 as f32, self.cursor.1 - strip.1 as f32);
                        if let Some(node) = self.chrome.hit_test(local.0, local.1, strip.2, strip.3) {
                            self.chrome.dispatch_click(node, PointerClick::at(local));
                            self.apply_chrome_intents();
                        }
                        self.request_redraw();
                    } else if in_rect(self.cursor, content_rect) {
                        let local = (
                            self.cursor.0 - content_rect.0 as f32,
                            self.cursor.1 - content_rect.1 as f32,
                        );
                        match self.content.click_at(local.0, local.1) {
                            ClickOutcome::Scrolled => self.request_redraw(),
                            ClickOutcome::Navigate(href) => {
                                // A content link: resolve against the current URL and
                                // route it through the chrome's navigate path, so the
                                // omnibar + history update and the content reloads.
                                let url = resolve_href(&self.loaded_url, &href);
                                self.chrome.navigate_to(url);
                                self.apply_chrome_intents();
                            }
                            ClickOutcome::None => {}
                        }
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let (_, content_rect) = self.regions();
                    if in_rect(self.cursor, content_rect) {
                        let (dx, dy) = wheel_delta_from_winit(delta);
                        if self.content.scroll_by(dx, dy) {
                            self.request_redraw();
                        }
                    }
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if event.state != ElementState::Pressed {
                        return;
                    }
                    if self.chrome.focused().is_some() {
                        // The omnibar holds focus: Enter submits the URL, every other
                        // key edits the field.
                        if matches!(event.logical_key, Key::Named(NamedKey::Enter)) {
                            self.chrome.submit_omnibar();
                            self.apply_chrome_intents();
                        } else if let Some(key_event) =
                            key_event_from_winit(&event.logical_key, self.mods)
                        {
                            self.chrome.dispatch_key(key_event);
                        }
                        self.request_redraw();
                    } else if let Some(key) = scroll_key(&event.logical_key, self.mods.shift) {
                        // Nothing focused in the chrome: keys scroll the content.
                        if self.content.scroll_for_key(key) {
                            self.request_redraw();
                        }
                    }
                }
                WindowEvent::RedrawRequested => self.render(event_loop),
                _ => {}
            }
        }
    }
}
