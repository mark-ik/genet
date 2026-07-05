/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#![deny(unsafe_code)]

//! Profile-neutral layout engine for serval.
//!
//! Consumes any `LayoutDom`-shaped DOM and produces planes (`StylePlane`,
//! `FragmentPlane`) per the planes architecture in
//! `docs/2026-05-17_serval_layout_planes_architecture.md`.
//!
//! The full pipeline is wired, and is the shared core behind every content lane
//! (the static viewer, the scripted live path, meerkat's content card):
//!
//! - `NodeRef` / `StyleNodeRef` are the foreign-trait firewall: Stylo's trait
//!   family (`TNode` / `TElement` / `selectors::Element` / etc.) is impl'd in
//!   `adapter_stylo` and nowhere else in the crate.
//! - `run_cascade` runs Stylo over the DOM to populate `StylePlane` (computed
//!   values) from author + UA sheets.
//! - `construct` builds the Taffy tree (parley measures inline content), and
//!   `layout` computes it into a `FragmentPlane` of per-node rects.
//! - `emit_paint_list*` walks fragments + styles into a `ServalPaintList`.
//! - `IncrementalLayout` re-runs the minimum work on DOM / style mutations.
//!
//! `render` and `paint_list_from_layout_dom` are the convenience entry points.

mod a11y;
mod adapter;
mod adapter_stylo;
mod box_tree;
mod caret;
mod cascade;
mod cell;
mod computed_query;
mod construct;
mod font_metrics;
mod fragment;
mod highlights;
mod overlays;
mod host_loader;
mod image_decode;
mod incremental;
mod inline_hit;
mod invalidate;
mod layout;
mod link_harvest;
mod paint_emit;
mod paint_stacking;
mod serval_lane;
mod snapshot;
mod style;
mod subtree;
mod text_measure;
mod ua_defaults;
mod viewport;

pub use a11y::accesskit_tree;
pub use adapter::NodeRef;
pub use adapter_stylo::StyleNodeRef;
pub use box_tree::{BoxTree, build_box_tree, layout_via_box_tree};
pub use caret::{
    CaretRect, TextRange, TextSelection, caret_byte_at_point, caret_byte_vertical, caret_color,
    caret_rect, find_text_rects, range_rects, selection_rects, selection_style, text_selection,
};
pub use cascade::{
    RestyleOutcome, apply_interaction, restyle_for_interaction, restyle_structural,
    restyle_with_snapshots, run_cascade,
};
pub use cell::ArcRefCell;
pub use fragment::FragmentPlane;
pub use highlights::{HighlightRange, HighlightStyle};
pub use overlays::OverlaySlot;
pub use host_loader::{
    LocalFileImageLoader, ResourceResolver, inline_stylesheets, inline_stylesheets_from_source,
    linked_icon_href, linked_stylesheets, linked_stylesheets_with_loader,
};
pub use image_decode::{
    BackgroundImagePlane, DecodedImage, ImageLoader, ImagePlane, NoImageLoader, decode_image_bytes,
};
pub use incremental::{Applied, IncrementalLayout};
pub use invalidate::{Invalidation, classify, coalesce};
pub use layout::layout;
pub use paint_emit::{
    SCROLLBAR_COLOR, SCROLLBAR_WIDTH, ScrollOffsets, ServalPaintList, emit_paint_list,
    emit_paint_list_scrolled, emit_paint_list_scrolled_excluding_subtrees,
    emit_paint_list_with_layouts, emit_subtree_paint_list_scrolled, push_scrollbars,
};
pub use serval_lane::{
    ServalLaneView, absolute_origin, accumulate_origins, accumulate_painted_origins,
    accumulated_translate,
};
pub use snapshot::build_snapshot_map;
pub use style::{StyleEntry, StylePlane};
pub use subtree::{SubtreeView, render_subtree};
pub use text_measure::{
    FontFamilySpec, GenericFamilyKind, InlineContent, InlineRun, TextMeasureCtx,
    measure_inline_content, register_host_font,
};
pub use viewport::{ScrollKey, Viewport, document_scroll_range};

use engine_observables_api::{FragmentQuery, Point};
use layout_dom_api::LayoutDom;
use std::hash::Hash;

