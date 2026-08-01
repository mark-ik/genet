//! K4c5a shadow comparison for live table inline sizing.
//!
//! Buckram's fixed algorithm runs beside Livery's live `fixed_column_widths`
//! and every disagreement is recorded against the table's box identity.
//! Automatic tables are noted at box-build time and processed after fragment
//! collection, when the algorithm tree can answer intrinsic queries as a
//! scratch structure: Buckram's automatic algorithm runs on measured cell
//! intrinsics and its columns are compared against the Taffy-inferred track
//! widths read back from the painted fragments. Nothing here changes a
//! painted result. K4c5b makes Buckram authoritative, and may only do so once
//! this ledger is silent or every divergence carries an accepted rule.
//!
//! On the production inline route, automatic tables are counted, not
//! compared: that tree offers no post-collection intrinsic seam yet.

use std::hash::Hash;

use buckram::{
    AlgorithmNodeId, BoxId, CaptionMinContribution, IntrinsicSizes,
    TableAutomaticColumnMeasureInput, TableAutomaticInlineSizingIndefinite,
    TableAutomaticInlineSizingInput, TableAutomaticInlineSizingOutcome, TableCellInlineMeasure,
    TableDeferral, TableFixedInlineSizingInput, TableFixedInlineSizingOutcome, TableGrid,
    TableInlineBorderMetrics, TableInlineSizingError, TableSeparatedBorderMetrics,
    TableTrackVisibility, measure_automatic_columns, size_automatic_table_inline,
    size_fixed_table_inline,
};
use layout_dom_api::LayoutDom;
use livery::{
    ComputedValues,
    values::{BorderCollapse, Display as CssDisplay, TableLayout as CssTableLayout},
};

use crate::{
    StylePlane,
    box_tree::GeneratedBoxTree,
    table_sizing::{
        automatic_table_track_inputs, fixed_table_track_inputs, table_cell_inline_style,
        table_inline_constraints,
    },
};

/// The quantity a shadow comparison disagreed on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableSizingQuantity {
    ColumnCount,
    ColumnSize(usize),
    /// An automatic table's live side is a Taffy-inferred track, not an
    /// algorithm. Divergence here measures how far grid inference sits from
    /// the CSS 2.1 automatic algorithm, which is exactly the movement K4c5b
    /// will cause when Buckram becomes authoritative.
    InferredColumnSize(usize),
}

/// One disagreement between Buckram's result and the live path, attributable to
/// a table box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableSizingDivergence {
    pub table: BoxId,
    pub quantity: TableSizingQuantity,
    pub buckram: f32,
    pub livery: f32,
}

/// Why a table produced no comparable Buckram result.
#[derive(Clone, Debug, PartialEq)]
pub enum TableShadowSkip {
    /// A named K4 gap. Never a silent fallback.
    Deferred(TableDeferral),
    /// Fixed layout declined its own arithmetic, per CSS 2.1 17.5.2.1.
    HandedToAutomatic,
    /// The live path produced no column vector to compare against.
    NoLiveResult,
    /// An automatic table on the production inline route, which has neither
    /// live columns nor a way to measure cell intrinsics.
    AutomaticOnInlineRoute,
    /// A grid cell built no algorithm node, so its intrinsic pair cannot be
    /// measured.
    AutomaticIncompleteCells,
    /// Buckram declined a used size for an explicitly named missing basis.
    AutomaticIndefinite(TableAutomaticInlineSizingIndefinite),
    Error(TableInlineSizingError),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableShadowLedger {
    pub compared: usize,
    pub agreed: usize,
    pub divergences: Vec<TableSizingDivergence>,
    pub skipped: Vec<(BoxId, TableShadowSkip)>,
}

impl TableShadowLedger {
    pub fn is_silent(&self) -> bool {
        self.divergences.is_empty()
    }

    /// Fold another ledger in. Atomic subtrees each build under their own
    /// `BuildState`, and dropping their ledgers would report tables reached
    /// through the text path as silently unshadowed.
    pub fn merge(&mut self, other: Self) {
        self.compared += other.compared;
        self.agreed += other.agreed;
        self.divergences.extend(other.divergences);
        self.skipped.extend(other.skipped);
    }

    fn skip(&mut self, table: BoxId, reason: TableShadowSkip) {
        self.skipped.push((table, reason));
    }

    fn deferrals(&self) -> impl Iterator<Item = (BoxId, TableDeferral)> + '_ {
        self.skipped.iter().filter_map(|(table, skip)| match skip {
            TableShadowSkip::Deferred(deferral) => Some((*table, *deferral)),
            _ => None,
        })
    }

    /// Counts per named K4 gap, so a deferral can never be read as support.
    pub fn deferral_count(&self, deferral: TableDeferral) -> usize {
        self.deferrals().filter(|(_, one)| *one == deferral).count()
    }
}

