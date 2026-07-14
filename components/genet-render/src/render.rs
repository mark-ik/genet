/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `ScriptedDom` → `netrender::Scene`.
//!
//! A focused mirror of the retired pelt-viewer's `build_scene` (Masonry-era,
//! removed 2026-06-12),
//! but pointed at a live, mutable [`ScriptedDom`] instead of parsing HTML: the
//! genet engine pipeline (cascade → layout → emit) runs over the same DOM that
//! Cambium's `GenetAppRunner` diffs. This is the render
//! half of Stage 1b — state change → DOM diff → genet layout/paint, offline.
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
#[cfg(test)]
use paint_list_api::PaintCmd;
use genet_layout::{
    BackgroundImagePlane, BoxTree, FragmentPlane, ImageLoader, ImagePlane, IncrementalLayout,
    ScrollOffsets, GenetLaneView, GenetPaintList, StylePlane, TextMeasureCtx,
    TextRange, accumulate_painted_origins, accumulated_translate, caret_byte_at_point,
    caret_byte_vertical, caret_color, caret_rect, emit_paint_list_with_layouts,
    layout, paint_list_from_layout_dom, push_scrollbars, range_rects,
    run_cascade, selection_rects, selection_style,
};
use genet_scripted_dom::{NodeId, ScriptedDom};

/// Caret bar thickness, device px.
const CARET_WIDTH: f32 = 2.0;
/// Caret bar colour (near-black, opaque).
const CARET_COLOR: ColorF = ColorF {
    r: 0.12,
    g: 0.12,
    b: 0.20,
    a: 1.0,
};
/// Selection highlight colour (translucent blue — text shows through, since the
/// highlight paints over the text).
const SELECTION_COLOR: ColorF = ColorF {
    r: 0.40,
    g: 0.60,
    b: 0.95,
    a: 0.40,
};
/// Focus-ring outline colour (the `:focus-visible` indicator — a blue stroke).
const FOCUS_RING_COLOR: ColorF = ColorF {
    r: 0.42,
    g: 0.62,
    b: 0.98,
    a: 0.95,
};
/// Focus-ring outline thickness, device px.
const FOCUS_RING_WIDTH: f32 = 2.0;

/// What to paint for the focused element: the node, and — when it is an editable
/// field (`editable`) — the caret's byte offset and an optional selected byte range.
/// A focus ring is drawn on the node regardless; the caret/selection are gated on
/// `editable`, so a focused **button** rings without a spurious caret. Byte offsets
/// (the layer works in bytes); the host converts from its char-index model.
pub struct TextCursor {
    pub node: NodeId,
    pub caret: usize,
    pub selection: Option<(usize, usize)>,
    /// Whether the focused node is an editable text field — gates the caret + selection
    /// paint. `false` for a focused non-text control (a button), which still rings.
    pub editable: bool,
}

/// Run cascade → layout → paint-emit over `dom` and translate the paint list to
/// a [`netrender::Scene`] at `width`×`height`.
///
/// `stylesheets` are author CSS applied on top of genet's UA defaults. Unlike
/// the static viewer there is no inline `<style>` / `<link>` collection: the
/// chrome DOM the runner builds carries no document-embedded stylesheets, so the
/// caller's sheets are the whole author set.
///
/// `cursor` is `Some(TextCursor)` to paint a focused field's selection highlight
/// (translucent, via [`genet_layout::selection_rects`]) and caret bar (via
/// [`genet_layout::caret_rect`]) over its laid-out text. Both are appended after
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
    let plist =
        paint_list_from_scripted_dom(dom, stylesheets, width, height, cursor, scroll_offsets);
    paint::translate_paint_list(&plist)
}

