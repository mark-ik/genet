/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Producer-side: emit [`ServalPaintList`] from `FragmentPlane` +
//! `StylePlane` + DOM.
//!
//! Walks the DOM in paint order (pre-order traversal — normal-flow
//! paint order matches DOM order; positioned descendants would
//! reorder via z-index, but the probe doesn't exercise positioning).
//! Reads per-node layout from `FragmentPlane`, reads per-node style
//! from `StylePlane`, and produces a closed-set [`PaintCmd`] stream.
//!
//! ## Probe v1 scope (2026-05-18)
//!
//! - `DrawRect` per element with non-default background. The probe
//!   currently emits an opaque white rect per element since the
//!   cascade runs against an empty stylist; once real stylesheets
//!   apply, [`background_color_of`] becomes the place that reads
//!   `ComputedValues::background.background_color`.
//! - `DrawText` per text leaf with **empty glyph runs**. Real glyph
//!   shaping requires either (a) re-shaping in the emit phase or (b)
//!   caching the parley `Layout` from measure. Both are reasonable —
//!   deferred to a follow-up that picks one based on profile-data;
//!   for the trait-surface probe, empty glyphs is enough to validate
//!   that emission produces the right command structure.
//! - Coordinates are absolute (pre-order accumulated offsets), no
//!   `PushTransform`/`PopTransform` yet. The compositor model fits
//!   nicely with `taffy::Layout.location` being parent-relative, but
//!   emitting it requires `<element>` ↔ `<transform>` bookkeeping
//!   that's deferred until a renderer pulls on it.
//!
//! Cf. `docs/2026-05-17_paintlist_polyglot_renderer.md` (PM-3).

use std::hash::Hash;

use layout_dom_api::{LayoutDom, NodeKind};
use malloc_size_of_derive::MallocSizeOf;
use paint_list_api::{
    BorderRadius, BorderSide, BorderStyle, ColorF, CommonPlacement, DeviceIntSize, EngineId,
    FontInstanceKey, FontResource, GlyphInstance, IdNamespace, LayoutPoint, LayoutRect,
    LayoutSideOffsets, LayoutTransform, NormalBorder, PaintCmd, PaintList, RectItem, TextOptions,
    TextRunItem, TransformSpec,
};
use paint_list_api::items::{BorderDetails, BorderItem};
use paint_list_api::specs::TransformKind;
use parley::PositionedLayoutItem;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::construct::ConstructedTree;
use crate::fragment::FragmentPlane;
use crate::style::StylePlane;
use crate::text_measure::TextMeasureCtx;

/// Namespace for the font-instance keys this producer mints. Keys are
/// unique within one paint list; the namespace just disambiguates them
/// from other `FontInstanceKey` sources if they ever share a registry.
const SERVAL_FONT_NAMESPACE: IdNamespace = IdNamespace(0);

/// Serval's concrete [`PaintList`] impl. Built by [`emit_paint_list`].
#[derive(Clone, Debug, Default, Deserialize, MallocSizeOf, Serialize)]
pub struct ServalPaintList {
    viewport: DeviceIntSize,
    commands: Vec<PaintCmd>,
    generation: u64,
    fonts: Vec<FontResource>,
}

impl ServalPaintList {
    /// Construct an empty paint list. Mainly used by tests.
    pub fn new(viewport: DeviceIntSize) -> Self {
        Self {
            viewport,
            commands: Vec::new(),
            generation: 0,
            fonts: Vec::new(),
        }
    }
}

impl PaintList for ServalPaintList {
    fn engine_id(&self) -> EngineId {
        EngineId::SERVAL
    }
    fn viewport(&self) -> DeviceIntSize {
        self.viewport
    }
    fn generation_id(&self) -> u64 {
        self.generation
    }
    fn commands(&self) -> &[PaintCmd] {
        &self.commands
    }
    fn fonts(&self) -> &[FontResource] {
        &self.fonts
    }
}

