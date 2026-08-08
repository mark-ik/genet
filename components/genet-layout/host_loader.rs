/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Host-side resource adapter: turn a parsed document plus a base
//! directory into the author stylesheets and image bytes the engine
//! consumes. The core layout path stays filesystem-free (it takes
//! sheets as `&str` and an [`ImageLoader`] trait); this module is the
//! batteries-included adapter that real hosts (the live viewer, the WPT
//! runner) share so the resolution rules live in one place.

use std::path::PathBuf;

use layout_dom_api::{LayoutDom, LocalName, Namespace};

use crate::image_decode::ImageLoader;

/// Resolves a resource URL referenced by a document to a local file.
///
/// `base_dir` is the document's own directory; relative URLs resolve
/// against it. `tests_root` (when set) is the root a `/`-absolute URL
/// resolves against (the WPT corpus root); without it, `/`-absolute
/// URLs do not resolve. Remote (`http(s)://`, `//`) and `data:` URLs
/// always return `None`: the caller fetches or decodes those.
#[derive(Clone, Default)]
pub struct ResourceResolver {
    pub base_dir: Option<PathBuf>,
    pub tests_root: Option<PathBuf>,
}

impl ResourceResolver {
    /// A resolver rooted at a single document directory (the common
    /// case: a live page with no corpus root).
    pub fn at(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base_dir.into()),
            tests_root: None,
        }
    }

    /// The document's base URL as a `file://` string, for the cascade's
    /// relative-`url()` resolution (pass to [`run_cascade`](crate::run_cascade)'s
    /// `base_url`). Built from `base_dir` as a directory (trailing slash,
    /// so `url(support/x.png)` resolves against it). `None` when there is
    /// no `base_dir` or the path is not absolute (file URLs require one).
    pub fn base_url(&self) -> Option<String> {
        let base = self.base_dir.as_ref()?;
        // File URLs require an absolute path; the WPT runner roots tests
        // at a relative dir, so canonicalize first (falling back to the
        // path as-given if canonicalize fails, e.g. in tests).
        let abs = std::fs::canonicalize(base).unwrap_or_else(|_| base.clone());
        // `url`'s file-path API exists only on filesystem targets; on wasm there is
        // no local base (resources come from the host loader). (wasm de-IPC pass.)
        #[cfg(any(unix, windows, target_os = "redox", target_os = "wasi"))]
        {
            url::Url::from_directory_path(&abs)
                .ok()
                .map(|u| u.to_string())
        }
        #[cfg(not(any(unix, windows, target_os = "redox", target_os = "wasi")))]
        {
            let _ = abs;
            None
        }
    }

    /// Resolve `url` to a path, or `None` if it is remote, a `data:`
    /// URI, empty, or unresolvable under this resolver's roots.
    ///
    /// A `file://` URL (which Stylo produces when it resolves a relative
    /// CSS `url()` against the document's `file://` base) maps straight
    /// to its local path. Relative URLs resolve against `base_dir`,
    /// `/`-absolute against `tests_root`.
    pub fn resolve(&self, url: &str) -> Option<PathBuf> {
        let url = url.split(['#', '?']).next().unwrap_or(url).trim();
        if url.is_empty()
            || url.starts_with("http://")
            || url.starts_with("https://")
            || url.starts_with("//")
            || url.starts_with("data:")
        {
            return None;
        }
        // `file://` URL: Stylo already resolved a relative CSS `url()`
        // against the document base into an absolute file URL. Convert
        // it back to a local path directly.
        if url.starts_with("file:") {
            #[cfg(any(unix, windows, target_os = "redox", target_os = "wasi"))]
            {
                return url::Url::parse(url)
                    .ok()
                    .and_then(|u| u.to_file_path().ok());
            }
            #[cfg(not(any(unix, windows, target_os = "redox", target_os = "wasi")))]
            {
                return None;
            }
        }
        match url.strip_prefix('/') {
            Some(rest) => self.tests_root.as_ref().map(|root| root.join(rest)),
            None => self.base_dir.as_ref().map(|base| base.join(url)),
        }
    }
}

/// An [`ImageLoader`] that reads `<img>` / `background-image` files from
/// disk through a [`ResourceResolver`]. Remote URLs yield `None`
/// (rendered as missing); `data:` URIs are decoded upstream and never
/// reach the loader.
pub struct LocalFileImageLoader {
    pub resolver: ResourceResolver,
}