/// Render any `LayoutDom` document to a `netrender::Scene` through the shared
/// content pipeline ([`genet_layout::paint_list_from_layout_dom`]): cascade →
/// image decode → layout → emit → translate. `loader` supplies `<img>` /
/// `background-image` bytes (`data:` URIs decode inline regardless, so
/// [`genet_layout::NoImageLoader`] still yields inline images).
///
/// This is the content lane (fetched pages, static documents; the shape the
/// retired pelt-viewer's `build_scene` shared). Unlike [`scene_from_scripted_dom`]
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
    D::NodeId: Copy + Eq + Hash + Send + Sync + 'static,
    L: ImageLoader,
{
    let plist = paint_list_from_layout_dom(dom, stylesheets, loader, width, height, scroll_offsets);
    paint::translate_paint_list(&plist)
}

/// The same cascade → layout → paint-emit pipeline as [`scene_from_scripted_dom`]
/// but stopping at the [`GenetPaintList`] — the engine-agnostic command stream,
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
) -> GenetPaintList {
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
        // Caret + selection only for an editable field; a focused button rings without them.
        if c.editable {
            if let Some((start, end)) = c.selection {
                let rects = selection_rects(dom, c.node, start, end, &built, &text_ctx, &fragments);
                // `::selection { background }` when the field (or an ancestor) sets
                // one, else the theme default highlight.
                let highlight = selection_style(dom, &styles, c.node)
                    .map(|(bg, _fg)| ColorF {
                        r: bg[0],
                        g: bg[1],
                        b: bg[2],
                        a: bg[3],
                    })
                    .unwrap_or(SELECTION_COLOR);
                plist.push_selection(&rects, highlight);
            }
            if let Some(rect) = caret_rect(
                dom,
                c.node,
                c.caret,
                &built,
                &text_ctx,
                &fragments,
                CARET_WIDTH,
            ) {
                // `caret-color: auto` — track the field's cascaded text colour so the caret stays
                // legible on every theme (invisible near-black on a dark field otherwise); fall back
                // to the dark default when the colour can't be resolved.
                let caret = caret_color(dom, &styles, c.node)
                    .map(|[r, g, b, a]| ColorF { r, g, b, a })
                    .unwrap_or(CARET_COLOR);
                plist.push_caret(rect, caret);
            }
        }
        // The `:focus-visible` ring on the focused node (a CSS transform shifts where it paints;
        // `accumulated_translate` is `(0, 0)` for untransformed content).
        let translate = accumulated_translate(dom, &styles, c.node);
        push_focus_ring(
            &mut plist,
            dom,
            &fragments,
            scroll_offsets,
            c.node,
            translate,
        );
    }

    push_scrollbars(&mut plist, dom, &fragments, scroll_offsets);
    plist
}

/// `IncrementalLayout` session → `netrender::Scene`: the C3 chrome path. The
/// session retains cascade + layout across frames, so a steady (attribute-only)
/// frame skips the cascade+layout the stateless [`scene_from_scripted_dom`] redoes
/// every time — the cheap-path plan's halved chrome frame. Same overlays (focused
/// field selection + caret, scrollbars) as the stateless path, sourced from the
/// session's retained artifacts so the output is identical for a given DOM.
///
/// The caller owns the session (rebuilding it on a structural / resize / theme
/// change, applying the attribute-only batch otherwise) and guarantees it is on
/// the emittable path before calling. `cursor` paints a focused field's
/// selection + caret; `None` paints neither.
pub fn scene_from_session(
    session: &IncrementalLayout<NodeId>,
    dom: &ScriptedDom,
    cursor: Option<TextCursor>,
    scroll_offsets: &ScrollOffsets<NodeId>,
    width: u32,
    height: u32,
) -> netrender::Scene {
    let plist = paint_list_from_session(session, dom, cursor, scroll_offsets, width, height);
    paint::translate_paint_list(&plist)
}