/// Subpixel tolerance. K4c reconciles column sums against the table width at
/// this scale, so a smaller difference is not a real disagreement.
const TOLERANCE: f32 = 0.01;

/// Tolerance for comparisons whose live side is a painted fragment. Fragments
/// are pixel-rounded cumulatively (three 28.9px tracks paint as 29, 29, 28),
/// while Buckram's output is unrounded arithmetic, so 1px is the honest unit
/// of agreement there. Real divergences are far beyond it.
const INFERRED_TOLERANCE: f32 = 1.0;

/// The live path resolves `rem` against a hardcoded 16px rather than the root
/// element's computed font size, in `length_percentage_px` and
/// `border_width_px`. The shadow must use the same value or every `rem` table
/// would report a divergence that is an artifact of this comparison rather
/// than a disagreement about sizing. Fixing the live assumption is its own
/// change; K4c5b should not inherit it silently.
const LIVE_ROOT_FONT_SIZE: f32 = 16.0;

/// Run Buckram's fixed algorithm beside the live result and record any
/// disagreement. `live_columns` is exactly what `fixed_column_widths` returned.
#[expect(clippy::too_many_arguments, reason = "one shadow call site, no state")]
pub(crate) fn shadow_fixed_table<D>(
    dom: &D,
    boxes: &GeneratedBoxTree<D::NodeId>,
    styles: &StylePlane<D::NodeId>,
    grid: &TableGrid,
    table: BoxId,
    table_node: D::NodeId,
    computed: &ComputedValues,
    font_size: f32,
    containing_width: Option<f32>,
    live_columns: Option<&[f32]>,
    ledger: &mut TableShadowLedger,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if computed.table_layout != CssTableLayout::Fixed {
        return;
    }
    let Some(live_columns) = live_columns else {
        ledger.skip(table, TableShadowSkip::NoLiveResult);
        return;
    };

    let input = match fixed_input(
        dom,
        boxes,
        styles,
        grid,
        table_node,
        computed,
        font_size,
        containing_width,
    ) {
        Ok(input) => input,
        Err(error) => {
            ledger.skip(table, classify(error));
            return;
        },
    };

    match size_fixed_table_inline(&input) {
        Ok(TableFixedInlineSizingOutcome::Fixed(result)) => {
            compare(table, &result.column_sizes, live_columns, ledger);
        },
        Ok(TableFixedInlineSizingOutcome::Automatic(_)) => {
            ledger.skip(table, TableShadowSkip::HandedToAutomatic);
        },
        Err(error) => ledger.skip(table, classify(error)),
    }
}

fn classify(error: TableInlineSizingError) -> TableShadowSkip {
    match error {
        TableInlineSizingError::Deferral(deferral) => TableShadowSkip::Deferred(deferral),
        other => TableShadowSkip::Error(other),
    }
}

fn compare(table: BoxId, buckram: &[f32], livery: &[f32], ledger: &mut TableShadowLedger) {
    ledger.compared += 1;
    if buckram.len() != livery.len() {
        ledger.divergences.push(TableSizingDivergence {
            table,
            quantity: TableSizingQuantity::ColumnCount,
            buckram: buckram.len() as f32,
            livery: livery.len() as f32,
        });
        return;
    }
    let mut agreed = true;
    for (index, (one, other)) in buckram.iter().zip(livery).enumerate() {
        if (one - other).abs() > TOLERANCE {
            agreed = false;
            ledger.divergences.push(TableSizingDivergence {
                table,
                quantity: TableSizingQuantity::ColumnSize(index),
                buckram: *one,
                livery: *other,
            });
        }
    }
    if agreed {
        ledger.agreed += 1;
    }
}

/// Lower the table box's own geometry into the shared sizing input.
fn sizing_input<'a, D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    grid: &'a TableGrid,
    table_node: D::NodeId,
    computed: &ComputedValues,
    font_size: f32,
    containing_width: Option<f32>,
) -> Result<buckram::TableInlineSizingInput<'a>, TableInlineSizingError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let axes = buckram::FlowAxes::HORIZONTAL_LTR;
    let root_font_size = LIVE_ROOT_FONT_SIZE;
    let border_metrics = match computed.border_collapse {
        BorderCollapse::Collapse => TableInlineBorderMetrics::CollapsedPendingK4g,
        BorderCollapse::Separate => {
            let spacing = computed.border_spacing.horizontal;
            TableInlineBorderMetrics::Separated(TableSeparatedBorderMetrics {
                table_offsets: table_cell_inline_style(computed, axes, font_size, root_font_size)?
                    .offsets,
                inline_spacing: spacing.unit.to_px(spacing.value, font_size, root_font_size),
            })
        },
    };

    // A caption contributes a minimum that only K4e can measure. Deferring is
    // the whole point: inventing zero here would look like support.
    let caption_min = if dom.dom_children(table_node).into_iter().any(|child| {
        styles
            .get(child)
            .is_some_and(|style| style.display == CssDisplay::TableCaption)
    }) {
        CaptionMinContribution::PendingK4e
    } else {
        CaptionMinContribution::NoCaption
    };

    Ok(buckram::TableInlineSizingInput {
        grid,
        available_inline_size: containing_width,
        table_constraints: table_inline_constraints(computed, font_size, root_font_size),
        border_metrics,
        caption_min,
        track_visibility: TableTrackVisibility::all_visible(grid),
    })
}

