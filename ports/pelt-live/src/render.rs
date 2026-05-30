/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `ScriptedDom` → `netrender::Scene`.
//!
//! A focused mirror of [`pelt-viewer`'s `build_scene`](../../pelt-viewer/render.rs),
//! but pointed at a live, mutable [`ScriptedDom`] instead of parsing HTML: the
//! serval engine pipeline (cascade → layout → emit) runs over the same DOM that
//! [`ServalAppRunner`](xilem_serval::ServalAppRunner) diffs. This is the render
//! half of Stage 1b — state change → DOM diff → serval layout/paint, offline.
//!
//! `run_cascade` / `layout` / `emit_paint_list_with_layouts` are all generic
//! over `D: LayoutDom`, and `ScriptedDom` is a `LayoutDom`, so this compiles for
//! the scripted DOM exactly as it does for the static one. The image/fetcher
//! paths the static viewer carries are dropped: a counter has no `<img>` and no
//! `background-image`, so we lay out against an empty
//! [`ImagePlane`]/[`BackgroundImagePlane`].
//!
//! GPU-free by construction (no wgpu): scene *production* and presentation stay
//! separable, and the test driver asserts on the produced `Scene`/layout
//! without a window.

use engine_observables_api::{FragmentQuery, Point};
use paint_list_api::{ColorF, DeviceIntSize};
use serval_layout::{
    BackgroundImagePlane, FragmentPlane, ImagePlane, ServalLaneView, StylePlane, caret_rect,
    emit_paint_list_with_layouts, layout, run_cascade,
};
use serval_scripted_dom::{NodeId, ScriptedDom};

/// Caret bar thickness, device px.
const CARET_WIDTH: f32 = 2.0;
/// Caret bar colour (near-black, opaque).
const CARET_COLOR: ColorF = ColorF { r: 0.12, g: 0.12, b: 0.20, a: 1.0 };

/// Run cascade → layout → paint-emit over `dom` and translate the paint list to
/// a [`netrender::Scene`] at `width`×`height`.
///
/// `stylesheets` are author CSS applied on top of serval's UA defaults. Unlike
/// the static viewer there is no inline `<style>` / `<link>` collection: the
/// chrome DOM the runner builds carries no document-embedded stylesheets, so the
/// caller's sheets are the whole author set.
///
/// `caret` is `Some((node, byte_offset))` to paint a text caret at that offset
/// within `node`'s laid-out text — typically the focused field's element and its
/// cursor position. Drawn as a thin filled bar via
/// [`serval_layout::caret_rect`], appended after the layout walk (absolute
/// coords). `None` paints no caret.
pub fn scene_from_scripted_dom(
    dom: &ScriptedDom,
    stylesheets: &[&str],
    width: u32,
    height: u32,
    caret: Option<(NodeId, usize)>,
) -> netrender::Scene {
    let mut styles: StylePlane<NodeId> = StylePlane::new();
    run_cascade(
        dom,
        &mut styles,
        euclid::Size2D::new(width as f32, height as f32),
        stylesheets,
    );

    // A counter has no replaced content and no CSS backgrounds, so both image
    // planes are empty (the box tree's replaced-leaf sizing reads nothing).
    let images = ImagePlane::new();
    let bg_images = BackgroundImagePlane::new();

    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width as f32),
        height: taffy::AvailableSpace::Definite(height as f32),
    };
    let (fragments, built, text_ctx) = layout(dom, &styles, &images, viewport);
    let mut plist = emit_paint_list_with_layouts(
        dom,
        &styles,
        &fragments,
        &built,
        &text_ctx,
        &images,
        &bg_images,
        DeviceIntSize::new(width as i32, height as i32),
    );

    // Overlay the caret (if any) as a thin bar at its absolute position. Appended
    // after emit, so it draws over the text at scene coordinates.
    if let Some((node, byte_offset)) = caret {
        if let Some(rect) =
            caret_rect(dom, node, byte_offset, &built, &text_ctx, &fragments, CARET_WIDTH)
        {
            plist.push_caret(rect, CARET_COLOR);
        }
    }

    paint::translate_paint_list(&plist)
}

/// Run only the cascade → layout half (no paint emission) over `dom`, returning
/// the per-node [`FragmentPlane`].
///
/// The layout-level companion to [`scene_from_scripted_dom`]: it lets the test
/// driver assert that a node was reached by layout (`rect_of(node).is_some()`)
/// independent of paint emission, which is the plan's fallback assertion level.
pub fn fragments_from_scripted_dom(
    dom: &ScriptedDom,
    stylesheets: &[&str],
    width: u32,
    height: u32,
) -> FragmentPlane<NodeId> {
    let mut styles: StylePlane<NodeId> = StylePlane::new();
    run_cascade(
        dom,
        &mut styles,
        euclid::Size2D::new(width as f32, height as f32),
        stylesheets,
    );
    let images = ImagePlane::new();
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width as f32),
        height: taffy::AvailableSpace::Definite(height as f32),
    };
    let (fragments, _built, _text_ctx) = layout(dom, &styles, &images, viewport);
    fragments
}

/// Lay out `dom` and hit-test the point `(x, y)`, returning the topmost
/// (paint-order) node containing it — the `point → NodeId` half of input
/// dispatch (Stage 2a). `None` if the point falls outside every fragment.
///
/// This consumes serval's existing query surface
/// ([`ServalLaneView::hit_test`], part of `engine_observables_api`) rather than
/// adding a new spatial index. The reverse `SourceNodeId → NodeId` is trivial
/// here: `ScriptedDom::opaque_id(id)` is just `id`'s raw arena index, so
/// [`NodeId::from_raw`] inverts it directly (no O(n) walk like the generic
/// `ServalLaneView::find_by_source_id`).
pub fn hit_test_node(
    dom: &ScriptedDom,
    stylesheets: &[&str],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
) -> Option<NodeId> {
    let mut styles: StylePlane<NodeId> = StylePlane::new();
    run_cascade(
        dom,
        &mut styles,
        euclid::Size2D::new(width as f32, height as f32),
        stylesheets,
    );
    let images = ImagePlane::new();
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width as f32),
        height: taffy::AvailableSpace::Definite(height as f32),
    };
    let (fragments, _built, _text_ctx) = layout(dom, &styles, &images, viewport);

    let view = ServalLaneView::new(dom, &styles, &fragments);
    view.hit_test(Point::new(x, y))
        .map(|hit| NodeId::from_raw(hit.source_node.0 as usize))
}
