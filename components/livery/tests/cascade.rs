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
fn directional_border_shorthands_expand_to_their_three_longhands() {
    for (shorthand, expected) in [
        (
            "border-left: 100px solid black",
            [
                "border-left-color",
                "border-left-style",
                "border-left-width",
            ],
        ),
        (
            "border-right: 100px solid black",
            [
                "border-right-color",
                "border-right-style",
                "border-right-width",
            ],
        ),
    ] {
        let block = parse_declaration_block(shorthand);
        assert!(block.errors.is_empty(), "{shorthand}: {:?}", block.errors);
        assert_eq!(
            block
                .declarations
                .iter()
                .map(|declaration| declaration.property.metadata().name)
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn background_color_shorthand_accepts_the_bounded_color_form() {
    let block = parse_declaration_block("background: black");
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    assert_eq!(block.declarations.len(), 1);
    assert_eq!(
        block.declarations[0].property.metadata().name,
        "background-color"
    );
    assert!(matches!(
        &block.declarations[0].value,
        livery::cascade::DeclaredValue::Value(PropertyValue::Color(Color::Rgba {
            red: 0,
            green: 0,
            blue: 0,
            alpha: 255,
        }))
    ));
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
fn transition_shorthand_accepts_the_bounded_border_bottom_color_lane() {
    let block = parse_declaration_block("transition: border-bottom-color 100ms");
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    assert!(matches!(
        block
            .declarations
            .first()
            .map(|declaration| &declaration.value),
        Some(livery::cascade::DeclaredValue::Value(
            PropertyValue::TransitionProperty(TransitionProperty::BorderBottomColor)
        ))
    ));
}

#[test]
fn transition_shorthand_accepts_the_bounded_background_position_lane() {
    let block = parse_declaration_block("transition: background-position 100ms");
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    assert!(matches!(
        block
            .declarations
            .first()
            .map(|declaration| &declaration.value),
        Some(livery::cascade::DeclaredValue::Value(
            PropertyValue::TransitionProperty(TransitionProperty::BackgroundPosition)
        ))
    ));
}

#[test]
fn transition_shorthand_accepts_the_bounded_box_shadow_lane() {
    let block = parse_declaration_block("transition: box-shadow 100ms");
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    assert!(matches!(
        block
            .declarations
            .first()
            .map(|declaration| &declaration.value),
        Some(livery::cascade::DeclaredValue::Value(
            PropertyValue::TransitionProperty(TransitionProperty::BoxShadow)
        ))
    ));
}

#[test]
fn transition_shorthand_accepts_the_bounded_background_image_lane() {
    let block = parse_declaration_block("transition: background-image 100ms");
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    assert!(matches!(
        block
            .declarations
            .first()
            .map(|declaration| &declaration.value),
        Some(livery::cascade::DeclaredValue::Value(
            PropertyValue::TransitionProperty(TransitionProperty::BackgroundImage)
        ))
    ));
}

#[test]
fn transition_shorthand_accepts_the_bounded_border_style_lane() {
    let block = parse_declaration_block("transition: border-top-style 100ms");
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    assert!(matches!(
        block
            .declarations
            .first()
            .map(|declaration| &declaration.value),
        Some(livery::cascade::DeclaredValue::Value(
            PropertyValue::TransitionProperty(TransitionProperty::BorderTopStyle)
        ))
    ));
}

#[test]
fn transition_shorthand_accepts_the_bounded_background_repeat_lane() {
    let block = parse_declaration_block("transition: background-repeat 100ms");
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    assert!(matches!(
        block
            .declarations
            .first()
            .map(|declaration| &declaration.value),
        Some(livery::cascade::DeclaredValue::Value(
            PropertyValue::TransitionProperty(TransitionProperty::BackgroundRepeat)
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
fn transition_shorthand_preserves_a_side_color_list() {
    let block = parse_declaration_block(
        "transition: opacity 100ms, border-left-color 100ms, border-right-color 100ms",
    );
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    let Some(livery::cascade::DeclaredValue::Value(PropertyValue::TransitionProperty(property))) =
        block
            .declarations
            .first()
            .map(|declaration| &declaration.value)
    else {
        panic!("expected transition-property declaration");
    };
    assert_eq!(
        property.to_string(),
        "opacity, border-left-color, border-right-color"
    );
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
