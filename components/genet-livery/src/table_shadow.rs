//! K4c5b: Buckram owns live table inline sizing.
//!
//! Every flattenable table is noted at box-build time. Before the main
//! compute, [`buckram_table_columns`] lowers the table once, runs Buckram's
//! fixed or automatic algorithm, and the caller pins the resulting columns
//! as explicit grid tracks. The temporary Grid/Flex bridge is a placement
//! consumer only: it may place cells, it may not infer a track.
//!
//! A table Buckram cannot size yet defers under a named skip and falls back
//! to Taffy inference, counted, never silent. After fragment collection,
//! [`verify_assigned_columns`] asserts the painted single-span cell widths
//! match what Buckram assigned; a divergence there means the bridge failed
//! to honor an explicit track, which is an invariant violation rather than
//! a sizing disagreement.

use std::hash::Hash;

use buckram::{
    AlgorithmNodeId, BoxId, CaptionMinContribution, IntrinsicSizes,
    TableAutomaticColumnMeasureInput, TableAutomaticInlineSizingIndefinite,
    TableAutomaticInlineSizingInput, TableAutomaticInlineSizingOutcome, TableBlockLayout,
    TableCellInlineMeasure, TableDeferral, TableFixedInlineSizingInput,
    TableFixedInlineSizingOutcome, TableGrid, TableInlineBorderMetrics, TableInlineSizingError,
    TableInlineSizingResult, TableSeparatedBorderMetrics, TableTrackVisibility,
    measure_automatic_columns, size_automatic_table_inline, size_fixed_table_inline,
};
use layout_dom_api::LayoutDom;
use livery::{
    ComputedValues,
    values::{BorderCollapse, Display as CssDisplay, TableLayout as CssTableLayout},
};

use crate::{
    StylePlane,
    box_tree::GeneratedBoxTree,
    table_block::TableBlockLedger,
    table_sizing::{
        automatic_table_track_inputs, fixed_table_track_inputs, table_cell_inline_style,
        table_inline_constraints,
    },
};

/// The quantity a verification disagreed on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableSizingQuantity {
    ColumnCount,
    /// A painted single-span cell's width differs from the Buckram column it
    /// was pinned to: the bridge failed to honor an explicit track.
    ColumnSize(usize),
}

/// One disagreement between Buckram's assigned columns and the painted
/// fragments, attributable to a table box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableSizingDivergence {
    pub table: BoxId,
    pub quantity: TableSizingQuantity,
    pub buckram: f32,
    pub livery: f32,
}

/// Why a table received no Buckram columns and fell back to Taffy inference.
#[derive(Clone, Debug, PartialEq)]
pub enum TableShadowSkip {
    /// A named K4 gap. Never a silent fallback.
    Deferred(TableDeferral),
    /// A grid cell built no algorithm node, so its intrinsic pair cannot be
    /// measured.
    AutomaticIncompleteCells,
    /// Buckram declined a used size for an explicitly named missing basis.
    AutomaticIndefinite(TableAutomaticInlineSizingIndefinite),
    Error(TableInlineSizingError),
}

/// Deferral counters and the honoring-verification record for one layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableShadowLedger {
    /// Tables whose columns Buckram assigned.
    pub assigned: usize,
    /// Assigned tables whose painted fragments were verified.
    pub verified: usize,
    /// Verified tables whose fragments matched the assignment.
    pub honored: usize,
    pub divergences: Vec<TableSizingDivergence>,
    pub skipped: Vec<(BoxId, TableShadowSkip)>,
    /// K4d6b's block-axis counters. Nested rather than parallel: a table's
    /// two axes are one dispatch decision, and two ledgers threaded through
    /// three routes would drift apart.
    pub block: TableBlockLedger,
}

impl TableShadowLedger {
    pub fn is_silent(&self) -> bool {
        self.divergences.is_empty()
    }

