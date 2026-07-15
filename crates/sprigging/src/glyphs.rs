// SPDX-License-Identifier: MIT OR Apache-2.0

//! First catalog leaves (tier-2 Path-A glyphs from the widget catalog,
//! `docs/history/2026-07-08_chisel_widget_catalog.md`): [`GraphCanvas`], [`Meter`],
//! [`Knob`]. Pure geometry: data in, `PaintCmd`s out. Labels/values belong in
//! DOM siblings, interaction in the view layer over the leaf.

use paint_list_api::ColorF;

use crate::path::Path;
use crate::{Leaf, PaintCx, Size, SizeHint, round_stroke};

/// One node of a [`GraphCanvas`], in normalized `0..1` coordinates (scaled to
/// the leaf's box at paint time).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphGlyphNode {
    pub x: f32,
    pub y: f32,
    pub color: ColorF,
}

/// Pane-local camera for a [`GraphCanvas`]. Pan is expressed in normalized
/// scene coordinates; zoom is centred on `(0.5, 0.5)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphViewport {
    pub pan: (f32, f32),
    pub zoom: f32,
}

impl Default for GraphViewport {
    fn default() -> Self {
        Self {
            pan: (0.0, 0.0),
            zoom: 1.0,
        }
    }
}

impl GraphViewport {
    /// Project one normalized scene point into a leaf-local box. View-layer hit
    /// targets use this same function, keeping paint and interaction aligned.
    pub fn project(self, point: (f32, f32), size: Size, inset: f32) -> (f32, f32) {
        let zoom = self.zoom.max(0.01);
        let x = (point.0 - 0.5) * zoom + 0.5 + self.pan.0;
        let y = (point.1 - 0.5) * zoom + 0.5 + self.pan.1;
        let width = (size.width - 2.0 * inset).max(0.0);
        let height = (size.height - 2.0 * inset).max(0.0);
        (inset + x * width, inset + y * height)
    }
}

/// A node-link canvas shared by full panes and bounded glyphs such as link
/// previews, breadcrumb thumbnails, and hover cards. Layout is caller-computed;
/// normalized node coordinates are projected through a pane-local viewport.
pub struct GraphCanvas {
    nodes: Vec<GraphGlyphNode>,
    /// Index pairs into `nodes`; out-of-range pairs are skipped.
    edges: Vec<(u16, u16)>,
    pub node_radius: f32,
    pub edge_width: f32,
    pub edge_color: ColorF,
    /// Ring around the selected node.
    pub selection_color: ColorF,
    /// Outer ring around the keyboard-focused node.
    pub focus_color: ColorF,
    viewport: GraphViewport,
    selected: Option<u16>,
    focus: Option<u16>,
    hovered: Option<u16>,
    intrinsic: Size,
    dirty: bool,
}

