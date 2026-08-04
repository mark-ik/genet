//! Splitting one table element's computed values across two boxes.
//!
//! CSS 2.1 section 17.4 names the properties used on the table wrapper box
//! rather than on the table grid box, and CSS Tables 3 section 3.6.1 extends
//! that list with the properties that establish a containing block or a
//! stacking context. The same section names the mechanism for whichever box
//! does not own a property: "Where these values aren't applied to the table
//! grid/wrapper, unset values are used instead." So neither box is written by
//! hand; each is the element's own style with one list copied across.

use livery::{
    ComputedValues, PropertyId,
    values::{Display as CssDisplay, Length, LengthPercentage, Size},
};

/// Properties used on the table wrapper box and not on the table grid box.
///
/// The first group is CSS 2.1 section 17.4 verbatim. The second is CSS
/// Tables 3 section 3.6.1, narrowed to what Livery computes: `clip`,
/// `clip-path`, `filter`, `isolation`, `mask-*`, `mix-blend-mode`,
/// `perspective`, and the `transform-*` longhands past `transform` are all
/// `[[unimplemented]]` in `properties.toml`, so they have no computed value
/// to move. They belong on this list the day Livery grows them.
///
/// `opacity` and `transform` are on it now but are not yet observable: paint
/// reads both from the style plane by DOM node, not from either box. K4e4
/// owns making that selection explicit.
const WRAPPER_PROPERTIES: &[PropertyId] = &[
    // CSS 2.1 section 17.4.
    PropertyId::Position,
    PropertyId::Float,
    PropertyId::MarginTop,
    PropertyId::MarginRight,
    PropertyId::MarginBottom,
    PropertyId::MarginLeft,
    PropertyId::Top,
    PropertyId::Right,
    PropertyId::Bottom,
    PropertyId::Left,
    // CSS Tables 3 section 3.6.1.
    PropertyId::OverflowX,
    PropertyId::OverflowY,
    PropertyId::Opacity,
    PropertyId::Transform,
];

/// The wrapper's half of a table element's computed values.
///
/// The wrapper is anonymous, and CSS 2.1 section 9.2.1.1 would have it
/// inherit from its own parent, which is the table element's parent. It
/// inherits from the *table element* here instead, and the difference is
/// only visible where the table sets an inherited property itself. That case
/// is the one that argues for this choice: a `margin: 2em` written on the
/// table resolves against the table's font size, and the margin is now the
/// wrapper's. Nothing else observes the wrapper's inherited values, because
/// its children are the grid and the captions and each carries its own
/// computed style.
pub(crate) fn wrapper_style(table: &ComputedValues) -> ComputedValues {
    let mut style = ComputedValues::for_child(table);
    for property in WRAPPER_PROPERTIES {
        style.copy_property_from(*property, table);
    }
    // CSS Tables 3 section 2.2.1: the wrapper is `inline-block` for an
    // inline-table and `block` for a table, and establishes a block
    // formatting context either way.
    style.display = match table.display {
        CssDisplay::InlineTable => CssDisplay::InlineBlock,
        _ => CssDisplay::Block,
    };
    style
}

/// The grid's half of a table element's computed values.
///
/// Width, height, borders, padding, and background stay, which is CSS 2.1
/// section 17.4's complement: "all other values of non-inheritable properties
/// are used on the table box and not the table wrapper box".
///
/// `containing` is the *wrapper's* containing block, which the same section
/// makes the basis for a percentage width or height on the table: "Percentages
/// on 'width' and 'height' on the table are relative to the table wrapper
/// box's containing block, not the table wrapper box itself." Resolving them
/// here rather than leaving them to the layout engine is what keeps that rule
/// out of a loop, because the wrapper is itself sized from the grid.
pub(crate) fn grid_style(
    table: &ComputedValues,
    containing: (Option<f32>, Option<f32>),
) -> ComputedValues {
    // Every migrated property is non-inheritable, so `unset` is the initial
    // value and one default style supplies all of them.
    let unset = ComputedValues::default();
    let mut style = table.clone();
    for property in WRAPPER_PROPERTIES {
        style.copy_property_from(*property, &unset);
    }
    style.width = against_the_wrappers_containing_block(style.width, containing.0);
    style.height = against_the_wrappers_containing_block(style.height, containing.1);
    style
}

