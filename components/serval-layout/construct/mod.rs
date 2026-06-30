/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! DOM inline-content gathering + cascade property readers.
//!
//! Shared helpers consumed by [`crate::box_tree`] when it builds the
//! layout arena: inline-formatting-context detection, gathering an
//! inline subtree into [`InlineContent`] (styled runs + replaced inline
//! boxes), replaced-element sizing, and the per-element cascade reads
//! (font + color) that style each text run.
//!
//! (Previously this module also built the `TaffyTree` directly; that
//! owned-`Style` path was retired when the box tree — Taffy's trait-impl
//! tree over `TaffyStyloStyle` — became the engine. See
//! `docs/2026-05-25_box_tree_trait_impl_plan.md`.)

use std::hash::Hash;
use std::ops::Range;

use layout_dom_api::{LayoutDom, NodeKind};

use servo_arc::Arc as ServoArc;
use style::properties::ComputedValues;

use crate::adapter::NodeRef;
use crate::box_tree::PseudoKind;
use crate::image_decode::ImagePlane;
use crate::style::StylePlane;
use crate::text_measure::{
    FontFamilySpec, GenericFamilyKind, InlineBlockBox, InlineBoxItem, InlineContent, InlineRun,
    LineHeightSpec,
};

/// Default font size used for runs whose element has no cascaded
/// `font-size` (hand-rolled style fixtures). 16 px matches the
/// CSS/UA-stylesheet convention and parley's own default.
const DEFAULT_FONT_SIZE: f32 = 16.0;

/// Whether `elem` establishes an inline formatting context: every
/// element child is either `display:inline` or a replaced inline box
/// (`<img>`), text children flow inline by nature, and there is at
/// least one piece of *inline text* (a text node or a non-replaced
/// inline element) to flow.
///
/// The inline-text requirement keeps a lone `<img>` on the block path:
/// `<body><img></body>` stays a block with the image as its own child
/// box (the established, working behavior). Only when an `<img>` is
/// mixed with text — `<p>before <img> after</p>` — does the element
/// become an inline context where the image flows as a parley
/// `InlineBox` among the runs.
///
/// Comments / PIs are ignored. With no cascade data (`is_inline_element`
/// → `None`), non-replaced elements are treated as block — preserving
/// the pre-inline behavior for hand-rolled style fixtures.
mod gather;
mod list_marker;
mod style_read;

pub(crate) use gather::*;
pub(crate) use list_marker::*;
pub(crate) use style_read::*;

pub(crate) fn establishes_inline_context<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    elem: NodeRef<'a, D>,
) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut has_inline_text = false;
    let mut replaced_count = 0;
    for child in elem.dom_children() {
        match dom.kind(child.id()) {
            // Whitespace-only text is collapsible formatting, not real inline
            // content — it must not by itself turn a block container into an
            // inline context.
            NodeKind::Text => {
                if dom.text(child.id()).is_some_and(|t| !t.trim().is_empty()) {
                    has_inline_text = true;
                }
            },
            NodeKind::Element => {
                if is_replaced(dom, child.id()) {
                    if is_floating(styles, child.id()) {
                        return false;
                    }
                    // A replaced element flows as an inline box. A *lone* img
                    // with no other inline content stays on the block path
                    // (intrinsic sizing); two or more flow inline, side by side.
                    replaced_count += 1;
                    continue;
                }
                if has_clearance(styles, child.id()) {
                    return false;
                }
                // `inline-block` is an atomic inline-level box: it participates
                // in the line like inline content (so it keeps this an inline
                // context), but `gather_runs` reserves it as an atomic
                // `InlineBox` rather than recursing into its content.
                if is_inline_block(styles, child.id()) {
                    has_inline_text = true;
                } else if is_inline_element(styles, child.id()).unwrap_or(false) {
                    has_inline_text = true;
                } else {
                    // A block-level child forces block layout.
                    return false;
                }
            },
            _ => {},
        }
    }
    // Inline context when there is real inline text / an inline element /
    // inline-block, or when two or more replaced boxes flow side by side. A
    // single lone img with nothing else stays on the block path.
    has_inline_text || replaced_count >= 2
}

/// Whether an element is replaced content we render as its own box rather than
/// as flowed inline text: decoded `<img>`, embedded content with the CSS default
/// object size, and host-composited media/canvas lanes.
pub(crate) fn is_replaced<D>(dom: &D, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    use html5ever::local_name;
    dom.element_name(id).is_some_and(|q| {
        q.local == local_name!("img")
            || q.local == local_name!("iframe")
            || q.local == local_name!("canvas")
            || q.local == local_name!("video")
            || q.local == local_name!("object")
            || q.local == local_name!("embed")
            // `<external-texture>` is a host-composited replaced element (see
            // `external_texture_key_of`): a custom name, so compared by string rather
            // than a `local_name!` atom. It sizes like the default-object replaced
            // elements (300×150, CSS-overridable).
            || q.local.as_ref() == "external-texture"
    })
}

