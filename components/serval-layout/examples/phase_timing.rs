// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! §0 cold-layout phase-breakdown harness.
//!
//! Splits serval-layout's cold cost into cascade / box-tree-build / shaping /
//! taffy-compute / fragment-readback, the prerequisite measurement for the
//! parallel-cascade thesis in mere's
//! `2026-06-19_cross_platform_parallelism_strategy.md` §0. Until this split
//! exists, "parallelism owns the cold cost" is a hypothesis: a perfect parallel
//! cascade caps the achievable win if box-tree-build or shaping dominates.
//!
//! Run (native release; debug is ~6x slower and not representative):
//!
//! ```text
//! # aggregate cascade + total-layout over N warm passes
//! cargo run -p serval-layout --release --example phase_timing
//! # add the per-phase split (build/shape/compute/readback), grouped per pass;
//! # read the LAST pass (warmest). On a real saved page, pass its path.
//! SERVAL_LAYOUT_TIMING=1 cargo run -p serval-layout --release --example phase_timing -- --iters=2 path/to/page.html
//! ```
//!
//! Defaults to a synthetic ~550 KB text-heavy page with real CSS, so it runs
//! with no fixture; pass a file path to measure a real saved page. Images are
//! not decoded (an empty `ImagePlane`), so the numbers reflect cascade + box +
//! text, not image decode (a separate cost).

use std::time::Instant;

use serval_layout::{
    inline_stylesheets_from_source, layout_via_box_tree, run_cascade, ImagePlane, StylePlane,
    TextMeasureCtx,
};
use serval_static_dom::StaticDocument;

const VIEWPORT_W: f32 = 1200.0;
const VIEWPORT_H: f32 = 900.0;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let iters: usize = args
        .iter()
        .find_map(|a| a.strip_prefix("--iters="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(7)
        .max(1);
    let path = args.iter().find(|a| !a.starts_with("--")).cloned();

    let html = match &path {
        Some(p) => std::fs::read_to_string(p).unwrap_or_else(|e| {
            eprintln!("read {p}: {e}");
            std::process::exit(1);
        }),
        None => synthetic_page(550_000),
    };
    println!(
        "page: {}  ({} KB)",
        path.as_deref().unwrap_or("<synthetic>"),
        html.len() / 1024
    );

    let sheets_owned = inline_stylesheets_from_source(&html);
    let sheets: Vec<&str> = sheets_owned.iter().map(String::as_str).collect();
    println!("author stylesheets: {}", sheets.len());

    // Parse once. Parse is the HTML->DOM cost, distinct from the layout cost
    // §0 cares about; reported for context.
    let t = Instant::now();
    let doc = StaticDocument::parse(&html);
    let parse_ms = ms(t);

    // Font discovery is a one-time cost; measure it once for the report.
    let t = Instant::now();
    drop(TextMeasureCtx::new());
    let font_ms = ms(t);

    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(VIEWPORT_W),
        height: taffy::AvailableSpace::Definite(VIEWPORT_H),
    };
    let images = ImagePlane::new();

    // When SERVAL_LAYOUT_TIMING is set, layout_via_box_tree prints its four
    // sub-phases per pass; mark the passes so the warmest (last) is identifiable.
    let lib_timing = std::env::var_os("SERVAL_LAYOUT_TIMING").is_some();

    let mut cascade = Vec::with_capacity(iters);
    let mut layout = Vec::with_capacity(iters);
    // One warm-up pass (k = 0) primes caches / branch predictors, then `iters`
    // measured passes.
    for k in 0..=iters {
        if lib_timing {
            eprintln!("--- pass {k} (warmup={}) ---", k == 0);
        }
        let mut styles = StylePlane::new();

        let t = Instant::now();
        run_cascade(
            &doc,
            &mut styles,
            euclid::default::Size2D::new(VIEWPORT_W, VIEWPORT_H),
            &sheets,
            None,
        );
        let c = ms(t);

        // Fresh context per pass = a genuine cold first paint: `reset()` then drops
        // nothing. Reusing one context across full re-layouts would add a cross-pass
        // cache-drop cost that no real frame pays (warm frames are *incremental*, not
        // full relayout), inflating the total above the true cold cost.
        let mut text_ctx = TextMeasureCtx::new();
        let t = Instant::now();
        let _ = layout_via_box_tree(&doc, &styles, &images, viewport, &mut text_ctx);
        let l = ms(t);

        if k > 0 {
            cascade.push(c);
            layout.push(l);
        }
    }

    let (cmin, cmed) = min_med(&mut cascade);
    let (lmin, lmed) = min_med(&mut layout);

    println!();
    println!("one-time costs:");
    println!("  parse (HTML->DOM)   {parse_ms:>9.3} ms");
    println!("  font discovery      {font_ms:>9.3} ms  (amortized across frames)");
    println!();
    println!("per-pass, warm steady-state (n={iters}):     min     median");
    println!("  cascade           {cmin:>9.3} {cmed:>9.3} ms");
    println!("  layout total      {lmin:>9.3} {lmed:>9.3} ms");
    println!("  ----------------------------------------");
    println!("  cascade+layout    {:>9.3} {:>9.3} ms", cmin + lmin, cmed + lmed);
    println!();
    println!("cold first paint ~= parse + font discovery + cascade + layout");
    if lib_timing {
        println!("(per-phase layout split above as [layout-timing]; read the last pass)");
    } else {
        println!("(re-run with SERVAL_LAYOUT_TIMING=1 --iters=2 for the layout phase split)");
    }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// (min, median) of the samples, sorted in place.
fn min_med(v: &mut [f64]) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (v[0], v[v.len() / 2])
}

