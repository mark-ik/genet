/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The on-screen static document viewer (`pelt --engine static <url>`).
//!
//! A thin winit shell over a [`LoadedDocument`](crate::document::LoadedDocument)
//! presented through the shared [`SurfaceHost`](genet_winit_host::SurfaceHost):
//! the second instance of the orrery-host pattern (a window-agnostic content lib
//! plus a thin shell that maps winit events onto the content's semantic input and
//! rasterizes + composites its scene per frame). The document is the content;
//! wheel scrolling is its only interaction in V1, fed through the shared
//! default-action helper into the document's viewport.

use crate::{DesktopHostProfile, WindowingMode};
use genet_host_api::EngineProfile;

/// Configuration for one static-viewer run.
pub struct StaticViewerConfig {
    pub profile: DesktopHostProfile,
    pub url: String,
    pub title: String,
    /// Requested physical client size. `None` keeps the profile's established size.
    pub size: Option<(u32, u32)>,
    /// Exit after this many presented frames. `None` keeps the window interactive.
    pub frames: Option<u32>,
}

impl StaticViewerConfig {
    pub fn new(engine: EngineProfile, windowing: WindowingMode, url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            profile: DesktopHostProfile::new(engine, windowing),
            title: "Pelt".into(),
            url,
            size: None,
            frames: None,
        }
    }

    /// Request a physical client size for a headed run.
    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.size = Some((width.max(1), height.max(1)));
        self
    }

    /// Exit after presenting `frames` frames, for deterministic headed smoke runs.
    pub fn with_frame_limit(mut self, frames: u32) -> Self {
        self.frames = Some(frames.max(1));
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticViewerOutcome {
    pub url: String,
    pub created_window: bool,
    pub redraws: u32,
    /// The physical client size the headed run actually achieved, or `(0, 0)` when
    /// no window was created.
    pub size: (u32, u32),
}

/// Turn a document title into the stable native-window title, falling back to
/// the loaded URL's host.
///
/// Gemini, gopher, finger and nex carry no title element, so without the
/// fallback every capsule opens a window called plain "Pelt" -- indistinguishable
/// in the taskbar from every other one.
#[cfg(feature = "viewer")]
pub(crate) fn pelt_window_title(document_title: Option<&str>, url: Option<&str>) -> String {
    let named = document_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .or_else(|| url.and_then(crate::url_host));
    match named {
        Some(title) => format!("Pelt — {title}"),
        None => "Pelt".into(),
    }
}

/// Pelt's native window treatment, shared by all three headed profiles. The icon is
/// intentionally generated here: it is a real taskbar/title-bar icon without adding a
/// platform-specific asset pipeline to the reference host.
#[cfg(feature = "viewer")]
pub(crate) fn pelt_window_attributes(
    title: impl Into<String>,
    width: u32,
    height: u32,
) -> winit::window::WindowAttributes {
    use winit::dpi::PhysicalSize;
    use winit::window::{Icon, Window};

    const EDGE: u32 = 32;
    let mut rgba = vec![0; (EDGE * EDGE * 4) as usize];
    for y in 0..EDGE {
        for x in 0..EDGE {
            let i = ((y * EDGE + x) * 4) as usize;
            let (r, g, b) = if (4..28).contains(&x) && (4..28).contains(&y) {
                if (9..23).contains(&x) && (9..15).contains(&y)
                    || (9..15).contains(&x) && (9..24).contains(&y)
                    || (14..23).contains(&x) && (18..24).contains(&y)
                {
                    (133, 202, 255)
                } else {
                    (43, 43, 51)
                }
            } else {
                (0, 0, 0)
            };
            rgba[i..i + 4].copy_from_slice(&[
                r,
                g,
                b,
                if r == 0 && g == 0 && b == 0 { 0 } else { 255 },
            ]);
        }
    }
    let icon = Icon::from_rgba(rgba, EDGE, EDGE).expect("the fixed Pelt icon is valid RGBA");
    let attributes = Window::default_attributes()
        .with_title(title)
        .with_inner_size(PhysicalSize::new(width.max(1), height.max(1)))
        .with_window_icon(Some(icon));
    #[cfg(windows)]
    let attributes = {
        use winit::platform::windows::{Color, WindowAttributesExtWindows};
        attributes
            .with_title_background_color(Some(Color::from_rgb(43, 43, 51)))
            .with_title_text_color(Color::from_rgb(245, 245, 247))
    };
    attributes
}

/// Convert a physical window extent to the logical CSS/layout extent used by
/// Pelt's scenes. Keep this conversion beside the window attributes so every
/// headed profile shares the same DPI convention.
pub(crate) fn logical_extent(physical: u32, scale_factor: f32) -> u32 {
    ((physical.max(1) as f32 / scale_factor.max(1.0)).round() as u32).max(1)
}

/// Convert a physical winit pointer coordinate to the matching logical scene
/// coordinate. Layout, painting, and hit tests all use this one space.
pub(crate) fn logical_position(physical: f32, scale_factor: f32) -> f32 {
    physical / scale_factor.max(1.0)
}

#[cfg(test)]
mod dpi_tests {
    use super::{logical_extent, logical_position};

    #[test]
    fn physical_window_space_maps_to_one_logical_scene_space() {
        assert_eq!(logical_extent(1600, 2.0), 800);
        assert_eq!(logical_extent(900, 1.5), 600);
        assert!((logical_position(640.0, 2.0) - 320.0).abs() < f32::EPSILON);
    }
}

#[cfg(all(test, feature = "viewer"))]
mod title_tests {
    use super::pelt_window_title;

    /// A titled document names the window. The smolweb formats carry no title
    /// element at all, so without a fallback every capsule opened a window
    /// called plain "Pelt" and the taskbar could not tell two of them apart.
    #[test]
    fn a_window_is_named_by_its_document_then_by_its_host() {
        let named = Some("Merely | Local-first software");
        assert_eq!(
            pelt_window_title(named, Some("https://merelyllc.com")),
            "Pelt — Merely | Local-first software"
        );
        for url in [
            "gemini://geminiprotocol.net/",
            "gemini://user@geminiprotocol.net:1965/page",
        ] {
            assert_eq!(
                pelt_window_title(None, Some(url)),
                "Pelt — geminiprotocol.net",
                "naming a window for {url}"
            );
        }
        // A blank title is as absent as no title.
        assert_eq!(
            pelt_window_title(Some("   "), Some("gopher://gopher.floodgap.com/")),
            "Pelt — gopher.floodgap.com"
        );
        // Nothing to fall back on: a local file has no authority.
        assert_eq!(pelt_window_title(None, Some("C:\\docs\\a.html")), "Pelt");
        assert_eq!(pelt_window_title(None, None), "Pelt");
    }
}

/// Product-level local receipt for the explicit Livery pin. It uses the same
/// registry construction as `run_livery_viewer`, but keeps scene inspection
/// GPU-free so the resource and interaction assertions are stable in CI.
#[cfg(all(test, feature = "livery"))]
mod livery_route_tests {
    use genet_documents::{LiveryDocumentSession, LiverySessionEngine, LocalFetcher};
    use inker::{
        DocumentSession, SessionEngine, SessionRegistry, SessionScrollKey, SessionSpawnRequest,
    };
    use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};
    use netrender::{Scene, SceneOp};

    fn node_by_id(
        dom: &genet_static_dom::StaticDocument,
        expected: &str,
    ) -> genet_static_dom::StaticNodeId {
        fn find(
            dom: &genet_static_dom::StaticDocument,
            node: genet_static_dom::StaticNodeId,
            expected: &str,
        ) -> Option<genet_static_dom::StaticNodeId> {
            if dom.kind(node) == NodeKind::Element
                && dom.attribute(node, &Namespace::default(), &LocalName::from("id"))
                    == Some(expected)
            {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| find(dom, child, expected))
        }

        find(dom, dom.document(), expected).expect("fixture element by id")
    }

    #[test]
    fn local_livery_route_keeps_resource_identity_and_interaction_after_resize() {
        let fixture = format!(
            r"{}\..\examples\livery-route\index.html",
            env!("CARGO_MANIFEST_DIR")
        );
        let mut registry: SessionRegistry<Scene> = SessionRegistry::new();
        registry.register(Box::new(LiverySessionEngine::new(LocalFetcher)));
        let request = SessionSpawnRequest::new(&fixture).with_viewport(960, 640);
        let mut session = registry
            .spawn(inker::routing::ENGINE_GENET_LIVERY, &request)
            .expect("Pelt can spawn the explicit Livery pin");

        let first = session.frame(960, 640);
        assert!(
            first
                .ops
                .iter()
                .any(|operation| matches!(operation, SceneOp::Image(_))),
            "the linked CSS background or HTML image reaches the product scene"
        );

        let concrete = session
            .as_any()
            .downcast_mut::<LiveryDocumentSession>()
            .expect("the registry returned a Livery document session");
        let resources = concrete.resource_set();
        assert_eq!(
            resources.stylesheets.len(),
            4,
            "two inline and two linked sheets"
        );
        assert!(
            resources.stylesheets.iter().any(|sheet| sheet
                .source_url
                .as_deref()
                .is_some_and(|url| url.replace('\\', "/").ends_with("assets/route.css"))),
            "the linked stylesheet retains its own local identity"
        );
        assert!(
            resources.resources.iter().any(|resource| resource
                .resolved_url
                .replace('\\', "/")
                .ends_with("resources/servo_64.png")),
            "the linked image remains attributed to its source-relative URL"
        );
        assert!(
            resources.resources.iter().any(|resource| resource
                .resolved_url
                .replace('\\', "/")
                .ends_with("components/genet-layout/Ahem.ttf")),
            "the linked font remains attributed to its stylesheet-relative URL"
        );
        assert!(
            resources.diagnostics.is_empty(),
            "the product fixture has no missing or deferred resources: {:?}",
            resources.diagnostics
        );

        let body = node_by_id(concrete.document().dom(), "route-body");
        assert_eq!(
            concrete
                .document()
                .computed_style(body, "background-color")
                .as_deref(),
            Some("rgb(243, 236, 220)"),
            "the print-media sheet does not apply on the screen route"
        );
        let source_order = node_by_id(concrete.document().dom(), "source-order");
        assert_eq!(
            concrete
                .document()
                .computed_style(source_order, "color")
                .as_deref(),
            Some("rgb(107, 31, 45)"),
            "the later inline stylesheet wins the linked sheet at equal specificity"
        );

        // A second frame at a different viewport is the same resize path the
        // headed viewer uses. Link geometry must survive it and drive fragment
        // navigation before ordinary viewport scrolling resumes.
        let _resized = session.frame(640, 480);
        assert!(
            session.content_height(640, 480) > 480,
            "fixture is scrollable"
        );
        let link = session
            .links()
            .into_iter()
            .find(|link| link.url == "#resource-target")
            .expect("fixture jump link");
        assert!(matches!(
            session.click_at(link.rect[0] + 2.0, link.rect[1] + 2.0),
            inker::SessionClick::Handled
        ));
        assert!(session.scroll_for_key(SessionScrollKey::Home));
        assert!(session.scroll_by(0.0, 120.0));
        assert!(session.scroll_at(8.0, 8.0, 0.0, -80.0));
    }
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
            size: (0, 0),
        }),
        WindowingMode::Headed => run_headed(config),
    }
}

