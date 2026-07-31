//! Automatic table column measures over K4b's normalized grid.
//!
//! This module computes intrinsic column facts only. It deliberately does not
//! select a table width or turn a percentage into pixels: K4c4 owns that
//! second decision. The only ordering here is K4b's logical column order.

use crate::BoxId;

use super::{
    InlineSizeConstraint, TableBoxSizing, TableCellInlineMeasure, TableDeferral,
    TableInlineConstraints, TableInlineProperty, TableInlineSizingError, TableInlineSizingInput,
    TableTrackVisibilityState,
};

/// CSS constraints for one normalized K4b column in automatic layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableAutomaticColumnInput {
    /// The `<col>` box for an explicit K4b column, or `None` for a column
    /// created by cell placement.
    pub source: Option<BoxId>,
    pub constraints: TableInlineConstraints,
}

impl TableAutomaticColumnInput {
    fn from_grid(source: Option<BoxId>) -> Self {
        Self {
            source,
            constraints: TableInlineConstraints::default(),
        }
    }
}

/// CSS constraints for one normalized K4b column-group range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableAutomaticColumnGroupInput {
    pub source: BoxId,
    pub constraints: TableInlineConstraints,
}

impl TableAutomaticColumnGroupInput {
    fn from_grid(source: BoxId) -> Self {
        Self {
            source,
            constraints: TableInlineConstraints::default(),
        }
    }
}

/// Complete input to K4c3's automatic column-measure algorithm.
///
/// The vectors retain K4b order and box identity. This keeps style lowering
/// separate from placement and makes any future adapter unable to aggregate
/// intrinsic columns outside Buckram.
#[derive(Clone, Debug, PartialEq)]
pub struct TableAutomaticColumnMeasureInput<'a> {
    pub sizing: TableInlineSizingInput<'a>,
    pub columns: Vec<TableAutomaticColumnInput>,
    pub column_groups: Vec<TableAutomaticColumnGroupInput>,
    /// One K4c1 measurement for every K4b cell, in K4b topology order.
    pub cells: Vec<TableCellInlineMeasure>,
}

impl<'a> TableAutomaticColumnMeasureInput<'a> {
    /// Construct automatic constraints directly from K4b's normalized tracks.
    pub fn new(sizing: TableInlineSizingInput<'a>) -> Self {
        Self {
            columns: sizing
                .grid
                .columns
                .iter()
                .map(|column| TableAutomaticColumnInput::from_grid(column.source))
                .collect(),
            column_groups: sizing
                .grid
                .column_groups
                .iter()
                .map(|group| TableAutomaticColumnGroupInput::from_grid(group.source))
                .collect(),
            sizing,
            cells: Vec::new(),
        }
    }

    fn validate(&self) -> Result<(), TableInlineSizingError> {
        let grid = self.sizing.grid;
        if self.columns.len() != grid.columns.len() {
            return Err(TableInlineSizingError::AutomaticColumnInputCountMismatch {
                expected: grid.columns.len(),
                actual: self.columns.len(),
            });
        }
        for (index, (track, input)) in grid.columns.iter().zip(&self.columns).enumerate() {
            if track.source != input.source {
                return Err(TableInlineSizingError::AutomaticColumnSourceMismatch {
                    index,
                    expected: track.source,
                    actual: input.source,
                });
            }
        }

        if self.column_groups.len() != grid.column_groups.len() {
            return Err(
                TableInlineSizingError::AutomaticColumnGroupInputCountMismatch {
                    expected: grid.column_groups.len(),
                    actual: self.column_groups.len(),
                },
            );
        }
        for (index, (group, input)) in grid
            .column_groups
            .iter()
            .zip(&self.column_groups)
            .enumerate()
        {
            if group.source != input.source {
                return Err(TableInlineSizingError::AutomaticColumnGroupSourceMismatch {
                    index,
                    expected: group.source,
                    actual: input.source,
                });
            }
            if group
                .start
                .checked_add(group.span)
                .is_none_or(|end| end > grid.columns.len())
            {
                return Err(TableInlineSizingError::InvalidColumnGroupRange {
                    start: group.start,
                    span: group.span,
                });
            }
        }

        if self.cells.len() != grid.cells.len() {
            return Err(TableInlineSizingError::AutomaticCellInputCountMismatch {
                expected: grid.cells.len(),
                actual: self.cells.len(),
            });
        }
        for (index, (cell, measure)) in grid.cells.iter().zip(&self.cells).enumerate() {
            if cell.source != measure.box_id {
                return Err(TableInlineSizingError::AutomaticCellSourceMismatch {
                    index,
                    expected: cell.source,
                    actual: measure.box_id,
                });
            }
        }
        Ok(())
    }
}

