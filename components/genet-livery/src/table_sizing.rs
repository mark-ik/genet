//! Livery's non-live lowering into Buckram's table inline-sizing contracts.
//!
//! The temporary Grid/Flex bridge does not consume this module yet. Keeping the
//! lowering separate makes its CSS inputs reviewable and proves that Buckram
//! receives logical edges and box identity rather than backend layout state.

use buckram::{
    AffineLengthPercentage, CellBlockOffsets, CellInlineOffsets, CollapsedBorderMetricError,
    CollapsedBorderMetrics, FlowAxes, InlineSizeConstraint, PhysicalSide, ResolvedTableBorderGrid,
    TableAutomaticColumnGroupInput, TableAutomaticColumnInput, TableBlockConstraint,
    TableBorderError, TableBorderOrderKey, TableBorderOrigin, TableBorderResolutionError,
    TableBorderSides, TableBorderSource, TableBorderSources, TableBorderStyle, TableBoxSizing,
    TableCellAlignment, TableCellBlockStyle, TableCellInlineStyle, TableCollapsedBorderMetrics,
    TableDeferral, TableFixedColumnGroupInput, TableFixedColumnInput, TableGrid,
    TableInlineConstraints, TableInlineProperty, TableInlineSizingError,
    collect_table_border_candidates, project_collapsed_border_metrics,
};
use livery::{
    ComputedValues,
    values::{
        BorderStyle, BorderWidth, BoxSizing, ComputedColor, LengthPercentage, Size, VerticalAlign,
    },
};

use crate::{box_tree::GeneratedBoxTree, layout::border_width_px, style::StylePlane};

/// Lower a computed cell style into logical table sizing data. The caller must
/// supply the already-computed local and root font sizes. A percentage padding
/// or a non-affine size remains explicit until the sizing algorithm knows its
/// basis.
pub(crate) fn table_cell_inline_style(
    computed: &ComputedValues,
    axes: FlowAxes,
    font_size: f32,
    root_font_size: f32,
) -> Result<TableCellInlineStyle, TableInlineSizingError> {
    Ok(TableCellInlineStyle {
        constraints: table_inline_constraints(computed, font_size, root_font_size),
        offsets: CellInlineOffsets {
            padding_start: logical_padding(
                computed,
                axes.inline_start(),
                font_size,
                root_font_size,
                TableInlineProperty::PaddingInlineStart,
            )?,
            padding_end: logical_padding(
                computed,
                axes.inline_end(),
                font_size,
                root_font_size,
                TableInlineProperty::PaddingInlineEnd,
            )?,
            border_start: logical_border(computed, axes.inline_start(), font_size),
            border_end: logical_border(computed, axes.inline_end(), font_size),
        },
    })
}

/// The retained B1 winner grid together with B2's metric projection. Neither
/// is sizing or paint input yet: K4g4 consumes metrics and K4g5 owns geometry.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CollapsedTableBorders {
    pub winners: ResolvedTableBorderGrid<ComputedColor>,
    pub metrics: CollapsedBorderMetrics,
}

/// Lower the table box's own collapsed-model inline geometry. Its padding is
/// still authored by the table, while K4g3's outer winners replace its
/// declared borders and `border-spacing` entirely.
pub(crate) fn collapsed_table_inline_metrics(
    computed: &ComputedValues,
    axes: FlowAxes,
    font_size: f32,
    root_font_size: f32,
    metrics: &CollapsedBorderMetrics,
) -> Result<TableCollapsedBorderMetrics, TableInlineSizingError> {
    let mut table_padding =
        table_cell_inline_style(computed, axes, font_size, root_font_size)?.offsets;
    table_padding.border_start = 0.0;
    table_padding.border_end = 0.0;
    let outer_start = metrics.table_outer.inline_start;
    let outer_end = metrics.table_outer.inline_end;
    if !outer_start.is_finite() || outer_start < 0.0 || !outer_end.is_finite() || outer_end < 0.0 {
        return Err(TableInlineSizingError::InvalidBorderMetrics);
    }
    Ok(TableCollapsedBorderMetrics {
        table_padding,
        outer_start,
        outer_end,
    })
}

