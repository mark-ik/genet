/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Layout entry point.
//!
//! Runs the end-to-end pipeline over a `LayoutDom` + cascaded
//! `StylePlane`: build the box tree (Taffy's trait-impl tree over
//! `stylo_taffy::TaffyStyloStyle`), compute layout against a viewport
//! (with parley measuring text leaves + the `ImagePlane` sizing replaced
//! elements), then read per-node results back into a `FragmentPlane`.
//!
//! This is a thin wrapper over [`crate::box_tree::layout_via_box_tree`];
//! it exists so callers have a stable `layout(...)` entry point and so
//! the return type (`BoxTree`) carries the `node_map` + node-context
//! lookups paint emission needs.
//!
//! Cf. `docs/2026-05-25_box_tree_trait_impl_plan.md`,
//! `docs/2026-05-17_serval_layout_planes_architecture.md`.

use std::hash::Hash;

use layout_dom_api::LayoutDom;

use crate::box_tree::{layout_via_box_tree, BoxTree};
use crate::fragment::FragmentPlane;
use crate::image_decode::ImagePlane;
use crate::style::StylePlane;
use crate::text_measure::TextMeasureCtx;

/// Run the layout pipeline against a viewport.
///
/// Returns the per-node [`FragmentPlane`], the [`BoxTree`] (for emit's
/// `node_map` + `get_node_context` lookups), and the [`TextMeasureCtx`]
/// holding the cached `parley::Layout` per text leaf (paint emission
/// reads from here to extract positioned glyphs without re-shaping).
pub fn layout<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    viewport: taffy::Size<taffy::AvailableSpace>,
) -> (FragmentPlane<D::NodeId>, BoxTree<D::NodeId>, TextMeasureCtx)
where
    D: LayoutDom,
    // `Send + Sync` is required by the parallel shaping pre-pass in
    // `layout_via_box_tree`; the concrete DOM node ids satisfy it for free.
    D::NodeId: Copy + Eq + Hash + Send + Sync,
{
    // Stateless entry: a fresh context per call (back-compat for callers that
    // do not yet hold a persistent one). Session / host callers should hold a
    // `TextMeasureCtx` and call [`layout_via_box_tree`] to skip font discovery.
    let mut text_ctx = TextMeasureCtx::new();
    let (fragments, tree) = layout_via_box_tree(dom, styles, images, viewport, &mut text_ctx);
    (fragments, tree, text_ctx)
}

#[cfg(test)]
mod tests {
    use html5ever::local_name;
    use layout_dom_api::LayoutDom;
    use serval_static_dom::{StaticDocument, StaticNodeId};
    use taffy::prelude::*;

    use super::*;
    use crate::adapter::NodeRef;
    use crate::cascade::run_cascade;