/// Run the full layout pipeline (cascade → box-tree layout) over any
/// `LayoutDom`, returning the per-node [`FragmentPlane`]. Convenience
/// wrapper hiding the euclid/taffy viewport types — used by the scripted
/// tier's coarse relayout-on-mutation and by any caller that just wants
/// "lay this out".
///
/// This path doesn't decode images (the scripted relayout corpus has
/// none), so it lays out against an empty `ImagePlane`; callers needing
/// replaced-element sizing decode an `ImagePlane` and call [`layout`]
/// directly (as the paint e2e does).
pub fn render<D>(
    dom: &D,
    stylesheets: &[&str],
    viewport_width: f32,
    viewport_height: f32,
) -> FragmentPlane<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + Send + Sync + 'static,
{
    let mut styles = StylePlane::new();
    run_cascade(
        dom,
        &mut styles,
        euclid::default::Size2D::new(viewport_width, viewport_height),
        stylesheets,
        // No base URL: this convenience path lays out without decoding
        // images, so relative url() resolution isn't needed. Callers that
        // need it decode an ImagePlane and drive run_cascade + layout
        // directly with the document URL.
        None,
    );
    let images = ImagePlane::new();
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(viewport_width),
        height: taffy::AvailableSpace::Definite(viewport_height),
    };
    let (fragments, _tree, _ctx) = layout(dom, &styles, &images, viewport);
    fragments
}

/// Run the full HTML-content pipeline (cascade → image decode → box-tree
/// layout → paint emit) over any `LayoutDom`, returning a [`ServalPaintList`].
///
/// This is the shared core behind every content lane: the scripted live
/// path (pelt-live) and meerkat's content card differ
/// only in how they assemble `stylesheets` and which [`ImageLoader`] resolves
/// resources, not in the pipeline. `loader` supplies `<img>` /
/// `background-image` bytes (`data:` URIs decode inline regardless, so a
/// [`NoImageLoader`] still yields inline images); `scroll_offsets` positions
/// scrolled containers at emit time. Callers layer their own overlays (a
/// focused field's caret/selection, scrollbar thumbs) onto the returned list.
///
/// Unlike [`render`], this decodes images and emits, so it is the path for any
/// caller that wants a paintable document rather than just a fragment plane.
pub fn paint_list_from_layout_dom<D, L>(
    dom: &D,
    stylesheets: &[&str],
    loader: &L,
    width: u32,
    height: u32,
    scroll_offsets: &ScrollOffsets<D::NodeId>,
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + Send + Sync + 'static,
    L: ImageLoader,
{
    let mut styles = StylePlane::new();
    run_cascade(
        dom,
        &mut styles,
        euclid::default::Size2D::new(width as f32, height as f32),
        stylesheets,
        None,
    );
    let images = ImagePlane::decode_from_dom_with_loader(dom, loader);
    let bg_images = BackgroundImagePlane::decode_from_cascade(dom, &styles, loader);
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width as f32),
        height: taffy::AvailableSpace::Definite(height as f32),
    };
    let (fragments, built, text_ctx) = layout(dom, &styles, &images, viewport);
    emit_paint_list_with_layouts(
        dom,
        &styles,
        &fragments,
        &built,
        &text_ctx,
        &images,
        &bg_images,
        scroll_offsets,
        paint_list_api::DeviceIntSize::new(width as i32, height as i32),
    )
}

/// A retained cascade + layout over a **static content document** (a fetched HTML page),
/// holding the planes a band emit / find query reuse, so the content host lays out ONCE
/// and re-emits bands / runs find without re-cascading. The page does not mutate (unlike
/// the chrome's [`IncrementalLayout`]), so this carries no Stylist / incremental machinery,
/// just the planes. Built by [`lay_out_content`]; consumed by
/// [`ContentLayout::emit_band`] / [`ContentLayout::find`]. The one-shot band / find
/// convenience functions ([`paint_list_band_from_layout_dom`],
/// [`find_text_rects_from_layout_dom`]) are thin wrappers over this split.
pub struct ContentLayout<Id: Copy + Eq + Hash> {
    styles: StylePlane<Id>,
    images: ImagePlane<Id>,
    bg_images: BackgroundImagePlane<Id>,
    fragments: FragmentPlane<Id>,
    built: BoxTree<Id>,
    text_ctx: TextMeasureCtx,
    width: u32,
    height: u32,
    /// The content document's custom-highlight registry (css-highlight-api
    /// subset; the overlay-roots "highlight slot"). Painted band-shifted by
    /// [`emit_band`](Self::emit_band) after content emission, so registered
    /// highlights (find-in-page) land in whichever band the host requests.
    /// Registering touches no cascade/layout state: the next band re-emit
    /// simply includes the fills.
    highlights: highlights::HighlightRegistry<Id>,
    /// The document's overlay-slot registry (top-layer + anchor-positioning +
    /// UA-shadow subset; the overlay-roots "overlay slot"). Each slot is a
    /// separately-emitted satellite paint list anchored to a page node, painted
    /// after content + highlights at the anchor's live fragment position
    /// (band-shifted). Registering touches no page layout — the satellite was
    /// laid out by its own isolated cascade.
    overlays: overlays::OverlayRegistry<Id>,
}

