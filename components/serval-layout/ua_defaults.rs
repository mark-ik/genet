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
//! `display` flips plus the collapse-free metric defaults real browsers
//! ship (the `<h1>`..`<h6>` font-size scale + weight). Those matter
//! because a thin UA sheet shifts *every* box, so the page looks wrong
//! before any harder feature does. The spec block-flow *margins* are not
//! here yet (they need a margin-collapse-parity engine fix first — see
//! the NOTE by the heading rules). User stylesheets override (later
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
fieldset, form, details, summary, dialog, menu,
table, caption, thead, tbody, tfoot, tr {
    display: block;
}

/* Inline emphasis — real browsers ship these; they drive the
   per-run weight/style of inline formatting contexts. */
b, strong { font-weight: bold; }
i, em, cite, var, dfn, address { font-style: italic; }

/* Heading scale + weight (WHATWG rendering §15.3.3 / CSS2.2 §B). Without
   these every heading renders at body size and un-bolded — the biggest
   single "every box shifts" gap that is also collapse-free (font-size and
   weight change a box's content, not its margins, so the full and
   incremental layout paths agree). The heading *margins* the spec also
   ships are deferred with the rest of the block margins (see NOTE below).
   Nested-section heading rescaling (`:is(article,…) h1`) is also deferred. */
h1 { font-size: 2em;    font-weight: bold; }
h2 { font-size: 1.5em;  font-weight: bold; }
h3 { font-size: 1.17em; font-weight: bold; }
h4 {                    font-weight: bold; }
h5 { font-size: 0.83em; font-weight: bold; }
h6 { font-size: 0.67em; font-weight: bold; }

/* NOTE: the spec block-flow margins (`p`/`h1`..`h6`/`ul`/`ol`/`blockquote`/
   `figure`/`pre`/`dd` and the `body { margin: 8px }` gutter) are deliberately
   NOT set here yet. They are correct in the *full* layout path (verified) but
   expose two engine divergences that need fixing first, not a sheet line:
     1. `body`'s margin is dropped by the full box-tree root handling (a
        root-child margin gap), while IncrementalLayout applies it — the two
        paths disagree on the document gutter.
     2. A first child's top margin collapses *through* its block parent in
        full-document layout (html → body → p), but NOT in IncrementalLayout's
        splice, which re-lays-out the subtree in isolation (a `SubtreeView`
        root establishes its own formatting context). So adding block margins
        mis-positions the first spliced child relative to a full recompute.
   Both are tracked in the real-web layout fidelity plan (UA-sheet item) as a
   margin-collapse-parity fix that must land before the UA margins do. */

/* Lists indent so their markers (emitted by paint as a hanging bullet /
   ordinal) have room to sit in the padding, left of each item's content.
   `list-style-type` is inherited, so the item's marker kind comes from its
   list: bullets for `ul`, numbers for `ol`. */
ul, ol { padding-left: 40px; }
ul { list-style-type: disc; }
ol { list-style-type: decimal; }
"#;
