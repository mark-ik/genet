//! Used-width selection for automatic CSS tables.
//!
//! K4c3 owns basis-free column measures. This module is the next, still
//! model-only, step: it selects a used table width from those measures and
//! assigns that width to logical K4b columns. No completed fragment, Taffy
//! track, or adapter geometry can enter this boundary.

use crate::{IntrinsicSizeCache, IntrinsicSizes, LogicalAxis};

use super::{
    InlineSizeConstraint, TableAutomaticColumnMeasures, TableBoxSizing, TableDeferral,
    TableInlineBorderMetrics, TableInlineProperty, TableInlineSizingError, TableInlineSizingInput,
    TableInlineSizingResult, TableTrackVisibilityState,
};

/// Complete K4c4 input. K4c3's measures retain the K4b logical column order
/// and the sizing input carries only CSS-facing table geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct TableAutomaticInlineSizingInput<'a> {
    pub sizing: TableInlineSizingInput<'a>,
    pub measures: &'a TableAutomaticColumnMeasures,
}

/// Automatic sizing either has a concrete used size or explicitly records why
/// the containing inline basis is not usable yet.
#[derive(Clone, Debug, PartialEq)]
pub enum TableAutomaticInlineSizingOutcome {
    Sized(TableInlineSizingResult),
    Indefinite(TableAutomaticInlineSizingIndefinite),
}

/// An automatic table must not substitute a viewport or a fragment width for
/// one of these missing CSS bases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableAutomaticInlineSizingIndefinite {
    ContainingInlineSize,
    PercentageBasis(TableInlineProperty),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ConstraintResolution {
    Value(Option<f32>),
    Indefinite(TableAutomaticInlineSizingIndefinite),
}

/// Size an automatic-layout table from K4c3's intrinsic column measures.
///
/// The distribution function is the CSS 2 interpolation observed in local
/// Chrome 150 and Firefox 153: after percentage and absolute constrained
/// tracks receive their selected width, the remaining width between intrinsic
/// bounds grows the unqualified tracks in proportion to their
/// `max-content - min-content` slack. Width above that upper guess grows the
/// same eligible tracks in proportion to their max-content measures.
pub fn size_automatic_table_inline(
    input: &TableAutomaticInlineSizingInput<'_>,
) -> Result<TableAutomaticInlineSizingOutcome, TableInlineSizingError> {
    validate(input)?;

    let (table_offsets, undistributable) = separated_metrics(&input.sizing)?;
    let grid_intrinsic_sizes = grid_intrinsic_sizes(input, table_offsets, undistributable)?;
    let caption_min = input.sizing.caption_min.measured()?.unwrap_or(0.0);
    let intrinsic_sizes = IntrinsicSizes::new(
        grid_intrinsic_sizes.min_content.max(caption_min),
        grid_intrinsic_sizes.max_content.max(caption_min),
    )
    .ok_or(TableInlineSizingError::InvalidResultSize)?;

    let available = valid_available_inline_size(input.sizing.available_inline_size)?;
    let preferred = resolve_constraint(
        input.sizing.table_constraints.preferred,
        available,
        grid_intrinsic_sizes,
        TableInlineProperty::Width,
    )?;
    let minimum = resolve_constraint(
        input.sizing.table_constraints.minimum,
        available,
        grid_intrinsic_sizes,
        TableInlineProperty::MinWidth,
    )?;
    let maximum = resolve_constraint(
        input.sizing.table_constraints.maximum,
        available,
        grid_intrinsic_sizes,
        TableInlineProperty::MaxWidth,
    )?;
    let (
        ConstraintResolution::Value(preferred),
        ConstraintResolution::Value(minimum),
        ConstraintResolution::Value(maximum),
    ) = (preferred, minimum, maximum)
    else {
        return Ok(TableAutomaticInlineSizingOutcome::Indefinite(
            [preferred, minimum, maximum]
                .into_iter()
                .find_map(|resolution| match resolution {
                    ConstraintResolution::Indefinite(indefinite) => Some(indefinite),
                    ConstraintResolution::Value(_) => None,
                })
                .expect("the non-value constraint resolution is indefinite"),
        ));
    };

    let Some(requested_table_size) = preferred.or_else(|| {
        available.map(|available| {
            grid_intrinsic_sizes
                .max_content
                .min(available)
                .max(caption_min)
                .max(grid_intrinsic_sizes.min_content)
        })
    }) else {
        return Ok(TableAutomaticInlineSizingOutcome::Indefinite(
            TableAutomaticInlineSizingIndefinite::ContainingInlineSize,
        ));
    };

    let minimum = minimum.unwrap_or(0.0);
    let maximum = maximum.unwrap_or(f32::INFINITY).max(minimum);
    let constrained_table_size = requested_table_size.max(minimum).min(maximum);
    let used_table_inline_size = constrained_table_size
        .max(caption_min)
        .max(grid_intrinsic_sizes.min_content);
    let used_grid_inline_size = table_size_to_grid_size(
        used_table_inline_size,
        input.sizing.table_constraints.box_sizing,
        table_offsets,
    )?;
    let assignable_column_inline_size = (used_grid_inline_size - undistributable).max(0.0);
    let column_sizes = distribute_columns(
        input.measures,
        used_table_inline_size,
        assignable_column_inline_size,
    )?;

    TableInlineSizingResult::new(
        &input.sizing,
        intrinsic_sizes,
        used_table_inline_size,
        used_grid_inline_size,
        column_sizes,
    )
    .map(TableAutomaticInlineSizingOutcome::Sized)
}