/// Lower every grid cell, supplying each content pair from `content_for` by
/// K4b cell index.
fn lowered_cells<D>(
    boxes: &GeneratedBoxTree<D::NodeId>,
    styles: &StylePlane<D::NodeId>,
    grid: &TableGrid,
    font_size: f32,
    mut content_for: impl FnMut(
        usize,
        BoxId,
        &buckram::TableCellInlineStyle,
    ) -> Result<IntrinsicSizes, TableInlineSizingError>,
) -> Result<Vec<TableCellInlineMeasure>, TableInlineSizingError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let axes = buckram::FlowAxes::HORIZONTAL_LTR;
    let mut cells = Vec::with_capacity(grid.cells.len());
    for (index, cell) in grid.cells.iter().enumerate() {
        let Some(style) = boxes
            .origin_node(cell.source)
            .and_then(|node| styles.get(node))
            .cloned()
        else {
            return Err(TableInlineSizingError::InvalidOffsets {
                box_id: cell.source,
            });
        };
        let lowered = table_cell_inline_style(&style, axes, font_size, LIVE_ROOT_FONT_SIZE)?;
        cells.push(TableCellInlineMeasure {
            box_id: cell.source,
            content: content_for(index, cell.source, &lowered)?,
            preferred: lowered.constraints.preferred,
            minimum: lowered.constraints.minimum,
            maximum: lowered.constraints.maximum,
            box_sizing: lowered.constraints.box_sizing,
            offsets: lowered.offsets,
        });
    }
    Ok(cells)
}

/// Lower the live table once into Buckram's fixed input.
#[expect(
    clippy::too_many_arguments,
    reason = "lowering takes the whole context"
)]
fn fixed_input<'a, D>(
    dom: &D,
    boxes: &GeneratedBoxTree<D::NodeId>,
    styles: &StylePlane<D::NodeId>,
    grid: &'a TableGrid,
    table_node: D::NodeId,
    computed: &ComputedValues,
    font_size: f32,
    containing_width: Option<f32>,
) -> Result<TableFixedInlineSizingInput<'a>, TableInlineSizingError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let root_font_size = LIVE_ROOT_FONT_SIZE;
    let sizing = sizing_input(
        dom,
        styles,
        grid,
        table_node,
        computed,
        font_size,
        containing_width,
    )?;
    let style_of = |source: BoxId| {
        boxes
            .origin_node(source)
            .and_then(|node| styles.get(node))
            .cloned()
    };
    let (columns, column_groups) = fixed_table_track_inputs(grid, |source| {
        style_of(source).map_or_else(Default::default, |style| {
            table_inline_constraints(&style, font_size, root_font_size)
        })
    });
    // Fixed layout never consults content, by definition of the algorithm.
    // The automatic shadow supplies real measures.
    let cells = lowered_cells::<D>(boxes, styles, grid, font_size, |_, _, _| {
        IntrinsicSizes::new(0.0, 0.0).ok_or(TableInlineSizingError::InvalidResultSize)
    })?;

    Ok(TableFixedInlineSizingInput {
        sizing,
        columns,
        column_groups,
        cells,
    })
}

/// An automatic table noted at box-build time, processed once the algorithm
/// tree can answer intrinsic queries and the fragments carry the live
/// (Taffy-inferred) column widths.
pub struct PendingAutomaticShadow<Id> {
    pub table: BoxId,
    pub node: Id,
    pub grid: TableGrid,
    /// One entry per K4b grid cell, in topology order.
    pub cell_nodes: Vec<Option<AlgorithmNodeId>>,
    pub font_size: f32,
    pub containing_width: Option<f32>,
}

/// Record an automatic table on the production inline route, which has
/// neither live columns nor a way to measure cell intrinsics yet.
pub(crate) fn note_automatic_inline_route(table: BoxId, ledger: &mut TableShadowLedger) {
    ledger.skip(table, TableShadowSkip::AutomaticOnInlineRoute);
}

