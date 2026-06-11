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

mod adapter;
mod adapter_stylo;
mod box_tree;
mod caret;
mod cascade;
mod cell;
mod construct;
mod font_metrics;
mod fragment;
mod host_loader;
mod image_decode;
mod incremental;
mod invalidate;
mod layout;
mod paint_emit;
mod paint_stacking;
mod serval_lane;
mod snapshot;
mod style;
mod subtree;
mod text_measure;
mod ua_defaults;

pub use adapter::NodeRef;
pub use adapter_stylo::StyleNodeRef;
pub use box_tree::{build_box_tree, layout_via_box_tree, BoxTree};
pub use caret::{
    caret_byte_at_point, caret_byte_vertical, caret_rect, selection_rects, CaretRect,
};
pub use cascade::{
    apply_interaction, restyle_for_interaction, restyle_structural, restyle_with_snapshots,
    run_cascade, RestyleOutcome,
};
pub use cell::ArcRefCell;
pub use fragment::FragmentPlane;
pub use host_loader::{
    inline_stylesheets, inline_stylesheets_from_source, linked_stylesheets,
    linked_stylesheets_with_loader, LocalFileImageLoader, ResourceResolver,
};
pub use incremental::{Applied, IncrementalLayout};
pub use image_decode::{
    BackgroundImagePlane, DecodedImage, ImageLoader, ImagePlane, NoImageLoader,
};
pub use invalidate::{classify, coalesce, Invalidation};
pub use layout::layout;
pub use paint_emit::{
    emit_paint_list, emit_paint_list_with_layouts, ScrollOffsets, ServalPaintList,
};
pub use serval_lane::ServalLaneView;
pub use snapshot::build_snapshot_map;
pub use style::{StyleEntry, StylePlane};
pub use subtree::{render_subtree, SubtreeView};
pub use text_measure::{
    measure_inline_content, FontFamilySpec, GenericFamilyKind, InlineContent, InlineRun,
    TextMeasureCtx,
};

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
    D::NodeId: Copy + Eq + Hash + 'static,
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
/// This is the shared core behind every content lane: the static viewer
/// (`pelt-viewer`), the scripted live path, and meerkat's content card differ
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
    D::NodeId: Copy + Eq + Hash + 'static,
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