/// One column's basis-free automatic-layout measure.
///
/// `percentage` is a bounded fraction in K4b logical column order. It is not
/// a resolved length. A constrained column has an explicit non-percentage
/// `width` contribution; a pure percentage does not make it constrained.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableColumnMeasure {
    pub min_content: f32,
    pub max_content: f32,
    pub percentage: f32,
    pub constrained: bool,
}

impl TableColumnMeasure {
    fn automatic() -> Self {
        Self {
            min_content: 0.0,
            max_content: 0.0,
            percentage: 0.0,
            constrained: false,
        }
    }

    pub(super) fn validate(self) -> Result<Self, TableInlineSizingError> {
        if !self.min_content.is_finite()
            || !self.max_content.is_finite()
            || !self.percentage.is_finite()
            || self.min_content < 0.0
            || self.max_content < self.min_content
            || !(0.0..=1.0).contains(&self.percentage)
        {
            return Err(TableInlineSizingError::InvalidColumnMeasure);
        }
        Ok(self)
    }
}

/// The exact increase made for one spanning cell, in logical column order.
#[derive(Clone, Debug, PartialEq)]
pub struct TableSpanMeasureDistribution {
    pub source: BoxId,
    pub column_start: usize,
    pub column_span: usize,
    pub min_content_increase: Vec<f32>,
    pub max_content_increase: Vec<f32>,
}

/// K4c3's complete basis-free automatic column result.
#[derive(Clone, Debug, PartialEq)]
pub struct TableAutomaticColumnMeasures {
    pub columns: Vec<TableColumnMeasure>,
    pub span_distributions: Vec<TableSpanMeasureDistribution>,
}

#[derive(Clone, Copy, Debug)]
struct IntrinsicConstraint {
    absolute: Option<f32>,
    percentage: f32,
    constrained: bool,
}

impl IntrinsicConstraint {
    const NONE: Self = Self {
        absolute: None,
        percentage: 0.0,
        constrained: false,
    };
}

#[derive(Clone, Copy, Debug)]
struct CellContribution {
    min_content: f32,
    max_content: f32,
    percentage: f32,
    constrained: bool,
}

/// Compute automatic column measures without choosing a table width.
///
/// The selected rules are the local Chrome 150 / Firefox 153 interop rule:
/// a span distributes excess over non-constrained, non-percentage columns in
/// proportion to their existing corresponding measure, falling back to equal
/// shares when every eligible measure is zero. Percentage demands consume the
/// remaining aggregate in logical K4b column order.
pub fn measure_automatic_columns(
    input: &TableAutomaticColumnMeasureInput<'_>,
) -> Result<TableAutomaticColumnMeasures, TableInlineSizingError> {
    input.validate()?;
    ensure_automatic_prerequisites(&input.sizing)?;

    let grid = input.sizing.grid;
    let mut columns = vec![TableColumnMeasure::automatic(); grid.columns.len()];

    // A single-column cell is a direct column contribution. Process it before
    // column and group ranges, then reserve spanning work for lower-span order.
    for (cell, measure) in grid.cells.iter().zip(&input.cells) {
        if cell.column_span == 1 {
            apply_cell_to_column(&mut columns[cell.column], cell_contribution(*measure)?);
        }
    }

    for (index, column) in input.columns.iter().enumerate() {
        apply_track_constraints(&mut columns[index], column.constraints, column.source)?;
    }
    for (group, group_input) in grid.column_groups.iter().zip(&input.column_groups) {
        apply_group_constraints(
            &mut columns,
            group.start,
            group.span,
            group_input.constraints,
            Some(group_input.source),
        )?;
    }

    let mut spanning = grid
        .cells
        .iter()
        .zip(&input.cells)
        .filter(|(cell, _)| cell.column_span > 1)
        .collect::<Vec<_>>();
    spanning
        .sort_by_key(|(cell, _)| (cell.column_span, cell.column, cell.row, cell.source.index()));

    let mut span_distributions = Vec::with_capacity(spanning.len());
    for (cell, measure) in spanning {
        let contribution = cell_contribution(*measure)?;
        let Some(end) = cell
            .column
            .checked_add(cell.column_span)
            .filter(|end| *end <= columns.len())
        else {
            return Err(TableInlineSizingError::InvalidColumnGroupRange {
                start: cell.column,
                span: cell.column_span,
            });
        };

        apply_spanning_percentage(&mut columns[cell.column..end], contribution.percentage);
        let max_before = columns[cell.column..end]
            .iter()
            .map(|column| column.max_content)
            .collect::<Vec<_>>();
        let min_content_increase = distribute_span_excess(
            &mut columns[cell.column..end],
            contribution.min_content,
            SpanMeasureKind::MinContent,
        );
        distribute_span_excess(
            &mut columns[cell.column..end],
            contribution.max_content,
            SpanMeasureKind::MaxContent,
        );
        let max_content_increase = columns[cell.column..end]
            .iter()
            .zip(max_before)
            .map(|(column, before)| column.max_content - before)
            .collect();
        span_distributions.push(TableSpanMeasureDistribution {
            source: cell.source,
            column_start: cell.column,
            column_span: cell.column_span,
            min_content_increase,
            max_content_increase,
        });
    }

    normalize_percentages(&mut columns);
    columns
        .into_iter()
        .map(TableColumnMeasure::validate)
        .collect::<Result<Vec<_>, _>>()
        .map(|columns| TableAutomaticColumnMeasures {
            columns,
            span_distributions,
        })
}

