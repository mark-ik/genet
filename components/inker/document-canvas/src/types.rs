/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Render-packet types.
//!
//! All coordinates are in *logical pixels* relative to the viewport's
//! top-left origin. Y grows downward (screen convention).

use serde::{Deserialize, Serialize};

/// Viewport into which the document is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
    /// Device-pixel ratio. v1 layout doesn't use this directly (it lays out
    /// in logical pixels), but downstream rasterisers need it.
    pub scale_factor: f32,
}

impl Viewport {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            scale_factor: 1.0,
        }
    }

    pub fn with_scale_factor(mut self, scale_factor: f32) -> Self {
        self.scale_factor = scale_factor;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    pub fn from_xywh(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            origin: Point::new(x, y),
            size: Size::new(w, h),
        }
    }

    pub fn max_x(&self) -> f32 {
        self.origin.x + self.size.width
    }

    pub fn max_y(&self) -> f32 {
        self.origin.y + self.size.height
    }
}

/// One positioned glyph in a glyph run. Position is relative to the run's
/// `origin`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionedGlyph {
    /// Glyph index in the font (parley uses `u32`; downstream renderers
    /// typically downcast as needed).
    pub glyph_id: u32,
    /// X offset from the run's origin.
    pub x: f32,
    /// Y offset from the run's origin (typically 0; non-zero for rare
    /// scripts).
    pub y: f32,
    pub advance: f32,
}

/// Identifies the concrete font face a [`GlyphRun`] was *shaped against*,
/// as parley actually chose it (after any fallback). Index into the
/// out-of-band [`crate::FontTable`] sidecar that rides alongside the
/// packet — the bytes live there, not on the (serializable) packet.
///
/// This is the face the glyph **ids** index into. Shipping any other
/// face (e.g. one re-resolved from `font_family`/`weight`/`style`, which
/// are the *requested* attributes, not necessarily the chosen face) makes
/// the renderer index the wrong outlines on fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct FontFaceId(pub u32);