/// Replace a cell's declared inline borders with the B2 winners that meet its
/// four grid sides. Padding and CSS size constraints remain authored values.
pub(crate) fn collapsed_cell_inline_style(
    computed: &ComputedValues,
    axes: FlowAxes,
    font_size: f32,
    root_font_size: f32,
    metrics: &CollapsedBorderMetrics,
    cell: buckram::BoxId,
) -> Result<TableCellInlineStyle, TableInlineSizingError> {
    let mut style = table_cell_inline_style(computed, axes, font_size, root_font_size)?;
    let Some(cell_metrics) = metrics.cell_offsets.iter().find(|entry| entry.cell == cell) else {
        return Err(TableInlineSizingError::InvalidOffsets { box_id: cell });
    };
    style.offsets.border_start = cell_metrics.sides.inline_start.projected_half_width;
    style.offsets.border_end = cell_metrics.sides.inline_end.projected_half_width;
    if !style.offsets.is_valid() {
        return Err(TableInlineSizingError::InvalidOffsets { box_id: cell });
    }
    Ok(style)
}

/// The block-axis counterpart of [`collapsed_cell_inline_style`].
pub(crate) fn collapsed_cell_block_style(
    computed: &ComputedValues,
    axes: FlowAxes,
    font_size: f32,
    root_font_size: f32,
    metrics: &CollapsedBorderMetrics,
    cell: buckram::BoxId,
) -> Result<TableCellBlockStyle, TableInlineSizingError> {
    let mut style = table_cell_block_style(computed, axes, font_size, root_font_size)?;
    let Some(cell_metrics) = metrics.cell_offsets.iter().find(|entry| entry.cell == cell) else {
        return Err(TableInlineSizingError::InvalidOffsets { box_id: cell });
    };
    style.offsets.border_start = cell_metrics.sides.block_start.projected_half_width;
    style.offsets.border_end = cell_metrics.sides.block_end.projected_half_width;
    if style.offsets.total().is_none() {
        return Err(TableInlineSizingError::InvalidOffsets { box_id: cell });
    }
    Ok(style)
}

/// Lower every physical computed border side into the table's logical axes,
/// collect K4g1's candidates, resolve K4g2's winners, then project K4g3's
/// metric seam.
pub(crate) fn collapsed_table_borders<Id>(
    boxes: &GeneratedBoxTree<Id>,
    styles: &StylePlane<Id>,
    grid: &TableGrid,
    table: buckram::BoxId,
    table_computed: &ComputedValues,
    font_size: f32,
) -> Result<CollapsedTableBorders, CollapsedBorderLoweringError>
where
    Id: Copy + Eq + std::hash::Hash,
{
    let axes = boxes[table].flow;
    let default = ComputedValues::default();
    let style_of = |source| {
        boxes
            .origin_node(source)
            .and_then(|node| styles.get(node))
            .unwrap_or(&default)
    };
    let source = |source, origin, order, computed: &ComputedValues| TableBorderSource {
        source,
        origin,
        sides: table_border_sides(computed, axes, font_size),
        order,
    };
    let table = source(
        table,
        TableBorderOrigin::Table,
        table_border_order_key(grid, axes, grid.grid),
        table_computed,
    );
    let row_groups = grid
        .row_groups
        .iter()
        .map(|group| {
            source(
                group.source,
                TableBorderOrigin::RowGroup,
                table_border_order_key(grid, axes, group.source),
                style_of(group.source),
            )
        })
        .collect::<Vec<_>>();
    let rows = grid
        .rows
        .iter()
        .map(|track| {
            let source_id = track.source.unwrap_or(grid.grid);
            let order = table_border_order_for_position(grid, axes, track.index, 0);
            source(
                source_id,
                TableBorderOrigin::Row,
                order,
                track.source.map_or(&default, style_of),
            )
        })
        .collect::<Vec<_>>();
    let column_groups = grid
        .column_groups
        .iter()
        .map(|group| {
            source(
                group.source,
                TableBorderOrigin::ColumnGroup,
                table_border_order_key(grid, axes, group.source),
                style_of(group.source),
            )
        })
        .collect::<Vec<_>>();
    let columns = grid
        .columns
        .iter()
        .map(|track| {
            let source_id = track.source.unwrap_or(grid.grid);
            let order = table_border_order_for_position(grid, axes, 0, track.index);
            source(
                source_id,
                TableBorderOrigin::Column,
                order,
                track.source.map_or(&default, style_of),
            )
        })
        .collect::<Vec<_>>();
    let cells = grid
        .cells
        .iter()
        .map(|cell| {
            source(
                cell.source,
                TableBorderOrigin::Cell,
                table_border_order_key(grid, axes, cell.source),
                style_of(cell.source),
            )
        })
        .collect::<Vec<_>>();
    let candidates = collect_table_border_candidates(
        grid,
        &crate::table_shadow::track_visibility(boxes, styles, grid),
        TableBorderSources {
            table,
            row_groups,
            rows,
            column_groups,
            columns,
            cells,
        },
    )?;
    let winners = candidates.resolve()?;
    let metrics = project_collapsed_border_metrics(grid, &winners)?;
    Ok(CollapsedTableBorders { winners, metrics })
}

