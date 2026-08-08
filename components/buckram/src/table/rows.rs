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

impl TableBlockSizingInput<'_> {
    /// The part of a definite table block size the rows may occupy, with the
    /// table's own box-sizing already applied. `None` when the table has no
    /// definite block size.
    ///
    /// Everything downstream reads this rather than `table_constraint`
    /// directly, so the box-sizing decision is made once: it is both the
    /// target row distribution grows toward and the basis a percentage row or
    /// cell height resolves against.
    fn distributable_block_size(&self, undistributable: f32) -> Option<f32> {
        let definite = definite_block_size(self.table_constraint)?;
        let distributable = match self.table_box_sizing {
            TableBoxSizing::ContentBox => definite,
            TableBoxSizing::BorderBox => definite - undistributable,
        };
        distributable.is_finite().then(|| distributable.max(0.0))
    }
}

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

/// Collapsed-model geometry outside distributable row tracks.
///
/// The two values are K4g3's accepted half-width outer winners. Collapsed
/// rows have neither table border-spacing nor a second declared table border
/// contribution.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TableCollapsedBlockMetrics {
    pub table_padding_start: f32,
    pub table_padding_end: f32,
    pub outer_start: f32,
    pub outer_end: f32,
}

impl TableCollapsedBlockMetrics {
    pub fn undistributable_block_size(self, _row_count: usize) -> Option<f32> {
        [
            self.table_padding_start,
            self.table_padding_end,
            self.outer_start,
            self.outer_end,
        ]
        .into_iter()
        .all(|value| value.is_finite() && value >= 0.0)
        .then_some(
            self.table_padding_start + self.table_padding_end + self.outer_start + self.outer_end,
        )
        .filter(|total| total.is_finite())
    }
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
    Collapsed(TableCollapsedBlockMetrics),
    CollapsedPendingK4g,
}

#[derive(Clone, Copy, Debug)]
struct ResolvedBlockMetrics {
    table_offset_start: f32,
    block_spacing: f32,
    undistributable: f32,
}

fn resolved_block_metrics(
    metrics: TableBlockBorderMetrics,
    row_count: usize,
    table: BoxId,
) -> Result<ResolvedBlockMetrics, TableRowLayoutError> {
    let resolved = match metrics {
        TableBlockBorderMetrics::Separated(metrics) => ResolvedBlockMetrics {
            table_offset_start: metrics.table_offset_start,
            block_spacing: metrics.block_spacing,
            undistributable: 0.0,
        },
        TableBlockBorderMetrics::Collapsed(metrics) => ResolvedBlockMetrics {
            table_offset_start: metrics.table_padding_start + metrics.outer_start,
            block_spacing: 0.0,
            undistributable: 0.0,
        },
        TableBlockBorderMetrics::CollapsedPendingK4g => {
            return Err(TableRowLayoutError::Deferral(
                TableBlockDeferral::CollapsedBlockBorderMetricsPendingK4g,
            ));
        },
    };
    let undistributable = match metrics {
        TableBlockBorderMetrics::Separated(metrics) => {
            metrics.undistributable_block_size(row_count)
        },
        TableBlockBorderMetrics::Collapsed(metrics) => {
            metrics.undistributable_block_size(row_count)
        },
        TableBlockBorderMetrics::CollapsedPendingK4g => unreachable!(),
    };
    let Some(undistributable) = undistributable else {
        return Err(TableRowLayoutError::InvalidCellOutput { box_id: table });
    };
    Ok(ResolvedBlockMetrics {
        undistributable,
        ..resolved
    })
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
    /// Non-empty per-row-group inputs must follow K4b's visual group order.
    /// An empty slice means every group is `auto`, which keeps pure callers
    /// from inventing a CSS box just to name an unconstrained group.
    RowGroupInputCountMismatch {
        expected: usize,
        actual: usize,
    },
    /// K4b topology supplied a row-group range outside its row vector.
    RowGroupRangeOutOfBounds {
        start: usize,
        span: usize,
        rows: usize,
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

    /// The distance from the cell's border-box block-start edge to its
    /// content-box block-start edge.
    pub fn block_start(self) -> f32 {
        self.border_start + self.padding_start
    }

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

/// How a cell aligns its contents in the table's block axis.
///
/// CSS 2.1 section 17.5.3 gives table cells only these four behaviors.
/// `sub`, `super`, `text-top`, `text-bottom`, lengths, and percentages do not
/// apply to a table cell and behave as `Baseline`; the adapter collapses them
/// when it lowers `vertical-align`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TableCellAlignment {
    #[default]
    Baseline,
    Top,
    Middle,
    Bottom,
}

/// One cell's lowered block-axis style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableCellBlockStyle {
    pub alignment: TableCellAlignment,
    pub offsets: CellBlockOffsets,
    /// The cell's specified block size. CSS 2.1 section 17.5.3 makes this a
    /// constraint on the cell's row; it does not enlarge the cell's own
    /// content box, so it is kept apart from the measured content size.
    pub specified: TableBlockConstraint,
    pub box_sizing: TableBoxSizing,
    /// Whether the cell's contents contain a block size that gains a basis
    /// once the cell's own used block size is known. The adapter computes
    /// this from computed styles. A cell whose dependency set is empty is
    /// never relaid out, which is what keeps the percentage pass cheap.
    pub percentage_dependent_contents: bool,
}