/// An [`IncrementalLayout`] session over **any** [`LayoutDom`] → [`netrender::Scene`],
/// a plain display surface (no caret / selection / scrollbar overlays). The session
/// owns the document viewport, so this paints at its current document scroll
/// ([`IncrementalLayout::scroll_by`](genet_layout::IncrementalLayout::scroll_by)) —
/// the render-first path pelt's static viewer draws through (lay out once, re-emit
/// per scroll). The `ScriptedDom`-bound [`scene_from_session`] adds focused-field
/// overlays; this is the bare content surface for a document viewer.
pub fn scene_from_session_dom<D>(
    session: &IncrementalLayout<D::NodeId>,
    dom: &D,
    width: u32,
    height: u32,
) -> netrender::Scene
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + Send + Sync + 'static,
{
    let plist = session.emit_paint_list(
        dom,
        &ScrollOffsets::default(),
        DeviceIntSize::new(width as i32, height as i32),
    );
    paint::translate_paint_list(&plist)
}

/// The [`GenetPaintList`] half of [`scene_from_session`] — emit from the session
/// plus the focused-field + scrollbar overlays, before lowering to a Scene. The
/// session companion to [`paint_list_from_scripted_dom`], for a host that
/// composites the chrome list with another producer's stream.
pub fn paint_list_from_session(
    session: &IncrementalLayout<NodeId>,
    dom: &ScriptedDom,
    cursor: Option<TextCursor>,
    scroll_offsets: &ScrollOffsets<NodeId>,
    width: u32,
    height: u32,
) -> GenetPaintList {
    let mut plist = session.emit_paint_list(
        dom,
        scroll_offsets,
        DeviceIntSize::new(width as i32, height as i32),
    );

    // Same overlay order as the stateless path: selection highlight (under) then
    // caret (over), both at absolute coords, sourced from the session's retained
    // layout so a session-rendered field matches the stateless render byte for byte.
    if let Some(c) = cursor {
        if c.editable {
            if let Some((start, end)) = c.selection {
                let rects = session.selection_rects(dom, c.node, start, end);
                let highlight = session
                    .selection_style(dom, c.node)
                    .map(|(bg, _fg)| ColorF {
                        r: bg[0],
                        g: bg[1],
                        b: bg[2],
                        a: bg[3],
                    })
                    .unwrap_or(SELECTION_COLOR);
                plist.push_selection(&rects, highlight);
            }
            if let Some(rect) = session.caret_rect(dom, c.node, c.caret, CARET_WIDTH) {
                // `caret-color: auto` (see the static path above): track the field's text colour.
                let caret = session
                    .caret_color(dom, c.node)
                    .map(|[r, g, b, a]| ColorF { r, g, b, a })
                    .unwrap_or(CARET_COLOR);
                plist.push_caret(rect, caret);
            }
        }
        // The `:focus-visible` ring (the retained layout knows the node's accumulated transform).
        let translate = session.accumulated_translate(dom, c.node);
        push_focus_ring(
            &mut plist,
            dom,
            session.fragments(),
            scroll_offsets,
            c.node,
            translate,
        );
    }

    push_scrollbars(&mut plist, dom, session.fragments(), scroll_offsets);
    plist
}

/// Draw a `:focus-visible` ring — a thin outline — on the focused `node` at its **painted**
/// bounds (its painted origin shifted by `translate`, the accumulated CSS transform fragments
/// omit). The caller passes whichever node carries focus (field or button), so this rings any
/// focused element; the caret/selection above are gated on the field case. No-op for a node with
/// no laid-out box. (Upstreaming P3 — ported from the meerkat host's focus ring.)
fn push_focus_ring(
    plist: &mut GenetPaintList,
    dom: &ScriptedDom,
    fragments: &FragmentPlane<NodeId>,
    scroll_offsets: &ScrollOffsets<NodeId>,
    node: NodeId,
    translate: (f32, f32),
) {
    let Some(r) = fragments.rect_of(node) else {
        return;
    };
    let origins = accumulate_painted_origins(dom, fragments, scroll_offsets);
    let Some(p) = origins.get(&node) else { return };
    let (x, y) = (p.x + translate.0, p.y + translate.1);
    let (w, h, t) = (r.size.width, r.size.height, FOCUS_RING_WIDTH);
    plist.push_fill(x, y, w, t, FOCUS_RING_COLOR);
    plist.push_fill(x, y + h - t, w, t, FOCUS_RING_COLOR);
    plist.push_fill(x, y, t, h, FOCUS_RING_COLOR);
    plist.push_fill(x + w - t, y, t, h, FOCUS_RING_COLOR);
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
    LaidOutDocument::compute(dom, stylesheets, width, height).caret_screen_rect(node, caret_byte)
}

