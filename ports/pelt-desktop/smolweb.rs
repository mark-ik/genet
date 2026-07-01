/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Smolweb documents as native serval views (the `smolweb` feature).
//!
//! The smolweb twin of [`Chrome`](crate::chrome::Chrome): errand parses a fetched
//! capsule into a per-format AST, a `smolweb-views` view renders it into a
//! `ScriptedDom`, and serval lays it out and paints it to a [`Scene`] through the
//! same GPU-free path the chrome uses. Native, not gemtext-to-HTML — real focusable
//! link elements, per-format classes, per-site theming. A link click resolves to a
//! navigation target via the view's `on_navigate` action.
//!
//! This is the GPU-free foundation (load → parse → view → scene, plus click → URL);
//! the windowed viewer that presents it is the integration step on top, mirroring
//! [`static_viewer`](crate::static_viewer) / [`chrome_viewer`](crate::chrome_viewer).

use std::cell::RefCell;
use std::rc::Rc;

use errand::parse::{feed, gemtext, gopher};
use netrender::Scene;
use pelt_core::ResourceFetcher;
use serval_layout::{IncrementalLayout, ScrollKey, ScrollOffsets};
use serval_render::scene_from_session_dom;
use serval_scripted_dom::{NodeId, ScriptedDom};
use smolweb_views::{feed_view, gemtext_view, gopher_view, stylesheet, SmolwebTheme, SmolwebView};
use xilem_serval::{DomHandle, PointerClick, ServalAppRunner};

/// A link-click navigation target, the action the smolweb views bubble.
struct Navigate(String);
impl xilem_serval::Action for Navigate {}

/// A fetched capsule parsed into the AST its native view consumes.
enum Content {
    Gemtext(Vec<gemtext::GemLine>),
    Gopher(Vec<gopher::GopherItem>),
    Feed(feed::Feed),
}

/// The view's app state: just the parsed content (the view is a pure projection of
/// it; navigation bubbles out as a [`Navigate`] action, not via state).
struct SmolwebState {
    content: Content,
}

type SmolwebLogic = fn(&SmolwebState) -> SmolwebView<SmolwebState, Navigate>;

/// Project the parsed content onto its native view, each link emitting `Navigate`.
fn view(state: &SmolwebState) -> SmolwebView<SmolwebState, Navigate> {
    match &state.content {
        Content::Gemtext(lines) => gemtext_view(lines, |url| Navigate(url.to_string())),
        Content::Gopher(items) => gopher_view(items, |url| Navigate(url.to_string())),
        Content::Feed(parsed) => feed_view(parsed, |url| Navigate(url.to_string())),
    }
}

/// A loaded smolweb document: a serval `ScriptedDom` built from a native smolweb
/// view, rendered GPU-free to a [`Scene`], with link clicks resolving to URLs.
pub struct SmolwebDocument {
    runner: ServalAppRunner<SmolwebState, SmolwebLogic, SmolwebView<SmolwebState, Navigate>, Navigate>,
    sheets: Vec<String>,
    /// The retained cascade + layout session, owner of the viewport scroll. Built
    /// lazily at the first render size and rebuilt on resize; `None` before the first
    /// frame. Mirrors `LoadedDocument`'s render-first session.
    session: Option<IncrementalLayout<NodeId>>,
    /// The size `session` was laid out at, to detect a resize.
    size: (u32, u32),
}

impl SmolwebDocument {
    /// Fetch `url` (a smolweb scheme) through `fetcher` and parse + theme it. `Err`
    /// when the fetch fails (the caller surfaces a load error).
    pub fn load(
        fetcher: &impl ResourceFetcher,
        url: &str,
        theme: SmolwebTheme,
    ) -> Result<Self, String> {
        let bytes = fetcher
            .fetch(url)
            .ok_or_else(|| format!("could not load {url}"))?;
        Ok(Self::parse(url, &String::from_utf8_lossy(&bytes), theme))
    }

