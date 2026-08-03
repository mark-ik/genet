//! K4d6a: the table's own fragment subtree.
//!
//! Every table-internal box gets a fragment with its own logical rectangle
//! and structural parent, emitted from the accepted K4c inline result, K4d3
//! row sizing, and K4d5 alignment. Nothing is reconstructed later from
//! painted cells: a column's rectangle exists because a column track exists,
//! not because something inferred it from the cells that happen to sit in it.
//!
//! Fragments are emitted, not committed. K4d6b hands the tree to Livery's
//! `FragmentTree`; until then no draft can reach painted output, exactly as
//! K4d1's draft discipline requires.

use crate::{BoxId, LogicalRect};

use super::{
    TableAlignment, TableBlockBorderMetrics, TableBlockDeferral, TableBlockSizingInput,
    TableCellLayoutOutput, TableRowLayoutError, TableRowSizing, TableTrackGroupKind,
};

/// The role a table-internal fragment plays in the table model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableFragmentRole {
    Grid,
    RowGroup(TableTrackGroupKind),
    Row,
    ColumnGroup,
    Column,
    Cell,
}

/// One emitted table-internal fragment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableFragment {
    /// The CSS box this fragment belongs to. `None` for a track created
    /// implicitly by placement, which has no box and therefore no identity to
    /// borrow from a neighbour.
    pub box_id: Option<BoxId>,
    pub role: TableFragmentRole,
    /// The fragment's border-box rectangle in the table's logical axes,
    /// relative to the grid's block-start inline-start corner.
    pub rect: LogicalRect,
    /// This fragment's own rectangle unioned with everything it contains.
    pub overflow: LogicalRect,
    /// Structural parent within the same vector. Parents always precede
    /// their children, so a consumer can commit in order.
    pub parent: Option<usize>,
}

/// The table's complete fragment subtree, in tree order with the grid first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableFragments {
    fragments: Vec<TableFragment>,
    /// Index of each K4b cell's fragment, in K4b cell order.
    cell_fragments: Vec<usize>,
}

impl TableFragments {
    pub fn fragments(&self) -> &[TableFragment] {
        &self.fragments
    }

    pub fn grid(&self) -> Option<&TableFragment> {
        self.fragments.first()
    }

    /// The fragment emitted for one K4b cell, by cell index.
    pub fn cell(&self, index: usize) -> Option<&TableFragment> {
        self.cell_fragments
            .get(index)
            .and_then(|at| self.fragments.get(*at))
    }

    pub fn len(&self) -> usize {
        self.fragments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }

    /// Every fragment with the given role, in tree order.
    pub fn with_role(&self, role: TableFragmentRole) -> impl Iterator<Item = &TableFragment> {
        self.fragments
            .iter()
            .filter(move |fragment| fragment.role == role)
    }
}

fn union(one: LogicalRect, other: LogicalRect) -> LogicalRect {
    let inline_start = one.inline_start.min(other.inline_start);
    let block_start = one.block_start.min(other.block_start);
    let inline_end =
        (one.inline_start + one.inline_size).max(other.inline_start + other.inline_size);
    let block_end = (one.block_start + one.block_size).max(other.block_start + other.block_size);
    LogicalRect {
        inline_start,
        block_start,
        inline_size: inline_end - inline_start,
        block_size: block_end - block_start,
    }
}

