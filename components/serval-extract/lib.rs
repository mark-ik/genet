/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The extraction lane: render-free content extraction over [`LayoutDom`].
//!
//! "We don't just want to render the web, we want to analyze it too." This crate
//! turns a parsed document into the structured content a crawler or the eidetic
//! browsing corpus wants — links, title, (soon) headings, main text, metadata —
//! with **no cascade, layout, or paint**. Its single dependency is the
//! profile-neutral [`layout_dom_api`], so the dep graph itself is the witness that
//! extraction pulls none of the render stack (the render ladder's witness
//! discipline, applied to the orthogonal extraction axis).
//!
//! Extraction is **not a lower render rung**: it is a different *output* (data, not
//! pixels) that can draw from any rung's DOM. The cheap path runs over a no-JS
//! [`serval_static_dom::StaticDocument`] (static-parse extract); the same functions
//! run over a script-mutated DOM for the post-JS / SPA case (headless-scripted-DOM
//! extract), since both are just `LayoutDom`s.
//!
//! All output is **unresolved and rect-free**: an `href` is the raw attribute value
//! (the caller owns the page URL and resolves it), and there is no geometry — this
//! is the counterpart to the layout-coupled `LinkHit` (`href` + rect), for code that
//! wants the link graph without laying the page out.

#![deny(unsafe_code)]

use layout_dom_api::{LayoutDom, LocalName, Namespace};

/// One extracted hyperlink — the rect-free counterpart to a laid-out `LinkHit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The raw `href` attribute value, **unresolved**: extraction owns no URL
    /// context, so the caller resolves it against the page URL.
    pub href: String,
    /// The anchor's visible text: its descendants' text, whitespace-collapsed.
    pub text: String,
    /// The `rel` token list, if present (`nofollow`, `noopener`, …). A crawler
    /// honours `nofollow` when building its frontier; extraction just reports it.
    pub rel: Option<String>,
}

/// A render-free extraction of a parsed document: the structured content a crawler
/// or the eidetic corpus wants, with no cascade / layout / paint. Grows field by
/// field as the extraction lane lands (headings, main text, and metadata are the
/// next slices); `Default` is the empty extract.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageExtract {
    /// The document `<title>` text, whitespace-collapsed, if present and non-empty.
    pub title: Option<String>,
    /// Every `<a href>` in document order — the crawl frontier's source.
    pub links: Vec<Link>,
}

/// Extract the structured content of `dom` without rendering it. The one-call entry
/// for the eidetic sink; the field functions below are the à-la-carte pieces.
pub fn extract<D: LayoutDom>(dom: &D) -> PageExtract {
    PageExtract {
        title: extract_title(dom),
        links: extract_links(dom),
    }
}

/// Every `<a href>` in the document, in document (pre-order) order. The **rect-free
/// anchor enumerator**: the link extractor for a crawl frontier, with no layout.
/// Anchors without an `href` (named anchors / placeholders) are skipped — they are
/// not navigable targets.
pub fn extract_links<D: LayoutDom>(dom: &D) -> Vec<Link> {
    let mut out = Vec::new();
    walk_links(dom, dom.document(), &mut out);
    out
}

fn walk_links<D: LayoutDom>(dom: &D, id: D::NodeId, out: &mut Vec<Link>) {
    if local_name(dom, id) == Some("a") {
        if let Some(href) = attr(dom, id, "href") {
            out.push(Link {
                href,
                text: text_of(dom, id),
                rel: attr(dom, id, "rel"),
            });
        }
    }
    for child in dom.dom_children(id) {
        walk_links(dom, child, out);
    }
}

/// The document `<title>` text (whitespace-collapsed), or `None` if absent/empty.
pub fn extract_title<D: LayoutDom>(dom: &D) -> Option<String> {
    let id = find_first(dom, dom.document(), "title")?;
    let text = text_of(dom, id);
    (!text.is_empty()).then_some(text)
}

