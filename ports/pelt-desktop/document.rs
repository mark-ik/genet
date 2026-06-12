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
use serval_layout::{NoImageLoader, ScrollOffsets, inline_stylesheets};
use serval_render::scene_from_layout_dom;
use serval_static_dom::StaticDocument;

/// Structural display defaults a minimal viewer layers over serval's UA cascade,
/// so a plain HTML document lays out as a stack of blocks rather than one inline
/// run, and document metadata stays unpainted. (V1; a fuller UA sheet is a
/// follow-up.)
const DEFAULT_SHEET: &[&str] = &[
    "html, body, div, p, h1, h2, h3, h4, h5, h6, ul, ol, li, dl, dt, dd, \
     section, article, header, footer, nav, main, aside, figure, figcaption, \
     blockquote, pre, table, thead, tbody, tr, hr, form, fieldset { display: block; }",
    "head, style, script, title, meta, link, base { display: none; }",
    "body { padding: 8px; }",
];

/// A local-scheme [`ResourceFetcher`]: `data:` decodes the inline payload,
/// `file://` (and a bare filesystem path) read from disk. `http(s)` is deferred to
/// a future `netfetch` feature -- V1 is local-first -- so it falls through to a
/// failed read and a clean `None`.
pub struct LocalFetcher;

impl ResourceFetcher for LocalFetcher {
    fn fetch(&self, url: &str) -> Option<Vec<u8>> {
        if let Some(rest) = url.strip_prefix("data:") {
            return decode_data_url(rest);
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

/// Decode a `data:` URL payload (everything after `data:`): split on the first
/// comma into the metadata and the data. A percent-encoded text payload decodes to
/// its bytes; `;base64` payloads are deferred (a follow-up adds base64).
fn decode_data_url(rest: &str) -> Option<Vec<u8>> {
    let (meta, data) = rest.split_once(',')?;
    if meta.ends_with(";base64") {
        return None; // base64 data: is a V1 follow-up.
    }
    Some(percent_encoding::percent_decode_str(data).collect())
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

/// A parsed static document plus its resolved author stylesheets, rendered to a
/// [`netrender::Scene`] on demand (the viewer re-renders on resize / scroll).
pub struct LoadedDocument {
    doc: StaticDocument,
    /// The structural UA defaults plus the document's own inline `<style>` sheets.
    sheets: Vec<String>,
}

impl LoadedDocument {
    /// Fetch `url` through `fetcher`, parse the bytes as HTML, and resolve its
    /// stylesheets. `Err` when the fetch fails (missing file, unsupported scheme).
    pub fn load(fetcher: &impl ResourceFetcher, url: &str) -> Result<Self, String> {
        let bytes = fetcher
            .fetch(url)
            .ok_or_else(|| format!("could not load {url}"))?;
        Ok(Self::parse(&String::from_utf8_lossy(&bytes)))
    }

    /// Parse already-loaded HTML (the fetch-free half, for tests and inline
    /// `data:` content), layering the document's inline sheets over the defaults.
    pub fn parse(html: &str) -> Self {
        let doc = StaticDocument::parse(html);
        let mut sheets: Vec<String> = DEFAULT_SHEET.iter().map(|s| s.to_string()).collect();
        sheets.extend(inline_stylesheets(&doc));
        Self { doc, sheets }
    }

    /// Render the document to a [`netrender::Scene`] at `width`×`height`. (V1
    /// paints with no images, `NoImageLoader`, and unscrolled; the present loop's
    /// scroll arrives in step 2.)
    pub fn scene(&self, width: u32, height: u32) -> Scene {
        let sheets: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
        scene_from_layout_dom(
            &self.doc,
            &sheets,
            &NoImageLoader,
            width,
            height,
            &ScrollOffsets::default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `data:` document loads, parses, and paints text (glyph runs in the
    /// scene) -- the whole load -> parse -> serval-render path, no window.
    #[test]
    fn data_url_loads_and_renders_text() {
        let doc = LoadedDocument::load(&LocalFetcher, "data:text/html,<h1>Hello</h1><p>World</p>")
            .expect("a data: URL loads");
        let scene = doc.scene(400, 300);
        assert!(
            scene.ops.iter().any(|op| matches!(op, netrender::SceneOp::GlyphRun(_))),
            "the rendered document paints text",
        );
    }

    /// A percent-encoded `data:` payload decodes before parsing.
    #[test]
    fn percent_encoded_data_url_decodes() {
        // "<h1>Hi</h1>" percent-encoded.
        let doc = LoadedDocument::load(&LocalFetcher, "data:text/html,%3Ch1%3EHi%3C%2Fh1%3E")
            .expect("a percent-encoded data: URL loads");
        assert!(!doc.scene(400, 300).ops.is_empty(), "the decoded document renders");
    }

    /// A bare filesystem path reads from disk (the primary CLI case).
    #[test]
    fn bare_path_reads_from_disk() {
        let dir = std::env::temp_dir().join("pelt-viewer-doc-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("doc.html");
        std::fs::write(&path, "<h1>From disk</h1>").expect("write temp html");
        let doc = LoadedDocument::load(&LocalFetcher, path.to_str().expect("utf8 path"))
            .expect("a bare path loads from disk");
        assert!(!doc.scene(400, 300).ops.is_empty(), "the on-disk document renders");
    }

    /// A missing file is a clean error, not a panic.
    #[test]
    fn missing_file_is_an_error() {
        assert!(
            LoadedDocument::load(&LocalFetcher, "/no/such/pelt/file.html").is_err(),
            "a missing file surfaces as Err",
        );
    }

    /// base64 `data:` is deferred in V1: it does not pretend to decode.
    #[test]
    fn base64_data_url_is_deferred() {
        assert!(
            LocalFetcher.fetch("data:text/html;base64,PGgxPkhpPC9oMT4=").is_none(),
            "base64 data: is a V1 follow-up (None for now)",
        );
    }
}
