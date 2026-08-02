//! K4d6b: Buckram lays out a live table's block axis.
//!
//! K4c5b made Buckram authoritative for live table columns. This module
//! lowers the block axis the same way: the table's own block metrics, every
//! cell's block style, and every row's constraint travel into
//! [`buckram::layout_table_block`], which owns the phase order. The result is
//! a complete set of cell rectangles and the emitted fragment subtree.
//!
//! A table Buckram cannot lay out defers under a named gap and is counted,
//! never silent. The ledger separates the two failure shapes that matter: a
//! deferral is a gap the plan already names, while a divergence is Buckram
//! and the live tree disagreeing about geometry.

use std::hash::Hash;

use buckram::{
    BoxId, CellBlockOffsets, FlowAxes, TableBlockBorderMetrics, TableBlockConstraint,
    TableBlockDeferral, TableBlockLayout, TableBlockSizingInput, TableCellBlockStyle,
    TableCellFormatter, TableCellLayoutInput, TableCellLayoutOutput, TableGrid,
    TableInlineSizingError, TableInlineSizingResult, TableRowLayoutError,
    TableSeparatedBlockMetrics, TableTrackVisibility, layout_table_block,
};
use livery::{
    ComputedValues,
    values::{BorderCollapse, Size},
};

use crate::{
    StylePlane,
    box_tree::GeneratedBoxTree,
    table_shadow::LIVE_ROOT_FONT_SIZE,
    table_sizing::{block_size_constraint, table_cell_block_style, table_cell_inline_style},
};

/// Why a table received no Buckram block layout.
#[derive(Clone, Debug, PartialEq)]
pub enum TableBlockSkip {
    /// A named K4 gap in the block axis.
    Deferred(TableBlockDeferral),
    /// A named K4 gap reached while lowering a style.
    DeferredInLowering(TableInlineSizingError),
    /// A grid cell built no algorithm node, so it cannot be formatted.
    IncompleteCells,
    /// A cell or row carries a block-axis constraint Buckram has no contract
    /// for yet. Dropping it silently would change the table.
    UnmodeledConstraint(TableBlockProperty),
    Error(TableRowLayoutError),
}

/// A block-axis CSS property with no Buckram contract yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableBlockProperty {
    CellMaxBlockSize,
    RowMaxBlockSize,
}

/// The block-axis quantity a verification disagreed on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableBlockQuantity {
    CellBlockStart,
    CellBlockSize,
}

/// One disagreement between Buckram's block layout and the painted fragments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableBlockDivergence {
    pub table: BoxId,
    pub cell: BoxId,
    pub quantity: TableBlockQuantity,
    pub buckram: f32,
    pub livery: f32,
}

/// Counters for one layout's block-axis dispatch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableBlockLedger {
    /// Tables whose block axis Buckram laid out.
    pub laid_out: usize,
    /// Cells the percentage pass relaid out, across every laid-out table.
    pub relaid_out: usize,
    /// Laid-out tables compared against their painted fragments.
    pub verified: usize,
    /// Verified tables whose painted cells matched Buckram's rectangles.
    pub agreed: usize,
    pub divergences: Vec<TableBlockDivergence>,
    pub skipped: Vec<(BoxId, TableBlockSkip)>,
}

impl TableBlockLedger {
    /// Fold another ledger in. Atomic subtrees each build under their own
    /// state, and dropping their ledgers would leave tables reached through
    /// the text path unaccounted.
    pub fn merge(&mut self, other: Self) {
        self.laid_out += other.laid_out;
        self.relaid_out += other.relaid_out;
        self.verified += other.verified;
        self.agreed += other.agreed;
        self.divergences.extend(other.divergences);
        self.skipped.extend(other.skipped);
    }

    fn skip(&mut self, table: BoxId, reason: TableBlockSkip) {
        self.skipped.push((table, reason));
    }

    /// Counts per named K4 gap, so a deferral can never be read as support.
    pub fn deferral_count(&self, deferral: TableBlockDeferral) -> usize {
        self.skipped
            .iter()
            .filter(|(_, skip)| matches!(skip, TableBlockSkip::Deferred(one) if *one == deferral))
            .count()
    }
}

/// One cell's block-axis facts, in K4b cell order.
pub(crate) struct CellBlockInput {
    pub style: TableCellBlockStyle,
    /// The cell's resolved inline offsets, which K4c's accepted result makes
    /// definite. `format_table_cells` subtracts them from the spanned columns.
    pub inline_offsets: f32,
    /// A `min-height` floor on the cell's border box, kept apart from the
    /// measured content exactly as Buckram's contract separates them.
    pub min_block_size: f32,
}

/// Everything the block pipeline needs that only the caller's tree can
/// supply, keyed by K4b cell index.
pub(crate) struct TableBlockInputs {
    pub cells: Vec<CellBlockInput>,
    pub rows: Vec<TableBlockConstraint>,
    pub table_constraint: TableBlockConstraint,
    pub border_metrics: TableBlockBorderMetrics,
    pub inline_spacing: f32,
}

