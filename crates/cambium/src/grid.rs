/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The data grid (Sprigging's flagship arrangement widget): a sticky
//! header over virtualized, absolutely-placed rows.
//!
//! Geometry comes from [`sprigging::GridSpec`]; row *content* comes from the
//! caller's cell function (any view — text, a `custom_leaf` sparkline, a
//! button), so the grid owns arrangement and nothing else. Only the
//! [`VirtualWindow`](sprigging::VirtualWindow) rows exist as DOM; scrolling is
//! caller state (wire [`on_wheel`](crate::on_wheel) around the grid, clamping
//! with [`GridSpec::max_scroll`]), so the header is "sticky" by construction —
//! it simply never scrolls. Header cells fire `on_header_click(col)` on click,
//! Enter, or Space for sort-by-column, which is likewise caller state: the grid
//! re-renders whatever order the cell function exposes.
//!
//! The root, headers, rows, and cells expose the ARIA grid roles and indices.
//! Only materialized rows exist in the DOM, while `aria-rowcount` reports the
//! full model size.
//!
//! Theming rides classes (`grid`, `grid-header`, `grid-header-cell`,
//! `grid-body`, `grid-row`, `grid-row-even` / `-odd`, `grid-cell`); inline
//! styles carry only geometry.

use sprigging::{GridSpec, Placement};

use crate::pod::GenetElement;
use crate::{
    AnyView, GenetCtx, Key, NamedKey, PointerClick, arrangement::placed_with, el, on_click, on_key,
};

/// The erased view type grid cells (and the grid itself) use.
pub type GridView<State, Action> = Box<dyn AnyView<State, Action, GenetCtx, GenetElement>>;

/// Build a data grid. `cell(row, col)` supplies each materialized cell's view;
/// `scroll` is the caller's scroll offset into the body (device px);
/// `on_header_click` receives the clicked column index (sort state lives with
/// the caller).
pub fn data_grid<State, Action>(
    spec: &GridSpec,
    total_rows: usize,
    viewport_height: f32,
    scroll: f32,
    cell: impl Fn(usize, usize) -> GridView<State, Action>,
    on_header_click: impl Fn(&mut State, usize) + Clone + 'static,
    row_class: impl Fn(usize) -> Option<String>,
) -> GridView<State, Action>
where
    State: 'static,
    Action: 'static,
{
    let width = spec.total_width();

    // Sticky header: one relative strip that never scrolls.
    let header_cells: Vec<GridView<State, Action>> = spec
        .columns
        .iter()
        .enumerate()
        .map(|(c, col)| {
            let click_handler = on_header_click.clone();
            let key_handler = on_header_click.clone();
            let header_cell = placed_with(
                Placement::new(spec.col_x(c), 0.0),
                format!("width: {}px; height: {}px;", col.width, spec.header_height),
                col.title.clone(),
            )
            .attr("class", "grid-header-cell")
            .attr("role", "columnheader")
            .attr("aria-colindex", (c + 1).to_string())
            .attr("tabindex", "0");
            Box::new(on_key(
                on_click(header_cell, move |s: &mut State, _: PointerClick| {
                    click_handler(s, c)
                }),
                move |s: &mut State, event| {
                    if matches!(event.key, Key::Named(NamedKey::Enter | NamedKey::Space)) {
                        key_handler(s, c);
                        event.prevent_default();
                    }
                },
            )) as GridView<State, Action>
        })
        .collect();
    let header = el::<_, State, Action>("div", header_cells)
        .attr("class", "grid-header")
        .attr("role", "row")
        .attr("aria-rowindex", "1")
        .attr(
            "style",
            format!(
                "position: relative; width: {width}px; height: {}px;",
                spec.header_height
            ),
        );

    // Virtualized body: only the window's rows exist; each is placed at its
    // viewport-relative y (row y minus scroll), cells placed by column.
    let window = spec.window(total_rows, viewport_height, scroll);
    let rows: Vec<GridView<State, Action>> = window
        .range()
        .map(|r| {
            let cells: Vec<GridView<State, Action>> = spec
                .columns
                .iter()
                .enumerate()
                .map(|(c, col)| {
                    Box::new(
                        placed_with(
                            Placement::new(spec.col_x(c), 0.0),
                            format!("width: {}px; height: {}px;", col.width, spec.row_height),
                            cell(r, c),
                        )
                        .attr("class", "grid-cell")
                        .attr("role", "gridcell")
                        .attr("aria-colindex", (c + 1).to_string()),
                    ) as GridView<State, Action>
                })
                .collect();
            let zebra = if r % 2 == 0 {
                "grid-row grid-row-even"
            } else {
                "grid-row grid-row-odd"
            };
            // Per-row class hook: selection / active / status styling the caller
            // owns (every real table needs it), appended to the zebra base.
            let class = match row_class(r) {
                Some(extra) => format!("{zebra} {extra}"),
                None => zebra.to_string(),
            };
            Box::new(
                placed_with(
                    Placement::new(0.0, r as f32 * spec.row_height - scroll),
                    format!("width: {width}px; height: {}px;", spec.row_height),
                    cells,
                )
                .attr("class", class)
                .attr("role", "row")
                .attr("aria-rowindex", (r + 2).to_string())
                .attr("data-row", r.to_string()),
            ) as GridView<State, Action>
        })
        .collect();
    let body = el::<_, State, Action>("div", rows).attr("class", "grid-body").attr(
        "style",
        format!(
            "position: relative; overflow-x: hidden; overflow-y: hidden; width: {width}px; height: {viewport_height}px;"
        ),
    );

    Box::new(
        el::<_, State, Action>("div", (header, body))
            .attr("class", "grid")
            .attr("role", "grid")
            .attr("aria-rowcount", (total_rows + 1).to_string())
            .attr("aria-colcount", spec.columns.len().to_string())
            .attr("style", format!("display: block; width: {width}px;")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags::custom_leaf;
    use crate::{DomHandle, GenetAppRunner};
    use genet_scripted_dom::{NodeId, ScriptedDom};
    use layout_dom_api::{LayoutDom, LocalName, Namespace};
    use sprigging::GridColumn;
    use std::cell::RefCell;
    use std::rc::Rc;

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

    struct GridState {
        scroll: f32,
        descending: bool,
        rows: usize,
    }

    fn view(state: &GridState) -> GridView<GridState, ()> {
        let rows = state.rows;
        let descending = state.descending;
        data_grid(
            &spec(),
            rows,
            300.0,
            state.scroll,
            move |r, c| {
                // Sort order is caller state: ascending shows r, descending
                // shows the mirrored index.
                let i = if descending { rows - 1 - r } else { r };
                if c == 2 {
                    // Sparkline-in-cell shape: a custom leaf as a cell view.
                    Box::new(custom_leaf::<GridState, ()>(1000 + i as u64, 96, 16))
                } else {
                    Box::new(el::<_, GridState, ()>("span", format!("r{i}c{c}")))
                }
            },
            |s: &mut GridState, _col| s.descending = !s.descending,
            // Mark row 0 selected, to exercise the per-row class hook.
            |r| (r == 0).then(|| "grid-row-selected".to_string()),
        )
    }

    fn class_of(dom: &ScriptedDom, node: NodeId) -> String {
        dom.attribute(node, &Namespace::from(""), &LocalName::from("class"))
            .unwrap_or_default()
            .to_string()
    }

    fn find_all(dom: &ScriptedDom, node: NodeId, class: &str, out: &mut Vec<NodeId>) {
        if class_of(dom, node).split_whitespace().any(|c| c == class) {
            out.push(node);
        }
        for child in dom.dom_children(node) {
            find_all(dom, child, class, out);
        }
    }

    fn texts_under(dom: &ScriptedDom, node: NodeId, out: &mut String) {
        if let Some(t) = dom.text(node) {
            out.push_str(t);
        }
        for child in dom.dom_children(node) {
            texts_under(dom, child, out);
        }
    }

    /// The composed done conditions: a 10k-row grid materializes only the
    /// window (sticky header always present), scrolling shifts rows without
    /// growing the DOM, and a custom leaf rides as a cell view.
    #[test]
    fn grid_virtualizes_rows_under_a_sticky_header() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            view,
            GridState {
                scroll: 0.0,
                descending: false,
                rows: 10_000,
            },
        );
        let root = runner.root();
        {
            let d = dom.borrow();
            let mut headers = Vec::new();
            find_all(&d, root, "grid-header-cell", &mut headers);
            assert_eq!(headers.len(), 3, "one header cell per column");
            let mut rows = Vec::new();
            find_all(&d, root, "grid-row", &mut rows);
            assert!(
                rows.len() < 25,
                "10k rows materialize a window, got {}",
                rows.len()
            );
            // The per-row class hook rides on the row's class: row 0 is the
            // window's first row at scroll 0 and the view marks it selected.
            let mut selected = Vec::new();
            find_all(&d, root, "grid-row-selected", &mut selected);
            assert_eq!(selected.len(), 1, "row_class marks exactly row 0 selected");
            assert!(
                class_of(&d, selected[0]).contains("grid-row"),
                "hook appends to the zebra base"
            );
            let mut leaves = Vec::new();
            find_all(&d, root, "grid-cell", &mut leaves);
            assert_eq!(
                leaves.len(),
                rows.len() * 3,
                "three cells per materialized row"
            );
        }

        runner.update(|s| s.scroll = 4800.0);
        let d = dom.borrow();
        let mut rows = Vec::new();
        find_all(&d, root, "grid-row", &mut rows);
        assert!(rows.len() < 25, "window stays a sliver after scroll");
        let first_row_class = d
            .attribute(rows[0], &Namespace::from(""), &LocalName::from("data-row"))
            .unwrap_or_default();
        assert_eq!(
            first_row_class, "198",
            "window starts at scroll/24 minus overscan"
        );
        // The sparkline cell rides along: a custom leaf inside the row.
        let mut text = String::new();
        texts_under(&d, rows[0], &mut text);
        assert!(
            text.contains("r198c0"),
            "cell content reflects the row: {text}"
        );
    }

    /// Header click routes through the grid's on_click wiring into caller
    /// state: the sort flips and the same materialized window shows the
    /// mirrored order.
    #[test]
    fn header_click_flips_the_sort() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            view,
            GridState {
                scroll: 0.0,
                descending: false,
                rows: 100,
            },
        );
        let root = runner.root();
        let (header, first_row) = {
            let d = dom.borrow();
            let mut headers = Vec::new();
            find_all(&d, root, "grid-header-cell", &mut headers);
            let mut rows = Vec::new();
            find_all(&d, root, "grid-row", &mut rows);
            (headers[0], rows[0])
        };
        {
            let d = dom.borrow();
            let mut text = String::new();
            texts_under(&d, first_row, &mut text);
            assert!(text.contains("r0c0"), "ascending starts at row 0: {text}");
        }

        runner.dispatch_click(header, PointerClick::at((1.0, 1.0)));

        let d = dom.borrow();
        let mut rows = Vec::new();
        find_all(&d, root, "grid-row", &mut rows);
        let mut text = String::new();
        texts_under(&d, rows[0], &mut text);
        assert!(
            text.contains("r99c0"),
            "descending shows the mirrored index in the same window: {text}",
        );
    }
}