    /// Fold another ledger in. Atomic subtrees each build under their own
    /// `BuildState`, and dropping their ledgers would leave tables reached
    /// through the text path unaccounted.
    pub fn merge(&mut self, other: Self) {
        self.assigned += other.assigned;
        self.verified += other.verified;
        self.honored += other.honored;
        self.divergences.extend(other.divergences);
        self.skipped.extend(other.skipped);
        self.block.merge(other.block);
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

/// Tolerance for verifying assigned columns against painted fragments.
/// Fragments are pixel-rounded cumulatively (three 28.9px tracks paint as
/// 29, 29, 28), while Buckram's output is unrounded arithmetic, so 1px is
/// the honest unit of agreement.
const FRAGMENT_TOLERANCE: f32 = 1.0;

/// The live path resolves `rem` against a hardcoded 16px rather than the root
/// element's computed font size, in `length_percentage_px` and
/// `border_width_px`. The lowering matches that constant so Buckram's columns
/// and the rest of live layout resolve font-relative units identically.
/// Fixing the live assumption is its own change.
pub(crate) const LIVE_ROOT_FONT_SIZE: f32 = 16.0;

/// A flattenable table noted at box-build time. Its columns are computed
/// before the main compute and verified against fragments after collection.
pub struct PendingTable<Id> {
    pub table: BoxId,
    pub node: Id,
    pub taffy_table: AlgorithmNodeId,
    /// The table wrapper box's node, once K4e1's wrapper has been built. It is
    /// built after the grid, so it registers itself here rather than arriving
    /// with the rest.
    pub wrapper: Option<AlgorithmNodeId>,
    pub grid: TableGrid,
    /// One entry per K4b grid cell, in topology order.
    pub cell_nodes: Vec<Option<AlgorithmNodeId>>,
    pub font_size: f32,
    pub containing_width: Option<f32>,
    pub containing_height: Option<f32>,
    /// The inline result Buckram assigned, once `buckram_table_columns` has
    /// run. The whole result is retained, not just its columns: K4d's block
    /// pipeline takes it as its inline input, and re-deriving the grid width
    /// or the undistributable remainder from the column vector alone would
    /// reintroduce exactly the arithmetic K4c owns.
    pub assigned: Option<TableInlineSizingResult>,
    /// The block-axis result, once the K4d pipeline has run.
    pub block: Option<TableBlockLayout>,
}

/// Compute Buckram's authoritative inline result for one live table.
///
/// `cell_border_box_intrinsics` are min/max-content border-box widths
/// measured through the live intrinsic machinery, per K4b cell; the cell's
/// lowered offsets convert them to the content pairs Buckram's contract
/// expects. They are consulted by the automatic algorithm, including the
/// CSS 2.1 fallback of a fixed-layout table whose width is not definite.
/// `None` means Buckram declined; the reason is recorded in the ledger.
#[expect(clippy::too_many_arguments, reason = "one call site per route")]
pub(crate) fn buckram_table_columns<D>(
    dom: &D,
    boxes: &GeneratedBoxTree<D::NodeId>,
    styles: &StylePlane<D::NodeId>,
    grid: &TableGrid,
    table: BoxId,
    table_node: D::NodeId,
    computed: &ComputedValues,
    font_size: f32,
    containing_width: Option<f32>,
    cell_border_box_intrinsics: &[Option<IntrinsicSizes>],
    ledger: &mut TableShadowLedger,
) -> Option<TableInlineSizingResult>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if computed.table_layout == CssTableLayout::Fixed {
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
                return None;
            },
        };
        match size_fixed_table_inline(&input) {
            Ok(TableFixedInlineSizingOutcome::Fixed(result)) => {
                ledger.assigned += 1;
                return Some(result);
            },
            // CSS 2.1 17.5.2.1: fixed layout with an indefinite width uses
            // the automatic algorithm. Fall through.
            Ok(TableFixedInlineSizingOutcome::Automatic(_)) => {},
            Err(error) => {
                ledger.skip(table, classify(error));
                return None;
            },
        }
    }
    automatic_columns(
        dom,
        boxes,
        styles,
        grid,
        table,
        table_node,
        computed,
        font_size,
        containing_width,
        cell_border_box_intrinsics,
        ledger,
    )
}