    /// Parse already-fetched `body` for `url` (the fetch-free half, for tests).
    pub fn parse(url: &str, body: &str, theme: SmolwebTheme) -> Self {
        let content = detect(url, body);
        // Structural display defaults (div/p/h1/… block) under the themed sheet, the
        // same base the static document path layers its sheets over.
        let mut sheets: Vec<String> =
            crate::STRUCTURAL_SHEET.iter().map(|s| s.to_string()).collect();
        sheets.push(stylesheet(theme, url));
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = ServalAppRunner::new(dom, view as SmolwebLogic, SmolwebState { content });
        Self { runner, sheets, session: None, size: (0, 0) }
    }

    /// Build (or rebuild, on a size change) the retained layout session.
    fn ensure_session(&mut self, width: u32, height: u32) {
        if self.session.is_some() && self.size == (width, height) {
            return;
        }
        let sheets: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
        let session = {
            let dom = self.runner.dom();
            let dom = dom.borrow();
            // `&*dom`: `IncrementalLayout::new` is generic over `D: LayoutDom`.
            IncrementalLayout::new(&*dom, &sheets, width.max(1) as f32, height.max(1) as f32)
        };
        self.session = Some(session);
        self.size = (width, height);
    }

    /// Render the document to a [`Scene`] at `width`×`height`, at the current scroll.
    /// Rebuilds the session on a size change.
    pub fn frame(&mut self, width: u32, height: u32) -> Scene {
        self.ensure_session(width, height);
        let dom = self.runner.dom();
        let dom = dom.borrow();
        let session = self.session.as_ref().expect("session built by ensure_session");
        scene_from_session_dom(session, &*dom, width.max(1), height.max(1))
    }

    /// Scroll by a device-px wheel delta; returns whether the viewport moved (false at
    /// an edge or before the first frame).
    pub fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let dom = self.runner.dom();
        let dom = dom.borrow();
        let before = session.viewport_scroll();
        before != session.scroll_by(&*dom, dx, dy)
    }

    /// Scroll by a wheel delta at scene point `(x, y)`: a nested `overflow` container
    /// under the pointer takes it first, else the viewport. Returns whether anything
    /// moved. A no-op before the first frame.
    pub fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let dom = self.runner.dom();
        let dom = dom.borrow();
        session.scroll_at(&*dom, x, y, dx, dy)
    }

    /// Apply a keyboard scroll default (arrows / page / home-end); returns whether the
    /// viewport moved. A no-op before the first frame.
    pub fn scroll_for_key(&mut self, key: ScrollKey) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let dom = self.runner.dom();
        let dom = dom.borrow();
        session.scroll_for_key(&*dom, key)
    }

    /// Scroll to an absolute vertical offset (device px), clamped to the document's
    /// scroll range. A no-op before the first frame (the caller should [`frame`](Self::frame)
    /// once first, e.g. at `(width, height)`, before scrolling to a host-requested band).
    pub fn scroll_to(&mut self, y: f32) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let dom = self.runner.dom();
        let dom = dom.borrow();
        let x = session.viewport_scroll().0;
        session.set_viewport_scroll(&*dom, (x, y));
    }

    /// The document's full content height (px): the viewport height plus its maximum
    /// downward scroll ([`serval_layout::IncrementalLayout::scroll_range`]) at
    /// `width`×`height`. Builds (or reuses) the session at that size. A host uses this
    /// to decide whether the capsule needs scroll banding at all.
    pub fn content_height(&mut self, width: u32, height: u32) -> u32 {
        self.ensure_session(width, height);
        let dom = self.runner.dom();
        let dom = dom.borrow();
        let session = self.session.as_ref().expect("session built by ensure_session");
        let (_, max_y) = session.scroll_range(&*dom);
        height.max(1) + max_y.round() as u32
    }

    /// Every link's hit rect(s) + href, in full-document px (unscrolled) — see
    /// [`serval_layout::IncrementalLayout::link_rects`]. A host that composites this
    /// document's flat scene (rather than querying a retained packet) ships this
    /// alongside the scene so a click resolves via a cached rect table, the same
    /// mechanism the HTML/serval lane uses. Empty before the first frame.
    pub fn links(&self) -> Vec<(String, [f32; 4])> {
        let Some(session) = self.session.as_ref() else {
            return Vec::new();
        };
        let dom = self.runner.dom();
        let dom = dom.borrow();
        session.link_rects(&*dom)
    }

    /// Resolve a click at scene-local `(x, y)` to a navigation target, if it landed on
    /// a link. Uses the current layout (built at `width`×`height` if needed), then
    /// dispatches the click and returns the first navigation its handlers emitted.
    pub fn click_at(&mut self, x: f32, y: f32, width: u32, height: u32) -> Option<String> {
        self.ensure_session(width, height);
        let node = {
            let session = self.session.as_ref()?;
            let dom = self.runner.dom();
            let dom = dom.borrow();
            session.hit_test(&*dom, x, y, &ScrollOffsets::default())?
        };
        let actions = self.runner.dispatch_click(node, PointerClick::at((x, y)));
        actions.into_iter().next().map(|Navigate(url)| url)
    }

    /// The document DOM handle (for the host's hit-testing / inspection).
    pub fn dom(&self) -> DomHandle {
        self.runner.dom()
    }
}

