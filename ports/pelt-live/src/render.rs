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

use std::hash::Hash;

use engine_observables_api::{FragmentQuery, Point};
use layout_dom_api::LayoutDom;
use paint_list_api::{ColorF, DeviceIntSize};
use serval_layout::{
    BackgroundImagePlane, FragmentPlane, ImageLoader, ImagePlane, ScrollOffsets, ServalLaneView,
    ServalPaintList, StylePlane, caret_byte_at_point, caret_byte_vertical, caret_rect,
    emit_paint_list_with_layouts, layout, paint_list_from_layout_dom, run_cascade, selection_rects,
};
use serval_scripted_dom::{NodeId, ScriptedDom};

/// Caret bar thickness, device px.
const CARET_WIDTH: f32 = 2.0;
/// Caret bar colour (near-black, opaque).
const CARET_COLOR: ColorF = ColorF { r: 0.12, g: 0.12, b: 0.20, a: 1.0 };
/// Selection highlight colour (translucent blue — text shows through, since the
/// highlight paints over the text).
const SELECTION_COLOR: ColorF = ColorF { r: 0.40, g: 0.60, b: 0.95, a: 0.40 };
/// Scrollbar thumb colour (translucent dark grey, on the container's right edge).
const SCROLLBAR_COLOR: ColorF = ColorF { r: 0.30, g: 0.30, b: 0.36, a: 0.65 };
/// Scrollbar thumb width, device px.
const SCROLLBAR_WIDTH: f32 = 8.0;

/// What to paint for a focused text field's cursor: the element, the caret's
/// byte offset, and an optional selected byte range. Byte offsets (the layer
/// works in bytes); the host converts from its char-index model.
pub struct TextCursor {
    pub node: NodeId,
    pub caret: usize,
    pub selection: Option<(usize, usize)>,
}

/// Run cascade → layout → paint-emit over `dom` and translate the paint list to
/// a [`netrender::Scene`] at `width`×`height`.
///
/// `stylesheets` are author CSS applied on top of serval's UA defaults. Unlike
/// the static viewer there is no inline `<style>` / `<link>` collection: the
/// chrome DOM the runner builds carries no document-embedded stylesheets, so the
/// caller's sheets are the whole author set.
///
/// `cursor` is `Some(TextCursor)` to paint a focused field's selection highlight
/// (translucent, via [`serval_layout::selection_rects`]) and caret bar (via
/// [`serval_layout::caret_rect`]) over its laid-out text. Both are appended after
/// the layout walk (absolute coords); the selection goes under the caret. `None`
/// paints neither.
pub fn scene_from_scripted_dom(
    dom: &ScriptedDom,
    stylesheets: &[&str],
    width: u32,
    height: u32,
    cursor: Option<TextCursor>,
    scroll_offsets: &ScrollOffsets<NodeId>,
) -> netrender::Scene {
    let plist = paint_list_from_scripted_dom(dom, stylesheets, width, height, cursor, scroll_offsets);
    paint::translate_paint_list(&plist)
}

/// Render any `LayoutDom` document to a `netrender::Scene` through the shared
/// content pipeline ([`serval_layout::paint_list_from_layout_dom`]): cascade →
/// image decode → layout → emit → translate. `loader` supplies `<img>` /
/// `background-image` bytes (`data:` URIs decode inline regardless, so
/// [`serval_layout::NoImageLoader`] still yields inline images).
///
/// This is the content lane (fetched pages, the static viewer's documents),
/// shared with `pelt-viewer`'s `build_scene`. Unlike [`scene_from_scripted_dom`]
/// it adds no caret/selection/scrollbar overlays — a display surface, not a
/// focused editable field.
pub fn scene_from_layout_dom<D, L>(
    dom: &D,
    stylesheets: &[&str],
    loader: &L,
    width: u32,
    height: u32,
    scroll_offsets: &ScrollOffsets<D::NodeId>,
) -> netrender::Scene
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + 'static,
    L: ImageLoader,
{
    let plist = paint_list_from_layout_dom(dom, stylesheets, loader, width, height, scroll_offsets);
    paint::translate_paint_list(&plist)
}

/// The same cascade → layout → paint-emit pipeline as [`scene_from_scripted_dom`]
/// but stopping at the [`ServalPaintList`] — the engine-agnostic command stream,
/// before it is lowered to a `netrender::Scene`. A host that composites this
/// document with another producer's paint stream (e.g. the orrery's scene-paint
/// underlay) wants the list, not a finished scene, so it can merge the two
/// command streams into one scene via `paint_list_render::composite_paint_layers`.
pub fn paint_list_from_scripted_dom(
    dom: &ScriptedDom,
    stylesheets: &[&str],
    width: u32,
    height: u32,
    cursor: Option<TextCursor>,
    scroll_offsets: &ScrollOffsets<NodeId>,
) -> ServalPaintList {
    let mut styles: StylePlane<NodeId> = StylePlane::new();
    run_cascade(
        dom,
        &mut styles,
        euclid::Size2D::new(width as f32, height as f32),
        stylesheets,
        None,
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
        scroll_offsets,
        DeviceIntSize::new(width as i32, height as i32),
    );

    // Overlay the focused field's selection highlight (under) then caret (over),
    // both at absolute positions — appended after emit, so they draw over the
    // text at scene coordinates.
    if let Some(c) = cursor {
        if let Some((start, end)) = c.selection {
            let rects = selection_rects(dom, c.node, start, end, &built, &text_ctx, &fragments);
            plist.push_selection(&rects, SELECTION_COLOR);
        }
        if let Some(rect) =
            caret_rect(dom, c.node, c.caret, &built, &text_ctx, &fragments, CARET_WIDTH)
        {
            plist.push_caret(rect, CARET_COLOR);
        }
    }

    // A scrollbar thumb for each scrolled container: a bar on the box's right
    // edge, height ∝ visible/content, position ∝ offset/scrollable. Absolute
    // coords (the scroller's parent-relative box ≈ absolute for a top-level
    // container; nested scrollers would need origin accumulation).
    for (&node, &(_ox, oy)) in scroll_offsets {
        let Some(r) = fragments.rect_of(node) else { continue };
        let inner_h =
            r.size.height - r.padding.top - r.padding.bottom - r.border.top - r.border.bottom;
        let content_h = r.content_size.height;
        let scrollable = content_h - inner_h;
        if scrollable <= 0.5 {
            continue;
        }
        let thumb_h = (r.size.height * (inner_h / content_h)).max(24.0);
        let thumb_y = r.location.y + (oy / scrollable) * (r.size.height - thumb_h);
        let thumb_x = r.location.x + r.size.width - SCROLLBAR_WIDTH;
        plist.push_fill(thumb_x, thumb_y, SCROLLBAR_WIDTH, thumb_h, SCROLLBAR_COLOR);
    }

    plist
}

