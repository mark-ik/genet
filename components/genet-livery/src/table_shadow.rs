//! K4c5a shadow comparison for live fixed-table inline sizing.
//!
//! Buckram's fixed algorithm runs beside Livery's live `fixed_column_widths`
//! and every disagreement is recorded against the table's box identity. Nothing
//! here changes a painted result. K4c5b makes Buckram authoritative and deletes
//! the live helper, and it may only do so once this ledger is silent.
//!
//! Automatic tables are counted, not compared. Buckram's automatic algorithm
//! needs a `TableIntrinsicMeasureProvider`, and intrinsic content sizes do not
//! exist at box-build time, so a like-for-like comparison has no partner yet.

use std::hash::Hash;

use buckram::{
    BoxId, CaptionMinContribution, IntrinsicSizes, TableCellInlineMeasure, TableDeferral,
    TableFixedInlineSizingInput, TableFixedInlineSizingOutcome, TableGrid,
    TableInlineBorderMetrics, TableInlineSizingError, TableSeparatedBorderMetrics,
    TableTrackVisibility, size_fixed_table_inline,
};
use layout_dom_api::LayoutDom;
use livery::{
    ComputedValues,
    values::{BorderCollapse, Display as CssDisplay, TableLayout as CssTableLayout},
};

use crate::{
    StylePlane,
    box_tree::GeneratedBoxTree,
    table_sizing::{fixed_table_track_inputs, table_cell_inline_style, table_inline_constraints},
};

/// The quantity a shadow comparison disagreed on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableSizingQuantity {
    ColumnCount,
    ColumnSize(usize),
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

    let style_of = |source: BoxId| {
        boxes
            .origin_node(source)
            .and_then(|node| styles.get(node))
            .cloned()
    };

    let sizing = buckram::TableInlineSizingInput {
        grid,
        available_inline_size: containing_width,
        table_constraints: table_inline_constraints(computed, font_size, root_font_size),
        border_metrics,
        caption_min,
        track_visibility: TableTrackVisibility::all_visible(grid),
    };

    let (columns, column_groups) = fixed_table_track_inputs(grid, |source| {
        style_of(source).map_or_else(Default::default, |style| {
            table_inline_constraints(&style, font_size, root_font_size)
        })
    });

    let mut cells = Vec::with_capacity(grid.cells.len());
    for cell in &grid.cells {
        let Some(style) = style_of(cell.source) else {
            return Err(TableInlineSizingError::InvalidOffsets {
                box_id: cell.source,
            });
        };
        let lowered = table_cell_inline_style(&style, axes, font_size, root_font_size)?;
        cells.push(TableCellInlineMeasure {
            box_id: cell.source,
            // Fixed layout never consults content, by definition of the
            // algorithm. K4c5a's automatic step supplies real measures.
            content: IntrinsicSizes::new(0.0, 0.0)
                .ok_or(TableInlineSizingError::InvalidResultSize)?,
            preferred: lowered.constraints.preferred,
            minimum: lowered.constraints.minimum,
            maximum: lowered.constraints.maximum,
            box_sizing: lowered.constraints.box_sizing,
            offsets: lowered.offsets,
        });
    }

    Ok(TableFixedInlineSizingInput {
        sizing,
        columns,
        column_groups,
        cells,
    })
}