/// A run of glyphs that share a font + style.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GlyphRun {
    /// Origin of the run, relative to the packet's content origin.
    pub origin: Point,
    pub font_size: f32,
    /// The face the glyph ids in `glyphs` were shaped against (parley's
    /// actual choice). Resolves to bytes via the [`crate::FontTable`]
    /// sidecar.
    pub font_face: FontFaceId,
    /// Requested family label, kept for a11y / debug. May differ from the
    /// face actually shaped against (`font_face`) on fallback — do **not**
    /// use it to resolve render bytes.
    pub font_family: String,
    /// CSS-style weight: 100..900. Requested, not necessarily the chosen
    /// face's own weight. For a11y / debug.
    pub font_weight: u16,
    /// Requested style. For a11y / debug (see `font_weight`).
    pub font_style: TextStyle,
    pub glyphs: Vec<PositionedGlyph>,
    /// Y of the baseline relative to `origin.y`.
    pub baseline_y: f32,
    /// Premultiplied RGBA (0..=1) the run's glyphs paint in, resolved from
    /// the style sheet's color token for this block / inline role (body,
    /// heading, link, code, badge). The renderer paints the glyphs in this
    /// color; it is not re-derived downstream.
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextStyle {
    Normal,
    Italic,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderedBlock {
    /// Index into [`inker::EngineDocument::blocks`] this rendered block
    /// came from. Lets consumers correlate rendered geometry with source
    /// content (for selection, citation, debug overlays, etc.).
    pub source_block_index: usize,
    /// Total bounds of the rendered block, in packet-local coordinates.
    pub bounds: Rect,
    pub kind: RenderedBlockKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RenderedBlockKind {
    /// Block whose visual content is laid-out text (heading, paragraph,
    /// code block, list-item content, metadata row, etc.).
    Text { glyph_runs: Vec<GlyphRun> },
    /// Image — v1 reserves space; downstream renderer fetches + paints
    /// the bytes.
    Image { url: String, alt: String },
    /// Horizontal rule. The bounds is the rule's strip; renderer paints a
    /// hairline at the vertical center.
    Rule,
    /// Container for nested blocks (Quote, List, FeedHeader/Entry, etc.).
    /// Children's bounds are in packet-local coordinates.
    Group { children: Vec<RenderedBlock> },
}

/// Hit-testable region the host translates into a navigation / interaction
/// event when the user clicks / hovers / focuses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InteractionRegion {
    pub bounds: Rect,
    pub kind: InteractionKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum InteractionKind {
    /// Inline link — clicking navigates the URL.
    Link { url: String },
}

/// The output of [`crate::layout_document`]. A pure-data record describing
/// what to render where; no GPU resources, no host types.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentRenderPacket {
    pub viewport: Viewport,
    /// Total content extent. May exceed `viewport.height` (scrolling) but
    /// width matches the viewport's available width.
    pub content_bounds: Rect,
    pub blocks: Vec<RenderedBlock>,
    pub interactions: Vec<InteractionRegion>,
}

impl DocumentRenderPacket {
    /// Derive a packet holding only the content that intersects the vertical band
    /// `[band_y, band_y + band_h]`, with every coordinate translated so the band's
    /// top sits at `y = 0`. The host lowers this windowed packet into a band-tall
    /// texture, so a tall document renders one viewport-sized slice at a time rather
    /// than one giant (capped, failure-prone) texture. `viewport` and `content_bounds`
    /// are set to the band; the caller keeps the full packet for the scroll range and
    /// for windowing the next band. Blocks and interactions outside the band are
    /// dropped; a `Group` keeps only the children that intersect (so a long list
    /// emits only its visible items).
    pub fn window(&self, band_y: f32, band_h: f32) -> DocumentRenderPacket {
        let top = band_y;
        let bot = band_y + band_h;
        let blocks = self
            .blocks
            .iter()
            .filter_map(|b| window_block(b, top, bot))
            .collect();
        let interactions = self
            .interactions
            .iter()
            .filter(|r| rect_intersects_band(r.bounds, top, bot))
            .map(|r| InteractionRegion {
                bounds: translate_rect_y(r.bounds, -band_y),
                kind: r.kind.clone(),
            })
            .collect();
        DocumentRenderPacket {
            viewport: Viewport {
                width: self.viewport.width,
                height: band_h,
                scale_factor: self.viewport.scale_factor,
            },
            content_bounds: Rect::from_xywh(0.0, 0.0, self.content_bounds.size.width, band_h),
            blocks,
            interactions,
        }
    }

    /// The URL of the topmost link whose rect contains `(x, y)`, in full-document
    /// coordinates (the host subtracts the card origin and adds the scroll before
    /// calling). Last match wins, so a link nested in an enclosing region resolves to
    /// the innermost. `None` when the point lands on no link. The host hit-tests
    /// against this retained packet instead of a parallel link-rect table.
    /// (Inline-link nav.)
    pub fn link_at(&self, x: f32, y: f32) -> Option<&str> {
        self.interactions
            .iter()
            .rev()
            .find(|r| rect_contains(r.bounds, x, y))
            .map(|r| match &r.kind {
                InteractionKind::Link { url } => url.as_str(),
            })
    }

    /// The deepest rendered block whose bounds contain `(x, y)` (full-document
    /// coordinates). Walks into `Group` children, so a point inside a list resolves to
    /// the innermost item, not the container. Groundwork for find-in-page and
    /// selection (point to block to source text via `RenderedBlock::source_block_index`).
    pub fn block_at(&self, x: f32, y: f32) -> Option<&RenderedBlock> {
        fn deepest(blocks: &[RenderedBlock], x: f32, y: f32) -> Option<&RenderedBlock> {
            blocks.iter().rev().find_map(|b| {
                if !rect_contains(b.bounds, x, y) {
                    return None;
                }
                if let RenderedBlockKind::Group { children } = &b.kind {
                    if let Some(child) = deepest(children, x, y) {
                        return Some(child);
                    }
                }
                Some(b)
            })
        }
        deepest(&self.blocks, x, y)
    }
}

/// A rect overlaps the half-open vertical band `[top, bot)`.
fn rect_intersects_band(r: Rect, top: f32, bot: f32) -> bool {
    r.origin.y < bot && r.max_y() > top
}

/// Whether `(x, y)` lies within `r` (inclusive edges).
fn rect_contains(r: Rect, x: f32, y: f32) -> bool {
    x >= r.origin.x && x <= r.max_x() && y >= r.origin.y && y <= r.max_y()
}

/// Shift a rect vertically by `dy` (x and size unchanged).
fn translate_rect_y(r: Rect, dy: f32) -> Rect {
    Rect::new(Point::new(r.origin.x, r.origin.y + dy), r.size)
}

/// Window one block to the band: drop it if it does not intersect, else translate
/// its geometry by `-top` and recurse into `Group` children. A `Text` block's runs
/// all sit within its (intersecting) bounds, so each run's origin just shifts.
fn window_block(block: &RenderedBlock, top: f32, bot: f32) -> Option<RenderedBlock> {
    if !rect_intersects_band(block.bounds, top, bot) {
        return None;
    }
    let kind = match &block.kind {
        RenderedBlockKind::Text { glyph_runs } => RenderedBlockKind::Text {
            glyph_runs: glyph_runs
                .iter()
                .map(|run| {
                    let mut run = run.clone();
                    run.origin = Point::new(run.origin.x, run.origin.y - top);
                    run
                })
                .collect(),
        },
        RenderedBlockKind::Group { children } => RenderedBlockKind::Group {
            children: children
                .iter()
                .filter_map(|c| window_block(c, top, bot))
                .collect(),
        },
        RenderedBlockKind::Image { url, alt } => RenderedBlockKind::Image {
            url: url.clone(),
            alt: alt.clone(),
        },
        RenderedBlockKind::Rule => RenderedBlockKind::Rule,
    };
    Some(RenderedBlock {
        source_block_index: block.source_block_index,
        bounds: translate_rect_y(block.bounds, -top),
        kind,
    })
}

#[cfg(test)]
mod window_tests {
    use super::*;

    fn text_block(y: f32, h: f32, run_y: f32) -> RenderedBlock {
        RenderedBlock {
            source_block_index: 0,
            bounds: Rect::from_xywh(0.0, y, 400.0, h),
            kind: RenderedBlockKind::Text {
                glyph_runs: vec![GlyphRun {
                    origin: Point::new(0.0, run_y),
                    font_size: 16.0,
                    font_face: FontFaceId(0),
                    font_family: "x".into(),
                    font_weight: 400,
                    font_style: TextStyle::Normal,
                    glyphs: Vec::new(),
                    baseline_y: 12.0,
                    color: [0.0, 0.0, 0.0, 1.0],
                }],
            },
        }
    }

    fn packet(blocks: Vec<RenderedBlock>, total_h: f32) -> DocumentRenderPacket {
        DocumentRenderPacket {
            viewport: Viewport::new(400.0, 600.0),
            content_bounds: Rect::from_xywh(0.0, 0.0, 400.0, total_h),
            blocks,
            interactions: Vec::new(),
        }
    }

    #[test]
    fn window_keeps_only_intersecting_blocks_translated_to_band_origin() {
        // Three stacked blocks; a band over the middle one keeps only it, shifted so
        // the band top is y=0.
        let p = packet(
            vec![
                text_block(0.0, 100.0, 12.0),
                text_block(1000.0, 100.0, 1012.0),
                text_block(2000.0, 100.0, 2012.0),
            ],
            2100.0,
        );
        let w = p.window(950.0, 200.0); // band [950, 1150]
        assert_eq!(
            w.blocks.len(),
            1,
            "only the middle block intersects the band"
        );
        assert_eq!(
            w.blocks[0].bounds.origin.y, 50.0,
            "block translated by -band_y"
        );
        let RenderedBlockKind::Text { glyph_runs } = &w.blocks[0].kind else {
            panic!("text block");
        };
        assert_eq!(
            glyph_runs[0].origin.y, 62.0,
            "run origin translated by -band_y"
        );
        assert_eq!(w.viewport.height, 200.0, "viewport is the band height");
        assert_eq!(
            w.content_bounds.size.height, 200.0,
            "content_bounds is the band"
        );
        assert_eq!(w.viewport.width, 400.0, "width is preserved");
    }

    #[test]
    fn window_keeps_a_block_straddling_the_band_edge() {
        // A block spanning the band's top edge is kept (partial visibility).
        let p = packet(vec![text_block(900.0, 200.0, 912.0)], 1200.0);
        let w = p.window(1000.0, 200.0); // band [1000, 1200]; block covers [900, 1100]
        assert_eq!(w.blocks.len(), 1, "the straddling block is kept");
        assert_eq!(
            w.blocks[0].bounds.origin.y, -100.0,
            "its top is above the band, at -100"
        );
    }

    #[test]
    fn window_filters_group_children_to_the_band() {
        // A group spanning the whole document keeps only the children in the band.
        let group = RenderedBlock {
            source_block_index: 0,
            bounds: Rect::from_xywh(0.0, 0.0, 400.0, 3000.0),
            kind: RenderedBlockKind::Group {
                children: vec![
                    text_block(0.0, 50.0, 12.0),
                    text_block(1500.0, 50.0, 1512.0),
                    text_block(2900.0, 50.0, 2912.0),
                ],
            },
        };
        let w = packet(vec![group], 3000.0).window(1400.0, 200.0); // band [1400, 1600]
        let RenderedBlockKind::Group { children } = &w.blocks[0].kind else {
            panic!("group survives");
        };
        assert_eq!(children.len(), 1, "only the in-band child is kept");
        assert_eq!(
            children[0].bounds.origin.y, 100.0,
            "child translated into the band"
        );
    }

    #[test]
    fn window_filters_and_translates_interactions() {
        let mut p = packet(vec![text_block(1000.0, 100.0, 1012.0)], 2000.0);
        p.interactions = vec![
            InteractionRegion {
                bounds: Rect::from_xywh(0.0, 1010.0, 80.0, 20.0),
                kind: InteractionKind::Link { url: "in".into() },
            },
            InteractionRegion {
                bounds: Rect::from_xywh(0.0, 50.0, 80.0, 20.0),
                kind: InteractionKind::Link { url: "out".into() },
            },
        ];
        let w = p.window(950.0, 200.0);
        assert_eq!(w.interactions.len(), 1, "only the in-band link survives");
        assert_eq!(
            w.interactions[0].bounds.origin.y, 60.0,
            "link translated into the band"
        );
        assert!(matches!(&w.interactions[0].kind, InteractionKind::Link { url } if url == "in"));
    }

    #[test]
    fn link_at_returns_the_innermost_link_url() {
        let mut p = packet(vec![text_block(0.0, 100.0, 12.0)], 200.0);
        p.interactions = vec![
            InteractionRegion {
                bounds: Rect::from_xywh(0.0, 10.0, 80.0, 20.0),
                kind: InteractionKind::Link {
                    url: "outer".into(),
                },
            },
            InteractionRegion {
                bounds: Rect::from_xywh(10.0, 12.0, 40.0, 16.0),
                kind: InteractionKind::Link {
                    url: "inner".into(),
                },
            },
        ];
        assert_eq!(
            p.link_at(20.0, 18.0),
            Some("inner"),
            "the last (innermost) match wins"
        );
        assert_eq!(
            p.link_at(70.0, 18.0),
            Some("outer"),
            "outside inner, inside outer"
        );
        assert_eq!(p.link_at(300.0, 300.0), None, "no link at an empty point");
    }

    #[test]
    fn block_at_finds_the_deepest_block() {
        let mut child = text_block(100.0, 50.0, 112.0);
        child.source_block_index = 7;
        let group = RenderedBlock {
            source_block_index: 0,
            bounds: Rect::from_xywh(0.0, 0.0, 400.0, 300.0),
            kind: RenderedBlockKind::Group {
                children: vec![child],
            },
        };
        let p = packet(vec![group], 300.0);
        assert_eq!(
            p.block_at(10.0, 120.0).map(|b| b.source_block_index),
            Some(7),
            "a point inside the child resolves to the child, not the group"
        );
        assert_eq!(
            p.block_at(10.0, 290.0).map(|b| b.source_block_index),
            Some(0),
            "a point in the group but outside any child resolves to the group"
        );
        assert!(
            p.block_at(500.0, 500.0).is_none(),
            "no block at an outside point"
        );
    }
}
