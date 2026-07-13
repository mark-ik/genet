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
//! [`genet_static_dom::StaticDocument`] (static-parse extract); the same functions
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

/// One extracted heading: its level (`1`–`6` for `<h1>`–`<h6>`) and collapsed text.
/// The document outline — structure for the corpus and a summarization signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// The heading level, `1`–`6`.
    pub level: u8,
    /// The heading's visible text, whitespace-collapsed.
    pub text: String,
}

/// The document's self-description: the metadata a page declares about itself. All
/// values are **unresolved** (a `canonical` href is the raw attribute). `Default` is
/// "nothing declared".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    /// `<meta name="description">` content — the page's own summary.
    pub description: Option<String>,
    /// `<link rel="canonical" href>` — the canonical URL the page claims (raw).
    pub canonical: Option<String>,
    /// OpenGraph `<meta property="og:*">` pairs with the `og:` prefix stripped, in
    /// document order: `("title", …)`, `("description", …)`, `("image", …)`,
    /// `("site_name", …)`, `("type", …)`, `("url", …)`, and the long tail.
    pub open_graph: Vec<(String, String)>,
}

/// A render-free extraction of a parsed document: the structured content a crawler
/// or the eidetic corpus wants, with no cascade / layout / paint. Grows field by
/// field as the extraction lane lands (main text is the next slice); `Default` is
/// the empty extract.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageExtract {
    /// The document `<title>` text, whitespace-collapsed, if present and non-empty.
    pub title: Option<String>,
    /// The page's declared metadata (description / canonical / OpenGraph).
    pub metadata: Metadata,
    /// The `<h1>`–`<h6>` outline in document order.
    pub headings: Vec<Heading>,
    /// The page's full **visible text**, whitespace-collapsed (non-rendered
    /// subtrees — `<script>` / `<style>` / `<head>` / … — excluded). This is the
    /// indexing/corpus text (everything the reader could see, chrome included);
    /// for the article body alone, see [`main_text`](Self::main_text).
    pub text: String,
    /// The **reader-mode article body**: the main content block by a
    /// readability heuristic (semantic landmarks + paragraph density + class/id
    /// signals), with page chrome (nav / header / footer / aside) dropped. `None`
    /// when no contentful block stands out (a link list, an app shell). This is the
    /// per-page payload for a reader-mode crawl.
    pub main_text: Option<String>,
    /// Every `<a href>` in document order — the crawl frontier's source.
    pub links: Vec<Link>,
}