/// Choose a parser by scheme, with a feed sniff for the gemini family (an XML body
/// served over gemini is a feed; otherwise gemtext).
fn detect(url: &str, body: &str) -> Content {
    let scheme = url.split_once("://").map(|(s, _)| s).unwrap_or("");
    match scheme {
        "gopher" => Content::Gopher(gopher::parse(body)),
        _ if looks_like_feed(body) => match feed::parse(body) {
            Ok(parsed) => Content::Feed(parsed),
            // A malformed "feed" falls back to gemtext rather than erroring.
            Err(_) => Content::Gemtext(gemtext::parse(body)),
        },
        // gemini / spartan / guppy / nex / finger and anything else: gemtext.
        _ => Content::Gemtext(gemtext::parse(body)),
    }
}

fn looks_like_feed(body: &str) -> bool {
    let trimmed = body.trim_start();
    trimmed.starts_with("<?xml") || trimmed.starts_with("<rss") || trimmed.starts_with("<feed")
}

/// The smolweb document as windowed [`ViewerContent`](crate::static_viewer::windowed::ViewerContent),
/// so it plugs into the shared winit shell like the static document. v1 is read-only:
/// no scroll yet, and in-window link navigation is the chrome/tile lanes' job (the
/// bare viewer has no history), so a click is a no-op here.
#[cfg(feature = "viewer")]
impl crate::static_viewer::windowed::ViewerContent for SmolwebDocument {
    fn frame(&mut self, width: u32, height: u32) -> Scene {
        SmolwebDocument::frame(self, width, height)
    }
    fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
        SmolwebDocument::scroll_by(self, dx, dy)
    }
    fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        SmolwebDocument::scroll_at(self, x, y, dx, dy)
    }
    fn scroll_for_key(&mut self, key: serval_layout::ScrollKey) -> bool {
        SmolwebDocument::scroll_for_key(self, key)
    }
    fn click_at(&mut self, _x: f32, _y: f32) -> bool {
        // The bare viewer has no history; navigation is the chrome browser's job
        // (see the `BrowsableContent` impl below), so a click is a no-op here.
        false
    }
}

