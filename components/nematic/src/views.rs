/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Native [`xilem_serval`] views for smolweb formats.
//!
//! Each format gets its own idiomatic view (the Lagrange / Geopard approach:
//! render natively, not flattened into one document model). The views build
//! genet element trees directly — real focusable links, format-specific classes
//! for theming — so genet lays them out and netrender paints them, the same path
//! the host chrome and the djot note tile take. They consume [`errand::parse`]
//! ASTs and depend on no document model, so pelt and the Mere host share them.
//!
//! Theming lives in [`theme`]: a per-site palette by default (each capsule its own
//! color identity), overridable with presets. See [`stylesheet`].
//!
//! v1 ships [`gemtext_view`]; gopher and feed views follow.

use errand::parse::feed::{Feed, FeedEntry};
use errand::parse::gemtext::GemLine;
use errand::parse::gopher::{GopherItem, GopherKind};
use errand::parse::nex::{self, NexEntry};
use xilem_serval::{
    AnyView, ElementView, PointerClick, GenetCtx, GenetElement, a, clickable, div, el, h1, h2,
    h3, li, p, span, text, ul,
};

mod theme;
pub use theme::{SmolwebPalette, SmolwebTheme, stylesheet};

/// A built smolweb view: a boxed, type-erased element view. Boxing erases the
/// concrete view type (so a document's heterogeneous line elements share one child
/// sequence) and, as the return type, keeps the view from capturing the input
/// `&[…]` lifetime — the `chrome.rs` / `tile_surface.rs` `*View` pattern.
pub type SmolwebView<State, Action> = Box<dyn AnyView<State, Action, GenetCtx, GenetElement>>;

fn boxed<State, Action>(
    view: impl ElementView<State, Action> + 'static,
) -> SmolwebView<State, Action>
where
    State: 'static,
    Action: 'static,
{
    Box::new(view)
}

/// Build a native gemtext document view under a `div.gemtext`.
///
/// Each line maps to a semantic element: `h1`–`h3` headings, a focusable `a` per
/// link line, `pre` for preformatted blocks. Consecutive lines group the way
/// gemtext reads — text runs into one paragraph, `* ` items into one list, `> `
/// lines into one blockquote. Links are keyboard-reachable [`clickable`]s that
/// carry `href` (so pelt's link resolution still works) and emit `on_navigate(url)`
/// as the action the host routes.
pub fn gemtext_view<State, Action, N>(
    lines: &[GemLine],
    on_navigate: N,
) -> SmolwebView<State, Action>
where
    State: 'static,
    // The host's action type opts into bubbling via `xilem_serval::Action`; a link
    // click emits `on_navigate(url)` as that action.
    Action: xilem_serval::Action + 'static,
    N: Fn(&str) -> Action + Clone + 'static,
{
    let mut builder = Builder::new(on_navigate);
    for line in lines {
        builder.handle(line);
    }
    builder.finish()
}

/// Accumulates line elements, grouping text/list/quote runs into single blocks
/// the way gemtext reads, then emits the `div.gemtext`.
struct Builder<State, Action, N> {
    children: Vec<SmolwebView<State, Action>>,
    para: Vec<String>,
    quote: Vec<String>,
    list: Vec<SmolwebView<State, Action>>,
    on_navigate: N,
}