/// Emit the table's fragment subtree.
///
/// `cell_outputs` supply each cell's own overflow, which unions upward into
/// the cell, its row, that row's group, and the grid.
pub fn emit_table_fragments(
    input: &TableBlockSizingInput<'_>,
    sizing: &TableRowSizing,
    alignment: &TableAlignment,
    cell_outputs: &[(BoxId, TableCellLayoutOutput)],
    inline_spacing: f32,
) -> Result<TableFragments, TableRowLayoutError> {
    let grid = input.grid;
    if sizing.row_sizes.len() != grid.rows.len() {
        return Err(TableRowLayoutError::RowInputCountMismatch {
            expected: grid.rows.len(),
            actual: sizing.row_sizes.len(),
        });
    }
    if alignment.cells.len() != grid.cells.len() || cell_outputs.len() != grid.cells.len() {
        return Err(TableRowLayoutError::CellInputCountMismatch {
            expected: grid.cells.len(),
            actual: alignment.cells.len().min(cell_outputs.len()),
        });
    }
    if matches!(
        input.border_metrics,
        TableBlockBorderMetrics::CollapsedPendingK4g
    ) {
        return Err(TableRowLayoutError::Deferral(
            TableBlockDeferral::CollapsedBlockBorderMetricsPendingK4g,
        ));
    }

    let grid_rect = LogicalRect {
        inline_start: 0.0,
        block_start: 0.0,
        inline_size: input.inline.used_grid_inline_size,
        block_size: sizing.used_table_block_size,
    };
    let mut fragments = vec![TableFragment {
        box_id: Some(grid.grid),
        role: TableFragmentRole::Grid,
        rect: grid_rect,
        overflow: grid_rect,
        parent: None,
    }];

    // Row groups, then their rows. A row outside every group hangs directly
    // from the grid, which is what an anonymous row-group-free table needs.
    let mut row_parents = vec![0usize; grid.rows.len()];
    for group in &grid.row_groups {
        let end = (group.start + group.span).min(grid.rows.len());
        if group.start >= end {
            continue;
        }
        let block_start = sizing.row_offsets[group.start];
        let last = end - 1;
        let block_size = sizing.row_offsets[last] + sizing.row_sizes[last] - block_start;
        let rect = LogicalRect {
            inline_start: 0.0,
            block_start,
            inline_size: grid_rect.inline_size,
            block_size,
        };
        fragments.push(TableFragment {
            box_id: Some(group.source),
            role: TableFragmentRole::RowGroup(group.kind),
            // A group's rectangle is the exact union of its track range,
            // never a box reconstructed from the cells inside it.
            rect,
            overflow: rect,
            parent: Some(0),
        });
        row_parents[group.start..end].fill(fragments.len() - 1);
    }

    let mut row_fragments = Vec::with_capacity(grid.rows.len());
    for (index, track) in grid.rows.iter().enumerate() {
        let rect = LogicalRect {
            inline_start: 0.0,
            block_start: sizing.row_offsets[index],
            inline_size: grid_rect.inline_size,
            block_size: sizing.row_sizes[index],
        };
        fragments.push(TableFragment {
            box_id: track.source,
            role: TableFragmentRole::Row,
            rect,
            overflow: rect,
            parent: Some(row_parents[index]),
        });
        row_fragments.push(fragments.len() - 1);
    }

    // Column groups and columns. Their rectangles come from the track model,
    // so a column exists even where no cell occupies it.
    let mut inline_offsets = Vec::with_capacity(grid.columns.len());
    let mut cursor = inline_spacing;
    for size in &input.inline.column_sizes {
        inline_offsets.push(cursor);
        cursor += size + inline_spacing;
    }
    let mut column_parents = vec![0usize; grid.columns.len()];
    for group in &grid.column_groups {
        let end = (group.start + group.span).min(grid.columns.len());
        if group.start >= end {
            continue;
        }
        let inline_start = inline_offsets[group.start];
        let last = end - 1;
        let inline_size = inline_offsets[last] + input.inline.column_sizes[last] - inline_start;
        let rect = LogicalRect {
            inline_start,
            block_start: 0.0,
            inline_size,
            block_size: grid_rect.block_size,
        };
        fragments.push(TableFragment {
            box_id: Some(group.source),
            role: TableFragmentRole::ColumnGroup,
            rect,
            overflow: rect,
            parent: Some(0),
        });
        column_parents[group.start..end].fill(fragments.len() - 1);
    }
    for (index, track) in grid.columns.iter().enumerate() {
        let rect = LogicalRect {
            inline_start: inline_offsets[index],
            block_start: 0.0,
            inline_size: input.inline.column_sizes[index],
            block_size: grid_rect.block_size,
        };
        fragments.push(TableFragment {
            box_id: track.source,
            role: TableFragmentRole::Column,
            rect,
            overflow: rect,
            parent: Some(column_parents[index]),
        });
    }

    // Cells, under the row they originate in. A spanning cell gets exactly
    // one fragment covering its whole range; nothing is split here, and K6
    // owns the fragmented case.
    let mut cell_fragments = Vec::with_capacity(grid.cells.len());
    for (index, cell) in grid.cells.iter().enumerate() {
        let placement = &alignment.cells[index];
        let overflow = union(placement.rect, {
            let cell_overflow = cell_outputs[index].1.overflow;
            LogicalRect {
                inline_start: placement.rect.inline_start + cell_overflow.inline_start,
                block_start: placement.rect.block_start + cell_overflow.block_start,
                inline_size: cell_overflow.inline_size,
                block_size: cell_overflow.block_size,
            }
        });
        fragments.push(TableFragment {
            box_id: Some(cell.source),
            role: TableFragmentRole::Cell,
            rect: placement.rect,
            overflow,
            parent: Some(row_fragments[cell.row]),
        });
        cell_fragments.push(fragments.len() - 1);
    }

    // Union every fragment's overflow into its structural ancestors. Children
    // always follow their parents, so one reverse sweep suffices.
    for index in (1..fragments.len()).rev() {
        let (overflow, parent) = (fragments[index].overflow, fragments[index].parent);
        if let Some(parent) = parent {
            fragments[parent].overflow = union(fragments[parent].overflow, overflow);
        }
    }
    Ok(TableFragments {
        fragments,
        cell_fragments,
    })
}

