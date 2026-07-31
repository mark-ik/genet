//! CSS 2 fixed-table inline sizing over K4b's normalized grid.
//!
//! This is deliberately a model-only algorithm. It receives only the K4b
//! topology and K4c1 logical sizing inputs. Livery's Grid/Flex bridge remains
//! the live renderer until K4c5, so no Taffy track or completed fragment may
//! enter here.

use crate::BoxId;

use super::{
    InlineSizeConstraint, TableBoxSizing, TableCellInlineMeasure, TableDeferral,
    TableInlineBorderMetrics, TableInlineConstraints, TableInlineProperty, TableInlineSizingError,
    TableInlineSizingInput, TableInlineSizingResult, TableTrackVisibilityState,
};

/// CSS constraints for one normalized K4b column track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableFixedColumnInput {
    /// The `<col>` box when this is an explicit K4b column, or `None` for an
    /// implicit column created by cell placement.
    pub source: Option<BoxId>,
    pub constraints: TableInlineConstraints,
}

impl TableFixedColumnInput {
    fn from_grid(source: Option<BoxId>) -> Self {
        Self {
            source,
            constraints: TableInlineConstraints::default(),
        }
    }
}

/// CSS constraints for one normalized K4b column-group range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableFixedColumnGroupInput {
    pub source: BoxId,
    pub constraints: TableInlineConstraints,
}

impl TableFixedColumnGroupInput {
    fn from_grid(source: BoxId) -> Self {
        Self {
            source,
            constraints: TableInlineConstraints::default(),
        }
    }
}

/// Complete input to the fixed inline algorithm.
///
/// The column and group vectors deliberately retain K4b order and identity.
/// That lets the adapter map computed styles by `BoxId` without rediscovering
/// tracks through DOM traversal.
#[derive(Clone, Debug, PartialEq)]
pub struct TableFixedInlineSizingInput<'a> {
    pub sizing: TableInlineSizingInput<'a>,
    pub columns: Vec<TableFixedColumnInput>,
    pub column_groups: Vec<TableFixedColumnGroupInput>,
    /// One K4c1 measurement for every K4b cell, in K4b topology order.
    pub cells: Vec<TableCellInlineMeasure>,
}

impl<'a> TableFixedInlineSizingInput<'a> {
    /// Construct auto constraints directly from K4b's normalized tracks.
    pub fn new(sizing: TableInlineSizingInput<'a>) -> Self {
        Self {
            columns: sizing
                .grid
                .columns
                .iter()
                .map(|column| TableFixedColumnInput::from_grid(column.source))
                .collect(),
            column_groups: sizing
                .grid
                .column_groups
                .iter()
                .map(|group| TableFixedColumnGroupInput::from_grid(group.source))
                .collect(),
            sizing,
            cells: Vec::new(),
        }
    }