/// A lowering failure remains distinct from the normal K4g sizing deferral.
/// B2 retains the metric model while K4g4 remains its first sizing consumer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollapsedBorderLoweringError {
    Candidates(TableBorderError),
    Resolution(TableBorderResolutionError),
    Metrics(CollapsedBorderMetricError),
}

impl From<TableBorderError> for CollapsedBorderLoweringError {
    fn from(error: TableBorderError) -> Self {
        Self::Candidates(error)
    }
}

impl From<TableBorderResolutionError> for CollapsedBorderLoweringError {
    fn from(error: TableBorderResolutionError) -> Self {
        Self::Resolution(error)
    }
}

impl From<CollapsedBorderMetricError> for CollapsedBorderLoweringError {
    fn from(error: CollapsedBorderMetricError) -> Self {
        Self::Metrics(error)
    }
}

fn table_border_sides(
    computed: &ComputedValues,
    axes: FlowAxes,
    font_size: f32,
) -> TableBorderSides<ComputedColor> {
    TableBorderSides {
        inline_start: physical_table_border(computed, axes.inline_start(), font_size),
        inline_end: physical_table_border(computed, axes.inline_end(), font_size),
        block_start: physical_table_border(computed, axes.block_start(), font_size),
        block_end: physical_table_border(computed, axes.block_end(), font_size),
    }
}

fn physical_table_border(
    computed: &ComputedValues,
    side: PhysicalSide,
    font_size: f32,
) -> (TableBorderStyle, f32, ComputedColor) {
    let (style, width, color) = match side {
        PhysicalSide::Top => (
            computed.border_top_style,
            computed.border_top_width,
            computed.border_top_color.clone(),
        ),
        PhysicalSide::Right => (
            computed.border_right_style,
            computed.border_right_width,
            computed.border_right_color.clone(),
        ),
        PhysicalSide::Bottom => (
            computed.border_bottom_style,
            computed.border_bottom_width,
            computed.border_bottom_color.clone(),
        ),
        PhysicalSide::Left => (
            computed.border_left_style,
            computed.border_left_width,
            computed.border_left_color.clone(),
        ),
    };
    (
        table_border_style(style),
        border_width_px(style, width, font_size),
        color,
    )
}

fn table_border_style(style: BorderStyle) -> TableBorderStyle {
    match style {
        BorderStyle::None => TableBorderStyle::None,
        BorderStyle::Hidden => TableBorderStyle::Hidden,
        BorderStyle::Dotted => TableBorderStyle::Dotted,
        BorderStyle::Dashed => TableBorderStyle::Dashed,
        BorderStyle::Solid => TableBorderStyle::Solid,
        BorderStyle::Double => TableBorderStyle::Double,
        BorderStyle::Groove => TableBorderStyle::Groove,
        BorderStyle::Ridge => TableBorderStyle::Ridge,
        BorderStyle::Inset => TableBorderStyle::Inset,
        BorderStyle::Outset => TableBorderStyle::Outset,
    }
}