#[cfg(test)]
mod tests {
    use super::super::FragmentDraftTree;
    use super::*;
    use crate::{
        Baselines, BoxGeneration, BoxOrigin, BoxTreeInput, CssBoxTree, DisplayInside,
        DisplayOutside, DisplayRole, FlowAxes, InternalTableRole, IntrinsicSizes,
        PositioningScheme, TableBlockConstraint, TableCellBlockStyle, TableGrid, TableGridInputs,
        TableInlineSizingInput, TableInlineSizingResult, TableSeparatedBlockMetrics,
        TableTrackVisibility, align_table_cells, generate_box_tree, measure_single_span_rows,
        size_table_rows,
    };

    fn table_role(role: InternalTableRole) -> DisplayRole {
        DisplayRole {
            generation: BoxGeneration::Normal,
            outside: None,
            inside: None,
            list_item: false,
            internal_table: Some(role),
        }
    }

    fn leaf(id: u8, role: InternalTableRole) -> BoxTreeInput<u8> {
        BoxTreeInput::new(
            BoxOrigin::Element(id),
            table_role(role),
            FlowAxes::HORIZONTAL_LTR,
            PositioningScheme::Static,
            false,
            vec![],
        )
    }

    /// A two-row table in one row group, with a colgroup over both columns.
    fn grid() -> TableGrid {
        let row = |id: u8, cells: Vec<BoxTreeInput<u8>>| {
            BoxTreeInput::new(
                BoxOrigin::Element(id),
                table_role(InternalTableRole::Row),
                FlowAxes::HORIZONTAL_LTR,
                PositioningScheme::Static,
                false,
                cells,
            )
        };
        let tree: CssBoxTree<u8> = generate_box_tree([BoxTreeInput::new(
            BoxOrigin::Element(1),
            DisplayRole {
                generation: BoxGeneration::Normal,
                outside: Some(DisplayOutside::Block),
                inside: Some(DisplayInside::Table),
                list_item: false,
                internal_table: None,
            },
            FlowAxes::HORIZONTAL_LTR,
            PositioningScheme::Static,
            false,
            vec![
                BoxTreeInput::new(
                    BoxOrigin::Element(80),
                    table_role(InternalTableRole::ColumnGroup),
                    FlowAxes::HORIZONTAL_LTR,
                    PositioningScheme::Static,
                    false,
                    vec![
                        leaf(81, InternalTableRole::Column),
                        leaf(82, InternalTableRole::Column),
                    ],
                ),
                BoxTreeInput::new(
                    BoxOrigin::Element(90),
                    table_role(InternalTableRole::RowGroup),
                    FlowAxes::HORIZONTAL_LTR,
                    PositioningScheme::Static,
                    false,
                    vec![
                        row(
                            100,
                            vec![
                                leaf(3, InternalTableRole::Cell),
                                leaf(4, InternalTableRole::Cell),
                            ],
                        ),
                        row(101, vec![leaf(5, InternalTableRole::Cell)]),
                    ],
                ),
            ],
        )]);
        TableGrid::from_box_tree(
            &tree,
            tree.principal_box(1).expect("table grid"),
            &TableGridInputs::default(),
        )
    }