/// The smolweb document as [`BrowsableContent`](crate::chrome_viewer::windowed::BrowsableContent),
/// so it hosts in the shared chrome browser (omnibar + back/forward + navigation), the
/// same shell the HTML viewer uses. A link click resolves to its `on_navigate` URL,
/// which the shell loads.
#[cfg(all(feature = "viewer", feature = "chrome"))]
impl crate::chrome_viewer::windowed::BrowsableContent for SmolwebDocument {
    fn load(url: &str) -> Result<Self, String> {
        SmolwebDocument::load(&crate::document::LocalFetcher, url, SmolwebTheme::default())
    }
    fn frame(&mut self, width: u32, height: u32) -> Scene {
        SmolwebDocument::frame(self, width, height)
    }
    fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        SmolwebDocument::scroll_at(self, x, y, dx, dy)
    }
    fn scroll_for_key(&mut self, key: ScrollKey) -> bool {
        SmolwebDocument::scroll_for_key(self, key)
    }
    fn click_at(
        &mut self,
        x: f32,
        y: f32,
        width: u32,
        height: u32,
    ) -> crate::chrome_viewer::windowed::ContentClick {
        use crate::chrome_viewer::windowed::ContentClick;
        match SmolwebDocument::click_at(self, x, y, width, height) {
            Some(url) => ContentClick::Navigate(url),
            None => ContentClick::None,
        }
    }
}

