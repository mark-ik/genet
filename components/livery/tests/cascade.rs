use livery::PropertyValue;
use livery::cascade::{
    CascadeLayer, MatchedDeclaration, Origin, Specificity, cascade, parse_declaration_block,
};
use livery::values::{
    Color, FontWeight, Length, LengthPercentage, Margin, Size, TransitionProperty,
};

fn matched(
    css: &str,
    origin: Origin,
    layer: CascadeLayer,
    specificity: u32,
    source_order: u64,
) -> MatchedDeclaration {
    let mut block = parse_declaration_block(css);
    assert!(block.errors.is_empty(), "{css}: {:?}", block.errors);
    assert_eq!(block.declarations.len(), 1, "{css}");
    MatchedDeclaration {
        declaration: block.declarations.remove(0),
        origin,
        layer,
        specificity: Specificity(specificity),
        source_order,
    }
}

#[test]
fn declaration_parser_expands_the_lane_shorthands_and_recovers() {
    let block = parse_declaration_block(
        "margin: 1px 2px 3px 4px !important;\
         border: 1px solid #abc;\
         white-space: pre;\
         width: florp;\
         future-property: yes",
    );

    assert_eq!(block.declarations.len(), 18);
    assert_eq!(block.errors.len(), 2);
    assert!(block.declarations[..4].iter().all(|decl| decl.important));
    assert_eq!(block.declarations[0].property.metadata().name, "margin-top");
    assert_eq!(
        block.declarations[3].property.metadata().name,
        "margin-left"
    );
}

#[test]
fn transition_shorthand_expands_to_the_opacity_clock_controls() {
    let block = parse_declaration_block("transition: opacity 100ms");
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    assert_eq!(block.declarations.len(), 2);
    assert_eq!(
        block.declarations[0].property.metadata().name,
        "transition-property"
    );
    assert_eq!(
        block.declarations[1].property.metadata().name,
        "transition-duration"
    );
}

#[test]
fn transition_shorthand_accepts_the_bounded_background_color_lane() {
    let block = parse_declaration_block("transition: background-color 100ms");
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    assert!(matches!(
        block
            .declarations
            .first()
            .map(|declaration| &declaration.value),
        Some(livery::cascade::DeclaredValue::Value(
            PropertyValue::TransitionProperty(TransitionProperty::BackgroundColor)
        ))
    ));
}

#[test]
fn transition_shorthand_accepts_the_bounded_border_top_color_lane() {
    let block = parse_declaration_block("transition: border-top-color 100ms");
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    assert!(matches!(
        block
            .declarations
            .first()
            .map(|declaration| &declaration.value),
        Some(livery::cascade::DeclaredValue::Value(
            PropertyValue::TransitionProperty(TransitionProperty::BorderTopColor)
        ))
    ));
}

#[test]
fn transition_shorthand_merges_the_bounded_two_property_list() {
    let block = parse_declaration_block("transition: opacity 100ms, background-color 100ms");
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    assert!(matches!(
        block
            .declarations
            .first()
            .map(|declaration| &declaration.value),
        Some(livery::cascade::DeclaredValue::Value(
            PropertyValue::TransitionProperty(TransitionProperty::OpacityAndBackgroundColor)
        ))
    ));
}

#[test]
fn transition_shorthand_merges_the_bounded_three_property_list() {
    let block =
        parse_declaration_block("transition: color 100ms, opacity 100ms, background-color 100ms");
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    assert!(matches!(
        block
            .declarations
            .first()
            .map(|declaration| &declaration.value),
        Some(livery::cascade::DeclaredValue::Value(
            PropertyValue::TransitionProperty(
                TransitionProperty::OpacityAndBackgroundColorAndColor
            )
        ))
    ));
}

#[test]
fn origin_importance_specificity_and_source_order_follow_the_cascade() {
    let declarations = vec![
        matched(
            "color: #111111",
            Origin::User,
            CascadeLayer::Unlayered,
            100,
            0,
        ),
        matched(
            "color: #222222",
            Origin::Author,
            CascadeLayer::Unlayered,
            1,
            1,
        ),
        matched(
            "font-weight: 400",
            Origin::Author,
            CascadeLayer::Unlayered,
            10,
            2,
        ),
        matched(
            "font-weight: 700",
            Origin::Author,
            CascadeLayer::Unlayered,
            10,
            3,
        ),
        matched(
            "background-color: #333333 !important",
            Origin::Author,
            CascadeLayer::Unlayered,
            1000,
            4,
        ),
        matched(
            "background-color: #444444 !important",
            Origin::User,
            CascadeLayer::Unlayered,
            1,
            5,
        ),
    ];

    let values = cascade(None, declarations);
    assert_eq!(values.color, "#222222".parse::<Color>().unwrap());
    assert_eq!(values.font_weight, FontWeight::Number(700));
    assert_eq!(values.background_color, "#444444".parse::<Color>().unwrap());
}

#[test]
fn cascade_layers_reverse_for_important_declarations() {
    let values = cascade(
        None,
        vec![
            matched(
                "color: #111111",
                Origin::Author,
                CascadeLayer::Layer(0),
                1,
                0,
            ),
            matched(
                "color: #222222",
                Origin::Author,
                CascadeLayer::Layer(1),
                1,
                1,
            ),
            matched(
                "color: #333333",
                Origin::Author,
                CascadeLayer::Unlayered,
                1,
                2,
            ),
            matched(
                "background-color: #444444 !important",
                Origin::Author,
                CascadeLayer::Unlayered,
                1,
                3,
            ),
            matched(
                "background-color: #555555 !important",
                Origin::Author,
                CascadeLayer::Layer(1),
                1,
                4,
            ),
            matched(
                "background-color: #666666 !important",
                Origin::Author,
                CascadeLayer::Layer(0),
                1,
                5,
            ),
        ],
    );

    assert_eq!(values.color, "#333333".parse::<Color>().unwrap());
    assert_eq!(values.background_color, "#666666".parse::<Color>().unwrap());
}

#[test]
fn inheritance_and_css_wide_keywords_are_property_aware() {
    let parent = livery::ComputedValues {
        color: "#3568b8".parse().unwrap(),
        width: Size::Value(LengthPercentage::Length(Length::rem(42.0))),
        margin_left: Margin::Value(LengthPercentage::Length(Length::px(12.0))),
        ..Default::default()
    };
    let values = cascade(
        Some(&parent),
        vec![
            matched(
                "color: unset",
                Origin::Author,
                CascadeLayer::Unlayered,
                1,
                0,
            ),
            matched(
                "width: inherit",
                Origin::Author,
                CascadeLayer::Unlayered,
                1,
                1,
            ),
            matched(
                "margin-left: unset",
                Origin::Author,
                CascadeLayer::Unlayered,
                1,
                2,
            ),
        ],
    );

    assert_eq!(values.color, parent.color);
    assert_eq!(values.width, parent.width);
    assert_eq!(values.margin_left, Margin::Value(LengthPercentage::ZERO));
}