    fn output(content: f32, overflow: LogicalRect) -> TableCellLayoutOutput {
        TableCellLayoutOutput {
            content_block_size: content,
            border_box_min_block_size: 0.0,
            baselines: Baselines::synthesized_from_block_end(content),
            overflow,
            fragments: FragmentDraftTree::default(),
        }
    }

    fn emit(grid: &TableGrid, overflows: Vec<LogicalRect>) -> (TableFragments, TableRowSizing) {
        let inline = {
            let sizing = TableInlineSizingInput {
                grid,
                available_inline_size: Some(100.0),
                table_constraints: super::super::TableInlineConstraints::default(),
                border_metrics: super::super::TableInlineBorderMetrics::Separated(
                    super::super::TableSeparatedBorderMetrics::default(),
                ),
                caption_min: super::super::CaptionMinContribution::NoCaption,
                track_visibility: TableTrackVisibility::all_visible(grid),
            };
            TableInlineSizingResult::new(
                &sizing,
                IntrinsicSizes::new(100.0, 100.0).expect("intrinsic pair"),
                100.0,
                100.0,
                vec![60.0, 40.0],
            )
            .expect("inline result")
        };
        let input = TableBlockSizingInput {
            grid,
            inline: &inline,
            table_constraint: TableBlockConstraint::Auto,
            table_box_sizing: crate::TableBoxSizing::BorderBox,
            border_metrics: TableBlockBorderMetrics::Separated(
                TableSeparatedBlockMetrics::default(),
            ),
            available_block_size: None,
            track_visibility: TableTrackVisibility::all_visible(grid),
        };
        let styles = vec![TableCellBlockStyle::default(); grid.cells.len()];
        let outputs = grid
            .cells
            .iter()
            .zip(&overflows)
            .enumerate()
            .map(|(index, (cell, overflow))| {
                (cell.source, output(20.0 + index as f32 * 10.0, *overflow))
            })
            .collect::<Vec<_>>();
        let row_constraints = vec![TableBlockConstraint::Auto; grid.rows.len()];
        let measures = measure_single_span_rows(&input, &styles, &outputs, &row_constraints)
            .expect("measures");
        let sizing = size_table_rows(&input, &measures, &styles, &outputs).expect("sizing");
        let alignment =
            align_table_cells(&input, &sizing, &styles, &outputs, 0.0).expect("alignment");
        let fragments =
            emit_table_fragments(&input, &sizing, &alignment, &outputs, 0.0).expect("fragments");
        (fragments, sizing)
    }

