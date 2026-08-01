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
    AffineLengthPercentage, TableGrid, TableInlineSizingError, TableInlineSizingResult,
    TableTrackVisibility,
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

/// One row's measured block-axis facts, in K4b row order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableRowMeasure {
    pub row: BoxId,
    pub min_block_size: f32,
    pub preferred: TableBlockConstraint,
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