fn ensure_automatic_prerequisites(
    sizing: &TableInlineSizingInput<'_>,
) -> Result<(), TableInlineSizingError> {
    if sizing
        .track_visibility
        .columns
        .iter()
        .chain(sizing.track_visibility.rows.iter())
        .any(|visibility| *visibility == TableTrackVisibilityState::Collapsed)
    {
        return Err(TableInlineSizingError::Deferral(
            TableDeferral::TrackVisibilityPendingK4f,
        ));
    }
    // Cell offsets include declared borders. Their use is only sound in the
    // separated model until K4g supplies collapsed-border winners.
    sizing.separated_undistributable_inline_size()?;
    Ok(())
}

fn cell_contribution(
    measure: TableCellInlineMeasure,
) -> Result<CellContribution, TableInlineSizingError> {
    let outer = measure.outer_content_sizes()?;
    let preferred = cell_constraint(measure, measure.preferred, TableInlineProperty::Width)?;
    let minimum = cell_constraint(measure, measure.minimum, TableInlineProperty::MinWidth)?;
    let maximum = cell_constraint(measure, measure.maximum, TableInlineProperty::MaxWidth)?;

    // A maximum applies before minimum floors. This is the explicit CSS
    // min-width precedence, not a repair by swapping min and max values.
    let mut min_content = outer.min_content;
    let mut max_content = outer.max_content;
    if let Some(maximum) = maximum.absolute {
        // A content contribution cannot have a maximum below its own
        // min-content contribution. This preserves the intrinsic invariant;
        // it is not a swap of the two values.
        max_content = max_content.min(maximum).max(min_content);
    }
    for minimum in [preferred.absolute, minimum.absolute].into_iter().flatten() {
        min_content = min_content.max(minimum);
        max_content = max_content.max(minimum);
    }
    let percentage = preferred
        .percentage
        .max(minimum.percentage)
        .max(maximum.percentage);
    let constrained = preferred.constrained;
    let contribution = CellContribution {
        min_content,
        max_content,
        percentage,
        constrained,
    };
    valid_cell_contribution(contribution, measure.box_id)
}

fn valid_cell_contribution(
    contribution: CellContribution,
    box_id: BoxId,
) -> Result<CellContribution, TableInlineSizingError> {
    if !contribution.min_content.is_finite()
        || !contribution.max_content.is_finite()
        || !contribution.percentage.is_finite()
        || contribution.min_content < 0.0
        || contribution.max_content < contribution.min_content
        || contribution.percentage < 0.0
    {
        return Err(TableInlineSizingError::InvalidIntrinsicPair { box_id });
    }
    Ok(contribution)
}

fn cell_constraint(
    measure: TableCellInlineMeasure,
    constraint: InlineSizeConstraint,
    property: TableInlineProperty,
) -> Result<IntrinsicConstraint, TableInlineSizingError> {
    let mut contribution = intrinsic_constraint(constraint, Some(measure.box_id), property)?;
    if let Some(absolute) = contribution.absolute {
        contribution.absolute = Some(match measure.box_sizing {
            TableBoxSizing::ContentBox => {
                // Automatic measures run before a table width exists, so a
                // padding percentage has no basis and must not be sampled at
                // zero. K4c5 routes these tables to the compatibility bridge.
                if measure.offsets.needs_percentage_basis() {
                    return Err(TableInlineSizingError::Deferral(
                        TableDeferral::PercentagePaddingPendingBasis,
                    ));
                }
                absolute
                    + measure.offsets.absolute_total().ok_or(
                        TableInlineSizingError::InvalidOffsets {
                            box_id: measure.box_id,
                        },
                    )?
            },
            TableBoxSizing::BorderBox => absolute,
        });
    }
    Ok(contribution)
}