/// Extract the structured content of `dom` without rendering it. The one-call entry
/// for the eidetic sink; the field functions below are the à-la-carte pieces.
pub fn extract<D: LayoutDom>(dom: &D) -> PageExtract {
    PageExtract {
        title: extract_title(dom),
        metadata: extract_metadata(dom),
        headings: extract_headings(dom),
        text: extract_text(dom),
        main_text: extract_main_text(dom),
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

/// The `<h1>`–`<h6>` outline in document (pre-order) order, each with its level and
/// collapsed text. Empty headings are skipped (no text to outline).
pub fn extract_headings<D: LayoutDom>(dom: &D) -> Vec<Heading> {
    let mut out = Vec::new();
    walk_headings(dom, dom.document(), &mut out);
    out
}

fn walk_headings<D: LayoutDom>(dom: &D, id: D::NodeId, out: &mut Vec<Heading>) {
    if let Some(level) = local_name(dom, id).and_then(heading_level) {
        let text = text_of(dom, id);
        if !text.is_empty() {
            out.push(Heading { level, text });
        }
    }
    for child in dom.dom_children(id) {
        walk_headings(dom, child, out);
    }
}

/// `1`–`6` for `h1`–`h6`, else `None`.
fn heading_level(name: &str) -> Option<u8> {
    match name.as_bytes() {
        [b'h', d @ b'1'..=b'6'] => Some(d - b'0'),
        _ => None,
    }
}

/// The page's declared [`Metadata`]: `<meta name="description">`, the
/// `<link rel="canonical">` href, and OpenGraph `<meta property="og:*">` pairs.
/// Walks the whole tree (not just `<head>`) since pages place these loosely.
pub fn extract_metadata<D: LayoutDom>(dom: &D) -> Metadata {
    let mut md = Metadata::default();
    walk_metadata(dom, dom.document(), &mut md);
    md
}

fn walk_metadata<D: LayoutDom>(dom: &D, id: D::NodeId, md: &mut Metadata) {
    match local_name(dom, id) {
        Some("meta") => {
            // OpenGraph (`property="og:*"`) takes precedence over `name`; a `<meta>`
            // carries one or the other. Only the *first* description wins.
            if let Some(prop) = attr(dom, id, "property") {
                if let Some(key) = prop.strip_prefix("og:") {
                    if let Some(content) = attr(dom, id, "content") {
                        md.open_graph.push((key.to_string(), content));
                    }
                }
            } else if attr(dom, id, "name").as_deref() == Some("description") {
                if md.description.is_none() {
                    md.description = attr(dom, id, "content").filter(|c| !c.is_empty());
                }
            }
        },
        Some("link") => {
            if md.canonical.is_none() && rel_has(dom, id, "canonical") {
                md.canonical = attr(dom, id, "href").filter(|h| !h.is_empty());
            }
        },
        _ => {},
    }
    for child in dom.dom_children(id) {
        walk_metadata(dom, child, md);
    }
}

/// Whether `id`'s `rel` attribute contains the (space-separated, case-insensitive)
/// token `token` — `rel` is a token list (`"stylesheet preload"`, `"canonical"`).
fn rel_has<D: LayoutDom>(dom: &D, id: D::NodeId, token: &str) -> bool {
    attr(dom, id, "rel").is_some_and(|rel| {
        rel.split_whitespace()
            .any(|t| t.eq_ignore_ascii_case(token))
    })
}

/// The page's full **visible text**, whitespace-collapsed: every text node except
/// those under non-rendered elements (`<script>` / `<style>` / `<template>` /
/// `<noscript>` / the document `<head>`). The indexing/corpus text — deliberately
/// *not* a main-content heuristic (which would drop nav/footer chrome); that
/// readability pass is a later slice that can build on this.
pub fn extract_text<D: LayoutDom>(dom: &D) -> String {
    let mut out = String::new();
    collect_visible_text(dom, dom.document(), &mut out);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Names of subtrees that carry no visible page text and are skipped wholesale.
fn is_non_rendered(name: &str) -> bool {
    matches!(name, "script" | "style" | "template" | "noscript" | "head")
}

fn collect_visible_text<D: LayoutDom>(dom: &D, id: D::NodeId, out: &mut String) {
    if local_name(dom, id).is_some_and(is_non_rendered) {
        return; // skip the whole subtree
    }
    if let Some(t) = dom.text(id) {
        out.push_str(t);
        out.push(' '); // separator so adjacent inline runs don't fuse
    }
    for child in dom.dom_children(id) {
        collect_visible_text(dom, child, out);
    }
}

// ---- reader-mode / main-content extraction ------------------------------------
//
// A compact readability heuristic (technique borrowed from readability.js): a
// semantic `<main>` wins outright; otherwise score block containers by paragraph
// density and class/id signal and take the best. Then emit that block's text with
// chrome (nav / header / footer / aside) and non-rendered subtrees dropped. The
// per-page payload for a reader-mode crawl.

/// Positive class/id signals: an element whose `class`/`id` contains one of these is
/// likely the article body (readability.js's positive lexicon, trimmed).
const POSITIVE_HINTS: &[&str] = &[
    "article", "body", "content", "entry", "main", "page", "post", "text", "blog", "story",
    "column", "prose",
];

/// Negative class/id signals: chrome, boilerplate, furniture. An element matching one
/// is penalized as unlikely to be the article body.
const NEGATIVE_HINTS: &[&str] = &[
    "nav",
    "menu",
    "header",
    "footer",
    "sidebar",
    "comment",
    "ads",
    "banner",
    "sponsor",
    "social",
    "share",
    "related",
    "promo",
    "masthead",
    "widget",
    "byline",
    "breadcrumb",
];

/// The page's **reader-mode article body**: locate the main content block and return
/// its chrome-free text, whitespace-collapsed. `None` when nothing contentful stands
/// out (an app shell, a pure link list). The per-page payload for a reader-mode crawl.
pub fn extract_main_text<D: LayoutDom>(dom: &D) -> Option<String> {
    let root = main_content_root(dom)?;
    let text = chrome_free_text(dom, root);
    (!text.is_empty()).then_some(text)
}

/// The element most likely to hold the article body: a semantic `<main>` if present
/// (the modern, unambiguous answer), else the best-scoring candidate block (score
/// must be positive — a page with only chrome yields `None`).
fn main_content_root<D: LayoutDom>(dom: &D) -> Option<D::NodeId> {
    if let Some(main) = find_first(dom, dom.document(), "main") {
        return Some(main);
    }
    let mut best: Option<(i32, D::NodeId)> = None;
    score_candidates(dom, dom.document(), &mut best);
    best.filter(|(score, _)| *score > 0).map(|(_, id)| id)
}

/// Score every candidate block in the tree, tracking the maximum.
fn score_candidates<D: LayoutDom>(dom: &D, id: D::NodeId, best: &mut Option<(i32, D::NodeId)>) {
    if is_candidate_block(dom, id) {
        let score = score_block(dom, id);
        let better = match *best {
            Some((s, _)) => score > s,
            None => true,
        };
        if better {
            *best = Some((score, id));
        }
    }
    for child in dom.dom_children(id) {
        score_candidates(dom, child, best);
    }
}

/// The block-level containers that can be the article root.
fn is_candidate_block<D: LayoutDom>(dom: &D, id: D::NodeId) -> bool {
    matches!(
        local_name(dom, id),
        Some("div" | "section" | "article" | "td")
    )
}

/// A block's readability score: a tag bonus, the class/id signal, and paragraph
/// density (the dominant term — an article body is mostly paragraph text).
fn score_block<D: LayoutDom>(dom: &D, id: D::NodeId) -> i32 {
    let mut score = classid_signal(dom, id);
    score += match local_name(dom, id) {
        Some("article") => 25,
        Some("section") => 8,
        Some("td") => 3,
        _ => 0,
    };
    // Paragraph density in ~50-char units, capped so one giant block doesn't wholly
    // swamp the class/tag signal.
    score += (paragraph_text_len(dom, id) / 50).min(50) as i32;
    score
}

/// Sum of descendant `<p>` text lengths under `id` (tiny paragraphs ignored — UI
/// labels, not prose).
fn paragraph_text_len<D: LayoutDom>(dom: &D, id: D::NodeId) -> usize {
    let mut total = 0;
    walk_paragraph_len(dom, id, &mut total);
    total
}

fn walk_paragraph_len<D: LayoutDom>(dom: &D, id: D::NodeId, total: &mut usize) {
    if local_name(dom, id) == Some("p") {
        let len = text_of(dom, id).len();
        if len >= 25 {
            *total += len;
        }
    }
    for child in dom.dom_children(id) {
        walk_paragraph_len(dom, child, total);
    }
}

/// The class/id signal: `+25` if any positive hint and `-25` if any negative hint
/// appears in the element's `class` or `id` (substring, lowercased), as readability does.
fn classid_signal<D: LayoutDom>(dom: &D, id: D::NodeId) -> i32 {
    let haystack = format!(
        "{} {}",
        attr(dom, id, "class").unwrap_or_default(),
        attr(dom, id, "id").unwrap_or_default(),
    )
    .to_ascii_lowercase();
    let mut score = 0;
    if POSITIVE_HINTS.iter().any(|h| haystack.contains(h)) {
        score += 25;
    }
    if NEGATIVE_HINTS.iter().any(|h| haystack.contains(h)) {
        score -= 25;
    }
    score
}

/// Text under `root` with chrome (nav / header / footer / aside) and non-rendered
/// (script / style / …) subtrees dropped — the reader-mode body text.
fn chrome_free_text<D: LayoutDom>(dom: &D, root: D::NodeId) -> String {
    let mut out = String::new();
    collect_main_text(dom, root, &mut out);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn collect_main_text<D: LayoutDom>(dom: &D, id: D::NodeId, out: &mut String) {
    if local_name(dom, id).is_some_and(is_chrome_or_non_rendered) {
        return; // skip chrome / non-rendered subtrees
    }
    if let Some(t) = dom.text(id) {
        out.push_str(t);
        out.push(' ');
    }
    for child in dom.dom_children(id) {
        collect_main_text(dom, child, out);
    }
}

/// Subtrees excluded from reader-mode body text: non-rendered content plus page
/// chrome (the landmarks that are not the article).
fn is_chrome_or_non_rendered(name: &str) -> bool {
    is_non_rendered(name) || matches!(name, "nav" | "header" | "footer" | "aside")
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
    use genet_static_dom::StaticDocument;

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
        assert_eq!(
            links.len(),
            2,
            "two href anchors; the named anchor is skipped: {links:?}"
        );
        assert_eq!(
            links[0],
            Link {
                href: "/one".into(),
                text: "First".into(),
                rel: None
            }
        );
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

    #[test]
    fn extracts_the_heading_outline() {
        let doc = StaticDocument::parse(
            "<body>\
                <h1>Title</h1>\
                <h2>Section <em>one</em></h2>\
                <p>body</p>\
                <h3></h3>\
                <h2>Section two</h2>\
             </body>",
        );
        assert_eq!(
            extract_headings(&doc),
            vec![
                Heading {
                    level: 1,
                    text: "Title".into()
                },
                Heading {
                    level: 2,
                    text: "Section one".into()
                },
                // the empty <h3> is skipped
                Heading {
                    level: 2,
                    text: "Section two".into()
                },
            ],
        );
    }

    #[test]
    fn extracts_description_canonical_and_open_graph() {
        let doc = StaticDocument::parse(
            "<html><head>\
                <meta name=\"description\" content=\"A page about things.\">\
                <link rel=\"canonical\" href=\"https://example.com/page\">\
                <meta property=\"og:title\" content=\"Things\">\
                <meta property=\"og:image\" content=\"https://example.com/og.png\">\
             </head><body></body></html>",
        );
        let md = extract_metadata(&doc);
        assert_eq!(md.description.as_deref(), Some("A page about things."));
        assert_eq!(md.canonical.as_deref(), Some("https://example.com/page"));
        assert_eq!(
            md.open_graph,
            vec![
                ("title".to_string(), "Things".to_string()),
                (
                    "image".to_string(),
                    "https://example.com/og.png".to_string()
                ),
            ],
        );
    }

    #[test]
    fn canonical_rel_is_a_token_list() {
        // `rel` is a space-separated token list; `canonical` need not be the only token.
        let doc =
            StaticDocument::parse("<head><link rel=\"alternate canonical\" href=\"/c\"></head>");
        assert_eq!(extract_metadata(&doc).canonical.as_deref(), Some("/c"));
    }

    #[test]
    fn missing_metadata_is_all_none() {
        let doc = StaticDocument::parse("<body><p>no meta</p></body>");
        assert_eq!(extract_metadata(&doc), Metadata::default());
    }

    #[test]
    fn visible_text_excludes_script_and_style() {
        let doc = StaticDocument::parse(
            "<html><head><title>T</title><style>p{color:red}</style></head>\
             <body><h1>Heading</h1><p>Para one.</p>\
             <script>var x = 'not visible';</script>\
             <p>Para two.</p></body></html>",
        );
        // <head> (title + style) and the inline <script> are excluded; body text is
        // concatenated and whitespace-collapsed.
        assert_eq!(extract_text(&doc), "Heading Para one. Para two.");
    }

    #[test]
    fn full_extract_carries_text() {
        let doc = StaticDocument::parse(
            "<html><head><title>T</title></head><body><p>Hello world.</p></body></html>",
        );
        assert_eq!(extract(&doc).text, "Hello world.");
    }

    #[test]
    fn main_text_prefers_the_main_landmark_and_drops_chrome() {
        let doc = StaticDocument::parse(
            "<body>\
                <nav><a href='/'>Home</a> Menu Junk Links</nav>\
                <header>Site Title Boilerplate Banner</header>\
                <main>\
                    <h1>Article Heading</h1>\
                    <p>This is the first paragraph of the real article body, long enough to count.</p>\
                    <p>And a second paragraph continuing the genuine article content here.</p>\
                    <aside>Inline aside promo junk to drop</aside>\
                </main>\
                <footer>Copyright Footer Junk</footer>\
             </body>",
        );
        let main = extract_main_text(&doc).expect("an article body");
        assert!(
            main.contains("first paragraph of the real article body"),
            "{main}"
        );
        assert!(main.contains("second paragraph"), "{main}");
        // Chrome outside the landmark, and an aside *inside* it, are all dropped.
        assert!(!main.contains("Menu Junk"), "nav dropped: {main}");
        assert!(!main.contains("Footer Junk"), "footer dropped: {main}");
        assert!(
            !main.contains("Boilerplate Banner"),
            "header dropped: {main}"
        );
        assert!(
            !main.contains("aside promo junk"),
            "inline aside dropped: {main}"
        );
    }

    #[test]
    fn main_text_scores_content_over_sidebar() {
        // No <main>: the heuristic must pick the article div over the sidebar by
        // paragraph density + class signal, and drop an inline footer within it.
        let doc = StaticDocument::parse(
            "<body>\
                <div class='sidebar'><p>Ads and promo links and sponsor junk over here.</p></div>\
                <div class='article-content'>\
                    <p>The genuine article body paragraph one, with substantial readable prose.</p>\
                    <p>Paragraph two of the genuine article, with more substantial readable content.</p>\
                    <footer>inline footer junk to drop</footer>\
                </div>\
             </body>",
        );
        let main = extract_main_text(&doc).expect("an article body");
        assert!(
            main.contains("genuine article body paragraph one"),
            "{main}"
        );
        assert!(
            main.contains("Paragraph two of the genuine article"),
            "{main}"
        );
        assert!(
            !main.contains("Ads and promo"),
            "sidebar lost to scoring: {main}"
        );
        assert!(
            !main.contains("footer junk"),
            "inline footer dropped: {main}"
        );
    }

    #[test]
    fn main_text_is_none_for_a_link_list() {
        // A nav-only page (an app shell / index) has no article body.
        let doc = StaticDocument::parse(
            "<body><nav><a href='/a'>A</a><a href='/b'>B</a><a href='/c'>C</a></nav></body>",
        );
        assert_eq!(extract_main_text(&doc), None);
    }

    #[test]
    fn full_extract_carries_main_text() {
        let doc = StaticDocument::parse(
            "<body><main><p>The article body paragraph with enough prose to register.</p></main></body>",
        );
        assert!(
            extract(&doc)
                .main_text
                .as_deref()
                .is_some_and(|m| m.contains("article body paragraph")),
        );
    }
}