impl GraphCanvas {
    pub fn new(nodes: Vec<GraphGlyphNode>, edges: Vec<(u16, u16)>, intrinsic: Size) -> Self {
        Self {
            nodes,
            edges,
            node_radius: 2.5,
            edge_width: 1.0,
            edge_color: ColorF {
                r: 0.5,
                g: 0.5,
                b: 0.55,
                a: 1.0,
            },
            selection_color: ColorF {
                r: 0.95,
                g: 0.72,
                b: 0.25,
                a: 1.0,
            },
            focus_color: ColorF {
                r: 0.30,
                g: 0.58,
                b: 0.95,
                a: 1.0,
            },
            viewport: GraphViewport::default(),
            selected: None,
            focus: None,
            hovered: None,
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

    /// Change the pane-local camera. A bounded swatch and a full canvas use the
    /// same projection; only their viewport and measured box differ.
    pub fn set_viewport(&mut self, viewport: GraphViewport) {
        if self.viewport != viewport {
            self.viewport = viewport;
            self.dirty = true;
        }
    }

    pub fn viewport(&self) -> GraphViewport {
        self.viewport
    }

    /// Set transient and durable emphasis by node index.
    pub fn set_emphasis(
        &mut self,
        selected: Option<u16>,
        focus: Option<u16>,
        hovered: Option<u16>,
    ) {
        if (self.selected, self.focus, self.hovered) != (selected, focus, hovered) {
            self.selected = selected;
            self.focus = focus;
            self.hovered = hovered;
            self.dirty = true;
        }
    }

    /// Project a node through the exact camera and inset used by [`Leaf::paint`].
    pub fn node_local_position(&self, index: usize, size: Size) -> Option<(f32, f32)> {
        let node = self.nodes.get(index)?;
        Some(
            self.viewport
                .project((node.x, node.y), size, self.node_radius + self.edge_width),
        )
    }
}

impl Leaf for GraphCanvas {
    /// A node-link glyph announces as a graphics object. Its interior structure
    /// (which node, which link) is not reachable through a single AccessKit node;
    /// publishing the nodes as semantic children is the leaf-publication work the
    /// native-automation plan scopes to phase 2 proper. Until then a screen reader
    /// at least learns that this is a graph and how big it is.
    ///
    /// The name is a **fallback only**. This glyph is reused as a button face, a
    /// link preview, and a breadcrumb thumbnail, so the author who placed it knows
    /// what it depicts and says so with `aria-label`; a generic self-description
    /// must never overwrite that.
    fn accessibility(&mut self, node: &mut accesskit::Node) {
        node.set_role(accesskit::Role::GraphicsObject);
        if node.label().is_none() {
            node.set_label(format!(
                "graph: {} nodes, {} links",
                self.nodes.len(),
                self.edges.len()
            ));
        }
    }

    fn measure(&mut self, _known: SizeHint, _available: SizeHint) -> Size {
        self.intrinsic
    }

    fn paint(&mut self, cx: &mut PaintCx<'_>) {
        let s = cx.size();
        // Inset so node circles at 0/1 coordinates stay inside the box.
        let inset = self.node_radius + self.edge_width;
        let place = |n: &GraphGlyphNode| self.viewport.project((n.x, n.y), s, inset);
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
        for (index, n) in self.nodes.iter().enumerate() {
            let (x, y) = place(n);
            if self.hovered == u16::try_from(index).ok() {
                let mut wash = n.color;
                wash.a *= 0.24;
                cx.fill_path(Path::circle(x, y, self.node_radius * 1.9), wash);
            }
            if self.selected == u16::try_from(index).ok() {
                cx.stroke_path(
                    Path::circle(x, y, self.node_radius + 2.0),
                    round_stroke(self.selection_color, 1.5),
                );
            }
            if self.focus == u16::try_from(index).ok() {
                cx.stroke_path(
                    Path::circle(x, y, self.node_radius + 4.0),
                    round_stroke(self.focus_color, 1.0),
                );
            }
            cx.fill_path(Path::circle(x, y, self.node_radius), n.color);
        }
        self.dirty = false;
    }

    fn paint_dirty(&self) -> bool {
        self.dirty
    }
}

/// Compatibility name for the original miniature-only graph leaf. `GraphGlyph`
/// and `GraphCanvas` are the same renderer; the latter names its use for both
/// bounded swatches and full canvas panes.
pub type GraphGlyph = GraphCanvas;

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
            track_color: ColorF {
                r: 0.12,
                g: 0.12,
                b: 0.14,
                a: 1.0,
            },
            fill_color: ColorF {
                r: 0.30,
                g: 0.80,
                b: 0.35,
                a: 1.0,
            },
            peak_color: ColorF {
                r: 0.95,
                g: 0.85,
                b: 0.25,
                a: 1.0,
            },
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
    /// A level meter announces as [`accesskit::Role::Meter`] carrying its normalized level.
    /// Read-only: a meter reports, it is not actuated, so it declares no action
    /// and never lands in the host's routable set.
    fn accessibility(&mut self, node: &mut accesskit::Node) {
        node.set_role(accesskit::Role::Meter);
        node.set_numeric_value(self.value as f64);
        node.set_min_numeric_value(0.0);
        node.set_max_numeric_value(1.0);
    }

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
            track_color: ColorF {
                r: 0.20,
                g: 0.20,
                b: 0.24,
                a: 1.0,
            },
            value_color: ColorF {
                r: 0.40,
                g: 0.60,
                b: 0.95,
                a: 1.0,
            },
            needle_color: ColorF {
                r: 0.90,
                g: 0.90,
                b: 0.92,
                a: 1.0,
            },
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
    /// A knob announces as [`accesskit::Role::Slider`] carrying its normalized value, and
    /// declares the actions assistive tech and automation invoke on a slider.
    /// Declaring them here is what puts the knob in the host's routable set, so
    /// a leaf interior is actuated through the same path a `<button>` is.
    fn accessibility(&mut self, node: &mut accesskit::Node) {
        node.set_role(accesskit::Role::Slider);
        node.set_numeric_value(self.value as f64);
        node.set_min_numeric_value(0.0);
        node.set_max_numeric_value(1.0);
        node.add_action(accesskit::Action::SetValue);
        node.add_action(accesskit::Action::Increment);
        node.add_action(accesskit::Action::Decrement);
    }

    fn measure(&mut self, _known: SizeHint, _available: SizeHint) -> Size {
        self.intrinsic
    }

    fn paint(&mut self, cx: &mut PaintCx<'_>) {
        let s = cx.size();
        let (cx0, cy0) = (s.width * 0.5, s.height * 0.5);
        let r = (s.width.min(s.height) * 0.5 - self.arc_width).max(1.0);
        // Track: the full sweep.
        cx.stroke_path(
            Path::new()
                .arc(cx0, cy0, r, KNOB_START, KNOB_START + KNOB_SWEEP)
                .build(),
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
        ColorF {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        }
    }

    fn paint_of(leaf: &mut dyn Leaf, w: f32, h: f32) -> Vec<PaintCmd> {
        let mut cmds = Vec::new();
        let mut cx = PaintCx::new(
            &mut cmds,
            Size {
                width: w,
                height: h,
            },
        );
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
                GraphGlyphNode {
                    x: 0.0,
                    y: 0.0,
                    color: white(),
                },
                GraphGlyphNode {
                    x: 1.0,
                    y: 0.5,
                    color: white(),
                },
                GraphGlyphNode {
                    x: 0.5,
                    y: 1.0,
                    color: white(),
                },
            ],
            vec![(0, 1), (1, 2), (7, 0)], // (7,0) out of range: skipped
            Size {
                width: 20.0,
                height: 20.0,
            },
        );
        let cmds = paint_of(&mut g, 20.0, 20.0);
        let strokes = count(
            &cmds,
            |c| matches!(c, PaintCmd::DrawPath(p) if p.stroke.is_some()),
        );
        let fills = count(
            &cmds,
            |c| matches!(c, PaintCmd::DrawPath(p) if p.fill.is_some()),
        );
        assert_eq!(strokes, 2, "two in-range edges");
        assert_eq!(fills, 3, "three node circles");
        // Painter order: edges first.
        assert!(matches!(&cmds[0], PaintCmd::DrawPath(p) if p.stroke.is_some()));
        assert!(!g.paint_dirty());
    }