/// Lower one live table's block axis. Returns `None` with a named skip when
/// any part of the lowering has no contract yet.
pub(crate) fn table_block_inputs<Id>(
    boxes: &GeneratedBoxTree<Id>,
    styles: &StylePlane<Id>,
    grid: &TableGrid,
    table: BoxId,
    computed: &ComputedValues,
    font_size: f32,
    ledger: &mut TableBlockLedger,
) -> Option<TableBlockInputs>
where
    Id: Copy + Eq + Hash,
{
    let axes = FlowAxes::HORIZONTAL_LTR;
    let root = LIVE_ROOT_FONT_SIZE;
    let style_of = |source: BoxId| {
        boxes
            .origin_node(source)
            .and_then(|node| styles.get(node))
            .cloned()
    };

    let border_metrics = match computed.border_collapse {
        BorderCollapse::Collapse => TableBlockBorderMetrics::CollapsedPendingK4g,
        BorderCollapse::Separate => {
            // The table's own block-axis padding and border, lowered through
            // the cell contract because the two boxes take the same edges.
            // Only its offsets are read here.
            let table_style = match table_cell_block_style(computed, axes, font_size, root) {
                Ok(style) => style,
                Err(error) => {
                    ledger.skip(table, TableBlockSkip::DeferredInLowering(error));
                    return None;
                },
            };
            let spacing = computed.border_spacing.vertical;
            TableBlockBorderMetrics::Separated(TableSeparatedBlockMetrics {
                table_offset_start: table_style.offsets.padding_start
                    + table_style.offsets.border_start,
                table_offset_end: table_style.offsets.padding_end + table_style.offsets.border_end,
                block_spacing: spacing.unit.to_px(spacing.value, font_size, root),
            })
        },
    };

    let mut cells = Vec::with_capacity(grid.cells.len());
    for cell in &grid.cells {
        let Some(style) = style_of(cell.source) else {
            ledger.skip(table, TableBlockSkip::IncompleteCells);
            return None;
        };
        if style.max_height != Size::None {
            ledger.skip(
                table,
                TableBlockSkip::UnmodeledConstraint(TableBlockProperty::CellMaxBlockSize),
            );
            return None;
        }
        let lowered = match lower_cell(boxes, styles, &style, cell.source, axes, font_size) {
            Ok(lowered) => lowered,
            Err(error) => {
                ledger.skip(table, TableBlockSkip::DeferredInLowering(error));
                return None;
            },
        };
        cells.push(lowered);
    }

    let mut rows = Vec::with_capacity(grid.rows.len());
    for track in &grid.rows {
        // An implicit row track has no CSS box, so it carries no constraint.
        let Some(style) = track.source.and_then(style_of) else {
            rows.push(TableBlockConstraint::Auto);
            continue;
        };
        if style.max_height != Size::None {
            ledger.skip(
                table,
                TableBlockSkip::UnmodeledConstraint(TableBlockProperty::RowMaxBlockSize),
            );
            return None;
        }
        rows.push(block_size_constraint(style.height, font_size, root));
    }

    let spacing = computed.border_spacing.horizontal;
    Some(TableBlockInputs {
        cells,
        rows,
        table_constraint: block_size_constraint(computed.height, font_size, root),
        border_metrics,
        inline_spacing: spacing.unit.to_px(spacing.value, font_size, root),
    })
}

fn lower_cell<Id>(
    boxes: &GeneratedBoxTree<Id>,
    styles: &StylePlane<Id>,
    computed: &ComputedValues,
    source: BoxId,
    axes: FlowAxes,
    font_size: f32,
) -> Result<CellBlockInput, TableInlineSizingError>
where
    Id: Copy + Eq + Hash,
{
    let root = LIVE_ROOT_FONT_SIZE;
    let mut style = table_cell_block_style(computed, axes, font_size, root)?;
    style.percentage_dependent_contents =
        contents_depend_on_block_size(boxes, styles, source, computed);
    let inline_offsets = table_cell_inline_style(computed, axes, font_size, root)?
        .offsets
        .absolute_total()
        .ok_or(TableInlineSizingError::Deferral(
            buckram::TableDeferral::PercentagePaddingPendingBasis,
        ))?;
    let min_block_size = match block_size_constraint(computed.min_height, font_size, root) {
        TableBlockConstraint::Value(value) if !value.needs_percentage_basis() => {
            value.resolve(0.0).unwrap_or(0.0)
        },
        // A percentage or unreduced minimum has no basis here. Zero is the
        // CSS initial value, so this is the same floor an absent min-height
        // gives, not an invented one.
        _ => 0.0,
    };
    Ok(CellBlockInput {
        style,
        inline_offsets,
        min_block_size,
    })
}

