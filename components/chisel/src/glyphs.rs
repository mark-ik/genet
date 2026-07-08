//! First catalog leaves (tier-2 Path-A glyphs from the widget catalog,
//! `docs/2026-07-08_chisel_widget_catalog.md`): [`GraphGlyph`], [`Meter`],
//! [`Knob`]. Pure geometry: data in, `PaintCmd`s out. Labels/values belong in
//! DOM siblings, interaction in the view layer (`on_pointer` around the leaf).

use paint_list_api::ColorF;

use crate::path::Path;
use crate::{Leaf, PaintCx, Size, SizeHint, round_stroke, solid_stroke};

/// One node of a [`GraphGlyph`], in normalized `0..1` coordinates (scaled to
/// the leaf's box at paint time).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphGlyphNode {
    pub x: f32,
    pub y: f32,
    pub color: ColorF,
}

/// A miniature node-link graph: the "button with the graph on it" glyph, also
/// link previews, breadcrumb thumbnails, hover cards. Layout is
/// caller-precomputed (the leaf stays dumb); node coordinates are normalized
/// `0..1` and scaled to the box.
pub struct GraphGlyph {
    nodes: Vec<GraphGlyphNode>,
    /// Index pairs into `nodes`; out-of-range pairs are skipped.
    edges: Vec<(u16, u16)>,
    pub node_radius: f32,
    pub edge_width: f32,
    pub edge_color: ColorF,
    intrinsic: Size,
    dirty: bool,
}

impl GraphGlyph {
    pub fn new(nodes: Vec<GraphGlyphNode>, edges: Vec<(u16, u16)>, intrinsic: Size) -> Self {
        Self {
            nodes,
            edges,
            node_radius: 2.5,
            edge_width: 1.0,
            edge_color: ColorF { r: 0.5, g: 0.5, b: 0.55, a: 1.0 },
            intrinsic,
            dirty: true,
        }
    }

    /// Replace the graph data (layout recompute stays with the caller).
    pub fn set_graph(&mut self, nodes: Vec<GraphGlyphNode>, edges: Vec<(u16, u16)>) {
        self.nodes = nodes;
        self.edges = edges;
        self.dirty = true;
    }
}

impl Leaf for GraphGlyph {
    fn measure(&mut self, _known: SizeHint, _available: SizeHint) -> Size {
        self.intrinsic
    }

    fn paint(&mut self, cx: &mut PaintCx<'_>) {
        let s = cx.size();
        // Inset so node circles at 0/1 coordinates stay inside the box.
        let inset = self.node_radius + self.edge_width;
        let (w, h) = (
            (s.width - 2.0 * inset).max(0.0),
            (s.height - 2.0 * inset).max(0.0),
        );
        let place = |n: &GraphGlyphNode| (inset + n.x * w, inset + n.y * h);
        // Edges under nodes.
        for &(a, b) in &self.edges {
            let (Some(na), Some(nb)) = (self.nodes.get(a as usize), self.nodes.get(b as usize))
            else {
                continue;
            };
            let (ax, ay) = place(na);
            let (bx, by) = place(nb);
            cx.stroke_path(
                Path::polyline(&[(ax, ay), (bx, by)]),
                round_stroke(self.edge_color, self.edge_width),
            );
        }
        for n in &self.nodes {
            let (x, y) = place(n);
            cx.fill_path(Path::circle(x, y, self.node_radius), n.color);
        }
        self.dirty = false;
    }

    fn paint_dirty(&self) -> bool {
        self.dirty
    }
}

/// A level meter bar: track + proportional fill + optional peak tick.
/// Vertical fills bottom-up, horizontal left-to-right. Value/peak are `0..=1`.
pub struct Meter {
    value: f32,
    peak: Option<f32>,
    pub vertical: bool,
    pub track_color: ColorF,
    pub fill_color: ColorF,
    pub peak_color: ColorF,
    /// Peak tick thickness, device px.
    pub peak_thickness: f32,
    intrinsic: Size,
    dirty: bool,
}