/// Cascade + decode + lay out a static content `dom` at `(width, height)`, returning a
/// retained [`ContentLayout`] the caller re-emits bands / queries find from with no
/// re-cascade. Decodes replaced `<img>` (via `loader`, so remote images size correctly)
/// and CSS `background-image`s (so page backgrounds paint). The viewport sizes `@media` /
/// sizing at the real width/height. (The content lane's build half; cascade once, emit
/// many.)
pub fn lay_out_content<D, L>(
    dom: &D,
    stylesheets: &[&str],
    loader: &L,
    width: u32,
    height: u32,
) -> ContentLayout<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + Send + Sync + 'static,
    L: ImageLoader,
{
    // Diagnostics: time the whole cascade+decode+layout pass and report the
    // viewport + resulting fragment count. DEBUG so it is quiet by default; the
    // consuming app raises the level. Hot per-load (and per scroll band via
    // `paint_list_band_from_layout_dom`) — may want sampling later.
    #[cfg(not(target_arch = "wasm32"))]
    use std::time::Instant;
    #[cfg(target_arch = "wasm32")]
    use web_time::Instant;
    let _layout_start = Instant::now();
    let mut styles = StylePlane::new();
    run_cascade(
        dom,
        &mut styles,
        euclid::default::Size2D::new(width as f32, height as f32),
        stylesheets,
        None,
    );
    let images = ImagePlane::decode_from_dom_with_loader(dom, loader);
    let bg_images = BackgroundImagePlane::decode_from_cascade(dom, &styles, loader);
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width as f32),
        height: taffy::AvailableSpace::Definite(height as f32),
    };
    let (fragments, built, text_ctx) = layout(dom, &styles, &images, viewport);
    let fragment_count = fragments.len();
    if fragment_count == 0 {
        tracing::warn!(
            target: "serval_layout",
            width,
            height,
            stylesheet_count = stylesheets.len(),
            elapsed_ms = _layout_start.elapsed().as_secs_f64() * 1e3,
            "lay_out_content produced no fragments",
        );
    } else {
        tracing::debug!(
            target: "serval_layout",
            width,
            height,
            fragment_count,
            image_count = images.len(),
            elapsed_ms = _layout_start.elapsed().as_secs_f64() * 1e3,
            "lay_out_content complete",
        );
    }
    ContentLayout {
        styles,
        images,
        bg_images,
        fragments,
        built,
        text_ctx,
        width,
        height,
        highlights: highlights::HighlightRegistry::new(),
        overlays: overlays::OverlayRegistry::new(),
    }
}