impl<State, Action, N> Builder<State, Action, N>
where
    State: 'static,
    Action: xilem_serval::Action + 'static,
    N: Fn(&str) -> Action + Clone + 'static,
{
    fn new(on_navigate: N) -> Self {
        Self {
            children: Vec::new(),
            para: Vec::new(),
            quote: Vec::new(),
            list: Vec::new(),
            on_navigate,
        }
    }

    fn handle(&mut self, line: &GemLine) {
        match line {
            GemLine::Heading { level, text: t } => {
                self.flush_all();
                let class = format!("gemtext-h{level}");
                let heading = match level {
                    1 => h1(text(t.clone())),
                    2 => h2(text(t.clone())),
                    _ => h3(text(t.clone())),
                };
                self.children.push(boxed(heading.attr("class", class)));
            },
            GemLine::Text(t) => {
                self.flush_list();
                self.flush_quote();
                self.para.push(t.clone());
            },
            GemLine::Link { url, label } => {
                self.flush_all();
                let link = self.link(url, label);
                self.children.push(link);
            },
            GemLine::Item(t) => {
                self.flush_para();
                self.flush_quote();
                self.list
                    .push(boxed(li(text(t.clone())).attr("class", "gemtext-item")));
            },
            GemLine::Quote(t) => {
                self.flush_para();
                self.flush_list();
                self.quote.push(t.clone());
            },
            GemLine::Pre { alt, text: t } => {
                self.flush_all();
                let mut pre = el("pre", text(t.clone())).attr("class", "gemtext-pre");
                if let Some(alt) = alt {
                    pre = pre.attr("data-alt", alt.clone());
                }
                self.children.push(boxed(pre));
            },
            GemLine::Blank => self.flush_para(),
        }
    }

    /// A focusable link line: `p.gemtext-linkline > a.gemtext-link[href]`, emitting
    /// `on_navigate(url)` on activation.
    fn link(&self, url: &str, label: &str) -> SmolwebView<State, Action> {
        let display = if label.is_empty() { url } else { label };
        let navigate = self.on_navigate.clone();
        let target = url.to_string();
        let anchor = a(text(display.to_string()))
            .attr("href", url)
            .attr("class", "gemtext-link");
        let link = clickable(anchor, move |_state: &mut State, _click: PointerClick| {
            navigate(&target)
        });
        boxed(p(link).attr("class", "gemtext-linkline"))
    }

    fn flush_para(&mut self) {
        if self.para.is_empty() {
            return;
        }
        // Consecutive text lines flow into one paragraph (gemtext clients re-wrap).
        let joined = std::mem::take(&mut self.para).join(" ");
        self.children
            .push(boxed(p(text(joined)).attr("class", "gemtext-text")));
    }

    fn flush_quote(&mut self) {
        if self.quote.is_empty() {
            return;
        }
        let joined = std::mem::take(&mut self.quote).join(" ");
        self.children.push(boxed(
            el("blockquote", text(joined)).attr("class", "gemtext-quote"),
        ));
    }

    fn flush_list(&mut self) {
        if self.list.is_empty() {
            return;
        }
        let items = std::mem::take(&mut self.list);
        self.children
            .push(boxed(ul(items).attr("class", "gemtext-list")));
    }

    fn flush_all(&mut self) {
        self.flush_para();
        self.flush_list();
        self.flush_quote();
    }

    fn finish(mut self) -> SmolwebView<State, Action> {
        self.flush_all();
        boxed(div(self.children).attr("class", "gemtext"))
    }
}

/// Build a native gopher menu view under a `div.gopher`.
///
/// Consecutive `i` info lines fold into one monospace `pre` (gopher menus carry
/// ASCII art); `3` errors render as their own line; every resource line is a
/// type-marked, focusable link (`p.gopher-itemline > span.gopher-type + a[href]`)
/// emitting `on_navigate(url)`. The item type drives the marker and a `data-kind`
/// attribute for theming.
pub fn gopher_view<State, Action, N>(
    items: &[GopherItem],
    on_navigate: N,
) -> SmolwebView<State, Action>
where
    State: 'static,
    Action: xilem_serval::Action + 'static,
    N: Fn(&str) -> Action + Clone + 'static,
{
    let mut children: Vec<SmolwebView<State, Action>> = Vec::new();
    let mut info: Vec<String> = Vec::new();
    for item in items {
        match &item.kind {
            GopherKind::Info => info.push(item.display.clone()),
            GopherKind::Error => {
                flush_info(&mut info, &mut children);
                let line = format!("[error] {}", item.display);
                children.push(boxed(p(text(line)).attr("class", "gopher-error")));
            },
            _ => {
                flush_info(&mut info, &mut children);
                // Every non-info/error item carries a URL.
                if let Some(url) = &item.url {
                    children.push(gopher_link(&item.kind, &item.display, url, &on_navigate));
                }
            },
        }
    }
    flush_info(&mut info, &mut children);
    boxed(div(children).attr("class", "gopher"))
}