impl LocalFileImageLoader {
    pub fn new(resolver: ResourceResolver) -> Self {
        Self { resolver }
    }
}

impl ImageLoader for LocalFileImageLoader {
    fn load(&self, url: &str) -> Option<Vec<u8>> {
        std::fs::read(self.resolver.resolve(url)?).ok()
    }
}

/// Text of every `<style>` element in document order: the document's
/// inline author stylesheets. Walks the parsed DOM, so it sees only
/// well-formed style blocks (preferred for the render path).
pub fn inline_stylesheets<D: LayoutDom>(dom: &D) -> Vec<String> {
    let mut sheets = Vec::new();
    collect_inline_stylesheets(dom, dom.document(), &mut sheets);
    sheets
}

fn collect_inline_stylesheets<D: LayoutDom>(dom: &D, id: D::NodeId, sheets: &mut Vec<String>) {
    if dom
        .element_name(id)
        .is_some_and(|q| q.local.as_ref() == "style")
    {
        let mut css = String::new();
        for child in dom.dom_children(id) {
            if let Some(text) = dom.text(child) {
                css.push_str(text);
            }
        }
        if !css.trim().is_empty() {
            sheets.push(css);
        }
    }
    for child in dom.dom_children(id) {
        collect_inline_stylesheets(dom, child, sheets);
    }
}

/// Text of every `<style>` block by scanning the raw source. Robust to
/// parse failures (does not require a successfully built DOM), so it is
/// preferred for crash-smoke over [`inline_stylesheets`].
pub fn inline_stylesheets_from_source(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(open) = lower[from..].find("<style") {
        let open = from + open;
        let Some(gt) = lower[open..].find('>') else {
            break;
        };
        let content_start = open + gt + 1;
        let Some(close_rel) = lower[content_start..].find("</style>") else {
            break;
        };
        let close = content_start + close_rel;
        out.push(html[content_start..close].to_string());
        from = close + "</style>".len();
    }
    out
}

/// Contents of each `<link rel="stylesheet" href>` whose `href`
/// resolves to a readable local file through `resolver`. Remote and
/// unresolvable hrefs are skipped silently. Convenience wrapper over
/// [`linked_stylesheets_with_loader`] with a local-filesystem loader; use the
/// loader form to also pull remote sheets through a host fetcher.
pub fn linked_stylesheets<D: LayoutDom>(dom: &D, resolver: &ResourceResolver) -> Vec<String> {
    linked_stylesheets_with_loader(dom, &LocalFileImageLoader::new(resolver.clone()))
}

/// Contents of each `<link rel="stylesheet" href>` whose `href` the `loader`
/// supplies bytes for. The [`ImageLoader`] is the host's general resource-bytes
/// seam (despite the name): a loader that fetches remote URLs — e.g. the viewer's
/// netfetcher-backed loader — makes external stylesheets load, while a
/// local-only loader keeps the filesystem behavior of [`linked_stylesheets`].
/// Bytes are decoded as UTF-8; non-UTF-8 or unavailable hrefs are skipped.
pub fn linked_stylesheets_with_loader<D, L>(dom: &D, loader: &L) -> Vec<String>
where
    D: LayoutDom,
    L: ImageLoader,
{
    let mut sheets = Vec::new();
    collect_linked_stylesheets(dom, dom.document(), loader, &mut sheets, false);
    sheets
}

/// Collect author stylesheets in document order. Inline `<style>` and linked
/// `<link rel=stylesheet>` nodes share the same walk, so the CSS vector keeps
/// the order-of-appearance cascade tie-breaker. A linked sheet's `media`
/// attribute is preserved by wrapping its source in an `@media` rule; Stylo
/// then evaluates it against the screen device instead of accidentally applying
/// a print sheet to every document.
///
/// `loader` receives each raw `href`. Hosts that have a document base should
/// resolve it before fetching (and do the same for image resources).
pub fn author_stylesheets_with_loader<D, L>(dom: &D, loader: &L) -> Vec<String>
where
    D: LayoutDom,
    L: ImageLoader,
{
    let mut sheets = Vec::new();
    collect_author_stylesheets(dom, dom.document(), loader, &mut sheets);
    sheets
}