fn intrinsic_constraint(
    constraint: InlineSizeConstraint,
    box_id: Option<BoxId>,
    property: TableInlineProperty,
) -> Result<IntrinsicConstraint, TableInlineSizingError> {
    match constraint {
        InlineSizeConstraint::Auto
        | InlineSizeConstraint::None
        | InlineSizeConstraint::MinContent
        | InlineSizeConstraint::MaxContent => Ok(IntrinsicConstraint::NONE),
        InlineSizeConstraint::Unreduced => {
            Err(TableInlineSizingError::UnreducedConstraint { box_id, property })
        },
        InlineSizeConstraint::Value(value) => {
            affine_intrinsic_constraint(value, true, box_id, property)
        },
        // `fit-content()` preserves its percentage threshold for K4c4. A pure
        // absolute threshold remains an intrinsic bound, never a resolved
        // used width.
        InlineSizeConstraint::FitContent(value) => {
            affine_intrinsic_constraint(value, false, box_id, property)
        },
    }
}

fn affine_intrinsic_constraint(
    value: super::AffineLengthPercentage,
    constrained: bool,
    box_id: Option<BoxId>,
    property: TableInlineProperty,
) -> Result<IntrinsicConstraint, TableInlineSizingError> {
    if !value.absolute.is_finite() || !value.percentage.is_finite() {
        return Err(TableInlineSizingError::InvalidConstraint { box_id, property });
    }
    Ok(IntrinsicConstraint {
        absolute: (constrained || value.percentage == 0.0).then_some(value.absolute.max(0.0)),
        percentage: value.percentage.max(0.0),
        constrained: constrained && (value.absolute != 0.0 || value.percentage == 0.0),
    })
}

fn apply_cell_to_column(column: &mut TableColumnMeasure, contribution: CellContribution) {
    column.min_content = column.min_content.max(contribution.min_content);
    column.max_content = column.max_content.max(contribution.max_content);
    column.percentage = column.percentage.max(contribution.percentage);
    column.constrained |= contribution.constrained;
}

fn apply_track_constraints(
    column: &mut TableColumnMeasure,
    constraints: TableInlineConstraints,
    source: Option<BoxId>,
) -> Result<(), TableInlineSizingError> {
    let contribution =
        intrinsic_constraint(constraints.preferred, source, TableInlineProperty::Width)?;
    if let Some(absolute) = contribution.absolute {
        column.min_content = column.min_content.max(absolute);
        column.max_content = column.max_content.max(absolute);
    }
    column.percentage = column.percentage.max(contribution.percentage);
    column.constrained |= contribution.constrained;
    Ok(())
}

fn apply_group_constraints(
    columns: &mut [TableColumnMeasure],
    start: usize,
    span: usize,
    constraints: TableInlineConstraints,
    source: Option<BoxId>,
) -> Result<(), TableInlineSizingError> {
    let Some(end) = start.checked_add(span).filter(|end| *end <= columns.len()) else {
        return Err(TableInlineSizingError::InvalidColumnGroupRange { start, span });
    };
    let contribution =
        intrinsic_constraint(constraints.preferred, source, TableInlineProperty::Width)?;
    if let Some(absolute) = contribution.absolute {
        distribute_span_excess(
            &mut columns[start..end],
            absolute,
            SpanMeasureKind::MinContent,
        );
        distribute_span_excess(
            &mut columns[start..end],
            absolute,
            SpanMeasureKind::MaxContent,
        );
    }
    if contribution.percentage > 0.0 {
        apply_spanning_percentage(&mut columns[start..end], contribution.percentage);
    }
    if contribution.constrained {
        for column in &mut columns[start..end] {
            column.constrained = true;
        }
    }
    Ok(())
}

fn apply_spanning_percentage(columns: &mut [TableColumnMeasure], requested: f32) {
    if requested <= 0.0 || columns.is_empty() {
        return;
    }
    let existing = columns.iter().map(|column| column.percentage).sum::<f32>();
    let excess = (requested - existing).max(0.0);
    if excess == 0.0 {
        return;
    }
    let eligible = columns
        .iter()
        .enumerate()
        .filter_map(|(index, column)| (!column.constrained).then_some(index))
        .collect::<Vec<_>>();
    let targets = if eligible.is_empty() {
        (0..columns.len()).collect()
    } else {
        eligible
    };
    let share = excess / targets.len() as f32;
    for index in targets {
        columns[index].percentage += share;
    }
}

#[derive(Clone, Copy)]
enum SpanMeasureKind {
    MinContent,
    MaxContent,
}