/// CSS2's positional tie is defined in table-flow coordinates, not raw
/// physical left and top. Writing mode maps physical sides above; this key
/// then compares logical block-start first and reverses logical inline order
/// for RTL tables.
fn table_border_order_key(
    grid: &TableGrid,
    axes: FlowAxes,
    source: buckram::BoxId,
) -> TableBorderOrderKey {
    if source == grid.grid {
        return TableBorderOrderKey(0);
    }
    if let Some(cell) = grid.cells.iter().find(|cell| cell.source == source) {
        return table_border_order_for_position(grid, axes, cell.row, cell.column);
    }
    if let Some(track) = grid.rows.iter().find(|track| track.source == Some(source)) {
        return table_border_order_for_position(grid, axes, track.index, 0);
    }
    if let Some(group) = grid.row_groups.iter().find(|group| group.source == source) {
        return table_border_order_for_position(grid, axes, group.start, 0);
    }
    if let Some(track) = grid
        .columns
        .iter()
        .find(|track| track.source == Some(source))
    {
        return table_border_order_for_position(grid, axes, 0, track.index);
    }
    if let Some(group) = grid
        .column_groups
        .iter()
        .find(|group| group.source == source)
    {
        return table_border_order_for_position(grid, axes, 0, group.start);
    }
    TableBorderOrderKey(u32::try_from(source.index()).unwrap_or(u32::MAX))
}

fn table_border_order_for_position(
    grid: &TableGrid,
    axes: FlowAxes,
    row: usize,
    column: usize,
) -> TableBorderOrderKey {
    let columns = grid.columns.len().max(1);
    let inline = match axes.direction {
        buckram::Direction::Ltr => column,
        buckram::Direction::Rtl => columns.saturating_sub(1).saturating_sub(column),
    };
    let order = row.saturating_mul(columns).saturating_add(inline);
    TableBorderOrderKey(u32::try_from(order).unwrap_or(u32::MAX))
}

/// Lower a computed cell style into Buckram's block-axis contract.
///
/// Buckram's block-axis offsets are plain lengths, so a percentage padding has
/// to resolve here or not at all. A table cell's containing block for that
/// percentage is the table box, whose content width Livery cannot name from
/// K4c's result alone: the undistributable remainder folds the table's own
/// padding and border together with separated spacing. So a percentage defers
/// under the same named gap the inline axis uses, and the ledger counts it.
/// Picking one of the two plausible bases here would be exactly the invented
/// geometry this boundary exists to prevent.
pub(crate) fn table_cell_block_style(
    computed: &ComputedValues,
    axes: FlowAxes,
    font_size: f32,
    root_font_size: f32,
) -> Result<TableCellBlockStyle, TableInlineSizingError> {
    Ok(TableCellBlockStyle {
        alignment: table_cell_alignment(computed),
        offsets: CellBlockOffsets {
            padding_start: absolute_padding(
                computed,
                axes.block_start(),
                font_size,
                root_font_size,
                TableInlineProperty::PaddingInlineStart,
            )?,
            padding_end: absolute_padding(
                computed,
                axes.block_end(),
                font_size,
                root_font_size,
                TableInlineProperty::PaddingInlineEnd,
            )?,
            border_start: logical_border(computed, axes.block_start(), font_size),
            border_end: logical_border(computed, axes.block_end(), font_size),
        },
        specified: block_size_constraint(computed.height, font_size, root_font_size),
        box_sizing: match computed.box_sizing {
            BoxSizing::ContentBox => TableBoxSizing::ContentBox,
            BoxSizing::BorderBox => TableBoxSizing::BorderBox,
        },
        // Filled in by the caller, which can see the cell's descendants.
        percentage_dependent_contents: false,
    })
}

/// Lower a computed block-axis size into Buckram's constraint. A percentage
/// travels unresolved: only the table's own specified block size is a valid
/// basis for it, and K4d4 owns that decision.
pub(crate) fn block_size_constraint(
    value: Size,
    font_size: f32,
    root_font_size: f32,
) -> TableBlockConstraint {
    match value {
        // `none` is `max-height`'s initial value and is not a constraint.
        Size::Auto | Size::None => TableBlockConstraint::Auto,
        // No intrinsic keyword gives a table row or cell a definite block
        // size, and CSS 2.1 defines no behavior for them here. Treating one
        // as automatic would silently drop an author's declaration.
        Size::MinContent | Size::MaxContent | Size::FitContent(_) => {
            TableBlockConstraint::Unreduced
        },
        Size::Value(value) => affine_length_percentage(value, font_size, root_font_size)
            .map_or(TableBlockConstraint::Unreduced, TableBlockConstraint::Value),
    }
}