    #[test]
    fn every_table_box_gets_a_fragment_with_its_structural_parent() {
        let grid = grid();
        let (fragments, sizing) = emit(&grid, vec![LogicalRect::default(); grid.cells.len()]);

        // Grid first, parents always before their children.
        assert_eq!(
            fragments.grid().expect("grid").role,
            TableFragmentRole::Grid
        );
        for (index, fragment) in fragments.fragments().iter().enumerate() {
            if let Some(parent) = fragment.parent {
                assert!(parent < index, "a parent must precede its child");
            }
        }

        // One fragment per table box: grid, row group, two rows, column
        // group, two columns, three cells.
        assert_eq!(fragments.len(), 1 + 1 + 2 + 1 + 2 + 3);
        assert_eq!(fragments.with_role(TableFragmentRole::Row).count(), 2);
        assert_eq!(fragments.with_role(TableFragmentRole::Column).count(), 2);
        assert_eq!(fragments.with_role(TableFragmentRole::Cell).count(), 3);
        assert_eq!(
            fragments
                .with_role(TableFragmentRole::RowGroup(TableTrackGroupKind::Body))
                .count(),
            1
        );

        // A row group's rectangle is the exact union of its track range.
        let group = fragments
            .with_role(TableFragmentRole::RowGroup(TableTrackGroupKind::Body))
            .next()
            .expect("row group");
        let expected = sizing.row_offsets[1] + sizing.row_sizes[1] - sizing.row_offsets[0];
        assert!((group.rect.block_size - expected).abs() < 0.05, "{group:?}");

        // A column group spans both columns exactly.
        let column_group = fragments
            .with_role(TableFragmentRole::ColumnGroup)
            .next()
            .expect("column group");
        assert!((column_group.rect.inline_size - 100.0).abs() < 0.05);
        // Columns come from the track model, not from the cells in them.
        let columns = fragments
            .with_role(TableFragmentRole::Column)
            .map(|fragment| fragment.rect.inline_size)
            .collect::<Vec<_>>();
        assert_eq!(columns, vec![60.0, 40.0]);
    }

    /// Row 1 holds a single cell, so column 1 has no cell in it at all. Its
    /// fragment still exists with the track's own rectangle.
    #[test]
    fn a_column_without_cells_still_has_its_own_rectangle() {
        let grid = grid();
        let (fragments, sizing) = emit(&grid, vec![LogicalRect::default(); grid.cells.len()]);
        let columns = fragments
            .with_role(TableFragmentRole::Column)
            .collect::<Vec<_>>();
        assert_eq!(columns.len(), 2);
        // Both columns run the full grid block size, whatever occupies them.
        for column in columns {
            assert!((column.rect.block_size - sizing.used_table_block_size).abs() < 0.05);
        }
    }

    /// A cell's overflow unions into its row, that row's group, and the grid,
    /// without any of them losing their own rectangle.
    #[test]
    fn overflow_unions_upward_through_the_structural_parents() {
        let grid = grid();
        let mut overflows = vec![LogicalRect::default(); grid.cells.len()];
        // The first cell overflows well past the grid's block end.
        overflows[0] = LogicalRect {
            inline_start: 0.0,
            block_start: 0.0,
            inline_size: 10.0,
            block_size: 500.0,
        };
        let (fragments, sizing) = emit(&grid, overflows);

        let grid_fragment = fragments.grid().expect("grid");
        assert!(
            grid_fragment.overflow.block_size >= 500.0,
            "{grid_fragment:?}"
        );
        // The grid's own rectangle is untouched by the overflow union.
        assert!((grid_fragment.rect.block_size - sizing.used_table_block_size).abs() < 0.05);

        let row = fragments
            .with_role(TableFragmentRole::Row)
            .next()
            .expect("first row");
        assert!(row.overflow.block_size >= 500.0, "{row:?}");
        assert!((row.rect.block_size - sizing.row_sizes[0]).abs() < 0.05);
    }