/// The texture key of an `<external-texture key="…">` element or a WebGL-backed
/// `<canvas data-serval-external-texture-key="…">` / media element. The producer
/// mints the `u64` key out of band and registers the matching `wgpu::Texture`
/// with the renderer; the element only carries the stable key + a box, so paint emits a
/// [`PaintCmd::DrawExternalTexture`](paint_list_api::PaintCmd) the host composites.
/// Missing / unparseable keys yield `None` (the element paints nothing).
pub(crate) fn external_texture_key_of<D>(dom: &D, id: D::NodeId) -> Option<u64>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    use html5ever::{LocalName, ns};
    let local = dom.element_name(id)?.local.as_ref();
    let attr = if local == "external-texture" {
        "key"
    } else if local == "canvas" || local == "video" {
        "data-serval-external-texture-key"
    } else {
        return None;
    };
    dom.attribute(id, &ns!(), &LocalName::from(attr))?
        .parse()
        .ok()
}

/// Whether `id` is a replaced element that, lacking intrinsic content, falls
/// back to the CSS default object size (300×150) — every replaced element except
/// `<img>` (which sizes to its decoded pixels, or 0 when undecoded).
fn uses_default_object_size<D>(dom: &D, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    dom.element_name(id)
        .is_some_and(|q| q.local != html5ever::local_name!("img"))
}

/// Whether `id` has `white-space: nowrap` (CSS `text-wrap-mode: nowrap`) — its
/// inline content lays out on a single line, not soft-wrapped to the available
/// width. `false` (wrap) when the cascade has not run.
fn no_wrap_of<NodeId: Copy + Eq + Hash>(styles: &StylePlane<NodeId>, id: NodeId) -> bool {
    use style::properties::longhands::text_wrap_mode::computed_value::T as Mode;
    styles
        .get(id)
        .and_then(|e| e.borrow_data())
        .is_some_and(|d| {
            matches!(
                d.styles.primary().get_inherited_text().text_wrap_mode,
                Mode::Nowrap
            )
        })
}

/// Read an element's cascaded outer display: `Some(true)` for
/// `display:inline`, `Some(false)` for block-level, `None` when the
/// cascade hasn't run for this element.
fn is_inline_element<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<bool> {
    use style::values::specified::box_::DisplayOutside;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let display = data.styles.primary().get_box().display;
    Some(matches!(display.outside(), DisplayOutside::Inline))
}

fn is_floating<NodeId: Copy + Eq + Hash>(styles: &StylePlane<NodeId>, id: NodeId) -> bool {
    styles
        .get(id)
        .and_then(|e| e.borrow_data())
        .is_some_and(|d| {
            stylo_taffy::convert::float(d.styles.primary().get_box().clone_float()).is_floated()
        })
}

fn has_clearance<NodeId: Copy + Eq + Hash>(styles: &StylePlane<NodeId>, id: NodeId) -> bool {
    styles
        .get(id)
        .and_then(|e| e.borrow_data())
        .is_some_and(|d| {
            !matches!(
                stylo_taffy::convert::clear(d.styles.primary().get_box().clone_clear()),
                taffy::Clear::None
            )
        })
}

/// Collapse each maximal run of ASCII/Unicode whitespace in `s` to a single
/// space (CSS `white-space: normal` collapsing). Source newlines + indentation
/// become a single space rather than a literal run or a forced line break.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(c);
            prev_ws = false;
        }
    }
    out
}

/// Collapse runs of whitespace, but preserve forced line breaks: a run that
/// contains a `\n` becomes a single `\n` (parley breaks there), any other run a
/// single space (CSS `white-space-collapse: preserve-breaks`, i.e. `pre-line`).
fn collapse_preserving_breaks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run_ws = false;
    let mut run_has_break = false;
    for c in s.chars() {
        if c.is_whitespace() {
            run_ws = true;
            run_has_break |= c == '\n';
        } else {
            if run_ws {
                out.push(if run_has_break { '\n' } else { ' ' });
                run_ws = false;
                run_has_break = false;
            }
            out.push(c);
        }
    }
    if run_ws {
        out.push(if run_has_break { '\n' } else { ' ' });
    }
    out
}

/// Apply `text`'s computed `white-space-collapse` to source `text`, the CSS
/// step that turns formatting whitespace into rendered whitespace + forced
/// breaks before it reaches parley. `Collapse` (the `white-space: normal` /
/// `nowrap` default) folds every run to one space; `Preserve` / `BreakSpaces`
/// (`pre` / `pre-wrap`) keep whitespace and newlines verbatim — parley breaks at
/// each `\n`; `PreserveBreaks` (`pre-line`) collapses spaces but keeps newlines.
fn apply_white_space_collapse<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
    text: &str,
) -> String {
    use style::computed_values::white_space_collapse::T as Collapse;
    let mode = styles
        .get(id)
        .and_then(|e| e.borrow_data())
        .map(|d| d.styles.primary().get_inherited_text().white_space_collapse)
        .unwrap_or(Collapse::Collapse);
    match mode {
        Collapse::Collapse => collapse_whitespace(text),
        Collapse::Preserve | Collapse::BreakSpaces => text.to_string(),
        Collapse::PreserveBreaks => collapse_preserving_breaks(text),
    }
}