    #[test]
    fn graph_canvas_camera_and_emphasis_share_one_paint_path() {
        let mut canvas = GraphCanvas::new(
            vec![GraphGlyphNode {
                x: 0.25,
                y: 0.5,
                color: white(),
            }],
            Vec::new(),
            Size {
                width: 100.0,
                height: 60.0,
            },
        );
        canvas.set_viewport(GraphViewport {
            pan: (0.1, 0.0),
            zoom: 2.0,
        });
        canvas.set_emphasis(Some(0), Some(0), Some(0));
        let local = canvas
            .node_local_position(
                0,
                Size {
                    width: 100.0,
                    height: 60.0,
                },
            )
            .expect("projected node");
        assert!(local.0 < 50.0, "zoom and pan project through the camera");

        let cmds = paint_of(&mut canvas, 100.0, 60.0);
        assert_eq!(
            count(&cmds, |cmd| matches!(cmd, PaintCmd::DrawPath(_))),
            4,
            "hover wash + selection ring + focus ring + node"
        );
    }

    #[test]
    fn meter_paints_track_fill_peak_and_gates_on_change() {
        let mut m = Meter::new(
            true,
            Size {
                width: 8.0,
                height: 40.0,
            },
        );
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
        let mut m = Meter::new(
            false,
            Size {
                width: 40.0,
                height: 8.0,
            },
        );
        m.set_level(0.0, None);
        let cmds = paint_of(&mut m, 40.0, 8.0);
        assert_eq!(count(&cmds, |c| matches!(c, PaintCmd::DrawRect(_))), 1);
    }

    #[test]
    fn knob_paints_track_value_needle_and_clamps() {
        let mut k = Knob::new(Size {
            width: 24.0,
            height: 24.0,
        });
        k.set_value(1.7);
        assert_eq!(k.value(), 1.0, "clamped");
        let cmds = paint_of(&mut k, 24.0, 24.0);
        let strokes = count(
            &cmds,
            |c| matches!(c, PaintCmd::DrawPath(p) if p.stroke.is_some()),
        );
        assert_eq!(strokes, 3, "track arc + value arc + needle");

        // At zero the value arc is skipped.
        let mut k0 = Knob::new(Size {
            width: 24.0,
            height: 24.0,
        });
        let cmds0 = paint_of(&mut k0, 24.0, 24.0);
        let strokes0 = count(
            &cmds0,
            |c| matches!(c, PaintCmd::DrawPath(p) if p.stroke.is_some()),
        );
        assert_eq!(strokes0, 2, "track + needle only");
    }
}
