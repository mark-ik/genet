/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The on-screen browser shell (V2): a chrome strip beside a content document, as two
//! separate roots composited in one window.
//!
//! Two `ScriptedDom`s that never reference each other: the [`Chrome`] (omnibar +
//! back/forward, a Cambium view tree) and the content [`LoadedDocument`]. Each
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
        WindowingMode::Headed => {
            windowed::run::<crate::document::LoadedDocument>(config, side, thickness)
        },
    }
}

/// Run the browser shell over a **smolweb** capsule (gemini/gopher/feed): the same
/// chrome strip + navigation, with a [`SmolwebDocument`](crate::SmolwebDocument)
/// content root rendered natively and themed per-site. Headless returns immediately.
#[cfg(feature = "smolweb")]
pub fn run_smolweb_browser(
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
        WindowingMode::Headed => windowed::run::<crate::SmolwebDocument>(config, side, thickness),
    }
}

pub(crate) mod windowed {
    use std::sync::Arc;

    use super::{StaticViewerConfig, StaticViewerOutcome};
    use crate::chrome::{Chrome, ChromeIntent, StripSide};
    use crate::document::{ClickOutcome, LoadedDocument, LocalFetcher};
    use crate::href::resolve_href;
    use cambium::{
        Key as CambiumKey, KeyEvent, Modifiers, NamedKey as CambiumNamedKey, PointerClick,
    };
    use genet_layout::ScrollKey;
    use genet_winit_host::{SurfaceHost, wheel_delta_from_winit};
    use netrender::external_texture::ExternalTexturePlacement;
    use netrender::{ColorLoad, NetrenderOptions};
    use winit::application::ApplicationHandler;
    use winit::dpi::PhysicalSize;
    use winit::event::{ElementState, MouseButton, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::keyboard::{Key, ModifiersState, NamedKey};
    use winit::window::{Window, WindowId};

    // Migration adapter. C4 replaces Pelt's compatibility dependency with
    // `cambium-winit`; the Genet presentation crate is already GUI-independent.
    fn key_event_from_winit(key: &Key, mods: Modifiers) -> Option<KeyEvent> {
        let mapped = match key {
            Key::Character(s) => CambiumKey::Character(s.to_string()),
            Key::Named(named) => CambiumKey::Named(match named {
                NamedKey::Backspace => CambiumNamedKey::Backspace,
                NamedKey::Enter => CambiumNamedKey::Enter,
                NamedKey::Tab => CambiumNamedKey::Tab,
                NamedKey::Escape => CambiumNamedKey::Escape,
                NamedKey::Space => CambiumNamedKey::Space,
                NamedKey::ArrowLeft => CambiumNamedKey::ArrowLeft,
                NamedKey::ArrowRight => CambiumNamedKey::ArrowRight,
                NamedKey::ArrowUp => CambiumNamedKey::ArrowUp,
                NamedKey::ArrowDown => CambiumNamedKey::ArrowDown,
                NamedKey::Delete => CambiumNamedKey::Delete,
                NamedKey::Home => CambiumNamedKey::Home,
                NamedKey::End => CambiumNamedKey::End,
                _ => CambiumNamedKey::Other,
            }),
            Key::Dead(_) | Key::Unidentified(_) => return None,
        };
        Some(KeyEvent::with_mods(mapped, mods))
    }

    fn modifiers_from_winit(state: ModifiersState) -> Modifiers {
        Modifiers {
            shift: state.shift_key(),
            ctrl: state.control_key(),
            alt: state.alt_key(),
            meta: state.super_key(),
        }
    }

    /// A rect `(x, y, w, h)` in window pixels.
    type Rect = (u32, u32, u32, u32);

    /// Content the browser shell can host: load by URL, render, scroll, and report
    /// what a click resolved to. `LoadedDocument` (HTML) and `SmolwebDocument`
    /// (gemini/gopher/feed) both implement it, so they share this one chrome shell —
    /// the same omnibar + back/forward + navigation, different document underneath.
    pub(crate) trait BrowsableContent: Sized {
        /// Fetch + parse `url` into a document (via the host's `LocalFetcher`).
        fn load(url: &str) -> Result<Self, String>;
        /// Render at `width`×`height` at the current scroll.
        fn frame(&mut self, width: u32, height: u32) -> netrender::Scene;
        /// Scroll by a device-px wheel delta at content-local `(x, y)`; return whether
        /// anything moved.
        fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool;
        /// Apply a keyboard scroll default; return whether the viewport moved.
        fn scroll_for_key(&mut self, key: ScrollKey) -> bool;
        /// Resolve a click at content-local `(x, y)` within a `width`×`height` content
        /// area.
        fn click_at(&mut self, x: f32, y: f32, width: u32, height: u32) -> ContentClick;
    }

    /// What a content click resolved to (the flavour-neutral `ClickOutcome`).
    pub(crate) enum ContentClick {
        /// Nothing actionable.
        None,
        /// An in-page scroll happened (the host redraws).
        Scrolled,
        /// A link to `target` (raw; the shell resolves it against the current URL).
        Navigate(String),
    }

    impl BrowsableContent for LoadedDocument {
        fn load(url: &str) -> Result<Self, String> {
            LoadedDocument::load(&LocalFetcher, url)
        }
        fn frame(&mut self, width: u32, height: u32) -> netrender::Scene {
            LoadedDocument::frame(self, width, height)
        }
        fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
            LoadedDocument::scroll_at(self, x, y, dx, dy)
        }
        fn scroll_for_key(&mut self, key: ScrollKey) -> bool {
            LoadedDocument::scroll_for_key(self, key)
        }
        fn click_at(&mut self, x: f32, y: f32, _width: u32, _height: u32) -> ContentClick {
            match LoadedDocument::click_at(self, x, y) {
                ClickOutcome::None => ContentClick::None,
                ClickOutcome::Scrolled => ContentClick::Scrolled,
                ClickOutcome::Navigate(href) => ContentClick::Navigate(href),
            }
        }
    }