impl Meter {
    pub fn new(vertical: bool, intrinsic: Size) -> Self {
        Self {
            value: 0.0,
            peak: None,
            vertical,
            track_color: ColorF { r: 0.12, g: 0.12, b: 0.14, a: 1.0 },
            fill_color: ColorF { r: 0.30, g: 0.80, b: 0.35, a: 1.0 },
            peak_color: ColorF { r: 0.95, g: 0.85, b: 0.25, a: 1.0 },
            peak_thickness: 2.0,
            intrinsic,
            dirty: true,
        }
    }

    /// Set level + peak, clamped to `0..=1`; dirties only on change.
    pub fn set_level(&mut self, value: f32, peak: Option<f32>) {
        let value = value.clamp(0.0, 1.0);
        let peak = peak.map(|p| p.clamp(0.0, 1.0));
        if value != self.value || peak != self.peak {
            self.value = value;
            self.peak = peak;
            self.dirty = true;
        }
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}

impl Leaf for Meter {
    fn measure(&mut self, _known: SizeHint, _available: SizeHint) -> Size {
        self.intrinsic
    }

    fn paint(&mut self, cx: &mut PaintCx<'_>) {
        let s = cx.size();
        cx.fill_rect(0.0, 0.0, s.width, s.height, self.track_color);
        if self.value > 0.0 {
            if self.vertical {
                let fh = s.height * self.value;
                cx.fill_rect(0.0, s.height - fh, s.width, fh, self.fill_color);
            } else {
                cx.fill_rect(0.0, 0.0, s.width * self.value, s.height, self.fill_color);
            }
        }
        if let Some(p) = self.peak {
            let t = self.peak_thickness;
            if self.vertical {
                let y = ((1.0 - p) * s.height).min(s.height - t);
                cx.fill_rect(0.0, y, s.width, t, self.peak_color);
            } else {
                let x = (p * s.width - t).max(0.0);
                cx.fill_rect(x, 0.0, t, s.height, self.peak_color);
            }
        }
        self.dirty = false;
    }

    fn paint_dirty(&self) -> bool {
        self.dirty
    }
}

/// Sweep geometry shared by the knob arcs: 270 degrees from lower-left
/// (135 deg, screen convention y-down) clockwise through up to lower-right.
const KNOB_START: f32 = 135.0 * std::f32::consts::PI / 180.0;
const KNOB_SWEEP: f32 = 270.0 * std::f32::consts::PI / 180.0;

/// A rotary knob: track arc + value arc + needle. Value is `0..=1`. Pointer
/// interaction belongs to the wrapping view (`on_pointer` maps drag delta to
/// `set_value`); the leaf only paints.
pub struct Knob {
    value: f32,
    pub track_color: ColorF,
    pub value_color: ColorF,
    pub needle_color: ColorF,
    pub arc_width: f32,
    intrinsic: Size,
    dirty: bool,
}

impl Knob {
    pub fn new(intrinsic: Size) -> Self {
        Self {
            value: 0.0,
            track_color: ColorF { r: 0.20, g: 0.20, b: 0.24, a: 1.0 },
            value_color: ColorF { r: 0.40, g: 0.60, b: 0.95, a: 1.0 },
            needle_color: ColorF { r: 0.90, g: 0.90, b: 0.92, a: 1.0 },
            arc_width: 3.0,
            intrinsic,
            dirty: true,
        }
    }

    /// Set the value, clamped to `0..=1`; dirties only on change.
    pub fn set_value(&mut self, value: f32) {
        let value = value.clamp(0.0, 1.0);
        if value != self.value {
            self.value = value;
            self.dirty = true;
        }
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}

impl Leaf for Knob {
    fn measure(&mut self, _known: SizeHint, _available: SizeHint) -> Size {
        self.intrinsic
    }

    fn paint(&mut self, cx: &mut PaintCx<'_>) {
        let s = cx.size();
        let (cx0, cy0) = (s.width * 0.5, s.height * 0.5);
        let r = (s.width.min(s.height) * 0.5 - self.arc_width).max(1.0);
        // Track: the full sweep.
        cx.stroke_path(
            Path::new().arc(cx0, cy0, r, KNOB_START, KNOB_START + KNOB_SWEEP).build(),
            round_stroke(self.track_color, self.arc_width),
        );
        // Value arc over it.
        let a = KNOB_START + KNOB_SWEEP * self.value;
        if self.value > 0.0 {
            cx.stroke_path(
                Path::new().arc(cx0, cy0, r, KNOB_START, a).build(),
                round_stroke(self.value_color, self.arc_width),
            );
        }
        // Needle from 30% radius out to the arc, at the value angle.
        cx.stroke_path(
            Path::polyline(&[
                (cx0 + 0.3 * r * a.cos(), cy0 + 0.3 * r * a.sin()),
                (cx0 + r * a.cos(), cy0 + r * a.sin()),
            ]),
            round_stroke(self.needle_color, self.arc_width * 0.66),
        );
        self.dirty = false;
    }