/// One padding edge as a plain length. A percentage has no basis Livery can
/// name, so it defers rather than sampling at zero.
fn absolute_padding(
    computed: &ComputedValues,
    side: PhysicalSide,
    font_size: f32,
    root_font_size: f32,
    property: TableInlineProperty,
) -> Result<f32, TableInlineSizingError> {
    let value = logical_padding(computed, side, font_size, root_font_size, property)?;
    if value.needs_percentage_basis() {
        return Err(TableInlineSizingError::Deferral(
            TableDeferral::PercentagePaddingPendingBasis,
        ));
    }
    value
        .resolve(0.0)
        .filter(|resolved| resolved.is_finite() && *resolved >= 0.0)
        .ok_or(TableInlineSizingError::InvalidConstraint {
            box_id: None,
            property,
        })
}

/// Lower `vertical-align` to a table cell's block-axis alignment.
///
/// CSS 2.1 section 17.5.3 gives table cells only `baseline`, `top`,
/// `middle`, and `bottom`. The remaining values do not apply to a table cell
/// and behave as `baseline`, so they collapse here rather than reaching
/// Buckram as distinctions the table algorithm would have to ignore.
pub(crate) fn table_cell_alignment(computed: &ComputedValues) -> TableCellAlignment {
    match computed.vertical_align {
        VerticalAlign::Top => TableCellAlignment::Top,
        VerticalAlign::Middle => TableCellAlignment::Middle,
        VerticalAlign::Bottom => TableCellAlignment::Bottom,
        VerticalAlign::Baseline
        | VerticalAlign::Sub
        | VerticalAlign::Super
        | VerticalAlign::TextTop
        | VerticalAlign::TextBottom
        | VerticalAlign::Length(_) => TableCellAlignment::Baseline,
    }
}

/// Lower the width, min-width, max-width, and box-sizing values shared by a
/// table grid and a cell. A table adapter will consume the same contract in
/// K4c5, without moving this CSS interpretation into Buckram.
pub(crate) fn table_inline_constraints(
    computed: &ComputedValues,
    font_size: f32,
    root_font_size: f32,
) -> TableInlineConstraints {
    TableInlineConstraints {
        preferred: inline_size_constraint(computed.width, font_size, root_font_size),
        minimum: inline_size_constraint(computed.min_width, font_size, root_font_size),
        maximum: inline_size_constraint(computed.max_width, font_size, root_font_size),
        box_sizing: match computed.box_sizing {
            BoxSizing::ContentBox => TableBoxSizing::ContentBox,
            BoxSizing::BorderBox => TableBoxSizing::BorderBox,
        },
    }
}

/// Lower explicit K4b column and column-group boxes in their normalized table
/// order. Implicit columns deliberately retain automatic constraints. This is
/// a non-live adapter seam for K4c2: neither DOM traversal nor backend grid
/// tracks can influence the Buckram fixed-sizing input.
pub(crate) fn fixed_table_track_inputs(
    grid: &TableGrid,
    mut constraints_for: impl FnMut(buckram::BoxId) -> TableInlineConstraints,
) -> (Vec<TableFixedColumnInput>, Vec<TableFixedColumnGroupInput>) {
    let columns = grid
        .columns
        .iter()
        .map(|column| TableFixedColumnInput {
            source: column.source,
            constraints: column.source.map(&mut constraints_for).unwrap_or_default(),
        })
        .collect();
    let column_groups = grid
        .column_groups
        .iter()
        .map(|group| TableFixedColumnGroupInput {
            source: group.source,
            constraints: constraints_for(group.source),
        })
        .collect();
    (columns, column_groups)
}

/// Lower explicit K4b column and column-group boxes for K4c3's automatic
/// measures. This is a model-only seam: intrinsic aggregation remains in
/// Buckram, and no Grid/Flex bridge state enters the result.
pub(crate) fn automatic_table_track_inputs(
    grid: &TableGrid,
    mut constraints_for: impl FnMut(buckram::BoxId) -> TableInlineConstraints,
) -> (
    Vec<TableAutomaticColumnInput>,
    Vec<TableAutomaticColumnGroupInput>,
) {
    let columns = grid
        .columns
        .iter()
        .map(|column| TableAutomaticColumnInput {
            source: column.source,
            constraints: column.source.map(&mut constraints_for).unwrap_or_default(),
        })
        .collect();
    let column_groups = grid
        .column_groups
        .iter()
        .map(|group| TableAutomaticColumnGroupInput {
            source: group.source,
            constraints: constraints_for(group.source),
        })
        .collect();
    (columns, column_groups)
}

