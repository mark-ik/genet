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
/// - `<html>` and `<body>` to `display: block` and `width / height:
///   100%`, so an empty document still fills the viewport. The
///   `100%` resolves against the synthetic Taffy root (which
///   `construct` gives explicit viewport dimensions).
/// - Common block-flow elements (`div`, `section`, `article`,
///   `header`, `footer`, `nav`, `main`, `aside`, `p`, headings,
///   lists, `blockquote`, `pre`, `figure`, `address`, `hr`) to
///   `display: block`. Stylo's default would leave them inline.
pub const UA_DEFAULTS: &str = r#"
html, body {
    display: block;
    width: 100%;
    height: 100%;
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
"#;