/// Whether `id` is `display: inline-block` — inline-level outside, but an
/// independent (flow-root) formatting context inside.
pub(crate) fn is_inline_block<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> bool {
    use style::values::specified::box_::{DisplayInside, DisplayOutside};
    let Some(entry) = styles.get(id) else {
        return false;
    };
    let Some(data) = entry.borrow_data() else {
        return false;
    };
    let display = data.styles.primary().get_box().display;
    matches!(display.outside(), DisplayOutside::Inline)
        && matches!(display.inside(), DisplayInside::FlowRoot)
}

/// Intrinsic content size for replaced elements. Decoded image-backed content
/// uses its pixels; host/media/canvas lanes use dimension attributes when
/// present, otherwise the CSS default object size, 300×150. An undecoded `<img>`
/// has no intrinsic size.
pub(crate) fn replaced_intrinsic_size<D>(
    dom: &D,
    images: &ImagePlane<D::NodeId>,
    id: D::NodeId,
) -> Option<(f32, f32)>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if let Some(decoded) = images.get(id) {
        return Some((decoded.width as f32, decoded.height as f32));
    }
    if uses_default_object_size(dom, id) {
        return Some(default_object_size_from_attrs(dom, id));
    }
    None
}

fn default_object_size_from_attrs<D>(dom: &D, id: D::NodeId) -> (f32, f32)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    (
        dimension_attr(dom, id, "width").unwrap_or(300.0),
        dimension_attr(dom, id, "height").unwrap_or(150.0),
    )
}

fn dimension_attr<D>(dom: &D, id: D::NodeId, name: &str) -> Option<f32>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    use html5ever::{LocalName, ns};
    let value = dom.attribute(id, &ns!(), &LocalName::from(name))?;
    let parsed = value.trim().parse::<f32>().ok()?;
    (parsed > 0.0 && parsed.is_finite()).then_some(parsed)
}

/// Pixel size for a replaced element: intrinsic/default object size plus definite
/// CSS width/height. When only one axis is definite, the other axis is resolved
/// from the sizing intrinsic ratio. `contain: size` can override the sizing
/// intrinsic through `contain-intrinsic-width` / `contain-intrinsic-height`, but
/// the content intrinsic remains unchanged for paint's `object-fit` ratio.
/// Shared by the block-level replaced leaf ([`crate::box_tree`]) and inline
/// replaced boxes. Non-length dimensions (`auto`, percentages) otherwise leave
/// the intrinsic/default size in place.
pub(crate) fn replaced_px_size<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    id: D::NodeId,
) -> (f32, f32)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let (base_w, base_h) = replaced_sizing_intrinsic_size(dom, styles, images, id)
        .or_else(|| replaced_intrinsic_size(dom, images, id))
        .unwrap_or((0.0, 0.0));

    let css_size = styles
        .get(id)
        .and_then(|entry| entry.borrow_data())
        .map(|data| {
            let pos = data.styles.primary().get_position();
            (definite_px(&pos.width), definite_px(&pos.height))
        })
        .unwrap_or((None, None));

    match css_size {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) if base_w > 0.0 && base_h > 0.0 => (w, w * base_h / base_w),
        (None, Some(h)) if base_w > 0.0 && base_h > 0.0 => (h * base_w / base_h, h),
        (Some(w), None) => (w, base_h),
        (None, Some(h)) => (base_w, h),
        (None, None) => (base_w, base_h),
    }
}

fn replaced_sizing_intrinsic_size<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    id: D::NodeId,
) -> Option<(f32, f32)>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let (intrinsic_w, intrinsic_h) = replaced_intrinsic_size(dom, images, id)?;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let primary = data.styles.primary();
    let contain = primary.get_box().clone_contain();
    if !contain.contains(style::values::specified::box_::Contain::SIZE) {
        return None;
    }
    let override_size = styles.contain_intrinsic_override(id)?;
    let width = override_size.width.unwrap_or(intrinsic_w);
    let height = override_size.height.unwrap_or(intrinsic_h);
    Some((width, height))
}

/// A CSS `Size` as definite pixels, or `None` for `auto` / percentage /
/// intrinsic keywords.
fn definite_px(size: &style::values::computed::Size) -> Option<f32> {
    use style::values::computed::Size as CssSize;
    match size {
        CssSize::LengthPercentage(lp) => lp.0.to_length().map(|l| l.px()),
        _ => None,
    }
}
