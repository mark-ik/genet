/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#![deny(unsafe_code)]

//! Profile-neutral layout engine for serval.
//!
//! Consumes any `LayoutDom`-shaped DOM and produces planes
//! (`StylePlane`, eventually `LayoutPlane`, `FragmentPlane`) per the
//! planes architecture in
//! `docs/2026-05-17_serval_layout_planes_architecture.md`.
//!
//! Probe slice (2026-05-17): minimum end-to-end is wired —
//! `NodeRef` (foreign-trait firewall for Stylo, draft impls in
//! `adapter_stylo.rs` deferred) + `StylePlane` (hand-built today; cascade
//! populates later) + `construct` (DOM → Taffy tree) + `taffy::compute_root_layout`
//! + `FragmentPlane` (per-node rects).

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
pub use cascade::{restyle_structural, restyle_with_snapshots, run_cascade, RestyleOutcome};
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