fn collect_linked_stylesheets<D, L>(
    dom: &D,
    id: D::NodeId,
    loader: &L,
    sheets: &mut Vec<String>,
    wrap_media: bool,
) where
    D: LayoutDom,
    L: ImageLoader,
{
    if let Some(css) = linked_stylesheet_at(dom, id, loader, wrap_media) {
        sheets.push(css);
    }
    for child in dom.dom_children(id) {
        collect_linked_stylesheets(dom, child, loader, sheets, wrap_media);
    }
}

fn linked_stylesheet_at<D, L>(
    dom: &D,
    id: D::NodeId,
    loader: &L,
    wrap_media: bool,
) -> Option<String>
where
    D: LayoutDom,
    L: ImageLoader,
{
    let no_ns = Namespace::default();
    let rel_attr = LocalName::from("rel");
    let href_attr = LocalName::from("href");
    let media_attr = LocalName::from("media");

    let is_stylesheet = dom
        .element_name(id)
        .is_some_and(|q| q.local.as_ref() == "link")
        && dom.attribute(id, &no_ns, &rel_attr).is_some_and(|rel| {
            rel.split_ascii_whitespace()
                .any(|token| token.eq_ignore_ascii_case("stylesheet"))
        });
    if !is_stylesheet {
        return None;
    }
    let href = dom.attribute(id, &no_ns, &href_attr)?;
    let css = String::from_utf8(loader.load(href)?).ok()?;
    let media = dom
        .attribute(id, &no_ns, &media_attr)
        .filter(|media| !media.trim().is_empty());
    Some(match (wrap_media, media) {
        (true, Some(media)) => format!("@media {media} {{\n{css}\n}}"),
        _ => css,
    })
}

fn collect_author_stylesheets<D, L>(dom: &D, id: D::NodeId, loader: &L, sheets: &mut Vec<String>)
where
    D: LayoutDom,
    L: ImageLoader,
{
    if dom
        .element_name(id)
        .is_some_and(|q| q.local.as_ref() == "style")
    {
        let mut css = String::new();
        for child in dom.dom_children(id) {
            if let Some(text) = dom.text(child) {
                css.push_str(text);
            }
        }
        if !css.trim().is_empty() {
            sheets.push(css);
        }
    }

    // Keep the link branch separate from the inline branch. A `<link>` never
    // has CSS text children, but processing the current node here means
    // stylesheet assembly follows the DOM exactly rather than doing two
    // order-losing walks.
    if let Some(css) = linked_stylesheet_at(dom, id, loader, true) {
        sheets.push(css);
    }
    for child in dom.dom_children(id) {
        collect_author_stylesheets(dom, child, loader, sheets);
    }
}