    pub(crate) fn run<C: BrowsableContent + 'static>(
        config: StaticViewerConfig,
        side: StripSide,
        thickness: u32,
    ) -> Result<StaticViewerOutcome, String> {
        // Load the content before opening a window, so a bad URL fails fast.
        let content = C::load(&config.url)?;
        let chrome = Chrome::new(config.url.clone(), side, thickness);
        let event_loop =
            EventLoop::new().map_err(|error| format!("could not create event loop: {error}"))?;
        let mut app = BrowserApp::new(config, chrome, content);
        event_loop
            .run_app(&mut app)
            .map_err(|error| format!("browser event loop failed: {error}"))?;
        Ok(app.outcome())
    }

    /// The two-root browser: chrome + content + window + present stack. Generic over
    /// the content document so the HTML and smolweb browsers share one shell.
    struct BrowserApp<C: BrowsableContent> {
        config: StaticViewerConfig,
        chrome: Chrome,
        content: C,
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

    impl<C: BrowsableContent> BrowserApp<C> {
        fn new(config: StaticViewerConfig, chrome: Chrome, content: C) -> Self {
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
                StripSide::Bottom => (
                    (0, h.saturating_sub(t), w, t),
                    (0, 0, w, h.saturating_sub(t)),
                ),
                StripSide::Left => ((0, 0, t, h), (t, 0, w.saturating_sub(t), h)),
                StripSide::Right => (
                    (w.saturating_sub(t), 0, t, h),
                    (0, 0, w.saturating_sub(t), h),
                ),
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
                match C::load(&url) {
                    Ok(doc) => {
                        self.content = doc;
                        self.loaded_url = url;
                    },
                    Err(error) => eprintln!("[pelt] could not navigate to {url}: {error}"),
                }
            }
            self.request_redraw();
        }

        fn render(&mut self, event_loop: &ActiveEventLoop) {
            let Some(host) = self.host.as_ref() else {
                return;
            };
            let (win_w, win_h) = (self.width.max(1), self.height.max(1));
            let (strip, content_rect) = self.regions();

            // Each root renders its own scene at its layer size.
            let chrome_scene = self.chrome.frame(strip.2.max(1), strip.3.max(1));
            let content_scene = self
                .content
                .frame(content_rect.2.max(1), content_rect.3.max(1));

            // Rasterize both layers (kept alive until present), then composite each into
            // its window rect over the backbuffer.
            let (_ct, chrome_view) = host.rasterize(
                &chrome_scene,
                strip.2.max(1),
                strip.3.max(1),
                ColorLoad::Clear(wgpu::Color {
                    r: 0.17,
                    g: 0.17,
                    b: 0.2,
                    a: 1.0,
                }),
            );
            let (_vt, content_view) = host.rasterize(
                &content_scene,
                content_rect.2.max(1),
                content_rect.3.max(1),
                ColorLoad::Clear(wgpu::Color::WHITE),
            );
            let Some(frame) = host.acquire() else { return };
            let target = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
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
            },
            _ => return None,
        })
    }

    /// Whether `key` is the paste key — `V` (combined with Ctrl/Cmd by the caller) or a
    /// keyboard's dedicated Paste key.
    fn is_paste_key(key: &Key) -> bool {
        matches!(key, Key::Character(s) if s.eq_ignore_ascii_case("v"))
            || matches!(key, Key::Named(NamedKey::Paste))
    }

    /// Read UTF-8 text from the OS clipboard, or `None` if the clipboard can't be opened
    /// (e.g. a headless host) or holds no text.
    fn read_clipboard() -> Option<String> {
        arboard::Clipboard::new().ok()?.get_text().ok()
    }

    impl<C: BrowsableContent> ApplicationHandler for BrowserApp<C> {
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
                    eprintln!("[pelt-browser] {err}");
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
                    self.request_redraw();
                },
                WindowEvent::CursorMoved { position, .. } => {
                    self.cursor = (position.x as f32, position.y as f32);
                },
                WindowEvent::ModifiersChanged(mods) => {
                    self.mods = modifiers_from_winit(mods.state());
                },
                WindowEvent::MouseInput { state, button, .. } => {
                    if state != ElementState::Pressed || button != MouseButton::Left {
                        return;
                    }
                    let (strip, content_rect) = self.regions();
                    if in_rect(self.cursor, strip) {
                        let local = (
                            self.cursor.0 - strip.0 as f32,
                            self.cursor.1 - strip.1 as f32,
                        );
                        if let Some(node) = self.chrome.hit_test(local.0, local.1, strip.2, strip.3)
                        {
                            self.chrome.dispatch_click(node, PointerClick::at(local));
                            self.apply_chrome_intents();
                        }
                        self.request_redraw();
                    } else if in_rect(self.cursor, content_rect) {
                        let local = (
                            self.cursor.0 - content_rect.0 as f32,
                            self.cursor.1 - content_rect.1 as f32,
                        );
                        match self.content.click_at(
                            local.0,
                            local.1,
                            content_rect.2,
                            content_rect.3,
                        ) {
                            ContentClick::Scrolled => self.request_redraw(),
                            ContentClick::Navigate(href) => {
                                // A content link: resolve against the current URL and
                                // route it through the chrome's navigate path, so the
                                // omnibar + history update and the content reloads.
                                let url = resolve_href(&self.loaded_url, &href);
                                self.chrome.navigate_to(url);
                                self.apply_chrome_intents();
                            },
                            ContentClick::None => {},
                        }
                    }
                },
                WindowEvent::MouseWheel { delta, .. } => {
                    let (_, content_rect) = self.regions();
                    if in_rect(self.cursor, content_rect) {
                        let (dx, dy) = wheel_delta_from_winit(delta);
                        // The content renders into a sub-rect below the chrome strip;
                        // convert the cursor to content-local space (as the click path
                        // does) so the wheel scrolls the nested container under the
                        // pointer, else the document viewport.
                        let local = (
                            self.cursor.0 - content_rect.0 as f32,
                            self.cursor.1 - content_rect.1 as f32,
                        );
                        if self.content.scroll_at(local.0, local.1, dx, dy) {
                            self.request_redraw();
                        }
                    }
                },
                WindowEvent::KeyboardInput { event, .. } => {
                    if event.state != ElementState::Pressed {
                        return;
                    }
                    if self.chrome.focused().is_some() {
                        // The omnibar holds focus: Enter submits the URL, Ctrl/Cmd+V
                        // pastes the clipboard, every other key edits the field.
                        if matches!(event.logical_key, Key::Named(NamedKey::Enter)) {
                            self.chrome.submit_omnibar();
                            self.apply_chrome_intents();
                        } else if (self.mods.ctrl || self.mods.meta)
                            && is_paste_key(&event.logical_key)
                        {
                            if let Some(text) = read_clipboard() {
                                self.chrome.paste(&text);
                            }
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
                },
                WindowEvent::RedrawRequested => self.render(event_loop),
                _ => {},
            }
        }
    }
}