/// The caret byte after moving one visual line — `delta` is `-1` (up) or `+1`
/// (down) — from `caret_byte` within `node`'s laid-out text, keeping a sticky goal
/// column. Runs cascade → layout, then [`caret_byte_vertical`], so ArrowUp / ArrowDown
/// in a textarea follow parley's *wrapped* rows, not just `\n` breaks. Pass `goal_x`
/// `None` to seed the column from the caret, or the previous call's returned value to
/// hold it across a run; returns `(new_byte, goal_x)`. `None` if `node` has no text
/// layout. The host feeds the byte to `TextInput::set_caret_byte` and keeps `goal_x`.
pub fn soft_wrap_caret_byte(
    dom: &ScriptedDom,
    stylesheets: &[&str],
    width: u32,
    height: u32,
    node: NodeId,
    caret_byte: usize,
    delta: isize,
    goal_x: Option<f32>,
) -> Option<(usize, f32)> {
    LaidOutDocument::compute(dom, stylesheets, width, height)
        .soft_wrap_caret_byte(node, caret_byte, delta, goal_x)
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
    LaidOutDocument::compute(dom, stylesheets, width, height).caret_byte_at(node, x, y)
}

/// Highlight rects `(x, y, w, h)` for a multi-leaf selection `range` over `dom`
/// — the §3 selection-range geometry as a compute-once wrapper. Prefer
/// [`LaidOutDocument::range_rects`] when also running other queries on the same
/// frame; this is the thin one-shot for a host that only needs the highlight.
pub fn range_rects_from_scripted_dom(
    dom: &ScriptedDom,
    stylesheets: &[&str],
    width: u32,
    height: u32,
    range: TextRange<NodeId>,
) -> Vec<(f32, f32, f32, f32)> {
    LaidOutDocument::compute(dom, stylesheets, width, height).range_rects(range)
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
    LaidOutDocument::compute(dom, stylesheets, width, height).into_fragments()
}

/// Lay out `dom` and hit-test the point `(x, y)`, returning the topmost
/// (paint-order) node containing it — the `point → NodeId` half of input
/// dispatch (Stage 2a). `None` if the point falls outside every fragment.
///
/// This consumes genet's existing query surface
/// ([`GenetLaneView::hit_test`], part of `engine_observables_api`) rather than
/// adding a new spatial index. The reverse `SourceNodeId → NodeId` is trivial
/// here: `ScriptedDom::opaque_id(id)` is just `id`'s raw arena index, so
/// [`NodeId::from_raw`] inverts it directly (no O(n) walk like the generic
/// `GenetLaneView::find_by_source_id`).
pub fn hit_test_node(
    dom: &ScriptedDom,
    stylesheets: &[&str],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    scroll_offsets: &ScrollOffsets<NodeId>,
) -> Option<NodeId> {
    LaidOutDocument::compute(dom, stylesheets, width, height).hit_test(x, y, scroll_offsets)
}

/// A laid-out document: one cascade + layout, with every point query served
/// from the retained artifacts (styles + fragments + box tree + text context).
/// Build once per dirty frame and run many queries, instead of each query
/// re-running cascade+layout. This is the stateless companion to
/// `IncrementalLayout` (which retains the same fields); the cheap-path plan's
/// C1 seam. The free `*_from_scripted_dom` / `hit_test_node` / caret functions
/// are thin compute-once-then-query wrappers over it.
pub struct LaidOutDocument<'a> {
    dom: &'a ScriptedDom,
    styles: StylePlane<NodeId>,
    fragments: FragmentPlane<NodeId>,
    built: BoxTree<NodeId>,
    text_ctx: TextMeasureCtx,
}