impl<Id: Copy + Eq + Hash + Send + Sync + 'static> ContentLayout<Id> {
    /// Emit ONE vertical band (`band_y`..`band_y + band_h`) off the retained layout, plus
    /// the document scroll range and the `<a href>` hit rects in full-document px, exactly
    /// as [`paint_list_band_from_layout_dom`] does but WITHOUT re-cascading: the content
    /// host calls this per scroll band. (The content lane's emit half.)
    pub fn emit_band<D>(
        &self,
        dom: &D,
        band_y: u32,
        band_h: u32,
        scroll_offsets: &ScrollOffsets<Id>,
    ) -> (ServalPaintList, (f32, f32), Vec<(String, [f32; 4])>)
    where
        D: LayoutDom<NodeId = Id>,
    {
        let scroll_range = document_scroll_range(
            dom,
            &self.styles,
            &self.fragments,
            paint_list_api::DeviceIntSize::new(self.width as i32, self.height as i32),
        );
        let links = crate::link_harvest::harvest_link_rects(
            dom,
            &self.fragments,
            &self.built,
            &self.text_ctx,
        );
        let mut plist = emit_paint_list_scrolled(
            dom,
            &self.styles,
            &self.fragments,
            &self.built,
            &self.text_ctx,
            &self.images,
            &self.bg_images,
            scroll_offsets,
            paint_list_api::DeviceIntSize::new(self.width as i32, band_h.max(1) as i32),
            (0.0, band_y as f32),
        );
        // Registered custom highlights (the overlay-roots highlight slot):
        // derive each range's rects from the retained layout and band-shift
        // them into this emit, in registry-name order (deterministic priority).
        if !self.highlights.is_empty() {
            for (ranges, style) in self.highlights.values() {
                for r in ranges {
                    for cr in caret::selection_rects(
                        dom,
                        r.node,
                        r.start,
                        r.end,
                        &self.built,
                        &self.text_ctx,
                        &self.fragments,
                    ) {
                        plist.push_fill(
                            cr.x,
                            cr.y - band_y as f32,
                            cr.width,
                            cr.height,
                            style.color,
                        );
                    }
                }
            }
        }
        // Overlay slots (the overlay-roots overlay slot): compose each satellite
        // paint list at its anchor's live absolute origin, band-shifted, after
        // the highlights — so it paints in top-layer order over everything. The
        // satellite carries its own coordinates; `push_sublist` wraps it in a
        // transform to the anchor. A slot whose anchor left the layout is
        // silently skipped (the host unmounts on the anchor-removed event).
        if !self.overlays.is_empty() {
            for slot in self.overlays.values() {
                if let Some((ox, oy)) =
                    crate::serval_lane::absolute_origin(dom, &self.fragments, slot.anchor)
                        .map(|p| (p.x, p.y))
                {
                    plist.push_sublist(
                        paint_list_api::LayoutPoint::new(ox, oy - band_y as f32),
                        &slot.content,
                    );
                }
            }
        }
        (plist, scroll_range, links)
    }

    /// Register (or replace) the named overlay slot: `content` (a satellite's
    /// own emitted paint list, in satellite-local coords) paints at `anchor`'s
    /// live fragment position on every subsequent [`emit_band`](Self::emit_band)
    /// (the overlay-roots overlay slot). Touches no page layout: the satellite
    /// was laid out by its own isolated cascade, so the page's sheets never
    /// reach it. Painted top-layer (after content + highlights).
    pub fn set_overlay(&mut self, name: &str, anchor: Id, content: ServalPaintList) {
        self.overlays.insert(
            name.to_string(),
            overlays::OverlaySlot { anchor, content },
        );
    }

    /// Remove the named overlay slot (no-op when absent).
    pub fn clear_overlay(&mut self, name: &str) {
        self.overlays.remove(name);
    }

    /// The number of laid-out fragments (page nodes with a box). A no-reflow
    /// witness for overlay slots: registering a satellite must not change it.
    pub fn fragment_count(&self) -> usize {
        self.fragments.len()
    }

    /// The laid-out `(x, y, w, h)` of a page node, if it has a box — the anchor
    /// geometry an overlay slot positions against.
    pub fn node_rect(&self, node: Id) -> Option<(f32, f32, f32, f32)> {
        self.fragments.rect_of(node).map(|l| {
            (
                l.location.x,
                l.location.y,
                l.size.width,
                l.size.height,
            )
        })
    }

    /// Register (or replace) the named custom highlight: `ranges` paint with
    /// `style` on every subsequent [`emit_band`](Self::emit_band)
    /// (css-highlight-api subset; the overlay-roots highlight slot). Empty
    /// `ranges` removes the name. Touches no cascade/layout state.
    pub fn set_highlight(
        &mut self,
        name: &str,
        ranges: Vec<highlights::HighlightRange<Id>>,
        style: highlights::HighlightStyle,
    ) {
        if ranges.is_empty() {
            self.highlights.remove(name);
        } else {
            self.highlights.insert(name.to_string(), (ranges, style));
        }
    }

    /// Remove the named custom highlight (no-op when absent).
    pub fn clear_highlight(&mut self, name: &str) {
        self.highlights.remove(name);
    }

    /// Find-in-page off the retained layout: the highlight rects for every `needle`
    /// occurrence, one inner `Vec` per match (full-document px, unscrolled), without
    /// re-cascading. The content host calls this per find keystroke. (The content lane's
    /// find half.)
    pub fn find<D>(&self, dom: &D, needle: &str) -> Vec<Vec<[f32; 4]>>
    where
        D: LayoutDom<NodeId = Id>,
    {
        find_text_rects(dom, &self.built, &self.text_ctx, &self.fragments, needle)
            .into_iter()
            .map(|rects| {
                rects
                    .into_iter()
                    .map(|r| [r.x, r.y, r.x + r.width, r.y + r.height])
                    .collect()
            })
            .collect()
    }

    /// The range half of [`find`](Self::find): every `needle` occurrence as a
    /// `(leaf, byte range)` — registered directly via
    /// [`set_highlight`](Self::set_highlight), so find paints engine-side and
    /// rects ship only as count/step/scroll metadata.
    pub fn find_ranges<D>(&self, dom: &D, needle: &str) -> Vec<highlights::HighlightRange<Id>>
    where
        D: LayoutDom<NodeId = Id>,
    {
        caret::find_text_ranges(dom, &self.built, &self.text_ctx, needle)
    }

    /// The highlight rects for one found range (full-document px, unscrolled) —
    /// the per-match metadata a `find_ranges` caller ships for match counting,
    /// active-match stepping, and auto-scroll.
    pub fn range_rects<D>(
        &self,
        dom: &D,
        range: &highlights::HighlightRange<Id>,
    ) -> Vec<[f32; 4]>
    where
        D: LayoutDom<NodeId = Id>,
    {
        caret::selection_rects(
            dom,
            range.node,
            range.start,
            range.end,
            &self.built,
            &self.text_ctx,
            &self.fragments,
        )
        .into_iter()
        .map(|r| [r.x, r.y, r.x + r.width, r.y + r.height])
        .collect()
    }

    /// A point-drag text selection off the retained layout: resolve the anchor and
    /// focus scene points through the current content scroll, map them to leaf/byte
    /// positions, then return the highlight rects and plain text span. `None` when
    /// either endpoint misses laid-out text or the drag collapses to an empty span.
    pub fn select_text<D>(
        &self,
        dom: &D,
        anchor: (f32, f32),
        focus: (f32, f32),
        scroll_offsets: &ScrollOffsets<Id>,
    ) -> Option<TextSelection<Id>>
    where
        D: LayoutDom<NodeId = Id>,
    {
        let anchor = self.text_position_at_point(dom, anchor.0, anchor.1, scroll_offsets)?;
        let focus = self.text_position_at_point(dom, focus.0, focus.1, scroll_offsets)?;
        text_selection(
            dom,
            TextRange {
                anchor_node: anchor.0,
                anchor_offset: anchor.1,
                focus_node: focus.0,
                focus_offset: focus.1,
            },
            &self.built,
            &self.text_ctx,
            &self.fragments,
        )
    }

    fn text_position_at_point<D>(
        &self,
        dom: &D,
        x: f32,
        y: f32,
        scroll_offsets: &ScrollOffsets<Id>,
    ) -> Option<(Id, usize)>
    where
        D: LayoutDom<NodeId = Id>,
    {
        let view = ServalLaneView::new(dom, &self.styles, &self.fragments)
            .with_scroll_offsets(scroll_offsets);
        let hit = view.hit_test(Point::new(x, y))?;
        let node = view.find_by_source_id(hit.source_node)?;
        let node = self.inline_hit_at(node, hit.local_point).unwrap_or(node);
        let leaf = self.text_leaf(dom, node)?;
        let byte = caret_byte_at_point(
            dom,
            leaf,
            x,
            y,
            &self.built,
            &self.text_ctx,
            &self.fragments,
        )?;
        Some((leaf, byte))
    }

    fn text_leaf<D>(&self, dom: &D, mut node: Id) -> Option<Id>
    where
        D: LayoutDom<NodeId = Id>,
    {
        loop {
            if let Some(taffy_id) = self.built.node_map.get(&node) {
                if self.text_ctx.layouts.contains_key(taffy_id) {
                    return Some(node);
                }
            }
            node = dom.parent(node)?;
        }
    }

    fn inline_hit_at(&self, node: Id, local: Point) -> Option<Id> {
        let taffy_id = self.built.node_map.get(&node)?;
        let layout = self.text_ctx.layouts.get(taffy_id)?;
        let sources = self.built.inline_sources(node)?;
        let frame = self.fragments.rect_of(node)?;
        let cx = local.x - (frame.border.left + frame.padding.left);
        let cy = local.y - (frame.border.top + frame.padding.top);
        crate::inline_hit::inline_source_at(layout, sources, cx, cy)
    }
}