/// Run the opt-in Livery engine through its inker registry entry. The
/// incumbent static viewer deliberately continues using its existing direct
/// `LoadedDocument` route until its frozen receipts are re-blessed.
#[cfg(feature = "livery")]
pub fn run_livery_viewer(config: StaticViewerConfig) -> Result<StaticViewerOutcome, String> {
    use genet_documents::LiverySessionEngine;
    use inker::{SessionRegistry, SessionSpawnRequest};
    use netrender::Scene;

    if matches!(config.profile.windowing, WindowingMode::Headless) {
        return Ok(StaticViewerOutcome {
            url: config.url,
            created_window: false,
            redraws: 0,
            size: (0, 0),
        });
    }
    let (width, height) = config.size.unwrap_or((800, 600));
    let mut registry: SessionRegistry<Scene> = SessionRegistry::new();
    registry.register(Box::new(LiverySessionEngine::new(
        crate::document::LocalFetcher,
    )));
    let request = SessionSpawnRequest::new(&config.url).with_viewport(width, height);
    let session = registry
        .spawn(inker::routing::ENGINE_GENET_LIVERY, &request)
        .map_err(|error| format!("could not spawn engine genet.livery: {error}"))?;
    run_headed_with(config, LiveryViewerContent { session })
}

/// Without the `viewer` feature there is no render / present stack, so a headed run
/// cannot proceed. (The light contracts + smoke build excludes it.)
#[cfg(not(feature = "viewer"))]
fn run_headed(_config: StaticViewerConfig) -> Result<StaticViewerOutcome, String> {
    Err(
        "the static viewer needs the `viewer` feature (pelt's default build enables it)"
            .to_string(),
    )
}