    /// A spanning cell gets exactly one fragment covering its whole range.
    #[test]
    fn a_spanning_cell_gets_one_unfragmented_rectangle() {
        let row = |id: u8, cells: Vec<BoxTreeInput<u8>>| {
            BoxTreeInput::new(
                BoxOrigin::Element(id),
                table_role(InternalTableRole::Row),
                FlowAxes::HORIZONTAL_LTR,
                PositioningScheme::Static,
                false,
                cells,
            )
        };
        let tree: CssBoxTree<u8> = generate_box_tree([BoxTreeInput::new(
            BoxOrigin::Element(1),
            DisplayRole {
                generation: BoxGeneration::Normal,
                outside: Some(DisplayOutside::Block),
                inside: Some(DisplayInside::Table),
                list_item: false,
                internal_table: None,
            },
            FlowAxes::HORIZONTAL_LTR,
            PositioningScheme::Static,
            false,
            vec![BoxTreeInput::new(
                BoxOrigin::Element(90),
                table_role(InternalTableRole::RowGroup),
                FlowAxes::HORIZONTAL_LTR,
                PositioningScheme::Static,
                false,
                vec![
                    row(
                        100,
                        vec![
                            leaf(3, InternalTableRole::Cell),
                            leaf(200, InternalTableRole::Cell),
                        ],
                    ),
                    row(101, vec![leaf(4, InternalTableRole::Cell)]),
                ],
            )],
        )]);
        let mut inputs = TableGridInputs::default();
        inputs.set_cell(
            tree.principal_box(200).expect("spanner"),
            super::super::TableCellInput {
                row_span: super::super::TableRowSpan::Count(2),
                ..super::super::TableCellInput::default()
            },
        );
        let grid =
            TableGrid::from_box_tree(&tree, tree.principal_box(1).expect("table grid"), &inputs);
        let spanner = grid
            .cells
            .iter()
            .position(|cell| cell.row_span == 2)
            .expect("spanning cell");
        let (fragments, sizing) = emit(&grid, vec![LogicalRect::default(); grid.cells.len()]);

        assert_eq!(fragments.with_role(TableFragmentRole::Cell).count(), 3);
        let fragment = fragments.cell(spanner).expect("spanner fragment");
        let expected = sizing.row_sizes[0] + sizing.row_sizes[1];
        assert!(
            (fragment.rect.block_size - expected).abs() < 0.05,
            "one fragment must cover the whole span: {fragment:?}"
        );
    }

    #[test]
    fn collapsed_block_metrics_defer_before_any_fragment_is_emitted() {
        let grid = grid();
        let inline = {
            let sizing = TableInlineSizingInput {
                grid: &grid,
                available_inline_size: Some(100.0),
                table_constraints: super::super::TableInlineConstraints::default(),
                border_metrics: super::super::TableInlineBorderMetrics::Separated(
                    super::super::TableSeparatedBorderMetrics::default(),
                ),
                caption_min: super::super::CaptionMinContribution::NoCaption,
                track_visibility: TableTrackVisibility::all_visible(&grid),
            };
            TableInlineSizingResult::new(
                &sizing,
                IntrinsicSizes::new(100.0, 100.0).expect("intrinsic pair"),
                100.0,
                100.0,
                vec![60.0, 40.0],
            )
            .expect("inline result")
        };
        let input = TableBlockSizingInput {
            grid: &grid,
            inline: &inline,
            table_constraint: TableBlockConstraint::Auto,
            table_box_sizing: crate::TableBoxSizing::BorderBox,
            border_metrics: TableBlockBorderMetrics::CollapsedPendingK4g,
            available_block_size: None,
            track_visibility: TableTrackVisibility::all_visible(&grid),
        };
        let sizing = TableRowSizing {
            used_table_block_size: 0.0,
            row_offsets: vec![0.0; grid.rows.len()],
            row_sizes: vec![0.0; grid.rows.len()],
        };
        // Well-formed inputs, so the collapsed-border deferral is what the
        // emitter actually reaches rather than a length check.
        let alignment = TableAlignment {
            cells: grid
                .cells
                .iter()
                .map(|cell| super::super::TableCellPlacement {
                    box_id: cell.source,
                    row_start: cell.row,
                    row_span: cell.row_span,
                    column_start: cell.column,
                    column_span: cell.column_span,
                    rect: LogicalRect::default(),
                    content_block_offset: 0.0,
                })
                .collect(),
            rows: vec![
                super::super::TableRowBaseline {
                    baseline: 0.0,
                    from_aligned_cell: false,
                };
                grid.rows.len()
            ],
            baselines: Baselines::synthesized_from_block_end(0.0),
        };
        let outputs = grid
            .cells
            .iter()
            .map(|cell| (cell.source, output(0.0, LogicalRect::default())))
            .collect::<Vec<_>>();
        assert_eq!(
            emit_table_fragments(&input, &sizing, &alignment, &outputs, 0.0),
            Err(TableRowLayoutError::Deferral(
                TableBlockDeferral::CollapsedBlockBorderMetricsPendingK4g
            ))
        );
    }
}
