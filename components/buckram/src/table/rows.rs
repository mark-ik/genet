//! K4d row-layout contracts over K4b's grid and K4c's column sizes.
//!
//! This module owns the boundary through which table block-axis layout will
//! be computed. K4d1 defines the contracts and the cell-formatting dispatch;
//! the row arithmetic itself lands gate by gate (K4d2 single-span minima,
//! K4d3 rowspans and used height, K4d4 percentage relayout, K4d5 alignment
//! and baselines, K4d6 live dispatch and bridge deletion).
//!
//! No Taffy type may enter this module. A cell's contents are formatted
//! through [`TableCellFormatter`], which the adapter implements over whatever
//! formatting context the cell contains; the table algorithm sees only the
//! typed output: content block size, border-box minimum, baselines, overflow,
//! and fragment drafts.

use crate::{Baselines, BoxId, LogicalRect};

use super::{
    AffineLengthPercentage, TableBoxSizing, TableGrid, TableInlineSizingError,
    TableInlineSizingResult, TableTrackVisibility,
};

/// A block-axis CSS size constraint before row layout has a basis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TableBlockConstraint {
    Auto,
    Value(AffineLengthPercentage),
    /// A computed expression which cannot yet reduce to an affine
    /// length-percentage without losing CSS semantics.
    Unreduced,
}

/// Block-axis geometry that does not belong to a distributable row size.
///
/// Unlike the inline offsets, block padding is pre-resolved: CSS resolves a
/// padding percentage against the *inline* size of the containing block, and
/// K4c's accepted inline result makes that basis real before row layout runs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TableSeparatedBlockMetrics {
    pub table_offset_start: f32,
    pub table_offset_end: f32,
    pub block_spacing: f32,
}

impl TableSeparatedBlockMetrics {
    /// The two table edges plus one spacing interval before, after, and
    /// between every K4b row.
    pub fn undistributable_block_size(self, row_count: usize) -> Option<f32> {
        let values = [
            self.table_offset_start,
            self.table_offset_end,
            self.block_spacing,
        ];
        if !values
            .iter()
            .all(|value| value.is_finite() && *value >= 0.0)
        {
            return None;
        }
        let gaps = row_count.checked_add(1)? as f32;
        let total = self.table_offset_start + self.table_offset_end + self.block_spacing * gaps;
        total.is_finite().then_some(total)
    }
}

/// Border-model geometry for the block axis. Declared borders are not an
/// acceptable stand-in for collapsed-border winners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TableBlockBorderMetrics {
    Separated(TableSeparatedBlockMetrics),
    CollapsedPendingK4g,
}

/// Named block-axis distinctions deferred to later gates or explicit interop
/// records. An undefined percentage never silently becomes zero or `auto`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableBlockDeferral {
    PercentageBlockBasisIndefinite,
    PercentageBlockCycle,
    FragmentationDependentRowspan,
    CollapsedBlockBorderMetricsPendingK4g,
}

/// Errors and deferrals from the row-layout boundary.
#[derive(Clone, Debug, PartialEq)]
pub enum TableRowLayoutError {
    Deferral(TableBlockDeferral),
    /// A cell references columns outside K4c's assigned vector.
    ColumnSpanOutOfBounds {
        box_id: BoxId,
        column_start: usize,
        column_span: usize,
        columns: usize,
    },
    /// A formatter output violated its contract (non-finite size, negative
    /// minimum, invalid baselines).
    InvalidCellOutput {
        box_id: BoxId,
    },
    /// The inline result and grid disagree about column count; the input was
    /// assembled from mismatched gates.
    InlineResultMismatch {
        expected: usize,
        actual: usize,
    },
    /// Per-cell block inputs do not match K4b's cell vector.
    CellInputCountMismatch {
        expected: usize,
        actual: usize,
    },
    /// A per-cell input is not aligned with K4b's cell order.
    CellSourceMismatch {
        index: usize,
        expected: BoxId,
        actual: BoxId,
    },
    /// Per-row block inputs do not match K4b's row vector.
    RowInputCountMismatch {
        expected: usize,
        actual: usize,
    },
    Inline(TableInlineSizingError),
}

/// Which pass a cell is being formatted for. First-pass measurement precedes
/// row sizing; the percentage pass reruns cells whose contents depend on the
/// resolved row block size, replacing their drafts rather than duplicating
/// them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TableCellLayoutPass {
    Measure,
    ResolvePercentages { cell_block_size: f32 },
}

/// One cell-format request. The inline size is exact: K4c's spanned columns
/// plus the spacing the span crosses, minus the cell's resolved inline
/// offsets. The formatter must not re-derive it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableCellLayoutInput {
    pub box_id: BoxId,
    pub content_inline_size: f32,
    pub available_block_size: Option<f32>,
    pub percentage_basis: Option<f32>,
    pub pass: TableCellLayoutPass,
}