#[cfg(feature = "viewer")]
fn run_headed(config: StaticViewerConfig) -> Result<StaticViewerOutcome, String> {
    // Load the document before opening a window, so a bad URL fails fast (and the
    // caller reports the error) rather than flashing an empty window.
    let doc = crate::document::LoadedDocument::load(&crate::document::LocalFetcher, &config.url)?;
    run_headed_with(config, doc)
}

#[cfg(feature = "livery")]
struct LiveryViewerContent {
    session: Box<dyn inker::DocumentSession<netrender::Scene>>,
}

#[cfg(feature = "livery")]
impl windowed::ViewerContent for LiveryViewerContent {
    fn title(&self) -> Option<String> {
        self.session.inspect().and_then(|report| report.title)
    }

    fn frame(&mut self, width: u32, height: u32) -> netrender::Scene {
        self.session.frame(width, height)
    }

    fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
        self.session.scroll_by(dx, dy)
    }

    fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        self.session.scroll_at(x, y, dx, dy)
    }

    fn scroll_for_key(&mut self, key: genet_layout::ScrollKey) -> bool {
        let key = match key {
            genet_layout::ScrollKey::Up => inker::SessionScrollKey::LineUp,
            genet_layout::ScrollKey::Down => inker::SessionScrollKey::LineDown,
            genet_layout::ScrollKey::PageUp => inker::SessionScrollKey::PageUp,
            genet_layout::ScrollKey::PageDown => inker::SessionScrollKey::PageDown,
            genet_layout::ScrollKey::Home => inker::SessionScrollKey::Home,
            genet_layout::ScrollKey::End => inker::SessionScrollKey::End,
            genet_layout::ScrollKey::Left | genet_layout::ScrollKey::Right => return false,
        };
        self.session.scroll_for_key(key)
    }

    fn click_at(&mut self, x: f32, y: f32) -> bool {
        matches!(self.session.click_at(x, y), inker::SessionClick::Handled)
    }
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

    use genet_layout::ScrollKey;
    use genet_winit_host::{SurfaceHost, wheel_delta_from_winit};
    use netrender::external_texture::ExternalTexturePlacement;
    use netrender::{ColorLoad, NetrenderOptions, Scene};
    use winit::application::ApplicationHandler;
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
        /// The document title for native window chrome, when this content has one.
        fn title(&self) -> Option<String> {
            None
        }
        /// Render at `width`×`height` at the current scroll.
        fn frame(&mut self, width: u32, height: u32) -> Scene;
        /// Scroll by a device-px wheel delta; return whether the offset moved.
        fn scroll_by(&mut self, dx: f32, dy: f32) -> bool;
        /// Scroll by a device-px wheel delta at scene point `(x, y)`: the wheel default
        /// action routes to the nearest `overflow: scroll/auto` container under the
        /// pointer, falling through to the document viewport. Returns whether anything
        /// moved. The default ignores the position and scrolls the viewport (the
        /// behaviour for content with no retained per-element scroll, e.g. the scripted
        /// document's per-frame-rebuilt session); the static [`LoadedDocument`] overrides
        /// it with the position-aware nested scroll its persistent session retains.
        fn scroll_at(&mut self, _x: f32, _y: f32, dx: f32, dy: f32) -> bool {
            self.scroll_by(dx, dy)
        }
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
        fn title(&self) -> Option<String> {
            LoadedDocument::inspect(self).title
        }

        fn frame(&mut self, width: u32, height: u32) -> Scene {
            LoadedDocument::frame_for_viewer(self, width, height)
        }
        fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
            LoadedDocument::scroll_by(self, dx, dy)
        }
        fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
            LoadedDocument::scroll_at(self, x, y, dx, dy)
        }
        fn scroll_for_key(&mut self, key: ScrollKey) -> bool {
            LoadedDocument::scroll_for_key(self, key)
        }
        fn click_at(&mut self, x: f32, y: f32) -> bool {
            // The bare viewer has no chrome/history to navigate, so it only acts on the
            // in-page scroll; a cross-document link is a no-op here (navigation is the
            // chrome (V2) and tile (V5) lanes' job).
            matches!(
                LoadedDocument::click_at(self, x, y),
                crate::document::ClickOutcome::Scrolled
            )
        }
    }

    /// Map a winit key (with the shift state) to a [`ScrollKey`] default action, or
    /// `None` for keys that do not scroll. `Space` / `Shift+Space` are
    /// `PageDown` / `PageUp` (scope doc rule 5's key list). Pelt-inline for now; this
    /// lifts to `genet-winit-host` when meerkat shares the decode.
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
        /// Physical device pixels per logical CSS/layout pixel.
        scale_factor: f32,
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
                width: config.size.map_or(800, |size| size.0),
                height: config.size.map_or(600, |size| size.1),
                scale_factor: 1.0,
                config,
                doc,
                window: None,
                host: None,
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
                size: if self.window.is_some() {
                    (self.width, self.height)
                } else {
                    (0, 0)
                },
            }
        }

        fn window_title(&self) -> String {
            super::pelt_window_title(self.doc.title().as_deref(), Some(&self.config.url))
        }

        fn logical_size(&self) -> (u32, u32) {
            (
                super::logical_extent(self.width, self.scale_factor),
                super::logical_extent(self.height, self.scale_factor),
            )
        }

        /// Render the document at the current size + scroll and present it. The
        /// per-frame shape `genet-winit-host` documents: rasterize the scene into a
        /// texture, acquire the backbuffer, composite the texture onto it, present.
        fn render(&mut self, event_loop: &ActiveEventLoop) {
            // Advance script time-based work (timers + GC) against the frame clock
            // before laying out; `more` is true while the content has pending work
            // (scripted timers), so the shell keeps the frame loop running.
            let now_ms = self.start.elapsed().as_secs_f64() * 1000.0;
            let more = self.doc.pump(now_ms);
            let Some(host) = self.host.as_ref() else {
                return;
            };
            let (w, h) = self.logical_size();
            let scene = self.doc.frame(w, h);
            // White canvas: a document with no root/body background paints over white
            // (the page background), as a browser does.
            let (_tex, view) = host.rasterize_scaled(
                &scene,
                self.width.max(1),
                self.height.max(1),
                ColorLoad::Clear(wgpu::Color::WHITE),
                self.scale_factor,
            );
            let Some(frame) = host.acquire() else { return };
            let target = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            host.renderer().compose_external_texture(
                &view,
                &target,
                host.format(),
                self.width,
                self.height,
                ExternalTexturePlacement::new([0.0, 0.0, self.width as f32, self.height as f32]),
            );
            frame.present();
            self.redraws += 1;
            if self
                .config
                .frames
                .is_some_and(|limit| self.redraws >= limit)
            {
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
            let attributes =
                super::pelt_window_attributes(self.window_title(), self.width, self.height);
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
            self.scale_factor = window.scale_factor() as f32;
            window.set_title(&self.window_title());
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
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    self.scale_factor = scale_factor as f32;
                    if let Some(window) = self.window.as_ref() {
                        let size = window.inner_size();
                        self.width = size.width.max(1);
                        self.height = size.height.max(1);
                    }
                    if let Some(host) = self.host.as_mut() {
                        host.resize(self.width, self.height);
                    }
                    self.request_redraw();
                },
                WindowEvent::MouseWheel { delta, .. } => {
                    // The shared wheel default action (scope doc rule 5): map the wheel
                    // to a device-px delta and scroll at the cursor — a nested
                    // `overflow: scroll/auto` container under the pointer takes it first,
                    // else the document viewport. The viewer fills the window, so the
                    // cursor is already in document space. Redraw only when something
                    // moved (not at an edge).
                    let (dx, dy) = wheel_delta_from_winit(delta);
                    let (dx, dy) = (dx / self.scale_factor, dy / self.scale_factor);
                    if self.doc.scroll_at(self.cursor.0, self.cursor.1, dx, dy) {
                        self.request_redraw();
                    }
                },
                WindowEvent::ModifiersChanged(mods) => {
                    self.shift = mods.state().shift_key();
                },
                WindowEvent::CursorMoved { position, .. } => {
                    self.cursor = (
                        super::logical_position(position.x as f32, self.scale_factor),
                        super::logical_position(position.y as f32, self.scale_factor),
                    );
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