    fn paint_dirty(&self) -> bool {
        self.dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paint_list_api::PaintCmd;

    fn white() -> ColorF {
        ColorF { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }
    }

    fn paint_of(leaf: &mut dyn Leaf, w: f32, h: f32) -> Vec<PaintCmd> {
        let mut cmds = Vec::new();
        let mut cx = PaintCx::new(&mut cmds, Size { width: w, height: h });
        leaf.paint(&mut cx);
        cmds
    }

    fn count(cmds: &[PaintCmd], pred: impl Fn(&PaintCmd) -> bool) -> usize {
        cmds.iter().filter(|c| pred(c)).count()
    }

    #[test]
    fn graph_glyph_paints_edges_under_nodes() {
        let mut g = GraphGlyph::new(
            vec![
                GraphGlyphNode { x: 0.0, y: 0.0, color: white() },
                GraphGlyphNode { x: 1.0, y: 0.5, color: white() },
                GraphGlyphNode { x: 0.5, y: 1.0, color: white() },
            ],
            vec![(0, 1), (1, 2), (7, 0)], // (7,0) out of range: skipped
            Size { width: 20.0, height: 20.0 },
        );
        let cmds = paint_of(&mut g, 20.0, 20.0);
        let strokes = count(&cmds, |c| {
            matches!(c, PaintCmd::DrawPath(p) if p.stroke.is_some())
        });
        let fills = count(&cmds, |c| {
            matches!(c, PaintCmd::DrawPath(p) if p.fill.is_some())
        });
        assert_eq!(strokes, 2, "two in-range edges");
        assert_eq!(fills, 3, "three node circles");
        // Painter order: edges first.
        assert!(matches!(&cmds[0], PaintCmd::DrawPath(p) if p.stroke.is_some()));
        assert!(!g.paint_dirty());
    }

    #[test]
    fn meter_paints_track_fill_peak_and_gates_on_change() {
        let mut m = Meter::new(true, Size { width: 8.0, height: 40.0 });
        m.set_level(0.5, Some(0.8));
        let cmds = paint_of(&mut m, 8.0, 40.0);
        assert_eq!(
            count(&cmds, |c| matches!(c, PaintCmd::DrawRect(_))),
            3,
            "track + fill + peak"
        );
        assert!(!m.paint_dirty());
        m.set_level(0.5, Some(0.8));
        assert!(!m.paint_dirty(), "same level does not dirty");
        m.set_level(0.6, Some(0.8));
        assert!(m.paint_dirty(), "changed level dirties");
    }

    #[test]
    fn meter_at_zero_paints_track_only() {
        let mut m = Meter::new(false, Size { width: 40.0, height: 8.0 });
        m.set_level(0.0, None);
        let cmds = paint_of(&mut m, 40.0, 8.0);
        assert_eq!(count(&cmds, |c| matches!(c, PaintCmd::DrawRect(_))), 1);
    }

    #[test]
    fn knob_paints_track_value_needle_and_clamps() {
        let mut k = Knob::new(Size { width: 24.0, height: 24.0 });
        k.set_value(1.7);
        assert_eq!(k.value(), 1.0, "clamped");
        let cmds = paint_of(&mut k, 24.0, 24.0);
        let strokes = count(&cmds, |c| {
            matches!(c, PaintCmd::DrawPath(p) if p.stroke.is_some())
        });
        assert_eq!(strokes, 3, "track arc + value arc + needle");

        // At zero the value arc is skipped.
        let mut k0 = Knob::new(Size { width: 24.0, height: 24.0 });
        let cmds0 = paint_of(&mut k0, 24.0, 24.0);
        let strokes0 = count(&cmds0, |c| {
            matches!(c, PaintCmd::DrawPath(p) if p.stroke.is_some())
        });
        assert_eq!(strokes0, 2, "track + needle only");
    }
}
