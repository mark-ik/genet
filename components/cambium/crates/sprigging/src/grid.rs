// SPDX-License-Identifier: MIT OR Apache-2.0

//! Data-grid math (the catalog's flagship arrangement-leaf widget): a column
//! model over [`VirtualWindow`](crate::VirtualWindow) row virtualization.
//! Pure, engine-free; the view half (`cambium::data_grid`) turns it into
//! a sticky header + virtualized, absolutely-placed rows.

use crate::arrange::VirtualWindow;

/// One column: a header title and a fixed width (device px). Variable /
/// weighted widths are a later refinement.
#[derive(Clone, Debug, PartialEq)]
pub struct GridColumn {
    pub title: String,
    pub width: f32,
}

impl GridColumn {
    pub fn new(title: impl Into<String>, width: f32) -> Self {
        Self {
            title: title.into(),
            width,
        }
    }
}

/// The grid's shape: columns plus row/header pitch. Row *content* stays with
/// the caller (a cell-view function); this is geometry only.
#[derive(Clone, Debug, PartialEq)]
pub struct GridSpec {
    pub columns: Vec<GridColumn>,
    pub row_height: f32,
    pub header_height: f32,
    /// Rows materialized beyond the viewport on each side.
    pub overscan: usize,
}

impl GridSpec {
    /// Column `i`'s x offset (prefix sum of widths).
    pub fn col_x(&self, i: usize) -> f32 {
        self.columns.iter().take(i).map(|c| c.width).sum()
    }

    pub fn total_width(&self) -> f32 {
        self.col_x(self.columns.len())
    }

    /// The virtualization window for `total_rows` at `scroll` in a
    /// `viewport_height` body (header excluded; the header never scrolls).
    pub fn window(&self, total_rows: usize, viewport_height: f32, scroll: f32) -> VirtualWindow {
        VirtualWindow {
            total_rows,
            row_height: self.row_height,
            viewport_height,
            scroll,
            overscan: self.overscan,
        }
    }

    /// The largest scroll that still shows a full viewport (clamp target for
    /// wheel handlers); `0` when everything fits.
    pub fn max_scroll(&self, total_rows: usize, viewport_height: f32) -> f32 {
        (total_rows as f32 * self.row_height - viewport_height).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> GridSpec {
        GridSpec {
            columns: vec![
                GridColumn::new("name", 120.0),
                GridColumn::new("value", 80.0),
                GridColumn::new("trend", 100.0),
            ],
            row_height: 24.0,
            header_height: 28.0,
            overscan: 2,
        }
    }

    #[test]
    fn columns_lay_out_by_prefix_sum() {
        let s = spec();
        assert_eq!(s.col_x(0), 0.0);
        assert_eq!(s.col_x(1), 120.0);
        assert_eq!(s.col_x(2), 200.0);
        assert_eq!(s.total_width(), 300.0);
    }

    #[test]
    fn window_and_scroll_clamp_are_consistent() {
        let s = spec();
        let w = s.window(10_000, 300.0, 4800.0);
        assert_eq!(w.range().start, 198);
        assert!(w.range().len() < 25);
        assert_eq!(s.max_scroll(10_000, 300.0), 239_700.0);
        assert_eq!(s.max_scroll(5, 300.0), 0.0, "everything fits: no scroll");
    }
}