/// One cell-format result. A formatting context returns its own baselines and
/// overflow directly; the table algorithm never rediscovers them by walking a
/// backend tree.
#[derive(Clone, Debug, PartialEq)]
pub struct TableCellLayoutOutput {
    pub content_block_size: f32,
    pub border_box_min_block_size: f32,
    pub baselines: Baselines,
    pub overflow: LogicalRect,
    pub fragments: FragmentDraftTree,
}

impl TableCellLayoutOutput {
    fn is_valid(&self) -> bool {
        self.content_block_size.is_finite()
            && self.content_block_size >= 0.0
            && self.border_box_min_block_size.is_finite()
            && self.border_box_min_block_size >= 0.0
    }
}

/// Formats one cell's contents at an exact inline size. The adapter
/// implements this over leaf, block, inline, flex, or grid formatting
/// contexts; the table algorithm dispatches through it and never sees the
/// backend.
pub trait TableCellFormatter {
    fn format_cell(
        &mut self,
        input: TableCellLayoutInput,
    ) -> Result<TableCellLayoutOutput, TableRowLayoutError>;
}

/// Resolved block-axis padding and border for one cell.
///
/// Unlike the inline offsets these are plain lengths. CSS resolves a padding
/// percentage against the containing block's *inline* size, which K4c's
/// accepted result already made definite, so nothing is carried unresolved
/// here.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CellBlockOffsets {
    pub padding_start: f32,
    pub padding_end: f32,
    pub border_start: f32,
    pub border_end: f32,
}

impl CellBlockOffsets {
    pub const ZERO: Self = Self {
        padding_start: 0.0,
        padding_end: 0.0,
        border_start: 0.0,
        border_end: 0.0,
    };

    pub fn total(self) -> Option<f32> {
        let values = [
            self.padding_start,
            self.padding_end,
            self.border_start,
            self.border_end,
        ];
        if !values
            .iter()
            .all(|value| value.is_finite() && *value >= 0.0)
        {
            return None;
        }
        let total = values.iter().sum::<f32>();
        total.is_finite().then_some(total)
    }
}

/// One cell's lowered block-axis style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableCellBlockStyle {
    pub offsets: CellBlockOffsets,
    /// The cell's specified block size. CSS 2.1 section 17.5.3 makes this a
    /// constraint on the cell's row; it does not enlarge the cell's own
    /// content box, so it is kept apart from the measured content size.
    pub specified: TableBlockConstraint,
    pub box_sizing: TableBoxSizing,
}

impl Default for TableCellBlockStyle {
    fn default() -> Self {
        Self {
            offsets: CellBlockOffsets::ZERO,
            specified: TableBlockConstraint::Auto,
            box_sizing: TableBoxSizing::ContentBox,
        }
    }
}

/// One row's measured block-axis facts, in K4b row order.
///
/// `row` is `None` for a row track created implicitly by placement, which has
/// no CSS box. Inventing an identity for it would make a later fragment
/// attributable to a box that does not exist.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableRowMeasure {
    pub row: Option<BoxId>,
    /// The content-and-constraint minimum for this row's border boxes. It
    /// never includes separated spacing, which the table level adds exactly
    /// once.
    pub min_block_size: f32,
    /// The row's own specified constraint, retained unreduced so K4d4 can
    /// resolve a percentage once a basis exists.
    pub preferred: TableBlockConstraint,
    /// Whether the row or one of its single-row cells supplied a definite,
    /// non-percentage block size.
    pub constrained: bool,
}

/// Complete block-sizing input. Row layout consumes the accepted K4c inline
/// result; it never re-derives a column.
#[derive(Clone, Debug, PartialEq)]
pub struct TableBlockSizingInput<'a> {
    pub grid: &'a TableGrid,
    pub inline: &'a TableInlineSizingResult,
    pub table_constraint: TableBlockConstraint,
    pub border_metrics: TableBlockBorderMetrics,
    pub available_block_size: Option<f32>,
    pub track_visibility: TableTrackVisibility,
}

/// One cell's final placement, covering exactly its normalized span.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableCellPlacement {
    pub box_id: BoxId,
    pub row_start: usize,
    pub row_span: usize,
    pub column_start: usize,
    pub column_span: usize,
    /// The cell's border-box rectangle in the table's logical axes.
    pub rect: LogicalRect,
    /// Block-axis alignment offset applied to the cell's content.
    pub content_block_offset: f32,
}