/// Run Buckram's automatic algorithm for one noted live table.
///
/// `cell_border_box_intrinsics` are min/max-content border-box widths
/// measured through the live intrinsic machinery, per K4b cell. The cell's
/// lowered offsets convert them to the content pairs Buckram's contract
/// expects. `live_columns` are Taffy-inferred track widths derived from
/// single-span cell fragments, `None` where no fragment answers for a column.
#[expect(clippy::too_many_arguments, reason = "one shadow call site, no state")]
pub(crate) fn shadow_automatic_table<D>(
    dom: &D,
    boxes: &GeneratedBoxTree<D::NodeId>,
    styles: &StylePlane<D::NodeId>,
    pending: &PendingAutomaticShadow<D::NodeId>,
    computed: &ComputedValues,
    cell_border_box_intrinsics: &[Option<IntrinsicSizes>],
    live_columns: &[Option<f32>],
    ledger: &mut TableShadowLedger,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let grid = &pending.grid;
    let sizing = match sizing_input(
        dom,
        styles,
        grid,
        pending.node,
        computed,
        pending.font_size,
        pending.containing_width,
    ) {
        Ok(sizing) => sizing,
        Err(error) => {
            ledger.skip(pending.table, classify(error));
            return;
        },
    };
    let (columns, column_groups) = automatic_table_track_inputs(grid, |source| {
        boxes
            .origin_node(source)
            .and_then(|node| styles.get(node))
            .map_or_else(Default::default, |style| {
                table_inline_constraints(style, pending.font_size, LIVE_ROOT_FONT_SIZE)
            })
    });
    let cells = match lowered_cells::<D>(
        boxes,
        styles,
        grid,
        pending.font_size,
        |index, source, lowered| {
            let raw = cell_border_box_intrinsics
                .get(index)
                .copied()
                .flatten()
                .ok_or(TableInlineSizingError::InvalidResultSize)?;
            if lowered.offsets.needs_percentage_basis() {
                return Err(TableInlineSizingError::Deferral(
                    TableDeferral::PercentagePaddingPendingBasis,
                ));
            }
            let offsets = lowered
                .offsets
                .absolute_total()
                .ok_or(TableInlineSizingError::InvalidOffsets { box_id: source })?;
            // Live layout measures border boxes; Buckram's contract carries
            // the content pair and adds offsets itself.
            IntrinsicSizes::new(
                (raw.min_content - offsets).max(0.0),
                (raw.max_content - offsets).max(0.0),
            )
            .ok_or(TableInlineSizingError::InvalidResultSize)
        },
    ) {
        Ok(cells) => cells,
        Err(TableInlineSizingError::InvalidResultSize) => {
            ledger.skip(pending.table, TableShadowSkip::AutomaticIncompleteCells);
            return;
        },
        Err(error) => {
            ledger.skip(pending.table, classify(error));
            return;
        },
    };

    let input = TableAutomaticColumnMeasureInput {
        sizing,
        columns,
        column_groups,
        cells,
    };
    let measures = match measure_automatic_columns(&input) {
        Ok(measures) => measures,
        Err(error) => {
            ledger.skip(pending.table, classify(error));
            return;
        },
    };
    match size_automatic_table_inline(&TableAutomaticInlineSizingInput {
        sizing: input.sizing,
        measures: &measures,
    }) {
        Ok(TableAutomaticInlineSizingOutcome::Sized(result)) => {
            compare_inferred(pending.table, &result.column_sizes, live_columns, ledger);
        },
        Ok(TableAutomaticInlineSizingOutcome::Indefinite(reason)) => {
            ledger.skip(pending.table, TableShadowSkip::AutomaticIndefinite(reason));
        },
        Err(error) => ledger.skip(pending.table, classify(error)),
    }
}

/// Compare Buckram's automatic columns against Taffy-inferred live widths,
/// where a live width exists.
fn compare_inferred(
    table: BoxId,
    buckram: &[f32],
    live: &[Option<f32>],
    ledger: &mut TableShadowLedger,
) {
    if buckram.len() != live.len() {
        ledger.compared += 1;
        ledger.divergences.push(TableSizingDivergence {
            table,
            quantity: TableSizingQuantity::ColumnCount,
            buckram: buckram.len() as f32,
            livery: live.len() as f32,
        });
        return;
    }
    let mut comparable = 0usize;
    let mut agreed = true;
    for (index, (one, other)) in buckram.iter().zip(live).enumerate() {
        let Some(other) = other else { continue };
        comparable += 1;
        if (one - other).abs() > INFERRED_TOLERANCE {
            agreed = false;
            ledger.divergences.push(TableSizingDivergence {
                table,
                quantity: TableSizingQuantity::InferredColumnSize(index),
                buckram: *one,
                livery: *other,
            });
        }
    }
    if comparable == 0 {
        ledger.skip(table, TableShadowSkip::NoLiveResult);
        return;
    }
    ledger.compared += 1;
    if agreed {
        ledger.agreed += 1;
    }
}