fn distribute_span_excess(
    columns: &mut [TableColumnMeasure],
    required: f32,
    kind: SpanMeasureKind,
) -> Vec<f32> {
    let current = columns
        .iter()
        .map(|column| match kind {
            SpanMeasureKind::MinContent => column.min_content,
            SpanMeasureKind::MaxContent => column.max_content,
        })
        .sum::<f32>();
    let excess = (required - current).max(0.0);
    let mut increase = vec![0.0; columns.len()];
    if excess == 0.0 || columns.is_empty() {
        return increase;
    }
    let eligible = columns
        .iter()
        .enumerate()
        .filter_map(|(index, column)| {
            (!column.constrained && column.percentage == 0.0).then_some(index)
        })
        .collect::<Vec<_>>();
    let eligible = if eligible.is_empty() {
        (0..columns.len()).collect()
    } else {
        eligible
    };
    let weights = eligible
        .iter()
        .map(|index| match kind {
            SpanMeasureKind::MinContent => columns[*index].min_content,
            SpanMeasureKind::MaxContent => columns[*index].max_content,
        })
        .collect::<Vec<_>>();
    let target_count = eligible.len() as f32;
    let total_weight = weights.iter().sum::<f32>();
    for (index, weight) in eligible.into_iter().zip(weights) {
        let share = if total_weight > 0.0 {
            excess * weight / total_weight
        } else {
            excess / target_count
        };
        increase[index] = share;
        match kind {
            SpanMeasureKind::MinContent => {
                columns[index].min_content += share;
                columns[index].max_content =
                    columns[index].max_content.max(columns[index].min_content);
            },
            SpanMeasureKind::MaxContent => columns[index].max_content += share,
        }
    }
    increase
}

