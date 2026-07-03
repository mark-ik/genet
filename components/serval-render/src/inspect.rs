/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Content introspection: a [`LayoutDom`] → a queryable [`ContentReport`].
//!
//! The structural read of a document's addressed content — the engine substrate the
//! Inspector renders (mere's content-devtools pane; pelt's `inspect tile`), and a stable
//! test oracle: assert against this semantic report rather than pixels, so a check
//! survives a theme change or a 1px nudge and doubles as a structure-regression guard.
//! The twin of [`a11y`](crate::a11y) (the same DOM, surfaced as an accesskit tree for
//! the OS); this is the same DOM as the *content model* a reader walks.
//!
//! Slice 1 (structure): title, an outline of role + name, outgoing links, headings.
//! Scripts, metadata, and the network slice (headers / cookies / trackers) hang off the
//! same surface later. See `docs/2026-06-13_content_inspection_scope.md`.

use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};

/// A structural report of a document's addressed content.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentReport {
    /// The `<title>` text, if any.
    pub title: Option<String>,
    /// The element outline (painted elements only; metadata tags are skipped), in
    /// document order.
    pub outline: Vec<OutlineEntry>,
    /// Outgoing `<a href>` targets, in document order.
    pub links: Vec<String>,
    /// Heading (`<h1>`..`<h6>`) text, in document order.
    pub headings: Vec<String>,
}

/// One element in the structural outline.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineEntry {
    /// Nesting depth among painted elements (the document root is depth 0).
    pub depth: usize,
    /// A coarse semantic role (`"link"`, `"heading"`, `"paragraph"`, …).
    pub role: &'static str,
    /// The element's accessible name — its direct text content, trimmed.
    pub name: String,
}

/// Produce a [`ContentReport`] for `dom`: a structural read of the addressed content.
pub fn content_report<D: LayoutDom>(dom: &D) -> ContentReport {
    let mut report = ContentReport::default();
    walk(dom, dom.document(), 0, &mut report);
    report
}

/// Tags that carry no painted content — kept out of the outline (but still walked, so a
/// `<title>` inside `<head>` is found).
fn is_metadata(tag: &str) -> bool {
    matches!(
        tag,
        "head" | "style" | "script" | "title" | "meta" | "link" | "base" | "html"
    )
}

fn walk<D: LayoutDom>(dom: &D, node: D::NodeId, depth: usize, report: &mut ContentReport) {
    let mut child_depth = depth;
    if let Some(tag) = dom.element_name(node).map(|q| q.local.as_ref().to_string()) {
        if !is_metadata(&tag) {
            report.outline.push(OutlineEntry {
                depth,
                role: role_of(&tag),
                name: direct_text(dom, node),
            });
            child_depth = depth + 1;
        }
        match tag.as_str() {
            "title" => {
                let text = direct_text(dom, node);
                if !text.is_empty() {
                    report.title = Some(text);
                }
            },
            "a" => {
                if let Some(href) =
                    dom.attribute(node, &Namespace::default(), &LocalName::from("href"))
                {
                    report.links.push(href.to_string());
                }
            },
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let text = direct_text(dom, node);
                if !text.is_empty() {
                    report.headings.push(text);
                }
            },
            _ => {},
        }
    }
    for child in dom.dom_children(node) {
        walk(dom, child, child_depth, report);
    }
}

/// A coarse semantic role per tag — the inspector's element-kind column, richer than
/// the OS a11y mapping (which need only distinguish focusable/landmark roles).
fn role_of(tag: &str) -> &'static str {
    match tag {
        "a" => "link",
        "button" => "button",
        "input" | "textarea" => "textbox",
        "p" => "paragraph",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "heading",
        "ul" | "ol" => "list",
        "li" => "listitem",
        "img" => "image",
        "label" => "label",
        "nav" => "navigation",
        "header" => "banner",
        "footer" => "contentinfo",
        "main" => "main",
        "section" | "article" => "region",
        _ => "group",
    }
}

/// The element's direct text-child content, trimmed (its accessible name).
fn direct_text<D: LayoutDom>(dom: &D, node: D::NodeId) -> String {
    let mut name = String::new();
    for child in dom.dom_children(node) {
        if dom.kind(child) == NodeKind::Text {
            if let Some(text) = dom.text(child) {
                name.push_str(text);
            }
        }
    }
    name.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serval_static_dom::StaticDocument;

    /// The report reads a document's structure: title, headings, links, and an outline
    /// with semantic roles — the substrate the Inspector renders and the tests assert.
    #[test]
    fn report_reads_structure() {
        let doc = StaticDocument::parse(
            "<title>Demo</title><h1>Heading</h1><p>a para</p>\
             <a href=\"/next\">link</a><a href=\"https://x.test/\">x</a>",
        );
        let report = content_report(&doc);
        assert_eq!(report.title.as_deref(), Some("Demo"));
        assert_eq!(report.headings, vec!["Heading"]);
        assert_eq!(report.links, vec!["/next", "https://x.test/"]);
        assert!(
            report
                .outline
                .iter()
                .any(|e| e.role == "heading" && e.name == "Heading"),
            "the heading is in the outline: {:?}",
            report.outline,
        );
        assert!(
            report.outline.iter().any(|e| e.role == "link"),
            "the links are in the outline"
        );
        assert!(
            report
                .outline
                .iter()
                .any(|e| e.role == "paragraph" && e.name == "a para")
        );
    }

    /// A bare document with no structure reports empties, not panics.
    #[test]
    fn empty_document_reports_empty() {
        let doc = StaticDocument::parse("<div></div>");
        let report = content_report(&doc);
        assert!(report.title.is_none());
        assert!(report.links.is_empty());
        assert!(report.headings.is_empty());
    }
}
