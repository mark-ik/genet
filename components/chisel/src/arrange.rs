//! Arrangement-leaf math (catalog tier 3): pure placement + virtualization
//! helpers for a container that owns its children's x/y/z while the children
//! stay real, first-class DOM nodes (hit-test, a11y, paint all engine-native).
//! The view half (`xilem_serval::placed` / `arrangement`) turns these into
//! absolutely-positioned elements; this module is engine-free math so hosts
//! that place without the view layer (a card compositor) reuse it.
//! See `docs/2026-07-08_chisel_widget_catalog.md`.

/// Where an arranged child sits, in the container's local px. The child sizes
/// itself (its own CSS); the arrangement owns position and stacking only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    /// CSS `z-index` within the container's stacking context: raise a card by
    /// giving it the highest z (serval's `paint_stacking` orders natively).
    pub z: i32,
}

impl Placement {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y, z: 0 }
    }

    pub fn with_z(mut self, z: i32) -> Self {
        self.z = z;
        self
    }

    /// The inline style that realizes this placement on a wrapper element.
    pub fn style(&self) -> String {
        format!(
            "position: absolute; left: {}px; top: {}px; z-index: {};",
            self.x, self.y, self.z
        )
    }
}

/// Fixed-row-height virtualization: which rows of a large collection to
/// materialize for the current viewport, and where. CSS cannot say
/// "materialize rows 400..430 of 100k"; this + an arrangement container can:
/// the container takes the full [`total_height`](Self::total_height) (so
/// scrollbars and scroll ranges stay honest) while only
/// [`range`](Self::range) rows exist as DOM.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VirtualWindow {
    pub total_rows: usize,
    /// Row pitch, device px (fixed-height virtualization; variable heights are
    /// a later refinement).
    pub row_height: f32,
    pub viewport_height: f32,
    /// Scroll offset into the full extent, device px.
    pub scroll: f32,
    /// Extra rows materialized above and below the viewport so small scrolls
    /// reveal painted content before the next rebuild.
    pub overscan: usize,
}

impl VirtualWindow {
    /// The rows to materialize (clamped to the collection).
    pub fn range(&self) -> std::ops::Range<usize> {
        if self.total_rows == 0 || self.row_height <= 0.0 {
            return 0..0;
        }
        let first = (self.scroll / self.row_height).floor().max(0.0) as usize;
        let visible = (self.viewport_height / self.row_height).ceil() as usize + 1;
        let start = first.saturating_sub(self.overscan);
        let end = (first + visible + self.overscan).min(self.total_rows);
        start..end
    }

    /// The full content extent the container should claim, so the scroll range
    /// matches the whole collection.
    pub fn total_height(&self) -> f32 {
        self.total_rows as f32 * self.row_height
    }

    /// Row `i`'s placement (top-left, base z).
    pub fn row_placement(&self, i: usize) -> Placement {
        Placement::new(0.0, i as f32 * self.row_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_materializes_a_slice_not_the_collection() {
        let vw = VirtualWindow {
            total_rows: 10_000,
            row_height: 24.0,
            viewport_height: 300.0,
            scroll: 4800.0,
            overscan: 3,
        };
        let r = vw.range();
        assert_eq!(r.start, 197, "first visible 200 minus overscan");
        assert_eq!(r.end, 217, "200 + ceil(300/24)+1 = 214 plus overscan");
        assert!(r.len() < 30, "a sliver of 10k rows");
        assert_eq!(vw.total_height(), 240_000.0);
        assert_eq!(vw.row_placement(200).y, 4800.0);
    }

    #[test]
    fn window_clamps_at_the_edges() {
        let vw = VirtualWindow {
            total_rows: 10,
            row_height: 24.0,
            viewport_height: 300.0,
            scroll: 0.0,
            overscan: 5,
        };
        assert_eq!(vw.range(), 0..10, "clamped to the collection");
        let empty = VirtualWindow { total_rows: 0, ..vw };
        assert_eq!(empty.range(), 0..0);
    }

    #[test]
    fn placement_style_realizes_position_and_z() {
        let s = Placement::new(12.0, 34.0).with_z(7).style();
        assert!(s.contains("left: 12px") && s.contains("top: 34px") && s.contains("z-index: 7"));
    }
}