fn normalize_percentages(columns: &mut [TableColumnMeasure]) {
    let mut remaining = 1.0;
    for column in columns {
        let requested = column.percentage.clamp(0.0, 1.0);
        column.percentage = requested.min(remaining);
        remaining = (remaining - column.percentage).max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AffineLengthPercentage, BoxGeneration, BoxOrigin, BoxTreeInput, CssBoxTree, Direction,
        DisplayInside, DisplayOutside, DisplayRole, FlowAxes, InternalTableRole, PositioningScheme,
        TableCellInput, TableGrid, TableGridInputs, TableRowSpan, TableSeparatedBorderMetrics,
        TableTrackVisibility, WritingMode, generate_box_tree,
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

    fn node(id: u8, role: InternalTableRole, children: Vec<BoxTreeInput<u8>>) -> BoxTreeInput<u8> {
        BoxTreeInput::new(
            BoxOrigin::Element(id),
            table_role(role),
            FlowAxes::HORIZONTAL_LTR,
            PositioningScheme::Static,
            false,
            children,
        )
    }

    fn grid(cells: Vec<BoxTreeInput<u8>>, cell_inputs: &[(u8, TableCellInput)]) -> TableGrid {
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
            vec![node(
                2,
                InternalTableRole::RowGroup,
                vec![node(3, InternalTableRole::Row, cells)],
            )],
        )]);
        let mut inputs = TableGridInputs::default();
        for (source, input) in cell_inputs {
            inputs.set_cell(tree.principal_box(*source).expect("cell"), *input);
        }
        TableGrid::from_box_tree(&tree, tree.principal_box(1).expect("grid"), &inputs)
    }

    fn directional_grid(axes: FlowAxes) -> TableGrid {
        let cell = |id| {
            BoxTreeInput::new(
                BoxOrigin::Element(id),
                table_role(InternalTableRole::Cell),
                axes,
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
            axes,
            PositioningScheme::Static,
            false,
            vec![BoxTreeInput::new(
                BoxOrigin::Element(2),
                table_role(InternalTableRole::RowGroup),
                axes,
                PositioningScheme::Static,
                false,
                vec![BoxTreeInput::new(
                    BoxOrigin::Element(3),
                    table_role(InternalTableRole::Row),
                    axes,
                    PositioningScheme::Static,
                    false,
                    vec![cell(4), cell(5), cell(6)],
                )],
            )],
        )]);
        TableGrid::from_box_tree(
            &tree,
            tree.principal_box(1).expect("grid"),
            &TableGridInputs::default(),
        )
    }

    fn sizing<'a>(grid: &'a TableGrid) -> TableInlineSizingInput<'a> {
        TableInlineSizingInput {
            grid,
            available_inline_size: Some(600.0),
            table_constraints: TableInlineConstraints::default(),
            border_metrics: super::super::TableInlineBorderMetrics::Separated(
                TableSeparatedBorderMetrics::default(),
            ),
            caption_min: super::super::CaptionMinContribution::NoCaption,
            track_visibility: TableTrackVisibility::all_visible(grid),
        }
    }

    fn measure(box_id: BoxId, min_content: f32, max_content: f32) -> TableCellInlineMeasure {
        TableCellInlineMeasure {
            box_id,
            content: crate::IntrinsicSizes::new(min_content, max_content).expect("intrinsic pair"),
            preferred: InlineSizeConstraint::Auto,
            minimum: InlineSizeConstraint::Auto,
            maximum: InlineSizeConstraint::None,
            box_sizing: TableBoxSizing::ContentBox,
            offsets: super::super::CellInlineOffsets::ZERO,
        }
    }

    fn input<'a>(
        grid: &'a TableGrid,
        cells: Vec<TableCellInlineMeasure>,
    ) -> TableAutomaticColumnMeasureInput<'a> {
        let mut input = TableAutomaticColumnMeasureInput::new(sizing(grid));
        input.cells = cells;
        input
    }

    #[test]
    fn empty_and_single_column_measures_keep_intrinsic_constraints_basis_free() {
        let empty = grid(vec![], &[]);
        let empty_measures = measure_automatic_columns(&input(&empty, vec![])).expect("empty grid");
        assert!(empty_measures.columns.is_empty());

        let grid = grid(vec![node(4, InternalTableRole::Cell, vec![])], &[]);
        let cell = grid.cells[0].source;
        let mut cell_measure = measure(cell, 10.0, 30.0);
        cell_measure.preferred =
            InlineSizeConstraint::Value(AffineLengthPercentage::new(20.0, 0.4).expect("width"));
        cell_measure.minimum =
            InlineSizeConstraint::Value(AffineLengthPercentage::new(25.0, 0.0).expect("min width"));
        let measures =
            measure_automatic_columns(&input(&grid, vec![cell_measure])).expect("measure");
        assert_eq!(
            measures.columns,
            vec![TableColumnMeasure {
                min_content: 25.0,
                max_content: 30.0,
                percentage: 0.4,
                constrained: true,
            }]
        );
    }

    #[test]
    fn content_above_a_specified_width_remains_an_intrinsic_lower_bound() {
        let grid = grid(vec![node(4, InternalTableRole::Cell, vec![])], &[]);
        let mut cell_measure = measure(grid.cells[0].source, 50.0, 80.0);
        cell_measure.preferred =
            InlineSizeConstraint::Value(AffineLengthPercentage::new(20.0, 0.0).expect("width"));

        let result = measure_automatic_columns(&input(&grid, vec![cell_measure]))
            .expect("automatic measures");
        assert_eq!(
            result.columns,
            vec![TableColumnMeasure {
                min_content: 50.0,
                max_content: 80.0,
                percentage: 0.0,
                constrained: true,
            }]
        );
    }

    #[test]
    fn maximum_constraint_never_swaps_an_intrinsic_pair() {
        let grid = grid(vec![node(4, InternalTableRole::Cell, vec![])], &[]);
        let mut cell_measure = measure(grid.cells[0].source, 50.0, 80.0);
        cell_measure.maximum =
            InlineSizeConstraint::Value(AffineLengthPercentage::new(20.0, 0.0).expect("max"));

        let result = measure_automatic_columns(&input(&grid, vec![cell_measure]))
            .expect("automatic measures");
        assert_eq!(
            result.columns,
            vec![TableColumnMeasure {
                min_content: 50.0,
                max_content: 50.0,
                percentage: 0.0,
                constrained: false,
            }]
        );
    }

    #[test]
    fn column_and_group_constraints_apply_to_normalized_ranges() {
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
                node(
                    2,
                    InternalTableRole::ColumnGroup,
                    vec![
                        node(3, InternalTableRole::Column, vec![]),
                        node(4, InternalTableRole::Column, vec![]),
                    ],
                ),
                node(
                    5,
                    InternalTableRole::RowGroup,
                    vec![node(
                        6,
                        InternalTableRole::Row,
                        vec![
                            node(7, InternalTableRole::Cell, vec![]),
                            node(8, InternalTableRole::Cell, vec![]),
                        ],
                    )],
                ),
            ],
        )]);
        let table = tree.principal_box(1).expect("table");
        let grid = TableGrid::from_box_tree(&tree, table, &TableGridInputs::default());
        let mut automatic = input(
            &grid,
            grid.cells
                .iter()
                .map(|cell| measure(cell.source, 10.0, 10.0))
                .collect(),
        );
        automatic.columns[0].constraints.preferred = InlineSizeConstraint::Value(
            AffineLengthPercentage::new(40.0, 0.0).expect("column width"),
        );
        automatic.column_groups[0].constraints.preferred = InlineSizeConstraint::Value(
            AffineLengthPercentage::new(100.0, 0.0).expect("group width"),
        );
        let result = measure_automatic_columns(&automatic).expect("measure");
        assert_eq!(
            result.columns,
            vec![
                TableColumnMeasure {
                    min_content: 40.0,
                    max_content: 40.0,
                    percentage: 0.0,
                    constrained: true
                },
                TableColumnMeasure {
                    min_content: 60.0,
                    max_content: 60.0,
                    percentage: 0.0,
                    constrained: true
                },
            ]
        );
    }

    #[test]
    fn spans_follow_lower_span_measures_and_record_proportional_excess() {
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
            vec![node(
                2,
                InternalTableRole::RowGroup,
                vec![
                    node(
                        3,
                        InternalTableRole::Row,
                        vec![
                            node(4, InternalTableRole::Cell, vec![]),
                            node(5, InternalTableRole::Cell, vec![]),
                        ],
                    ),
                    node(
                        6,
                        InternalTableRole::Row,
                        vec![node(7, InternalTableRole::Cell, vec![])],
                    ),
                ],
            )],
        )]);
        let mut inputs = TableGridInputs::default();
        inputs.set_cell(
            tree.principal_box(4).expect("lower span"),
            TableCellInput {
                column_span: 2,
                row_span: TableRowSpan::Count(1),
            },
        );
        inputs.set_cell(
            tree.principal_box(7).expect("higher span"),
            TableCellInput {
                column_span: 3,
                row_span: TableRowSpan::Count(1),
            },
        );
        let grid = TableGrid::from_box_tree(&tree, tree.principal_box(1).expect("grid"), &inputs);
        let mut measures = grid
            .cells
            .iter()
            .map(|cell| measure(cell.source, 0.0, 0.0))
            .collect::<Vec<_>>();
        measures[0].content = crate::IntrinsicSizes::new(150.0, 150.0).expect("span pair");
        measures[2].content = crate::IntrinsicSizes::new(300.0, 300.0).expect("span pair");
        let mut automatic = input(&grid, measures);
        automatic.columns[0].constraints.preferred = InlineSizeConstraint::Value(
            AffineLengthPercentage::new(20.0, 0.0).expect("fixed column"),
        );
        let result = measure_automatic_columns(&automatic).expect("spans");
        assert_eq!(
            result.columns,
            vec![
                TableColumnMeasure {
                    min_content: 20.0,
                    max_content: 20.0,
                    percentage: 0.0,
                    constrained: true
                },
                TableColumnMeasure {
                    min_content: 280.0,
                    max_content: 280.0,
                    percentage: 0.0,
                    constrained: false
                },
                TableColumnMeasure {
                    min_content: 0.0,
                    max_content: 0.0,
                    percentage: 0.0,
                    constrained: false
                },
            ]
        );
        assert_eq!(result.span_distributions.len(), 2);
        assert_eq!(
            result.span_distributions[0].min_content_increase,
            vec![0.0, 130.0]
        );
        assert_eq!(
            result.span_distributions[1].max_content_increase,
            vec![0.0, 150.0, 0.0]
        );
    }

    #[test]
    fn pure_percentages_stay_unconstrained_and_consume_logical_remainder() {
        let grid = grid(
            vec![
                node(4, InternalTableRole::Cell, vec![]),
                node(5, InternalTableRole::Cell, vec![]),
                node(6, InternalTableRole::Cell, vec![]),
            ],
            &[],
        );
        let mut measures = grid
            .cells
            .iter()
            .map(|cell| measure(cell.source, 0.0, 0.0))
            .collect::<Vec<_>>();
        for measure in &mut measures[..2] {
            measure.preferred = InlineSizeConstraint::Value(
                AffineLengthPercentage::new(0.0, 0.6).expect("percentage"),
            );
        }
        let result = measure_automatic_columns(&input(&grid, measures)).expect("percentages");
        assert_eq!(result.columns[0].percentage, 0.6);
        assert!((result.columns[1].percentage - 0.4).abs() < 0.000_01);
        assert_eq!(result.columns[2], TableColumnMeasure::automatic());
        assert!(result.columns.iter().all(|column| !column.constrained));
    }

    #[test]
    fn a_span_keeps_fixed_and_percentage_tracks_out_of_its_auto_growth() {
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
            vec![node(
                2,
                InternalTableRole::RowGroup,
                vec![node(
                    3,
                    InternalTableRole::Row,
                    vec![node(4, InternalTableRole::Cell, vec![])],
                )],
            )],
        )]);
        let mut inputs = TableGridInputs::default();
        inputs.set_cell(
            tree.principal_box(4).expect("span"),
            TableCellInput {
                column_span: 3,
                row_span: TableRowSpan::Count(1),
            },
        );
        let grid = TableGrid::from_box_tree(&tree, tree.principal_box(1).expect("grid"), &inputs);
        let mut automatic = input(&grid, vec![measure(grid.cells[0].source, 300.0, 300.0)]);
        automatic.columns[0].constraints.preferred = InlineSizeConstraint::Value(
            AffineLengthPercentage::new(20.0, 0.0).expect("fixed width"),
        );
        automatic.columns[1].constraints.preferred = InlineSizeConstraint::Value(
            AffineLengthPercentage::new(0.0, 0.6).expect("percentage width"),
        );

        let result = measure_automatic_columns(&automatic).expect("span measures");
        assert_eq!(
            result.columns,
            vec![
                TableColumnMeasure {
                    min_content: 20.0,
                    max_content: 20.0,
                    percentage: 0.0,
                    constrained: true,
                },
                TableColumnMeasure {
                    min_content: 0.0,
                    max_content: 0.0,
                    percentage: 0.6,
                    constrained: false,
                },
                TableColumnMeasure {
                    min_content: 280.0,
                    max_content: 280.0,
                    percentage: 0.0,
                    constrained: false,
                },
            ]
        );
    }

    #[test]
    fn unrelated_cell_order_does_not_change_measures() {
        let grid = grid(
            vec![
                node(4, InternalTableRole::Cell, vec![]),
                node(5, InternalTableRole::Cell, vec![]),
                node(6, InternalTableRole::Cell, vec![]),
            ],
            &[],
        );
        let cells = grid
            .cells
            .iter()
            .map(|cell| {
                measure(
                    cell.source,
                    (cell.column + 1) as f32 * 10.0,
                    (cell.column + 1) as f32 * 20.0,
                )
            })
            .collect::<Vec<_>>();
        let forward = measure_automatic_columns(&input(&grid, cells.clone())).expect("forward");
        let mut reversed = cells;
        reversed.reverse();
        let mut reversed_input = TableAutomaticColumnMeasureInput::new(sizing(&grid));
        // The input contract is topology ordered; restore it by BoxId after an
        // unrelated collection order change before passing the boundary.
        reversed.sort_by_key(|measure| {
            grid.cells
                .iter()
                .position(|cell| cell.source == measure.box_id)
        });
        reversed_input.cells = reversed;
        let reverse = measure_automatic_columns(&reversed_input).expect("reverse");
        assert_eq!(forward.columns, reverse.columns);
    }

    #[test]
    fn reversing_direction_keeps_logical_column_measures_unchanged() {
        let ltr = directional_grid(FlowAxes::HORIZONTAL_LTR);
        let rtl = directional_grid(FlowAxes::new(WritingMode::HorizontalTb, Direction::Rtl));
        let measures_for = |grid: &TableGrid| {
            let mut cells = grid
                .cells
                .iter()
                .map(|cell| measure(cell.source, 10.0, 30.0))
                .collect::<Vec<_>>();
            cells[0].preferred = InlineSizeConstraint::Value(
                AffineLengthPercentage::new(0.0, 0.6).expect("percentage"),
            );
            cells[1].preferred = InlineSizeConstraint::Value(
                AffineLengthPercentage::new(0.0, 0.6).expect("percentage"),
            );
            measure_automatic_columns(&input(grid, cells)).expect("logical measures")
        };
        assert_eq!(measures_for(&ltr).columns, measures_for(&rtl).columns);
    }

    #[test]
    fn collapsed_borders_and_tracks_remain_explicit_deferrals() {
        let grid = grid(vec![node(4, InternalTableRole::Cell, vec![])], &[]);
        let mut automatic = input(&grid, vec![measure(grid.cells[0].source, 1.0, 1.0)]);
        automatic.sizing.border_metrics =
            super::super::TableInlineBorderMetrics::CollapsedPendingK4g;
        assert_eq!(
            measure_automatic_columns(&automatic),
            Err(TableInlineSizingError::Deferral(
                TableDeferral::CollapsedBorderMetricsPendingK4g
            ))
        );

        automatic.sizing.border_metrics = super::super::TableInlineBorderMetrics::Separated(
            TableSeparatedBorderMetrics::default(),
        );
        automatic.sizing.track_visibility.columns[0] = TableTrackVisibilityState::Collapsed;
        assert_eq!(
            measure_automatic_columns(&automatic),
            Err(TableInlineSizingError::Deferral(
                TableDeferral::TrackVisibilityPendingK4f
            ))
        );
    }
}