/// Whether any descendant of `cell` carries a block size that gains a basis
/// once the cell's own used block size is known.
///
/// Over-reporting only costs one extra format pass, which the percentage pass
/// bounds; under-reporting silently produces the wrong geometry. So a
/// percentage anywhere in the subtree counts.
fn contents_depend_on_block_size<Id>(
    boxes: &GeneratedBoxTree<Id>,
    styles: &StylePlane<Id>,
    cell: BoxId,
    cell_style: &ComputedValues,
) -> bool
where
    Id: Copy + Eq + Hash,
{
    // A cell whose own block size is indefinite gives its descendants no
    // basis to gain, so nothing inside it can depend on one.
    if cell_style.height == Size::Auto {
        return false;
    }
    let mut stack = boxes[cell].children().to_vec();
    while let Some(box_id) = stack.pop() {
        if let Some(style) = boxes.origin_node(box_id).and_then(|node| styles.get(node))
            && [style.height, style.min_height, style.max_height]
                .into_iter()
                .any(is_percentage_size)
        {
            return true;
        }
        stack.extend_from_slice(boxes[box_id].children());
    }
    false
}

fn is_percentage_size(size: Size) -> bool {
    matches!(
        size,
        Size::Value(
            livery::values::LengthPercentage::Percentage(_)
                | livery::values::LengthPercentage::Calc(_)
        )
    )
}

/// Run Buckram's block pipeline for one live table.
pub(crate) fn buckram_table_block(
    grid: &TableGrid,
    table: BoxId,
    inline: &TableInlineSizingResult,
    inputs: &TableBlockInputs,
    available_block_size: Option<f32>,
    formatter: &mut impl TableCellFormatter,
    ledger: &mut TableBlockLedger,
) -> Option<TableBlockLayout> {
    let cell_styles = inputs
        .cells
        .iter()
        .map(|cell| cell.style)
        .collect::<Vec<_>>();
    let input = TableBlockSizingInput {
        grid,
        inline,
        table_constraint: inputs.table_constraint,
        border_metrics: inputs.border_metrics,
        available_block_size,
        track_visibility: TableTrackVisibility::all_visible(grid),
    };
    match layout_table_block(
        &input,
        &cell_styles,
        &inputs.rows,
        inputs.inline_spacing,
        |index, _| {
            inputs
                .cells
                .get(index)
                .map_or(0.0, |cell| cell.inline_offsets)
        },
        formatter,
    ) {
        Ok(layout) => {
            ledger.laid_out += 1;
            ledger.relaid_out += layout.relaid_out.len();
            Some(layout)
        },
        Err(TableRowLayoutError::Deferral(deferral)) => {
            ledger.skip(table, TableBlockSkip::Deferred(deferral));
            None
        },
        Err(error) => {
            ledger.skip(table, TableBlockSkip::Error(error));
            None
        },
    }
}

/// A [`TableCellFormatter`] whose per-cell work the caller supplies, because
/// only the caller owns the algorithm tree the cell lives in.
pub(crate) struct CellFormatter<F>(pub F);

impl<F> TableCellFormatter for CellFormatter<F>
where
    F: FnMut(TableCellLayoutInput) -> Result<TableCellLayoutOutput, TableRowLayoutError>,
{
    fn format_cell(
        &mut self,
        input: TableCellLayoutInput,
    ) -> Result<TableCellLayoutOutput, TableRowLayoutError> {
        (self.0)(input)
    }
}

/// The content block size a formatted cell reports, from the border box the
/// backend produced and the offsets Buckram will add back itself.
pub(crate) fn cell_content_block_size(border_box: f32, offsets: CellBlockOffsets) -> f32 {
    (border_box - offsets.total().unwrap_or(0.0)).max(0.0)
}

/// Fragments are pixel-rounded cumulatively while Buckram's output is
/// unrounded arithmetic, so 1px is the honest unit of agreement. This matches
/// the inline axis's tolerance for the same reason.
const FRAGMENT_TOLERANCE: f32 = 1.0;

/// Compare Buckram's cell rectangles against the painted fragments.
///
/// `live_cell` answers with a painted cell's block-start and block-size, both
/// already made relative to the table grid's own painted origin: Buckram's
/// rectangles are grid-relative, and comparing against absolute coordinates
/// would report the table's position in the page as a table-layout
/// disagreement.
///
/// While the Grid bridge still places cells, a divergence here is information
/// rather than a failure: it is the measured distance between the two
/// engines, and the set of tables where they already agree is what makes the
/// cutover's movement attributable.
pub(crate) fn verify_table_block(
    table: BoxId,
    layout: &TableBlockLayout,
    live_cell: impl Fn(BoxId) -> Option<(f32, f32)>,
    ledger: &mut TableBlockLedger,
) {
    let mut comparable = 0usize;
    let mut agreed = true;
    for placement in &layout.alignment.cells {
        let Some((block_start, block_size)) = live_cell(placement.box_id) else {
            continue;
        };
        comparable += 1;
        for (quantity, buckram, livery) in [
            (
                TableBlockQuantity::CellBlockStart,
                placement.rect.block_start,
                block_start,
            ),
            (
                TableBlockQuantity::CellBlockSize,
                placement.rect.block_size,
                block_size,
            ),
        ] {
            if (buckram - livery).abs() > FRAGMENT_TOLERANCE {
                agreed = false;
                ledger.divergences.push(TableBlockDivergence {
                    table,
                    cell: placement.box_id,
                    quantity,
                    buckram,
                    livery,
                });
            }
        }
    }
    if comparable == 0 {
        return;
    }
    ledger.verified += 1;
    if agreed {
        ledger.agreed += 1;
    }
}