fn inline_size_constraint(size: Size, font_size: f32, root_font_size: f32) -> InlineSizeConstraint {
    match size {
        Size::Auto => InlineSizeConstraint::Auto,
        Size::None => InlineSizeConstraint::None,
        Size::MinContent => InlineSizeConstraint::MinContent,
        Size::MaxContent => InlineSizeConstraint::MaxContent,
        Size::FitContent(value) => affine_length_percentage(value, font_size, root_font_size)
            .map_or(
                InlineSizeConstraint::Unreduced,
                InlineSizeConstraint::FitContent,
            ),
        Size::Value(value) => affine_length_percentage(value, font_size, root_font_size)
            .map_or(InlineSizeConstraint::Unreduced, InlineSizeConstraint::Value),
    }
}

fn affine_length_percentage(
    value: LengthPercentage,
    font_size: f32,
    root_font_size: f32,
) -> Option<AffineLengthPercentage> {
    match value.resolve_font_relative(font_size, root_font_size) {
        LengthPercentage::Zero => Some(AffineLengthPercentage::ZERO),
        LengthPercentage::Length(length) => AffineLengthPercentage::new(
            length.unit.to_px(length.value, font_size, root_font_size),
            0.0,
        ),
        LengthPercentage::Percentage(percentage) => AffineLengthPercentage::new(0.0, percentage),
        LengthPercentage::Calc(calc) => AffineLengthPercentage::new(calc.px, calc.percentage),
        // Non-linear math remains a first-class unresolved constraint. K4c
        // must not sample it at zero merely because no table basis exists.
        LengthPercentage::Math(_) => None,
    }
}

fn logical_padding(
    computed: &ComputedValues,
    side: PhysicalSide,
    font_size: f32,
    root_font_size: f32,
    property: TableInlineProperty,
) -> Result<AffineLengthPercentage, TableInlineSizingError> {
    let value = match side {
        PhysicalSide::Top => computed.padding_top.0,
        PhysicalSide::Right => computed.padding_right.0,
        PhysicalSide::Bottom => computed.padding_bottom.0,
        PhysicalSide::Left => computed.padding_left.0,
    };
    let Some(value) = affine_length_percentage(value, font_size, root_font_size) else {
        return Err(TableInlineSizingError::UnreducedConstraint {
            box_id: None,
            property,
        });
    };
    // The percentage travels to Buckram unresolved. Livery does not own the
    // containing-block basis and must not invent one here.
    (value.absolute >= 0.0 && value.percentage >= 0.0)
        .then_some(value)
        .ok_or(TableInlineSizingError::InvalidConstraint {
            box_id: None,
            property,
        })
}