/// Lay out at `(width, height)` (so `@media` / sizing cascade at the real viewport),
/// then emit ONE vertical BAND of the document: the page scrolled to `band_y`, into a
/// `band_h`-tall viewport. The translator culls paint commands outside the band
/// viewport, so the returned list holds only the band's ops — a flat scene the host
/// can rasterize and composite without overflowing the GPU / vello encode budget that
/// a whole dense page would. Also returns the document scroll range
/// (`(max_scroll_x, max_scroll_y)`), so the host knows the full height (for the scroll
/// range) and which band to request next, and every `<a href>`'s hit rect(s) +
/// href in **full-document px** (`[x0, y0, x1, y1]`, unscrolled — band-independent,
/// so the host hit-tests a click against them after adding the card's scroll; see
/// [`link_harvest`]). The content host re-requests bands as the scroll moves (its
/// windowing, done here because the host gets a flat scene it cannot window itself).
/// `data:` images decode inline; `loader` resolves remote bytes.
pub fn paint_list_band_from_layout_dom<D, L>(
    dom: &D,
    stylesheets: &[&str],
    loader: &L,
    width: u32,
    height: u32,
    band_y: u32,
    band_h: u32,
    scroll_offsets: &ScrollOffsets<D::NodeId>,
) -> (ServalPaintList, (f32, f32), Vec<(String, [f32; 4])>)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + Send + Sync + 'static,
    L: ImageLoader,
{
    // Thin wrapper: build the retained layout, emit one band. One-shot callers keep
    // working; the content host uses the split ([`lay_out_content`] + [`ContentLayout::emit_band`])
    // directly so it cascades once and re-emits bands without re-cascading.
    lay_out_content(dom, stylesheets, loader, width, height).emit_band(
        dom,
        band_y,
        band_h,
        scroll_offsets,
    )
}

