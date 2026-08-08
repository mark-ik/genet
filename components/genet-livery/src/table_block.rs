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
    AlgorithmKind, AlgorithmLayout, AlgorithmNodeId, AlgorithmTree, BoxId, BoxOrigin,
    CellBlockOffsets, FlowAxes, InternalTableRole, TableBlockBorderMetrics, TableBlockConstraint,
    TableBlockDeferral, TableBlockLayout, TableBlockSizingInput, TableBoxSizing,
    TableCellBlockStyle, TableCellFormatter, TableCellLayoutInput, TableCellLayoutOutput,
    TableGrid, TableInlineSizingError, TableInlineSizingResult, TableRowLayoutError,
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
    /// A K4b row-group source could not be lowered to its computed block
    /// constraint, so accepting the table would silently discard geometry.
    IncompleteRowGroup,
    Error(TableRowLayoutError),
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
}

/// Everything the block pipeline needs that only the caller's tree can
/// supply, keyed by K4b cell index.
pub(crate) struct TableBlockInputs {
    pub cells: Vec<CellBlockInput>,
    pub rows: Vec<TableBlockConstraint>,
    /// K4d3's measured group-height constraint, aligned with K4b visual row
    /// groups. A group is still a structural range, never a synthetic row.
    pub row_groups: Vec<TableBlockConstraint>,
    pub table_constraint: TableBlockConstraint,
    pub table_box_sizing: TableBoxSizing,
    pub border_metrics: TableBlockBorderMetrics,
    pub inline_spacing: f32,
    /// K4f's row and column mask, built once during lowering so the two axes
    /// collapse the same tracks.
    pub track_visibility: TableTrackVisibility,
}

/// Lower one live table's block axis. Returns `None` with a named skip when
/// any part of the lowering has no contract yet.
#[expect(clippy::too_many_arguments, reason = "one call site per route")]
pub(crate) fn table_block_inputs<Id>(
    boxes: &GeneratedBoxTree<Id>,
    styles: &StylePlane<Id>,
    grid: &TableGrid,
    table: BoxId,
    computed: &ComputedValues,
    font_size: f32,
    containing_block_size: Option<f32>,
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

    // `min-height` and `max-height` reach neither cells nor rows. CSS 2.1
    // section 10.7 leaves their effect on table cells, rows, and row groups
    // undefined, and the K4d4c matrix measured Chrome 150 and Firefox 153
    // ignoring them outright in all eight cases. Ignoring them is therefore
    // the modeled behavior, not a gap, so nothing defers for them.
    let mut cells = Vec::with_capacity(grid.cells.len());
    for cell in &grid.cells {
        let Some(style) = style_of(cell.source) else {
            ledger.skip(table, TableBlockSkip::IncompleteCells);
            return None;
        };
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
        rows.push(block_size_constraint(style.height, font_size, root));
    }

    let mut row_groups = Vec::with_capacity(grid.row_groups.len());
    for group in &grid.row_groups {
        // K4b records direct rows as one-row visual groups so spans and
        // ordering stay uniform. They have no row-group box of their own:
        // reusing the row's `height` as a group constraint would make that
        // height participate twice. Only an authored row-group element owns
        // the CSS `height` that B3 carries to the row algorithm.
        if !matches!(boxes[group.source].origin, BoxOrigin::Element(_))
            || !matches!(
                boxes[group.source].display.internal_table,
                Some(
                    InternalTableRole::RowGroup
                        | InternalTableRole::HeaderGroup
                        | InternalTableRole::FooterGroup
                )
            )
        {
            row_groups.push(TableBlockConstraint::Auto);
            continue;
        }
        // An authored group must have an author style. Inventing `auto` for a
        // missing style would turn an adapter fault into silent geometry.
        let Some(style) = style_of(group.source) else {
            ledger.skip(table, TableBlockSkip::IncompleteRowGroup);
            return None;
        };
        row_groups.push(block_size_constraint(style.height, font_size, root));
    }

    let spacing = computed.border_spacing.horizontal;
    Some(TableBlockInputs {
        cells,
        rows,
        row_groups,
        table_constraint: resolved_table_block_size(
            block_size_constraint(computed.height, font_size, root),
            containing_block_size,
        ),
        // The UA stylesheet gives a `<table>` element `border-box` and leaves
        // a `display: table` box at `content-box`, so this is read from the
        // computed style rather than assumed.
        table_box_sizing: match computed.box_sizing {
            livery::values::BoxSizing::ContentBox => TableBoxSizing::ContentBox,
            livery::values::BoxSizing::BorderBox => TableBoxSizing::BorderBox,
        },
        border_metrics,
        inline_spacing: spacing.unit.to_px(spacing.value, font_size, root),
        track_visibility: crate::table_shadow::track_visibility(boxes, styles, grid),
    })
}