#[expect(clippy::too_many_arguments, reason = "one call site")]
fn automatic_columns<D>(
    dom: &D,
    boxes: &GeneratedBoxTree<D::NodeId>,
    styles: &StylePlane<D::NodeId>,
    grid: &TableGrid,
    table: BoxId,
    table_node: D::NodeId,
    computed: &ComputedValues,
    font_size: f32,
    containing_width: Option<f32>,
    cell_border_box_intrinsics: &[Option<IntrinsicSizes>],
    ledger: &mut TableShadowLedger,
) -> Option<TableInlineSizingResult>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let sizing = match sizing_input(
        dom,
        styles,
        grid,
        table_node,
        computed,
        font_size,
        containing_width,
    ) {
        Ok(sizing) => sizing,
        Err(error) => {
            ledger.skip(table, classify(error));
            return None;
        },
    };
    let (columns, column_groups) = automatic_table_track_inputs(grid, |source| {
        boxes
            .origin_node(source)
            .and_then(|node| styles.get(node))
            .map_or_else(Default::default, |style| {
                table_inline_constraints(style, font_size, LIVE_ROOT_FONT_SIZE)
            })
    });
    let cells =
        match lowered_cells::<D>(boxes, styles, grid, font_size, |index, source, lowered| {
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
            // Live layout measures border boxes; Buckram's contract carries the
            // content pair and adds offsets itself.
            IntrinsicSizes::new(
                (raw.min_content - offsets).max(0.0),
                (raw.max_content - offsets).max(0.0),
            )
            .ok_or(TableInlineSizingError::InvalidResultSize)
        }) {
            Ok(cells) => cells,
            Err(TableInlineSizingError::InvalidResultSize) => {
                ledger.skip(table, TableShadowSkip::AutomaticIncompleteCells);
                return None;
            },
            Err(error) => {
                ledger.skip(table, classify(error));
                return None;
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
            ledger.skip(table, classify(error));
            return None;
        },
    };
    match size_automatic_table_inline(&TableAutomaticInlineSizingInput {
        sizing: input.sizing,
        measures: &measures,
    }) {
        Ok(TableAutomaticInlineSizingOutcome::Sized(result)) => {
            ledger.assigned += 1;
            Some(result)
        },
        Ok(TableAutomaticInlineSizingOutcome::Indefinite(reason)) => {
            ledger.skip(table, TableShadowSkip::AutomaticIndefinite(reason));
            None
        },
        Err(error) => {
            ledger.skip(table, classify(error));
            None
        },
    }
}

/// Assert the painted fragments honored an assigned column vector. `live`
/// holds one painted single-span cell width per K4b column, where a fragment
/// answers for it.
pub(crate) fn verify_assigned_columns(
    table: BoxId,
    assigned: &[f32],
    live: &[Option<f32>],
    ledger: &mut TableShadowLedger,
) {
    if assigned.len() != live.len() {
        ledger.verified += 1;
        ledger.divergences.push(TableSizingDivergence {
            table,
            quantity: TableSizingQuantity::ColumnCount,
            buckram: assigned.len() as f32,
            livery: live.len() as f32,
        });
        return;
    }
    let mut comparable = 0usize;
    let mut honored = true;
    for (index, (one, other)) in assigned.iter().zip(live).enumerate() {
        let Some(other) = other else { continue };
        comparable += 1;
        if (one - other).abs() > FRAGMENT_TOLERANCE {
            honored = false;
            ledger.divergences.push(TableSizingDivergence {
                table,
                quantity: TableSizingQuantity::ColumnSize(index),
                buckram: *one,
                livery: *other,
            });
        }
    }
    if comparable == 0 {
        return;
    }
    ledger.verified += 1;
    if honored {
        ledger.honored += 1;
    }
}

fn classify(error: TableInlineSizingError) -> TableShadowSkip {
    match error {
        TableInlineSizingError::Deferral(deferral) => TableShadowSkip::Deferred(deferral),
        other => TableShadowSkip::Error(other),
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
