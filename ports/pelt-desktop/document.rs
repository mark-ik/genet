/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Document loading + scene production for the static viewer (V1).
//!
//! The non-windowing half of `pelt --engine static <url>`: a
//! [`ResourceFetcher`](pelt_core::ResourceFetcher) for local schemes, and a parsed
//! [`LoadedDocument`] that renders to a [`netrender::Scene`] through `serval-render`.
//! GPU-free and testable; the windowed present loop (`static_viewer`) drives it.

use netrender::Scene;
use pelt_core::ResourceFetcher;
use serval_layout::{inline_stylesheets, IncrementalLayout, ScrollKey, ScrollOffsets};
use serval_render::{content_report, scene_from_session_dom, ContentReport};
use serval_static_dom::{StaticDocument, StaticNodeId};

/// A local-scheme [`ResourceFetcher`]: `data:` decodes the inline payload,
/// `file://` (and a bare filesystem path) read from disk. `http(s)` is deferred to
/// a future `netfetch` feature -- V1 is local-first -- so it falls through to a
/// failed read and a clean `None`.
pub struct LocalFetcher;

impl ResourceFetcher for LocalFetcher {
    fn fetch(&self, url: &str) -> Option<Vec<u8>> {
        if url.starts_with("data:") {
            // The spec `data:` parser (the same one `serval-layout` decodes inline
            // `<img>` payloads with): handles percent-encoded *and* `;base64` bodies,
            // the charset / mime header, and the optional fragment.
            let parsed = data_url::DataUrl::process(url).ok()?;
            return parsed.decode_to_vec().ok().map(|(bytes, _fragment)| bytes);
        }
        if let Some(rest) = url.strip_prefix("file://") {
            return std::fs::read(file_url_to_path(rest)).ok();
        }
        // Anything else is treated as a filesystem path: the bare-path CLI case
        // (`pelt --engine static doc.html`) and a Windows drive path (`C:\x`) a
        // scheme check would misread. `http(s)` has no V1 fetcher, so it lands
        // here and fails to `None`.
        std::fs::read(url).ok()
    }
}

/// Map the part after `file://` to a filesystem path: drop an empty / `localhost`
/// authority, and on Windows turn the `/C:/…` form back into `C:/…`.
fn file_url_to_path(after_scheme: &str) -> String {
    let path = match after_scheme.split_once('/') {
        Some((auth, rest)) if auth.is_empty() || auth.eq_ignore_ascii_case("localhost") => {
            format!("/{rest}")
        }
        _ => after_scheme.to_string(),
    };
    #[cfg(windows)]
    if let Some(rest) = path.strip_prefix('/') {
        if rest.as_bytes().get(1) == Some(&b':') {
            return rest.to_string();
        }
    }
    path
}

/// A parsed static document plus its resolved author stylesheets, rendered through
/// a retained layout session that owns the document viewport. The viewer lays out
/// once per size (rebuilding on resize) and re-emits per scroll — the render-first
/// path — so wheel scrolling never re-runs layout.
pub struct LoadedDocument {
    doc: StaticDocument,
    /// The structural UA defaults plus the document's own inline `<style>` sheets.
    sheets: Vec<String>,
    /// The retained cascade + layout session, owner of the document viewport (size
    /// + propagated overflow + scroll). Built lazily at the first render size and
    /// rebuilt on a resize (which re-resolves `%`-height and viewport units);
    /// `None` before the first frame.
    session: Option<IncrementalLayout<StaticNodeId>>,
    /// The size `session` was laid out at, to detect a resize.
    size: (u32, u32),
    /// A `url#id` fragment to scroll to once, applied on the first frame after the
    /// session exists (anchor-fragment navigation on load). Cleared after applying.
    pending_fragment: Option<String>,
}

/// What a content click ([`LoadedDocument::click_at`]) resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClickOutcome {
    /// Nothing actionable under the point.
    None,
    /// An in-page `#fragment` link; the document scrolled to its target (host redraws).
    Scrolled,
    /// A link to another resource; the host resolves the href against the current URL
    /// (see [`resolve_href`]) and loads it.
    Navigate(String),
}