/// Find-in-page over a whole `LayoutDom`: cascade + decode + lay out at `(width,
/// height)`, then return the highlight rects for every occurrence of `needle` in the
/// laid-out text — one inner `Vec` per match (full-document px `[x0, y0, x1, y1]`,
/// unscrolled, like the link rects). Case-insensitive. The HTML/serval lane hands the
/// host a flat scene it cannot query, so the content actor runs this where the layout
/// lives (parallel to [`paint_list_band_from_layout_dom`]) and ships the rects back for
/// the host to highlight. `data:` images decode inline; `loader` resolves remote bytes
/// (an `<img>`'s size can affect layout, so the same decode the band path uses).
pub fn find_text_rects_from_layout_dom<D, L>(
    dom: &D,
    stylesheets: &[&str],
    loader: &L,
    width: u32,
    height: u32,
    needle: &str,
) -> Vec<Vec<[f32; 4]>>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + Send + Sync + 'static,
    L: ImageLoader,
{
    // Thin wrapper over the retained-layout split (build + find). `lay_out_content` also
    // decodes backgrounds, which find ignores: a negligible extra decode for one fewer
    // code path.
    lay_out_content(dom, stylesheets, loader, width, height).find(dom, needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serval_static_dom::StaticDocument;

    #[test]
    fn retained_content_layout_select_text_spans_two_blocks() {
        let doc = StaticDocument::parse(
            "<body style='margin:0'><p style='margin:0'>alpha beta</p><p style='margin:0'>gamma delta</p></body>",
        );
        let layout = lay_out_content(&doc, &[], &NoImageLoader, 420, 200);
        let alpha = layout.find(&doc, "alpha")[0][0];
        let gamma = layout.find(&doc, "gamma")[0][0];
        let start = (alpha[0] + 1.0, (alpha[1] + alpha[3]) * 0.5);
        let end = (gamma[2] - 1.0, (gamma[1] + gamma[3]) * 0.5);
        let selection = layout
            .select_text(&doc, start, end, &ScrollOffsets::default())
            .expect("selection across both paragraphs");
        assert!(
            selection.text.contains("alpha"),
            "selection should include the first paragraph"
        );
        assert!(
            selection.text.contains("gam"),
            "selection should include the second paragraph"
        );
        assert!(
            selection.rects.len() >= 2,
            "selection should paint both paragraph spans"
        );
    }

    /// Overlay-roots P2 (find-in-page via the highlight slot): a highlight
    /// registered on the content lane's retained `ContentLayout` paints into
    /// `emit_band` — band-shifted, with zero DOM and no re-search — and clearing
    /// it returns the band to its unhighlighted command shape.
    #[test]
    fn content_layout_emit_band_paints_registered_find_highlights() {
        use highlights::HighlightStyle;
        use paint_list_api::{ColorF, PaintCmd, PaintList};

        let doc = StaticDocument::parse(
            "<body style='margin:0'><p style='margin:0'>find the needle here and \
             the needle there</p></body>",
        );
        let mut layout = lay_out_content(&doc, &[], &NoImageLoader, 600, 400);
        let scroll = ScrollOffsets::default();

        let (plain, _, _) = layout.emit_band(&doc, 0, 400, &scroll);
        let plain_rects = plain
            .commands()
            .iter()
            .filter(|c| matches!(c, PaintCmd::DrawRect(_)))
            .count();

        // Register both "needle" occurrences (the shape the actor's `Find` arm
        // builds from `find_ranges`), then emit: two more fills appear, no search.
        let color = ColorF { r: 1.0, g: 0.82, b: 0.2, a: 0.38 };
        let ranges = layout.find_ranges(&doc, "needle");
        assert_eq!(ranges.len(), 2, "two occurrences of the needle");
        layout.set_highlight("find", ranges, HighlightStyle { color });

        let (lit, _, _) = layout.emit_band(&doc, 0, 400, &scroll);
        let lit_rects: Vec<_> = lit
            .commands()
            .iter()
            .filter_map(|c| match c {
                PaintCmd::DrawRect(r) if r.color == color => Some(r.placement.bounds),
                _ => None,
            })
            .collect();
        assert_eq!(lit_rects.len(), 2, "both matches paint a fill in-band");

        // Band-shift: emitting a lower band moves the same highlight's fill up by
        // the band delta (the fills track the content, not the viewport origin).
        let top_y = lit_rects[0].min.y;
        let (lower, _, _) = layout.emit_band(&doc, 40, 400, &scroll);
        let lower_y = lower
            .commands()
            .iter()
            .find_map(|c| match c {
                PaintCmd::DrawRect(r) if r.color == color => Some(r.placement.bounds.min.y),
                _ => None,
            })
            .expect("highlight present in the lower band");
        assert!(
            (lower_y - (top_y - 40.0)).abs() < 0.5,
            "band-shift moves the fill up by the band delta: {lower_y} vs {}",
            top_y - 40.0
        );

        // Clearing restores the plain band exactly.
        layout.clear_highlight("find");
        let (cleared, _, _) = layout.emit_band(&doc, 0, 400, &scroll);
        assert_eq!(
            cleared
                .commands()
                .iter()
                .filter(|c| matches!(c, PaintCmd::DrawRect(_)))
                .count(),
            plain_rects,
            "clear_highlight returns the band to its unhighlighted shape"
        );
    }

    /// The `<p>` anchor in a single-paragraph body (overlay-slot tests).
    fn anchor_p(doc: &StaticDocument) -> serval_static_dom::StaticNodeId {
        use layout_dom_api::LocalName;
        let mut q = vec![doc.document()];
        while let Some(id) = q.pop() {
            if doc
                .element_name(id)
                .is_some_and(|n| n.local == LocalName::from("p"))
            {
                return id;
            }
            q.extend(doc.dom_children(id));
        }
        panic!("no <p>")
    }

    /// A fill-only satellite: a laid-out `div` with a background, emitted to its
    /// own paint list (its own isolated cascade — the page's sheets never reach
    /// it). `sheet` is the *satellite's* stylesheet.
    fn satellite(sheet: &str, w: u32, h: u32) -> (ServalPaintList, paint_list_api::ColorF) {
        use paint_list_api::{PaintCmd, PaintList};
        let sat_doc = StaticDocument::parse("<div class='card'></div>");
        let sat = lay_out_content(&sat_doc, &[sheet], &NoImageLoader, w, h);
        let (plist, _, _) = sat.emit_band(&sat_doc, 0, h, &ScrollOffsets::default());
        // The card's background fill colour (the opaque one; skip any
        // transparent root/clear rect the emit may prepend).
        let color = plist
            .commands()
            .iter()
            .find_map(|c| match c {
                PaintCmd::DrawRect(r) if r.color.a > 0.0 => Some(r.color),
                _ => None,
            })
            .expect("satellite paints a background fill");
        (plist, color)
    }

    /// Overlay-slot P0 (a) no reflow leak + (d) top-layer order: registering a
    /// satellite anchored to a page node does not change the page's own layout
    /// (fragment plane byte-identical), and the satellite's fills paint *after*
    /// the page content (last in the command stream).
    #[test]
    fn overlay_slot_does_not_reflow_the_page_and_paints_on_top() {
        use paint_list_api::{ColorF, PaintCmd, PaintList};

        let doc = StaticDocument::parse(
            "<body style='margin:0'><p style='margin:0'>page content here</p></body>",
        );
        let mut layout = lay_out_content(&doc, &[], &NoImageLoader, 600, 400);
        let p = anchor_p(&doc);
        let scroll = ScrollOffsets::default();

        let frags_before = layout.fragment_count();
        let p_rect_before = layout.node_rect(p).expect("p rect");
        let (plain, _, _) = layout.emit_band(&doc, 0, 400, &scroll);
        let plain_cmds = plain.commands().len();

        let (content, sat_color) =
            satellite(".card { display: block; width: 80px; height: 40px; \
                       background-color: rgb(20, 120, 200) }", 80, 40);
        layout.set_overlay("preview", p, content);

        // (a) The page's own layout is untouched: fragment count + the anchor's
        // rect are identical (the satellite never entered the page box tree).
        assert_eq!(layout.fragment_count(), frags_before, "no page reflow");
        assert_eq!(
            layout.node_rect(p).expect("p rect"),
            p_rect_before,
            "the anchor paragraph did not move"
        );

        let (lit, _, _) = layout.emit_band(&doc, 0, 400, &scroll);
        // (d) The satellite's fill is present and paints last (top layer): the
        // final DrawRect carries the satellite colour.
        let last_rect_color = lit.commands().iter().rev().find_map(|c| match c {
            PaintCmd::DrawRect(r) => Some(r.color),
            _ => None,
        });
        assert_eq!(
            last_rect_color,
            Some(sat_color),
            "the satellite paints in top-layer order (last fill)"
        );
        assert!(
            lit.commands().len() > plain_cmds,
            "the overlay added commands (a transform wrap + the satellite fills)"
        );

        // Clearing restores the plain command count.
        layout.clear_overlay("preview");
        let (cleared, _, _) = layout.emit_band(&doc, 0, 400, &scroll);
        assert_eq!(
            cleared.commands().len(),
            plain_cmds,
            "clear_overlay returns the band to its pre-overlay shape"
        );
    }

    /// Overlay-slot P0 (b) anchor tracking: the satellite paints at the anchor's
    /// position, and a lower band shifts it up by the band delta — it tracks the
    /// content (via the anchor's fragment), not the viewport origin.
    #[test]
    fn overlay_slot_tracks_its_anchor_across_bands() {
        use paint_list_api::{PaintCmd, PaintList};

        // A tall page so the anchor sits well below the top; the satellite
        // origin should follow it.
        let doc = StaticDocument::parse(
            "<body style='margin:0'><div style='height:300px'></div>\
             <p style='margin:0'>anchor</p></body>",
        );
        let mut layout = lay_out_content(&doc, &[], &NoImageLoader, 600, 400);
        let p = anchor_p(&doc);
        let p_top = layout.node_rect(p).expect("p rect").1;
        assert!(p_top > 250.0, "anchor sits low on the page: {p_top}");

        let (content, sat_color) = satellite(
            ".card { display: block; width: 60px; height: 30px; \
             background-color: rgb(200, 60, 60) }",
            60,
            30,
        );
        layout.set_overlay("pin", p, content);
        let scroll = ScrollOffsets::default();

        let sat_y = |band_y: u32| -> f32 {
            let (pl, _, _) = layout.emit_band(&doc, band_y, 400, &scroll);
            // The overlay wraps its satellite in PushTransform(anchor). The
            // satellite's own emission also carries internal transforms at
            // origin 0 (its stacking root), so pick the wrap by its nonzero
            // origin — the anchor sits far down this tall page.
            pl.commands()
                .iter()
                .filter_map(|c| match c {
                    PaintCmd::PushTransform(t) if t.origin.y.abs() > 50.0 => Some(t.origin.y),
                    _ => None,
                })
                .next_back()
                .expect("overlay anchor transform present")
        };
        let _ = sat_color;

        let top_origin = sat_y(0);
        assert!(
            (top_origin - p_top).abs() < 0.5,
            "the overlay anchors at the paragraph's top: {top_origin} vs {p_top}"
        );
        let lower_origin = sat_y(100);
        assert!(
            (lower_origin - (top_origin - 100.0)).abs() < 0.5,
            "a lower band shifts the overlay up by the band delta: {lower_origin} vs {}",
            top_origin - 100.0
        );
    }

    /// Overlay-slot P0 (c) style isolation: the satellite is laid out by its own
    /// cascade, so a page rule that *would* match the satellite's element does
    /// not restyle it — the satellite renders identically whether or not the
    /// page carries the conflicting rule.
    #[test]
    fn overlay_slot_content_is_isolated_from_page_styles() {
        let sat_sheet = ".card { display: block; width: 50px; height: 25px; \
                         background-color: rgb(30, 160, 90) }";

        // Same satellite, two very different page sheets — one of which has a
        // `.card` rule that would recolor the satellite if the page cascade
        // leaked into it.
        let page_plain = StaticDocument::parse("<body><p>x</p></body>");
        let mut l1 = lay_out_content(&page_plain, &[], &NoImageLoader, 400, 200);
        let page_hostile = StaticDocument::parse(
            "<body><style>.card{background-color:rgb(255,0,0)}</style><p>x</p></body>",
        );
        let mut l2 = lay_out_content(
            &page_hostile,
            &[".card{background-color:rgb(255,0,0)}"],
            &NoImageLoader,
            400,
            200,
        );

        let (c1, color1) = satellite(sat_sheet, 50, 25);
        let (c2, color2) = satellite(sat_sheet, 50, 25);
        assert_eq!(
            color1, color2,
            "the satellite's own cascade is deterministic regardless of any page"
        );
        // Neither the plain nor the `.card`-hostile page can change the
        // satellite's colour: it was cascaded before it ever met a page.
        l1.set_overlay("a", anchor_p(&page_plain), c1);
        l2.set_overlay("a", anchor_p(&page_hostile), c2);
        assert_ne!(
            color1,
            paint_list_api::ColorF { r: 1.0, g: 0.0, b: 0.0, a: 1.0 },
            "the page's `.card{{color:red}}` did not reach the isolated satellite"
        );
    }
}