/// Publish the table-grid intrinsic pair under the grid's standards-owned
/// identity. A wrapper may still need K4e's caption contribution, so this
/// deliberately caches the grid rather than manufacturing a wrapper answer.
pub fn cache_automatic_table_grid_intrinsic_sizes(
    input: &TableAutomaticInlineSizingInput<'_>,
    cache: &mut IntrinsicSizeCache,
) -> Result<IntrinsicSizes, TableInlineSizingError> {
    validate(input)?;
    let (table_offsets, undistributable) = separated_metrics(&input.sizing)?;
    let intrinsic_sizes = grid_intrinsic_sizes(input, table_offsets, undistributable)?;
    cache.insert(input.sizing.grid.grid, LogicalAxis::Inline, intrinsic_sizes);
    Ok(intrinsic_sizes)
}

fn validate(input: &TableAutomaticInlineSizingInput<'_>) -> Result<(), TableInlineSizingError> {
    if input.measures.columns.len() != input.sizing.grid.columns.len() {
        return Err(TableInlineSizingError::ColumnCountMismatch {
            expected: input.sizing.grid.columns.len(),
            actual: input.measures.columns.len(),
        });
    }
    for measure in input.measures.columns.iter().copied() {
        measure.validate()?;
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
    Ok(())
}

fn separated_metrics(
    sizing: &TableInlineSizingInput<'_>,
) -> Result<(f32, f32), TableInlineSizingError> {
    let TableInlineBorderMetrics::Separated(metrics) = sizing.border_metrics else {
        return Err(TableInlineSizingError::Deferral(
            TableDeferral::CollapsedBorderMetricsPendingK4g,
        ));
    };
    let table_offsets = metrics
        .table_offsets
        .total()
        .ok_or(TableInlineSizingError::InvalidBorderMetrics)?;
    let undistributable = sizing.separated_undistributable_inline_size()?;
    Ok((table_offsets, undistributable))
}

fn grid_intrinsic_sizes(
    input: &TableAutomaticInlineSizingInput<'_>,
    table_offsets: f32,
    undistributable: f32,
) -> Result<IntrinsicSizes, TableInlineSizingError> {
    let minimum = input
        .measures
        .columns
        .iter()
        .map(|column| column.min_content)
        .sum::<f32>()
        + undistributable;
    let maximum = input
        .measures
        .columns
        .iter()
        .map(|column| column.max_content)
        .sum::<f32>()
        + undistributable;
    let minimum = grid_size_to_table_size(
        minimum,
        input.sizing.table_constraints.box_sizing,
        table_offsets,
    )?;
    let maximum = grid_size_to_table_size(
        maximum,
        input.sizing.table_constraints.box_sizing,
        table_offsets,
    )?;
    IntrinsicSizes::new(minimum, maximum).ok_or(TableInlineSizingError::InvalidResultSize)
}

fn valid_available_inline_size(
    available: Option<f32>,
) -> Result<Option<f32>, TableInlineSizingError> {
    match available {
        Some(available) if available.is_finite() && available >= 0.0 => Ok(Some(available)),
        Some(_) => Err(TableInlineSizingError::InvalidResultSize),
        None => Ok(None),
    }
}

fn resolve_constraint(
    constraint: InlineSizeConstraint,
    available: Option<f32>,
    intrinsic_sizes: IntrinsicSizes,
    property: TableInlineProperty,
) -> Result<ConstraintResolution, TableInlineSizingError> {
    let resolve_affine = |value: super::AffineLengthPercentage| {
        let Some(basis) = available.or((!value.needs_percentage_basis()).then_some(0.0)) else {
            return Ok(ConstraintResolution::Indefinite(
                TableAutomaticInlineSizingIndefinite::PercentageBasis(property),
            ));
        };
        value
            .resolve(basis)
            .map(|value| ConstraintResolution::Value(Some(value.max(0.0))))
            .ok_or(TableInlineSizingError::InvalidConstraint {
                box_id: None,
                property,
            })
    };
    match constraint {
        InlineSizeConstraint::Auto | InlineSizeConstraint::None => {
            Ok(ConstraintResolution::Value(None))
        },
        InlineSizeConstraint::MinContent => Ok(ConstraintResolution::Value(Some(
            intrinsic_sizes.min_content,
        ))),
        InlineSizeConstraint::MaxContent => Ok(ConstraintResolution::Value(Some(
            intrinsic_sizes.max_content,
        ))),
        InlineSizeConstraint::Value(value) => resolve_affine(value),
        InlineSizeConstraint::FitContent(value) => match resolve_affine(value)? {
            ConstraintResolution::Value(Some(value)) => Ok(ConstraintResolution::Value(Some(
                value.clamp(intrinsic_sizes.min_content, intrinsic_sizes.max_content),
            ))),
            ConstraintResolution::Value(None) => unreachable!("an affine constraint is definite"),
            ConstraintResolution::Indefinite(indefinite) => {
                Ok(ConstraintResolution::Indefinite(indefinite))
            },
        },
        InlineSizeConstraint::Unreduced => Err(TableInlineSizingError::UnreducedConstraint {
            box_id: None,
            property,
        }),
    }
}

fn distribute_columns(
    measures: &TableAutomaticColumnMeasures,
    used_table_inline_size: f32,
    assignable_column_inline_size: f32,
) -> Result<Vec<f32>, TableInlineSizingError> {
    let mut sizes = measures
        .columns
        .iter()
        .map(|column| column.min_content)
        .collect::<Vec<_>>();
    let minimum_total = sizes.iter().sum::<f32>();
    if minimum_total > assignable_column_inline_size + TableInlineSizingResult::SUBPIXEL_TOLERANCE {
        return Err(TableInlineSizingError::GridSizeMismatch {
            expected: minimum_total,
            actual: assignable_column_inline_size,
        });
    }
    let mut remaining = (assignable_column_inline_size - minimum_total).max(0.0);

    let percentage_demands = measures
        .columns
        .iter()
        .enumerate()
        .filter_map(|(index, column)| {
            let target = (column.percentage * used_table_inline_size).max(0.0);
            (target > sizes[index]).then_some((index, target - sizes[index]))
        })
        .collect::<Vec<_>>();
    remaining = distribute_weighted(&mut sizes, &percentage_demands, remaining);

    let eligible = measures
        .columns
        .iter()
        .enumerate()
        .filter_map(|(index, column)| {
            (!column.constrained && column.percentage == 0.0).then_some(index)
        })
        .collect::<Vec<_>>();
    let eligible = if eligible.is_empty() {
        (0..sizes.len()).collect::<Vec<_>>()
    } else {
        eligible
    };

    let intrinsic_slack = eligible
        .iter()
        .map(|index| {
            (
                *index,
                (measures.columns[*index].max_content - sizes[*index]).max(0.0),
            )
        })
        .collect::<Vec<_>>();
    remaining = distribute_weighted(&mut sizes, &intrinsic_slack, remaining);
    if remaining > 0.0 && !eligible.is_empty() {
        let max_content_weights = eligible
            .iter()
            .map(|index| (*index, measures.columns[*index].max_content.max(0.0)))
            .collect::<Vec<_>>();
        remaining = distribute_weighted(&mut sizes, &max_content_weights, remaining);
    }
    if remaining > TableInlineSizingResult::SUBPIXEL_TOLERANCE {
        return Err(TableInlineSizingError::GridSizeMismatch {
            expected: assignable_column_inline_size,
            actual: sizes.iter().sum(),
        });
    }
    Ok(sizes)
}

/// Assign up to `available` in proportion to the positive requested amounts.
/// The last logical track receives the float remainder, keeping the aggregate
/// sum stable without a physical-direction tie break.
fn distribute_weighted(sizes: &mut [f32], demands: &[(usize, f32)], available: f32) -> f32 {
    let demands = demands
        .iter()
        .copied()
        .filter(|(_, demand)| demand.is_finite() && *demand > 0.0)
        .collect::<Vec<_>>();
    let requested = demands.iter().map(|(_, demand)| demand).sum::<f32>();
    if available <= 0.0 || requested <= 0.0 || demands.is_empty() {
        return available;
    }
    let allocation = available.min(requested);
    let mut remainder = allocation;
    for (position, (index, demand)) in demands.iter().enumerate() {
        let share = if position + 1 == demands.len() {
            remainder
        } else {
            allocation * *demand / requested
        };
        sizes[*index] += share;
        remainder -= share;
    }
    available - allocation
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AffineLengthPercentage, BoxGeneration, BoxOrigin, BoxTreeInput, CssBoxTree, Direction,
        DisplayInside, DisplayOutside, DisplayRole, FlowAxes, InternalTableRole, IntrinsicSizeKind,
        IntrinsicSizeQuery, PositioningScheme, TableGrid, TableGridInputs,
        TableSeparatedBorderMetrics, TableTrackVisibility, WritingMode, generate_box_tree,
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

    fn grid(axes: FlowAxes) -> TableGrid {
        grid_with_column_count(axes, 3)
    }

    fn grid_with_column_count(axes: FlowAxes, column_count: usize) -> TableGrid {
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
                table_role(InternalTableRole::Row),
                axes,
                PositioningScheme::Static,
                false,
                (0..column_count)
                    .map(|index| cell(3 + index as u8))
                    .collect(),
            )],
        )]);
        TableGrid::from_box_tree(
            &tree,
            tree.principal_box(1).expect("table grid"),
            &TableGridInputs::default(),
        )
    }

    fn measures() -> TableAutomaticColumnMeasures {
        TableAutomaticColumnMeasures {
            columns: vec![
                super::super::TableColumnMeasure {
                    min_content: 70.0,
                    max_content: 150.0,
                    percentage: 0.0,
                    constrained: false,
                },
                super::super::TableColumnMeasure {
                    min_content: 50.0,
                    max_content: 200.0,
                    percentage: 0.0,
                    constrained: false,
                },
                super::super::TableColumnMeasure {
                    min_content: 30.0,
                    max_content: 250.0,
                    percentage: 0.0,
                    constrained: false,
                },
            ],
            span_distributions: Vec::new(),
        }
    }

    fn input<'a>(
        grid: &'a TableGrid,
        measures: &'a TableAutomaticColumnMeasures,
    ) -> TableAutomaticInlineSizingInput<'a> {
        TableAutomaticInlineSizingInput {
            sizing: TableInlineSizingInput {
                grid,
                available_inline_size: Some(400.0),
                table_constraints: super::super::TableInlineConstraints::default(),
                border_metrics: TableInlineBorderMetrics::Separated(
                    TableSeparatedBorderMetrics::default(),
                ),
                caption_min: super::super::CaptionMinContribution::NoCaption,
                track_visibility: TableTrackVisibility::all_visible(grid),
            },
            measures,
        }
    }

    fn sized(outcome: TableAutomaticInlineSizingOutcome) -> TableInlineSizingResult {
        match outcome {
            TableAutomaticInlineSizingOutcome::Sized(result) => result,
            TableAutomaticInlineSizingOutcome::Indefinite(reason) => {
                panic!("expected a concrete table size, got {reason:?}")
            },
        }
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.01,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn automatic_used_width_clamps_to_intrinsic_bounds_and_distributes_slack() {
        let grid = grid(FlowAxes::HORIZONTAL_LTR);
        let measures = measures();
        let mut input = input(&grid, &measures);

        for (available, expected_width) in [
            (100.0, 150.0),
            (150.0, 150.0),
            (300.0, 300.0),
            (600.0, 600.0),
            (900.0, 600.0),
        ] {
            input.sizing.available_inline_size = Some(available);
            let result = sized(size_automatic_table_inline(&input).expect("automatic table"));
            assert_close(result.used_table_inline_size, expected_width);
            assert_close(
                result.assignable_column_inline_size,
                result.column_sizes.iter().sum(),
            );
            assert_close(
                result.used_grid_inline_size,
                result.assignable_column_inline_size + result.undistributable_inline_size,
            );
        }

        input.sizing.available_inline_size = Some(300.0);
        let result = sized(size_automatic_table_inline(&input).expect("intermediate table"));
        assert_eq!(result.column_sizes.len(), 3);
        assert_close(result.column_sizes[0], 96.66667);
        assert_close(result.column_sizes[1], 100.0);
        assert_close(result.column_sizes[2], 103.33333);
    }

    #[test]
    fn empty_single_and_many_column_sums_remain_exact() {
        let empty_grid = grid_with_column_count(FlowAxes::HORIZONTAL_LTR, 0);
        let empty_measures = TableAutomaticColumnMeasures {
            columns: Vec::new(),
            span_distributions: Vec::new(),
        };
        let mut empty_input = input(&empty_grid, &empty_measures);
        empty_input.sizing.available_inline_size = Some(0.0);
        let empty = sized(size_automatic_table_inline(&empty_input).expect("empty table"));
        assert_eq!(empty.column_sizes, Vec::<f32>::new());
        assert_close(empty.used_table_inline_size, 0.0);

        let one_grid = grid_with_column_count(FlowAxes::HORIZONTAL_LTR, 1);
        let one_measures = TableAutomaticColumnMeasures {
            columns: vec![super::super::TableColumnMeasure {
                min_content: 20.0,
                max_content: 50.0,
                percentage: 0.0,
                constrained: false,
            }],
            span_distributions: Vec::new(),
        };
        let mut one_input = input(&one_grid, &one_measures);
        one_input.sizing.available_inline_size = Some(35.5);
        let one = sized(size_automatic_table_inline(&one_input).expect("single column table"));
        assert_eq!(one.column_sizes.len(), 1);
        assert_close(one.column_sizes[0], 35.5);
        assert_close(one.column_sizes.iter().sum(), 35.5);
    }

    #[test]
    fn definite_width_caption_and_minimums_are_lower_bounds_even_against_maximum() {
        let grid = grid(FlowAxes::HORIZONTAL_LTR);
        let measures = measures();
        let mut input = input(&grid, &measures);
        input.sizing.table_constraints.preferred =
            InlineSizeConstraint::Value(AffineLengthPercentage::new(120.0, 0.0).expect("width"));
        input.sizing.table_constraints.minimum =
            InlineSizeConstraint::Value(AffineLengthPercentage::new(180.0, 0.0).expect("minimum"));
        input.sizing.table_constraints.maximum =
            InlineSizeConstraint::Value(AffineLengthPercentage::new(160.0, 0.0).expect("maximum"));
        input.sizing.caption_min = super::super::CaptionMinContribution::Measured(220.0);

        let result = sized(size_automatic_table_inline(&input).expect("definite table"));
        assert_close(result.used_table_inline_size, 220.0);
        assert_close(result.intrinsic_sizes.min_content, 220.0);
        assert_close(result.intrinsic_sizes.max_content, 600.0);
    }

    #[test]
    fn intrinsic_keywords_and_affine_fit_content_remain_table_constraints() {
        let grid = grid(FlowAxes::HORIZONTAL_LTR);
        let measures = measures();
        let mut input = input(&grid, &measures);
        input.sizing.available_inline_size = None;

        input.sizing.table_constraints.preferred = InlineSizeConstraint::MinContent;
        assert_close(
            sized(size_automatic_table_inline(&input).expect("min-content table"))
                .used_table_inline_size,
            150.0,
        );
        input.sizing.table_constraints.preferred = InlineSizeConstraint::MaxContent;
        assert_close(
            sized(size_automatic_table_inline(&input).expect("max-content table"))
                .used_table_inline_size,
            600.0,
        );
        input.sizing.available_inline_size = Some(500.0);
        input.sizing.table_constraints.preferred = InlineSizeConstraint::FitContent(
            AffineLengthPercentage::new(20.0, 0.5).expect("fit-content"),
        );
        assert_close(
            sized(size_automatic_table_inline(&input).expect("fit-content table"))
                .used_table_inline_size,
            270.0,
        );
    }

    #[test]
    fn indefinite_and_percentage_bases_are_named_without_a_viewport_substitute() {
        let grid = grid(FlowAxes::HORIZONTAL_LTR);
        let measures = measures();
        let mut input = input(&grid, &measures);
        input.sizing.available_inline_size = None;
        assert_eq!(
            size_automatic_table_inline(&input),
            Ok(TableAutomaticInlineSizingOutcome::Indefinite(
                TableAutomaticInlineSizingIndefinite::ContainingInlineSize,
            ))
        );

        input.sizing.table_constraints.preferred =
            InlineSizeConstraint::Value(AffineLengthPercentage::new(0.0, 0.5).expect("percentage"));
        assert_eq!(
            size_automatic_table_inline(&input),
            Ok(TableAutomaticInlineSizingOutcome::Indefinite(
                TableAutomaticInlineSizingIndefinite::PercentageBasis(TableInlineProperty::Width),
            ))
        );
    }

    #[test]
    fn percentage_and_constrained_tracks_keep_their_selected_width_before_slack() {
        let grid = grid(FlowAxes::HORIZONTAL_LTR);
        let mut measures = measures();
        measures.columns[0].percentage = 0.3;
        measures.columns[1].constrained = true;
        measures.columns[1].min_content = 80.0;
        measures.columns[1].max_content = 80.0;
        let mut input = input(&grid, &measures);
        input.sizing.available_inline_size = Some(400.0);
        let result = sized(size_automatic_table_inline(&input).expect("percentage table"));

        assert_close(result.column_sizes[0], 120.0);
        assert_close(result.column_sizes[1], 80.0);
        assert_close(result.column_sizes[2], 200.0);
        assert_close(result.column_sizes.iter().sum(), 400.0);
    }

    #[test]
    fn overconstrained_percentages_share_the_logical_remainder_and_preserve_the_sum() {
        let grid = grid(FlowAxes::HORIZONTAL_LTR);
        let mut measures = measures();
        measures.columns[0].percentage = 0.6;
        measures.columns[1].percentage = 0.4;
        let mut input = input(&grid, &measures);
        input.sizing.available_inline_size = Some(300.0);
        let result = sized(size_automatic_table_inline(&input).expect("percentage table"));

        assert_close(result.column_sizes[0], 161.66667);
        assert_close(result.column_sizes[1], 108.33333);
        assert_close(result.column_sizes[2], 30.0);
        assert_close(result.column_sizes.iter().sum(), 300.0);
    }

    #[test]
    fn separated_geometry_and_subpixels_keep_all_widths_distinct_and_exact() {
        let grid = grid(FlowAxes::HORIZONTAL_LTR);
        let measures = measures();
        let mut input = input(&grid, &measures);
        input.sizing.available_inline_size = Some(333.33334);
        input.sizing.border_metrics =
            TableInlineBorderMetrics::Separated(TableSeparatedBorderMetrics {
                table_offsets: super::super::CellInlineOffsets {
                    padding_start: 2.0,
                    padding_end: 3.0,
                    border_start: 4.0,
                    border_end: 5.0,
                },
                inline_spacing: 1.5,
            });
        let result = sized(size_automatic_table_inline(&input).expect("separated table"));
        assert_close(result.used_table_inline_size, 333.33334);
        assert_close(result.undistributable_inline_size, 20.0);
        assert_close(result.assignable_column_inline_size, 327.33334);
        assert_close(
            result.column_sizes.iter().sum::<f32>() + result.undistributable_inline_size,
            result.used_grid_inline_size,
        );
    }

    #[test]
    fn grid_intrinsic_sizes_cache_by_grid_identity_while_caption_defers_wrapper_work() {
        let grid = grid(FlowAxes::HORIZONTAL_LTR);
        let measures = measures();
        let mut input = input(&grid, &measures);
        input.sizing.caption_min = super::super::CaptionMinContribution::PendingK4e;
        let mut cache = IntrinsicSizeCache::default();

        assert_eq!(
            cache_automatic_table_grid_intrinsic_sizes(&input, &mut cache),
            Ok(IntrinsicSizes::new(150.0, 600.0).expect("intrinsic pair"))
        );
        assert_eq!(
            cache.get(IntrinsicSizeQuery::new(
                grid.grid,
                LogicalAxis::Inline,
                IntrinsicSizeKind::MinContent,
            )),
            Some(150.0)
        );
        assert_eq!(
            size_automatic_table_inline(&input),
            Err(TableInlineSizingError::Deferral(
                TableDeferral::CaptionMinPendingK4e,
            ))
        );
    }

    #[test]
    fn logical_direction_does_not_reorder_the_same_measures() {
        let measures = measures();
        let ltr = grid(FlowAxes::HORIZONTAL_LTR);
        let rtl = grid(FlowAxes {
            writing_mode: WritingMode::HorizontalTb,
            direction: Direction::Rtl,
        });
        let ltr = sized(size_automatic_table_inline(&input(&ltr, &measures)).expect("ltr"));
        let rtl = sized(size_automatic_table_inline(&input(&rtl, &measures)).expect("rtl"));
        assert_eq!(ltr.column_sizes, rtl.column_sizes);
        assert_eq!(ltr.intrinsic_sizes, rtl.intrinsic_sizes);
    }
}