impl LoadedDocument {
    /// Fetch `url` through `fetcher`, parse the bytes as HTML, and resolve its
    /// stylesheets. `Err` when the fetch fails (missing file, unsupported scheme).
    pub fn load(fetcher: &impl ResourceFetcher, url: &str) -> Result<Self, String> {
        // Split a `url#id` fragment off before fetching (the fetcher takes the
        // resource, not the fragment); a non-empty fragment scrolls into view on the
        // first frame (anchor-fragment navigation on load).
        let (resource, fragment) = match url.split_once('#') {
            Some((res, frag)) => (res, (!frag.is_empty()).then(|| frag.to_string())),
            None => (url, None),
        };
        let bytes = fetcher
            .fetch(resource)
            .ok_or_else(|| format!("could not load {resource}"))?;
        let mut me = Self::parse(&String::from_utf8_lossy(&bytes));
        me.pending_fragment = fragment;
        Ok(me)
    }

    /// Parse already-loaded HTML (the fetch-free half, for tests and inline
    /// `data:` content), layering the document's inline sheets over the defaults.
    pub fn parse(html: &str) -> Self {
        let doc = StaticDocument::parse(html);
        let mut sheets: Vec<String> =
            crate::STRUCTURAL_SHEET.iter().map(|s| s.to_string()).collect();
        sheets.extend(inline_stylesheets(&doc));
        Self { doc, sheets, session: None, size: (0, 0), pending_fragment: None }
    }

    /// Build (or rebuild, on a size change) the layout session for `width`×`height`.
    fn ensure_session(&mut self, width: u32, height: u32) {
        if self.session.is_some() && self.size == (width, height) {
            return;
        }
        let sheets: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
        self.session = Some(IncrementalLayout::new(&self.doc, &sheets, width as f32, height as f32));
        self.size = (width, height);
    }

    /// Render the document to a [`netrender::Scene`] at `width`×`height`, painting
    /// at the current document scroll. Rebuilds the layout session on a size change
    /// (re-resolving `%`-height and viewport units against the new viewport).
    pub fn frame(&mut self, width: u32, height: u32) -> Scene {
        self.ensure_session(width, height);
        // One-shot anchor-fragment scroll: now that the session / layout exists, bring
        // a `url#id` target into view so the document opens scrolled to it.
        if let Some(fragment) = self.pending_fragment.take() {
            if let Some(session) = self.session.as_mut() {
                session.scroll_to_id(&self.doc, &fragment);
            }
        }
        let session = self.session.as_ref().expect("session built by ensure_session");
        scene_from_session_dom(session, &self.doc, width, height)
    }

    /// Scroll the document by a device-px wheel delta, clamped to the
    /// scrollable-overflow range and the propagated overflow (a short page, or
    /// `overflow: hidden` on the root, does not scroll). Returns whether the offset
    /// changed, so the host can skip a redraw at an edge. A no-op before the first
    /// [`frame`](Self::frame) builds the session.
    pub fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let before = session.viewport_scroll();
        let after = session.scroll_by(&self.doc, dx, dy);
        before != after
    }

    /// Scroll by a device-px wheel delta at scene point `(x, y)`: the wheel routes to
    /// the nearest `overflow: scroll/auto` container under the pointer (CSS scroll
    /// chaining), falling through to the document viewport when none takes it — the
    /// position-aware wheel default action ([`IncrementalLayout::scroll_at`]). Returns
    /// whether anything moved (an inner scroller or the viewport), so the host can skip
    /// a redraw at an edge. A no-op before the first [`frame`](Self::frame). The
    /// superset of [`scroll_by`](Self::scroll_by): a document with no nested scroller
    /// behaves identically (the viewport takes every delta).
    pub fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        session.scroll_at(&self.doc, x, y, dx, dy)
    }

    /// Apply a keyboard scroll default action ([`ScrollKey`]) to the document
    /// viewport (clamped). Returns whether the offset moved, so the host can skip a
    /// redraw at an edge. A no-op before the first [`frame`](Self::frame).
    pub fn scroll_for_key(&mut self, key: ScrollKey) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        session.scroll_for_key(&self.doc, key)
    }

    /// Handle a click at scene point `(x, y)`. An in-page `<a href="#id">` scrolls its
    /// target into view ([`ClickOutcome::Scrolled`]); an `<a>` to another resource is
    /// reported as a [`ClickOutcome::Navigate`] for the host to resolve + load;
    /// elsewhere it is [`ClickOutcome::None`]. A no-op before the first frame.
    pub fn click_at(&mut self, x: f32, y: f32) -> ClickOutcome {
        let href = {
            let Some(session) = self.session.as_ref() else {
                return ClickOutcome::None;
            };
            session.link_href_at(&self.doc, x, y, &ScrollOffsets::default())
        };
        let Some(href) = href else {
            return ClickOutcome::None;
        };
        // An in-page `#fragment` scrolls within this document; any other href is a
        // navigation the host resolves against the current URL and loads.
        if let Some(fragment) = href.strip_prefix('#').filter(|f| !f.is_empty()) {
            let fragment = fragment.to_string();
            if let Some(session) = self.session.as_mut() {
                session.scroll_to_id(&self.doc, &fragment);
            }
            return ClickOutcome::Scrolled;
        }
        ClickOutcome::Navigate(href)
    }

    /// The current document scroll offset in device px (`(0, 0)` before the first
    /// frame).
    pub fn scroll(&self) -> (f32, f32) {
        self.session.as_ref().map_or((0.0, 0.0), |s| s.viewport_scroll())
    }

    /// A structural [`ContentReport`] of this document's addressed content (title,
    /// outline, links, headings) — the inspector's read model + the semantic oracle.
    pub fn inspect(&self) -> ContentReport {
        content_report(&self.doc)
    }
}