/// A Nex directory listing as a native view: each entry is a focusable link
/// resolved against `address` (nex has no inline link syntax; the listing IS
/// the links), directories marked with a trailing slash and a `data-kind`
/// for theming. The plan's "genuinely distinct formats get their own small
/// AST views" — nex's first (session-engines plan phase 5).
pub fn nex_view<State, Action, N>(
    address: &str,
    entries: &[NexEntry],
    on_navigate: N,
) -> SmolwebView<State, Action>
where
    State: 'static,
    Action: xilem_serval::Action + 'static,
    N: Fn(&str) -> Action + Clone + 'static,
{
    let base = nex::base_url(address);
    let mut children: Vec<SmolwebView<State, Action>> = Vec::new();
    for entry in entries {
        let label = if entry.is_dir {
            format!("{}/", entry.name.trim_end_matches('/'))
        } else {
            entry.name.clone()
        };
        let url = format!("{base}{}", entry.name.trim_start_matches('/'));
        let kind = if entry.is_dir { "dir" } else { "file" };
        let navigate = on_navigate.clone();
        let anchor = a(text(label))
            .attr("href", url.clone())
            .attr("class", "nex-link");
        let link = clickable(anchor, move |_state: &mut State, _click: PointerClick| {
            navigate(&url)
        });
        children.push(boxed(
            p(link).attr("class", "nex-entry").attr("data-kind", kind),
        ));
    }
    boxed(div(children).attr("class", "nex"))
}

/// Flush an accumulated run of `i` info lines into one monospace `pre`, preserving
/// the ASCII-art layout gopher menus often carry.
fn flush_info<State, Action>(info: &mut Vec<String>, children: &mut Vec<SmolwebView<State, Action>>)
where
    State: 'static,
    Action: 'static,
{
    if info.is_empty() {
        return;
    }
    let joined = std::mem::take(info).join("\n");
    children.push(boxed(el("pre", text(joined)).attr("class", "gopher-info")));
}

/// A type-marked, focusable gopher link line.
fn gopher_link<State, Action, N>(
    kind: &GopherKind,
    display: &str,
    url: &str,
    on_navigate: &N,
) -> SmolwebView<State, Action>
where
    State: 'static,
    Action: xilem_serval::Action + 'static,
    N: Fn(&str) -> Action + Clone + 'static,
{
    let (marker, kind_name) = kind_marker(kind);
    let navigate = on_navigate.clone();
    let target = url.to_string();
    let marker_span = span(text(marker)).attr("class", "gopher-type");
    let anchor = a(text(display.to_string()))
        .attr("href", url)
        .attr("class", "gopher-link");
    let link = clickable(anchor, move |_state: &mut State, _click: PointerClick| {
        navigate(&target)
    });
    boxed(
        p((marker_span, link))
            .attr("class", "gopher-itemline")
            .attr("data-kind", kind_name),
    )
}

/// Build a native feed view under a `div.feed`: an article-list reader, not a
/// document.
///
/// The feed title/subtitle head the view, then one `article.feed-entry` per entry
/// (title as a focusable link emitting `on_navigate(url)`, a relative-ish date, and
/// a summary snippet). Opening an entry recurses into whatever its URL's scheme
/// resolves to.
pub fn feed_view<State, Action, N>(feed: &Feed, on_navigate: N) -> SmolwebView<State, Action>
where
    State: 'static,
    Action: xilem_serval::Action + 'static,
    N: Fn(&str) -> Action + Clone + 'static,
{
    let mut children: Vec<SmolwebView<State, Action>> = Vec::new();
    if let Some(title) = &feed.title {
        children.push(boxed(h1(text(title.clone())).attr("class", "feed-title")));
    }
    if let Some(subtitle) = &feed.subtitle {
        children.push(boxed(
            p(text(subtitle.clone())).attr("class", "feed-subtitle"),
        ));
    }
    for entry in &feed.entries {
        children.push(feed_entry_card(entry, &on_navigate));
    }
    boxed(div(children).attr("class", "feed"))
}

/// One feed entry as an `article`: a (linked) title, an optional date, an optional
/// summary.
fn feed_entry_card<State, Action, N>(
    entry: &FeedEntry,
    on_navigate: &N,
) -> SmolwebView<State, Action>
where
    State: 'static,
    Action: xilem_serval::Action + 'static,
    N: Fn(&str) -> Action + Clone + 'static,
{
    let mut parts: Vec<SmolwebView<State, Action>> = Vec::new();
    let title = entry.title.clone().unwrap_or_default();
    match &entry.link {
        Some(url) => {
            let navigate = on_navigate.clone();
            let target = url.to_string();
            let anchor = a(text(title))
                .attr("href", url)
                .attr("class", "feed-entry-link");
            let link = clickable(anchor, move |_state: &mut State, _click: PointerClick| {
                navigate(&target)
            });
            parts.push(boxed(h2(link).attr("class", "feed-entry-title")));
        },
        None => parts.push(boxed(h2(text(title)).attr("class", "feed-entry-title"))),
    }
    if let Some(date) = &entry.date {
        parts.push(boxed(
            span(text(date.clone())).attr("class", "feed-entry-date"),
        ));
    }
    if let Some(summary) = &entry.summary {
        parts.push(boxed(
            p(text(summary.clone())).attr("class", "feed-entry-summary"),
        ));
    }
    boxed(el("article", parts).attr("class", "feed-entry"))
}