/// Dedups fonts referenced by glyph runs and assigns each a
/// [`FontInstanceKey`]. Keyed by parley's blob id (stable per font
/// file), so a font shared across many runs ships its bytes once.
#[derive(Default)]
struct FontCollector {
    fonts: Vec<FontResource>,
    by_blob: FxHashMap<u64, FontInstanceKey>,
    next_idx: u32,
}

impl FontCollector {
    /// Intern a parley `FontData`, returning the key the matching
    /// `TextRunItem::font_instance` should carry. Adds a
    /// [`FontResource`] (font bytes + index) on first sight of a blob.
    fn intern(&mut self, font: &parley::FontData) -> FontInstanceKey {
        let blob_id = font.data.id();
        if let Some(k) = self.by_blob.get(&blob_id) {
            return *k;
        }
        let key = FontInstanceKey::new(SERVAL_FONT_NAMESPACE, self.next_idx);
        self.next_idx += 1;
        self.by_blob.insert(blob_id, key);
        self.fonts.push(FontResource {
            key,
            data: font.data.data().to_vec(),
            index: font.index,
        });
        key
    }
}

/// Walk the DOM in pre-order, emitting paint commands for each
/// element + text leaf with a fragment. Coordinates are absolute
/// (parent-relative `taffy::Layout.location` accumulated through the
/// recursion). Element background colors come from
/// `ComputedValues::background_color` when the cascade has populated
/// `ElementData`; otherwise default to transparent.
///
/// Text glyph runs come from the `TextMeasureCtx`'s cached parley
/// `Layout`s (populated by `crate::layout::layout` via
/// `measure_text_leaf`); pass `None` for `text_ctx` to emit text
/// items without glyph data (probe-quality empty glyph runs — useful
/// when caller hasn't run layout yet, or wants to skip text shaping).
pub fn emit_paint_list<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    viewport: DeviceIntSize,
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    emit_paint_list_with_glyphs(dom, styles, fragments, None, viewport)
}

/// Variant of [`emit_paint_list`] that consumes the cached text
/// layouts. `constructed` provides the DOM → Taffy id mapping;
/// `text_ctx` provides the cached parley `Layout`s. When both are
/// present, `DrawText` items carry shaped+positioned glyph runs.
pub fn emit_paint_list_with_layouts<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    constructed: &ConstructedTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    viewport: DeviceIntSize,
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    emit_paint_list_with_glyphs(
        dom,
        styles,
        fragments,
        Some(GlyphSource { constructed, text_ctx }),
        viewport,
    )
}

/// Source for shaped-glyph lookup during emission. Borrowed view over
/// the constructed tree's node_map + the text measure cache.
struct GlyphSource<'a, NodeId: Copy + Eq + Hash> {
    constructed: &'a ConstructedTree<NodeId>,
    text_ctx: &'a TextMeasureCtx,
}

fn emit_paint_list_with_glyphs<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    glyphs: Option<GlyphSource<'_, D::NodeId>>,
    viewport: DeviceIntSize,
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut commands = Vec::new();
    let mut fonts = FontCollector::default();
    walk(
        dom,
        styles,
        fragments,
        glyphs.as_ref(),
        &mut fonts,
        dom.document(),
        &mut commands,
    );
    ServalPaintList {
        viewport,
        commands,
        generation: 0,
        fonts: fonts.fonts,
    }
}