/// The complete row-layout result. Offsets and sizes use the table's logical
/// block axis; physical coordinates are derived only when final fragments are
/// committed.
#[derive(Clone, Debug, PartialEq)]
pub struct TableRowLayoutResult {
    pub used_table_block_size: f32,
    pub row_offsets: Vec<f32>,
    pub row_sizes: Vec<f32>,
    pub cells: Vec<TableCellPlacement>,
    pub baselines: Baselines,
    pub overflow: LogicalRect,
    pub fragments: FragmentDraftTree,
}

/// One draft fragment. Drafts are deliberately not `Fragment`s: nothing can
/// insert them into a `FragmentTree` without the explicit commit that K4d6
/// owns, so a discarded measurement pass cannot leak into painted output by
/// construction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FragmentDraft {
    pub box_id: BoxId,
    pub logical_rect: LogicalRect,
    pub overflow: LogicalRect,
    /// Index of the parent draft within the same tree, tree order.
    pub parent: Option<usize>,
}

/// Draft fragments from one formatting pass. Replacing a cell's output
/// replaces its whole draft tree; there is no partial merge.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FragmentDraftTree {
    drafts: Vec<FragmentDraft>,
}

impl FragmentDraftTree {
    pub fn push(&mut self, draft: FragmentDraft) -> Option<usize> {
        if let Some(parent) = draft.parent
            && parent >= self.drafts.len()
        {
            return None;
        }
        self.drafts.push(draft);
        Some(self.drafts.len() - 1)
    }

    pub fn drafts(&self) -> &[FragmentDraft] {
        &self.drafts
    }

    pub fn len(&self) -> usize {
        self.drafts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.drafts.is_empty()
    }
}

/// The exact content inline size for one cell: K4c's spanned columns, plus
/// one separated spacing interval per crossed column boundary, minus the
/// cell's resolved inline offsets.
pub fn spanned_cell_content_inline_size(
    inline: &TableInlineSizingResult,
    inline_spacing: f32,
    box_id: BoxId,
    column_start: usize,
    column_span: usize,
    resolved_inline_offsets: f32,
) -> Result<f32, TableRowLayoutError> {
    let columns = inline.column_sizes.len();
    let span_end = column_start.checked_add(column_span);
    if column_span == 0 || span_end.is_none_or(|end| end > columns) {
        return Err(TableRowLayoutError::ColumnSpanOutOfBounds {
            box_id,
            column_start,
            column_span,
            columns,
        });
    }
    if !inline_spacing.is_finite()
        || inline_spacing < 0.0
        || !resolved_inline_offsets.is_finite()
        || resolved_inline_offsets < 0.0
    {
        return Err(TableRowLayoutError::InvalidCellOutput { box_id });
    }
    let spanned: f32 = inline.column_sizes[column_start..column_start + column_span]
        .iter()
        .sum();
    let crossed_gaps = (column_span - 1) as f32;
    Ok((spanned + inline_spacing * crossed_gaps - resolved_inline_offsets).max(0.0))
}