/// A short type-marker label and a `data-kind` name for a gopher item kind.
fn kind_marker(kind: &GopherKind) -> (&'static str, &'static str) {
    match kind {
        GopherKind::Submenu => ("[dir]", "submenu"),
        GopherKind::Text => ("[txt]", "text"),
        GopherKind::Search => ("[?]", "search"),
        GopherKind::Image => ("[img]", "image"),
        GopherKind::Binary => ("[bin]", "binary"),
        GopherKind::Sound => ("[snd]", "sound"),
        GopherKind::Telnet => ("[tel]", "telnet"),
        GopherKind::Url => ("[url]", "url"),
        GopherKind::Other(_) => ("[*]", "other"),
        // Info/Error are handled before the link path; never reached here.
        GopherKind::Info | GopherKind::Error => ("", "info"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use errand::parse::gemtext::parse as parse_gemtext;
    use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};
    use genet_scripted_dom::ScriptedDom;
    use std::cell::RefCell;
    use std::rc::Rc;
    use xilem_serval::GenetAppRunner;

    /// A test action type the link handler can emit (the marker opts it in).
    #[derive(Debug)]
    struct Nav(#[allow(dead_code)] String);
    impl xilem_serval::Action for Nav {}

    /// A gemtext document builds the expected element tree: a `div.gemtext` whose
    /// children are a heading, a paragraph, a link line, and a grouped list.
    #[test]
    fn gemtext_builds_native_element_tree() {
        let lines =
            parse_gemtext("# Title\n\nHello world.\n=> gemini://x.test/ A link\n* one\n* two\n");
        let dom = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = GenetAppRunner::<(), _, _, Nav>::new(
            dom.clone(),
            move |_: &()| gemtext_view::<(), Nav, _>(&lines, |url| Nav(url.to_string())),
            (),
        );

        let d = dom.borrow();
        let root = runner.root();
        assert_eq!(d.kind(root), NodeKind::Element);
        assert_eq!(d.element_name(root).unwrap().local.as_ref(), "div");

        let kids: Vec<_> = d.dom_children(root).collect();
        let names: Vec<&str> = kids
            .iter()
            .map(|n| d.element_name(*n).unwrap().local.as_ref())
            .collect();
        assert_eq!(
            names,
            vec!["h1", "p", "p", "ul"],
            "heading, paragraph, link line, list"
        );

        // The list groups both items.
        let ul = *kids.last().unwrap();
        assert_eq!(
            d.dom_children(ul).count(),
            2,
            "two list items grouped into one ul"
        );
    }

    /// A gopher menu builds: an info run folds to one `pre`, a resource is a
    /// type-marked link line (`p.gopher-itemline > span + a[href]`).
    #[test]
    fn gopher_builds_info_run_and_typed_links() {
        use errand::parse::gopher::parse as parse_gopher;
        let body = "iWelcome\t\texample.test\t70\r\nito the hole\t\texample.test\t70\r\n1Files\t/files\texample.test\t70\r\n";
        let items = parse_gopher(body);
        let dom = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = GenetAppRunner::<(), _, _, Nav>::new(
            dom.clone(),
            move |_: &()| gopher_view::<(), Nav, _>(&items, |url| Nav(url.to_string())),
            (),
        );

        let d = dom.borrow();
        let no_ns = Namespace::from("");
        let root = runner.root();
        assert_eq!(d.element_name(root).unwrap().local.as_ref(), "div");
        let kids: Vec<_> = d.dom_children(root).collect();
        let names: Vec<&str> = kids
            .iter()
            .map(|n| d.element_name(*n).unwrap().local.as_ref())
            .collect();
        assert_eq!(
            names,
            vec!["pre", "p"],
            "two info lines fold to one pre, then the item line"
        );

        // The item line: span marker + anchor with the synthesised gopher URL.
        let itemline = kids[1];
        let anchor = d
            .dom_children(itemline)
            .nth(1)
            .expect("the anchor after the marker span");
        assert_eq!(d.element_name(anchor).unwrap().local.as_ref(), "a");
        assert_eq!(
            d.attribute(anchor, &no_ns, &LocalName::from("href")),
            Some("gopher://example.test/1/files"),
        );
        assert_eq!(
            d.attribute(itemline, &no_ns, &LocalName::from("data-kind")),
            Some("submenu"),
        );
    }

    /// A feed builds an article-list: title heading, then one `article.feed-entry`
    /// per entry, the entry title a link to its URL.
    #[test]
    fn feed_builds_article_list() {
        use errand::parse::feed::{Feed, FeedEntry};
        let feed = Feed {
            title: Some("Capsule Log".into()),
            entries: vec![
                FeedEntry {
                    title: Some("First".into()),
                    link: Some("gemini://x.test/1".into()),
                    date: Some("2026-01-01".into()),
                    summary: Some("A summary.".into()),
                },
                FeedEntry {
                    title: Some("Second".into()),
                    ..FeedEntry::default()
                },
            ],
            ..Feed::default()
        };
        let dom = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = GenetAppRunner::<(), _, _, Nav>::new(
            dom.clone(),
            move |_: &()| feed_view::<(), Nav, _>(&feed, |url| Nav(url.to_string())),
            (),
        );

        let d = dom.borrow();
        let no_ns = Namespace::from("");
        let root = runner.root();
        let names: Vec<&str> = d
            .dom_children(root)
            .map(|n| d.element_name(n).unwrap().local.as_ref())
            .collect();
        assert_eq!(
            names,
            vec!["h1", "article", "article"],
            "feed title then two entries"
        );

        // First entry: an h2 title whose anchor links to the entry URL.
        let first = d.dom_children(root).nth(1).unwrap();
        let title = d.dom_children(first).next().unwrap();
        assert_eq!(d.element_name(title).unwrap().local.as_ref(), "h2");
        let anchor = d.dom_children(title).next().unwrap();
        assert_eq!(
            d.attribute(anchor, &no_ns, &LocalName::from("href")),
            Some("gemini://x.test/1"),
        );
    }

    /// A link line is a focusable anchor carrying its href (the navigable target).
    #[test]
    fn link_line_anchor_carries_href() {
        let lines = parse_gemtext("=> gemini://example.test/page  Example\n");
        let dom = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = GenetAppRunner::<(), _, _, Nav>::new(
            dom.clone(),
            move |_: &()| gemtext_view::<(), Nav, _>(&lines, |url| Nav(url.to_string())),
            (),
        );

        let d = dom.borrow();
        let no_ns = Namespace::from("");
        // div.gemtext > p.linkline > a[href]
        let p = d.dom_children(runner.root()).next().expect("a link line");
        let anchor = d.dom_children(p).next().expect("the anchor");
        assert_eq!(d.element_name(anchor).unwrap().local.as_ref(), "a");
        assert_eq!(
            d.attribute(anchor, &no_ns, &LocalName::from("href")),
            Some("gemini://example.test/page"),
        );
    }

    /// A nex directory listing builds one focusable link per entry
    /// (`p.nex-entry > a[href]`), hrefs resolved against the request address,
    /// directories kind-marked for theming.
    #[test]
    fn nex_builds_entry_links() {
        let entries = vec![
            errand::parse::nex::NexEntry {
                name: "docs/".to_string(),
                is_dir: true,
            },
            errand::parse::nex::NexEntry {
                name: "readme.txt".to_string(),
                is_dir: false,
            },
        ];
        let dom = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = GenetAppRunner::<(), _, _, Nav>::new(
            dom.clone(),
            move |_: &()| {
                nex_view::<(), Nav, _>("nex://example.test/", &entries, |url| {
                    Nav(url.to_string())
                })
            },
            (),
        );

        let d = dom.borrow();
        let no_ns = Namespace::from("");
        let root = runner.root();
        let rows: Vec<_> = d.dom_children(root).collect();
        assert_eq!(rows.len(), 2, "one row per entry");

        let anchor = d
            .dom_children(rows[0])
            .find(|&n| d.element_name(n).is_some_and(|q| q.local.as_ref() == "a"))
            .expect("the entry renders a focusable anchor");
        assert_eq!(
            d.attribute(anchor, &no_ns, &LocalName::from("href")),
            Some("nex://example.test/docs/"),
            "the directory href resolves against the request address"
        );
        assert_eq!(
            d.attribute(rows[0], &no_ns, &LocalName::from("data-kind")),
            Some("dir"),
            "the directory row is kind-marked for theming"
        );
        assert_eq!(
            d.attribute(rows[1], &no_ns, &LocalName::from("data-kind")),
            Some("file"),
        );
    }
}