/// Recursive paint-order walk emitting compositor-model commands:
///
/// For each node with a fragment:
///   1. `PushTransform` with the fragment's local origin (its
///      `taffy::Layout.location`, which is parent-relative).
///   2. The node's own paint primitive (`DrawRect` for elements, one
///      `DrawText` per parley glyph-run for text leaves), in local
///      `(0, 0, w, h)` coords.
///   3. Recurse into children — their `PushTransform` origins compose
///      with the active transform stack.
///   4. `PopTransform` matching the push.
///
/// Nodes without fragments (synthetic / skipped) don't push or pop,
/// but children still descend in the current coord space.
fn walk<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    glyphs: Option<&GlyphSource<'_, D::NodeId>>,
    fonts: &mut FontCollector,
    id: D::NodeId,
    commands: &mut Vec<PaintCmd>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let pushed = if let Some(l) = fragments.rect_of(id) {
        commands.push(PaintCmd::PushTransform(TransformSpec {
            origin: LayoutPoint::new(l.location.x, l.location.y),
            transform: LayoutTransform::identity(),
            kind: TransformKind::Standard,
        }));
        let local_bounds = LayoutRect::new(
            LayoutPoint::new(0.0, 0.0),
            LayoutPoint::new(l.size.width, l.size.height),
        );
        match dom.kind(id) {
            NodeKind::Element => {
                commands.push(PaintCmd::DrawRect(RectItem {
                    placement: CommonPlacement::new(local_bounds),
                    color: background_color_of(styles, id),
                }));
                if let Some((widths, normal)) = border_of(styles, id) {
                    commands.push(PaintCmd::DrawBorder(BorderItem {
                        placement: CommonPlacement::new(local_bounds),
                        widths,
                        details: BorderDetails::Normal(normal),
                    }));
                }
            }
            NodeKind::Text => {
                let color = text_color_of(dom, styles, id);
                let emitted = glyphs
                    .map(|g| emit_text_runs(g, id, local_bounds, color, fonts, commands))
                    .unwrap_or(false);
                if !emitted {
                    // Cache-less path (no layout cache, or no glyphs):
                    // emit one empty text run so the command structure
                    // still reflects the text node.
                    commands.push(PaintCmd::DrawText(TextRunItem {
                        placement: CommonPlacement::new(local_bounds),
                        font_instance: FontInstanceKey::default(),
                        // No shaped run to read a size from; 16 px is
                        // the CSS/UA default (matches TextLeaf::new).
                        font_size: 16.0,
                        color,
                        glyphs: Vec::new(),
                        options: TextOptions::default(),
                    }));
                }
            }
            _ => {}
        }
        true
    } else {
        false
    };

    for child in dom.dom_children(id) {
        walk(dom, styles, fragments, glyphs, fonts, child, commands);
    }

    if pushed {
        commands.push(PaintCmd::PopTransform);
    }
}

/// Emit one `DrawText` per parley glyph-run for the text node's
/// cached `Layout`. Each run is homogeneous in font + size, so it
/// becomes one `TextRunItem` carrying that run's `FontInstanceKey`
/// (interned into `fonts`), `font_size`, and positioned glyphs.
/// Returns whether any run was emitted (false → no cached layout, or
/// empty text; caller falls back to an empty run).
fn emit_text_runs<NodeId: Copy + Eq + Hash>(
    source: &GlyphSource<'_, NodeId>,
    dom_id: NodeId,
    bounds: LayoutRect,
    color: ColorF,
    fonts: &mut FontCollector,
    commands: &mut Vec<PaintCmd>,
) -> bool {
    let Some(taffy_id) = source.constructed.node_map.get(&dom_id) else {
        return false;
    };
    let Some(layout) = source.text_ctx.layouts.get(taffy_id) else {
        return false;
    };
    let mut emitted = false;
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else {
                continue;
            };
            let parley_run = run.run();
            let key = fonts.intern(parley_run.font());
            let font_size = parley_run.font_size();
            let glyphs: Vec<GlyphInstance> = run
                .positioned_glyphs()
                .map(|g| GlyphInstance {
                    index: g.id,
                    point: LayoutPoint::new(g.x, g.y),
                })
                .collect();
            if glyphs.is_empty() {
                continue;
            }
            commands.push(PaintCmd::DrawText(TextRunItem {
                placement: CommonPlacement::new(bounds),
                font_instance: key,
                font_size,
                color,
                glyphs,
                options: TextOptions::default(),
            }));
            emitted = true;
        }
    }
    emitted
}