fn logical_border(computed: &ComputedValues, side: PhysicalSide, font_size: f32) -> f32 {
    let (style, width): (BorderStyle, BorderWidth) = match side {
        PhysicalSide::Top => (computed.border_top_style, computed.border_top_width),
        PhysicalSide::Right => (computed.border_right_style, computed.border_right_width),
        PhysicalSide::Bottom => (computed.border_bottom_style, computed.border_bottom_width),
        PhysicalSide::Left => (computed.border_left_style, computed.border_left_width),
    };
    border_width_px(style, width, font_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buckram::{
        BoxGeneration, BoxOrigin, BoxTreeInput, CssBoxTree, Direction, DisplayInside,
        DisplayOutside, DisplayRole, InternalTableRole, PositioningScheme, TableGridInputs,
        WritingMode, generate_box_tree,
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

    fn k4b_grid() -> TableGrid {
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
        let table = tree.principal_box(1).expect("table grid");
        TableGrid::from_box_tree(&tree, table, &TableGridInputs::default())
    }

    #[test]
    fn computed_size_constraints_preserve_affine_percentages_and_box_sizing() {
        let computed = ComputedValues {
            width: "calc(12px + 40%)".parse().expect("width"),
            min_width: "10px".parse().expect("min width"),
            max_width: "fit-content(90%)".parse().expect("max width"),
            box_sizing: BoxSizing::BorderBox,
            ..ComputedValues::default()
        };

        let style = table_cell_inline_style(&computed, FlowAxes::HORIZONTAL_LTR, 16.0, 16.0)
            .expect("basis-free style lowering");
        assert_eq!(
            style.constraints.preferred,
            InlineSizeConstraint::Value(AffineLengthPercentage::new(12.0, 0.4).unwrap())
        );
        assert_eq!(
            style.constraints.maximum,
            InlineSizeConstraint::FitContent(AffineLengthPercentage::new(0.0, 0.9).unwrap())
        );
        assert_eq!(style.constraints.box_sizing, TableBoxSizing::BorderBox);
    }

    #[test]
    fn logical_edges_follow_writing_direction_before_buckram_receives_them() {
        let computed = ComputedValues {
            padding_left: "1px".parse().expect("left padding"),
            padding_right: "2px".parse().expect("right padding"),
            border_left_style: "solid".parse().expect("left border style"),
            border_left_width: "3px".parse().expect("left border width"),
            border_right_style: "solid".parse().expect("right border style"),
            border_right_width: "4px".parse().expect("right border width"),
            ..ComputedValues::default()
        };

        let ltr = table_cell_inline_style(&computed, FlowAxes::HORIZONTAL_LTR, 16.0, 16.0)
            .expect("LTR style");
        let rtl = table_cell_inline_style(
            &computed,
            FlowAxes::new(WritingMode::HorizontalTb, Direction::Rtl),
            16.0,
            16.0,
        )
        .expect("RTL style");
        assert_eq!(ltr.offsets.padding_start, AffineLengthPercentage::px(1.0));
        assert_eq!(ltr.offsets.border_start, 3.0);
        assert_eq!(rtl.offsets.padding_start, AffineLengthPercentage::px(2.0));
        assert_eq!(rtl.offsets.border_start, 4.0);
    }

    #[test]
    fn percentage_padding_is_carried_unresolved_and_math_stays_unreduced() {
        let mut computed = ComputedValues {
            padding_left: "10%".parse().expect("percentage padding"),
            ..ComputedValues::default()
        };
        let style = table_cell_inline_style(&computed, FlowAxes::HORIZONTAL_LTR, 16.0, 16.0)
            .expect("a padding percentage reaches Buckram unresolved");
        // Livery does not own the containing-block basis, so it neither
        // resolves the percentage nor samples it at zero.
        assert!(style.offsets.needs_percentage_basis());
        assert_eq!(style.offsets.padding_start.absolute, 0.0);
        assert_eq!(style.offsets.padding_start.resolve(200.0), Some(20.0));

        computed.padding_left = "0".parse().expect("zero padding");
        computed.width = "min(10px, 50%)".parse().expect("math width");
        let style = table_cell_inline_style(&computed, FlowAxes::HORIZONTAL_LTR, 16.0, 16.0)
            .expect("unreduced math is retained on the constraint");
        assert_eq!(style.constraints.preferred, InlineSizeConstraint::Unreduced);
    }

    #[test]
    fn collapsed_border_lowering_maps_physical_sides_once_and_reverses_only_the_rtl_tie() {
        let mut computed = ComputedValues::default();
        computed.border_top_style = BorderStyle::Dotted;
        computed.border_top_width = "1px".parse().expect("top width");
        computed.border_top_color = "red".parse().expect("top color");
        computed.border_right_style = BorderStyle::Dashed;
        computed.border_right_width = "2px".parse().expect("right width");
        computed.border_right_color = "green".parse().expect("right color");
        computed.border_bottom_style = BorderStyle::Double;
        computed.border_bottom_width = "3px".parse().expect("bottom width");
        computed.border_bottom_color = "blue".parse().expect("bottom color");
        computed.border_left_style = BorderStyle::Solid;
        computed.border_left_width = "4px".parse().expect("left width");
        computed.border_left_color = "black".parse().expect("left color");

        let horizontal = table_border_sides(&computed, FlowAxes::HORIZONTAL_LTR, 16.0);
        assert_eq!(horizontal.inline_start.0, TableBorderStyle::Solid);
        assert_eq!(horizontal.inline_start.1, 4.0);
        assert_eq!(horizontal.inline_start.2.to_srgb8(), Some((0, 0, 0, 255)));
        assert_eq!(horizontal.block_start.0, TableBorderStyle::Dotted);

        let vertical = table_border_sides(
            &computed,
            FlowAxes::new(WritingMode::VerticalRl, Direction::Ltr),
            16.0,
        );
        assert_eq!(vertical.inline_start.0, TableBorderStyle::Dotted);
        assert_eq!(vertical.block_start.0, TableBorderStyle::Dashed);

        let grid = k4b_grid();
        let first = grid.cells[0].source;
        let second = grid.cells[1].source;
        let ltr_first = table_border_order_key(&grid, FlowAxes::HORIZONTAL_LTR, first);
        let ltr_second = table_border_order_key(&grid, FlowAxes::HORIZONTAL_LTR, second);
        let rtl = FlowAxes::new(WritingMode::HorizontalTb, Direction::Rtl);
        let rtl_first = table_border_order_key(&grid, rtl, first);
        let rtl_second = table_border_order_key(&grid, rtl, second);
        assert!(
            ltr_first < ltr_second,
            "LTR chooses logical inline-start first"
        );
        assert!(rtl_second < rtl_first, "RTL reverses only inline position");
    }

    /// K4d5 lowering: only four `vertical-align` values reach a table cell.
    #[test]
    fn vertical_align_collapses_to_the_four_table_cell_behaviors() {
        let mut computed = ComputedValues::default();
        for (value, expected) in [
            ("top", TableCellAlignment::Top),
            ("middle", TableCellAlignment::Middle),
            ("bottom", TableCellAlignment::Bottom),
            ("baseline", TableCellAlignment::Baseline),
            // CSS 2.1 section 17.5.3: these do not apply to a table cell.
            ("sub", TableCellAlignment::Baseline),
            ("super", TableCellAlignment::Baseline),
            ("text-top", TableCellAlignment::Baseline),
            ("text-bottom", TableCellAlignment::Baseline),
            ("12px", TableCellAlignment::Baseline),
            ("40%", TableCellAlignment::Baseline),
        ] {
            computed.vertical_align = value.parse().expect(value);
            assert_eq!(table_cell_alignment(&computed), expected, "{value}");
        }
    }

    #[test]
    fn fixed_track_lowering_preserves_k4b_order_and_box_identity() {
        let grid = k4b_grid();
        let (columns, groups) = fixed_table_track_inputs(&grid, |source| TableInlineConstraints {
            preferred: InlineSizeConstraint::Value(
                AffineLengthPercentage::new(source.index() as f32, 0.0).expect("finite width"),
            ),
            ..TableInlineConstraints::default()
        });

        assert_eq!(columns.len(), grid.columns.len());
        assert_eq!(groups.len(), grid.column_groups.len());
        assert_eq!(
            columns
                .iter()
                .map(|column| column.source)
                .collect::<Vec<_>>(),
            grid.columns
                .iter()
                .map(|column| column.source)
                .collect::<Vec<_>>()
        );
        assert_eq!(groups[0].source, grid.column_groups[0].source);
        assert_eq!(
            columns[0].constraints.preferred,
            InlineSizeConstraint::Value(
                AffineLengthPercentage::new(columns[0].source.unwrap().index() as f32, 0.0)
                    .unwrap()
            )
        );
    }

    #[test]
    fn automatic_track_lowering_preserves_k4b_order_and_box_identity() {
        let grid = k4b_grid();
        let (columns, groups) =
            automatic_table_track_inputs(&grid, |source| TableInlineConstraints {
                preferred: InlineSizeConstraint::Value(
                    AffineLengthPercentage::new(source.index() as f32, 0.0).expect("finite width"),
                ),
                ..TableInlineConstraints::default()
            });

        assert_eq!(
            columns
                .iter()
                .map(|column| column.source)
                .collect::<Vec<_>>(),
            grid.columns
                .iter()
                .map(|column| column.source)
                .collect::<Vec<_>>(),
        );
        assert_eq!(groups.len(), grid.column_groups.len());
        assert_eq!(groups[0].source, grid.column_groups[0].source);
        assert_eq!(
            columns[0].constraints.preferred,
            InlineSizeConstraint::Value(
                AffineLengthPercentage::new(columns[0].source.unwrap().index() as f32, 0.0)
                    .unwrap()
            )
        );
    }
}