/// The first element with local name `name` in pre-order, or `None`.
fn find_first<D: LayoutDom>(dom: &D, id: D::NodeId, name: &str) -> Option<D::NodeId> {
    if local_name(dom, id) == Some(name) {
        return Some(id);
    }
    for child in dom.dom_children(id) {
        if let Some(found) = find_first(dom, child, name) {
            return Some(found);
        }
    }
    None
}

// ---- small DOM helpers (rect-free, allocation-light) --------------------------

/// `id`'s element local name as `&str`, or `None` for non-elements.
fn local_name<D: LayoutDom>(dom: &D, id: D::NodeId) -> Option<&str> {
    dom.element_name(id).map(|q| q.local.as_ref())
}

/// A null-namespace attribute (`href`, `rel`, `id`, … — the HTML common case).
fn attr<D: LayoutDom>(dom: &D, id: D::NodeId, name: &str) -> Option<String> {
    dom.attribute(id, &Namespace::from(""), &LocalName::from(name))
        .map(str::to_string)
}

/// The whitespace-collapsed concatenation of all descendant text under `id` — an
/// element's "visible text" for extraction (script/style content is parsed as text
/// children, but anchors and titles do not contain them, so no filtering is needed
/// at this slice; a main-text extractor will skip `<script>`/`<style>`).
fn text_of<D: LayoutDom>(dom: &D, id: D::NodeId) -> String {
    let mut raw = String::new();
    collect_text(dom, id, &mut raw);
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn collect_text<D: LayoutDom>(dom: &D, id: D::NodeId, out: &mut String) {
    if let Some(t) = dom.text(id) {
        out.push_str(t);
        out.push(' '); // a separator so adjacent inline text nodes don't fuse
    }
    for child in dom.dom_children(id) {
        collect_text(dom, child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serval_static_dom::StaticDocument;

    #[test]
    fn extracts_anchors_with_text_and_rel() {
        let doc = StaticDocument::parse(
            "<html><body>\
                <a href=\"/one\">First</a>\
                <p>not a link</p>\
                <a href=\"https://example.com/two\" rel=\"nofollow\">Second <b>bold</b></a>\
                <a name=\"anchor\">no href, skipped</a>\
             </body></html>",
        );
        let links = extract_links(&doc);
        assert_eq!(links.len(), 2, "two href anchors; the named anchor is skipped: {links:?}");
        assert_eq!(links[0], Link { href: "/one".into(), text: "First".into(), rel: None });
        assert_eq!(
            links[1],
            Link {
                href: "https://example.com/two".into(),
                text: "Second bold".into(), // descendant text concatenated + collapsed
                rel: Some("nofollow".into()),
            },
        );
    }

    #[test]
    fn anchor_href_is_unresolved_raw_attribute() {
        // Extraction owns no URL context: the relative href comes back verbatim, for
        // the caller to resolve against the page URL.
        let doc = StaticDocument::parse("<body><a href=\"../sub/page.html\">x</a></body>");
        assert_eq!(extract_links(&doc)[0].href, "../sub/page.html");
    }

    #[test]
    fn extracts_the_title_collapsed() {
        let doc = StaticDocument::parse(
            "<html><head><title>  Hello   World  </title></head><body></body></html>",
        );
        assert_eq!(extract_title(&doc).as_deref(), Some("Hello World"));
    }

    #[test]
    fn no_title_is_none() {
        let doc = StaticDocument::parse("<body><p>no title here</p></body>");
        assert_eq!(extract_title(&doc), None);
    }

    #[test]
    fn extract_bundles_title_and_links() {
        let doc = StaticDocument::parse(
            "<html><head><title>T</title></head><body><a href=\"/a\">A</a></body></html>",
        );
        let page = extract(&doc);
        assert_eq!(page.title.as_deref(), Some("T"));
        assert_eq!(page.links.len(), 1);
        assert_eq!(page.links[0].href, "/a");
    }

    #[test]
    fn empty_document_extracts_nothing() {
        let doc = StaticDocument::parse("");
        assert_eq!(extract(&doc), PageExtract::default());
    }
}