/// Read an element's background color from its `ComputedValues`.
/// Returns transparent when no cascade data is present (hand-rolled
/// styles bypass the cascade) — that matches CSS semantics for
/// "background-color: initial".
fn background_color_of<NodeId: Copy + Eq + std::hash::Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> ColorF {
    let Some(entry) = styles.get(id) else { return ColorF::TRANSPARENT; };
    let Some(data) = entry.borrow_data() else { return ColorF::TRANSPARENT; };
    let primary = data.styles.primary();
    let bg = &primary.get_background().background_color;
    let current = primary.get_inherited_text().color;
    stylo_color_to_paint(bg, current)
}

/// Resolve a text node's effective color: walk to its parent
/// element, read that element's `color` from `ComputedValues`
/// (a `color` value resolves to an `AbsoluteColor` directly —
/// `inherited_text.color` is already `AbsoluteColor`, not the
/// `Color` complex enum). Falls back to opaque black.
fn text_color_of<D>(dom: &D, styles: &StylePlane<D::NodeId>, text_id: D::NodeId) -> ColorF
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + std::hash::Hash,
{
    let Some(parent_id) = dom.parent(text_id) else { return ColorF::BLACK; };
    let Some(entry) = styles.get(parent_id) else { return ColorF::BLACK; };
    let Some(data) = entry.borrow_data() else { return ColorF::BLACK; };
    let primary = data.styles.primary();
    let absolute = primary.get_inherited_text().color;
    let srgb = absolute.into_srgb_legacy();
    let [r, g, b, a] = *srgb.raw_components();
    ColorF::new(r, g, b, a)
}

/// Convert Stylo's `computed::Color` to a PaintList `ColorF`.
/// Resolves `currentColor` via the provided `current_color`, then
/// flattens to sRGB and reads the raw `[r, g, b, a]` components.
fn stylo_color_to_paint(
    color: &style::values::computed::Color,
    current_color: style::color::AbsoluteColor,
) -> ColorF {
    let absolute = color.resolve_to_absolute(&current_color);
    let srgb = absolute.into_srgb_legacy();
    let [r, g, b, a] = *srgb.raw_components();
    ColorF::new(r, g, b, a)
}

/// Read an element's border (widths + per-side color/style) from
/// `ComputedValues`. Returns `None` if no side has a renderable
/// border (all widths zero or all sides are `none`/`hidden`) — keeps
/// the paint stream uncluttered for un-bordered elements.
fn border_of<NodeId: Copy + Eq + std::hash::Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<(LayoutSideOffsets, NormalBorder)> {
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let primary = data.styles.primary();
    let border = primary.get_border();
    let current_color = primary.get_inherited_text().color;

    let top_w = border.border_top_width.0.to_f32_px();
    let right_w = border.border_right_width.0.to_f32_px();
    let bottom_w = border.border_bottom_width.0.to_f32_px();
    let left_w = border.border_left_width.0.to_f32_px();

    let top_style = stylo_border_style(border.border_top_style);
    let right_style = stylo_border_style(border.border_right_style);
    let bottom_style = stylo_border_style(border.border_bottom_style);
    let left_style = stylo_border_style(border.border_left_style);

    // No-op early-out: every side is zero-width or none/hidden style.
    let renderable = |w: f32, s: BorderStyle| {
        w > 0.0 && !matches!(s, BorderStyle::None | BorderStyle::Hidden)
    };
    if !renderable(top_w, top_style)
        && !renderable(right_w, right_style)
        && !renderable(bottom_w, bottom_style)
        && !renderable(left_w, left_style)
    {
        return None;
    }

    let widths = LayoutSideOffsets::new(top_w, right_w, bottom_w, left_w);
    let details = NormalBorder {
        top: BorderSide {
            color: stylo_color_to_paint(&border.border_top_color, current_color),
            style: top_style,
        },
        right: BorderSide {
            color: stylo_color_to_paint(&border.border_right_color, current_color),
            style: right_style,
        },
        bottom: BorderSide {
            color: stylo_color_to_paint(&border.border_bottom_color, current_color),
            style: bottom_style,
        },
        left: BorderSide {
            color: stylo_color_to_paint(&border.border_left_color, current_color),
            style: left_style,
        },
        radius: BorderRadius::zero(),
        do_aa: true,
    };
    Some((widths, details))
}