    /// Find the first element descendant matching a local name, walking the
    /// subtree under `start`. Used to locate `<p>` etc. without depending on
    /// html5ever's auto-inserted `<head>` ordering.
    fn find_element<'a, D: LayoutDom>(
        start: NodeRef<'a, D>,
        local: html5ever::LocalName,
    ) -> Option<NodeRef<'a, D>> {
        let mut queue = vec![start];
        while let Some(node) = queue.pop() {
            if let Some(name) = node.dom().element_name(node.id()) {
                if name.local == local {
                    return Some(node);
                }
            }
            queue.extend(node.dom_children());
        }
        None
    }

    /// Cascade a fixture into a `StylePlane` (UA defaults + the given
    /// sheets), as the live pipeline does. The box tree reads
    /// `ComputedValues` directly — no Taffy-style refresh step.
    fn cascade(html: &str, sheets: &[&str]) -> (StaticDocument, StylePlane<StaticNodeId>) {
        let document = StaticDocument::parse(html);
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(&document, &mut styles, euclid::Size2D::new(800.0, 600.0), sheets, None);
        (document, styles)
    }

    fn viewport() -> Size<AvailableSpace> {
        Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        }
    }

    /// Cascade-driven font-size propagates from the parent element's
    /// `ComputedValues` into the text leaf. Sizing the parent
    /// dramatically (32px vs default 16px) makes the measured text
    /// height track the difference — parley's measured height is
    /// proportional to font-size for single-line text.
    #[test]
    fn parley_inherits_font_size_from_cascade() {
        let (doc_big, styles_big) = cascade("<html><body><p>Hello</p></body></html>", &["p { font-size: 32px; }"]);
        let (doc_def, styles_def) = cascade("<html><body><p>Hello</p></body></html>", &[]);
        let images = ImagePlane::new();

        let (frags_big, _, _) = layout(&doc_big, &styles_big, &images, viewport());
        let (frags_default, _, _) = layout(&doc_def, &styles_def, &images, viewport());

        let p_big = find_element(NodeRef::document(&doc_big), local_name!("p")).unwrap();
        let p_def = find_element(NodeRef::document(&doc_def), local_name!("p")).unwrap();
        let big = frags_big.rect_of(p_big.id()).expect("big <p> fragment");
        let default = frags_default.rect_of(p_def.id()).expect("default <p> fragment");

        assert!(
            big.size.height > default.size.height * 1.5,
            "expected 32px to produce >1.5x default (16px) text height; big={} default={}",
            big.size.height,
            default.size.height
        );
    }

    /// Cascaded `font-family` flows into the inline leaf's
    /// `InlineContent` run (read via the box tree's `get_node_context`).
    #[test]
    fn cascade_font_family_flows_into_text_leaf() {
        use crate::text_measure::{FontFamilySpec, GenericFamilyKind};

        let (document, styles) =
            cascade("<html><body><p>x</p></body></html>", &["p { font-family: monospace; }"]);
        let images = ImagePlane::new();
        let (_frags, built, _ctx) = layout(&document, &styles, &images, viewport());

        let p = find_element(NodeRef::document(&document), local_name!("p")).unwrap();
        let taffy_id = built.node_map.get(&p.id()).expect("<p> in node_map");
        let content = built
            .get_node_context(*taffy_id)
            .expect("<p> carries an InlineContent context");
        let run = content.runs.first().expect("inline content has a run");

        assert!(
            matches!(
                run.font_family,
                FontFamilySpec::Generic(GenericFamilyKind::Monospace)
            ),
            "expected monospace from cascade, got {:?}",
            run.font_family
        );
    }

    /// Inline flow: `<p>Hello <b>world</b> !</p>` gathers into one
    /// inline-context leaf whose runs carry per-element styling — the
    /// `<b>` run is bold (UA `b { font-weight: bold }`), the surrounding
    /// runs normal — and lays out on one line, not stacked.
    #[test]
    fn inline_flow_gathers_styled_runs_on_one_line() {
        let (document, styles) =
            cascade("<html><body><p>Hello <b>world</b> !</p></body></html>", &[]);
        let images = ImagePlane::new();
        let (fragments, built, _ctx) = layout(&document, &styles, &images, viewport());

        let p = find_element(NodeRef::document(&document), local_name!("p")).unwrap();
        let taffy_id = built.node_map.get(&p.id()).expect("<p> in node_map");
        let content = built.get_node_context(*taffy_id).expect("<p> carries InlineContent");

        assert!(
            content.runs.len() >= 2,
            "expected multiple runs (text + <b>), got {}",
            content.runs.len()
        );
        assert!(content.runs.iter().any(|r| r.weight < 500.0), "expected a normal-weight run");
        assert!(content.runs.iter().any(|r| r.weight >= 600.0), "expected a bold run from <b>");

        // One line: p's height is about a single line, well under two.
        let h = fragments.rect_of(p.id()).expect("<p> fragment").size.height;
        assert!(h < 40.0, "expected single-line height (<40px), got {h}");
    }

    /// A `<p>` whose only child is text establishes an inline formatting
    /// context, so it's the measured leaf; parley gives it a non-zero
    /// size and it appears in the box tree's `node_map`.
    #[test]
    fn parley_measures_inline_text() {
        let (document, styles) = cascade("<html><body><p>Hello, world!</p></body></html>", &[]);
        let images = ImagePlane::new();
        let (fragments, built, _ctx) = layout(&document, &styles, &images, viewport());

        let p = find_element(NodeRef::document(&document), local_name!("p")).unwrap();
        assert!(built.node_map.contains_key(&p.id()), "expected <p> in node_map");

        let rect = fragments.rect_of(p.id()).expect("<p> has a fragment");
        assert!(rect.size.width > 0.0, "expected positive width, got {}", rect.size.width);
        assert!(rect.size.height > 0.0, "expected positive height, got {}", rect.size.height);
    }

    /// `letter-spacing` widens the measured inline run: parley adds the spacing
    /// between characters at shape time, so the cached layout for `iiiii` is
    /// wider with `letter-spacing: 10px` than without (4 gaps, ~40px wider).
    #[test]
    fn letter_spacing_widens_measured_text() {
        let measured_width = |sheets: &[&str]| -> f32 {
            let (doc, styles) = cascade("<html><body><p>iiiii</p></body></html>", sheets);
            let images = ImagePlane::new();
            let (_frags, built, ctx) = layout(&doc, &styles, &images, viewport());
            let p = find_element(NodeRef::document(&doc), local_name!("p")).unwrap();
            let taffy_id = built.node_map.get(&p.id()).expect("<p> in node_map");
            ctx.layouts.get(taffy_id).expect("<p> cached layout").width()
        };
        let plain = measured_width(&[]);
        let spaced = measured_width(&["p { letter-spacing: 10px; }"]);
        assert!(
            spaced > plain + 30.0,
            "letter-spacing should widen measured text: plain={plain} spaced={spaced}"
        );
    }

    /// Font-relative units resolve through the real font (skrifa metrics), not
    /// Stylo's blind fallbacks: at `font-size: 100px`, `1em` is exactly 100px,
    /// `1ex` is the font's x-height, `1cap` its cap-height (taller than the
    /// x-height, shorter than the em), and `1ch` the advance of `0`. Needs system
    /// fonts present (true on the dev machines).
    #[test]
    fn font_relative_units_use_real_font_metrics() {
        let width_for = |w: &str| -> f32 {
            let css = format!("div {{ display: block; font-size: 100px; height: 1px; width: {w}; }}");
            let (doc, styles) = cascade("<html><body><div></div></body></html>", &[css.as_str()]);
            let images = ImagePlane::new();
            let (frags, _, _) = layout(&doc, &styles, &images, viewport());
            let div = find_element(NodeRef::document(&doc), local_name!("div")).unwrap();
            frags.rect_of(div.id()).expect("<div> fragment").size.width
        };

        let em = width_for("1em");
        let ex = width_for("1ex");
        let cap = width_for("1cap");
        let ch = width_for("1ch");

        assert!((em - 100.0).abs() < 0.5, "1em is exactly the font-size (100px): {em}");
        assert!(ex > 30.0 && ex < 80.0, "1ex is the real x-height (~half an em): {ex}");
        assert!(cap > ex && cap < em, "cap-height sits between x-height and em: cap={cap} ex={ex}");
        assert!(ch > 20.0 && ch < em, "1ch is the real `0` advance: {ch}");
    }

    /// Lay out `html` cascaded with `sheets` at a `w`x`h` viewport (the size both
    /// the cascade's `Device` — viewport units — and taffy's root — `%`-height —
    /// resolve against).
    fn layout_at(
        html: &str,
        sheets: &[&str],
        w: f32,
        h: f32,
    ) -> (StaticDocument, FragmentPlane<StaticNodeId>) {
        let document = StaticDocument::parse(html);
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(&document, &mut styles, euclid::Size2D::new(w, h), sheets, None);
        let images = ImagePlane::new();
        let vp = Size { width: AvailableSpace::Definite(w), height: AvailableSpace::Definite(h) };
        let (frags, _, _) = layout(&document, &styles, &images, vp);
        (document, frags)
    }

    /// Phase B — viewport units resolve against the cascade's `Device` size:
    /// `50vw`/`50vh` at an 800×600 viewport are 400×300.
    #[test]
    fn viewport_units_resolve_against_the_viewport() {
        let (doc, frags) = layout_at(
            "<html><body><div></div></body></html>",
            &["html, body, div { display: block; margin: 0; } div { width: 50vw; height: 50vh; }"],
            800.0,
            600.0,
        );
        let div = find_element(NodeRef::document(&doc), local_name!("div")).unwrap();
        let r = frags.rect_of(div.id()).expect("div fragment");
        assert!((r.size.width - 400.0).abs() < 1.0, "50vw of 800 = 400: {}", r.size.width);
        assert!((r.size.height - 300.0).abs() < 1.0, "50vh of 600 = 300: {}", r.size.height);
    }

    /// Phase B — viewport units re-resolve when the viewport changes: the same
    /// `50vw`/`50vh` div is 400×300 at 800×600 and 500×400 at 1000×800 (a resized
    /// window re-cascades against the new `Device`).
    #[test]
    fn viewport_units_reresolve_on_resize() {
        let measure = |w: f32, h: f32| {
            let (doc, frags) = layout_at(
                "<html><body><div></div></body></html>",
                &["html, body, div { display: block; margin: 0; } div { width: 50vw; height: 50vh; }"],
                w,
                h,
            );
            let div = find_element(NodeRef::document(&doc), local_name!("div")).unwrap();
            frags.rect_of(div.id()).expect("div fragment").size
        };
        let small = measure(800.0, 600.0);
        let large = measure(1000.0, 800.0);
        assert!((small.width - 400.0).abs() < 1.0, "50vw@800 = 400: {}", small.width);
        assert!((large.width - 500.0).abs() < 1.0, "50vw@1000 = 500: {}", large.width);
        assert!((large.height - 400.0).abs() < 1.0, "50vh@800 = 400: {}", large.height);
    }

    /// Phase B — the `%`-height chain: `html`/`body { height: 100% }` resolves
    /// against the initial containing block (viewport height), so both are
    /// viewport-tall. The classic element-first miss — taffy must treat the root's
    /// height basis as definite (the viewport), not indefinite.
    #[test]
    fn percent_height_chain_resolves_against_the_viewport() {
        let (doc, frags) = layout_at(
            "<html><body></body></html>",
            &["html, body { display: block; margin: 0; height: 100%; }"],
            800.0,
            600.0,
        );
        let html = find_element(NodeRef::document(&doc), local_name!("html")).unwrap();
        let body = find_element(NodeRef::document(&doc), local_name!("body")).unwrap();
        let html_h = frags.rect_of(html.id()).expect("html fragment").size.height;
        let body_h = frags.rect_of(body.id()).expect("body fragment").size.height;
        assert!((html_h - 600.0).abs() < 1.0, "html height:100% = viewport 600: {html_h}");
        assert!((body_h - 600.0).abs() < 1.0, "body height:100% = html 600: {body_h}");
    }

    /// The body box is content-height, not viewport-stretched: a short page's
    /// `<body>` (UA `height: auto`) shrinks to its content rather than filling the
    /// viewport, so its padding / margins do not overflow the root (the phantom
    /// document-scroll fix). The root `<html>` still fills the viewport.
    #[test]
    fn body_is_content_height_not_viewport_stretched() {
        let (doc, frags) = layout_at(
            "<html><body><div>x</div></body></html>",
            &["html, body, div { display: block; } body { padding: 8px; }"],
            800.0,
            600.0,
        );
        let html = find_element(NodeRef::document(&doc), local_name!("html")).unwrap();
        let body = find_element(NodeRef::document(&doc), local_name!("body")).unwrap();
        let html_h = frags.rect_of(html.id()).expect("html fragment").size.height;
        let body_h = frags.rect_of(body.id()).expect("body fragment").size.height;
        assert!((html_h - 600.0).abs() < 1.0, "the root <html> still fills the viewport: {html_h}");
        assert!(
            body_h < 100.0,
            "<body> is content-height (~one line + 16px padding), not viewport-stretched: {body_h}",
        );
    }
}