/// The focused field's caret rect in scene coordinates `(x, y, w, h)`, or
/// `None` if it has no layout. Runs cascade → layout → [`caret_rect`] for `node`
/// at `caret_byte`. The host feeds this to `set_ime_cursor_area` so the IME
/// candidate window appears at the caret (IME T3).
pub fn caret_screen_rect(
    dom: &ScriptedDom,
    stylesheets: &[&str],
    width: u32,
    height: u32,
    node: NodeId,
    caret_byte: usize,
) -> Option<(f32, f32, f32, f32)> {
    let mut styles: StylePlane<NodeId> = StylePlane::new();
    run_cascade(
        dom,
        &mut styles,
        euclid::Size2D::new(width as f32, height as f32),
        stylesheets,
        None,
    );
    let images = ImagePlane::new();
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width as f32),
        height: taffy::AvailableSpace::Definite(height as f32),
    };
    let (fragments, built, text_ctx) = layout(dom, &styles, &images, viewport);
    let r = caret_rect(dom, node, caret_byte, &built, &text_ctx, &fragments, CARET_WIDTH)?;
    Some((r.x, r.y, r.width, r.height))
}

/// The caret byte after moving one visual line — `delta` is `-1` (up) or `+1`
/// (down) — from `caret_byte` within `node`'s laid-out text. Runs cascade →
/// layout, then [`caret_byte_vertical`], so ArrowUp / ArrowDown in a textarea
/// follow parley's *wrapped* rows, not just `\n` breaks. `None` if `node` has no
/// text layout. The host feeds the result to `TextInput::set_caret_byte`.
pub fn soft_wrap_caret_byte(
    dom: &ScriptedDom,
    stylesheets: &[&str],
    width: u32,
    height: u32,
    node: NodeId,
    caret_byte: usize,
    delta: isize,
) -> Option<usize> {
    let mut styles: StylePlane<NodeId> = StylePlane::new();
    run_cascade(
        dom,
        &mut styles,
        euclid::Size2D::new(width as f32, height as f32),
        stylesheets,
        None,
    );
    let images = ImagePlane::new();
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width as f32),
        height: taffy::AvailableSpace::Definite(height as f32),
    };
    let (_fragments, built, text_ctx) = layout(dom, &styles, &images, viewport);
    caret_byte_vertical::<ScriptedDom>(node, caret_byte, &built, &text_ctx, delta)
}

/// The caret byte nearest scene point `(x, y)` within `node`'s laid-out text —
/// click-to-place-caret. Runs cascade → layout, then [`caret_byte_at_point`].
/// `None` if `node` has no text layout. The host maps a click on a focused field
/// to a caret position with this.
pub fn caret_byte_at(
    dom: &ScriptedDom,
    stylesheets: &[&str],
    width: u32,
    height: u32,
    node: NodeId,
    x: f32,
    y: f32,
) -> Option<usize> {
    let mut styles: StylePlane<NodeId> = StylePlane::new();
    run_cascade(
        dom,
        &mut styles,
        euclid::Size2D::new(width as f32, height as f32),
        stylesheets,
        None,
    );
    let images = ImagePlane::new();
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width as f32),
        height: taffy::AvailableSpace::Definite(height as f32),
    };
    let (fragments, built, text_ctx) = layout(dom, &styles, &images, viewport);
    caret_byte_at_point(dom, node, x, y, &built, &text_ctx, &fragments)
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
        None,
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
    scroll_offsets: &ScrollOffsets<NodeId>,
) -> Option<NodeId> {
    let mut styles: StylePlane<NodeId> = StylePlane::new();
    run_cascade(
        dom,
        &mut styles,
        euclid::Size2D::new(width as f32, height as f32),
        stylesheets,
        None,
    );
    let images = ImagePlane::new();
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width as f32),
        height: taffy::AvailableSpace::Definite(height as f32),
    };
    let (fragments, _built, _text_ctx) = layout(dom, &styles, &images, viewport);

    let view = ServalLaneView::new(dom, &styles, &fragments).with_scroll_offsets(scroll_offsets);
    view.hit_test(Point::new(x, y))
        .map(|hit| NodeId::from_raw(hit.source_node.0 as usize))
}