/// Map Stylo's specified BorderStyle to paint-types BorderStyle.
fn stylo_border_style(s: style::values::specified::border::BorderStyle) -> BorderStyle {
    use style::values::specified::border::BorderStyle as S;
    match s {
        S::None => BorderStyle::None,
        S::Solid => BorderStyle::Solid,
        S::Double => BorderStyle::Double,
        S::Dotted => BorderStyle::Dotted,
        S::Dashed => BorderStyle::Dashed,
        S::Hidden => BorderStyle::Hidden,
        S::Groove => BorderStyle::Groove,
        S::Ridge => BorderStyle::Ridge,
        S::Inset => BorderStyle::Inset,
        S::Outset => BorderStyle::Outset,
    }
}

#[cfg(test)]
mod tests {
    use html5ever::local_name;
    use layout_dom_api::LayoutDom;
    use paint_list_api::PaintList;
    use serval_static_dom::{StaticDocument, StaticNodeId};
    use taffy::prelude::*;

    use super::*;
    use crate::adapter::NodeRef;
    use crate::layout::layout;
    use crate::style::StyleEntry;

    fn build_style_plane(document: &StaticDocument) -> StylePlane<StaticNodeId> {
        let mut plane: StylePlane<StaticNodeId> = StylePlane::new();
        let root = NodeRef::document(document);
        let mut queue = vec![root];
        while let Some(node) = queue.pop() {
            if document.element_name(node.id()).is_some() {
                plane.insert(
                    node.id(),
                    StyleEntry {
                        taffy: Style {
                            display: Display::Block,
                            size: Size {
                                width: length(200.0),
                                height: length(50.0),
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                );
            }
            queue.extend(node.dom_children());
        }
        plane
    }

    #[test]
    fn emit_produces_drawrect_for_each_element_and_drawtext_for_text() {
        let document =
            StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _built, _ctx) = layout(&document, &styles, viewport);

        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            DeviceIntSize::new(800, 600),
        );

        // Trait accessor sanity.
        assert_eq!(plist.engine_id(), EngineId::SERVAL);
        assert_eq!(plist.viewport(), DeviceIntSize::new(800, 600));

        let mut rect_count = 0;
        let mut text_count = 0;
        for cmd in plist.commands() {
            match cmd {
                PaintCmd::DrawRect(_) => rect_count += 1,
                PaintCmd::DrawText(_) => text_count += 1,
                _ => {}
            }
        }

        // html, body, p — at least three element rects.
        assert!(
            rect_count >= 3,
            "expected at least 3 DrawRects (html/body/p), got {rect_count}"
        );
        // "Hello" — at least one text run.
        assert!(
            text_count >= 1,
            "expected at least 1 DrawText, got {text_count}"
        );
    }

    #[test]
    fn emit_round_trips_through_serde() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let styles = build_style_plane(&document);
        let (fragments, _, _) = layout(
            &document,
            &styles,
            Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            },
        );
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            DeviceIntSize::new(800, 600),
        );

        let json = serde_json::to_string(&plist).expect("serialize ServalPaintList");
        let parsed: ServalPaintList =
            serde_json::from_str(&json).expect("deserialize ServalPaintList");
        assert_eq!(parsed.commands().len(), plist.commands().len());
        assert_eq!(parsed.viewport(), plist.viewport());
    }

    /// Probe glyph caching: pass the layout's TextMeasureCtx +
    /// ConstructedTree to emission and verify the resulting DrawText
    /// items carry positioned glyph runs (non-empty) rather than the
    /// empty Vec the cache-less path produces.
    #[test]
    fn emit_with_layouts_extracts_positioned_glyphs() {
        let document =
            StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, viewport);

        let plist = emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            DeviceIntSize::new(800, 600),
        );

        let mut text_with_glyphs = 0;
        for cmd in plist.commands() {
            if let PaintCmd::DrawText(t) = cmd {
                if !t.glyphs.is_empty() {
                    text_with_glyphs += 1;
                }
            }
        }
        assert!(
            text_with_glyphs >= 1,
            "expected at least one DrawText with non-empty glyph run, got {text_with_glyphs}"
        );
    }

    /// Text emission populates the font side-table, and each
    /// `DrawText`'s `font_instance` resolves to a `FontResource` in
    /// the list's `fonts()`. This is the producer-side half of the
    /// FontRegistry contract: the bytes the renderer needs travel
    /// with the paint output, keyed to the run.
    #[test]
    fn emit_with_layouts_populates_font_table() {
        let document =
            StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, viewport);
        let plist = emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            DeviceIntSize::new(800, 600),
        );

        // At least one font was collected, and it carries non-empty
        // bytes (a real system font blob).
        assert!(!plist.fonts().is_empty(), "expected font side-table populated");
        assert!(
            plist.fonts().iter().all(|f| !f.data.is_empty()),
            "every FontResource should carry font bytes"
        );

        // Every text run with glyphs references a key present in fonts().
        let font_keys: std::collections::HashSet<_> =
            plist.fonts().iter().map(|f| f.key).collect();
        for cmd in plist.commands() {
            if let PaintCmd::DrawText(t) = cmd {
                if !t.glyphs.is_empty() {
                    assert!(
                        font_keys.contains(&t.font_instance),
                        "DrawText font_instance {:?} not in fonts() table",
                        t.font_instance
                    );
                    assert!(t.font_size > 0.0, "shaped run should have positive font_size");
                }
            }
        }
    }

    /// Sanity-check the cache-less emit path still produces empty
    /// glyph runs (probe-mode behavior — useful when caller hasn't
    /// run layout or doesn't want to pay for glyph extraction).
    #[test]
    fn emit_without_layouts_produces_empty_glyph_runs() {
        let document =
            StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            DeviceIntSize::new(800, 600),
        );

        for cmd in plist.commands() {
            if let PaintCmd::DrawText(t) = cmd {
                assert!(
                    t.glyphs.is_empty(),
                    "expected empty glyph run from cache-less emit"
                );
            }
        }
    }

    /// Probe DrawBorder emission: a CSS-declared border produces a
    /// DrawBorder command alongside the element's DrawRect, with the
    /// expected widths + per-side color.
    #[test]
    fn emit_draws_borders_when_cascade_assigns_them() {
        use crate::cascade::run_cascade;
        use paint_list_api::items::BorderDetails;

        let document =
            StaticDocument::parse("<html><body><p>x</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[
                "p { display: block; width: 100px; height: 50px; \
                    border: 4px solid rgb(0, 128, 255); }",
            ],
        );
        styles.refresh_taffy_from_cascade();

        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            DeviceIntSize::new(800, 600),
        );

        let mut found_p_border = false;
        for cmd in plist.commands() {
            if let PaintCmd::DrawBorder(item) = cmd {
                // The <p>'s border: all sides 4px, solid, color (0, 0.5, 1, 1).
                if (item.widths.top - 4.0).abs() < 0.001
                    && (item.widths.right - 4.0).abs() < 0.001
                    && (item.widths.bottom - 4.0).abs() < 0.001
                    && (item.widths.left - 4.0).abs() < 0.001
                {
                    if let BorderDetails::Normal(n) = &item.details {
                        if matches!(n.top.style, paint_list_api::BorderStyle::Solid)
                            && (n.top.color.b - 1.0).abs() < 0.05
                        {
                            found_p_border = true;
                        }
                    }
                }
            }
        }
        assert!(
            found_p_border,
            "expected a 4px solid blue DrawBorder for the <p> element"
        );
    }

    /// No border in CSS = no DrawBorder command. The probe-stage
    /// optimization that suppresses zero-width/none-style borders.
    #[test]
    fn emit_omits_drawborder_when_no_border_declared() {
        use crate::cascade::run_cascade;

        let document =
            StaticDocument::parse("<html><body><p>x</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        // No border in this sheet — only background.
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &["p { background-color: rgb(255, 0, 0); }"],
        );
        styles.refresh_taffy_from_cascade();

        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            DeviceIntSize::new(800, 600),
        );

        for cmd in plist.commands() {
            assert!(
                !matches!(cmd, PaintCmd::DrawBorder(_)),
                "expected no DrawBorder commands, got {cmd:?}"
            );
        }
    }

    /// Probe the cascade → emit color path: run a real stylesheet
    /// through the cascade and verify the emitted DrawRect for the
    /// matched element carries the cascaded color.
    #[test]
    fn emit_color_comes_from_cascade_when_stylesheet_applies() {
        use crate::cascade::run_cascade;

        let document =
            StaticDocument::parse("<html><body><p>x</p></body></html>");
        let mut styles = build_style_plane(&document);
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &["body { background-color: rgb(255, 0, 0); }"],
        );

        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            DeviceIntSize::new(800, 600),
        );

        // Among DrawRects, at least one must be opaque red — that's body.
        let mut found_red = false;
        for cmd in plist.commands() {
            if let PaintCmd::DrawRect(rect) = cmd {
                if (rect.color.r - 1.0).abs() < 0.001
                    && rect.color.g < 0.001
                    && rect.color.b < 0.001
                    && (rect.color.a - 1.0).abs() < 0.001
                {
                    found_red = true;
                }
            }
        }
        assert!(
            found_red,
            "expected at least one DrawRect with cascade-applied red background"
        );
    }

    #[test]
    fn emit_paint_order_is_pre_order() {
        // Sanity-check that children paint after parents (so they
        // appear later in the command list), matching pre-order DOM
        // traversal.
        let document = StaticDocument::parse(
            "<html><body><p>a</p><p>b</p></body></html>",
        );
        let styles = build_style_plane(&document);
        let (fragments, _, _) = layout(
            &document,
            &styles,
            Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            },
        );
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            DeviceIntSize::new(800, 600),
        );

        // The first command must be PushTransform (compositor model:
        // each fragment opens a new coord space before painting itself).
        match plist.commands().first() {
            Some(PaintCmd::PushTransform(_)) => {}
            other => panic!("expected leading PushTransform, got {other:?}"),
        }

        // The command right after the leading PushTransform must be a
        // DrawRect — the html element painting itself in local coords.
        match plist.commands().get(1) {
            Some(PaintCmd::DrawRect(_)) => {}
            other => panic!("expected DrawRect after leading PushTransform, got {other:?}"),
        }

        // Push/Pop pairs must balance — the compositor-stack invariant.
        let mut depth = 0i32;
        for cmd in plist.commands() {
            match cmd {
                PaintCmd::PushTransform(_) => depth += 1,
                PaintCmd::PopTransform => depth -= 1,
                _ => {}
            }
            assert!(depth >= 0, "transform stack underflowed at command {cmd:?}");
        }
        assert_eq!(depth, 0, "transform stack didn't return to zero");

        // Find the p count — there should be at least two.
        let p_count = document
            .dom_children(document.document())
            .flat_map(|html| document.dom_children(html))
            .flat_map(|body| document.dom_children(body))
            .filter(|id| {
                document
                    .element_name(*id)
                    .is_some_and(|q| q.local == local_name!("p"))
            })
            .count();
        assert_eq!(p_count, 2, "fixture has two <p> siblings");
    }
}
