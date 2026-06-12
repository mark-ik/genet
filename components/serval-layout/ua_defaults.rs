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
//! This is the minimal subset of WHATWG / WebKit's UA stylesheet
//! that gets a serval document close enough to look like HTML.
//! User stylesheets override (later origin entries win the cascade);
//! Stylo's `Origin::UserAgent` ordering handles that automatically.
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

/* Lists indent so their markers (emitted by paint as a hanging bullet /
   ordinal) have room to sit in the padding, left of each item's content.
   `list-style-type` is inherited, so the item's marker kind comes from its
   list: bullets for `ul`, numbers for `ol`. */
ul, ol { padding-left: 40px; }
ul { list-style-type: disc; }
ol { list-style-type: decimal; }
"#;
