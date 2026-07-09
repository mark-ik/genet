/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Baseline user-agent stylesheet, prepended to every cascade.
//!
//! Stylo's empty stylist gives every element the CSS initial value
//! for `display` — which is `inline`. Real browsers ship a UA
//! stylesheet that flips structural elements (`<html>`, `<body>`,
//! `<div>`, headings, lists, etc.) to `display: block` and sizes
//! the root to fill the viewport. Without that, an HTML document
//! parsed through Stylo lays out as one long inline stream.
//!
//! This is the load-bearing subset of WHATWG / WebKit's UA stylesheet
//! that gets a serval document close enough to look like HTML: the
//! `display` flips plus the metric defaults real browsers ship (the
//! `<body>` gutter, the `<h1>`..`<h6>` font-size scale + weight, and the
//! block-flow margins on headings / `<p>` / lists / `<blockquote>`).
//! Those matter because a thin UA sheet shifts *every* box, so the page
//! looks wrong before any harder feature does. The margins collapse,
//! including *through* a non-BFC parent; the incremental layout splice
//! handles that by falling back to a full relayout when a collapse would
//! cross the spliced subtree's boundary. User stylesheets override (later
//! origin entries win the cascade); Stylo's `Origin::UserAgent` ordering
//! handles that automatically.
//!
//! Inspired by:
//! - <https://html.spec.whatwg.org/multipage/rendering.html>
//! - WebKit's `html.css` baseline rules
//!
//! Grow this as the property surface grows — but stay minimal.
//! Heavyweight UA stylesheets (gradients, table layout, form
//! controls) are deferred until the cascade + box-tree + emit
//! actually exercise the matching properties.

/// Prepended to every `run_cascade` invocation. Always sets:
///
/// - `<html>` to `display: block` and `width / height: 100%`, so the root box
///   fills the viewport (the ICB-sized root); the `100%` resolves against the
///   Taffy root, which `construct` gives explicit viewport dimensions. `<body>`
///   to `display: block` and `width: 100%` only — its height stays `auto`
///   (content), so a short page's body does not stretch to the viewport and its
///   margins / padding therefore do not overflow it (no phantom document scroll).
///   A taller-than-viewport body overflows the root and scrolls, as it should.
/// - Common block-flow elements (`div`, `section`, `article`,
///   `header`, `footer`, `nav`, `main`, `aside`, `p`, headings,
///   lists, `blockquote`, `pre`, `figure`, `address`, `hr`) to
///   `display: block`. Stylo's default would leave them inline.
pub const UA_DEFAULTS: &str = r#"
html {
    display: block;
    width: 100%;
    height: 100%;
}

body {
    display: block;
    width: 100%;
    margin: 8px;
}

/* Document metadata never renders. Per the WHATWG rendering spec
   (`head, link, meta, script, style, title { display: none }`), these
   must not paint — otherwise the `<title>` and inline `<style>` source
   show up as visible page text. */
head, title, meta, link, style, script, base, noscript, template {
    display: none;
}

address, article, aside, blockquote, div, dl, dt, dd,
figure, figcaption, footer, h1, h2, h3, h4, h5, h6, header, hgroup,
hr, main, nav, ol, p, pre, section, ul, li,
fieldset, form, details, summary, dialog, menu {
    display: block;
}

/* Form controls the WHATWG rendering spec renders as `inline-block`. serval
   shipped no `display` for any of them, leaving them `inline`, where CSS
   `width` / `height` are ignored outright and an empty control (`<input>`) takes
   no box at all. `<button>` additionally hosts children: a replaced child (an
   `<img>`, an `<external-texture>`, a `<chisel-leaf>`) flows in its inline
   formatting context, which is now handled — inline replaced elements carry
   their leaf/texture payload as an `InlineBoxItem`.

   Intrinsic sizing (an `<input>`'s `size` attribute, a `<textarea>`'s
   `rows`/`cols`) is a separate concern and still absent: an unsized control
   shrink-to-fits its content. This rule only makes authored CSS sizing work. */
button, input, select, textarea {
    display: inline-block;
}

/* A hidden input never renders. */
input[type="hidden"] {
    display: none;
}

/* Table box hierarchy. serval lays a `display: table` box out as a CSS grid
   (`stylo_taffy` maps table -> grid; `box_tree` flattens the row-group / row
   nesting and gives each cell an explicit grid position). These real display
   values replace the old `table,tr,... { display: block }` stopgap that stacked
   every cell. `border-collapse` / `caption` placement / fixed table-layout are
   first-cut deferrals. */
table { display: table; }
thead, tbody, tfoot { display: table-row-group; }
tr { display: table-row; }
td, th { display: table-cell; }
caption { display: table-caption; }
th { font-weight: bold; }

/* Inline emphasis — real browsers ship these; they drive the
   per-run weight/style of inline formatting contexts. */
b, strong { font-weight: bold; }
i, em, cite, var, dfn, address { font-style: italic; }

/* Heading scale + weight + margins (WHATWG rendering §15.3.3 / CSS2.2 §B).
   Without these every heading renders at body size, un-bolded, and unspaced —
   the biggest single "every box shifts" gap. `em` margins resolve against each
   heading's own (just-set) font-size, so e.g. `h1`'s 0.67em is 0.67×2em. The
   margins collapse between adjacent blocks and, for a first/last child of a
   non-BFC parent, *through* the parent; IncrementalLayout's splice falls back
   to a full relayout in exactly that case (see `splice_loses_margin_collapse`),
   so both layout paths agree. Nested-section heading rescaling
   (`:is(article,…) h1`) is deferred. */
h1 { font-size: 2em;    margin: 0.67em 0; font-weight: bold; }
h2 { font-size: 1.5em;  margin: 0.83em 0; font-weight: bold; }
h3 { font-size: 1.17em; margin: 1em 0;    font-weight: bold; }
h4 {                    margin: 1.33em 0; font-weight: bold; }
h5 { font-size: 0.83em; margin: 1.67em 0; font-weight: bold; }
h6 { font-size: 0.67em; margin: 2.33em 0; font-weight: bold; }

/* Block-flow vertical rhythm. `em` margins resolve against the element's own
   font-size (1em ≈ one line). Adjacent block margins collapse, so stacked
   paragraphs sit one line apart, not two. */
p, dl { margin: 1em 0; }
blockquote, figure { margin: 1em 40px; }
/* `<pre>` preserves source whitespace + newlines and does not soft-wrap
   (`white-space: pre` = `white-space-collapse: preserve` + `text-wrap-mode:
   nowrap`), so code blocks / ASCII art keep their lines. */
pre { margin: 1em 0; white-space: pre; }
dd { margin-left: 40px; }
hr { margin: 0.5em 0; }

/* Lists indent so their markers (emitted by paint as a hanging bullet /
   ordinal) have room to sit in the padding, left of each item's content.
   `list-style-type` is inherited, so the item's marker kind comes from its
   list: bullets for `ul`, numbers for `ol`. */
ul, ol { margin: 1em 0; padding-left: 40px; }
ul { list-style-type: disc; }
ol { list-style-type: decimal; }
"#;