impl<'a> LaidOutDocument<'a> {
    /// Cascade + lay out `dom` against `width`×`height`, once.
    pub fn compute(dom: &'a ScriptedDom, stylesheets: &[&str], width: u32, height: u32) -> Self {
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
        Self {
            dom,
            styles,
            fragments,
            built,
            text_ctx,
        }
    }

    /// The per-node fragment plane (borrowed; `into_fragments` to own it).
    pub fn fragments(&self) -> &FragmentPlane<NodeId> {
        &self.fragments
    }

    /// Consume into the owned fragment plane.
    pub fn into_fragments(self) -> FragmentPlane<NodeId> {
        self.fragments
    }

    /// Topmost (paint-order) node containing `(x, y)`, or `None`.
    pub fn hit_test(
        &self,
        x: f32,
        y: f32,
        scroll_offsets: &ScrollOffsets<NodeId>,
    ) -> Option<NodeId> {
        let view = GenetLaneView::new(self.dom, &self.styles, &self.fragments)
            .with_scroll_offsets(scroll_offsets);
        view.hit_test(Point::new(x, y))
            .map(|hit| NodeId::from_raw(hit.source_node.0 as usize))
    }

    /// Screen rect `(x, y, w, h)` of the caret at `caret_byte` within `node`.
    pub fn caret_screen_rect(
        &self,
        node: NodeId,
        caret_byte: usize,
    ) -> Option<(f32, f32, f32, f32)> {
        let r = caret_rect(
            self.dom,
            node,
            caret_byte,
            &self.built,
            &self.text_ctx,
            &self.fragments,
            CARET_WIDTH,
        )?;
        Some((r.x, r.y, r.width, r.height))
    }

    /// Caret byte one soft-wrapped line `delta` (up `-1` / down `+1`) from `caret_byte`
    /// within `node`, keeping a sticky goal column: pass `goal_x` `None` to seed it from
    /// the caret, or the previous call's returned value to hold the column across a run.
    /// Returns `(new_byte, goal_x)`.
    pub fn soft_wrap_caret_byte(
        &self,
        node: NodeId,
        caret_byte: usize,
        delta: isize,
        goal_x: Option<f32>,
    ) -> Option<(usize, f32)> {
        caret_byte_vertical::<ScriptedDom>(
            node,
            caret_byte,
            &self.built,
            &self.text_ctx,
            delta,
            goal_x,
        )
    }

    /// Caret byte nearest scene point `(x, y)` within `node`'s laid-out text.
    pub fn caret_byte_at(&self, node: NodeId, x: f32, y: f32) -> Option<usize> {
        caret_byte_at_point(
            self.dom,
            node,
            x,
            y,
            &self.built,
            &self.text_ctx,
            &self.fragments,
        )
    }

    /// Highlight rects `(x, y, w, h)` for a selection `range` that may span several
    /// inline leaves across block boundaries — the multi-node selection geometry
    /// (pseudo follow-ups §3), served off this one layout. Endpoints are
    /// `(inline-formatting-leaf, byte offset)` pairs in the caller's selection
    /// order (a backwards drag is fine). Empty when the range is collapsed.
    pub fn range_rects(&self, range: TextRange<NodeId>) -> Vec<(f32, f32, f32, f32)> {
        range_rects(
            self.dom,
            range,
            &self.built,
            &self.text_ctx,
            &self.fragments,
        )
        .into_iter()
        .map(|r| (r.x, r.y, r.width, r.height))
        .collect()
    }