/// Open a window and present the smolweb capsule at `config.url`, themed per-site by
/// default (the Lagrange look). The smolweb twin of
/// [`run_static_viewer`](crate::run_static_viewer); a bad URL fails fast before the
/// window opens.
#[cfg(feature = "viewer")]
pub fn run_smolweb_viewer(
    config: crate::StaticViewerConfig,
) -> Result<crate::StaticViewerOutcome, String> {
    let doc = SmolwebDocument::load(
        &crate::document::LocalFetcher,
        &config.url,
        SmolwebTheme::default(),
    )?;
    crate::static_viewer::run_headed_with(config, doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use layout_dom_api::LayoutDom;

    /// A gemtext capsule renders to a scene with painted text — the GPU-free render
    /// path produces glyphs (mirrors the chrome's render test).
    #[test]
    fn gemtext_renders_text() {
        let mut doc = SmolwebDocument::parse("gemini://x.test/", "# Hello\n\nWorld.\n", SmolwebTheme::Site);
        let scene = doc.frame(800, 600);
        assert!(
            scene.ops.iter().any(|op| matches!(op, netrender::SceneOp::GlyphRun(_))),
            "the capsule paints text",
        );
    }

    /// A long capsule scrolls: after a frame builds the session, a downward wheel
    /// delta moves the viewport (the retained-session scroll path).
    #[test]
    fn long_capsule_scrolls() {
        let body: String = (0..200).map(|i| format!("Line {i}\n")).collect();
        let mut doc = SmolwebDocument::parse("gemini://x.test/", &body, SmolwebTheme::Plain);
        let _ = doc.frame(400, 300); // build the session at a short viewport
        assert!(!doc.scroll_by(0.0, -50.0), "at the top, scrolling up clamps (no move)");
        assert!(doc.scroll_by(0.0, 240.0), "a capsule taller than the viewport scrolls down");
    }

    /// `content_height` reports more than the viewport for a tall capsule, and
    /// `scroll_to` jumps to an absolute offset (clamped to the scroll range) — the two
    /// primitives a host's band-scroll protocol needs.
    #[test]
    fn content_height_and_scroll_to() {
        let body: String = (0..200).map(|i| format!("Line {i}\n")).collect();
        let mut doc = SmolwebDocument::parse("gemini://x.test/", &body, SmolwebTheme::Plain);
        let height = doc.content_height(400, 300);
        assert!(height > 300, "a 200-line capsule exceeds a 300px viewport: {height}");

        doc.scroll_to(1_000_000.0);
        let (_, y) = {
            let _ = doc.frame(400, 300);
            doc.session.as_ref().expect("framed").viewport_scroll()
        };
        assert!(y > 0.0 && y < 1_000_000.0, "scroll_to clamps to the scroll range: {y}");

        doc.scroll_to(0.0);
        let (_, y0) = {
            let _ = doc.frame(400, 300);
            doc.session.as_ref().expect("framed").viewport_scroll()
        };
        assert_eq!(y0, 0.0, "scroll_to(0) returns to the top");
    }

    /// `links()` is empty before the first frame, and after framing returns every link
    /// line's href + rect — the cached table a host resolves a click against.
    #[test]
    fn links_reports_hrefs_after_a_frame() {
        let mut doc = SmolwebDocument::parse(
            "gemini://x.test/",
            "=> gemini://x.test/a First\n=> gemini://x.test/b Second\n",
            SmolwebTheme::Plain,
        );
        assert!(doc.links().is_empty(), "no rects before the first frame");

        let _ = doc.frame(400, 300);
        let links = doc.links();
        let hrefs: Vec<&str> = links.iter().map(|(href, _)| href.as_str()).collect();
        assert!(hrefs.contains(&"gemini://x.test/a"));
        assert!(hrefs.contains(&"gemini://x.test/b"));
        for (_, rect) in &links {
            assert!(rect[2] > rect[0] && rect[3] > rect[1], "positive-area rect: {rect:?}");
        }
    }

    /// A short capsule's content height is just its own extent (no scroll needed), not
    /// artificially inflated to the viewport.
    #[test]
    fn short_capsule_content_height_is_not_inflated_past_viewport() {
        let mut doc = SmolwebDocument::parse("gemini://x.test/", "# Hi\n", SmolwebTheme::Plain);
        let height = doc.content_height(400, 300);
        assert_eq!(height, 300, "a short capsule's height floors at the viewport, no scroll range");
    }

    /// A gopher menu is detected by scheme and renders a typed item line.
    #[test]
    fn gopher_scheme_detected() {
        let mut doc = SmolwebDocument::parse(
            "gopher://x.test/",
            "1Files\t/files\tx.test\t70\r\n",
            SmolwebTheme::Plain,
        );
        let scene = doc.frame(800, 600);
        assert!(scene.ops.iter().any(|op| matches!(op, netrender::SceneOp::GlyphRun(_))));
    }

    /// An RSS body served over gemini is detected as a feed.
    #[test]
    fn feed_sniffed_from_xml_body() {
        let body = "<?xml version=\"1.0\"?><rss><channel><title>Log</title></channel></rss>";
        let mut doc = SmolwebDocument::parse("gemini://x.test/feed", body, SmolwebTheme::Dark);
        assert!(matches!(doc, SmolwebDocument { .. }));
        // The feed title paints.
        let scene = doc.frame(800, 600);
        assert!(scene.ops.iter().any(|op| matches!(op, netrender::SceneOp::GlyphRun(_))));
    }

    /// Clicking a gemtext link resolves to its navigation target — the `on_navigate`
    /// action bubbles out of `dispatch_click`. Finds the anchor node directly (no
    /// layout coords needed), the way the chrome's button test does.
    #[test]
    fn gemtext_link_click_resolves_to_url() {
        let mut doc = SmolwebDocument::parse(
            "gemini://x.test/",
            "=> gemini://x.test/page  A link\n",
            SmolwebTheme::Plain,
        );
        let anchor = find_anchor(&doc).expect("a link anchor in the DOM");
        let actions = doc.runner.dispatch_click(anchor, PointerClick::at((0.0, 0.0)));
        assert_eq!(
            actions.into_iter().next().map(|Navigate(url)| url).as_deref(),
            Some("gemini://x.test/page"),
        );
    }

    /// Walk the document DOM for the first `<a>` element.
    fn find_anchor(doc: &SmolwebDocument) -> Option<NodeId> {
        let dom = doc.dom();
        let dom = dom.borrow();
        let mut stack = vec![dom.document()];
        while let Some(node) = stack.pop() {
            if dom.element_name(node).is_some_and(|q| q.local.as_ref() == "a") {
                return Some(node);
            }
            for child in dom.dom_children(node) {
                stack.push(child);
            }
        }
        None
    }
}