/// Resolve the table's own percentage block size against its containing
/// block, which is Livery's to know and not Buckram's to guess.
///
/// This is ordinary CSS, unrelated to K4d4's rule for percentage *rows and
/// cells*: those resolve against the table's own specified size, while the
/// table itself resolves against its containing block like any other box.
/// A percentage with no definite basis computes to `auto`.
fn resolved_table_block_size(
    constraint: TableBlockConstraint,
    containing_block_size: Option<f32>,
) -> TableBlockConstraint {
    let TableBlockConstraint::Value(value) = constraint else {
        return constraint;
    };
    if !value.needs_percentage_basis() {
        return constraint;
    }
    match containing_block_size.and_then(|basis| value.resolve(basis)) {
        Some(resolved) if resolved.is_finite() && resolved >= 0.0 => {
            TableBlockConstraint::Value(buckram::AffineLengthPercentage::px(resolved))
        },
        _ => TableBlockConstraint::Auto,
    }
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
    Ok(CellBlockInput {
        style,
        inline_offsets,
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
        table_box_sizing: inputs.table_box_sizing,
        row_group_constraints: &inputs.row_groups,
        border_metrics: inputs.border_metrics,
        available_block_size,
        track_visibility: inputs.track_visibility.clone(),
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

/// Commit one table's Buckram block layout to the algorithm tree.
///
/// Every cell rectangle is written through the owned-context seam, each cell's
/// contents are shifted by the alignment offset the table chose, and the table
/// node is resized and re-dispatched so the backend reports that size without
/// laying the cells out again.
///
/// Rectangles are logical and the live path is horizontal LTR throughout, so
/// inline maps to x and block to y. A vertical writing mode reaches this only
/// once K4f gives the table one, and the flow axes it would need are not
/// invented here.
pub(crate) fn commit_table_block<S, Context, Source>(
    tree: &mut AlgorithmTree<S, Context, Source>,
    table_node: AlgorithmNodeId,
    layout: &TableBlockLayout,
    inline: &TableInlineSizingResult,
    node_of: impl Fn(BoxId) -> Option<AlgorithmNodeId>,
) where
    S: buckram::AlgorithmStyle,
{
    for placement in &layout.alignment.cells {
        let Some(node) = node_of(placement.box_id) else {
            continue;
        };
        tree.set_layout(
            node,
            AlgorithmLayout {
                x: placement.rect.inline_start,
                y: placement.rect.block_start,
                width: placement.rect.inline_size,
                height: placement.rect.block_size,
            },
        );
        // The cell's border box now fills its whole row range, so its
        // contents no longer sit where the formatting pass left them. CSS 2.1
        // section 17.5.3 places them by the cell's alignment, which is a
        // placement offset and never a change to the computed padding.
        if placement.content_block_offset.abs() > f32::EPSILON {
            for child in tree.children(node).to_vec() {
                let mut child_layout = tree.unrounded_layout(child);
                child_layout.y += placement.content_block_offset;
                tree.set_layout(child, child_layout);
            }
        }
    }
    tree.set_layout(
        table_node,
        AlgorithmLayout {
            // The table's own position stays the parent's decision; the
            // backend overwrites it when it places this node.
            x: tree.unrounded_layout(table_node).x,
            y: tree.unrounded_layout(table_node).y,
            width: inline.used_grid_inline_size,
            height: layout.sizing.used_table_block_size,
        },
    );
    // K4d5 is the one authority for a table's first and last baselines. Keep
    // that output on the algorithm node so an inline-table atom can consume
    // the first baseline instead of substituting its block-end edge.
    tree.set_baselines(table_node, layout.alignment.baselines);
    tree.set_kind(table_node, AlgorithmKind::Table);
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