    /// The accesskit accessibility tree derived from this layout.
    pub fn accesskit_tree(&self, focus: Option<NodeId>) -> accesskit::TreeUpdate {
        GenetLaneView::new(self.dom, &self.styles, &self.fragments).accesskit_tree(focus)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layout_dom_api::{LayoutDomMut, LocalName, Namespace, QualName};

    fn html(local: &str) -> QualName {
        QualName::new(
            None,
            Namespace::from("http://www.w3.org/1999/xhtml"),
            LocalName::from(local),
        )
    }

    fn attr(local: &str) -> QualName {
        QualName::new(None, Namespace::from(""), LocalName::from(local))
    }

    /// One `LaidOutDocument::compute` serves several queries (fragment rect and
    /// hit-test) off a single cascade+layout, instead of each query re-running
    /// the pipeline. The C1 seam.
    #[test]
    fn laid_out_document_serves_queries_from_one_layout() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let div = dom.create_element(html("div"));
        dom.set_attribute(div, attr("class"), "x");
        dom.append_child(root, div);

        let doc = LaidOutDocument::compute(
            &dom,
            &[
                "div { display: block; }",
                ".x { width: 40px; height: 20px; }",
            ],
            200,
            100,
        );

        // Fragment query.
        let rect = doc.fragments().rect_of(div).expect("div fragment");
        assert!(
            (rect.size.width - 40.0).abs() < 0.5,
            "div width 40, got {}",
            rect.size.width
        );

        // Hit-test query off the same computed layout.
        let offsets = ScrollOffsets::default();
        assert_eq!(
            doc.hit_test(5.0, 5.0, &offsets),
            Some(div),
            "hit-test finds the div from the same layout"
        );
    }

    /// The arrangement contract's paint half: absolutely-placed children in a
    /// `position: relative` container paint in `z-index` order (CSS 2.1
    /// Appendix E via `paint_stacking`), not DOM order — a raised card
    /// overpaints its sibling regardless of insertion order.
    #[test]
    fn arranged_children_paint_in_z_order_not_dom_order() {
        use paint_list_api::PaintList;
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let container = dom.create_element(html("div"));
        dom.set_attribute(
            container,
            attr("style"),
            "position: relative; width: 200px; height: 100px;",
        );
        dom.append_child(root, container);
        // DOM-first card carries the HIGHER z: if paint follows DOM order it
        // comes first; if it follows z (correct) it comes last.
        let raised = dom.create_element(html("div"));
        dom.set_attribute(
            raised,
            attr("style"),
            "position: absolute; left: 10px; top: 10px; z-index: 3; \
             width: 80px; height: 60px; background-color: rgb(200, 30, 30);",
        );
        dom.append_child(container, raised);
        let lowered = dom.create_element(html("div"));
        dom.set_attribute(
            lowered,
            attr("style"),
            "position: absolute; left: 40px; top: 20px; z-index: 2; \
             width: 80px; height: 60px; background-color: rgb(30, 30, 200);",
        );
        dom.append_child(container, lowered);

        let plist = paint_list_from_scripted_dom(
            &dom,
            &["div { display: block; }"],
            200,
            100,
            None,
            &ScrollOffsets::default(),
        );
        let pos_of = |r: f32, b: f32| {
            plist.commands().iter().position(|c| {
                matches!(c, PaintCmd::DrawRect(rect)
                    if (rect.color.r - r).abs() < 0.05 && (rect.color.b - b).abs() < 0.05)
            })
        };
        let red = pos_of(200.0 / 255.0, 30.0 / 255.0).expect("raised card paints");
        let blue = pos_of(30.0 / 255.0, 200.0 / 255.0).expect("lowered card paints");
        assert!(
            blue < red,
            "z-index 3 paints after z-index 2 despite DOM order (blue at {blue}, red at {red})",
        );
    }

    /// First element named `name` in `node`'s subtree (pre-order), or `None`.
    fn first_named(dom: &ScriptedDom, node: NodeId, name: &str) -> Option<NodeId> {
        if dom
            .element_name(node)
            .is_some_and(|q| q.local.as_ref() == name)
        {
            return Some(node);
        }
        dom.dom_children(node)
            .find_map(|c| first_named(dom, c, name))
    }