/// A synthetic, text-heavy page of roughly `target` bytes, with a real author
/// stylesheet (element / class / descendant / pseudo / nth-child selectors) so
/// the cascade does representative selector-matching work, and nested article
/// structure so box-tree build and inline shaping are exercised.
fn synthetic_page(target: usize) -> String {
    let style = r#"<style>
body { font-family: sans-serif; margin: 0 auto; max-width: 760px; color: #222; line-height: 1.6; }
h1, h2, h3 { font-weight: 700; line-height: 1.2; }
.post { padding: 1.5rem 1rem; border-bottom: 1px solid #eee; }
.post .title { font-size: 1.6rem; margin: 0 0 .5rem; }
.post .lead { font-size: 1.1rem; color: #444; }
.post p { margin: 0 0 1rem; }
.post .meta { list-style: none; padding: 0; display: flex; gap: .75rem; }
.post .meta li { font-size: .85rem; color: #888; }
.post .meta a { color: #06c; text-decoration: none; }
.post .meta a:hover { text-decoration: underline; }
.note { background: #f7f7f9; padding: .75rem 1rem; border-left: 3px solid #06c; }
.note p { margin: 0; font-size: .95rem; }
article:nth-child(odd) { background: #fcfcfc; }
article:nth-child(3n) .title { color: #084; }
blockquote { margin: 1rem 0; padding-left: 1rem; border-left: 3px solid #ccc; color: #555; font-style: italic; }
code { font-family: monospace; background: #f0f0f0; padding: .1em .3em; }
</style>"#;

    let s = "The quick brown fox jumps over the lazy dog, while a curious cascade of \
             nested boxes resolves its computed values and the box tree takes shape. ";

    let mut body = String::with_capacity(target + 8192);
    let mut i = 0usize;
    while body.len() < target {
        i += 1;
        body.push_str("<article class=\"post\">");
        body.push_str(&format!(
            "<h2 class=\"title\">Section {i}: On Layout, Cascade, and the Shape of Boxes</h2>"
        ));
        body.push_str(&format!("<p class=\"lead\">{s}{s}</p>"));
        body.push_str(&format!("<p>{s}{s}{s}</p>"));
        body.push_str(&format!(
            "<p>Consider <a href=\"#a{i}\">item {i}</a> with its <code>computed</code> values. {s}{s}</p>"
        ));
        body.push_str(&format!(
            "<ul class=\"meta\"><li>tag</li><li><a href=\"#t{i}\">topic {i}</a></li><li>{i} min read</li></ul>"
        ));
        body.push_str(&format!("<blockquote>{s}</blockquote>"));
        body.push_str(&format!("<div class=\"note\"><p>{s}{s}</p></div>"));
        body.push_str("</article>\n");
    }

    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <title>Synthetic Layout Bench</title>{style}</head>\
         <body><h1>Synthetic Layout Benchmark</h1>{body}</body></html>"
    )
}