impl Default for TableCellBlockStyle {
    fn default() -> Self {
        Self {
            alignment: TableCellAlignment::Baseline,
            offsets: CellBlockOffsets::ZERO,
            specified: TableBlockConstraint::Auto,
            box_sizing: TableBoxSizing::ContentBox,
            percentage_dependent_contents: false,
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
    /// The table's own box-sizing. This cannot be assumed: the UA stylesheet
    /// gives a `<table>` element `border-box` and leaves a `display: table`
    /// box at `content-box`, so the same specified height means two different
    /// used sizes.
    pub table_box_sizing: TableBoxSizing,
    /// Definite `height` constraints for K4b row groups, in visual row-group
    /// order. An empty slice means every group is unconstrained; otherwise it
    /// must align exactly with `grid.row_groups`.
    pub row_group_constraints: &'a [TableBlockConstraint],
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
    let _ = resolved_block_metrics(input.border_metrics, input.grid.rows.len(), input.grid.grid)?;

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

/// One cell's required border-box block size, and whether its own specified
/// height supplied a definite contribution.
fn cell_required_block_size(
    style: TableCellBlockStyle,
    output: &TableCellLayoutOutput,
    box_id: BoxId,
) -> Result<(f32, bool), TableRowLayoutError> {
    if !output.is_valid() {
        return Err(TableRowLayoutError::InvalidCellOutput { box_id });
    }
    let offsets = style
        .offsets
        .total()
        .ok_or(TableRowLayoutError::InvalidCellOutput { box_id })?;
    // Overflow is deliberately not consulted: it is retained on the output
    // and never inflates a row.
    let mut required = (output.content_block_size + offsets).max(output.border_box_min_block_size);
    let mut constrained = false;
    if let Some(specified) = definite_block_size(style.specified) {
        let as_border_box = match style.box_sizing {
            TableBoxSizing::ContentBox => specified + offsets,
            TableBoxSizing::BorderBox => specified,
        };
        required = required.max(as_border_box);
        constrained = true;
    }
    Ok((required, constrained))
}

/// The zero-weight fallback for a distribution site. Both branches are
/// measured, not assumed; see the K4d3 interop record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ZeroWeightFallback {
    /// Table and row-group height over rows with no measure at all: equal
    /// shares. Chrome 150 and Firefox 153 both give `[150, 150]` for a 300px
    /// table over two empty rows.
    EqualShares,
    /// Rowspan excess over rows with no measure at all: the last spanned row
    /// takes everything. Both engines give `[0, 200]`, not `[100, 100]`.
    LastEligibleRow,
}

/// Grow `sizes[scope]` to total `target`, following the measured rule.
///
/// Rows without a definite specified height absorb the growth. When every
/// row in scope is constrained they all participate instead, in proportion
/// to their sizes: Chrome 150 and Firefox 153 agree on that for a rowspan
/// (`[80, 120]` over definite 20/30), and Chrome extends it to table height
/// where Firefox instead splits the excess equally. Buckram follows Chrome
/// there, because it keeps one rule for both sites rather than two.
///
/// Weights are the rows' current sizes, so rows never shrink: the function
/// is only entered when `target` exceeds the scope's current total.
fn distribute_over_rows(
    sizes: &mut [f32],
    constrained: &[bool],
    start: usize,
    span: usize,
    target: f32,
    fallback: ZeroWeightFallback,
) {
    let end = start + span;
    let unconstrained = (start..end).filter(|index| !constrained[*index]).count();
    let eligible = (start..end)
        .filter(|index| unconstrained == 0 || !constrained[*index])
        .collect::<Vec<_>>();
    if eligible.is_empty() {
        return;
    }
    let fixed = (start..end)
        .filter(|index| !eligible.contains(index))
        .map(|index| sizes[index])
        .sum::<f32>();
    let share = (target - fixed).max(0.0);
    let weight = eligible.iter().map(|index| sizes[*index]).sum::<f32>();

    let mut remainder = share;
    for (position, index) in eligible.iter().enumerate() {
        let last = position + 1 == eligible.len();
        let size = if last {
            remainder
        } else if weight > 0.0 {
            share * sizes[*index] / weight
        } else {
            match fallback {
                ZeroWeightFallback::EqualShares => share / eligible.len() as f32,
                ZeroWeightFallback::LastEligibleRow => 0.0,
            }
        };
        sizes[*index] = size;
        remainder -= size;
    }
}

/// K4d3's block-axis result.
#[derive(Clone, Debug, PartialEq)]
pub struct TableRowSizing {
    pub used_table_block_size: f32,
    pub row_offsets: Vec<f32>,
    pub row_sizes: Vec<f32>,
}

/// K4d3: fit every spanning cell within its row range, then choose the
/// table's used block size.
///
/// Spanning cells are processed in increasing span order, so a wider span
/// always sees the rows a narrower one already grew. A definite table block
/// size is a minimum, never a maximum: a table shorter than its rows keeps
/// the rows. See the K4d3 interop record for the measured distribution.
pub fn size_table_rows(
    input: &TableBlockSizingInput<'_>,
    measures: &[TableRowMeasure],
    cell_styles: &[TableCellBlockStyle],
    cell_outputs: &[(BoxId, TableCellLayoutOutput)],
) -> Result<TableRowSizing, TableRowLayoutError> {
    let grid = input.grid;
    if measures.len() != grid.rows.len() {
        return Err(TableRowLayoutError::RowInputCountMismatch {
            expected: grid.rows.len(),
            actual: measures.len(),
        });
    }
    if !input.row_group_constraints.is_empty()
        && input.row_group_constraints.len() != grid.row_groups.len()
    {
        return Err(TableRowLayoutError::RowGroupInputCountMismatch {
            expected: grid.row_groups.len(),
            actual: input.row_group_constraints.len(),
        });
    }
    if cell_styles.len() != grid.cells.len() || cell_outputs.len() != grid.cells.len() {
        return Err(TableRowLayoutError::CellInputCountMismatch {
            expected: grid.cells.len(),
            actual: cell_styles.len().min(cell_outputs.len()),
        });
    }
    let metrics = resolved_block_metrics(input.border_metrics, grid.rows.len(), grid.grid)?;
    let undistributable = metrics.undistributable;

    let mut sizes = measures
        .iter()
        .map(|measure| measure.min_block_size)
        .collect::<Vec<_>>();
    let constrained = measures
        .iter()
        .map(|measure| measure.constrained)
        .collect::<Vec<_>>();

    // Increasing span order: a wider span must see the rows a narrower one
    // already grew, never the other way round.
    let mut spanning = Vec::new();
    for (index, cell) in grid.cells.iter().enumerate() {
        if cell.row_span <= 1 {
            continue;
        }
        if cell.row + cell.row_span > grid.rows.len() {
            return Err(TableRowLayoutError::Deferral(
                TableBlockDeferral::FragmentationDependentRowspan,
            ));
        }
        let (required, _) =
            cell_required_block_size(cell_styles[index], &cell_outputs[index].1, cell.source)?;
        spanning.push((cell.row_span, cell.row, required));
    }
    spanning.sort_by_key(|(span, row, _)| (*span, *row));

    for (span, row, required) in spanning {
        // Spacing between the spanned rows counts toward the cell's range,
        // so only the row sizes themselves have to make up the remainder.
        let crossed_spacing = metrics.block_spacing * (span - 1) as f32;
        let available = sizes[row..row + span].iter().sum::<f32>() + crossed_spacing;
        if required <= available {
            continue;
        }
        distribute_over_rows(
            &mut sizes,
            &constrained,
            row,
            span,
            required - crossed_spacing,
            ZeroWeightFallback::LastEligibleRow,
        );
    }

    // CSS 2.1 leaves row-group height undefined, but the focused T4 matrix
    // agrees across Chrome and Firefox: a definite group height is a minimum
    // distributed over exactly that group's rows by the normal table-height
    // rule. It must run after spanning-cell minima, since a group only grows
    // the row sizes K4d already established; the table's own minimum follows
    // afterwards and can grow every row in turn.
    for (group_index, group) in grid.row_groups.iter().enumerate() {
        let constraint = input
            .row_group_constraints
            .get(group_index)
            .copied()
            .unwrap_or(TableBlockConstraint::Auto);
        let Some(target) = definite_block_size(constraint) else {
            continue;
        };
        let Some(end) = group.start.checked_add(group.span) else {
            return Err(TableRowLayoutError::RowGroupRangeOutOfBounds {
                start: group.start,
                span: group.span,
                rows: grid.rows.len(),
            });
        };
        if end > sizes.len() {
            return Err(TableRowLayoutError::RowGroupRangeOutOfBounds {
                start: group.start,
                span: group.span,
                rows: grid.rows.len(),
            });
        }
        let current = sizes[group.start..end].iter().sum::<f32>();
        if target > current {
            distribute_over_rows(
                &mut sizes,
                &constrained,
                group.start,
                group.span,
                target,
                ZeroWeightFallback::EqualShares,
            );
        }
    }

    let rows_total = sizes.iter().sum::<f32>();
    let distributable = input.distributable_block_size(undistributable);
    if let Some(distributable) = distributable
        && distributable > rows_total
    {
        distribute_over_rows(
            &mut sizes,
            &constrained,
            0,
            grid.rows.len(),
            distributable,
            ZeroWeightFallback::EqualShares,
        );
    }

    // K4f: a collapsed row is removed after the distribution, never before it.
    // CSS 2.1 section 17.5.5 reduces the table's height by exactly what the
    // row occupied and leaves every other row the height it was given, so the
    // collapse is a subtraction rather than an input - which is also what
    // keeps the constraints that produced those heights intact.
    for (index, size) in sizes.iter_mut().enumerate() {
        if input.track_visibility.row_is_collapsed(index) {
            *size = 0.0;
        }
    }

    let mut row_offsets = Vec::with_capacity(grid.rows.len());
    let mut cursor = metrics.table_offset_start + metrics.block_spacing;
    for (index, size) in sizes.iter().enumerate() {
        row_offsets.push(cursor);
        // A collapsed row takes its following spacing interval with it;
        // leaving one behind would show the row's absence as a gap.
        if input.track_visibility.row_is_collapsed(index) {
            continue;
        }
        cursor += size + metrics.block_spacing;
    }
    // K4d3's rule stated directly rather than only as a consequence of the
    // distribution above: a definite table block size is a minimum. The
    // distribution reaches it whenever there is a row to grow, but a table
    // with no rows at all has nowhere to put it, and dropping it there would
    // collapse an empty table with a height to nothing.
    let collapsed_spacing = metrics.block_spacing
        * (0..grid.rows.len())
            .filter(|index| input.track_visibility.row_is_collapsed(*index))
            .count() as f32;
    let used_table_block_size = ((sizes.iter().sum::<f32>() + undistributable) - collapsed_spacing)
        .max(0.0)
        .max(distributable.unwrap_or(0.0) + undistributable - collapsed_spacing);
    if !used_table_block_size.is_finite() || sizes.iter().any(|size| !size.is_finite()) {
        return Err(TableRowLayoutError::InvalidCellOutput { box_id: grid.grid });
    }
    Ok(TableRowSizing {
        used_table_block_size,
        row_offsets,
        row_sizes: sizes,
    })
}

/// One row's baseline, as a distance from the row's block-start edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableRowBaseline {
    pub baseline: f32,
    /// False when no baseline-aligned cell originated in the row, so CSS
    /// 2.1's synthesis from the lowest cell content edge supplied it.
    pub from_aligned_cell: bool,
}

/// K4d5's alignment result.
#[derive(Clone, Debug, PartialEq)]
pub struct TableAlignment {
    pub cells: Vec<TableCellPlacement>,
    pub rows: Vec<TableRowBaseline>,
    /// The table's exported baseline set: first from the first row, last from
    /// the last row, per CSS Box Alignment 3's baseline-set model. CSS 2.1
    /// section 10.8 only defines the first, as an inline table's baseline.
    pub baselines: Baselines,
}

/// A cell's own first baseline, measured from its border-box block-start.
///
/// The formatting context returned this directly; nothing walks a backend
/// tree to rediscover it.
fn cell_baseline(style: TableCellBlockStyle, output: &TableCellLayoutOutput) -> Option<f32> {
    output
        .baselines
        .first
        .map(|baseline| style.offsets.block_start() + baseline)
}

/// Raise each row's minimum so baseline-aligned cells fit once their
/// baselines share a row baseline.
///
/// CSS 2.1 section 17.5.3 makes this a genuine growth step: a row must hold
/// the deepest cell above the shared baseline plus the deepest below it, and
/// that sum can exceed the tallest single cell. K4d2's content minima cannot
/// see it, so it is applied before K4d3 chooses row sizes.
pub fn apply_baseline_row_minima(
    input: &TableBlockSizingInput<'_>,
    cell_styles: &[TableCellBlockStyle],
    cell_outputs: &[(BoxId, TableCellLayoutOutput)],
    measures: &mut [TableRowMeasure],
) -> Result<(), TableRowLayoutError> {
    let grid = input.grid;
    if measures.len() != grid.rows.len() {
        return Err(TableRowLayoutError::RowInputCountMismatch {
            expected: grid.rows.len(),
            actual: measures.len(),
        });
    }
    if cell_styles.len() != grid.cells.len() || cell_outputs.len() != grid.cells.len() {
        return Err(TableRowLayoutError::CellInputCountMismatch {
            expected: grid.cells.len(),
            actual: cell_styles.len().min(cell_outputs.len()),
        });
    }
    for (row, measure) in measures.iter_mut().enumerate() {
        let mut above: f32 = 0.0;
        let mut below: f32 = 0.0;
        let mut any = false;
        for (index, cell) in grid.cells.iter().enumerate() {
            // A spanning cell's baseline belongs to the row it starts in, so
            // it never participates in a later row's baseline.
            if cell.row != row
                || cell.row_span != 1
                || cell_styles[index].alignment != TableCellAlignment::Baseline
            {
                continue;
            }
            let output = &cell_outputs[index].1;
            let Some(baseline) = cell_baseline(cell_styles[index], output) else {
                continue;
            };
            let (required, _) = cell_required_block_size(cell_styles[index], output, cell.source)?;
            any = true;
            above = above.max(baseline);
            below = below.max(required - baseline);
        }
        if any {
            measure.min_block_size = measure.min_block_size.max(above + below.max(0.0));
        }
    }
    Ok(())
}

/// K4d5: place every cell's content within the final row geometry and export
/// the table's baseline set.
///
/// CSS 2.1 section 17.5.3's ordered procedure: baseline-aligned cells
/// establish the row baseline, top cells sit at the row's block-start, and
/// bottom and middle cells are placed last within the row height K4d3 and
/// `apply_baseline_row_minima` already fixed. Alignment reads the accepted
/// inline result without writing to it, so no column size can change here.
pub fn align_table_cells(
    input: &TableBlockSizingInput<'_>,
    sizing: &TableRowSizing,
    cell_styles: &[TableCellBlockStyle],
    cell_outputs: &[(BoxId, TableCellLayoutOutput)],
    inline_spacing: f32,
) -> Result<TableAlignment, TableRowLayoutError> {
    let grid = input.grid;
    if sizing.row_sizes.len() != grid.rows.len() || sizing.row_offsets.len() != grid.rows.len() {
        return Err(TableRowLayoutError::RowInputCountMismatch {
            expected: grid.rows.len(),
            actual: sizing.row_sizes.len(),
        });
    }
    if cell_styles.len() != grid.cells.len() || cell_outputs.len() != grid.cells.len() {
        return Err(TableRowLayoutError::CellInputCountMismatch {
            expected: grid.cells.len(),
            actual: cell_styles.len().min(cell_outputs.len()),
        });
    }
    let metrics = resolved_block_metrics(input.border_metrics, grid.rows.len(), grid.grid)?;

    // Step 1: every row's baseline, from the cells that originate in it.
    let mut rows = Vec::with_capacity(grid.rows.len());
    for row in 0..grid.rows.len() {
        let mut aligned: Option<f32> = None;
        let mut lowest_content: f32 = 0.0;
        for (index, cell) in grid.cells.iter().enumerate() {
            if cell.row != row || cell.row_span != 1 {
                continue;
            }
            let output = &cell_outputs[index].1;
            let style = cell_styles[index];
            lowest_content =
                lowest_content.max(style.offsets.block_start() + output.content_block_size);
            if style.alignment == TableCellAlignment::Baseline
                && let Some(baseline) = cell_baseline(style, output)
            {
                aligned = Some(aligned.map_or(baseline, |current: f32| current.max(baseline)));
            }
        }
        rows.push(match aligned {
            Some(baseline) => TableRowBaseline {
                baseline,
                from_aligned_cell: true,
            },
            // CSS 2.1: with no baseline-aligned cell the row's baseline is
            // synthesized from the lowest cell content edge.
            None => TableRowBaseline {
                baseline: lowest_content,
                from_aligned_cell: false,
            },
        });
    }

    // Inline offsets come straight from K4c's accepted columns.
    let mut inline_offsets = Vec::with_capacity(grid.columns.len());
    let mut cursor = inline_spacing;
    for size in &input.inline.column_sizes {
        inline_offsets.push(cursor);
        cursor += size + inline_spacing;
    }

    // Steps 2 to 4: place each cell's content inside its final rectangle.
    let mut cells = Vec::with_capacity(grid.cells.len());
    for (index, cell) in grid.cells.iter().enumerate() {
        let row_end = cell.row + cell.row_span;
        let column_end = cell.column + cell.column_span;
        if row_end > grid.rows.len() || column_end > input.inline.column_sizes.len() {
            return Err(TableRowLayoutError::ColumnSpanOutOfBounds {
                box_id: cell.source,
                column_start: cell.column,
                column_span: cell.column_span,
                columns: input.inline.column_sizes.len(),
            });
        }
        let style = cell_styles[index];
        let output = &cell_outputs[index].1;
        let offsets = style
            .offsets
            .total()
            .ok_or(TableRowLayoutError::InvalidCellOutput {
                box_id: cell.source,
            })?;
        let block_size = sizing.row_sizes[cell.row..row_end].iter().sum::<f32>()
            + metrics.block_spacing * (cell.row_span - 1) as f32;
        let inline_size = input.inline.column_sizes[cell.column..column_end]
            .iter()
            .sum::<f32>()
            + inline_spacing * (cell.column_span - 1) as f32;
        let free = (block_size - offsets - output.content_block_size).max(0.0);
        let content_block_offset = match style.alignment {
            TableCellAlignment::Top => 0.0,
            TableCellAlignment::Bottom => free,
            TableCellAlignment::Middle => free / 2.0,
            TableCellAlignment::Baseline => cell_baseline(style, output).map_or(0.0, |baseline| {
                // Shift the content so the cell's own baseline lands on the
                // row's. Extra fill is a placement offset, never a change to
                // the computed padding that produced `offsets`.
                (rows[cell.row].baseline - baseline).clamp(0.0, free)
            }),
        };
        cells.push(TableCellPlacement {
            box_id: cell.source,
            row_start: cell.row,
            row_span: cell.row_span,
            column_start: cell.column,
            column_span: cell.column_span,
            rect: LogicalRect {
                inline_start: inline_offsets
                    .get(cell.column)
                    .copied()
                    .unwrap_or(inline_spacing),
                block_start: sizing.row_offsets[cell.row],
                inline_size,
                block_size,
            },
            content_block_offset,
        });
    }

    // CSS Box Alignment 3: the table's first baseline comes from its first
    // row and its last from its last row. Offsets stay logical.
    let first = rows.first().map(|row| sizing.row_offsets[0] + row.baseline);
    let last = rows
        .last()
        .map(|row| sizing.row_offsets[grid.rows.len() - 1] + row.baseline);
    let baselines = Baselines::new(first, last)
        .ok_or(TableRowLayoutError::InvalidCellOutput { box_id: grid.grid })?;

    Ok(TableAlignment {
        cells,
        rows,
        baselines,
    })
}

/// The result of K4d4's bounded percentage pass.
#[derive(Clone, Debug, PartialEq)]
pub struct TablePercentagePass {
    pub sizing: TableRowSizing,
    /// Cells relaid out because their contents gained a definite basis, in
    /// K4b cell order. Every other cell stayed single-pass.
    pub relaid_out: Vec<BoxId>,
}

/// A percentage constraint resolved against `basis`, or `None` when the
/// constraint carries no percentage to resolve.
fn resolved_against(constraint: TableBlockConstraint, basis: f32) -> Option<TableBlockConstraint> {
    match constraint {
        TableBlockConstraint::Value(value) if value.needs_percentage_basis() => value
            .resolve(basis)
            .filter(|size| size.is_finite() && *size >= 0.0)
            .map(|size| TableBlockConstraint::Value(AffineLengthPercentage::px(size))),
        _ => None,
    }
}

/// K4d4: resolve percentage row, cell, and cell-descendant block sizes once
/// a valid basis exists, in a bounded pair of named passes.
///
/// The basis for a percentage row or cell height is the table's own
/// *specified* definite block size, never the used size K4d3 computed. That
/// distinction is measured: with an indefinite table height both engines
/// treat a 50% row or cell as automatic and the table stays at its content
/// height, rather than resolving 50% against the height the content just
/// produced. So no cycle can form, and `PercentageBlockCycle` is unreachable
/// from this path rather than being a loop guard.
///
/// A cell whose contents depend on its block size is relaid out once, with
/// the cell's final used block size, replacing its first-pass drafts and
/// overflow. That second format pass never re-drives row sizing: growth
/// discovered there would start an unbounded stabilization loop, and neither
/// engine grows a row for it.
pub fn resolve_percentage_block_sizes(
    input: &TableBlockSizingInput<'_>,
    first_pass: &TableRowSizing,
    measures: &[TableRowMeasure],
    cell_styles: &[TableCellBlockStyle],
    cell_outputs: &mut [(BoxId, TableCellLayoutOutput)],
    row_constraints: &[TableBlockConstraint],
    formatter: &mut impl TableCellFormatter,
) -> Result<TablePercentagePass, TableRowLayoutError> {
    let grid = input.grid;
    let metrics = resolved_block_metrics(input.border_metrics, grid.rows.len(), grid.grid)?;
    let undistributable = metrics.undistributable;

    // Only a specified definite table height is a basis for a percentage row
    // or cell height.
    let mut sizing = first_pass.clone();
    if let Some(basis) = input.distributable_block_size(undistributable) {
        let rows = row_constraints
            .iter()
            .map(|constraint| resolved_against(*constraint, basis).unwrap_or(*constraint))
            .collect::<Vec<_>>();
        let styles = cell_styles
            .iter()
            .map(|style| TableCellBlockStyle {
                specified: resolved_against(style.specified, basis).unwrap_or(style.specified),
                ..*style
            })
            .collect::<Vec<_>>();
        let resolved_anything = rows != row_constraints
            || styles
                .iter()
                .zip(cell_styles)
                .any(|(one, other)| one.specified != other.specified);
        if resolved_anything {
            let resolved_measures = measure_single_span_rows(input, &styles, cell_outputs, &rows)?;
            sizing = size_table_rows(input, &resolved_measures, &styles, cell_outputs)?;
            sizing = shrink_percentage_growth(sizing, measures, basis, metrics);
        }
    }

    // Relayout only the cells whose contents actually gained a basis.
    let mut relaid_out = Vec::new();
    for (index, cell) in grid.cells.iter().enumerate() {
        if !cell_styles[index].percentage_dependent_contents {
            continue;
        }
        let end = cell.row + cell.row_span;
        if end > sizing.row_sizes.len() {
            return Err(TableRowLayoutError::Deferral(
                TableBlockDeferral::FragmentationDependentRowspan,
            ));
        }
        let crossed = metrics.block_spacing * (cell.row_span - 1) as f32;
        let cell_block_size = sizing.row_sizes[cell.row..end].iter().sum::<f32>() + crossed;
        let offsets =
            cell_styles[index]
                .offsets
                .total()
                .ok_or(TableRowLayoutError::InvalidCellOutput {
                    box_id: cell.source,
                })?;
        let content_block_size = (cell_block_size - offsets).max(0.0);
        let output = formatter.format_cell(TableCellLayoutInput {
            box_id: cell.source,
            content_inline_size: spanned_cell_content_inline_size(
                input.inline,
                0.0,
                cell.source,
                cell.column,
                cell.column_span,
                0.0,
            )?,
            available_block_size: Some(content_block_size),
            percentage_basis: Some(content_block_size),
            pass: TableCellLayoutPass::ResolvePercentages {
                cell_block_size: content_block_size,
            },
        })?;
        if !output.is_valid() {
            return Err(TableRowLayoutError::InvalidCellOutput {
                box_id: cell.source,
            });
        }
        // The final pass replaces the draft subtree and overflow outright;
        // nothing merges a measurement pass into the result.
        cell_outputs[index].1 = output;
        relaid_out.push(cell.source);
    }

    Ok(TablePercentagePass { sizing, relaid_out })
}

/// Shrink percentage-derived row growth back into a definite table height.
///
/// K4d3 established that a definite table block size is a *minimum*: a table
/// shorter than its rows keeps the rows. That is measured and correct for
/// rows sized by their content or their own specified height, which cannot
/// give the space back. Percentage-derived growth is the opposite: it was
/// computed *from* the table's height, so letting it overflow that height
/// would double the table, which is what
/// `table-as-item-cell-percentage-002` catches.
///
/// So each row shrinks only across the distance between its K4d2 minimum and
/// the size the resolved percentages asked for, in proportion to that
/// distance, and never below the minimum. A row that grew for content or a
/// length is untouched because its growth here is zero.
///
/// See the K4d4b interop matrix: Chrome 150 and Firefox 153 agree on all
/// seven cases, and this single rule accounts for every one of them.
fn shrink_percentage_growth(
    resolved: TableRowSizing,
    minima: &[TableRowMeasure],
    distributable: f32,
    metrics: ResolvedBlockMetrics,
) -> TableRowSizing {
    if minima.len() != resolved.row_sizes.len() {
        return resolved;
    }
    let floors = minima
        .iter()
        .map(|measure| measure.min_block_size)
        .collect::<Vec<_>>();
    let floor_total = floors.iter().sum::<f32>();
    let total = resolved.row_sizes.iter().sum::<f32>();
    // A table shorter than its own content minimum still keeps that content.
    let target = distributable.max(floor_total);
    let excess = total - target;
    if excess <= 0.0 {
        return resolved;
    }
    let growth = resolved
        .row_sizes
        .iter()
        .zip(&floors)
        .map(|(size, floor)| (size - floor).max(0.0))
        .collect::<Vec<_>>();
    let growth_total = growth.iter().sum::<f32>();
    if growth_total <= 0.0 || !growth_total.is_finite() {
        return resolved;
    }
    let keep = ((growth_total - excess) / growth_total).clamp(0.0, 1.0);
    let sizes = floors
        .iter()
        .zip(&growth)
        .map(|(floor, grown)| floor + grown * keep)
        .collect::<Vec<_>>();
    let mut row_offsets = Vec::with_capacity(sizes.len());
    let mut cursor = metrics.table_offset_start + metrics.block_spacing;
    for size in &sizes {
        row_offsets.push(cursor);
        cursor += size + metrics.block_spacing;
    }
    let undistributable = resolved.used_table_block_size - total;
    TableRowSizing {
        used_table_block_size: sizes.iter().sum::<f32>() + undistributable,
        row_offsets,
        row_sizes: sizes,
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
            let (required, cell_constrained) = cell_required_block_size(
                cell_styles[cell_index],
                &cell_outputs[cell_index].1,
                cell.source,
            )?;
            min_block_size = min_block_size.max(required);
            constrained |= cell_constrained;
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
            table_box_sizing: TableBoxSizing::BorderBox,
            row_group_constraints: &[],
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
            table_box_sizing: TableBoxSizing::BorderBox,
            row_group_constraints: &[],
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
            table_box_sizing: TableBoxSizing::BorderBox,
            row_group_constraints: &[],
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
            table_box_sizing: TableBoxSizing::BorderBox,
            row_group_constraints: &[],
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

    /// Run one interop-matrix case: `rows` gives each row's (minimum,
    /// definite-height) pair, `span` an optional (row, span, required)
    /// spanning cell, and `table` the table's own constraint.
    fn matrix_case(
        row_minima: &[(f32, bool)],
        span: Option<(usize, usize, f32)>,
        table: TableBlockConstraint,
        metrics: TableSeparatedBlockMetrics,
    ) -> TableRowSizing {
        // One cell per row, plus the spanning cell in its starting row.
        // Element ids: row r's own cell is 3+r, the spanner is 200.
        let rows_spec = (0..row_minima.len())
            .map(|r| {
                if span.is_some_and(|(start, _, _)| start == r) {
                    vec![3u8 + r as u8, 200]
                } else {
                    vec![3u8 + r as u8]
                }
            })
            .collect::<Vec<_>>();
        let refs = rows_spec.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let span_inputs = span
            .map(|(_, span, _)| vec![(200u8, span)])
            .unwrap_or_default();
        let grid = multi_row_grid(&refs, &span_inputs);

        let mut styles = vec![TableCellBlockStyle::default(); grid.cells.len()];
        let mut outputs = Vec::with_capacity(grid.cells.len());
        for cell in &grid.cells {
            if cell.row_span > 1 {
                outputs.push(output(span.expect("spanning case").2, 0.0));
            } else {
                let (minimum, definite) = row_minima[cell.row];
                if definite {
                    styles[outputs.len()].specified = px(minimum);
                    outputs.push(output(0.0, 0.0));
                } else {
                    outputs.push(output(minimum, 0.0));
                }
            }
        }
        let inline = inline_result(&grid, vec![10.0; grid.columns.len()]);
        let input = TableBlockSizingInput {
            grid: &grid,
            inline: &inline,
            table_constraint: table,
            table_box_sizing: TableBoxSizing::BorderBox,
            row_group_constraints: &[],
            border_metrics: TableBlockBorderMetrics::Separated(metrics),
            available_block_size: None,
            track_visibility: TableTrackVisibility::all_visible(&grid),
        };
        let paired = grid
            .cells
            .iter()
            .map(|cell| cell.source)
            .zip(outputs)
            .collect::<Vec<_>>();
        let row_constraints = vec![TableBlockConstraint::Auto; grid.rows.len()];
        let measures = measure_single_span_rows(&input, &styles, &paired, &row_constraints)
            .expect("row measures");
        size_table_rows(&input, &measures, &styles, &paired).expect("row sizing")
    }

    fn close(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
        for (one, other) in actual.iter().zip(expected) {
            assert!(
                (one - other).abs() < 0.05,
                "{actual:?} vs expected {expected:?}"
            );
        }
    }

    /// The K4d3 interop matrix, measured in Chrome 150 and Firefox 153 on
    /// 2026-08-01. Every case below reproduces a measurement; see the
    /// gate receipt for the full table and the one divergence.
    #[test]
    fn rowspan_excess_follows_the_measured_interop_matrix() {
        let zero = TableSeparatedBlockMetrics::default();
        // S1: two automatic rows with minima 20/40, spanner needs 200.
        // Both engines: proportional to the minima.
        let s1 = matrix_case(
            &[(20.0, false), (40.0, false)],
            Some((0, 2, 200.0)),
            TableBlockConstraint::Auto,
            zero,
        );
        close(&s1.row_sizes, &[66.67, 133.33]);
        assert!((s1.used_table_block_size - 200.0).abs() < 0.05);

        // S2: a definite row does not grow while an automatic row can.
        let s2 = matrix_case(
            &[(20.0, true), (40.0, false)],
            Some((0, 2, 200.0)),
            TableBlockConstraint::Auto,
            zero,
        );
        close(&s2.row_sizes, &[20.0, 180.0]);

        // S3: three rows, minima 10/20/30.
        let s3 = matrix_case(
            &[(10.0, false), (20.0, false), (30.0, false)],
            Some((0, 3, 200.0)),
            TableBlockConstraint::Auto,
            zero,
        );
        close(&s3.row_sizes, &[33.33, 66.67, 100.0]);

        // S4: every spanned row is empty, so there is no proportion to
        // follow. Both engines give the whole excess to the last row, not
        // an equal split.
        let s4 = matrix_case(
            &[(0.0, false), (0.0, false)],
            Some((0, 2, 200.0)),
            TableBlockConstraint::Auto,
            zero,
        );
        close(&s4.row_sizes, &[0.0, 200.0]);

        // S5: every spanned row is definite, so they all grow together in
        // proportion. Both engines agree here.
        let s5 = matrix_case(
            &[(20.0, true), (30.0, true)],
            Some((0, 2, 200.0)),
            TableBlockConstraint::Auto,
            zero,
        );
        close(&s5.row_sizes, &[80.0, 120.0]);
    }

    #[test]
    fn used_table_block_size_follows_the_measured_interop_matrix() {
        let zero = TableSeparatedBlockMetrics::default();
        // T1: 300px table over minima 20/40, proportional.
        let t1 = matrix_case(&[(20.0, false), (40.0, false)], None, px(300.0), zero);
        close(&t1.row_sizes, &[100.0, 200.0]);
        assert!((t1.used_table_block_size - 300.0).abs() < 0.05);

        // T2: an empty row has no measure, so proportion gives it nothing.
        let t2 = matrix_case(&[(60.0, false), (0.0, false)], None, px(300.0), zero);
        close(&t2.row_sizes, &[300.0, 0.0]);

        // T3: a definite table height is a minimum, never a maximum.
        let t3 = matrix_case(&[(20.0, false), (40.0, false)], None, px(10.0), zero);
        close(&t3.row_sizes, &[20.0, 40.0]);
        assert!((t3.used_table_block_size - 60.0).abs() < 0.05);

        // T5: a definite row keeps its height; the automatic row absorbs.
        let t5 = matrix_case(&[(20.0, true), (40.0, false)], None, px(300.0), zero);
        close(&t5.row_sizes, &[20.0, 280.0]);

        // T6: with no measure anywhere, table height splits equally. This
        // is the branch that differs from the rowspan site.
        let t6 = matrix_case(&[(0.0, false), (0.0, false)], None, px(300.0), zero);
        close(&t6.row_sizes, &[150.0, 150.0]);

        // T7: the one divergence. Chrome 150 distributes proportionally
        // ([120, 180]); Firefox 153 splits the excess equally ([145, 155]).
        // Buckram follows Chrome, keeping one rule for both distribution
        // sites rather than two.
        let t7 = matrix_case(&[(20.0, true), (30.0, true)], None, px(300.0), zero);
        close(&t7.row_sizes, &[120.0, 180.0]);
    }

    #[test]
    fn row_group_height_uses_the_table_distribution_rule_for_its_own_rows() {
        // T4 from the K4d3 matrix: `tbody { height: 200px }` over rows whose
        // content minima are 20px and 40px. Both tested browsers distribute
        // the group's minimum proportionally, and the table grows with it.
        let grid = multi_row_grid(&[&[3], &[4]], &[]);
        let inline = inline_result(&grid, vec![10.0]);
        let group_constraints = [px(200.0)];
        let mut input = block_input(&grid, &inline);
        input.row_group_constraints = &group_constraints;
        let styles = vec![TableCellBlockStyle::default(); grid.cells.len()];
        let outputs = grid
            .cells
            .iter()
            .map(|cell| {
                (
                    cell.source,
                    output(if cell.row == 0 { 20.0 } else { 40.0 }, 0.0),
                )
            })
            .collect::<Vec<_>>();
        let row_constraints = vec![TableBlockConstraint::Auto; grid.rows.len()];
        let measures = measure_single_span_rows(&input, &styles, &outputs, &row_constraints)
            .expect("row measures");

        let sizing = size_table_rows(&input, &measures, &styles, &outputs).expect("row sizing");
        close(&sizing.row_sizes, &[66.67, 133.33]);
        assert!((sizing.used_table_block_size - 200.0).abs() < 0.05);
    }

    #[test]
    fn nonempty_row_group_constraints_must_match_k4b_group_order() {
        let grid = multi_row_grid(&[&[3]], &[]);
        let inline = inline_result(&grid, vec![10.0]);
        let constraints = [TableBlockConstraint::Auto, TableBlockConstraint::Auto];
        let mut input = block_input(&grid, &inline);
        input.row_group_constraints = &constraints;
        let styles = vec![TableCellBlockStyle::default(); grid.cells.len()];
        let outputs = grid
            .cells
            .iter()
            .map(|cell| (cell.source, output(0.0, 0.0)))
            .collect::<Vec<_>>();
        let measures = vec![
            TableRowMeasure {
                row: None,
                min_block_size: 0.0,
                preferred: TableBlockConstraint::Auto,
                constrained: false,
            };
            grid.rows.len()
        ];

        assert_eq!(
            size_table_rows(&input, &measures, &styles, &outputs),
            Err(TableRowLayoutError::RowGroupInputCountMismatch {
                expected: 1,
                actual: 2,
            })
        );
    }

    #[test]
    fn spacing_counts_once_at_the_table_and_inside_a_span() {
        let metrics = TableSeparatedBlockMetrics {
            table_offset_start: 1.0,
            table_offset_end: 2.0,
            block_spacing: 5.0,
        };
        // Two rows of 20 and 40: edges 3 plus three intervals of 5 = 18.
        let sizing = matrix_case(
            &[(20.0, false), (40.0, false)],
            None,
            TableBlockConstraint::Auto,
            metrics,
        );
        close(&sizing.row_sizes, &[20.0, 40.0]);
        assert!((sizing.used_table_block_size - 78.0).abs() < 0.05);
        // Offsets start after the table edge and its first interval.
        close(&sizing.row_offsets, &[6.0, 31.0]);

        // The interval a span crosses counts toward the cell's range, so the
        // rows only have to supply 200 - 5.
        let spanned = matrix_case(
            &[(20.0, false), (40.0, false)],
            Some((0, 2, 200.0)),
            TableBlockConstraint::Auto,
            metrics,
        );
        assert!((spanned.row_sizes.iter().sum::<f32>() - 195.0).abs() < 0.05);
    }

    #[test]
    fn collapsed_outer_winners_replace_block_spacing_without_a_second_row_algorithm() {
        let grid = multi_row_grid(&[&[3], &[4]], &[]);
        let inline = inline_result(&grid, vec![40.0]);
        let mut input = block_input(&grid, &inline);
        input.border_metrics = TableBlockBorderMetrics::Collapsed(TableCollapsedBlockMetrics {
            outer_start: 1.5,
            outer_end: 2.5,
            ..TableCollapsedBlockMetrics::default()
        });
        let styles = vec![TableCellBlockStyle::default(); grid.cells.len()];
        let outputs = grid
            .cells
            .iter()
            .zip([output(20.0, 0.0), output(40.0, 0.0)])
            .map(|(cell, output)| (cell.source, output))
            .collect::<Vec<_>>();
        let rows = vec![TableBlockConstraint::Auto; grid.rows.len()];
        let measures = measure_single_span_rows(&input, &styles, &outputs, &rows)
            .expect("collapsed row measures");
        let sizing =
            size_table_rows(&input, &measures, &styles, &outputs).expect("collapsed row sizing");

        close(&sizing.row_sizes, &[20.0, 40.0]);
        close(&sizing.row_offsets, &[1.5, 21.5]);
        assert!((sizing.used_table_block_size - 64.0).abs() < 0.05);
    }

    /// The K4d3 monotonicity property: raising one spanning cell's
    /// requirement never shrinks a row inside its span.
    #[test]
    fn raising_a_spanning_requirement_never_shrinks_a_spanned_row() {
        let zero = TableSeparatedBlockMetrics::default();
        let mut previous = vec![0.0, 0.0];
        for required in [0.0_f32, 50.0, 120.0, 200.0, 480.0] {
            let sizing = matrix_case(
                &[(20.0, false), (40.0, false)],
                Some((0, 2, required)),
                TableBlockConstraint::Auto,
                zero,
            );
            for (now, before) in sizing.row_sizes.iter().zip(&previous) {
                assert!(
                    *now >= before - 0.05,
                    "required {required}: {:?} shrank below {previous:?}",
                    sizing.row_sizes
                );
            }
            previous = sizing.row_sizes;
        }
    }

    #[test]
    fn collapsed_block_metrics_defer_before_any_row_sizing() {
        let grid = multi_row_grid(&[&[3]], &[]);
        let inline = inline_result(&grid, vec![10.0]);
        let input = TableBlockSizingInput {
            grid: &grid,
            inline: &inline,
            table_constraint: TableBlockConstraint::Auto,
            table_box_sizing: TableBoxSizing::BorderBox,
            row_group_constraints: &[],
            border_metrics: TableBlockBorderMetrics::CollapsedPendingK4g,
            available_block_size: None,
            track_visibility: TableTrackVisibility::all_visible(&grid),
        };
        let measures = vec![TableRowMeasure {
            row: grid.rows[0].source,
            min_block_size: 10.0,
            preferred: TableBlockConstraint::Auto,
            constrained: false,
        }];
        let paired = vec![(grid.cells[0].source, output(10.0, 0.0))];
        assert_eq!(
            size_table_rows(
                &input,
                &measures,
                &[TableCellBlockStyle::default()],
                &paired
            ),
            Err(TableRowLayoutError::Deferral(
                TableBlockDeferral::CollapsedBlockBorderMetricsPendingK4g
            ))
        );
    }

    /// A formatter that resolves a percentage child against whatever basis
    /// the pass supplies, and counts how many times each cell was formatted.
    struct PercentageFormatter {
        /// Fraction of the cell's content block size each cell's child wants.
        child: Vec<Option<f32>>,
        seen: Vec<TableCellLayoutPass>,
    }

    impl TableCellFormatter for PercentageFormatter {
        fn format_cell(
            &mut self,
            input: TableCellLayoutInput,
        ) -> Result<TableCellLayoutOutput, TableRowLayoutError> {
            self.seen.push(input.pass);
            let cell = (self.seen.len() - 1) % self.child.len();
            let fraction = self.child[cell];
            let content = match (fraction, input.percentage_basis) {
                // A percentage child with no basis is zero, not automatic.
                (Some(_), None) => 0.0,
                (Some(fraction), Some(basis)) => basis * fraction,
                (None, _) => 0.0,
            };
            Ok(TableCellLayoutOutput {
                content_block_size: content,
                border_box_min_block_size: 0.0,
                baselines: Baselines::synthesized_from_block_end(content),
                overflow: LogicalRect::default(),
                fragments: FragmentDraftTree::default(),
            })
        }
    }

    fn percentage_case(
        row_constraints: Vec<TableBlockConstraint>,
        cell_specified: Vec<TableBlockConstraint>,
        dependent: Vec<bool>,
        table: TableBlockConstraint,
        first_pass_content: Vec<f32>,
    ) -> (TablePercentagePass, Vec<TableCellLayoutOutput>) {
        let rows_spec = (0..row_constraints.len())
            .map(|r| vec![3u8 + r as u8])
            .collect::<Vec<_>>();
        let refs = rows_spec.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let grid = multi_row_grid(&refs, &[]);
        let styles = cell_specified
            .iter()
            .zip(&dependent)
            .map(|(specified, dependent)| TableCellBlockStyle {
                specified: *specified,
                percentage_dependent_contents: *dependent,
                ..TableCellBlockStyle::default()
            })
            .collect::<Vec<_>>();
        let inline = inline_result(&grid, vec![10.0]);
        let input = TableBlockSizingInput {
            grid: &grid,
            inline: &inline,
            table_constraint: table,
            table_box_sizing: TableBoxSizing::BorderBox,
            row_group_constraints: &[],
            border_metrics: TableBlockBorderMetrics::Separated(
                TableSeparatedBlockMetrics::default(),
            ),
            available_block_size: None,
            track_visibility: TableTrackVisibility::all_visible(&grid),
        };
        let mut outputs = grid
            .cells
            .iter()
            .zip(&first_pass_content)
            .map(|(cell, content)| (cell.source, output(*content, 0.0)))
            .collect::<Vec<_>>();
        let measures = measure_single_span_rows(&input, &styles, &outputs, &row_constraints)
            .expect("first-pass measures");
        let first = size_table_rows(&input, &measures, &styles, &outputs).expect("first sizing");
        let mut formatter = PercentageFormatter {
            child: dependent
                .iter()
                .map(|d| if *d { Some(0.5) } else { None })
                .collect(),
            seen: Vec::new(),
        };
        let pass = resolve_percentage_block_sizes(
            &input,
            &first,
            &measures,
            &styles,
            &mut outputs,
            &row_constraints,
            &mut formatter,
        )
        .expect("percentage pass");
        let finals = outputs.into_iter().map(|(_, output)| output).collect();
        (pass, finals)
    }

    fn pct(fraction: f32) -> TableBlockConstraint {
        TableBlockConstraint::Value(
            AffineLengthPercentage::new(0.0, fraction).expect("finite percentage"),
        )
    }

    /// The K4d4 interop matrix, measured in Chrome 150 and Firefox 153 on
    /// 2026-08-01. A percentage row or cell height resolves only against a
    /// specified definite table height, never against the used height the
    /// content just produced.
    #[test]
    fn percentage_rows_and_cells_follow_the_measured_interop_matrix() {
        // P1: percentage row, indefinite table. Both engines treat it as
        // automatic and the table stays at its content height.
        let (p1, _) = percentage_case(
            vec![pct(0.5), TableBlockConstraint::Auto],
            vec![TableBlockConstraint::Auto; 2],
            vec![false; 2],
            TableBlockConstraint::Auto,
            vec![20.0, 40.0],
        );
        close(&p1.sizing.row_sizes, &[20.0, 40.0]);
        assert!((p1.sizing.used_table_block_size - 60.0).abs() < 0.05);

        // P2: percentage row against a definite 300px table.
        let (p2, _) = percentage_case(
            vec![pct(0.5), TableBlockConstraint::Auto],
            vec![TableBlockConstraint::Auto; 2],
            vec![false; 2],
            px(300.0),
            vec![20.0, 40.0],
        );
        close(&p2.sizing.row_sizes, &[150.0, 150.0]);

        // P3 and P4: a percentage cell height behaves exactly as a
        // percentage row height at both table heights.
        let (p3, _) = percentage_case(
            vec![TableBlockConstraint::Auto; 2],
            vec![pct(0.5), TableBlockConstraint::Auto],
            vec![false; 2],
            TableBlockConstraint::Auto,
            vec![20.0, 40.0],
        );
        close(&p3.sizing.row_sizes, &[20.0, 40.0]);
        let (p4, _) = percentage_case(
            vec![TableBlockConstraint::Auto; 2],
            vec![pct(0.5), TableBlockConstraint::Auto],
            vec![false; 2],
            px(300.0),
            vec![20.0, 40.0],
        );
        close(&p4.sizing.row_sizes, &[150.0, 150.0]);
    }

    #[test]
    fn percentage_cell_contents_gain_a_basis_only_from_the_final_cell_size() {
        // P6: a definite 100px cell gives its percentage child 50. The first
        // pass had no basis and produced zero; the second pass replaces it.
        let (p6, outputs) = percentage_case(
            vec![TableBlockConstraint::Auto],
            vec![px(100.0)],
            vec![true],
            TableBlockConstraint::Auto,
            vec![0.0],
        );
        close(&p6.sizing.row_sizes, &[100.0]);
        assert_eq!(p6.relaid_out.len(), 1);
        assert!((outputs[0].content_block_size - 50.0).abs() < 0.05);

        // P7, the one divergence. The cell's own height is automatic, but the
        // 300px table makes its used height definite. Chrome resolves the
        // child against it (150); Firefox leaves it at zero. Buckram follows
        // Chrome, because the second pass exists precisely to supply a basis
        // that appears only after row distribution.
        let (p7, outputs) = percentage_case(
            vec![TableBlockConstraint::Auto],
            vec![TableBlockConstraint::Auto],
            vec![true],
            px(300.0),
            vec![0.0],
        );
        close(&p7.sizing.row_sizes, &[300.0]);
        assert!((outputs[0].content_block_size - 150.0).abs() < 0.05);
    }

    /// P9's shape: a percentage cell holding a percentage child under an
    /// indefinite table. Both engines collapse it to zero. No basis ever
    /// appears, so nothing iterates and no cycle has to be detected.
    #[test]
    fn an_unbased_percentage_chain_collapses_without_iterating() {
        let (p9, outputs) = percentage_case(
            vec![TableBlockConstraint::Auto],
            vec![pct(0.5)],
            vec![true],
            TableBlockConstraint::Auto,
            vec![0.0],
        );
        close(&p9.sizing.row_sizes, &[0.0]);
        assert!((p9.sizing.used_table_block_size).abs() < 0.05);
        assert!((outputs[0].content_block_size).abs() < 0.05);
    }

    /// The plan's pass counter: a cell with no percentage dependency is
    /// never reformatted, and a dependent cell is reformatted exactly once.
    #[test]
    fn independent_cells_stay_single_pass() {
        let (pass, _) = percentage_case(
            vec![TableBlockConstraint::Auto; 3],
            vec![TableBlockConstraint::Auto; 3],
            vec![false, true, false],
            px(300.0),
            vec![10.0, 20.0, 30.0],
        );
        assert_eq!(
            pass.relaid_out.len(),
            1,
            "only the dependent cell may be relaid out: {pass:?}"
        );

        let (none, _) = percentage_case(
            vec![TableBlockConstraint::Auto; 2],
            vec![TableBlockConstraint::Auto; 2],
            vec![false; 2],
            px(300.0),
            vec![10.0, 20.0],
        );
        assert!(none.relaid_out.is_empty());
    }

    fn aligned_output(content: f32, baseline: Option<f32>) -> TableCellLayoutOutput {
        TableCellLayoutOutput {
            content_block_size: content,
            border_box_min_block_size: 0.0,
            baselines: Baselines::new(baseline, baseline)
                .unwrap_or(Baselines::synthesized_from_block_end(content)),
            overflow: LogicalRect::default(),
            fragments: FragmentDraftTree::default(),
        }
    }

    /// Run one alignment case over a single row of cells.
    fn align_case(
        cells: Vec<(TableCellAlignment, f32, Option<f32>)>,
        row_size: Option<f32>,
    ) -> (TableAlignment, TableRowSizing, Vec<f32>) {
        let ids = (0..cells.len()).map(|i| 3u8 + i as u8).collect::<Vec<_>>();
        let grid = multi_row_grid(&[&ids], &[]);
        let styles = cells
            .iter()
            .map(|(alignment, _, _)| TableCellBlockStyle {
                alignment: *alignment,
                ..TableCellBlockStyle::default()
            })
            .collect::<Vec<_>>();
        let outputs = grid
            .cells
            .iter()
            .zip(&cells)
            .map(|(cell, (_, content, baseline))| {
                (cell.source, aligned_output(*content, *baseline))
            })
            .collect::<Vec<_>>();
        let inline = inline_result(&grid, vec![10.0; grid.columns.len()]);
        let input = TableBlockSizingInput {
            grid: &grid,
            inline: &inline,
            table_constraint: row_size.map_or(TableBlockConstraint::Auto, px),
            table_box_sizing: TableBoxSizing::BorderBox,
            row_group_constraints: &[],
            border_metrics: TableBlockBorderMetrics::Separated(
                TableSeparatedBlockMetrics::default(),
            ),
            available_block_size: None,
            track_visibility: TableTrackVisibility::all_visible(&grid),
        };
        let mut measures =
            measure_single_span_rows(&input, &styles, &outputs, &[TableBlockConstraint::Auto])
                .expect("measures");
        apply_baseline_row_minima(&input, &styles, &outputs, &mut measures)
            .expect("baseline minima");
        let sizing = size_table_rows(&input, &measures, &styles, &outputs).expect("sizing");
        let alignment =
            align_table_cells(&input, &sizing, &styles, &outputs, 0.0).expect("alignment");
        let columns = inline.column_sizes.clone();
        (alignment, sizing, columns)
    }

    /// CSS 2.1 section 17.5.3: aligning baselines can make a row taller than
    /// its tallest cell, because the row must hold the deepest cell above the
    /// shared baseline plus the deepest below it.
    #[test]
    fn baseline_alignment_grows_a_row_beyond_its_tallest_cell() {
        // Cell A: 50 tall, baseline at 10, so 40 below it.
        // Cell B: 40 tall, baseline at 30, so 10 below it.
        // The row baseline is 30, so it needs 30 above and 40 below: 70,
        // though the tallest cell is only 50.
        let (alignment, sizing, _) = align_case(
            vec![
                (TableCellAlignment::Baseline, 50.0, Some(10.0)),
                (TableCellAlignment::Baseline, 40.0, Some(30.0)),
            ],
            None,
        );
        assert!((sizing.row_sizes[0] - 70.0).abs() < 0.05, "{sizing:?}");
        assert!((alignment.rows[0].baseline - 30.0).abs() < 0.05);
        assert!(alignment.rows[0].from_aligned_cell);
        // A is pushed down 20 so its baseline reaches the row's; B is already
        // there.
        assert!((alignment.cells[0].content_block_offset - 20.0).abs() < 0.05);
        assert!((alignment.cells[1].content_block_offset).abs() < 0.05);
    }

    #[test]
    fn every_table_cell_alignment_places_content_in_the_final_row() {
        // One baseline cell fixes a 60px row; the others are placed in it.
        let (alignment, sizing, _) = align_case(
            vec![
                (TableCellAlignment::Baseline, 60.0, Some(20.0)),
                (TableCellAlignment::Top, 20.0, Some(5.0)),
                (TableCellAlignment::Middle, 20.0, Some(5.0)),
                (TableCellAlignment::Bottom, 20.0, Some(5.0)),
            ],
            None,
        );
        assert!((sizing.row_sizes[0] - 60.0).abs() < 0.05);
        assert!(
            (alignment.cells[1].content_block_offset).abs() < 0.05,
            "top"
        );
        assert!(
            (alignment.cells[2].content_block_offset - 20.0).abs() < 0.05,
            "middle"
        );
        assert!(
            (alignment.cells[3].content_block_offset - 40.0).abs() < 0.05,
            "bottom"
        );
    }

    /// A row whose cells are all non-baseline, or whose cells report no line
    /// baseline at all, synthesizes from the lowest cell content edge.
    #[test]
    fn a_row_without_baseline_cells_synthesizes_from_the_lowest_content_edge() {
        let (alignment, _, _) = align_case(
            vec![
                (TableCellAlignment::Top, 30.0, Some(8.0)),
                (TableCellAlignment::Bottom, 45.0, Some(9.0)),
            ],
            None,
        );
        assert!(!alignment.rows[0].from_aligned_cell);
        assert!((alignment.rows[0].baseline - 45.0).abs() < 0.05);

        // A baseline-aligned cell with no line box does not establish one
        // either, so the row still synthesizes.
        let (empty, _, _) = align_case(vec![(TableCellAlignment::Baseline, 0.0, None)], None);
        assert!(!empty.rows[0].from_aligned_cell);
        assert!((empty.rows[0].baseline).abs() < 0.05);
    }

    /// The table exports its first baseline from its first row and its last
    /// from its last row, as logical offsets from the table's block-start.
    #[test]
    fn the_table_exports_first_and_last_row_baselines() {
        let grid = multi_row_grid(&[&[3], &[4]], &[]);
        let styles = vec![TableCellBlockStyle::default(); 2];
        let outputs = vec![
            (grid.cells[0].source, aligned_output(30.0, Some(12.0))),
            (grid.cells[1].source, aligned_output(50.0, Some(20.0))),
        ];
        let inline = inline_result(&grid, vec![10.0]);
        let metrics = TableSeparatedBlockMetrics {
            table_offset_start: 1.0,
            table_offset_end: 1.0,
            block_spacing: 4.0,
        };
        let input = TableBlockSizingInput {
            grid: &grid,
            inline: &inline,
            table_constraint: TableBlockConstraint::Auto,
            table_box_sizing: TableBoxSizing::BorderBox,
            row_group_constraints: &[],
            border_metrics: TableBlockBorderMetrics::Separated(metrics),
            available_block_size: None,
            track_visibility: TableTrackVisibility::all_visible(&grid),
        };
        let mut measures =
            measure_single_span_rows(&input, &styles, &outputs, &[TableBlockConstraint::Auto; 2])
                .expect("measures");
        apply_baseline_row_minima(&input, &styles, &outputs, &mut measures).expect("minima");
        let sizing = size_table_rows(&input, &measures, &styles, &outputs).expect("sizing");
        let alignment =
            align_table_cells(&input, &sizing, &styles, &outputs, 0.0).expect("alignment");

        // Row 0 starts after the table edge and its first interval.
        assert!((sizing.row_offsets[0] - 5.0).abs() < 0.05, "{sizing:?}");
        assert_eq!(alignment.baselines.first, Some(5.0 + 12.0));
        assert_eq!(alignment.baselines.last, Some(sizing.row_offsets[1] + 20.0));
    }

    /// A spanning cell's baseline belongs to the row it starts in, and never
    /// participates in a later row's baseline.
    #[test]
    fn a_spanning_cell_only_joins_its_starting_row_baseline() {
        let grid = multi_row_grid(&[&[3, 200], &[4]], &[(200u8, 2)]);
        let styles = vec![TableCellBlockStyle::default(); grid.cells.len()];
        let outputs = grid
            .cells
            .iter()
            .map(|cell| {
                let output = if cell.row_span > 1 {
                    aligned_output(80.0, Some(70.0))
                } else {
                    aligned_output(20.0, Some(8.0))
                };
                (cell.source, output)
            })
            .collect::<Vec<_>>();
        let inline = inline_result(&grid, vec![10.0; grid.columns.len()]);
        let input = TableBlockSizingInput {
            grid: &grid,
            inline: &inline,
            table_constraint: TableBlockConstraint::Auto,
            table_box_sizing: TableBoxSizing::BorderBox,
            row_group_constraints: &[],
            border_metrics: TableBlockBorderMetrics::Separated(
                TableSeparatedBlockMetrics::default(),
            ),
            available_block_size: None,
            track_visibility: TableTrackVisibility::all_visible(&grid),
        };
        let mut measures =
            measure_single_span_rows(&input, &styles, &outputs, &[TableBlockConstraint::Auto; 2])
                .expect("measures");
        apply_baseline_row_minima(&input, &styles, &outputs, &mut measures).expect("minima");
        let sizing = size_table_rows(&input, &measures, &styles, &outputs).expect("sizing");
        let alignment =
            align_table_cells(&input, &sizing, &styles, &outputs, 0.0).expect("alignment");
        // The 70px spanner baseline never reaches either row's baseline: row
        // 0 keeps its own 8px cell and row 1 keeps its own.
        assert!(
            (alignment.rows[0].baseline - 8.0).abs() < 0.05,
            "{alignment:?}"
        );
        assert!((alignment.rows[1].baseline - 8.0).abs() < 0.05);
    }

    /// Alignment reads K4c's accepted columns and never writes to them.
    #[test]
    fn alignment_does_not_alter_column_sizes() {
        let (alignment, _, columns) = align_case(
            vec![
                (TableCellAlignment::Baseline, 50.0, Some(10.0)),
                (TableCellAlignment::Bottom, 40.0, Some(30.0)),
            ],
            None,
        );
        assert_eq!(columns, vec![10.0, 10.0]);
        // Every cell rectangle covers exactly its own column.
        for cell in &alignment.cells {
            assert!((cell.rect.inline_size - 10.0).abs() < 0.05);
        }
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