    /// C3 parity fixture: the chrome rendered through an `IncrementalLayout`
    /// session ([`scene_from_session`]) is op-for-op identical to the stateless
    /// per-frame [`scene_from_scripted_dom`], across focused-field states (plain,
    /// caret, selection). The session may skip cascade+layout on a steady frame,
    /// but its Scene must match the stateless render byte for byte — the cheap-path
    /// plan's parity guard for swapping the chrome onto a session.
    #[test]
    fn session_render_matches_stateless_render() {
        const SHEET: &[&str] = &[
            "html, body, div { display: block; margin: 0; }",
            "body { padding: 8px; }",
            "#field { font-size: 20px; color: rgb(20, 20, 30); }",
        ];
        let (w, h) = (400u32, 200u32);

        let mut dom = ScriptedDom::new();
        let root = dom.document();
        dom.set_inner_html(root, "<div id=\"field\">hello world</div>");
        let field = first_named(&dom, root, "div").expect("a <div> with text");

        let scroll = ScrollOffsets::<NodeId>::default();
        // The session a C3 frame builds for this DOM: one full cascade + layout.
        let session = IncrementalLayout::new(&dom, SHEET, w as f32, h as f32);

        // (caret byte, optional selection range) per case; a fresh `TextCursor` per
        // call since it is `!Copy`.
        let cur = |c: Option<(usize, Option<(usize, usize)>)>| {
            c.map(|(caret, selection)| TextCursor {
                node: field,
                caret,
                selection,
                editable: true,
            })
        };
        let cases = [None, Some((3, None)), Some((6, Some((0, 5))))];

        for (i, case) in cases.into_iter().enumerate() {
            let stateless = scene_from_scripted_dom(&dom, SHEET, w, h, cur(case), &scroll);
            let sessioned = scene_from_session(&session, &dom, cur(case), &scroll, w, h);
            assert_eq!(
                format!("{:?}", stateless.ops),
                format!("{:?}", sessioned.ops),
                "case {i}: session render must match the stateless render op-for-op",
            );
        }
    }

    /// A focused field paints a `:focus-visible` ring **plus** its caret; a focused non-editable
    /// node (a button) rings **without** a caret — `editable` gates the caret/selection while the
    /// ring is unconditional. (Upstreaming P3 — focus ring.)
    #[test]
    fn focus_ring_rings_and_editable_gates_the_caret() {
        const SHEET: &[&str] = &["html, body, div { display: block; margin: 0; }"];
        let (w, h) = (400u32, 200u32);
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        dom.set_inner_html(root, "<div id=\"field\">hello</div>");
        let field = first_named(&dom, root, "div").expect("a <div>");
        let scroll = ScrollOffsets::<NodeId>::default();

        let ops = |cursor| {
            scene_from_scripted_dom(&dom, SHEET, w, h, cursor, &scroll)
                .ops
                .len()
        };
        let none = ops(None);
        let editable = ops(Some(TextCursor {
            node: field,
            caret: 5,
            selection: None,
            editable: true,
        }));
        let button = ops(Some(TextCursor {
            node: field,
            caret: 0,
            selection: None,
            editable: false,
        }));

        assert!(button > none, "a focused node adds the ring");
        assert!(
            editable > button,
            "an editable focus adds the caret on top of the ring"
        );
    }

    /// C2: a display surface rendered through `scene_from_session_dom` paints at the
    /// session's document scroll — scrolling the viewport changes the scene, so the
    /// session→scene path threads the document offset (the render-first viewer path).
    #[test]
    fn scene_from_session_paints_at_the_document_scroll() {
        const SHEET: &[&str] = &[
            "html, body, div { display: block; margin: 0; }",
            ".tall { height: 2000px; }",
        ];
        let (w, h) = (400u32, 300u32);
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        dom.set_inner_html(root, "<div class=\"tall\">scroll me</div>");

        let mut session = IncrementalLayout::new(&dom, SHEET, w as f32, h as f32);
        let still = scene_from_session_dom(&session, &dom, w, h);
        assert!(
            !still.ops.is_empty(),
            "the document renders a non-empty scene"
        );

        // Scroll the document; the session repaints translated, so the scene differs.
        session.set_viewport_scroll(&dom, (0.0, 400.0));
        let scrolled = scene_from_session_dom(&session, &dom, w, h);
        assert_ne!(
            format!("{:?}", still.ops),
            format!("{:?}", scrolled.ops),
            "scrolling the session viewport moves the rendered content",
        );
    }
}