/// The `href` of the page's favicon `<link rel="icon">` (also `rel="shortcut icon"`
/// or any `rel` whose space-separated tokens include `icon`), raw and unresolved —
/// the caller joins it against the document base. `None` when the head declares no
/// icon link (the caller may fall back to `/favicon.ico`). A read-only DOM scan
/// mirroring [`linked_stylesheets_with_loader`]; it fetches nothing.
pub fn linked_icon_href<D>(dom: &D) -> Option<String>
where
    D: LayoutDom,
{
    let no_ns = Namespace::default();
    let rel_attr = LocalName::from("rel");
    let href_attr = LocalName::from("href");

    let mut stack = vec![dom.document()];
    while let Some(id) = stack.pop() {
        let is_icon = dom
            .element_name(id)
            .is_some_and(|q| q.local.as_ref() == "link")
            && dom.attribute(id, &no_ns, &rel_attr).is_some_and(|rel| {
                rel.split_ascii_whitespace()
                    .any(|t| t.eq_ignore_ascii_case("icon"))
            });
        if is_icon {
            if let Some(href) = dom.attribute(id, &no_ns, &href_attr) {
                if !href.trim().is_empty() {
                    return Some(href.to_string());
                }
            }
        }
        for child in dom.dom_children(id) {
            stack.push(child);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use genet_static_dom::StaticDocument;

    #[test]
    fn inline_style_blocks_are_extracted_from_dom() {
        let doc = StaticDocument::parse(
            "<html><head><style>p { color: red; }</style>\
             <style>h1 { color: blue; }</style></head><body></body></html>",
        );
        let sheets = inline_stylesheets(&doc);
        assert_eq!(sheets.len(), 2);
        let joined = sheets.join("\n");
        assert!(joined.contains("color: red"));
        assert!(joined.contains("color: blue"));
    }

    #[test]
    fn inline_style_blocks_are_extracted_from_source() {
        let sheets = inline_stylesheets_from_source(
            "<style>p { color: red; }</style><STYLE>h1{color:blue}</STYLE>",
        );
        assert_eq!(sheets.len(), 2);
        assert!(sheets[0].contains("color: red"));
        assert!(sheets[1].contains("color:blue"));
    }

    #[test]
    fn linked_stylesheets_resolve_against_base_dir() {
        let dir = std::env::temp_dir().join(format!("genet_host_loader_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("site.css"), "body { color: green; }").unwrap();

        let doc = StaticDocument::parse(
            "<html><head>\
             <link rel=\"stylesheet\" href=\"site.css\">\
             <link rel=\"icon\" href=\"favicon.ico\">\
             </head><body></body></html>",
        );

        let resolver = ResourceResolver::at(&dir);
        let sheets = linked_stylesheets(&doc, &resolver);
        assert_eq!(sheets.len(), 1);
        assert!(sheets[0].contains("color: green"));

        assert!(linked_stylesheets(&doc, &ResourceResolver::default()).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn linked_stylesheets_load_remote_through_loader() {
        use crate::image_decode::ImageLoader;

        // A loader standing in for a fetcher: it serves one remote sheet.
        struct FakeFetcher;
        impl ImageLoader for FakeFetcher {
            fn load(&self, url: &str) -> Option<Vec<u8>> {
                (url == "https://cdn.example/site.css").then(|| b"body { color: purple; }".to_vec())
            }
        }

        let doc = StaticDocument::parse(
            "<html><head>\
             <link rel=\"stylesheet\" href=\"https://cdn.example/site.css\">\
             <link rel=\"stylesheet\" href=\"https://cdn.example/missing.css\">\
             </head><body></body></html>",
        );

        // The remote sheet the loader serves comes through; the one it doesn't is skipped.
        let sheets = linked_stylesheets_with_loader(&doc, &FakeFetcher);
        assert_eq!(sheets.len(), 1);
        assert!(sheets[0].contains("color: purple"));
    }

    #[test]
    fn author_stylesheets_keep_document_order_and_wrap_link_media() {
        use crate::image_decode::ImageLoader;

        struct FakeFetcher;
        impl ImageLoader for FakeFetcher {
            fn load(&self, url: &str) -> Option<Vec<u8>> {
                match url {
                    "early.css" => Some(b"p { color: green; }".to_vec()),
                    "print.css" => Some(b"p { color: black; }".to_vec()),
                    _ => None,
                }
            }
        }

        let doc = StaticDocument::parse(
            "<html><head><style>p { color: red; }</style>\
             <link rel=\"preload stylesheet\" href=\"early.css\">\
             <link rel=\"stylesheet\" href=\"print.css\" media=\"print\">\
             <style>p { color: blue; }</style></head><body></body></html>",
        );
        let sheets = author_stylesheets_with_loader(&doc, &FakeFetcher);
        assert_eq!(sheets.len(), 4);
        assert!(sheets[0].contains("color: red"));
        assert!(sheets[1].contains("color: green"));
        assert!(sheets[2].starts_with("@media print"));
        assert!(sheets[2].contains("color: black"));
        assert!(sheets[3].contains("color: blue"));
    }

    #[test]
    fn resolver_rejects_remote_and_data_urls() {
        let r = ResourceResolver {
            base_dir: Some("/base".into()),
            tests_root: Some("/root".into()),
        };
        assert!(r.resolve("https://example.com/a.css").is_none());
        assert!(r.resolve("//cdn/a.css").is_none());
        assert!(r.resolve("data:text/css,body{}").is_none());
        assert!(r.resolve("").is_none());
        assert_eq!(r.resolve("a/b.css"), Some(PathBuf::from("/base/a/b.css")));
        assert_eq!(r.resolve("/x/y.css"), Some(PathBuf::from("/root/x/y.css")));
        assert_eq!(
            r.resolve("a.css#frag?q=1"),
            Some(PathBuf::from("/base/a.css"))
        );
    }
}