/// K4d1's dispatch skeleton: format every grid cell at its exact K4c inline
/// size through the adapter's formatter, first pass. Row arithmetic over the
/// outputs is owned by K4d2 onward.
///
/// `resolved_offsets_of` supplies each cell's resolved inline offsets by K4b
/// cell index; the basis is real once the inline result exists, so the value
/// is a plain resolved total.
pub fn format_table_cells(
    input: &TableBlockSizingInput<'_>,
    inline_spacing: f32,
    mut resolved_offsets_of: impl FnMut(usize, BoxId) -> f32,
    formatter: &mut impl TableCellFormatter,
) -> Result<Vec<(BoxId, TableCellLayoutOutput)>, TableRowLayoutError> {
    if input.inline.column_sizes.len() != input.grid.columns.len() {
        return Err(TableRowLayoutError::InlineResultMismatch {
            expected: input.grid.columns.len(),
            actual: input.inline.column_sizes.len(),
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

    let mut outputs = Vec::with_capacity(input.grid.cells.len());
    for (index, cell) in input.grid.cells.iter().enumerate() {
        let content_inline_size = spanned_cell_content_inline_size(
            input.inline,
            inline_spacing,
            cell.source,
            cell.column,
            cell.column_span,
            resolved_offsets_of(index, cell.source),
        )?;
        let output = formatter.format_cell(TableCellLayoutInput {
            box_id: cell.source,
            content_inline_size,
            available_block_size: input.available_block_size,
            // First-pass percentages have no row basis yet; K4d4 owns the
            // resolve pass.
            percentage_basis: None,
            pass: TableCellLayoutPass::Measure,
        })?;
        if !output.is_valid() {
            return Err(TableRowLayoutError::InvalidCellOutput {
                box_id: cell.source,
            });
        }
        outputs.push((cell.source, output));
    }
    Ok(outputs)
}

/// A definite, non-percentage block size, or `None`.
///
/// A percentage is never sampled at zero here: it survives in the retained
/// constraint so K4d4 resolves it once a basis exists.
fn definite_block_size(constraint: TableBlockConstraint) -> Option<f32> {
    match constraint {
        TableBlockConstraint::Value(value) if !value.needs_percentage_basis() => value
            .resolve(0.0)
            .filter(|size| size.is_finite() && *size >= 0.0),
        _ => None,
    }
}

/// K4d2: content-based minimum block sizes for every K4b row.
///
/// Per CSS 2.1 section 17.5.3, a row's minimum is the maximum of its own
/// specified height, the specified height contributions of cells that occupy
/// only that row, and the minimum those cells' contents require. A cell's
/// specified height is kept as a row constraint and never overwrites the
/// measured content box.
///
/// Cells spanning more than one row contribute nothing here; distributing a
/// spanning cell's minimum is K4d3's decision and CSS 2.1 leaves it
/// undefined. Separated spacing is excluded: the table level adds it exactly
/// once. Column sizes are read-only, so later-row content cannot feed back
/// into K4c.
pub fn measure_single_span_rows(
    input: &TableBlockSizingInput<'_>,
    cell_styles: &[TableCellBlockStyle],
    cell_outputs: &[(BoxId, TableCellLayoutOutput)],
    row_constraints: &[TableBlockConstraint],
) -> Result<Vec<TableRowMeasure>, TableRowLayoutError> {
    let grid = input.grid;
    if cell_styles.len() != grid.cells.len() {
        return Err(TableRowLayoutError::CellInputCountMismatch {
            expected: grid.cells.len(),
            actual: cell_styles.len(),
        });
    }
    if cell_outputs.len() != grid.cells.len() {
        return Err(TableRowLayoutError::CellInputCountMismatch {
            expected: grid.cells.len(),
            actual: cell_outputs.len(),
        });
    }
    if row_constraints.len() != grid.rows.len() {
        return Err(TableRowLayoutError::RowInputCountMismatch {
            expected: grid.rows.len(),
            actual: row_constraints.len(),
        });
    }
    for (index, (cell, (source, _))) in grid.cells.iter().zip(cell_outputs).enumerate() {
        if cell.source != *source {
            return Err(TableRowLayoutError::CellSourceMismatch {
                index,
                expected: cell.source,
                actual: *source,
            });
        }
    }

    let mut measures = Vec::with_capacity(grid.rows.len());
    for (index, track) in grid.rows.iter().enumerate() {
        let preferred = row_constraints[index];
        let row_definite = definite_block_size(preferred);
        let mut min_block_size = row_definite.unwrap_or(0.0);
        let mut constrained = row_definite.is_some();

        for (cell_index, cell) in grid.cells.iter().enumerate() {
            // A continuing rowspan is K4d3's; an empty row simply has no
            // originating single-row cell and stays at its own constraint.
            if cell.row != index || cell.row_span != 1 {
                continue;
            }
            let style = cell_styles[cell_index];
            let output = &cell_outputs[cell_index].1;
            if !output.is_valid() {
                return Err(TableRowLayoutError::InvalidCellOutput {
                    box_id: cell.source,
                });
            }
            let offsets = style
                .offsets
                .total()
                .ok_or(TableRowLayoutError::InvalidCellOutput {
                    box_id: cell.source,
                })?;
            // The content the cell actually needs, as a border box. Overflow
            // is deliberately not consulted: it is retained on the output and
            // never inflates a row.
            let content_required = output.content_block_size + offsets;
            min_block_size = min_block_size
                .max(content_required)
                .max(output.border_box_min_block_size);

            if let Some(specified) = definite_block_size(style.specified) {
                let as_border_box = match style.box_sizing {
                    TableBoxSizing::ContentBox => specified + offsets,
                    TableBoxSizing::BorderBox => specified,
                };
                min_block_size = min_block_size.max(as_border_box);
                constrained = true;
            }
        }

        if !min_block_size.is_finite() || min_block_size < 0.0 {
            return Err(TableRowLayoutError::InvalidCellOutput { box_id: grid.grid });
        }
        measures.push(TableRowMeasure {
            row: track.source,
            min_block_size,
            preferred,
            constrained,
        });
    }
    Ok(measures)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BoxGeneration, BoxOrigin, BoxTreeInput, CssBoxTree, DisplayInside, DisplayOutside,
        DisplayRole, FlowAxes, InternalTableRole, IntrinsicSizes, PositioningScheme,
        TableGridInputs, generate_box_tree,
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

    /// One row of three cells, the third spanning two columns in a
    /// four-column grid.
    fn grid() -> TableGrid {
        let cell = |id| {
            BoxTreeInput::new(
                BoxOrigin::Element(id),
                table_role(InternalTableRole::Cell),
                FlowAxes::HORIZONTAL_LTR,
                PositioningScheme::Static,
                false,
                vec![],
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
                BoxOrigin::Element(2),
                table_role(InternalTableRole::Row),
                FlowAxes::HORIZONTAL_LTR,
                PositioningScheme::Static,
                false,
                vec![cell(3), cell(4), cell(5)],
            )],
        )]);
        let mut inputs = TableGridInputs::default();
        inputs.set_cell(
            tree.principal_box(5).expect("spanning cell"),
            super::super::TableCellInput {
                column_span: 2,
                ..super::super::TableCellInput::default()
            },
        );
        TableGrid::from_box_tree(&tree, tree.principal_box(1).expect("table grid"), &inputs)
    }

    fn inline_result(grid: &TableGrid, columns: Vec<f32>) -> TableInlineSizingResult {
        let total: f32 = columns.iter().sum();
        let sizing = super::super::TableInlineSizingInput {
            grid,
            available_inline_size: Some(total),
            table_constraints: super::super::TableInlineConstraints::default(),
            border_metrics: super::super::TableInlineBorderMetrics::Separated(
                super::super::TableSeparatedBorderMetrics::default(),
            ),
            caption_min: super::super::CaptionMinContribution::NoCaption,
            track_visibility: TableTrackVisibility::all_visible(grid),
        };
        TableInlineSizingResult::new(
            &sizing,
            IntrinsicSizes::new(total, total).expect("intrinsic pair"),
            total,
            total,
            columns,
        )
        .expect("reconciled inline result")
    }

    struct RecordingFormatter {
        requests: Vec<TableCellLayoutInput>,
    }

    impl TableCellFormatter for RecordingFormatter {
        fn format_cell(
            &mut self,
            input: TableCellLayoutInput,
        ) -> Result<TableCellLayoutOutput, TableRowLayoutError> {
            self.requests.push(input);
            let mut fragments = FragmentDraftTree::default();
            fragments.push(FragmentDraft {
                box_id: input.box_id,
                logical_rect: LogicalRect::default(),
                overflow: LogicalRect::default(),
                parent: None,
            });
            Ok(TableCellLayoutOutput {
                content_block_size: 10.0,
                border_box_min_block_size: 12.0,
                baselines: Baselines::synthesized_from_block_end(12.0),
                overflow: LogicalRect::default(),
                fragments,
            })
        }
    }

    #[test]
    fn cells_are_formatted_at_exact_spanned_inline_sizes() {
        let grid = grid();
        let inline = inline_result(&grid, vec![100.0, 80.0, 60.0, 40.0]);
        let input = TableBlockSizingInput {
            grid: &grid,
            inline: &inline,
            table_constraint: TableBlockConstraint::Auto,
            border_metrics: TableBlockBorderMetrics::Separated(
                TableSeparatedBlockMetrics::default(),
            ),
            available_block_size: None,
            track_visibility: TableTrackVisibility::all_visible(&grid),
        };
        let mut formatter = RecordingFormatter {
            requests: Vec::new(),
        };
        // 5px inline spacing; every cell carries 4px of resolved offsets.
        let outputs =
            format_table_cells(&input, 5.0, |_, _| 4.0, &mut formatter).expect("formatted cells");

        assert_eq!(outputs.len(), 3);
        let widths = formatter
            .requests
            .iter()
            .map(|request| request.content_inline_size)
            .collect::<Vec<_>>();
        // Single-span cells: column minus offsets. The spanning cell adds the
        // one crossed spacing interval: 60 + 40 + 5 - 4.
        assert_eq!(widths, vec![96.0, 76.0, 101.0]);
        assert!(
            formatter
                .requests
                .iter()
                .all(|request| request.pass == TableCellLayoutPass::Measure
                    && request.percentage_basis.is_none())
        );
    }

    #[test]
    fn collapsed_metrics_and_bad_spans_are_explicit() {
        let grid = grid();
        let inline = inline_result(&grid, vec![100.0, 80.0, 60.0, 40.0]);
        let mut formatter = RecordingFormatter {
            requests: Vec::new(),
        };
        let collapsed = TableBlockSizingInput {
            grid: &grid,
            inline: &inline,
            table_constraint: TableBlockConstraint::Auto,
            border_metrics: TableBlockBorderMetrics::CollapsedPendingK4g,
            available_block_size: None,
            track_visibility: TableTrackVisibility::all_visible(&grid),
        };
        assert_eq!(
            format_table_cells(&collapsed, 0.0, |_, _| 0.0, &mut formatter),
            Err(TableRowLayoutError::Deferral(
                TableBlockDeferral::CollapsedBlockBorderMetricsPendingK4g
            ))
        );

        // A span beyond K4c's columns is an error, never a clamp.
        let short = inline_result(&grid, vec![100.0, 80.0, 60.0, 40.0]);
        assert!(matches!(
            spanned_cell_content_inline_size(&short, 0.0, grid.cells[0].source, 3, 2, 0.0),
            Err(TableRowLayoutError::ColumnSpanOutOfBounds { .. })
        ));
    }

    /// A discarded measurement pass cannot leak fragments: drafts are not
    /// `Fragment`s, nothing commits them yet, and replacing a cell's output
    /// drops its whole draft tree.
    #[test]
    fn discarded_outputs_drop_their_draft_trees() {
        let grid = grid();
        let inline = inline_result(&grid, vec![100.0, 80.0, 60.0, 40.0]);
        let input = TableBlockSizingInput {
            grid: &grid,
            inline: &inline,
            table_constraint: TableBlockConstraint::Auto,
            border_metrics: TableBlockBorderMetrics::Separated(
                TableSeparatedBlockMetrics::default(),
            ),
            available_block_size: None,
            track_visibility: TableTrackVisibility::all_visible(&grid),
        };
        let mut formatter = RecordingFormatter {
            requests: Vec::new(),
        };
        let first =
            format_table_cells(&input, 0.0, |_, _| 0.0, &mut formatter).expect("first pass");
        assert!(first.iter().all(|(_, output)| output.fragments.len() == 1));
        // A second pass produces fresh outputs; the first pass's drafts have
        // no path into any FragmentTree and drop with the vector.
        drop(first);
        let second =
            format_table_cells(&input, 0.0, |_, _| 0.0, &mut formatter).expect("second pass");
        assert_eq!(second.len(), 3);
    }

    /// A grid from `rows`, each entry listing that row's cell element ids.
    /// `spans` maps an element id to its row span.
    fn multi_row_grid(rows: &[&[u8]], spans: &[(u8, usize)]) -> TableGrid {
        let cell = |id: u8| {
            BoxTreeInput::new(
                BoxOrigin::Element(id),
                table_role(InternalTableRole::Cell),
                FlowAxes::HORIZONTAL_LTR,
                PositioningScheme::Static,
                false,
                vec![],
            )
        };
        let row_inputs = rows
            .iter()
            .enumerate()
            .map(|(index, cells)| {
                BoxTreeInput::new(
                    BoxOrigin::Element(100 + index as u8),
                    table_role(InternalTableRole::Row),
                    FlowAxes::HORIZONTAL_LTR,
                    PositioningScheme::Static,
                    false,
                    cells.iter().copied().map(cell).collect(),
                )
            })
            .collect::<Vec<_>>();
        // One explicit row group, as `<tbody>` supplies in real markup. A
        // row span may not cross a row group, so rows in separate anonymous
        // groups would clamp every span to one row.
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
                row_inputs,
            )],
        )]);
        let mut inputs = TableGridInputs::default();
        for (id, span) in spans {
            inputs.set_cell(
                tree.principal_box(*id).expect("spanning cell"),
                super::super::TableCellInput {
                    row_span: super::super::TableRowSpan::Count(*span),
                    ..super::super::TableCellInput::default()
                },
            );
        }
        TableGrid::from_box_tree(&tree, tree.principal_box(1).expect("table grid"), &inputs)
    }

    fn block_input<'a>(
        grid: &'a TableGrid,
        inline: &'a TableInlineSizingResult,
    ) -> TableBlockSizingInput<'a> {
        TableBlockSizingInput {
            grid,
            inline,
            table_constraint: TableBlockConstraint::Auto,
            border_metrics: TableBlockBorderMetrics::Separated(
                TableSeparatedBlockMetrics::default(),
            ),
            available_block_size: None,
            track_visibility: TableTrackVisibility::all_visible(grid),
        }
    }

    fn output(content: f32, minimum: f32) -> TableCellLayoutOutput {
        TableCellLayoutOutput {
            content_block_size: content,
            border_box_min_block_size: minimum,
            baselines: Baselines::synthesized_from_block_end(content),
            overflow: LogicalRect::default(),
            fragments: FragmentDraftTree::default(),
        }
    }

    fn px(value: f32) -> TableBlockConstraint {
        TableBlockConstraint::Value(AffineLengthPercentage::px(value))
    }

    fn measures(
        grid: &TableGrid,
        columns: Vec<f32>,
        styles: Vec<TableCellBlockStyle>,
        outputs: Vec<TableCellLayoutOutput>,
        row_constraints: Vec<TableBlockConstraint>,
    ) -> Vec<TableRowMeasure> {
        let inline = inline_result(grid, columns);
        let input = block_input(grid, &inline);
        let paired = grid
            .cells
            .iter()
            .map(|cell| cell.source)
            .zip(outputs)
            .collect::<Vec<_>>();
        measure_single_span_rows(&input, &styles, &paired, &row_constraints).expect("row measures")
    }

    #[test]
    fn a_row_minimum_is_the_maximum_over_its_single_row_cells() {
        // Two rows of two cells with differing heights, plus padding and
        // border on one cell.
        let grid = multi_row_grid(&[&[3, 4], &[5, 6]], &[]);
        let padded = TableCellBlockStyle {
            offsets: CellBlockOffsets {
                padding_start: 2.0,
                padding_end: 3.0,
                border_start: 1.0,
                border_end: 4.0,
            },
            ..TableCellBlockStyle::default()
        };
        let rows = measures(
            &grid,
            vec![50.0, 50.0],
            vec![
                TableCellBlockStyle::default(),
                padded,
                TableCellBlockStyle::default(),
                TableCellBlockStyle::default(),
            ],
            vec![
                output(10.0, 0.0),
                // 20 content + 10 offsets = 30 border box, the row maximum.
                output(20.0, 0.0),
                output(7.0, 0.0),
                output(5.0, 0.0),
            ],
            vec![TableBlockConstraint::Auto; 2],
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].min_block_size, 30.0);
        assert_eq!(rows[1].min_block_size, 7.0);
        assert!(rows.iter().all(|row| !row.constrained));
    }

    #[test]
    fn cell_order_within_a_row_does_not_change_its_minimum() {
        let grid = multi_row_grid(&[&[3, 4, 5]], &[]);
        let styles = vec![TableCellBlockStyle::default(); 3];
        let heights = [9.0_f32, 21.0, 14.0];
        let forward = measures(
            &grid,
            vec![10.0; 3],
            styles.clone(),
            heights.iter().map(|h| output(*h, 0.0)).collect(),
            vec![TableBlockConstraint::Auto],
        );
        let reversed = measures(
            &grid,
            vec![10.0; 3],
            styles,
            heights.iter().rev().map(|h| output(*h, 0.0)).collect(),
            vec![TableBlockConstraint::Auto],
        );
        assert_eq!(forward[0].min_block_size, 21.0);
        assert_eq!(reversed[0].min_block_size, 21.0);
    }

    /// CSS 2.1 section 17.5.3: a cell's specified height constrains its row
    /// but does not enlarge the cell's own content box. The measured content
    /// stays available for the K4d4 pass.
    #[test]
    fn a_specified_cell_height_constrains_the_row_without_replacing_content() {
        let grid = multi_row_grid(&[&[3, 4]], &[]);
        let tall = TableCellBlockStyle {
            offsets: CellBlockOffsets {
                padding_start: 1.0,
                padding_end: 1.0,
                ..CellBlockOffsets::ZERO
            },
            specified: px(40.0),
            ..TableCellBlockStyle::default()
        };
        let rows = measures(
            &grid,
            vec![10.0, 10.0],
            vec![tall, TableCellBlockStyle::default()],
            // Content is only 5px; the 40px content-box specification plus
            // 2px offsets is what constrains the row.
            vec![output(5.0, 0.0), output(6.0, 0.0)],
            vec![TableBlockConstraint::Auto],
        );
        assert_eq!(rows[0].min_block_size, 42.0);
        assert!(rows[0].constrained);

        // A border-box specification is already the border box.
        let border_box = TableCellBlockStyle {
            box_sizing: TableBoxSizing::BorderBox,
            ..tall
        };
        let rows = measures(
            &grid,
            vec![10.0, 10.0],
            vec![border_box, TableCellBlockStyle::default()],
            vec![output(5.0, 0.0), output(6.0, 0.0)],
            vec![TableBlockConstraint::Auto],
        );
        assert_eq!(rows[0].min_block_size, 40.0);
    }

    #[test]
    fn a_row_height_competes_with_its_cells_and_percentages_survive() {
        let grid = multi_row_grid(&[&[3], &[4]], &[]);
        let rows = measures(
            &grid,
            vec![10.0],
            vec![TableCellBlockStyle::default(); 2],
            vec![output(30.0, 0.0), output(8.0, 0.0)],
            // Row 0's 12px loses to its 30px cell; row 1's 25px wins.
            vec![px(12.0), px(25.0)],
        );
        assert_eq!(rows[0].min_block_size, 30.0);
        assert_eq!(rows[1].min_block_size, 25.0);
        assert!(rows.iter().all(|row| row.constrained));

        // A percentage row height is never sampled at zero: it contributes
        // nothing definite and survives for K4d4 to resolve.
        let percentage =
            TableBlockConstraint::Value(AffineLengthPercentage::new(0.0, 0.5).expect("finite"));
        let rows = measures(
            &grid,
            vec![10.0],
            vec![TableCellBlockStyle::default(); 2],
            vec![output(30.0, 0.0), output(8.0, 0.0)],
            vec![percentage, TableBlockConstraint::Auto],
        );
        assert_eq!(rows[0].min_block_size, 30.0);
        assert!(!rows[0].constrained);
        assert_eq!(rows[0].preferred, percentage);
    }

    /// An empty row, a row short of cells, and a row holding only a
    /// continuing rowspan each stay at their own constraint. Nothing is
    /// invented for the missing slots.
    #[test]
    fn empty_rows_missing_cells_and_continuing_rowspans_invent_nothing() {
        // Row 0 has a cell spanning both rows plus a single-row cell; row 1
        // has one cell; row 2 is empty.
        let grid = multi_row_grid(&[&[3, 4], &[5], &[]], &[(3, 2)]);
        assert_eq!(grid.rows.len(), 3);
        // The spanning cell occupies rows 0 and 1; row 1's own cell is
        // displaced to column 1 by that occupancy.
        assert_eq!((grid.cells[0].row, grid.cells[0].row_span), (0, 2));
        let rows = measures(
            &grid,
            vec![10.0, 10.0],
            vec![TableCellBlockStyle::default(); 3],
            vec![
                // The spanning cell is tall but must not size row 0 alone.
                output(90.0, 0.0),
                output(11.0, 0.0),
                output(13.0, 0.0),
            ],
            vec![
                TableBlockConstraint::Auto,
                TableBlockConstraint::Auto,
                px(6.0),
            ],
        );
        assert_eq!(rows[0].min_block_size, 11.0);
        assert_eq!(rows[1].min_block_size, 13.0);
        // The empty row keeps only its own specified height.
        assert_eq!(rows[2].min_block_size, 6.0);
        assert!(rows[2].constrained);
    }

    /// Overflow is retained on the cell output and never inflates the row,
    /// and separated spacing belongs to the table level exactly once.
    #[test]
    fn overflow_and_spacing_stay_out_of_the_row_minimum() {
        let grid = multi_row_grid(&[&[3]], &[]);
        let mut overflowing = output(10.0, 0.0);
        overflowing.overflow = LogicalRect {
            inline_start: 0.0,
            block_start: 0.0,
            inline_size: 500.0,
            block_size: 500.0,
        };
        let rows = measures(
            &grid,
            vec![10.0],
            vec![TableCellBlockStyle::default()],
            vec![overflowing],
            vec![TableBlockConstraint::Auto],
        );
        assert_eq!(rows[0].min_block_size, 10.0);

        // Spacing is a table-level total: two edges plus one interval per gap.
        let metrics = TableSeparatedBlockMetrics {
            table_offset_start: 1.0,
            table_offset_end: 2.0,
            block_spacing: 5.0,
        };
        assert_eq!(metrics.undistributable_block_size(3), Some(23.0));
    }

    #[test]
    fn subpixel_minima_and_misaligned_inputs_are_exact() {
        let grid = multi_row_grid(&[&[3]], &[]);
        let rows = measures(
            &grid,
            vec![10.0],
            vec![TableCellBlockStyle {
                offsets: CellBlockOffsets {
                    padding_start: 0.25,
                    padding_end: 0.25,
                    ..CellBlockOffsets::ZERO
                },
                ..TableCellBlockStyle::default()
            }],
            vec![output(10.5, 0.0)],
            vec![TableBlockConstraint::Auto],
        );
        assert_eq!(rows[0].min_block_size, 11.0);

        // A per-row input vector of the wrong length is an explicit error.
        let inline = inline_result(&grid, vec![10.0]);
        let input = block_input(&grid, &inline);
        let paired = vec![(grid.cells[0].source, output(1.0, 0.0))];
        assert_eq!(
            measure_single_span_rows(
                &input,
                &[TableCellBlockStyle::default()],
                &paired,
                &[TableBlockConstraint::Auto; 2],
            ),
            Err(TableRowLayoutError::RowInputCountMismatch {
                expected: 1,
                actual: 2,
            })
        );
    }

    #[test]
    fn a_parent_draft_must_precede_its_children() {
        let box_id = grid().cells[0].source;
        let mut drafts = FragmentDraftTree::default();
        assert!(
            drafts
                .push(FragmentDraft {
                    box_id,
                    logical_rect: LogicalRect::default(),
                    overflow: LogicalRect::default(),
                    parent: Some(0),
                })
                .is_none()
        );
        let root = drafts.push(FragmentDraft {
            box_id,
            logical_rect: LogicalRect::default(),
            overflow: LogicalRect::default(),
            parent: None,
        });
        assert_eq!(root, Some(0));
    }
}