fn against_the_wrappers_containing_block(size: Size, basis: Option<f32>) -> Size {
    match (size, basis) {
        (Size::Value(LengthPercentage::Percentage(fraction)), Some(basis)) => {
            Size::Value(LengthPercentage::Length(Length::px(fraction * basis)))
        },
        (size, _) => size,
    }
}

#[cfg(test)]
mod tests {
    use livery::values::{Float as CssFloat, Inset, Margin, Padding, Position as CssPosition};

    use super::*;

    fn px(value: f32) -> LengthPercentage {
        LengthPercentage::Length(Length::px(value))
    }

    fn table() -> ComputedValues {
        ComputedValues {
            display: CssDisplay::Table,
            position: CssPosition::Absolute,
            float: CssFloat::Left,
            margin_top: Margin::Value(px(7.0)),
            left: Inset::Value(px(3.0)),
            ..ComputedValues::default()
        }
    }

    #[test]
    fn the_wrapper_takes_the_properties_css_2_1_section_17_4_names() {
        let wrapper = wrapper_style(&table());
        assert_eq!(wrapper.position, CssPosition::Absolute);
        assert_eq!(wrapper.float, CssFloat::Left);
        assert_eq!(wrapper.margin_top, Margin::Value(px(7.0)));
        assert_eq!(wrapper.left, Inset::Value(px(3.0)));
        assert_eq!(wrapper.display, CssDisplay::Block);
    }

    #[test]
    fn the_grid_sees_those_properties_unset_rather_than_zeroed() {
        let unset = ComputedValues::default();
        let grid = grid_style(&table(), (None, None));
        assert_eq!(grid.position, unset.position);
        assert_eq!(grid.float, unset.float);
        assert_eq!(grid.margin_top, unset.margin_top);
        assert_eq!(grid.left, unset.left);
        // Table-ness is not on the list, so the grid stays the table box.
        assert_eq!(grid.display, CssDisplay::Table);
    }

    #[test]
    fn everything_off_the_list_stays_on_the_grid() {
        let mut source = table();
        source.width = Size::Value(px(120.0));
        source.padding_top = Padding(px(5.0));
        let grid = grid_style(&source, (None, None));
        let wrapper = wrapper_style(&source);
        assert_eq!(grid.width, source.width);
        assert_eq!(grid.padding_top, source.padding_top);
        assert_eq!(wrapper.width, ComputedValues::default().width);
        assert_eq!(wrapper.padding_top, ComputedValues::default().padding_top);
    }

    #[test]
    fn a_percentage_table_width_resolves_against_the_wrappers_containing_block() {
        let mut source = table();
        source.width = Size::Value(LengthPercentage::Percentage(0.5));
        source.height = Size::Value(LengthPercentage::Percentage(0.25));
        let grid = grid_style(&source, (Some(200.0), Some(80.0)));
        assert_eq!(grid.width, Size::Value(px(100.0)));
        assert_eq!(grid.height, Size::Value(px(20.0)));
    }

    #[test]
    fn a_percentage_survives_an_indefinite_containing_block() {
        let mut source = table();
        source.width = Size::Value(LengthPercentage::Percentage(0.5));
        let grid = grid_style(&source, (None, None));
        assert_eq!(grid.width, source.width);
    }

    #[test]
    fn an_inline_table_wrapper_is_an_inline_block() {
        let source = ComputedValues {
            display: CssDisplay::InlineTable,
            ..ComputedValues::default()
        };
        assert_eq!(wrapper_style(&source).display, CssDisplay::InlineBlock);
        assert_eq!(
            grid_style(&source, (None, None)).display,
            CssDisplay::InlineTable
        );
    }
}
