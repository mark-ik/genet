//! Livery's non-live lowering into Buckram's table inline-sizing contracts.
//!
//! The temporary Grid/Flex bridge does not consume this module yet. Keeping the
//! lowering separate makes its CSS inputs reviewable and proves that Buckram
//! receives logical edges and box identity rather than backend layout state.

use buckram::{
    AffineLengthPercentage, CellInlineOffsets, FlowAxes, InlineSizeConstraint, PhysicalSide,
    TableBoxSizing, TableCellInlineStyle, TableFixedColumnGroupInput, TableFixedColumnInput,
    TableGrid, TableInlineConstraints, TableInlineProperty, TableInlineSizingError,
};
use livery::{
    ComputedValues,
    values::{BorderStyle, BorderWidth, BoxSizing, LengthPercentage, Size},
};

use crate::layout::border_width_px;

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
) -> Result<f32, TableInlineSizingError> {
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
    if value.needs_percentage_basis() {
        return Err(TableInlineSizingError::UnresolvedPercentageBasis {
            box_id: None,
            property,
        });
    }
    value.resolve(0.0).filter(|value| *value >= 0.0).ok_or(
        TableInlineSizingError::InvalidConstraint {
            box_id: None,
            property,
        },
    )
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
        let mut computed = ComputedValues::default();
        computed.width = "calc(12px + 40%)".parse().expect("width");
        computed.min_width = "10px".parse().expect("min width");
        computed.max_width = "fit-content(90%)".parse().expect("max width");
        computed.box_sizing = BoxSizing::BorderBox;

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
        let mut computed = ComputedValues::default();
        computed.padding_left = "1px".parse().expect("left padding");
        computed.padding_right = "2px".parse().expect("right padding");
        computed.border_left_style = "solid".parse().expect("left border style");
        computed.border_left_width = "3px".parse().expect("left border width");
        computed.border_right_style = "solid".parse().expect("right border style");
        computed.border_right_width = "4px".parse().expect("right border width");

        let ltr = table_cell_inline_style(&computed, FlowAxes::HORIZONTAL_LTR, 16.0, 16.0)
            .expect("LTR style");
        let rtl = table_cell_inline_style(
            &computed,
            FlowAxes::new(WritingMode::HorizontalTb, Direction::Rtl),
            16.0,
            16.0,
        )
        .expect("RTL style");
        assert_eq!(ltr.offsets.padding_start, 1.0);
        assert_eq!(ltr.offsets.border_start, 3.0);
        assert_eq!(rtl.offsets.padding_start, 2.0);
        assert_eq!(rtl.offsets.border_start, 4.0);
    }

    #[test]
    fn percentage_padding_and_nonlinear_math_are_not_sampled_at_zero() {
        let mut computed = ComputedValues::default();
        computed.padding_left = "10%".parse().expect("percentage padding");
        assert_eq!(
            table_cell_inline_style(&computed, FlowAxes::HORIZONTAL_LTR, 16.0, 16.0),
            Err(TableInlineSizingError::UnresolvedPercentageBasis {
                box_id: None,
                property: TableInlineProperty::PaddingInlineStart,
            })
        );

        computed.padding_left = "0".parse().expect("zero padding");
        computed.width = "min(10px, 50%)".parse().expect("math width");
        let style = table_cell_inline_style(&computed, FlowAxes::HORIZONTAL_LTR, 16.0, 16.0)
            .expect("unreduced math is retained on the constraint");
        assert_eq!(style.constraints.preferred, InlineSizeConstraint::Unreduced);
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
}