    fn validate(&self) -> Result<(), TableInlineSizingError> {
        let grid = self.sizing.grid;
        if self.columns.len() != grid.columns.len() {
            return Err(TableInlineSizingError::FixedColumnInputCountMismatch {
                expected: grid.columns.len(),
                actual: self.columns.len(),
            });
        }
        for (index, (track, input)) in grid.columns.iter().zip(&self.columns).enumerate() {
            if track.source != input.source {
                return Err(TableInlineSizingError::FixedColumnSourceMismatch {
                    index,
                    expected: track.source,
                    actual: input.source,
                });
            }
        }

        if self.column_groups.len() != grid.column_groups.len() {
            return Err(TableInlineSizingError::FixedColumnGroupInputCountMismatch {
                expected: grid.column_groups.len(),
                actual: self.column_groups.len(),
            });
        }
        for (index, (group, input)) in grid
            .column_groups
            .iter()
            .zip(&self.column_groups)
            .enumerate()
        {
            if group.source != input.source {
                return Err(TableInlineSizingError::FixedColumnGroupSourceMismatch {
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
            return Err(TableInlineSizingError::FixedCellInputCountMismatch {
                expected: grid.cells.len(),
                actual: self.cells.len(),
            });
        }
        for (index, (cell, measure)) in grid.cells.iter().zip(&self.cells).enumerate() {
            if cell.source != measure.box_id {
                return Err(TableInlineSizingError::FixedCellSourceMismatch {
                    index,
                    expected: cell.source,
                    actual: measure.box_id,
                });
            }
        }
        Ok(())
    }
}

/// Fixed layout either produces concrete logical column sizes or deliberately
/// hands control to K4c3's automatic algorithm.
#[derive(Clone, Debug, PartialEq)]
pub enum TableFixedInlineSizingOutcome {
    Fixed(TableInlineSizingResult),
    Automatic(TableFixedLayoutFallback),
}

/// Reasons fixed sizing does not enter its arithmetic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableFixedLayoutFallback {
    /// CSS 2 permits automatic layout when `table-layout: fixed` has
    /// `width: auto`; K4c chooses that conservative, interoperable policy.
    TableWidthAuto,
    /// Intrinsic keywords and fit-content require K4c3's automatic measures.
    TableWidthNotDefinite,
}

/// Size a `table-layout: fixed` table in the separated border model.
///
/// The precedence follows CSS 2: explicit columns, normalized column groups,
/// first-row cells, then equal shares for unresolved columns. Later-row
/// measurements are intentionally validated but never consulted. The returned
/// `used_table_inline_size` is measured at the table constraint's selected
/// box-sizing edge; `used_grid_inline_size` includes table borders, padding,
/// and each separated border-spacing interval exactly once.
pub fn size_fixed_table_inline(
    input: &TableFixedInlineSizingInput<'_>,
) -> Result<TableFixedInlineSizingOutcome, TableInlineSizingError> {
    input.validate()?;
    let grid = input.sizing.grid;
    if grid.columns.is_empty() {
        return Err(TableInlineSizingError::FixedLayoutWithoutColumns);
    }
    if input
        .sizing
        .track_visibility
        .columns
        .iter()
        .chain(input.sizing.track_visibility.rows.iter())
        .any(|visibility| *visibility == TableTrackVisibilityState::Collapsed)
    {
        return Err(TableInlineSizingError::Deferral(
            TableDeferral::TrackVisibilityPendingK4f,
        ));
    }
    if !matches!(
        input.sizing.caption_min,
        super::CaptionMinContribution::NoCaption
    ) {
        return Err(TableInlineSizingError::Deferral(
            TableDeferral::CaptionMinPendingK4e,
        ));
    }

    let Some(mut requested_table_size) = resolve_table_width(&input.sizing)? else {
        return Ok(TableFixedInlineSizingOutcome::Automatic(
            fixed_layout_fallback(input.sizing.table_constraints.preferred),
        ));
    };
    requested_table_size = clamp_table_size(requested_table_size, &input.sizing)?;

    let (table_offsets, separated_spacing, undistributable) = separated_metrics(&input.sizing)?;
    let requested_grid_size = table_size_to_grid_size(
        requested_table_size,
        input.sizing.table_constraints.box_sizing,
        table_offsets,
    )?;
    let requested_columns_size = (requested_grid_size - undistributable).max(0.0);

    let mut column_sizes = vec![None; grid.columns.len()];
    for (index, column) in input.columns.iter().enumerate() {
        column_sizes[index] = resolve_fixed_constraint(
            column.constraints.preferred,
            requested_table_size,
            column.source,
        )?;
    }

    for (group, group_input) in grid.column_groups.iter().zip(&input.column_groups) {
        let Some(group_size) = resolve_fixed_constraint(
            group_input.constraints.preferred,
            requested_table_size,
            Some(group_input.source),
        )?
        else {
            continue;
        };
        apply_range_minimum(&mut column_sizes, group.start, group.span, group_size)?;
    }

    for (cell, measure) in grid.cells.iter().zip(&input.cells) {
        if cell.row != 0 {
            continue;
        }
        let Some(cell_size) = fixed_cell_border_box_size(measure, requested_table_size)? else {
            continue;
        };
        apply_first_row_cell(&mut column_sizes, cell.column, cell.column_span, cell_size)?;
    }

    let unresolved_columns = column_sizes
        .iter()
        .enumerate()
        .filter_map(|(index, size)| size.is_none().then_some(index))
        .collect::<Vec<_>>();
    let fixed_sum = column_sizes.iter().flatten().sum::<f32>();
    if unresolved_columns.is_empty() {
        distribute_excess(
            &mut column_sizes,
            (requested_columns_size - fixed_sum).max(0.0),
        );
    } else {
        let share =
            ((requested_columns_size - fixed_sum).max(0.0)) / unresolved_columns.len() as f32;
        for index in unresolved_columns {
            column_sizes[index] = Some(share);
        }
    }
    let column_sizes = column_sizes
        .into_iter()
        .map(|size| size.expect("all unresolved columns receive an equal share"))
        .collect::<Vec<_>>();
    let used_grid_inline_size = column_sizes.iter().sum::<f32>() + undistributable;
    let used_table_inline_size = grid_size_to_table_size(
        used_grid_inline_size,
        input.sizing.table_constraints.box_sizing,
        table_offsets,
    )?;
    let intrinsic_sizes =
        crate::IntrinsicSizes::new(used_table_inline_size, used_table_inline_size)
            .ok_or(TableInlineSizingError::InvalidResultSize)?;
    let result = TableInlineSizingResult::new(
        &input.sizing,
        intrinsic_sizes,
        used_table_inline_size,
        used_grid_inline_size,
        column_sizes,
    )?;
    debug_assert!((separated_spacing + table_offsets - undistributable).abs() < 0.01);
    Ok(TableFixedInlineSizingOutcome::Fixed(result))
}

fn fixed_layout_fallback(constraint: InlineSizeConstraint) -> TableFixedLayoutFallback {
    match constraint {
        InlineSizeConstraint::Auto => TableFixedLayoutFallback::TableWidthAuto,
        InlineSizeConstraint::None
        | InlineSizeConstraint::MinContent
        | InlineSizeConstraint::MaxContent
        | InlineSizeConstraint::FitContent(_) => TableFixedLayoutFallback::TableWidthNotDefinite,
        InlineSizeConstraint::Value(_) | InlineSizeConstraint::Unreduced => unreachable!(
            "definite and unreduced constraints are handled before fixed layout falls back"
        ),
    }
}

fn resolve_table_width(
    sizing: &TableInlineSizingInput<'_>,
) -> Result<Option<f32>, TableInlineSizingError> {
    sizing.table_constraints.preferred.resolve_definite(
        sizing.available_inline_size,
        None,
        TableInlineProperty::Width,
    )
}

fn clamp_table_size(
    table_size: f32,
    sizing: &TableInlineSizingInput<'_>,
) -> Result<f32, TableInlineSizingError> {
    let minimum = sizing.table_constraints.minimum.resolve_definite(
        sizing.available_inline_size,
        None,
        TableInlineProperty::MinWidth,
    )?;
    let maximum = sizing.table_constraints.maximum.resolve_definite(
        sizing.available_inline_size,
        None,
        TableInlineProperty::MaxWidth,
    )?;
    let minimum = minimum.unwrap_or(0.0);
    let maximum = maximum.unwrap_or(f32::INFINITY).max(minimum);
    Ok(table_size.max(minimum).min(maximum))
}

fn separated_metrics(
    sizing: &TableInlineSizingInput<'_>,
) -> Result<(f32, f32, f32), TableInlineSizingError> {
    let TableInlineBorderMetrics::Separated(metrics) = sizing.border_metrics else {
        return Err(TableInlineSizingError::Deferral(
            TableDeferral::CollapsedBorderMetricsPendingK4g,
        ));
    };
    let table_offsets = metrics
        .table_offsets
        .total(sizing.table_padding_basis()?)
        .ok_or(TableInlineSizingError::InvalidBorderMetrics)?;
    let undistributable = sizing.separated_undistributable_inline_size()?;
    let separated_spacing = undistributable - table_offsets;
    if !separated_spacing.is_finite() || separated_spacing < 0.0 {
        return Err(TableInlineSizingError::InvalidBorderMetrics);
    }
    Ok((table_offsets, separated_spacing, undistributable))
}

fn table_size_to_grid_size(
    table_size: f32,
    box_sizing: TableBoxSizing,
    table_offsets: f32,
) -> Result<f32, TableInlineSizingError> {
    let grid_size = match box_sizing {
        TableBoxSizing::ContentBox => table_size + table_offsets,
        TableBoxSizing::BorderBox => table_size,
    };
    (grid_size.is_finite() && grid_size >= 0.0)
        .then_some(grid_size)
        .ok_or(TableInlineSizingError::InvalidResultSize)
}

fn grid_size_to_table_size(
    grid_size: f32,
    box_sizing: TableBoxSizing,
    table_offsets: f32,
) -> Result<f32, TableInlineSizingError> {
    let table_size = match box_sizing {
        TableBoxSizing::ContentBox => grid_size - table_offsets,
        TableBoxSizing::BorderBox => grid_size,
    };
    (table_size.is_finite() && table_size >= 0.0)
        .then_some(table_size)
        .ok_or(TableInlineSizingError::InvalidResultSize)
}

fn resolve_fixed_constraint(
    constraint: InlineSizeConstraint,
    table_width: f32,
    box_id: Option<BoxId>,
) -> Result<Option<f32>, TableInlineSizingError> {
    constraint.resolve_definite(Some(table_width), box_id, TableInlineProperty::Width)
}

fn fixed_cell_border_box_size(
    measure: &TableCellInlineMeasure,
    table_width: f32,
) -> Result<Option<f32>, TableInlineSizingError> {
    let Some(width) =
        resolve_fixed_constraint(measure.preferred, table_width, Some(measure.box_id))?
    else {
        return Ok(None);
    };
    let size = match measure.box_sizing {
        TableBoxSizing::ContentBox => {
            // Fixed layout knows the table width before it distributes columns,
            // so a cell padding percentage has a real basis here. It is the
            // same basis the cell's own width constraint resolves against.
            width
                + measure.offsets.total(table_width).ok_or(
                    TableInlineSizingError::InvalidOffsets {
                        box_id: measure.box_id,
                    },
                )?
        },
        TableBoxSizing::BorderBox => width,
    };
    (size.is_finite() && size >= 0.0)
        .then_some(Some(size))
        .ok_or(TableInlineSizingError::InvalidResultSize)
}

fn apply_range_minimum(
    column_sizes: &mut [Option<f32>],
    start: usize,
    span: usize,
    required: f32,
) -> Result<(), TableInlineSizingError> {
    let Some(end) = start
        .checked_add(span)
        .filter(|end| *end <= column_sizes.len())
    else {
        return Err(TableInlineSizingError::InvalidColumnGroupRange { start, span });
    };
    let current = column_sizes[start..end].iter().flatten().sum::<f32>();
    if current >= required {
        return Ok(());
    }
    let unresolved = (start..end)
        .filter(|index| column_sizes[*index].is_none())
        .collect::<Vec<_>>();
    let increase = required - current;
    if unresolved.is_empty() {
        let share = increase / span as f32;
        for size in &mut column_sizes[start..end] {
            *size = Some(size.unwrap_or(0.0) + share);
        }
    } else {
        let share = increase / unresolved.len() as f32;
        for index in unresolved {
            column_sizes[index] = Some(share);
        }
    }
    Ok(())
}

fn apply_first_row_cell(
    column_sizes: &mut [Option<f32>],
    start: usize,
    span: usize,
    cell_size: f32,
) -> Result<(), TableInlineSizingError> {
    let Some(end) = start
        .checked_add(span)
        .filter(|end| *end <= column_sizes.len())
    else {
        return Err(TableInlineSizingError::InvalidColumnGroupRange { start, span });
    };
    let share = cell_size / span as f32;
    for size in &mut column_sizes[start..end] {
        if size.is_none() {
            *size = Some(share);
        }
    }
    Ok(())
}

fn distribute_excess(column_sizes: &mut [Option<f32>], excess: f32) {
    if excess <= 0.0 {
        return;
    }
    let share = excess / column_sizes.len() as f32;
    for size in column_sizes {
        *size = Some(size.unwrap_or(0.0) + share);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AffineLengthPercentage, BoxGeneration, BoxOrigin, BoxTreeInput, CssBoxTree, DisplayInside,
        DisplayOutside, DisplayRole, InternalTableRole, PositioningScheme, TableCellInput,
        TableGrid, TableGridInputs, TableRowSpan, TableSeparatedBorderMetrics,
        TableTrackVisibility, generate_box_tree,
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
            crate::FlowAxes::HORIZONTAL_LTR,
            PositioningScheme::Static,
            false,
            children,
        )
    }

    fn grid(first_cell_spans_two_columns: bool) -> TableGrid {
        let first_row_cells = if first_cell_spans_two_columns {
            vec![
                node(8, InternalTableRole::Cell, vec![]),
                node(9, InternalTableRole::Cell, vec![]),
            ]
        } else {
            vec![
                node(8, InternalTableRole::Cell, vec![]),
                node(9, InternalTableRole::Cell, vec![]),
                node(10, InternalTableRole::Cell, vec![]),
            ]
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
            crate::FlowAxes::HORIZONTAL_LTR,
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
                node(5, InternalTableRole::Column, vec![]),
                node(
                    6,
                    InternalTableRole::RowGroup,
                    vec![
                        node(7, InternalTableRole::Row, first_row_cells),
                        node(
                            11,
                            InternalTableRole::Row,
                            vec![
                                node(12, InternalTableRole::Cell, vec![]),
                                node(13, InternalTableRole::Cell, vec![]),
                                node(14, InternalTableRole::Cell, vec![]),
                            ],
                        ),
                    ],
                ),
            ],
        )]);
        let table = tree.principal_box(1).expect("table grid");
        let mut inputs = TableGridInputs::default();
        if first_cell_spans_two_columns {
            inputs.set_cell(
                tree.principal_box(8).expect("first cell"),
                TableCellInput {
                    column_span: 2,
                    row_span: TableRowSpan::Count(1),
                },
            );
        }
        TableGrid::from_box_tree(&tree, table, &inputs)
    }

    fn absolute(value: f32) -> InlineSizeConstraint {
        InlineSizeConstraint::Value(AffineLengthPercentage::new(value, 0.0).expect("finite size"))
    }

    fn constraints(width: f32) -> TableInlineConstraints {
        TableInlineConstraints {
            preferred: absolute(width),
            ..TableInlineConstraints::default()
        }
    }

    fn measure(box_id: BoxId) -> TableCellInlineMeasure {
        TableCellInlineMeasure {
            box_id,
            content: crate::IntrinsicSizes::new(0.0, 0.0).expect("zero intrinsic pair"),
            preferred: InlineSizeConstraint::Auto,
            minimum: InlineSizeConstraint::Auto,
            maximum: InlineSizeConstraint::None,
            box_sizing: TableBoxSizing::ContentBox,
            offsets: super::super::CellInlineOffsets::ZERO,
        }
    }

    fn input(grid: &TableGrid, table_width: f32) -> TableFixedInlineSizingInput<'_> {
        let sizing = TableInlineSizingInput {
            grid,
            available_inline_size: Some(800.0),
            table_constraints: constraints(table_width),
            border_metrics: TableInlineBorderMetrics::Separated(
                TableSeparatedBorderMetrics::default(),
            ),
            caption_min: super::super::CaptionMinContribution::NoCaption,
            track_visibility: TableTrackVisibility::all_visible(grid),
        };
        let mut input = TableFixedInlineSizingInput::new(sizing);
        input.cells = grid.cells.iter().map(|cell| measure(cell.source)).collect();
        input
    }

    fn fixed(input: &TableFixedInlineSizingInput<'_>) -> TableInlineSizingResult {
        match size_fixed_table_inline(input).expect("fixed table sizing") {
            TableFixedInlineSizingOutcome::Fixed(result) => result,
            TableFixedInlineSizingOutcome::Automatic(reason) => {
                panic!("expected fixed sizing, fell back to {reason:?}")
            },
        }
    }

    #[test]
    fn normalized_column_and_group_constraints_precede_first_row_cells() {
        let grid = grid(false);
        let mut input = input(&grid, 300.0);
        input.columns[0].constraints = constraints(80.0);
        input.column_groups[0].constraints = constraints(120.0);
        input.cells[0].preferred = absolute(200.0);

        let result = fixed(&input);
        assert_eq!(result.column_sizes, vec![80.0, 40.0, 180.0]);
        assert_eq!(result.used_table_inline_size, 300.0);
        assert_eq!(result.used_grid_inline_size, 300.0);
    }

    #[test]
    fn first_row_spans_and_cell_box_sizing_use_logical_outer_sizes() {
        let grid = grid(true);
        let mut input = input(&grid, 170.0);
        input.cells[0].preferred = absolute(90.0);
        input.cells[0].offsets = super::super::CellInlineOffsets {
            padding_start: AffineLengthPercentage::px(2.0),
            padding_end: AffineLengthPercentage::px(3.0),
            border_start: 1.0,
            border_end: 4.0,
        };
        input.cells[1].preferred = absolute(70.0);
        input.cells[1].box_sizing = TableBoxSizing::BorderBox;

        let result = fixed(&input);
        assert_eq!(result.column_sizes, vec![50.0, 50.0, 70.0]);
        assert_eq!(result.used_table_inline_size, 170.0);
    }

    #[test]
    fn a_cell_padding_percentage_resolves_against_the_table_width() {
        let grid = grid(true);
        // 1% and 1.5% of a 200px table are the same 2px and 3px.
        let percentage = super::super::CellInlineOffsets {
            padding_start: AffineLengthPercentage::new(0.0, 0.01).expect("finite percentage"),
            padding_end: AffineLengthPercentage::new(0.0, 0.015).expect("finite percentage"),
            border_start: 1.0,
            border_end: 4.0,
        };
        let absolute_equivalent = super::super::CellInlineOffsets {
            padding_start: AffineLengthPercentage::px(2.0),
            padding_end: AffineLengthPercentage::px(3.0),
            border_start: 1.0,
            border_end: 4.0,
        };
        assert!(percentage.needs_percentage_basis());
        assert!(!absolute_equivalent.needs_percentage_basis());

        let mut percentage_input = input(&grid, 200.0);
        percentage_input.cells[0].preferred = absolute(90.0);
        percentage_input.cells[0].offsets = percentage;
        let mut absolute_input = input(&grid, 200.0);
        absolute_input.cells[0].preferred = absolute(90.0);
        absolute_input.cells[0].offsets = absolute_equivalent;

        let from_percentage = fixed(&percentage_input);
        let from_absolute = fixed(&absolute_input);
        assert_eq!(from_percentage.column_sizes, from_absolute.column_sizes);
        assert_eq!(
            from_percentage.used_table_inline_size,
            from_absolute.used_table_inline_size
        );
    }

    #[test]
    fn unresolved_columns_share_table_space_after_separated_metrics_once() {
        let grid = grid(false);
        let mut input = input(&grid, 104.0);
        input.sizing.border_metrics =
            TableInlineBorderMetrics::Separated(TableSeparatedBorderMetrics {
                table_offsets: super::super::CellInlineOffsets {
                    padding_start: AffineLengthPercentage::px(1.0),
                    padding_end: AffineLengthPercentage::px(1.0),
                    border_start: 1.0,
                    border_end: 1.0,
                },
                inline_spacing: 5.0,
            });

        let result = fixed(&input);
        assert_eq!(result.column_sizes, vec![28.0, 28.0, 28.0]);
        assert_eq!(result.used_table_inline_size, 104.0);
        assert_eq!(result.used_grid_inline_size, 108.0);
    }

    #[test]
    fn fixed_contributions_expand_a_too_small_table_and_extra_space_is_deterministic() {
        let grid = grid(false);
        let mut narrow = input(&grid, 200.0);
        for column in &mut narrow.columns {
            column.constraints = constraints(100.0);
        }
        let narrow_result = fixed(&narrow);
        assert_eq!(narrow_result.column_sizes, vec![100.0, 100.0, 100.0]);
        assert_eq!(narrow_result.used_table_inline_size, 300.0);

        let mut wide = input(&grid, 420.0);
        for column in &mut wide.columns {
            column.constraints = constraints(100.0);
        }
        let wide_result = fixed(&wide);
        assert_eq!(wide_result.column_sizes, vec![140.0, 140.0, 140.0]);
        assert_eq!(wide_result.used_table_inline_size, 420.0);
    }

    #[test]
    fn table_minimum_and_subpixel_remainder_remain_in_logical_column_order() {
        let grid = grid(false);
        let mut minimum = input(&grid, 120.0);
        minimum.sizing.table_constraints.minimum = absolute(240.0);
        assert_eq!(fixed(&minimum).column_sizes, vec![80.0, 80.0, 80.0]);

        let fractional = input(&grid, 101.0);
        let result = fixed(&fractional);
        assert!((result.column_sizes.iter().sum::<f32>() - 101.0).abs() < 0.001);
        assert!(
            result
                .column_sizes
                .iter()
                .all(|width| (*width - 101.0 / 3.0).abs() < 0.001)
        );
    }

    #[test]
    fn later_rows_cannot_change_fixed_column_sizes() {
        let grid = grid(false);
        let baseline = fixed(&input(&grid, 300.0));
        let mut with_later_width = input(&grid, 300.0);
        let later = with_later_width
            .cells
            .iter()
            .enumerate()
            .find(|(index, _)| grid.cells[*index].row == 1)
            .map(|(index, _)| index)
            .expect("later-row cell");
        with_later_width.cells[later].preferred = absolute(1_000.0);

        assert_eq!(fixed(&with_later_width), baseline);
    }

    #[test]
    fn auto_width_falls_back_without_fixed_arithmetic_and_collapsed_metrics_defer() {
        let grid = grid(false);
        let mut automatic = input(&grid, 300.0);
        automatic.sizing.table_constraints.preferred = InlineSizeConstraint::Auto;
        assert_eq!(
            size_fixed_table_inline(&automatic),
            Ok(TableFixedInlineSizingOutcome::Automatic(
                TableFixedLayoutFallback::TableWidthAuto
            ))
        );

        let mut collapsed = input(&grid, 300.0);
        collapsed.sizing.border_metrics = TableInlineBorderMetrics::CollapsedPendingK4g;
        assert_eq!(
            size_fixed_table_inline(&collapsed),
            Err(TableInlineSizingError::Deferral(
                TableDeferral::CollapsedBorderMetricsPendingK4g
            ))
        );
    }
}