/// Resolve a link `href` against the `base` URL the document was loaded from. Absolute
/// hrefs (a scheme like `https:` / `data:`, a Windows drive, or a root path) pass
/// through; a relative href joins onto the base's directory (everything up to its last
/// `/` or `\`). Pragmatic local-first resolution, not the full URL algorithm.
pub fn resolve_href(base: &str, href: &str) -> String {
    if has_scheme(href) || href.starts_with('/') || href.starts_with('\\') {
        return href.to_string();
    }
    let cut = base.rfind(['/', '\\']).map_or(0, |i| i + 1);
    format!("{}{}", &base[..cut], href)
}

/// Whether `url` begins with a URL scheme (`name:`) or a Windows drive (`C:`). A bare
/// relative path (`page.html`, `sub/page.html`) has neither.
fn has_scheme(url: &str) -> bool {
    match url.find(':') {
        Some(i) if i > 0 => {
            url[..i].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `data:` document loads, parses, and paints text (glyph runs in the
    /// scene) -- the whole load -> parse -> serval-render path, no window.
    #[test]
    fn data_url_loads_and_renders_text() {
        let mut doc =
            LoadedDocument::load(&LocalFetcher, "data:text/html,<h1>Hello</h1><p>World</p>")
                .expect("a data: URL loads");
        let scene = doc.frame(400, 300);
        assert!(
            scene.ops.iter().any(|op| matches!(op, netrender::SceneOp::GlyphRun(_))),
            "the rendered document paints text",
        );
    }

    /// A percent-encoded `data:` payload decodes before parsing.
    #[test]
    fn percent_encoded_data_url_decodes() {
        // "<h1>Hi</h1>" percent-encoded.
        let mut doc = LoadedDocument::load(&LocalFetcher, "data:text/html,%3Ch1%3EHi%3C%2Fh1%3E")
            .expect("a percent-encoded data: URL loads");
        assert!(!doc.frame(400, 300).ops.is_empty(), "the decoded document renders");
    }

    /// A `;base64` `data:` payload decodes before parsing (the spec parser handles the
    /// base64 body the hand-rolled splitter used to reject).
    #[test]
    fn base64_data_url_decodes() {
        // base64("<h1>Hi</h1>") = PGgxPkhpPC9oMT4=
        let mut doc =
            LoadedDocument::load(&LocalFetcher, "data:text/html;base64,PGgxPkhpPC9oMT4=")
                .expect("a base64 data: URL loads");
        let scene = doc.frame(400, 300);
        assert!(
            scene.ops.iter().any(|op| matches!(op, netrender::SceneOp::GlyphRun(_))),
            "the base64-decoded document paints its text",
        );
    }

    /// A bare filesystem path reads from disk (the primary CLI case).
    #[test]
    fn bare_path_reads_from_disk() {
        let dir = std::env::temp_dir().join("pelt-viewer-doc-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("doc.html");
        std::fs::write(&path, "<h1>From disk</h1>").expect("write temp html");
        let mut doc = LoadedDocument::load(&LocalFetcher, path.to_str().expect("utf8 path"))
            .expect("a bare path loads from disk");
        assert!(!doc.frame(400, 300).ops.is_empty(), "the on-disk document renders");
    }

    /// A document taller than the viewport scrolls: the offset advances on a wheel
    /// delta and clamps at the bottom edge (the session owns the viewport, so
    /// `scroll_by` routes through `IncrementalLayout` + `Viewport::clamp_scroll`).
    #[test]
    fn tall_document_scrolls_and_clamps() {
        let mut doc = LoadedDocument::parse(
            "<style>.tall { height: 2000px; }</style><div class=\"tall\">tall</div>",
        );
        // The first frame builds the session at 400×300.
        let _ = doc.frame(400, 300);
        assert_eq!(doc.scroll(), (0.0, 0.0), "starts at the top");

        assert!(doc.scroll_by(0.0, 250.0), "scrolling a tall document moves the offset");
        assert!((doc.scroll().1 - 250.0).abs() < 0.5, "offset advanced by 250: {:?}", doc.scroll());

        // Jump past the bottom: the offset clamps, and a further scroll is a no-op.
        let _ = doc.scroll_by(0.0, 100_000.0);
        let bottom = doc.scroll().1;
        assert!(bottom > 250.0, "scrolled near the bottom: {bottom}");
        assert!(!doc.scroll_by(0.0, 100.0), "already at the bottom edge → no change");
    }

    /// The wheel at a point scrolls a nested `overflow: scroll` container under the
    /// pointer (CSS scroll chaining), where a plain viewport wheel can't: a short page
    /// whose only scrollable content is a nested 100px scroller over 500px of content
    /// has no document-scroll headroom, so `scroll_by` is a no-op, but `scroll_at` over
    /// the scroller moves it — with the document viewport itself never moving. Proves
    /// the wheel → `IncrementalLayout::scroll_at` wiring end to end through the host.
    #[test]
    fn wheel_at_a_point_scrolls_a_nested_overflow_container() {
        let html = "<style>body{margin:0;padding:0} \
            .scroller{overflow:scroll;width:200px;height:100px} .inner{height:500px}</style>\
            <div class=\"scroller\"><div class=\"inner\">inner</div></div>";
        let mut doc = LoadedDocument::parse(html);
        let _ = doc.frame(400, 300);

        // The page fits the 300px viewport (the scroller is only 100px tall), so a
        // plain viewport wheel finds no headroom.
        assert!(!doc.scroll_by(0.0, 100.0), "the short page does not scroll its viewport");
        assert_eq!(doc.scroll(), (0.0, 0.0), "viewport stays at the top");

        // A wheel over the nested scroller (at scene point 50,50) scrolls IT, even
        // though the document viewport cannot move.
        assert!(
            doc.scroll_at(50.0, 50.0, 0.0, 100.0),
            "the wheel scrolls the nested container under the pointer",
        );
        assert_eq!(
            doc.scroll(),
            (0.0, 0.0),
            "the document viewport never moved — it was the inner container",
        );
    }

    /// A `url#id` fragment scrolls the target into view on the first frame: the
    /// document opens scrolled so the `#mark` element's top is at the viewport top.
    #[test]
    fn url_fragment_scrolls_into_view_on_load() {
        // A tall spacer, the target (id="mark"), then more height so the target's
        // top (1000px) sits within the scroll range. Body box zeroed so the target's
        // top is exactly 1000 (no UA padding offset).
        let html = "<style>body { margin: 0; padding: 0; } \
            .tall { height: 1000px; } .t { height: 60px; }</style>\
            <div class=\"tall\"></div><div id=\"mark\" class=\"t\">target</div>\
            <div class=\"tall\"></div>";
        let url = format!("data:text/html,{html}#mark");
        let mut doc = LoadedDocument::load(&LocalFetcher, &url).expect("loads with a fragment");
        let _ = doc.frame(400, 300);
        assert!(
            (doc.scroll().1 - 1000.0).abs() < 1.0,
            "opens scrolled to #mark at y=1000: {:?}",
            doc.scroll(),
        );
    }

    /// Clicking an in-page link (`<a href="#id">`) scrolls its target into view;
    /// a click that lands on no link is a no-op.
    #[test]
    fn in_page_link_click_scrolls_to_target() {
        let html = "<style>body { margin: 0; padding: 0; } a { display: block; height: 40px; } \
            .tall { height: 1000px; } .t { height: 60px; }</style>\
            <a href=\"#mark\">go</a><div class=\"tall\"></div>\
            <div id=\"mark\" class=\"t\">target</div><div class=\"tall\"></div>";
        let mut doc = LoadedDocument::parse(html);
        let _ = doc.frame(400, 300);

        // The link is a 40px block at the top; click inside it.
        assert_eq!(
            doc.click_at(10.0, 20.0),
            ClickOutcome::Scrolled,
            "clicking the in-page link scrolls to its target",
        );
        // #mark sits at y = 40 (link) + 1000 (spacer) = 1040.
        assert!((doc.scroll().1 - 1040.0).abs() < 1.0, "scrolled to #mark: {:?}", doc.scroll());

        // The point now shows the target (a div, not a link), so a click there is a
        // no-op.
        let before = doc.scroll();
        assert_eq!(doc.click_at(10.0, 20.0), ClickOutcome::None, "no link under the point now");
        assert_eq!(doc.scroll(), before, "scroll unchanged off a link");
    }

    /// Clicking an `<a>` to another resource reports a navigation (the host loads it),
    /// and does not scroll the current document.
    #[test]
    fn external_link_click_reports_navigation() {
        let html = "<style>body { margin: 0; padding: 0; } a { display: block; height: 40px; }</style>\
            <a href=\"next.html\">go</a>";
        let mut doc = LoadedDocument::parse(html);
        let _ = doc.frame(400, 300);
        assert_eq!(
            doc.click_at(10.0, 20.0),
            ClickOutcome::Navigate("next.html".to_string()),
            "an external link reports a navigation to its href",
        );
        assert_eq!(doc.scroll(), (0.0, 0.0), "a navigation does not scroll the current document");
    }

    /// `resolve_href` joins a relative link onto the base's directory and passes
    /// absolute hrefs (scheme / root path) through unchanged.
    #[test]
    fn resolve_href_joins_relative_and_passes_absolute() {
        assert_eq!(resolve_href("docs/a.html", "b.html"), "docs/b.html");
        assert_eq!(resolve_href("a.html", "sub/c.html"), "sub/c.html");
        assert_eq!(resolve_href("file:///x/a.html", "b.html"), "file:///x/b.html");
        assert_eq!(resolve_href("a.html", "https://example.org/p"), "https://example.org/p");
        assert_eq!(resolve_href("a.html", "data:text/html,<p>x</p>"), "data:text/html,<p>x</p>");
        assert_eq!(resolve_href("docs/a.html", "/root.html"), "/root.html");
    }

    /// Keyboard scroll defaults reach the document viewport through the session:
    /// `PageDown` advances a tall page, `Home` returns to the top.
    #[test]
    fn keyboard_scrolls_a_tall_document() {
        let mut doc = LoadedDocument::parse(
            "<style>.tall { height: 2000px; }</style><div class=\"tall\">tall</div>",
        );
        let _ = doc.frame(400, 300);
        assert!(doc.scroll_for_key(ScrollKey::PageDown), "PageDown scrolls a tall document");
        assert!(doc.scroll().1 > 0.0, "the offset advanced: {:?}", doc.scroll());
        assert!(doc.scroll_for_key(ScrollKey::Home), "Home returns to the top");
        assert_eq!(doc.scroll(), (0.0, 0.0));
    }

    /// A document with content shorter than the viewport does not scroll: the body
    /// is content-height (not viewport-stretched), so the UA `body { padding: 8px }`
    /// stays within the viewport-filling root. (Before the UA body-box fix this
    /// leaked ~16px of phantom scroll on every short page.)
    #[test]
    fn document_without_overflow_does_not_scroll() {
        let mut doc = LoadedDocument::parse("<div>short</div>");
        let _ = doc.frame(400, 300);
        assert!(!doc.scroll_by(0.0, 250.0), "a short page has no scroll headroom");
        assert_eq!(doc.scroll(), (0.0, 0.0));
    }

    /// A missing file is a clean error, not a panic.
    #[test]
    fn missing_file_is_an_error() {
        assert!(
            LoadedDocument::load(&LocalFetcher, "/no/such/pelt/file.html").is_err(),
            "a missing file surfaces as Err",
        );
    }

    /// A `LocalFetcher` `fetch` of an unreadable path is a clean `None` (the
    /// `http(s)`-without-netfetch case lands here too).
    #[test]
    fn missing_path_fetches_none() {
        assert!(
            LocalFetcher.fetch("definitely/not/a/real/file.html").is_none(),
            "an unreadable path (or an http URL with no netfetch) fetches None",
        );
    }
}
